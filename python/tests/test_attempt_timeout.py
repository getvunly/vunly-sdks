"""One attempt is bounded as a WHOLE (failure-modes.md FM-8, R-FM-3).

httpx applies a single `timeout` number to each phase (connect, read, write, pool) separately, so a server that answers at
once and then drips its body one byte at a time never trips the read timeout and holds a plain request open for as long as it
keeps dripping. The SDK promises "10 s per attempt" (conformance/README.md), as the other four SDKs
keep. These tests use a real local server, because only a real socket can drip.
"""

from __future__ import annotations

import asyncio
import gzip
import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Iterator

import httpx
import pytest

from vunly import AsyncClient, Client, Unavailable

KEY = "vk_test_synthetic_python_attempt_0001"
BODY = json.dumps({"credits": 1, "consumed": 0, "granted": 1}).encode().ljust(30)


class _Drip(BaseHTTPRequestHandler):
    """200 at once, then `interval` seconds between single bytes of the body."""

    interval = 0.1

    def log_message(self, *args: object) -> None:  # silence
        pass

    def do_GET(self) -> None:
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(BODY)))
        self.end_headers()
        try:
            for i in range(len(BODY)):
                self.wfile.write(BODY[i : i + 1])
                self.wfile.flush()
                time.sleep(self.interval)
        except OSError:
            pass  # the client gave up: that is the point


@pytest.fixture()
def drip_url() -> Iterator[str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Drip)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        yield f"http://127.0.0.1:{server.server_address[1]}"
    finally:
        server.shutdown()
        server.server_close()


def test_a_dripping_body_is_cut_at_the_attempt_timeout_by_the_sync_client(drip_url: str) -> None:
    c = Client(KEY, drip_url, timeout=0.4, max_retries=0)
    started = time.monotonic()
    with pytest.raises(Unavailable) as e:
        c.credits.balance("acme")
    elapsed = time.monotonic() - started
    # the body would take 3 s to arrive; the attempt is allowed 0.4 s, plus at most one gap between two bytes
    assert elapsed < 1.2, f"the attempt took {elapsed:.2f} s against a 0.4 s timeout"
    assert e.value.status == 0
    assert KEY not in str(e.value)


def test_a_dripping_body_is_cut_at_the_attempt_timeout_by_the_async_client(drip_url: str) -> None:
    async def go() -> float:
        c = AsyncClient(KEY, drip_url, timeout=0.4, max_retries=0)
        started = time.monotonic()
        try:
            with pytest.raises(Unavailable) as e:
                await c.credits.balance("acme")
            assert e.value.status == 0
        finally:
            await c.aclose()
        return time.monotonic() - started

    elapsed = asyncio.run(go())
    assert elapsed < 1.2, f"the attempt took {elapsed:.2f} s against a 0.4 s timeout"


def test_a_slow_but_in_time_answer_still_succeeds(drip_url: str) -> None:
    _Drip.interval = 0.005  # 30 bytes in about 0.15 s
    try:
        assert Client(KEY, drip_url, timeout=2.0, max_retries=0).credits.balance("acme").remaining == 1
        assert asyncio.run(_async_balance(drip_url)) == 1
    finally:
        _Drip.interval = 0.1


async def _async_balance(url: str) -> int:
    c = AsyncClient(KEY, url, timeout=2.0, max_retries=0)
    try:
        return (await c.credits.balance("acme")).remaining
    finally:
        await c.aclose()


def test_a_timed_out_attempt_is_retried_like_a_lost_answer_with_the_same_id(drip_url: str) -> None:
    # reads are retried after a timeout: 1 attempt + 2 retries, each cut at the deadline, and the call still ends in Unavailable
    c = Client(KEY, drip_url, timeout=0.2, max_retries=2, sleep=lambda s: None)
    started = time.monotonic()
    with pytest.raises(Unavailable):
        c.credits.balance("acme")
    assert time.monotonic() - started < 2.5


def test_a_gzip_encoded_answer_still_decodes_through_the_bounded_read() -> None:
    body = gzip.compress(json.dumps({"credits": 7, "consumed": 3, "granted": 10}).encode())

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, content=body, headers={"content-encoding": "gzip", "content-type": "application/json"})

    c = Client(KEY, "http://api.invalid", transport=httpx.MockTransport(handler))
    b = c.credits.balance("acme")
    assert (b.remaining, b.consumed, b.granted) == (7, 3, 10)


def test_an_error_status_keeps_its_class_through_the_bounded_read() -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(402, json={"error": {"code": "insufficient_credits", "message": "m", "requestId": "r1", "details": {"balance": 2}}})

    from vunly import LimitReached

    c = Client(KEY, "http://api.invalid", transport=httpx.MockTransport(handler), max_retries=0)
    with pytest.raises(LimitReached) as e:
        c.usage.record(id="req-abc-12345", customer="a", meter="tokens", quantity=1)
    assert e.value.request_id == "r1"
