"""Typed errors. Same classes, same meaning as the TypeScript SDK.

``code`` is the API's stable machine code; ``request_id`` helps support; ``message`` never contains your key.
"""

from __future__ import annotations

from typing import Any, Mapping


class VunlyError(Exception):
    """Base class of every error the SDK raises."""

    def __init__(
        self,
        code: str,
        message: str,
        status: int,
        request_id: str | None = None,
        details: Mapping[str, Any] | None = None,
    ) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.status = status
        self.request_id = request_id
        self.details = dict(details) if details else None

    def __str__(self) -> str:
        rid = f" [request {self.request_id}]" if self.request_id else ""
        return f"{self.code} ({self.status}): {self.message}{rid}"


class AuthError(VunlyError):
    """401/403: missing, wrong or revoked key, or the key lacks a scope. Never retried."""


class LimitReached(VunlyError):
    """402: the customer has too few credits. Do not run the work. ``details`` has ``balance`` and ``required``. Never retried."""


class InvalidRequest(VunlyError):
    """400/404/409: fix the request (bad fields, unknown customer, an id reused with different data). Never retried."""


class Unavailable(VunlyError):
    """A network failure, a timeout, 429 or 5xx that survived the SDK's retries. Safe to retry with the SAME id.

    ``retry_after_seconds`` is the server's ``Retry-After`` in seconds (capped at one day) when it sent a number, else None.
    A value above 30 s is not waited for by the SDK: it is surfaced here and the caller decides.
    """

    retry_after_seconds: float | None = None


def classify(status: int) -> type[VunlyError]:
    if status == 402:
        return LimitReached
    if status in (401, 403):
        return AuthError
    if status == 429 or status >= 500:
        return Unavailable
    return InvalidRequest
