"""Shared, I/O-free logic of the sync and async clients: request specs, retry policy, error mapping."""

from __future__ import annotations

import math
import random
import re
from dataclasses import dataclass, field
from typing import Any, Callable, Mapping
from urllib.parse import quote

import httpx

from . import models
from .errors import Unavailable, VunlyError, classify

#: The API's Idempotency-Key rule: 8 to 128 characters of A-Za-z0-9_.:- (\Z, not $: `$` also matches before a trailing newline)
_IDEMPOTENCY_ID = re.compile(r"^[A-Za-z0-9_.:-]{8,128}\Z")

#: The whole syntax of a Retry-After the SDK honours (conformance/README.md): ASCII digits, an optional
#: fraction, an optional exponent, an optional plus, or Inf / Infinity. float() alone would also accept "0x..." in some forms,
#: "nan", underscores ("1_0") and non-ASCII digits.
_RETRY_AFTER = re.compile(r"[ \t]*(?:\+?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?|\+?inf(?:inity)?)[ \t]*", re.IGNORECASE)
#: The largest wait reported to the caller; the SDK itself never waits more than ``RetryPolicy.max_retry_after``.
MAX_REPORTED_RETRY_AFTER = 86_400.0


def parse_retry_after(header: str | None) -> float | None:
    """The wait a Retry-After asks for in seconds (capped at one day), or None when it is not a number (an HTTP date,
    garbage: the normal backoff applies). A number too large for a float (``1e999``, hundreds of digits, ``Inf``) is a very
    long wait, NOT an absent header: ``float("1e999")`` is infinity, which is capped and then surfaced, never retried early."""
    if header is None or _RETRY_AFTER.fullmatch(header) is None:
        return None
    text = header.strip(" \t")
    secs = math.inf if "inf" in text.lower() else float(text)
    return min(secs, MAX_REPORTED_RETRY_AFTER)

WRONG_ROUTE_HINT = (
    " (check the API address you configured, VUNLY_API_URL or base_url: it should be the address only, "
    "for example https://host; a trailing /v1 is accepted)"
)


def normalize_base_url(url: str) -> str:
    """The API address without a trailing slash and without a trailing ``/v1``: the SDK adds ``/v1/...`` itself."""
    out = url.strip().rstrip("/")
    if out.lower().endswith("/v1"):
        out = out[:-3]
    return out.rstrip("/")


def check_id(id_: str) -> None:
    """Same refusal the API would give, without a round trip (and without running anything unbilled)."""
    if not isinstance(id_, str) or not _IDEMPOTENCY_ID.match(id_):
        from .errors import InvalidRequest

        raise InvalidRequest(
            "validation_error",
            "id must be 8 to 128 characters of A-Za-z0-9_.:- (it is sent as the Idempotency-Key)",
            400,
            None,
            {"fields": {"id": "8-128 chars of A-Za-z0-9_.:-"}},
        )


@dataclass(frozen=True)
class Spec:
    """One request and how to read its answer."""

    method: str
    path: str
    body: Any = None
    idempotency_key: str | None = None
    parse: Callable[[Mapping[str, Any], "Meta"], Any] = lambda data, meta: models.Raw(data, meta.request_id)
    #: The fields a successful body must carry, as ``{dotted.path: typespec}``. A typespec is "str", "int", "bool" or
    #: "list", with a trailing "?" for nullable-or-absent. The empty default means only "must be a JSON object" (an
    #: unmodelled ``Raw`` result). ``_read`` checks this before ``parse``, so a malformed 2xx is one ``invalid_response``.
    shape: Mapping[str, str] = field(default_factory=dict)
    #: The operation class of conformance/README.md ("R", "K", "N" or "X"); see ``klass``.
    retry_class: str | None = None

    def __post_init__(self) -> None:
        # An empty, "." or ".." segment (an id that was empty or dot-only) would change the route on the wire
        # (GET /v1/customers/ , GET /v1/): refused before any request.
        if any(seg in ("", ".", "..") for seg in self.path.split("?", 1)[0].split("/")[1:]):
            from .errors import InvalidRequest

            raise InvalidRequest(
                "validation_error",
                'an id in the path must not be empty, "." or ".."',
                400,
                None,
                {"fields": {"id": 'not empty, "." or ".."'}},
            )

    @property
    def klass(self) -> str:
        """R reads and the entitlement check; K keyed mutations; N naturally idempotent writes (upserts, deletes); X the rest.

        X (no key, not idempotent: a new token or endpoint every time) is retried only after a 429, which is refused
        before it runs. After a lost answer, a timeout or a 5xx the call may have happened, so the caller decides.
        """
        if self.retry_class is not None:
            return self.retry_class
        if self.method == "GET":
            return "R"
        if self.idempotency_key is not None:
            return "K"
        return "N" if self.method in ("DELETE", "PUT", "PATCH") else "X"


@dataclass(frozen=True)
class Meta:
    status: int
    request_id: str | None
    replayed: bool
    #: ``Vunly-Plan-Overage: true`` on the response (a paid plan past its included monthly volume).
    plan_overage: bool = False


@dataclass(frozen=True)
class RetryPolicy:
    """Retry only what is safe. A write carries its id as Idempotency-Key, so the API applies it at most once."""

    max_retries: int = 2
    backoff_base: float = 0.25
    backoff_max: float = 8.0
    #: A Retry-After longer than this is not waited for: the call raises Unavailable instead of hanging.
    max_retry_after: float = 30.0

    def retryable_status(self, status: int) -> bool:
        return status == 429 or status in (500, 502, 503, 504)

    def delay(self, attempt: int, retry_after: str | None, rng: Callable[[], float] = random.random) -> float | None:
        """Seconds to wait before attempt ``attempt`` (1 = first retry), or None to give up (Retry-After too long)."""
        wait = parse_retry_after(retry_after)
        if wait is not None and wait > 0:  # 0 is "no directive": the normal backoff applies
            return wait if wait <= self.max_retry_after else None
        capped = min(self.backoff_max, self.backoff_base * (2 ** (attempt - 1)))
        return capped * (0.5 + rng() / 2)  # jitter in [50%, 100%]


def headers_for(api_key: str, idempotency_key: str | None, user_agent: str) -> dict[str, str]:
    h = {"authorization": f"Bearer {api_key}", "accept": "application/json", "user-agent": user_agent}
    if idempotency_key is not None:
        h["idempotency-key"] = idempotency_key
    return h


#: The code the API would send for a status when the error body is missing (a proxy, an empty 5xx), so a bodiless
#: error is not reported as "internal_error" for every status.
def _fallback_code(status: int) -> str:
    if status in (401, 403):
        return "unauthenticated"
    if status == 402:
        return "insufficient_credits"
    if status == 429:
        return "rate_limited"
    if status >= 500:
        return "internal_error"
    return "validation_error"


def error_from(response: httpx.Response) -> VunlyError:
    status = response.status_code
    if 300 <= status < 400:
        # httpx does not follow redirects here, and the key must not travel to another address. A 3xx is a wrong base
        # URL, not a bad request: Unavailable, never retried; neither the Location nor the body is used.
        return Unavailable(
            "unexpected_redirect",
            f"the API answered {status} (a redirect); check the base URL, redirects are never followed",
            status,
            response.headers.get("request-id"),
            None,
        )
    try:
        data = response.json()
        err = data.get("error", {}) if isinstance(data, dict) else {}
    except ValueError:
        err = {}
    code = str(err.get("code") or _fallback_code(status))
    message = str(err.get("message") or f"request failed with status {status}")
    if status == 404 and "no such route" in message.lower():
        message += WRONG_ROUTE_HINT
    details = err.get("details")
    error = classify(status, code, details if isinstance(details, dict) else None)(code, message, status, err.get("requestId") or response.headers.get("request-id"), details)
    if isinstance(error, Unavailable):
        error.retry_after_seconds = parse_retry_after(response.headers.get("retry-after"))
    return error


def shape_problem(data: Any, shape: Mapping[str, str]) -> str | None:
    """None when ``data`` is a JSON object carrying every field of ``shape`` with the right type; else what is wrong.

    One place, strict: a string where a number is promised, a null where a value is promised, or a missing field is a
    malformed response, not a value the caller has to re-check. This is what keeps a bad 2xx from becoming, say,
    ``Entitlement(allowed="false")`` (truthy) and letting billable work run.
    """
    if not isinstance(data, dict):
        return "the body is not a JSON object"
    for path, spec in shape.items():
        nullable = spec.endswith("?")
        typ = spec[:-1] if nullable else spec
        value: Any = data
        for part in path.split("."):
            value = value.get(part) if isinstance(value, dict) else None
        if value is None:
            if nullable:
                continue
            return f"field {path!r} is missing"
        good = {
            "str": isinstance(value, str),
            "int": isinstance(value, int) and not isinstance(value, bool),
            "bool": isinstance(value, bool),
            "list": isinstance(value, list),
        }[typ]
        if not good:
            return f"field {path!r} is not {typ}"
    return None


def is_plan_stop(response: httpx.Response) -> bool:
    """A 429 that is the plan's volume stop, not a rate limit: only an OLDER server answers it that way (it is 402
    ``plan_limit_reached`` now, which is never retried). ``Retry-After`` is then the time to next month. Never retried."""
    if response.status_code != 429:
        return False
    try:
        data = response.json()
    except ValueError:
        return False
    err = data.get("error") if isinstance(data, dict) else None
    details = err.get("details") if isinstance(err, dict) else None
    return isinstance(details, dict) and details.get("reason") == "plan_included_volume"


def may_retry(spec: Spec, status: int | None, plan_stop: bool = False) -> bool:
    """Whether the retry rules allow another attempt of ``spec``. ``status`` is None after a lost answer or a timeout."""
    if plan_stop:
        return False
    if spec.klass == "X":
        return status == 429  # refused before it ran; anything else may have been applied
    return True


def network_error(exc: Exception) -> Unavailable:
    # The message names the kind of failure, never the key or the headers.
    return Unavailable("unavailable", f"network failure: {type(exc).__name__}", 0, None, None)


# ---- request specs: one place that knows paths and bodies ---------------------------------------------------------------
def _quote(s: str) -> str:
    return quote(s, safe="")


# Field shapes of the modelled bodies, so a spec reads once and `shape_problem` checks once.
_ENTRY = {"id": "str", "customerId": "str", "kind": "str", "credits": "int", "createdAt": "str"}


def customers_upsert(id_: str, name: str | None) -> Spec:
    body: dict[str, Any] = {"externalId": id_}
    if name is not None:
        body["name"] = name
    return Spec(
        "POST",
        "/v1/customers",
        body,
        parse=lambda d, m: models.Customer(d["id"], d["externalId"], d.get("name"), d["createdAt"], m.request_id),
        shape={"id": "str", "externalId": "str", "name": "str?", "createdAt": "str"},
        retry_class="N",  # an upsert: asking twice leaves the same customer
    )


_CUSTOMER_SHAPE = {"id": "str", "externalId": "str", "name": "str?", "createdAt": "str"}


def _customer(d: Mapping[str, Any], request_id: str | None) -> models.Customer:
    return models.Customer(d["id"], d["externalId"], d.get("name"), d["createdAt"], request_id)


def customers_get(customer: str) -> Spec:
    return Spec(
        "GET",
        f"/v1/customers/{_quote(customer)}",
        parse=lambda d, m: _customer(d, m.request_id),
        shape=_CUSTOMER_SHAPE,
    )


def _page_query(limit: int | None, cursor: str | None) -> str:
    q = {k: v for k, v in (("limit", limit), ("cursor", cursor)) if v is not None}
    return ("?" + "&".join(f"{k}={_quote(str(v))}" for k, v in q.items())) if q else ""


def customers_list(limit: int | None, cursor: str | None) -> Spec:
    return Spec(
        "GET",
        f"/v1/customers{_page_query(limit, cursor)}",
        parse=lambda d, m: models.Page([_customer(x, m.request_id) for x in d["data"]], d.get("nextCursor"), m.request_id),
        shape={"data": "list", "nextCursor": "str?"},
    )


def credits_ledger(customer: str, limit: int | None, cursor: str | None) -> Spec:
    return Spec(
        "GET",
        f"/v1/customers/{_quote(customer)}/ledger{_page_query(limit, cursor)}",
        parse=lambda d, m: models.Page([models.ledger_entry(x, m.request_id) for x in d["data"]], d.get("nextCursor"), m.request_id),
        shape={"data": "list", "nextCursor": "str?"},
    )


def plan_usage() -> Spec:
    return Spec(
        "GET",
        "/v1/plan-usage",
        parse=lambda d, m: models.PlanUsage(d["plan"], d["period"], d["usage"], d["unconfirmedLimits"], m.request_id),
        shape={"plan.key": "str", "period.start": "str", "usage.events": "int", "unconfirmedLimits.confirmed": "bool"},
    )


def credits_grant(id_: str, customer: str, credits: int, reason: str | None) -> Spec:
    check_id(id_)
    body: dict[str, Any] = {"credits": credits}
    if reason is not None:
        body["reason"] = reason
    return Spec(
        "POST",
        f"/v1/customers/{_quote(customer)}/credit-grants",
        body,
        id_,
        parse=lambda d, m: models.ledger_entry(d, m.request_id),
        shape=_ENTRY,
    )


def credits_balance(customer: str) -> Spec:
    return Spec(
        "GET",
        f"/v1/customers/{_quote(customer)}/balance",
        parse=lambda d, m: models.Balance(d["credits"], d["consumed"], d["granted"], m.request_id),
        shape={"credits": "int", "consumed": "int", "granted": "int"},
    )


def credits_usage(customer: str) -> Spec:
    # Not modelled yet: the whole decoded object comes back as Raw, but only when it has the documented shape.
    return Spec(
        "GET",
        f"/v1/customers/{_quote(customer)}/usage",
        shape={"customerId": "str", "balance.credits": "int", "balance.granted": "int", "balance.consumed": "int", "meters": "list"},
    )


def usage_record(id_: str, customer: str, meter: str, quantity: int) -> Spec:
    check_id(id_)
    return Spec(
        "POST",
        "/v1/usage-events",
        {"customerId": customer, "meter": meter, "quantity": quantity},
        id_,
        parse=lambda d, m: models.UsageResult(
            models.ledger_entry(d["event"], m.request_id),
            d["balance"],
            m.replayed,
            m.request_id,
            plan_overage=m.plan_overage and not m.replayed,
        ),
        shape={"balance": "int", **{f"event.{k}": v for k, v in _ENTRY.items()}},
    )


BATCH_MAX = 100
_CUSTOMER_ID = re.compile(r"^cus_[A-Za-z0-9]+\Z")
_METER_KEY = re.compile(r"^[a-z0-9][a-z0-9_.-]*\Z")


def _invalid(field_: str, why: str, message: str) -> VunlyError:
    from .errors import InvalidRequest

    return InvalidRequest("validation_error", message, 400, None, {"fields": {field_: why}})


def wire_event(e: "models.BatchEvent | Mapping[str, Any]") -> dict[str, Any]:
    """The wire form of one batch event, validated as the route validates it, so a bad event never costs a round trip.

    ``occurred_at`` left out is stamped now (UTC): the stamp is part of the returned dict, so a resend carries the same one.
    """
    from datetime import datetime, timezone

    if isinstance(e, Mapping):
        e = models.BatchEvent(
            e.get("id"), e.get("customer"), e.get("meter"), e.get("quantity"), e.get("occurred_at")  # type: ignore[arg-type]
        )
    check_id(e.id)
    if not isinstance(e.customer, str) or not _CUSTOMER_ID.match(e.customer) or len(e.customer) > 128:
        raise _invalid(
            "customer",
            "a Vunly cus_ id",
            "the batch route takes the Vunly customer id (cus_..., the id customers.upsert returns), not your external id",
        )
    if not isinstance(e.meter, str) or not _METER_KEY.match(e.meter) or len(e.meter) > 64:
        raise _invalid("meter", "1-64 chars of a-z0-9_.-", "meter is not a valid meter key")
    if isinstance(e.quantity, bool) or not isinstance(e.quantity, int) or not 1 <= e.quantity <= 1_000_000_000:
        raise _invalid("quantity", "an integer from 1 to 1000000000", "quantity must be an integer from 1 to 1000000000")
    at = e.occurred_at
    if at is None:
        at = datetime.now(timezone.utc)
    elif isinstance(at, str):
        try:
            at = datetime.fromisoformat(at.replace("Z", "+00:00"))
        except ValueError:
            raise _invalid("occurred_at", "a date", "occurred_at is not a valid date") from None
    if not isinstance(at, datetime):
        raise _invalid("occurred_at", "a date", "occurred_at is not a valid date")
    if at.tzinfo is None:
        at = at.replace(tzinfo=timezone.utc)  # a naive time is read as UTC, said here so it is not a surprise
    stamp = at.astimezone(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")
    return {"id": e.id, "customerId": e.customer, "meter": e.meter, "quantity": e.quantity, "occurredAt": stamp}


def usage_record_batch(wire: list[dict[str, Any]]) -> Spec:
    """``POST /v1/usage-events:batch`` for events already in wire form (``wire_event``). Retried like a keyed call: every event
    carries its own id, so asking again after a lost answer records nothing twice."""
    if not 1 <= len(wire) <= BATCH_MAX:
        raise _invalid("events", "1 to 100 events", "a batch holds 1 to 100 events")
    ids = [w["id"] for w in wire]
    if len(set(ids)) != len(ids):
        raise _invalid("events", "ids must be distinct", "an event id appears twice in the batch")

    def parse(d: Mapping[str, Any], m: "Meta") -> models.BatchResponse:
        raw = d["results"]
        if len(raw) != len(ids) or any(r["id"] != i or not isinstance(r["status"], str) for r, i in zip(raw, ids)):
            raise ValueError("the results do not match the events sent")
        limit = d.get("planLimit")
        return models.BatchResponse(
            [models.BatchEventResult(r["id"], r["status"], r.get("balance")) for r in raw],
            models.PlanLimitInfo(
                limit["reason"], limit["plan"], limit["includedMonthlyVolume"], limit["used"]
            )
            if limit
            else None,
            m.plan_overage,
            m.request_id,
        )

    return Spec("POST", "/v1/usage-events:batch", {"events": wire}, parse=parse, shape={"results": "list"}, retry_class="K")


def batch_route_off(e: VunlyError) -> VunlyError | None:
    """The clearer error for a 404 not_found from the batch route (it is off unless the deployment turned it on), else None."""
    from .errors import InvalidRequest

    if isinstance(e, InvalidRequest) and e.status == 404 and e.code == "not_found":
        return InvalidRequest(
            e.code,
            "the batch route is not available for this project (it is off by default and, today, staging only): use usage.record",
            e.status,
            e.request_id,
            e.details,
        )
    return None


class BatchEntry:
    """One event waiting in a batcher, with how many times it has been sent."""

    __slots__ = ("wire", "attempts", "waiter")

    def __init__(self, wire: dict[str, Any], waiter: Any) -> None:
        self.wire = wire
        self.attempts = 0
        self.waiter = waiter  # whatever the batcher resolves (a Future, an asyncio.Future)


class BatchBuffer:
    """The I/O-free state of a batcher: a bounded queue, the attempts of each event and what to do with an answer.

    Final outcomes are ``accepted``, ``duplicate``, ``insufficient_credits``, ``too_late``, ``meter_not_found``,
    ``plan_limit_reached`` and ``error`` (never resent), ``unavailable`` after ``max_attempts`` sends (not recorded, safe to
    send again later with the same id), or an exception for a failure a resend cannot fix (a bad key, a frozen project, a route
    that is off, a malformed answer).
    """

    def __init__(self, max_batch_size: int, max_pending: int, max_attempts: int, policy: RetryPolicy) -> None:
        if not isinstance(max_batch_size, int) or not 1 <= max_batch_size <= BATCH_MAX:
            raise ValueError("max_batch_size must be an integer from 1 to 100")
        if not isinstance(max_pending, int) or max_pending < 1:
            raise ValueError("max_pending must be at least 1")
        if not isinstance(max_attempts, int) or max_attempts < 1:
            raise ValueError("max_attempts must be at least 1")
        self.size, self.max_pending, self.max_attempts, self.policy = max_batch_size, max_pending, max_attempts, policy
        self.queue: list[BatchEntry] = []
        self.in_flight = 0
        self.by_id: dict[str, BatchEntry] = {}

    @property
    def pending(self) -> int:
        return len(self.queue) + self.in_flight

    def add(self, wire: dict[str, Any], waiter_factory: Callable[[], Any]) -> tuple[BatchEntry, bool]:
        """Queues ``wire``. Returns ``(entry, new)``: the same event added twice is one entry (``new`` False)."""
        known = self.by_id.get(wire["id"])
        if known is not None:
            same = all(known.wire[k] == wire[k] for k in ("customerId", "meter", "quantity"))
            if not same:
                from .errors import InvalidRequest

                raise InvalidRequest(
                    "idempotency_conflict", "this id is already queued with a different customer, meter or quantity", 409
                )
            return known, False
        if self.pending >= self.max_pending:
            raise Unavailable("unavailable", f"the batcher already holds {self.max_pending} events: slow down or flush", 0)
        entry = BatchEntry(wire, waiter_factory())
        self.by_id[wire["id"]] = entry
        self.queue.append(entry)
        return entry, True

    def take(self) -> list[BatchEntry]:
        taken, self.queue = self.queue[: self.size], self.queue[self.size :]
        self.in_flight += len(taken)
        return taken

    def finish(self, taken: list[BatchEntry]) -> None:
        self.in_flight -= len(taken)

    def _settle(self, entry: BatchEntry) -> None:
        self.by_id.pop(entry.wire["id"], None)

    def apply(
        self, taken: list[BatchEntry], response: "models.BatchResponse | None", error: Exception | None
    ) -> tuple[list[tuple[BatchEntry, Any, bool]], list[BatchEntry], float | None]:
        """Digests one answer. Returns ``(done, again, retry_after)``: ``done`` is ``(entry, value, is_error)``, ``again`` the
        entries to send again, ``retry_after`` the server's wait in seconds when the whole request said so."""
        done: list[tuple[BatchEntry, Any, bool]] = []
        again: list[BatchEntry] = []
        retry_after: float | None = None

        def unavailable_or_again(entry: BatchEntry) -> None:
            entry.attempts += 1
            if entry.attempts >= self.max_attempts:
                self._settle(entry)
                done.append((entry, models.BatchEventResult(entry.wire["id"], "unavailable"), False))
            else:
                again.append(entry)

        if error is None and response is not None:
            for entry, r in zip(taken, response.results):
                if r.status == "unavailable":
                    unavailable_or_again(entry)
                else:
                    self._settle(entry)
                    done.append((entry, r, False))
        elif isinstance(error, Unavailable) and error.code not in ("invalid_response", "unexpected_redirect"):
            # the whole request could not be answered (a 503 that survived the client's retries, a network failure): nothing was
            # confirmed, so every event is sent again with its id, and counts the attempt
            retry_after = error.retry_after_seconds
            for entry in taken:
                unavailable_or_again(entry)
        else:
            for entry in taken:
                self._settle(entry)
                done.append((entry, error, True))
        return done, again, retry_after

    def wait_for(self, again: list[BatchEntry], retry_after: float | None) -> float | None:
        """Seconds to wait before sending ``again``, or None when the server asked for longer than the SDK waits."""
        attempt = max(e.attempts for e in again)
        return self.policy.delay(attempt, None if retry_after is None else repr(retry_after))

    def give_up(self, again: list[BatchEntry]) -> list[tuple[BatchEntry, Any, bool]]:
        out: list[tuple[BatchEntry, Any, bool]] = []
        for entry in again:
            self._settle(entry)
            out.append((entry, models.BatchEventResult(entry.wire["id"], "unavailable"), False))
        return out

    def requeue(self, again: list[BatchEntry]) -> None:
        self.queue = [*again, *self.queue]


def entitlements_check(customer: str, feature: str, estimate: Mapping[str, Any] | None) -> Spec:
    body: dict[str, Any] = {"customerId": customer, "feature": feature}
    if estimate is not None:
        body["estimate"] = {"meter": estimate["meter"], "quantity": estimate["quantity"]}
    return Spec(
        "POST",
        "/v1/entitlements/check",
        body,
        parse=lambda d, m: models.Entitlement(d["allowed"], d["reason"], d["remaining"], d["required"], d.get("plan"), m.request_id),
        shape={"allowed": "bool", "reason": "str", "remaining": "int", "required": "int", "plan": "str?"},
        retry_class="R",  # read-only and advisory
    )


def plans_upsert_meter(key: str, units_per_credit: int) -> Spec:
    return Spec("POST", "/v1/meters", {"key": key, "unitsPerCredit": units_per_credit}, retry_class="N")


def plans_upsert(key: str, name: str, included_credits: int, meter: str, features: list[str]) -> Spec:
    return Spec(
        "POST",
        "/v1/plans",
        {"key": key, "name": name, "includedCredits": included_credits, "meter": meter, "features": features},
        retry_class="N",
    )


def plans_subscribe(customer: str, plan: str) -> Spec:
    # the same plan again is a 200 with the existing subscription: asking twice leaves one
    return Spec("POST", f"/v1/customers/{_quote(customer)}/subscription", {"plan": plan}, retry_class="N")


def plans_cancel(customer: str) -> Spec:
    return Spec("DELETE", f"/v1/customers/{_quote(customer)}/subscription")


def customer_tokens_create(customer: str, ttl_seconds: int | None) -> Spec:
    body: dict[str, Any] = {"customerId": customer}
    if ttl_seconds is not None:
        body["ttlSeconds"] = ttl_seconds
    return Spec(
        "POST",
        "/v1/customer-tokens",
        body,
        parse=lambda d, m: models.CustomerToken(d["id"], d["token"], d["customerId"], d["expiresAt"], m.request_id),
        shape={"id": "str", "token": "str", "customerId": "str", "expiresAt": "str"},
    )


def customer_tokens_revoke(token_id: str) -> Spec:
    return Spec("DELETE", f"/v1/customer-tokens/{_quote(token_id)}")
