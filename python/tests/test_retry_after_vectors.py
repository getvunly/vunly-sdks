"""The shared Retry-After vectors (conformance/retry-after-vectors.json, copied here so the package tests stand alone).

A number above the 30 s cap, or too large for a float (``1e999``, hundreds of digits, ``Inf``), is a very long wait: surfaced at
once with ``retry_after_seconds`` set, never retried early. A number up to 30 is the wait. Anything else is no header.
"""

from __future__ import annotations

import json
from pathlib import Path

import httpx
import pytest

from vunly import Client, Unavailable
from vunly._core import parse_retry_after

KEY = "vk_test_synthetic_python_retry_after_01"
VECTORS = json.loads((Path(__file__).parent / "retry-after-vectors.json").read_text(encoding="utf-8"))["vectors"]


def test_there_are_enough_vectors() -> None:
    assert len(VECTORS) >= 40


@pytest.mark.parametrize("vector", VECTORS, ids=lambda v: v["kind"] + ":" + v["header"][:20])
def test_the_parser_agrees_with_every_vector(vector: dict) -> None:
    got = parse_retry_after(vector["header"])
    if vector["kind"] == "ignored":
        assert got is None, f"{vector['header'][:30]!r} should mean none, got {got}"
    else:
        assert got == pytest.approx(vector["seconds"]), vector["header"][:30]


@pytest.mark.parametrize("vector", [v for v in VECTORS if v["kind"] == "surface" and v["header"].isascii()], ids=lambda v: v["header"][:20])
def test_a_very_long_wait_is_surfaced_at_once_and_never_retried_early(vector: dict) -> None:
    hits: list[int] = []
    slept: list[float] = []

    def handler(request: httpx.Request) -> httpx.Response:
        hits.append(1)
        return httpx.Response(429, headers={"retry-after": vector["header"]}, json={"error": {"code": "rate_limited", "message": "slow"}})

    c = Client(KEY, "http://api.invalid", max_retries=2, transport=httpx.MockTransport(handler), sleep=slept.append)  # type: ignore[arg-type]
    with pytest.raises(Unavailable) as e:
        c.credits.balance("acme")
    assert len(hits) == 1, "a very long Retry-After was retried early"
    assert slept == []
    assert e.value.retry_after_seconds == pytest.approx(vector["seconds"])


def test_an_ignored_header_means_the_normal_backoff_and_a_bare_zero_does_too() -> None:
    # (non-ASCII digits are covered by the parser test above: an HTTP header cannot carry them through httpx's test transport)
    for header in ("abc", "0x1p3", "1d", "-3", "NaN", "", "Wed, 21 Oct 2026 07:28:00 GMT", "0"):
        slept: list[float] = []
        hits: list[int] = []

        def handler(request: httpx.Request) -> httpx.Response:
            hits.append(1)
            if len(hits) == 1:
                return httpx.Response(429, headers={"retry-after": header}, json={"error": {"code": "rate_limited", "message": "slow"}})
            return httpx.Response(200, json={"credits": 1, "consumed": 0, "granted": 1})

        c = Client(KEY, "http://api.invalid", max_retries=2, transport=httpx.MockTransport(handler), sleep=slept.append)  # type: ignore[arg-type]
        assert c.credits.balance("acme").remaining == 1
        assert len(hits) == 2 and len(slept) == 1 and 0.1 <= slept[0] <= 0.25, f"{header!r}: slept {slept}"


def test_an_idempotency_id_with_a_trailing_newline_is_refused() -> None:
    c = Client(KEY, "http://api.invalid", max_retries=0, transport=httpx.MockTransport(lambda r: httpx.Response(200, json={})))
    with pytest.raises(Exception) as e:
        c.usage.record("abcdefgh\n", "acme", "tokens", 1)
    assert "id" in str(e.value).lower()
