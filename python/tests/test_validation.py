"""A 2xx the SDK cannot trust must become one ``Unavailable`` ``invalid_response`` (or a ``Raw`` for an unmodelled
route) — never a value of the wrong type handed to the caller, and never a bare ``KeyError``. This enumerates every
public method of ``Client`` (and the same set on ``AsyncClient``), asserts the table below covers them all (a new
method without an entry fails the test), and for each modelled method injects the malformed modes: {}, [], null,
non-JSON, empty, 204, and — from a minimal good body — every field set to null and to the wrong type.
"""

from __future__ import annotations

import asyncio
from typing import Any, Callable

import httpx
import pytest

import vunly
import vunly.client as client_module
from vunly import AsyncClient, Client, Unavailable

KEY = "vk_test_synthetic_validation_0001"
ID = "req-validation-0001"
ENTRY = {"id": "led_1", "customerId": "c", "kind": "debit", "credits": -1, "createdAt": "t"}


# name -> how to call it (resource method + kwargs), a minimal valid body, and whether it is an unmodelled Raw route.
# A Raw route accepts any JSON object (and an empty body); a non-object (list, null, non-JSON) is still invalid.
CASES: dict[str, dict[str, Any]] = {
    "customers.upsert": {"args": {"id": "acme"}, "good": {"id": "cus_1", "externalId": "acme", "createdAt": "t"}},
    "customers.get": {"args": {"customer": "acme"}, "good": {"id": "cus_1", "externalId": "acme", "createdAt": "t"}},
    # nextCursor is nullable, so the minimal good body leaves it out; `data` is the field that must be a list
    "customers.list": {"args": {"limit": 1}, "good": {"data": []}},
    "credits.ledger": {"args": {"customer": "c", "limit": 1}, "good": {"data": []}},
    "credits.grant": {"args": {"id": ID, "customer": "c", "credits": 1}, "good": ENTRY},
    "credits.balance": {"args": {"customer": "c"}, "good": {"credits": 1, "consumed": 0, "granted": 1}},
    "credits.usage": {
        "args": {"customer": "c"},
        "good": {"customerId": "c", "balance": {"credits": 1, "granted": 1, "consumed": 0}, "meters": []},
    },
    "usage.record": {"args": {"id": ID, "customer": "c", "meter": "m", "quantity": 1}, "good": {"event": ENTRY, "balance": 1}},
    "entitlements.check": {
        "args": {"customer": "c", "feature": "f"},
        "good": {"allowed": True, "reason": "ok", "remaining": 1, "required": 0},
    },
    "customer_tokens.create": {
        "args": {"customer": "c"},
        "good": {"id": "cut_1", "token": "vct_x", "customerId": "c", "expiresAt": "t"},
    },
    "plans.upsert_meter": {"args": {"key": "m", "units_per_credit": 1}, "raw": True},
    "plans.upsert": {"args": {"key": "p", "name": "P", "included_credits": 1, "meter": "m", "features": []}, "raw": True},
    "plans.subscribe": {"args": {"customer": "c", "plan": "p"}, "raw": True},
    "plans.cancel": {"args": {"customer": "c"}, "raw": True},
    "customer_tokens.revoke": {"args": {"id": "cut_1"}, "raw": True},
}


def _method_paths(client: Any) -> list[str]:
    out: list[str] = []
    for rname, resource in vars(client).items():
        if rname.startswith("_") or type(resource).__module__ != client_module.__name__:
            continue
        for m in dir(resource):
            if not m.startswith("_") and callable(getattr(resource, m)):
                out.append(f"{rname}.{m}")
    return sorted(out)


def _wrong_type(value: Any) -> Any:
    if isinstance(value, bool):
        return "not-a-bool"
    if isinstance(value, int):
        return "not-a-number"
    if isinstance(value, list):
        return 123
    if isinstance(value, dict):
        return []
    return 123  # a string: a number is wrong


def _malformed_bodies(case: dict[str, Any]) -> list[tuple[str, httpx.Response]]:
    # `json=None` sends an empty body, so send a literal `null` as content to test a real null answer
    non_object = [
        ("array", httpx.Response(200, json=[])),
        ("null", httpx.Response(200, content=b"null", headers={"content-type": "application/json"})),
        ("non-JSON", httpx.Response(200, content=b"<html>proxy</html>")),
    ]
    if case.get("raw"):
        return non_object
    modes = non_object + [
        ("empty object", httpx.Response(200, json={})),
        ("empty body", httpx.Response(200)),
        ("204", httpx.Response(204)),
    ]
    good: dict[str, Any] = case["good"]
    for field, value in good.items():
        modes.append((f"{field}=null", httpx.Response(200, json={**good, field: None})))
        modes.append((f"{field}=wrong", httpx.Response(200, json={**good, field: _wrong_type(value)})))
    return modes


def _client(handler: Callable[[httpx.Request], httpx.Response]) -> Client:
    return Client(KEY, "http://api.invalid", transport=httpx.MockTransport(handler))


def _resource_method(client: Any, name: str) -> Callable[..., Any]:
    resource, method = name.split(".")
    return getattr(getattr(client, resource), method)


def test_every_method_is_covered_by_the_table() -> None:
    sync = _method_paths(Client(KEY, "http://api.invalid"))
    asy = _method_paths(AsyncClient(KEY, "http://api.invalid"))
    assert sync == asy == sorted(CASES), f"methods and CASES differ: {set(sync) ^ set(CASES)}"


@pytest.mark.parametrize("name", list(CASES))
def test_a_malformed_2xx_is_invalid_response_sync(name: str) -> None:
    case = CASES[name]
    for mode, response in _malformed_bodies(case):
        c = _client(lambda _req, r=response: r)
        with pytest.raises(Unavailable) as e:
            _resource_method(c, name)(**case["args"])
        assert e.value.code == "invalid_response", f"{name} / {mode}"
    # the good body (or, for a Raw route, an empty object) is accepted
    ok = httpx.Response(201 if name in ("usage.record",) else 200, json=case.get("good", {}))
    _resource_method(_client(lambda _req: ok), name)(**case["args"])


@pytest.mark.parametrize("name", list(CASES))
def test_a_malformed_2xx_is_invalid_response_async(name: str) -> None:
    case = CASES[name]

    async def run() -> None:
        for mode, response in _malformed_bodies(case):
            async with AsyncClient(KEY, "http://api.invalid", transport=httpx.MockTransport(lambda _req, r=response: r)) as c:
                with pytest.raises(Unavailable) as e:
                    await _resource_method(c, name)(**case["args"])
                assert e.value.code == "invalid_response", f"{name} / {mode}"

    asyncio.run(run())


@pytest.mark.parametrize(
    "status,code",
    [(401, "unauthenticated"), (403, "unauthenticated"), (402, "insufficient_credits"), (429, "rate_limited"), (503, "internal_error"), (400, "validation_error")],
)
def test_an_error_without_a_body_takes_its_code_from_the_status(status: int, code: str) -> None:
    c = _client(lambda _req: httpx.Response(status))
    with pytest.raises(vunly.VunlyError) as e:
        c.customers.upsert("acme")
    assert e.value.code == code and e.value.status == status


def test_a_server_that_never_answers_times_out_not_hangs() -> None:
    def handler(_req: httpx.Request) -> httpx.Response:
        raise httpx.ReadTimeout("timed out")

    c = Client(KEY, "http://api.invalid", max_retries=0, transport=httpx.MockTransport(handler))
    with pytest.raises(Unavailable):
        c.credits.balance("c")
