"""verify_webhook: the shared vectors (webhook-vectors.json, copied here so the package tests stand alone) and the hardening rules."""

from __future__ import annotations

import hashlib
import hmac
import json
from pathlib import Path

import pytest

import vunly
from vunly import WebhookError, verify_webhook

DOC = json.loads((Path(__file__).parent / "webhook-vectors.json").read_text(encoding="utf-8"))
VECTORS = DOC["vectors"]
NOW = DOC["now"]
SECRET = "whsec_synthetic_vector"
BODY = '{"id":"evt_1","type":"credits.low","createdAt":"2026-10-28T10:00:00.000Z","data":{"n":1}}'


def sign(secret: str, timestamp: int, body: bytes | str = BODY) -> str:
    raw = body.encode("utf-8") if isinstance(body, str) else body
    digest = hmac.new(secret.encode(), f"{timestamp}.".encode() + raw, hashlib.sha256).hexdigest()
    return f"t={timestamp},v1={digest}"


def reason(header: str | None, secrets=SECRET, body: bytes | str = BODY, now: float = NOW) -> str | None:
    try:
        verify_webhook(body, header, secrets, now=now)
    except WebhookError as e:
        return e.reason
    return None


def test_there_are_enough_vectors() -> None:
    assert len(VECTORS) >= 20
    assert {v["expect"] for v in VECTORS} >= {
        "ok",
        "config",
        "missing_header",
        "malformed_header",
        "stale_timestamp",
        "no_matching_signature",
        "invalid_payload",
    }


@pytest.mark.parametrize("vector", VECTORS, ids=lambda v: v["name"])
def test_every_shared_vector(vector: dict) -> None:
    args = (vector["payload"], vector["header"], vector["secrets"])
    if vector["expect"] == "ok":
        event = verify_webhook(*args, now=DOC["now"])
        assert event.id and event.type
    elif vector["expect"] == "config":
        with pytest.raises(ValueError):
            verify_webhook(*args, now=DOC["now"])
    else:
        with pytest.raises(WebhookError) as caught:
            verify_webhook(*args, now=DOC["now"])
        assert caught.value.reason == vector["expect"]


def test_the_event_is_returned() -> None:
    event = verify_webhook(BODY, sign(SECRET, NOW), SECRET, now=NOW)
    assert (event.id, event.type, event.created_at, event.data) == ("evt_1", "credits.low", "2026-10-28T10:00:00.000Z", {"n": 1})


def test_it_is_exported() -> None:
    assert vunly.verify_webhook is verify_webhook
    assert {"verify_webhook", "WebhookError", "WebhookEvent"} <= set(vunly.__all__)


def test_the_tolerance_is_five_minutes_either_way_to_the_second() -> None:
    assert reason(sign(SECRET, NOW - 300)) is None
    assert reason(sign(SECRET, NOW + 300)) is None
    assert reason(sign(SECRET, NOW - 301)) == "stale_timestamp"
    assert reason(sign(SECRET, NOW + 301)) == "stale_timestamp"


def test_the_clock_defaults_to_now(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("vunly.webhooks.time.time", lambda: float(NOW))
    assert verify_webhook(BODY, sign(SECRET, NOW), SECRET).id == "evt_1"


def test_raw_bytes_and_text_are_the_same_payload() -> None:
    header = sign(SECRET, NOW)
    for body in (BODY.encode(), bytearray(BODY.encode()), memoryview(BODY.encode()), BODY):
        assert verify_webhook(body, header, SECRET, now=NOW).id == "evt_1"
    assert reason(header, body=BODY + " ") == "no_matching_signature"  # re-encoded or padded bodies do not verify


def test_the_payload_must_be_bytes_or_text() -> None:
    with pytest.raises(ValueError):
        verify_webhook({"id": "x"}, sign(SECRET, NOW), SECRET, now=NOW)  # type: ignore[arg-type]


def test_a_non_utf8_body_signed_correctly_is_an_invalid_payload() -> None:
    body = b'{"id":"a","type":"b","x":"\xff"}'
    assert reason(sign(SECRET, NOW, body), body=body) == "invalid_payload"


@pytest.mark.parametrize("bad", ["not json", "[]", '{"id":"","type":"x"}', '{"id":"a","type":3}', '{"id":1,"type":"x"}'])
def test_invalid_payloads_with_a_good_signature(bad: str) -> None:
    assert reason(sign(SECRET, NOW, bad), body=bad) == "invalid_payload"


def test_rotation_accepts_either_secret_and_rejects_others() -> None:
    assert reason(sign("whsec_synthetic_old", NOW), secrets=[SECRET, "whsec_synthetic_old"]) is None
    assert reason(sign(SECRET, NOW), secrets=("whsec_synthetic_old", SECRET)) is None
    assert reason(sign("whsec_synthetic_third", NOW), secrets=[SECRET, "whsec_synthetic_old"]) == "no_matching_signature"


@pytest.mark.parametrize(
    "secrets",
    [
        "",
        "   ",
        "\t\n",
        " 　",
        "\x00\x01",
        [],
        (),
        [""],
        [SECRET, ""],
        ["", SECRET],
        [SECRET, "  "],
        None,
        b"whsec_synthetic_bytes",
        [SECRET, None],
        [SECRET, b"x"],
        ["whsec_synthetic_x"] * 11,
        {SECRET},
    ],
)
def test_a_missing_or_blank_secret_is_a_configuration_error_before_the_signature_is_looked_at(secrets) -> None:
    # the header is garbage and the clock is far off: only the secret check can be what raises, and it is not a WebhookError
    for header in ("garbage", None, "", sign("", 1)):
        with pytest.raises(ValueError) as caught:
            verify_webhook(BODY, header, secrets, now=NOW)
        assert not isinstance(caught.value, WebhookError)


def test_the_signature_is_not_accepted_for_an_empty_secret_even_when_it_matches() -> None:
    forged = sign("", NOW)  # anyone can compute this
    with pytest.raises(ValueError):
        verify_webhook(BODY, forged, "", now=NOW)


def test_header_failures() -> None:
    good = sign(SECRET, NOW)
    digest = good.split("v1=")[1]
    assert reason(None) == "missing_header"
    assert reason("") == "missing_header"
    assert reason("  \t ") == "missing_header"
    assert reason("garbage") == "malformed_header"
    assert reason(f"t=abc,v1={digest}") == "malformed_header"
    assert reason(f"t=-5,v1={digest}") == "malformed_header"
    assert reason(f"t={NOW}") == "malformed_header"
    assert reason(f"t={NOW},v1=00") == "malformed_header"
    assert reason(f"t={NOW},v1={digest.upper()}") == "malformed_header"
    assert reason(good + "," + "x" * 5000) == "malformed_header"
    assert reason(12345) == "missing_header"  # type: ignore[arg-type]
    assert reason(f"t={NOW}, v1={digest}") is None  # spaces around the parts are fine
    assert reason(f"v1={digest},t={NOW}") is None  # order does not matter
    assert reason(f"t={NOW},v1={digest},v0=junk,flag") is None  # unknown parts are ignored


def test_the_signed_payload_is_timestamp_dot_body() -> None:
    other = hmac.new(SECRET.encode(), f"{NOW}{BODY}".encode(), hashlib.sha256).hexdigest()  # no dot
    assert reason(f"t={NOW},v1={other}") == "no_matching_signature"
    swapped = hmac.new(SECRET.encode(), f"{BODY}.{NOW}".encode(), hashlib.sha256).hexdigest()
    assert reason(f"t={NOW},v1={swapped}") == "no_matching_signature"


def test_every_candidate_signature_is_compared_in_constant_time(monkeypatch: pytest.MonkeyPatch) -> None:
    calls: list[tuple[str, str]] = []
    real = hmac.compare_digest

    def spy(a, b):  # type: ignore[no-untyped-def]
        calls.append((a, b))
        return real(a, b)

    monkeypatch.setattr("vunly.webhooks.hmac.compare_digest", spy)
    good = sign(SECRET, NOW)
    digest = good.split("v1=")[1]
    # the match is first, a second candidate and a second secret still get compared: no early exit
    assert reason(f"t={NOW},v1={digest},v1={'0' * 64}", secrets=[SECRET, "whsec_synthetic_old"]) is None
    assert len(calls) == 4


def test_the_body_is_not_copied_behind_the_timestamp(monkeypatch: pytest.MonkeyPatch) -> None:
    fed: list[bytes] = []
    real_new = hmac.new

    class Spy:
        def __init__(self, mac) -> None:  # type: ignore[no-untyped-def]
            self.mac = mac

        def update(self, data) -> None:  # type: ignore[no-untyped-def]
            fed.append(bytes(data))
            self.mac.update(data)

        def hexdigest(self) -> str:
            return self.mac.hexdigest()

    monkeypatch.setattr("vunly.webhooks.hmac.new", lambda *a, **k: Spy(real_new(*a, **k)))
    assert reason(sign(SECRET, NOW)) is None
    assert fed == [f"{NOW}.".encode(), BODY.encode()]


def test_errors_do_not_contain_the_secret_or_the_body() -> None:
    for call in (
        lambda: verify_webhook(BODY, sign("other", NOW), "whsec_synthetic_leak", now=NOW),
        lambda: verify_webhook(BODY, None, "whsec_synthetic_leak", now=NOW),
        lambda: verify_webhook(BODY, "garbage", "whsec_synthetic_leak", now=NOW),
    ):
        with pytest.raises(WebhookError) as caught:
            call()
        assert "whsec_synthetic_leak" not in str(caught.value) + repr(caught.value)
        assert "evt_1" not in str(caught.value)
    with pytest.raises(ValueError) as cfg:
        verify_webhook(BODY, "garbage", ["whsec_synthetic_leak", ""], now=NOW)
    assert "whsec_synthetic_leak" not in str(cfg.value)
