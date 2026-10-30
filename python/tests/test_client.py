"""Unit tests with an in-memory transport: retries, Retry-After, error mapping, id validation, secrets. No network."""

from __future__ import annotations

import asyncio
import json

import httpx
import pytest

import vunly
from vunly import (
    AsyncClient,
    AuthError,
    Client,
    InvalidRequest,
    LimitReached,
    RetryPolicy,
    Unavailable,
    normalize_base_url,
)

KEY = "vk_test_synthetic_python_unit_0001"
ENTRY = {
    "id": "led_1",
    "customerId": "cus_1",
    "kind": "debit",
    "credits": -2,
    "meter": "tokens",
    "quantity": 1050,
    "reason": None,
    "idempotencyKey": "req-abc-12345",
    "createdAt": "2026-07-01T12:00:00.000Z",
}


def err(status: int, code: str, message: str = "m", **extra: object) -> httpx.Response:
    return httpx.Response(
        status,
        json={"error": {"code": code, "message": message, "requestId": "req_x1"}},
        headers={"request-id": "req_x1", **{k: str(v) for k, v in extra.items()}},
    )


class Script:
    """A transport that answers from a list (an Exception in the list is raised) and records what it saw."""

    def __init__(self, *answers: object) -> None:
        self.answers = list(answers)
        self.seen: list[httpx.Request] = []

    def __call__(self, request: httpx.Request) -> httpx.Response:
        self.seen.append(request)
        a = self.answers.pop(0)
        if isinstance(a, Exception):
            raise a
        assert isinstance(a, httpx.Response)
        return a


def client(script: Script, **kw: object) -> tuple[Client, list[float]]:
    slept: list[float] = []
    return (
        Client(KEY, "http://api.invalid/v1/", transport=httpx.MockTransport(script), sleep=slept.append, **kw),  # type: ignore[arg-type]
        slept,
    )


RECORD = dict(id="req-abc-12345", customer="cus_1", meter="tokens", quantity=1050)


def test_record_sends_the_id_as_idempotency_key_and_reads_the_result() -> None:
    s = Script(httpx.Response(201, json={"event": ENTRY, "balance": 98}, headers={"request-id": "req_9"}))
    c, _ = client(s)
    r = c.usage.record(**RECORD)
    assert (r.remaining, r.replayed, r.event.credits, r.request_id) == (98, False, -2, "req_9")
    req = s.seen[0]
    assert req.headers["idempotency-key"] == "req-abc-12345"
    assert req.headers["authorization"] == f"Bearer {KEY}"
    assert req.url == "http://api.invalid/v1/usage-events"  # /v1 in base_url is accepted, never doubled
    assert json.loads(req.content) == {"customerId": "cus_1", "meter": "tokens", "quantity": 1050}


def test_replay_is_reported() -> None:
    s = Script(httpx.Response(200, json={"event": ENTRY, "balance": 98}, headers={"idempotent-replayed": "true"}))
    assert client(s)[0].usage.record(**RECORD).replayed is True


@pytest.mark.parametrize("url", ["http://h", "http://h/", "http://h/v1", "http://h/v1/", "  http://h/v1  "])
def test_every_base_url_form_is_one_address(url: str) -> None:
    assert normalize_base_url(url) == "http://h"
    assert normalize_base_url("https://h/proxy/v1/") == "https://h/proxy"
    assert normalize_base_url("https://h/v10") == "https://h/v10"


def test_503_then_success_is_retried_with_the_same_id_and_backoff() -> None:
    s = Script(err(503, "internal_error"), err(500, "internal_error"), httpx.Response(201, json={"event": ENTRY, "balance": 1}))
    c, slept = client(s)
    assert c.usage.record(**RECORD).remaining == 1
    assert len(s.seen) == 3
    assert {r.headers["idempotency-key"] for r in s.seen} == {"req-abc-12345"}, "a retry must reuse the id"
    assert len(slept) == 2 and all(0 < w <= 8 for w in slept)


def test_retries_stop_at_max_retries_and_raise_unavailable() -> None:
    s = Script(err(503, "internal_error"), err(503, "internal_error"), err(503, "internal_error"))
    c, slept = client(s)
    with pytest.raises(Unavailable) as e:
        c.usage.record(**RECORD)
    assert e.value.status == 503 and e.value.request_id == "req_x1"
    assert len(s.seen) == 3 and len(slept) == 2  # first try + 2 retries


def test_429_honours_retry_after() -> None:
    s = Script(err(429, "rate_limited", **{"Retry-After": 2}), httpx.Response(201, json={"event": ENTRY, "balance": 5}))
    c, slept = client(s)
    c.usage.record(**RECORD)
    assert slept == [2.0]


def test_a_retry_after_longer_than_the_cap_raises_instead_of_hanging() -> None:
    s = Script(err(429, "rate_limited", **{"Retry-After": 3600}))
    c, slept = client(s)
    with pytest.raises(Unavailable) as e:
        c.usage.record(**RECORD)
    assert e.value.status == 429 and e.value.code == "rate_limited"
    assert slept == [] and len(s.seen) == 1


@pytest.mark.parametrize(
    "status,code,cls",
    [
        (402, "insufficient_credits", LimitReached),
        (400, "validation_error", InvalidRequest),
        (404, "not_found", InvalidRequest),
        (409, "idempotency_conflict", InvalidRequest),
        (401, "unauthenticated", AuthError),
        (403, "missing_scope", AuthError),
    ],
)
def test_other_4xx_are_never_retried(status: int, code: str, cls: type) -> None:
    s = Script(err(status, code))
    c, slept = client(s)
    with pytest.raises(cls) as e:
        c.usage.record(**RECORD)
    assert (e.value.status, e.value.code, e.value.request_id) == (status, code, "req_x1")
    assert len(s.seen) == 1 and slept == []


def test_a_network_failure_is_retried_then_raised_as_unavailable() -> None:
    s = Script(httpx.ConnectError("refused"), httpx.ConnectTimeout("slow"), httpx.ReadTimeout("slow"))
    c, slept = client(s)
    with pytest.raises(Unavailable) as e:
        c.usage.record(**RECORD)
    assert e.value.status == 0 and e.value.code == "unavailable"
    assert len(s.seen) == 3 and len(slept) == 2


def test_a_network_failure_then_success_applies_the_same_id() -> None:
    s = Script(httpx.ReadTimeout("slow"), httpx.Response(200, json={"event": ENTRY, "balance": 98}, headers={"idempotent-replayed": "true"}))
    r = client(s)[0].usage.record(**RECORD)
    assert r.replayed is True and len(s.seen) == 2
    assert s.seen[0].headers["idempotency-key"] == s.seen[1].headers["idempotency-key"]


def test_a_bad_id_is_refused_before_any_request() -> None:
    s = Script()
    c, _ = client(s)
    for bad in ["short", "has space in it 1", "x" * 129, ""]:
        with pytest.raises(InvalidRequest) as e:
            c.usage.record(id=bad, customer="c", meter="m", quantity=1)
        assert (e.value.status, e.value.code) == (400, "validation_error")
    with pytest.raises(InvalidRequest):
        c.credits.grant(id="short", customer="c", credits=1)
    assert s.seen == []


def test_no_such_route_404_says_to_check_the_address() -> None:
    s = Script(err(404, "not_found", "no such route"))
    with pytest.raises(InvalidRequest) as e:
        client(s)[0].customers.upsert("acme")
    assert "check the API address" in e.value.message


def test_the_key_never_appears_in_repr_or_errors() -> None:
    s = Script(err(401, "unauthenticated", "missing, invalid or revoked key"))
    c, _ = client(s)
    assert KEY not in repr(c) and "hidden" in repr(c)
    with pytest.raises(AuthError) as e:
        c.customers.upsert("acme")
    assert KEY not in str(e.value) and KEY not in repr(e.value)
    tok = vunly.CustomerToken("cut_1", "vct_secretvalue", "cus_1", "2026-01-01T00:00:00Z")
    assert "vct_secretvalue" not in repr(tok)


def test_entitlement_check_and_balance_are_typed() -> None:
    s = Script(
        httpx.Response(200, json={"allowed": False, "reason": "insufficient_credits", "remaining": 3, "required": 5, "plan": "starter"}),
        httpx.Response(200, json={"customerId": "cus_1", "credits": 7, "granted": 10, "consumed": 3, "asOf": "x"}),
    )
    c, _ = client(s)
    d = c.entitlements.check("cus_1", "generate", {"meter": "tokens", "quantity": 5000})
    assert (d.allowed, d.reason, d.required) == (False, "insufficient_credits", 5)
    assert json.loads(s.seen[0].content)["estimate"] == {"meter": "tokens", "quantity": 5000}
    b = c.credits.balance("cus_1")
    assert (b.remaining, b.granted, b.consumed) == (7, 10, 3)


def test_custom_policy_can_turn_retries_off() -> None:
    s = Script(err(503, "internal_error"))
    c, slept = client(s, retry=RetryPolicy(max_retries=0))
    with pytest.raises(Unavailable):
        c.customers.upsert("acme")
    assert len(s.seen) == 1 and slept == []


def test_empty_key_or_address_is_refused_at_construction() -> None:
    with pytest.raises(ValueError):
        Client("", "http://h")
    with pytest.raises(ValueError):
        Client(KEY, "")


MALFORMED = [
    ("record: event without createdAt", lambda c: c.usage.record(**RECORD), {"event": {k: v for k, v in ENTRY.items() if k != "createdAt"}, "balance": 1}),
    ("upsert: no createdAt", lambda c: c.customers.upsert("acme"), {"id": "cus_1", "externalId": "acme"}),
    ("check: no required", lambda c: c.entitlements.check("c", "f"), {"allowed": True, "reason": "ok", "remaining": 1}),
    ("balance: not an object", lambda c: c.credits.balance("c"), []),
    ("balance: empty body", lambda c: c.credits.balance("c"), {}),
    ("usage: no balance", lambda c: c.credits.usage("c"), {"customerId": "cus_1", "meters": []}),
    ("usage: not an object", lambda c: c.credits.usage("c"), []),
    ("usage: meters not a list", lambda c: c.credits.usage("c"), {"customerId": "cus_1", "balance": {"credits": 1, "granted": 1, "consumed": 0}, "meters": None}),
]

USAGE_VIEW = {"customerId": "cus_1", "plan": None, "period": None, "balance": {"credits": 7, "granted": 10, "consumed": 3}, "meters": []}


def test_a_well_formed_usage_view_is_returned_as_raw() -> None:
    c, _ = client(Script(httpx.Response(200, json=USAGE_VIEW)))
    r = c.credits.usage("c")
    assert r.data["balance"]["credits"] == 7


def test_a_non_json_usage_view_is_invalid_response() -> None:
    c, _ = client(Script(httpx.Response(200, content=b"<html>proxy</html>")))
    with pytest.raises(Unavailable) as e:
        c.credits.usage("c")
    assert e.value.code == "invalid_response"


def test_async_usage_view_malformed_is_invalid_response() -> None:
    async def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"customerId": "cus_1", "meters": []})

    async def run() -> None:
        async with AsyncClient(KEY, "http://api.invalid", transport=httpx.MockTransport(handler)) as c:
            with pytest.raises(Unavailable) as e:
                await c.credits.usage("c")
            assert e.value.code == "invalid_response"

    asyncio.run(run())


@pytest.mark.parametrize("name,call,body", MALFORMED, ids=[m[0] for m in MALFORMED])
def test_a_malformed_2xx_is_a_vunly_error_not_a_keyerror_and_is_not_retried(name: str, call: object, body: object) -> None:
    s = Script(httpx.Response(200, json=body, headers={"request-id": "req_m1"}))
    c, slept = client(s)
    with pytest.raises(Unavailable) as e:
        call(c)  # type: ignore[operator]
    assert e.value.code == "invalid_response" and e.value.status == 200 and e.value.request_id == "req_m1"
    assert "malformed" in str(e.value) and len(s.seen) == 1 and slept == []


def test_a_non_json_2xx_is_a_vunly_error_too() -> None:
    c, _ = client(Script(httpx.Response(200, content=b"<html>proxy</html>")))
    with pytest.raises(Unavailable):
        c.credits.balance("c")


def test_async_client_turns_a_malformed_response_into_a_vunly_error() -> None:
    async def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(201, json={"event": {"id": "led_1"}, "balance": 1})

    async def run() -> None:
        async with AsyncClient(KEY, "http://api.invalid", transport=httpx.MockTransport(handler)) as c:
            with pytest.raises(Unavailable) as e:
                await c.usage.record(**RECORD)
            assert e.value.code == "invalid_response"

    asyncio.run(run())


# ---- async: the same behaviour ---------------------------------------------------------------------------------------
def test_async_client_retries_and_maps_errors_like_the_sync_one() -> None:
    answers = [err(503, "internal_error"), httpx.Response(201, json={"event": ENTRY, "balance": 4})]
    seen: list[httpx.Request] = []

    async def handler(request: httpx.Request) -> httpx.Response:
        seen.append(request)
        return answers.pop(0)

    slept: list[float] = []

    async def fake_sleep(d: float) -> None:
        slept.append(d)

    async def run() -> None:
        async with AsyncClient(KEY, "http://api.invalid", transport=httpx.MockTransport(handler), sleep=fake_sleep) as c:
            r = await c.usage.record(**RECORD)
            assert r.remaining == 4 and len(seen) == 2 and len(slept) == 1
            assert seen[0].headers["idempotency-key"] == seen[1].headers["idempotency-key"]
        answers.append(err(402, "insufficient_credits"))
        async with AsyncClient(KEY, "http://api.invalid", transport=httpx.MockTransport(handler), sleep=fake_sleep) as c:
            with pytest.raises(LimitReached):
                await c.usage.record(**RECORD)
            with pytest.raises(InvalidRequest):
                await c.usage.record(id="short", customer="c", meter="m", quantity=1)

    asyncio.run(run())


# ---- retry classes (conformance/README.md) ---------------------------------------------------------
TOKEN = {"id": "cut_1", "token": "vct_x", "customerId": "cus_1", "expiresAt": "t"}


def test_a_3xx_is_unexpected_redirect_class_unavailable_never_followed_or_retried() -> None:
    for status in (301, 302, 307, 308):
        body = httpx.Response(
            status,
            json={"error": {"code": "insufficient_credits", "message": "pay me"}},
            headers={"location": "http://elsewhere.invalid/v1/customers/cus_1"},
        )
        s = Script(body, body)
        c, slept = client(s)
        with pytest.raises(Unavailable) as e:
            c.customers.get("cus_1")
        assert e.value.code == "unexpected_redirect" and e.value.status == status
        assert "pay me" not in str(e.value) and "elsewhere" not in str(e.value)
        assert len(s.seen) == 1 and slept == []  # one request: not followed, not retried


def test_class_x_is_not_repeated_after_a_lost_answer_or_a_5xx() -> None:
    # a customer token is a new secret on every call and has no key: after a lost answer the caller decides
    s = Script(httpx.ConnectError("reset"), httpx.Response(201, json=TOKEN))
    c, slept = client(s)
    with pytest.raises(Unavailable) as e:
        c.customer_tokens.create("cus_1")
    assert e.value.status == 0 and len(s.seen) == 1 and slept == []
    s5 = Script(err(503, "internal_error"), httpx.Response(201, json=TOKEN))
    c5, _ = client(s5)
    with pytest.raises(Unavailable):
        c5.customer_tokens.create("cus_1")
    assert len(s5.seen) == 1


def test_class_x_is_repeated_after_a_429_because_it_was_refused_before_it_ran() -> None:
    s = Script(err(429, "rate_limited", **{"Retry-After": 1}), httpx.Response(201, json=TOKEN))
    c, slept = client(s)
    assert c.customer_tokens.create("cus_1").id == "cut_1"
    assert len(s.seen) == 2 and slept == [1.0]


def test_class_n_and_r_are_repeated_after_a_lost_answer() -> None:
    cust = {"id": "cus_1", "externalId": "acme", "createdAt": "t"}
    s = Script(httpx.ConnectError("reset"), httpx.Response(200, json=cust))
    c, _ = client(s)
    assert c.customers.upsert("acme").external_id == "acme"  # N: an upsert
    assert len(s.seen) == 2
    s2 = Script(httpx.ReadTimeout("slow"), httpx.Response(200, json={"credits": 1, "consumed": 0, "granted": 1}))
    c2, _ = client(s2)
    assert c2.credits.balance("acme").remaining == 1  # R: a read
    assert len(s2.seen) == 2
    s3 = Script(
        httpx.ConnectError("reset"),
        httpx.Response(200, json={"allowed": True, "reason": "ok", "remaining": 1, "required": 0}),
    )
    c3, _ = client(s3)
    assert c3.entitlements.check("acme", "f").allowed is True  # R: the check is a POST but read-only
    assert len(s3.seen) == 2


def test_the_plan_volume_stop_is_not_a_rate_limit_and_is_never_waited_on() -> None:
    stop = httpx.Response(
        429,
        json={
            "error": {
                "code": "rate_limited",
                "message": "plan volume",
                "requestId": "req_p1",
                "details": {"reason": "plan_included_volume", "plan": "free"},
            }
        },
        headers={"retry-after": "0"},  # even a short Retry-After: it is the time to the next month in real life
    )
    s = Script(stop, httpx.Response(201, json={"event": ENTRY, "balance": 1}))
    c, slept = client(s)
    with pytest.raises(Unavailable) as e:
        c.usage.record(**RECORD)
    assert e.value.status == 429 and e.value.details == {"reason": "plan_included_volume", "plan": "free"}
    assert len(s.seen) == 1 and slept == []


def test_the_overage_header_is_surfaced_and_a_replay_is_never_an_overage() -> None:
    body = {"event": ENTRY, "balance": 1}
    s = Script(
        httpx.Response(201, json=body, headers={"vunly-plan-overage": "true"}),
        httpx.Response(200, json=body, headers={"vunly-plan-overage": "true", "idempotent-replayed": "true"}),
        httpx.Response(201, json=body),
    )
    c, _ = client(s)
    assert c.usage.record(**RECORD).plan_overage is True
    assert c.usage.record(**RECORD).plan_overage is False  # replay
    assert c.usage.record(**RECORD).plan_overage is False  # within the included volume


def test_every_request_names_the_sdk() -> None:
    s = Script(httpx.Response(200, json={"credits": 1, "consumed": 0, "granted": 1}))
    c, _ = client(s)
    c.credits.balance("acme")
    assert s.seen[0].headers["user-agent"].startswith("vunly-python/")


def test_lists_and_plan_usage_are_read_as_typed_results() -> None:
    page = {"data": [ENTRY], "nextCursor": "abc"}
    last = {"data": [], "nextCursor": None}
    usage = {
        "plan": {"key": "free", "name": "Free"},
        "period": {"start": "a", "end": "b"},
        "usage": {"events": 3},
        "unconfirmedLimits": {"confirmed": False, "note": "n", "includedMonthlyEvents": 10, "activityRetentionDays": 7},
    }
    s = Script(httpx.Response(200, json=page), httpx.Response(200, json=last), httpx.Response(200, json=usage))
    c, _ = client(s)
    p1 = c.credits.ledger("acme", limit=1)
    assert [e.id for e in p1.data] == ["led_1"] and p1.next_cursor == "abc"
    assert s.seen[0].url == "http://api.invalid/v1/customers/acme/ledger?limit=1"
    p2 = c.credits.ledger("acme", limit=1, cursor=p1.next_cursor)
    assert p2.data == [] and p2.next_cursor is None
    assert "cursor=abc" in str(s.seen[1].url)
    u = c.plan_usage()
    assert u.plan["key"] == "free" and u.usage["events"] == 3 and u.unconfirmed_limits["confirmed"] is False
