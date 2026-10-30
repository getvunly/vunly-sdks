"""The `vunly` CLI must treat a malformed GET /v1/meters as a setup problem (exit 2 for verify, a FAIL row for
doctor), never a crash, and must never echo a key passed as `--api-key=<secret>`. These fail without the fixes.
"""

from __future__ import annotations

import httpx
import pytest

from vunly import cli

SECRET = "vk_test_synthetic_leak_0001"  # a fake key marked synthetic; must never reach stderr


def _resp(**kw: object) -> httpx.Response:
    return httpx.Response(request=httpx.Request("GET", "http://api.invalid/v1/meters"), **kw)  # type: ignore[arg-type]


@pytest.mark.parametrize(
    "resp",
    [
        _resp(status_code=200, content=b"<html>proxy</html>"),  # non-JSON
        _resp(status_code=200, json={}),  # empty object (the regressed case)
        _resp(status_code=200, json=None, content=b"null", headers={"content-type": "application/json"}),
        _resp(status_code=200, json=[]),  # array, not {data: [...]}
        _resp(status_code=200, json={"data": 5}),  # data not a list
    ],
    ids=["non-JSON", "empty-object", "null", "array", "data-not-list"],
)
def test_find_meter_malformed_is_setup_problem(resp: httpx.Response, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(cli, "_get", lambda base, key, path: resp)
    with pytest.raises(cli.SetupProblem):
        cli._find_meter("http://api.invalid", SECRET, None)


def test_doctor_handles_empty_meters_body_without_crashing(monkeypatch: pytest.MonkeyPatch) -> None:
    def fake_get(url: str, **_kw: object) -> httpx.Response:
        req = httpx.Request("GET", url)
        if url.endswith("/v1/health"):
            return httpx.Response(200, json={"status": "ok"}, request=req)
        if "/v1/meters" in url:
            return httpx.Response(200, json={}, request=req)  # the regressed empty-object case
        return httpx.Response(200, json={"data": []}, request=req)  # /v1/customers

    monkeypatch.setenv("VUNLY_API_URL", "http://api.invalid")
    monkeypatch.setenv("VUNLY_API_KEY", SECRET)
    monkeypatch.setattr(httpx, "get", fake_get)
    import argparse

    code = cli.cmd_doctor(argparse.Namespace())  # must not raise KeyError
    assert code in (0, 1)


def test_misused_api_key_flag_is_not_echoed(capsys: pytest.CaptureFixture[str]) -> None:
    # the equals form packs the secret into one token; the CLI must print the flag name only
    code = cli.main(["verify", f"--api-key={SECRET}"])
    err = capsys.readouterr().err
    assert code == 2
    assert SECRET not in err, f"the key leaked to stderr: {err!r}"
    assert "--api-key" in err  # the flag name is fine to show
    # the space form too (regression guard)
    code2 = cli.main(["verify", "--api-key", SECRET])
    assert code2 == 2 and SECRET not in capsys.readouterr().err


@pytest.mark.parametrize("date", ["Mon, 01 Jan 2026 00:00:00 -0000", "Mon, 01 Jan 2026 00:00:00 GMT"])
def test_doctor_date_header_naive_zone_does_not_crash(date: str, monkeypatch: pytest.MonkeyPatch) -> None:
    # A `Date` with a numeric `-0000` zone parses as a naive datetime; subtracting an aware now() used to TypeError
    # and crash `vunly doctor`. It must report a Clock row instead.
    def fake_get(url: str, **_kw: object) -> httpx.Response:
        req = httpx.Request("GET", url)
        if url.endswith("/v1/health"):
            return httpx.Response(200, json={"status": "ok"}, headers={"date": date}, request=req)
        if "/v1/meters" in url:
            return httpx.Response(200, json={"data": [{"key": "tokens", "unitsPerCredit": 1}]}, request=req)
        return httpx.Response(200, json={"data": []}, request=req)

    monkeypatch.setenv("VUNLY_API_URL", "http://api.invalid")
    monkeypatch.setenv("VUNLY_API_KEY", SECRET)
    monkeypatch.setattr(httpx, "get", fake_get)
    import argparse

    assert cli.cmd_doctor(argparse.Namespace()) in (0, 1)  # must not raise TypeError
