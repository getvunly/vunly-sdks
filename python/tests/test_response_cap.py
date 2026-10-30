"""The response body is capped at 16 MB (failure-modes.md FM-8, R-FM-2), as the Go and .NET SDKs do.

A broken or hostile server that never stops sending must cost the caller at most the cap plus a chunk, and must end in the SDK's
own error, not in memory exhaustion. These tests use a real local server, because only a real socket can stream without end.
"""

from __future__ import annotations

import asyncio
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Iterator

import pytest

from vunly import AsyncClient, Client, LimitReached, Unavailable
from vunly.client import MAX_RESPONSE_BYTES

KEY = "vk_test_synthetic_python_cap_000001"
CHUNK = b" " * (1 << 20)
HITS: list[str] = []
COMPLETED: list[str] = []


class _Endless(BaseHTTPRequestHandler):
    """Answers with `status` and then sends 64 MB of whitespace; remembers whether it managed to send it all."""

    status = 200

    def log_message(self, *args: object) -> None:
        pass

    def do_GET(self) -> None:
        HITS.append(self.path)
        self.send_response(self.status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(64 << 20))
        self.end_headers()
        try:
            for _ in range(64):
                self.wfile.write(CHUNK)
                self.wfile.flush()
            COMPLETED.append(self.path)
        except OSError:
            pass  # the client stopped reading: that is the point


@pytest.fixture()
def url() -> Iterator[str]:
    HITS.clear()
    COMPLETED.clear()
    _Endless.status = 200
    server = ThreadingHTTPServer(("127.0.0.1", 0), _Endless)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        yield f"http://127.0.0.1:{server.server_address[1]}"
    finally:
        server.shutdown()
        server.server_close()


def test_the_cap_is_16_mb() -> None:
    assert MAX_RESPONSE_BYTES == 16 * 1024 * 1024


def test_an_oversized_success_is_invalid_response_and_not_retried_by_the_sync_client(url: str) -> None:
    c = Client(KEY, url, timeout=20.0, max_retries=2, sleep=lambda s: None)
    with pytest.raises(Unavailable) as e:
        c.credits.balance("acme")
    assert e.value.code == "invalid_response"
    assert e.value.status == 200
    assert "16 MB" in str(e.value) and KEY not in str(e.value)
    assert len(HITS) == 1, "an oversized success is not retried"
    assert COMPLETED == [], "the client kept reading to the end of the body"


def test_an_oversized_success_is_invalid_response_by_the_async_client(url: str) -> None:
    async def go() -> None:
        c = AsyncClient(KEY, url, timeout=20.0, max_retries=2, sleep=lambda s: asyncio.sleep(0))
        try:
            with pytest.raises(Unavailable) as e:
                await c.credits.balance("acme")
            assert e.value.code == "invalid_response"
            assert e.value.status == 200
        finally:
            await c.aclose()

    asyncio.run(go())
    assert len(HITS) == 1
    assert COMPLETED == []


def test_an_oversized_error_body_is_still_classified_by_its_status(url: str) -> None:
    _Endless.status = 402
    c = Client(KEY, url, timeout=20.0, max_retries=0)
    with pytest.raises(LimitReached):
        c.credits.balance("acme")
    assert COMPLETED == []
