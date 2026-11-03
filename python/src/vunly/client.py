"""``Client`` (sync) and ``AsyncClient`` (asyncio): the same calls, the same results, the same errors."""

from __future__ import annotations

import asyncio
import time
from typing import TYPE_CHECKING, Any, Awaitable, Callable, Mapping, Sequence

import httpx

from . import _core
from ._core import Meta, RetryPolicy, Spec
from ._version import __version__
from .errors import Unavailable, VunlyError
from .models import (
    Balance,
    BatchEvent,
    BatchResponse,
    Customer,
    CustomerToken,
    Entitlement,
    LedgerEntry,
    Page,
    PlanUsage,
    Raw,
    UsageResult,
)

if TYPE_CHECKING:
    from .batcher import AsyncUsageBatcher, UsageBatcher

__all__ = ["Client", "AsyncClient"]

_UA = f"vunly-python/{__version__}"


def _meta(response: httpx.Response) -> Meta:
    return Meta(
        response.status_code,
        response.headers.get("request-id"),
        response.headers.get("idempotent-replayed") == "true",
        response.headers.get("vunly-plan-overage") == "true",
    )


def _malformed(response: httpx.Response, request_id: str | None, what: str) -> Unavailable:
    # A 2xx the SDK cannot trust: fail closed with the SDK's own error, never a bare KeyError or a value of the wrong
    # type handed to the caller. The call may have been applied, so retrying later with the SAME id is safe.
    return Unavailable(
        "invalid_response",
        f"the API answered {response.status_code} but the response is malformed ({what})",
        response.status_code,
        request_id,
        None,
    )


MAX_RESPONSE_BYTES = 16 * 1024 * 1024
"""The most of one response the SDK will hold in memory (as the Go and .NET SDKs): the API's largest page is far below it."""


def _too_large(streamed: httpx.Response) -> httpx.Response:
    """The body went over ``MAX_RESPONSE_BYTES`` and reading stopped. A 2xx is the SDK's own error (``invalid_response``,
    not retried: the status is not retryable, and the call may have been applied, so retrying later with the SAME id is safe);
    an error status is classified by its status, as an error with no body is."""
    if streamed.is_success:
        raise Unavailable(
            "invalid_response",
            f"the API answered {streamed.status_code} but the response is larger than {MAX_RESPONSE_BYTES // (1 << 20)} MB",
            streamed.status_code,
            streamed.headers.get("request-id"),
            None,
        )
    return _whole(streamed, b"")


def _whole(streamed: httpx.Response, content: bytes) -> httpx.Response:
    """A fully read copy of a streamed response. The chunks are already decoded, so the headers that describe the wire
    encoding are dropped, or httpx would try to decode the body a second time."""
    wire = {"content-encoding", "content-length", "transfer-encoding"}
    headers = [(k, v) for k, v in streamed.headers.multi_items() if k.lower() not in wire]
    return httpx.Response(streamed.status_code, headers=headers, content=content, request=streamed.request)


def _read(spec: Spec, response: httpx.Response) -> Any:
    meta = _meta(response)
    # One validation point for every method: decode, check the shape, then parse. A DELETE/204 has no body ({}).
    if response.content:
        try:
            data = response.json()
        except ValueError:
            raise _malformed(response, meta.request_id, "the body is not JSON") from None
    else:
        data = {}
    problem = _core.shape_problem(data, spec.shape)
    if problem is not None:
        raise _malformed(response, meta.request_id, problem) from None
    try:
        return spec.parse(data, meta)
    except (KeyError, TypeError, AttributeError, IndexError, ValueError):
        raise _malformed(response, meta.request_id, "unexpected shape") from None


def _setup(
    api_key: str, base_url: str, timeout: float, max_retries: int, retry: RetryPolicy | None
) -> tuple[str, str, RetryPolicy]:
    if not api_key:
        raise ValueError("Vunly: api_key is empty (read it from your environment)")
    if not base_url:
        raise ValueError("Vunly: base_url is empty (the API address from the setup step)")
    policy = retry or RetryPolicy(max_retries=max_retries)
    return api_key, _core.normalize_base_url(base_url), policy


class Client:
    """Synchronous client.

    >>> vunly = Client(api_key=os.environ["VUNLY_API_KEY"], base_url=os.environ["VUNLY_API_URL"])
    >>> vunly.usage.record(id="req_2026_0001", customer="cus_demo_01", meter="tokens", quantity=1050)

    Retries only what is safe, with the SAME id: a network failure or timeout, 429 (after ``Retry-After``) and
    500/502/503/504 (exponential backoff with jitter). 400/401/402/403/404/409 are never retried.
    """

    def __init__(
        self,
        api_key: str,
        base_url: str,
        *,
        timeout: float = 10.0,
        max_retries: int = 2,
        retry: RetryPolicy | None = None,
        transport: httpx.BaseTransport | None = None,
        sleep: Callable[[float], None] = time.sleep,
    ) -> None:
        self._key, self._base, self._retry = _setup(api_key, base_url, timeout, max_retries, retry)
        self._http = httpx.Client(base_url=self._base, timeout=timeout, transport=transport)
        self._timeout = timeout
        self._sleep = sleep
        self.customers = _Customers(self)
        self.credits = _Credits(self)
        self.usage = _Usage(self)
        self.entitlements = _Entitlements(self)
        self.plans = _Plans(self)
        self.customer_tokens = _CustomerTokens(self)

    def plan_usage(self) -> PlanUsage:
        """The project's plan and this month's usage. ``unconfirmed_limits`` is not a guarantee (``confirmed`` is false)."""
        return self._do(_core.plan_usage())

    def close(self) -> None:
        self._http.close()

    def __enter__(self) -> "Client":
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    def __repr__(self) -> str:  # never print the key
        return f"Client(base_url={self._base!r}, api_key=<hidden>)"

    def _attempt(self, spec: Spec) -> httpx.Response:
        """One attempt, bounded as a WHOLE by ``timeout``.

        httpx applies ``timeout`` to each phase (connect, read, write, pool) separately, so a server that answers at once
        and then drips its body one byte at a time never trips the read timeout and would hold a plain request open for as
        long as it keeps dripping. The body is streamed and the deadline is checked as each chunk arrives, so an attempt
        ends within ``timeout`` plus at most one gap between two chunks. ``ReadTimeout`` is an ``httpx.HTTPError``: it is
        retried like any lost answer and ends as ``Unavailable``.
        """
        deadline = time.monotonic() + self._timeout
        with self._http.stream(
            spec.method,
            spec.path,
            json=spec.body,
            headers=_core.headers_for(self._key, spec.idempotency_key, _UA),
        ) as streamed:
            if time.monotonic() > deadline:
                raise httpx.ReadTimeout("the attempt exceeded its timeout", request=streamed.request)
            chunks: list[bytes] = []
            total = 0
            for chunk in streamed.iter_bytes():  # already decoded (gzip and friends)
                total += len(chunk)
                if total > MAX_RESPONSE_BYTES:
                    return _too_large(streamed)  # leaving the ``with`` closes the connection: the rest is never read
                chunks.append(chunk)
                if time.monotonic() > deadline:
                    raise httpx.ReadTimeout("the attempt exceeded its timeout", request=streamed.request)
            return _whole(streamed, b"".join(chunks))

    def _do(self, spec: Spec) -> Any:
        attempt = 0
        while True:
            attempt += 1
            try:
                response = self._attempt(spec)
            except httpx.HTTPError as exc:
                if attempt <= self._retry.max_retries and _core.may_retry(spec, None):
                    self._sleep(self._retry.delay(attempt, None) or 0.0)
                    continue
                raise _core.network_error(exc) from None
            if response.is_success:
                return _read(spec, response)
            if (
                self._retry.retryable_status(response.status_code)
                and attempt <= self._retry.max_retries
                and _core.may_retry(spec, response.status_code, _core.is_plan_stop(response))
            ):
                wait = self._retry.delay(attempt, response.headers.get("retry-after"))
                if wait is not None:
                    self._sleep(wait)
                    continue
            raise _core.error_from(response)


class AsyncClient:
    """Asyncio client: ``await`` every call. Same retries, same errors, same results as ``Client``."""

    def __init__(
        self,
        api_key: str,
        base_url: str,
        *,
        timeout: float = 10.0,
        max_retries: int = 2,
        retry: RetryPolicy | None = None,
        transport: httpx.AsyncBaseTransport | None = None,
        sleep: Callable[[float], Awaitable[None]] = asyncio.sleep,
    ) -> None:
        self._key, self._base, self._retry = _setup(api_key, base_url, timeout, max_retries, retry)
        self._http = httpx.AsyncClient(base_url=self._base, timeout=timeout, transport=transport)
        self._timeout = timeout
        self._sleep = sleep
        self.customers = _ACustomers(self)
        self.credits = _ACredits(self)
        self.usage = _AUsage(self)
        self.entitlements = _AEntitlements(self)
        self.plans = _APlans(self)
        self.customer_tokens = _ACustomerTokens(self)

    async def plan_usage(self) -> PlanUsage:
        return await self._do(_core.plan_usage())

    async def aclose(self) -> None:
        await self._http.aclose()

    async def __aenter__(self) -> "AsyncClient":
        return self

    async def __aexit__(self, *exc: object) -> None:
        await self.aclose()

    def __repr__(self) -> str:
        return f"AsyncClient(base_url={self._base!r}, api_key=<hidden>)"

    async def _attempt(self, spec: Spec) -> httpx.Response:
        """One attempt, bounded as a WHOLE by ``timeout`` (see ``Client._attempt``): cancelled at the deadline."""

        async def read() -> httpx.Response:
            async with self._http.stream(
                spec.method,
                spec.path,
                json=spec.body,
                headers=_core.headers_for(self._key, spec.idempotency_key, _UA),
            ) as streamed:
                chunks: list[bytes] = []
                total = 0
                async for chunk in streamed.aiter_bytes():  # already decoded
                    total += len(chunk)
                    if total > MAX_RESPONSE_BYTES:
                        return _too_large(streamed)  # leaving the ``async with`` closes the connection
                    chunks.append(chunk)
                return _whole(streamed, b"".join(chunks))

        try:
            return await asyncio.wait_for(read(), self._timeout)
        except (asyncio.TimeoutError, TimeoutError):  # one name from Python 3.11, two before
            raise httpx.ReadTimeout("the attempt exceeded its timeout") from None

    async def _do(self, spec: Spec) -> Any:
        attempt = 0
        while True:
            attempt += 1
            try:
                response = await self._attempt(spec)
            except httpx.HTTPError as exc:
                if attempt <= self._retry.max_retries and _core.may_retry(spec, None):
                    await self._sleep(self._retry.delay(attempt, None) or 0.0)
                    continue
                raise _core.network_error(exc) from None
            if response.is_success:
                return _read(spec, response)
            if (
                self._retry.retryable_status(response.status_code)
                and attempt <= self._retry.max_retries
                and _core.may_retry(spec, response.status_code, _core.is_plan_stop(response))
            ):
                wait = self._retry.delay(attempt, response.headers.get("retry-after"))
                if wait is not None:
                    await self._sleep(wait)
                    continue
            raise _core.error_from(response)


# ---- resources: thin, so the two clients cannot drift apart (paths and bodies live in _core) ---------------------------
class _Customers:
    def __init__(self, c: Client) -> None:
        self._c = c

    def upsert(self, id: str, name: str | None = None) -> Customer:
        """Create the customer or return the existing one for this id (your own id)."""
        return self._c._do(_core.customers_upsert(id, name))

    def get(self, customer: str) -> Customer:
        """One customer, by your own id or Vunly's. ``InvalidRequest`` (``not_found``) when it does not exist."""
        return self._c._do(_core.customers_get(customer))

    def list(self, limit: int | None = None, cursor: str | None = None) -> Page:
        """Newest first, ``limit`` 1 to 100 (default 25). Follow ``next_cursor`` until it is ``None``."""
        return self._c._do(_core.customers_list(limit, cursor))


class _Credits:
    def __init__(self, c: Client) -> None:
        self._c = c

    def grant(self, id: str, customer: str, credits: int, reason: str | None = None) -> LedgerEntry:
        """Sandbox top-up. ``id`` is the Idempotency-Key: the same id grants once."""
        return self._c._do(_core.credits_grant(id, customer, credits, reason))

    def balance(self, customer: str) -> Balance:
        return self._c._do(_core.credits_balance(customer))

    def ledger(self, customer: str, limit: int | None = None, cursor: str | None = None) -> Page:
        """The customer's ledger entries, newest first. Follow ``next_cursor`` until it is ``None``."""
        return self._c._do(_core.credits_ledger(customer, limit, cursor))

    def usage(self, customer: str) -> Raw:
        """Current-period consumption per meter, with plan and balance (``.data`` is the decoded JSON)."""
        return self._c._do(_core.credits_usage(customer))


class _Usage:
    def __init__(self, c: Client) -> None:
        self._c = c

    def record(self, id: str, customer: str, meter: str, quantity: int) -> UsageResult:
        """Record usage once. ``id`` names the unit of work and is the Idempotency-Key: retry with the SAME id.

        Raises ``LimitReached`` (402) when the balance cannot cover it; nothing is debited then.
        """
        return self._c._do(_core.usage_record(id, customer, meter, quantity))

    def record_batch(self, events: Sequence["BatchEvent | Mapping[str, Any]"]) -> BatchResponse:
        """Record 1 to 100 events in ONE request through the batch route and return one result per event, in order.

        A request that is answered is not an error even when some events are not recorded: read each ``status``. Idempotent by each
        event's ``id`` for 24 hours. A whole-batch 503 is retried with the same ids after its ``Retry-After`` and surfaces as
        ``Unavailable`` when it survives the retries. Events with status ``unavailable`` were NOT recorded: send only those again,
        with the same ids (``usage.batcher()`` does it for you). The route is OFF unless the deployment turned it on for the
        project: until then this raises ``InvalidRequest`` (``not_found``, 404). ``customer`` is the Vunly ``cus_...`` id.
        """
        spec = _core.usage_record_batch([_core.wire_event(e) for e in events])
        try:
            return self._c._do(spec)
        except VunlyError as e:
            raise (_core.batch_route_off(e) or e) from None

    def batcher(
        self,
        *,
        max_batch_size: int = 100,
        flush_interval: float = 1.0,
        max_pending: int = 10_000,
        max_attempts: int = 5,
    ) -> "UsageBatcher":
        """A buffer in front of ``record_batch``: ``add`` events as they happen (it returns a ``Future`` of the event's final
        outcome); a background thread sends up to 100 at a time, every ``flush_interval`` seconds, and sends ``unavailable``
        events again with the same ids (waiting at least ``Retry-After``, then backing off, at most ``max_attempts`` sends).
        Call ``close()`` (or use ``with``) before the process ends."""
        from .batcher import UsageBatcher

        return UsageBatcher(
            self._c,
            max_batch_size=max_batch_size,
            flush_interval=flush_interval,
            max_pending=max_pending,
            max_attempts=max_attempts,
        )


class _Entitlements:
    def __init__(self, c: Client) -> None:
        self._c = c

    def check(self, customer: str, feature: str, estimate: Mapping[str, Any] | None = None) -> Entitlement:
        """Advisory, read-only. On ``allowed=False`` do not run the work. If this raises ``Unavailable``, fail closed."""
        return self._c._do(_core.entitlements_check(customer, feature, estimate))


class _Plans:
    def __init__(self, c: Client) -> None:
        self._c = c

    def upsert_meter(self, key: str, units_per_credit: int) -> Raw:
        return self._c._do(_core.plans_upsert_meter(key, units_per_credit))

    def upsert(self, key: str, name: str, included_credits: int, meter: str, features: list[str]) -> Raw:
        return self._c._do(_core.plans_upsert(key, name, included_credits, meter, features))

    def subscribe(self, customer: str, plan: str) -> Raw:
        return self._c._do(_core.plans_subscribe(customer, plan))

    def cancel(self, customer: str) -> Raw:
        return self._c._do(_core.plans_cancel(customer))


class _CustomerTokens:
    def __init__(self, c: Client) -> None:
        self._c = c

    def create(self, customer: str, ttl_seconds: int | None = None) -> CustomerToken:
        """A short-lived read token for ONE customer, for the usage widget in a browser. 30 to 3600 seconds."""
        return self._c._do(_core.customer_tokens_create(customer, ttl_seconds))

    def revoke(self, id: str) -> Raw:
        return self._c._do(_core.customer_tokens_revoke(id))


class _ACustomers:
    def __init__(self, c: AsyncClient) -> None:
        self._c = c

    async def upsert(self, id: str, name: str | None = None) -> Customer:
        return await self._c._do(_core.customers_upsert(id, name))

    async def get(self, customer: str) -> Customer:
        return await self._c._do(_core.customers_get(customer))

    async def list(self, limit: int | None = None, cursor: str | None = None) -> Page:
        return await self._c._do(_core.customers_list(limit, cursor))


class _ACredits:
    def __init__(self, c: AsyncClient) -> None:
        self._c = c

    async def grant(self, id: str, customer: str, credits: int, reason: str | None = None) -> LedgerEntry:
        return await self._c._do(_core.credits_grant(id, customer, credits, reason))

    async def balance(self, customer: str) -> Balance:
        return await self._c._do(_core.credits_balance(customer))

    async def ledger(self, customer: str, limit: int | None = None, cursor: str | None = None) -> Page:
        return await self._c._do(_core.credits_ledger(customer, limit, cursor))

    async def usage(self, customer: str) -> Raw:
        return await self._c._do(_core.credits_usage(customer))


class _AUsage:
    def __init__(self, c: AsyncClient) -> None:
        self._c = c

    async def record(self, id: str, customer: str, meter: str, quantity: int) -> UsageResult:
        return await self._c._do(_core.usage_record(id, customer, meter, quantity))

    async def record_batch(self, events: Sequence["BatchEvent | Mapping[str, Any]"]) -> BatchResponse:
        """As ``Client.usage.record_batch``."""
        spec = _core.usage_record_batch([_core.wire_event(e) for e in events])
        try:
            return await self._c._do(spec)
        except VunlyError as e:
            raise (_core.batch_route_off(e) or e) from None

    def batcher(
        self,
        *,
        max_batch_size: int = 100,
        flush_interval: float = 1.0,
        max_pending: int = 10_000,
        max_attempts: int = 5,
    ) -> "AsyncUsageBatcher":
        """As ``Client.usage.batcher``, for asyncio: ``await batcher.add(event)`` returns the event's final outcome."""
        from .batcher import AsyncUsageBatcher

        return AsyncUsageBatcher(
            self._c,
            max_batch_size=max_batch_size,
            flush_interval=flush_interval,
            max_pending=max_pending,
            max_attempts=max_attempts,
        )


class _AEntitlements:
    def __init__(self, c: AsyncClient) -> None:
        self._c = c

    async def check(self, customer: str, feature: str, estimate: Mapping[str, Any] | None = None) -> Entitlement:
        return await self._c._do(_core.entitlements_check(customer, feature, estimate))


class _APlans:
    def __init__(self, c: AsyncClient) -> None:
        self._c = c

    async def upsert_meter(self, key: str, units_per_credit: int) -> Raw:
        return await self._c._do(_core.plans_upsert_meter(key, units_per_credit))

    async def upsert(self, key: str, name: str, included_credits: int, meter: str, features: list[str]) -> Raw:
        return await self._c._do(_core.plans_upsert(key, name, included_credits, meter, features))

    async def subscribe(self, customer: str, plan: str) -> Raw:
        return await self._c._do(_core.plans_subscribe(customer, plan))

    async def cancel(self, customer: str) -> Raw:
        return await self._c._do(_core.plans_cancel(customer))


class _ACustomerTokens:
    def __init__(self, c: AsyncClient) -> None:
        self._c = c

    async def create(self, customer: str, ttl_seconds: int | None = None) -> CustomerToken:
        return await self._c._do(_core.customer_tokens_create(customer, ttl_seconds))

    async def revoke(self, id: str) -> Raw:
        return await self._c._do(_core.customer_tokens_revoke(id))
