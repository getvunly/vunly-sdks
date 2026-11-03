"""Batchers: a buffer in front of ``usage.record_batch``. ``UsageBatcher`` (a background thread, for ``Client``) and
``AsyncUsageBatcher`` (a task, for ``AsyncClient``) share the I/O-free ``_core.BatchBuffer``.

``add`` returns the event's FINAL outcome (``accepted``, ``duplicate``, ``insufficient_credits``, ``too_late``,
``meter_not_found``, ``plan_limit_reached``, ``error``, or ``unavailable`` when it could not be recorded after ``max_attempts``
sends: it is then NOT recorded and is safe to send again later with the same id). It raises for an event that is invalid
(``InvalidRequest``, nothing queued), a batcher that is closed or full (``Unavailable``), and each queued event fails with the
error when the request as a whole failed for a reason a resend cannot fix (``AuthError``, ``InvalidRequest``, ``LimitReached``
such as a frozen project).
"""

from __future__ import annotations

import asyncio
import threading
import time
from concurrent.futures import Future
from typing import TYPE_CHECKING, Any, Mapping

from . import _core
from .errors import Unavailable, VunlyError
from .models import BatchEvent, BatchEventResult, BatchResponse

if TYPE_CHECKING:
    from .client import AsyncClient, Client

__all__ = ["UsageBatcher", "AsyncUsageBatcher"]


def _check_interval(flush_interval: float) -> float:
    if isinstance(flush_interval, bool) or not isinstance(flush_interval, (int, float)) or flush_interval < 0:
        raise ValueError("flush_interval must be 0 or more (seconds)")
    return float(flush_interval)


class UsageBatcher:
    """Thread-based batcher for ``Client``. ``add`` returns a ``concurrent.futures.Future`` of the final outcome."""

    def __init__(
        self,
        client: "Client",
        *,
        max_batch_size: int = 100,
        flush_interval: float = 1.0,
        max_pending: int = 10_000,
        max_attempts: int = 5,
    ) -> None:
        self._c = client
        self._buf = _core.BatchBuffer(max_batch_size, max_pending, max_attempts, client._retry)
        self._interval = _check_interval(flush_interval)
        self._cond = threading.Condition()
        self._closed = False
        self._flush = False
        self._first_at: float | None = None
        self._thread: threading.Thread | None = None

    @property
    def pending(self) -> int:
        """Events buffered or being sent."""
        with self._cond:
            return self._buf.pending

    def add(self, event: "BatchEvent | Mapping[str, Any]") -> "Future[BatchEventResult]":
        wire = _core.wire_event(event)
        with self._cond:
            if self._closed:
                raise Unavailable("unavailable", "the batcher is closed", 0)
            entry, new = self._buf.add(wire, Future)
            if new:
                if self._first_at is None:
                    self._first_at = time.monotonic()
                self._ensure_thread()
                self._cond.notify_all()
            return entry.waiter

    def flush(self) -> None:
        """Send everything buffered now, sending ``unavailable`` events again, and return when each has its final outcome."""
        with self._cond:
            if self._buf.pending == 0:
                return
            self._flush = True
            self._ensure_thread()
            self._cond.notify_all()
            while self._buf.pending > 0:
                self._cond.wait(0.05)
            self._flush = False

    def close(self) -> None:
        """Flush, stop the thread and refuse further events."""
        self.flush()
        with self._cond:
            self._closed = True
            self._cond.notify_all()
            thread = self._thread
        if thread is not None:
            thread.join()

    def __enter__(self) -> "UsageBatcher":
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    # -- the worker -------------------------------------------------------------------------------------------------

    def _ensure_thread(self) -> None:
        if self._thread is None or not self._thread.is_alive():
            self._thread = threading.Thread(target=self._run, name="vunly-usage-batcher", daemon=False)
            self._thread.start()

    def _due(self) -> float | None:
        """Seconds until the buffer must be sent, 0 when it must be sent now, None when nothing is waiting or only a flush ends it."""
        buf = self._buf
        if not buf.queue:
            return None
        if len(buf.queue) >= buf.size or self._flush or self._closed:
            return 0.0
        if self._interval == 0:
            return None
        assert self._first_at is not None
        return max(0.0, self._first_at + self._interval - time.monotonic())

    def _run(self) -> None:
        while True:
            with self._cond:
                while True:
                    if self._closed and not self._buf.queue and self._buf.in_flight == 0:
                        return
                    due = self._due()
                    if due == 0.0:
                        break
                    self._cond.wait(due)
                taken = self._buf.take()
                self._first_at = time.monotonic() if self._buf.queue else None
            self._send(taken)

    def _send(self, taken: list[_core.BatchEntry]) -> None:
        response: BatchResponse | None = None
        error: Exception | None = None
        try:
            response = self._c._do(_core.usage_record_batch([t.wire for t in taken]))
        except VunlyError as e:
            error = _core.batch_route_off(e) or e
        except Exception as e:  # never leave a caller waiting on a Future that nothing will resolve
            error = e
        with self._cond:
            done, again, retry_after = self._buf.apply(taken, response, error)
        self._resolve(done)
        wait: float | None = None
        if again:
            wait = self._buf.wait_for(again, retry_after)
            if wait is None:
                # a Retry-After longer than the SDK waits: surface them as not recorded, never sit on them
                self._resolve(self._buf.give_up(again))
                again = []
            else:
                self._c._sleep(wait)
        with self._cond:
            self._buf.finish(taken)
            if again:
                self._buf.requeue(again)
                if self._first_at is None:
                    self._first_at = time.monotonic()
            self._cond.notify_all()

    @staticmethod
    def _resolve(done: list[tuple[_core.BatchEntry, Any, bool]]) -> None:
        for entry, value, is_error in done:
            fut: Future[BatchEventResult] = entry.waiter
            if fut.done():
                continue
            if is_error:
                fut.set_exception(value)
            else:
                fut.set_result(value)


class AsyncUsageBatcher:
    """Asyncio batcher for ``AsyncClient``. ``await add(event)`` returns the event's final outcome."""

    def __init__(
        self,
        client: "AsyncClient",
        *,
        max_batch_size: int = 100,
        flush_interval: float = 1.0,
        max_pending: int = 10_000,
        max_attempts: int = 5,
    ) -> None:
        self._c = client
        self._buf = _core.BatchBuffer(max_batch_size, max_pending, max_attempts, client._retry)
        self._interval = _check_interval(flush_interval)
        self._closed = False
        self._flush = False
        self._first_at: float | None = None
        self._task: asyncio.Task[None] | None = None
        self._wake = asyncio.Event()
        self._idle = asyncio.Event()
        self._idle.set()

    @property
    def pending(self) -> int:
        return self._buf.pending

    async def add(self, event: "BatchEvent | Mapping[str, Any]") -> BatchEventResult:
        wire = _core.wire_event(event)
        if self._closed:
            raise Unavailable("unavailable", "the batcher is closed", 0)
        loop = asyncio.get_running_loop()
        entry, new = self._buf.add(wire, loop.create_future)
        if new:
            if self._first_at is None:
                self._first_at = time.monotonic()
            self._idle.clear()
            if self._task is None or self._task.done():
                self._task = loop.create_task(self._run())
            self._wake.set()
        return await asyncio.shield(entry.waiter)

    async def flush(self) -> None:
        if self._buf.pending == 0:
            return
        self._flush = True
        self._wake.set()
        try:
            await self._idle.wait()
        finally:
            self._flush = False

    async def aclose(self) -> None:
        await self.flush()
        self._closed = True
        self._wake.set()
        task = self._task
        if task is not None:
            await task

    async def __aenter__(self) -> "AsyncUsageBatcher":
        return self

    async def __aexit__(self, *exc: object) -> None:
        await self.aclose()

    def _due(self) -> float | None:
        buf = self._buf
        if not buf.queue:
            return None
        if len(buf.queue) >= buf.size or self._flush or self._closed:
            return 0.0
        if self._interval == 0:
            return None
        assert self._first_at is not None
        return max(0.0, self._first_at + self._interval - time.monotonic())

    async def _run(self) -> None:
        while True:
            while True:
                if self._closed and not self._buf.queue and self._buf.in_flight == 0:
                    return
                due = self._due()
                if due == 0.0:
                    break
                self._wake.clear()
                try:
                    await asyncio.wait_for(self._wake.wait(), due)
                except (asyncio.TimeoutError, TimeoutError):
                    pass
            taken = self._buf.take()
            self._first_at = time.monotonic() if self._buf.queue else None
            await self._send(taken)

    async def _send(self, taken: list[_core.BatchEntry]) -> None:
        response: BatchResponse | None = None
        error: Exception | None = None
        try:
            response = await self._c._do(_core.usage_record_batch([t.wire for t in taken]))
        except VunlyError as e:
            error = _core.batch_route_off(e) or e
        except Exception as e:  # never leave a caller waiting on a future that nothing will resolve
            error = e
        done, again, retry_after = self._buf.apply(taken, response, error)
        self._resolve(done)
        if again:
            wait = self._buf.wait_for(again, retry_after)
            if wait is None:
                self._resolve(self._buf.give_up(again))
                again = []
            else:
                await self._c._sleep(wait)
        self._buf.finish(taken)
        if again:
            self._buf.requeue(again)
            if self._first_at is None:
                self._first_at = time.monotonic()
        if self._buf.pending == 0:
            self._idle.set()

    @staticmethod
    def _resolve(done: list[tuple[_core.BatchEntry, Any, bool]]) -> None:
        for entry, value, is_error in done:
            fut: asyncio.Future[BatchEventResult] = entry.waiter
            if fut.done():
                continue
            if is_error:
                fut.set_exception(value)
            else:
                fut.set_result(value)
