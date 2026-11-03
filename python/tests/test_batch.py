"""Usage through the batch route. The answers are the ones the route documents (apps/api/openapi.yaml, recordUsageBatch):
200 with one result per event in order, ``unavailable`` for an event that was not recorded, ``plan_limit_reached`` with
``planLimit``, a whole-batch 503 with Retry-After, 404 while the route is off. In-memory transport, no network."""

from __future__ import annotations

import asyncio
import itertools
import json
from typing import Any, Callable

import httpx
import pytest

from vunly import (
    AsyncClient,
    AuthError,
    BatchEvent,
    Client,
    InvalidRequest,
    Unavailable,
    VunlyError,
)

KEY = "vk_test_synthetic_python_batch_0001"
_n = itertools.count()


def ev(**over: Any) -> dict[str, Any]:
    e: dict[str, Any] = {"id": f"evt_py_{next(_n):06d}", "customer": "cus_abc123", "meter": "tokens", "quantity": 5}
    e.update(over)
    return e


def reply(body: dict[str, Any], status: Callable[[int], str] = lambda i: "accepted", headers: dict[str, str] | None = None) -> httpx.Response:
    return httpx.Response(
        200,
        json={"results": [{"id": e["id"], "status": status(i), "balance": 90} for i, e in enumerate(body["events"])]},
        headers=headers or {},
    )


def err(status: int, code: str, details: dict[str, Any] | None = None, **headers: str) -> httpx.Response:
    return httpx.Response(
        status,
        json={"error": {"code": code, "message": code, "requestId": "req_x", **({"details": details} if details else {})}},
        headers=headers,
    )


class Harness:
    """A transport that answers from ``handler(call_number, body)`` and records bodies and waits."""

    def __init__(self, handler: Callable[[int, dict[str, Any]], httpx.Response], max_retries: int = 0) -> None:
        self.bodies: list[dict[str, Any]] = []
        self.waits: list[float] = []
        self.headers: list[httpx.Headers] = []

        def transport(request: httpx.Request) -> httpx.Response:
            assert request.url.path == "/v1/usage-events:batch"
            assert request.method == "POST"
            body = json.loads(request.content)
            self.bodies.append(body)
            self.headers.append(request.headers)
            return handler(len(self.bodies), body)

        self.client = Client(KEY, "http://api.invalid", transport=httpx.MockTransport(transport), sleep=self.waits.append, max_retries=max_retries)


def statuses(rs: list[Any]) -> list[str]:
    return [r.status for r in rs]


# -- record_batch ---------------------------------------------------------------------------------------------------------


def test_record_batch_sends_the_route_body_and_returns_the_results_in_order() -> None:
    h = Harness(lambda _n, b: reply(b, lambda i: "duplicate" if i == 1 else "accepted"))
    events = [ev(), ev(), ev(occurred_at="2026-11-01T10:00:00+02:00")]
    r = h.client.usage.record_batch(events)
    assert statuses(r.results) == ["accepted", "duplicate", "accepted"]
    assert [x.id for x in r.results] == [e["id"] for e in events]
    assert r.plan_overage is False and r.plan_limit is None
    sent = h.bodies[0]["events"]
    assert sorted(sent[0]) == ["customerId", "id", "meter", "occurredAt", "quantity"]
    assert sent[0]["customerId"] == "cus_abc123"
    assert sent[2]["occurredAt"] == "2026-11-01T08:00:00.000Z"  # an offset date is sent as the same instant in UTC
    assert sent[0]["occurredAt"].endswith("Z")  # stamped when omitted
    assert h.headers[0]["authorization"] == f"Bearer {KEY}"
    assert "idempotency-key" not in h.headers[0]  # each event carries its own id


def test_record_batch_accepts_the_dataclass_too() -> None:
    h = Harness(lambda _n, b: reply(b))
    r = h.client.usage.record_batch([BatchEvent(id="evt_dataclass_1", customer="cus_x1", meter="tokens", quantity=2)])
    assert statuses(r.results) == ["accepted"]


def test_plan_limit_and_overage_are_surfaced() -> None:
    limit = {"reason": "plan_included_volume", "plan": "free", "includedMonthlyVolume": 10, "used": 10}
    h = Harness(lambda _n, b: httpx.Response(200, json={"results": [{"id": e["id"], "status": "plan_limit_reached"} for e in b["events"]], "planLimit": limit}))
    r = h.client.usage.record_batch([ev()])
    assert r.results[0].status == "plan_limit_reached"
    assert (r.plan_limit.plan, r.plan_limit.included_monthly_volume, r.plan_limit.used) == ("free", 10, 10)  # type: ignore[union-attr]
    paid = Harness(lambda _n, b: httpx.Response(200, json={"results": [{"id": e["id"], "status": "accepted"} for e in b["events"]]}, headers={"Vunly-Plan-Overage": "true"}))
    assert paid.client.usage.record_batch([ev()]).plan_overage is True


@pytest.mark.parametrize(
    "bad",
    [
        [ev(id="short")],
        [ev(customer="my-customer")],
        [ev(meter="Bad Meter")],
        [ev(quantity=0)],
        [ev(quantity=1.5)],
        [ev(quantity=True)],
        [ev(quantity=1_000_000_001)],
        [ev(occurred_at="not a date")],
        [],
        [ev() for _ in range(101)],
    ],
)
def test_bad_events_are_refused_before_any_request(bad: list[dict[str, Any]]) -> None:
    h = Harness(lambda _n, _b: pytest.fail("no request expected"))
    with pytest.raises(InvalidRequest):
        h.client.usage.record_batch(bad)
    assert h.bodies == []


def test_a_repeated_id_in_one_batch_is_refused_locally() -> None:
    h = Harness(lambda _n, _b: pytest.fail("no request expected"))
    with pytest.raises(InvalidRequest):
        h.client.usage.record_batch([ev(id="evt_repeated_1"), ev(id="evt_repeated_1")])


def test_100_events_go_in_one_request() -> None:
    h = Harness(lambda _n, b: reply(b))
    assert len(h.client.usage.record_batch([ev() for _ in range(100)]).results) == 100
    assert len(h.bodies) == 1


@pytest.mark.parametrize("results", [[], [{"id": "evt_other_1", "status": "accepted"}], [{"id": "x", "status": 1}]])
def test_results_that_do_not_match_are_invalid_response(results: list[Any]) -> None:
    h = Harness(lambda _n, _b: httpx.Response(200, json={"results": results}))
    with pytest.raises(Unavailable) as e:
        h.client.usage.record_batch([ev()])
    assert e.value.code == "invalid_response"


def test_a_whole_batch_503_is_retried_with_the_same_body_after_retry_after_then_surfaces() -> None:
    h = Harness(lambda _n, _b: err(503, "unavailable", {"reason": "shipping_behind"}, **{"Retry-After": "1"}), max_retries=2)
    with pytest.raises(Unavailable) as e:
        h.client.usage.record_batch([ev(), ev()])
    assert e.value.status == 503 and e.value.retry_after_seconds == 1
    assert len(h.bodies) == 3 and h.bodies[1] == h.bodies[0]
    assert h.waits == [1.0, 1.0]


def test_a_route_that_is_off_says_so() -> None:
    h = Harness(lambda _n, _b: err(404, "not_found"))
    with pytest.raises(InvalidRequest) as e:
        h.client.usage.record_batch([ev()])
    assert e.value.code == "not_found" and "usage.record" in e.value.message


def test_the_key_is_not_in_an_error() -> None:
    h = Harness(lambda _n, _b: err(403, "forbidden"))
    with pytest.raises(AuthError) as e:
        h.client.usage.record_batch([ev()])
    assert KEY not in str(e.value)


# -- the sync batcher ------------------------------------------------------------------------------------------------------


def test_a_full_buffer_is_sent_at_once_and_each_add_gets_its_own_outcome() -> None:
    h = Harness(lambda _n, b: reply(b, lambda i: "insufficient_credits" if i == 2 else "accepted"))
    with h.client.usage.batcher(max_batch_size=3, flush_interval=0) as b:
        futs = [b.add(ev()) for _ in range(3)]
        out = [f.result(timeout=5) for f in futs]
    assert statuses(out) == ["accepted", "accepted", "insufficient_credits"]
    assert len(h.bodies) == 1 and len(h.bodies[0]["events"]) == 3


def test_a_partial_buffer_waits_for_flush() -> None:
    h = Harness(lambda _n, b: reply(b))
    b = h.client.usage.batcher(flush_interval=0)
    f = b.add(ev())
    assert not f.done() and h.bodies == [] and b.pending == 1
    b.flush()
    assert f.result(timeout=5).status == "accepted"
    b.close()


def test_the_interval_sends_a_partial_buffer() -> None:
    h = Harness(lambda _n, b: reply(b))
    with h.client.usage.batcher(flush_interval=0.01) as b:
        assert b.add(ev()).result(timeout=5).status == "accepted"


def test_an_unavailable_event_is_resent_alone_with_the_same_id() -> None:
    def handler(n: int, b: dict[str, Any]) -> httpx.Response:
        if n == 1:
            return reply(b, lambda i: "unavailable" if i == 1 else "accepted")
        return reply(b)

    h = Harness(handler)
    with h.client.usage.batcher(flush_interval=0) as b:
        futs = [b.add(ev()) for _ in range(3)]
        b.flush()
        out = [f.result(timeout=5) for f in futs]
    assert statuses(out) == ["accepted"] * 3
    assert len(h.bodies) == 2
    assert h.bodies[1]["events"] == [h.bodies[0]["events"][1]]  # only that one, unchanged (same id, same occurredAt)
    assert len(h.waits) == 1 and h.waits[0] >= 0.125


def test_a_whole_batch_503_is_resent_after_its_retry_after() -> None:
    h = Harness(lambda n, b: err(503, "unavailable", {"reason": "shipping_behind"}, **{"Retry-After": "3"}) if n == 1 else reply(b))
    with h.client.usage.batcher(flush_interval=0) as b:
        futs = [b.add(ev()), b.add(ev())]
        b.flush()
        assert statuses([f.result(timeout=5) for f in futs]) == ["accepted", "accepted"]
    assert h.waits == [3.0]
    assert h.bodies[1] == h.bodies[0]


def test_it_stops_after_max_attempts_and_reports_unavailable() -> None:
    h = Harness(lambda _n, b: reply(b, lambda i: "unavailable"))
    with h.client.usage.batcher(flush_interval=0, max_attempts=3) as b:
        f = b.add(ev())
        b.flush()
        assert f.result(timeout=5).status == "unavailable"
    assert len(h.bodies) == 3


def test_a_retry_after_longer_than_the_sdk_waits_surfaces_the_events_at_once() -> None:
    h = Harness(lambda _n, _b: err(503, "unavailable", None, **{"Retry-After": "3600"}))
    with h.client.usage.batcher(flush_interval=0) as b:
        f = b.add(ev())
        b.flush()
        assert f.result(timeout=5).status == "unavailable"
    assert len(h.bodies) == 1 and h.waits == []


def test_final_outcomes_are_never_resent() -> None:
    final = ["plan_limit_reached", "duplicate", "too_late", "meter_not_found", "error"]
    h = Harness(lambda _n, b: reply(b, lambda i: final[i]))
    with h.client.usage.batcher(flush_interval=0) as b:
        futs = [b.add(ev()) for _ in final]
        b.flush()
        assert statuses([f.result(timeout=5) for f in futs]) == final
    assert len(h.bodies) == 1


def test_a_failure_a_resend_cannot_fix_fails_each_event_and_is_not_retried() -> None:
    h = Harness(lambda _n, _b: err(401, "unauthenticated"))
    with h.client.usage.batcher(flush_interval=0) as b:
        f = b.add(ev())
        b.flush()
        with pytest.raises(AuthError):
            f.result(timeout=5)
    assert len(h.bodies) == 1
    frozen = Harness(lambda _n, _b: err(403, "project_frozen"))
    with frozen.client.usage.batcher(flush_interval=0) as b2:
        f2 = b2.add(ev())
        b2.flush()
        with pytest.raises(VunlyError) as e:
            f2.result(timeout=5)
        assert e.value.code == "project_frozen"


def test_the_buffer_is_bounded() -> None:
    h = Harness(lambda _n, b: reply(b))
    b = h.client.usage.batcher(flush_interval=0, max_pending=2)
    f1, f2 = b.add(ev()), b.add(ev())
    with pytest.raises(Unavailable):
        b.add(ev())
    b.flush()
    assert f1.result(timeout=5).status == f2.result(timeout=5).status == "accepted"
    after = b.add(ev())
    b.close()
    assert after.result(timeout=5).status == "accepted"


def test_the_same_event_twice_is_one_send_and_a_different_body_is_a_conflict() -> None:
    h = Harness(lambda _n, b: reply(b))
    b = h.client.usage.batcher(flush_interval=0)
    e = ev()
    assert b.add(e) is b.add(dict(e))
    with pytest.raises(InvalidRequest) as err_:
        b.add({**e, "quantity": e["quantity"] + 1})
    assert err_.value.code == "idempotency_conflict"
    b.close()
    assert len(h.bodies[0]["events"]) == 1


def test_an_invalid_event_queues_nothing_and_close_refuses_more() -> None:
    h = Harness(lambda _n, b: reply(b))
    b = h.client.usage.batcher(flush_interval=0)
    with pytest.raises(InvalidRequest):
        b.add(ev(customer="not-cus"))
    assert b.pending == 0
    f = b.add(ev())
    b.close()
    assert f.result(timeout=5).status == "accepted"
    with pytest.raises(Unavailable) as e:
        b.add(ev())
    assert "closed" in e.value.message


@pytest.mark.parametrize("kw", [{"max_batch_size": 0}, {"max_batch_size": 101}, {"max_pending": 0}, {"max_attempts": 0}, {"flush_interval": -1}])
def test_nonsense_options_are_refused(kw: dict[str, Any]) -> None:
    h = Harness(lambda _n, _b: pytest.fail("no request expected"))
    with pytest.raises(ValueError):
        h.client.usage.batcher(**kw)


def test_more_than_a_batch_is_split_in_order() -> None:
    h = Harness(lambda _n, b: reply(b))
    events = [ev() for _ in range(250)]
    with h.client.usage.batcher(flush_interval=0) as b:
        futs = [b.add(e) for e in events]
        b.flush()
        out = [f.result(timeout=5) for f in futs]
    assert len(out) == 250
    assert all(len(x["events"]) <= 100 for x in h.bodies)
    assert [e["id"] for x in h.bodies for e in x["events"]] == [e["id"] for e in events]


# -- the async client and batcher ---------------------------------------------------------------------------------------


class AHarness:
    def __init__(self, handler: Callable[[int, dict[str, Any]], httpx.Response], max_retries: int = 0) -> None:
        self.bodies: list[dict[str, Any]] = []
        self.waits: list[float] = []

        async def transport(request: httpx.Request) -> httpx.Response:
            body = json.loads(request.content)
            self.bodies.append(body)
            return handler(len(self.bodies), body)

        async def sleep(s: float) -> None:
            self.waits.append(s)

        self.client = AsyncClient(KEY, "http://api.invalid", transport=httpx.MockTransport(transport), sleep=sleep, max_retries=max_retries)


def test_async_record_batch() -> None:
    async def go() -> None:
        h = AHarness(lambda _n, b: reply(b, lambda i: "duplicate" if i else "accepted"))
        r = await h.client.usage.record_batch([ev(), ev()])
        assert statuses(r.results) == ["accepted", "duplicate"]
        with pytest.raises(InvalidRequest):
            await h.client.usage.record_batch([ev(customer="x")])

    asyncio.run(go())


def test_async_batcher_resends_only_the_unavailable_event_and_resolves_every_add() -> None:
    async def go() -> None:
        def handler(n: int, b: dict[str, Any]) -> httpx.Response:
            if n == 1:
                return reply(b, lambda i: "unavailable" if i == 1 else "accepted")
            return reply(b)

        h = AHarness(handler)
        async with h.client.usage.batcher(flush_interval=0) as b:
            tasks = [asyncio.ensure_future(b.add(ev())) for _ in range(3)]
            await asyncio.sleep(0)
            await b.flush()
            out = await asyncio.gather(*tasks)
        assert statuses(list(out)) == ["accepted"] * 3
        assert len(h.bodies) == 2 and h.bodies[1]["events"] == [h.bodies[0]["events"][1]]
        assert len(h.waits) == 1

    asyncio.run(go())


def test_async_batcher_whole_503_final_failure_and_bounds() -> None:
    async def go() -> None:
        h = AHarness(lambda _n, _b: err(503, "unavailable", {"reason": "shipping_behind"}))
        async with h.client.usage.batcher(flush_interval=0, max_attempts=2) as b:
            t = asyncio.ensure_future(b.add(ev()))
            await asyncio.sleep(0)
            await b.flush()
            assert (await t).status == "unavailable"
        assert len(h.bodies) == 2

        auth = AHarness(lambda _n, _b: err(401, "unauthenticated"))
        async with auth.client.usage.batcher(flush_interval=0) as b2:
            t2 = asyncio.ensure_future(b2.add(ev()))
            await asyncio.sleep(0)
            await b2.flush()
            with pytest.raises(AuthError):
                await t2
        assert len(auth.bodies) == 1

        full = AHarness(lambda _n, b: reply(b))
        b3 = full.client.usage.batcher(flush_interval=0, max_pending=1)
        t3 = asyncio.ensure_future(b3.add(ev()))
        await asyncio.sleep(0)
        with pytest.raises(Unavailable):
            await b3.add(ev())
        await b3.aclose()
        assert (await t3).status == "accepted"
        with pytest.raises(Unavailable):
            await b3.add(ev())

    asyncio.run(go())


def test_async_interval_sends_a_partial_buffer() -> None:
    async def go() -> None:
        h = AHarness(lambda _n, b: reply(b))
        async with h.client.usage.batcher(flush_interval=0.01) as b:
            assert (await b.add(ev())).status == "accepted"

    asyncio.run(go())
