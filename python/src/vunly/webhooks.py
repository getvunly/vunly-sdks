"""Receiving Vunly webhooks: verify a delivery's signature before you trust or parse it. No HTTP is involved.

    from vunly import WebhookError, verify_webhook

    try:
        event = verify_webhook(raw_body, request.headers.get("Vunly-Signature"), os.environ["VUNLY_WEBHOOK_SECRET"])
    except WebhookError:
        return 400  # do not process the payload

Pass the RAW request body (the exact bytes received), not parsed and re-encoded JSON.
"""

from __future__ import annotations

import hashlib
import hmac
import json
import re
import time
from dataclasses import dataclass
from typing import Any, Sequence, Union

#: How far a signed timestamp may be from now (either way), in seconds, before it counts as stale.
WEBHOOK_TOLERANCE_SECONDS = 300
#: The longest ``Vunly-Signature`` header considered (characters); a real one is a couple of hundred.
_MAX_HEADER = 4096
#: How many ``v1`` signatures are considered (two while rotating is the normal most).
_MAX_SIGNATURES = 10
#: How many signing secrets one call accepts.
_MAX_SECRETS = 10

_TIMESTAMP = re.compile(r"[0-9]{1,12}")
_SIGNATURE = re.compile(r"[0-9a-f]{64}")
# A secret needs at least one printable ASCII character that is not a space.
_PRINTABLE = re.compile(r"[\x21-\x7e]")


class WebhookError(Exception):
    """A delivery was rejected. ``reason`` is one of ``missing_header``, ``malformed_header``, ``stale_timestamp``,
    ``no_matching_signature`` or ``invalid_payload``. Answer 400 and do not process the payload.

    The message never contains the secret or the body.
    """

    def __init__(self, reason: str) -> None:
        super().__init__(f"webhook rejected: {reason}")
        self.reason = reason


@dataclass(frozen=True)
class WebhookEvent:
    """A verified delivery. ``id`` is stable across retries: keep the ids you have handled and skip repeats."""

    id: str
    type: str
    created_at: str | None
    data: Any


SecretInput = Union[str, Sequence[str]]


def _check_secrets(secrets: object) -> list[str]:
    """A blank secret would verify a signature anyone can compute, so it (or an empty list) is a configuration error."""
    items = [secrets] if isinstance(secrets, str) else list(secrets) if isinstance(secrets, (list, tuple)) else None
    if (
        items is None
        or not 1 <= len(items) <= _MAX_SECRETS
        or not all(isinstance(s, str) and _PRINTABLE.search(s) for s in items)
    ):
        raise ValueError(
            f"verify_webhook: secrets must be 1 to {_MAX_SECRETS} strings, each with a printable character "
            "(the endpoint's signing secrets); is the environment variable set?"
        )
    return items  # type: ignore[return-value]


def _parse_header(header: str) -> tuple[str | None, list[str]]:
    timestamp: str | None = None
    signatures: list[str] = []
    for part in header.split(","):
        key, eq, value = part.partition("=")
        if not eq:
            continue
        key, value = key.strip(), value.strip()
        if key == "t" and _TIMESTAMP.fullmatch(value):
            timestamp = value
        elif key == "v1" and _SIGNATURE.fullmatch(value) and len(signatures) < _MAX_SIGNATURES:
            signatures.append(value)
    return timestamp, signatures


def verify_webhook(
    payload: bytes | bytearray | memoryview | str,
    signature_header: str | None,
    secrets: SecretInput,
    now: float | None = None,
) -> WebhookEvent:
    """Check a delivery and return the event, or raise.

    ``payload`` is the RAW request body (the exact bytes received; a ``str`` is encoded as UTF-8). ``signature_header`` is the
    ``Vunly-Signature`` header, ``t=<unix>,v1=<hex>``. ``secrets`` is the endpoint's signing secret, or a list of them while
    you rotate: a match with any one is enough. The signature is HMAC-SHA256 over ``"<t>." + payload``, compared in constant
    time; ``t`` must be within five minutes of now, either way. ``now`` (seconds since the epoch) is for tests.

    Raises ``WebhookError`` when the delivery is rejected (answer 400), and ``ValueError`` when no usable secret is given
    (checked first: a configuration error, not a bad delivery) or the payload is not bytes or text.
    """
    secret_list = _check_secrets(secrets)
    if isinstance(payload, str):
        body = payload.encode("utf-8")
    elif isinstance(payload, (bytes, bytearray, memoryview)):
        body = bytes(payload) if not isinstance(payload, bytes) else payload
    else:
        raise ValueError("verify_webhook: payload must be the raw request body (bytes or str)")

    if signature_header is None or not isinstance(signature_header, str) or not signature_header.strip():
        raise WebhookError("missing_header")
    if len(signature_header) > _MAX_HEADER:
        raise WebhookError("malformed_header")
    timestamp, signatures = _parse_header(signature_header)
    if timestamp is None or not signatures:
        raise WebhookError("malformed_header")
    current = int(time.time() if now is None else now)
    if abs(current - int(timestamp)) > WEBHOOK_TOLERANCE_SECONDS:
        raise WebhookError("stale_timestamp")

    prefix = f"{timestamp}.".encode("ascii")
    matched = False
    for secret in secret_list:
        mac = hmac.new(secret.encode("utf-8"), digestmod=hashlib.sha256)
        mac.update(prefix)  # the body is fed as it is, not copied behind the timestamp
        mac.update(body)
        wanted = mac.hexdigest()
        for signature in signatures:  # every candidate is compared: no early exit on a match
            if hmac.compare_digest(signature, wanted):
                matched = True
    if not matched:
        raise WebhookError("no_matching_signature")
    return _event(body)


def _event(body: bytes) -> WebhookEvent:
    try:
        data = json.loads(body.decode("utf-8"))
    except (UnicodeDecodeError, ValueError, RecursionError):
        raise WebhookError("invalid_payload") from None
    if (
        not isinstance(data, dict)
        or not isinstance(data.get("id"), str)
        or not data["id"]
        or not isinstance(data.get("type"), str)
        or not data["type"]
    ):
        raise WebhookError("invalid_payload")
    created = data.get("createdAt")
    return WebhookEvent(
        id=data["id"], type=data["type"], created_at=created if isinstance(created, str) else None, data=data.get("data")
    )
