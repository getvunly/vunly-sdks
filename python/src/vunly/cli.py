"""The ``vunly`` command: ``vunly verify``, ``vunly doctor``, ``vunly init``.

Same behaviour as the Node command of the TypeScript SDK (``npx vunly verify``), for projects without Node.

    export VUNLY_API_URL=...      the API address from the setup step (with or without /v1)
    export VUNLY_API_KEY=...      your sandbox key (vk_test_...); read from the environment ONLY

The key is never printed, never written to a file and never accepted as an argument. Exit codes: 0 everything passed,
1 a check failed, 2 the command could not run (a setup problem).
"""

from __future__ import annotations

import argparse
import os
import platform
import secrets
import sys
from pathlib import Path
from typing import Callable
from urllib.parse import urlparse

import httpx

from . import __version__
from ._core import normalize_base_url
from .client import Client
from .errors import AuthError, LimitReached, Unavailable, VunlyError

REPORT = "vunly-verify-report.md"


class SetupProblem(Exception):
    """The command cannot run: tell the user what to fix (exit code 2)."""


def _env() -> tuple[str, str]:
    base = normalize_base_url(os.environ.get("VUNLY_API_URL", ""))
    key = os.environ.get("VUNLY_API_KEY", "")
    if not base:
        raise SetupProblem("VUNLY_API_URL is not set (copy it from the setup step).")
    if not key:
        raise SetupProblem("VUNLY_API_KEY is not set (copy it from the setup step).")
    if not key.startswith("vk_test_"):
        raise SetupProblem("this command only runs with a sandbox key (vk_test_...). No other key is accepted.")
    return base, key


def _host(base: str) -> str:
    return urlparse(base).netloc or "(invalid url)"


def _get(base: str, key: str, path: str) -> httpx.Response:
    return httpx.get(f"{base}{path}", headers={"authorization": f"Bearer {key}"}, timeout=10)


# ---- verify ---------------------------------------------------------------------------------------------------------
def _find_meter(base: str, key: str, wanted: str | None) -> tuple[str, int]:
    try:
        res = _get(base, key, "/v1/meters")
    except httpx.HTTPError as exc:
        raise SetupProblem(f"cannot reach {base} ({type(exc).__name__}).") from None
    if res.status_code in (401, 403):
        raise SetupProblem(f"the API refused the key (HTTP {res.status_code}). Use the sandbox key from the setup step.")
    if res.status_code == 404:
        raise SetupProblem(
            "GET /v1/meters answered HTTP 404 (no such route). VUNLY_API_URL is probably not the API address: use the "
            f"address from the setup step, for example https://host (a trailing /v1 is accepted). Tried {base}/v1/meters."
        )
    if not res.is_success:
        raise SetupProblem(f"GET /v1/meters answered HTTP {res.status_code}.")
    try:
        data = res.json()
        meters = data.get("data") if isinstance(data, dict) else None  # .get: an empty object must not KeyError
    except ValueError:
        meters = None
    if not isinstance(meters, list) or not all(isinstance(m, dict) and "key" in m for m in meters):
        raise SetupProblem("GET /v1/meters answered with a body this tool did not understand.")
    if wanted:
        found = [m for m in meters if m["key"] == wanted]
        if not found:
            raise SetupProblem(f'the project has no meter "{wanted}" (it has: {", ".join(m["key"] for m in meters) or "none"}).')
    elif len(meters) == 1:
        found = meters
    else:
        have = "no meter yet" if not meters else "these meters: " + ", ".join(m["key"] for m in meters)
        raise SetupProblem(f"pass --meter <key>; the project has {have}.")
    return found[0]["key"], int(found[0]["unitsPerCredit"])


def cmd_verify(args: argparse.Namespace) -> int:
    base, key = _env()
    meter, per = _find_meter(base, key, args.meter)
    vunly = Client(api_key=key, base_url=base)
    run = secrets.token_hex(4)
    customer = f"vunly-verify-{run}"
    grant = 5
    results: list[tuple[str, str, bool, str]] = []

    def balance() -> int:
        return vunly.credits.balance(customer).remaining

    def check(id_: str, name: str, fn: Callable[[], str]) -> None:
        try:
            results.append((id_, name, True, fn()))
        except AssertionError as exc:
            results.append((id_, name, False, str(exc)))
        except AuthError as exc:
            results.append((id_, name, False, f"{exc.message} (the key was refused or lacks a scope: use the key from the setup step)"))
        except VunlyError as exc:
            results.append((id_, name, False, exc.message))

    def v1() -> str:
        vunly.customers.upsert(customer)
        vunly.customers.upsert(customer)
        assert balance() == 0, "a new customer should start at 0 credits"
        return f"customer {customer} created twice, one customer, balance 0"

    def v2() -> str:
        gid = f"verify-grant-{run}"
        vunly.credits.grant(id=gid, customer=customer, credits=grant)
        vunly.credits.grant(id=gid, customer=customer, credits=grant)
        b = balance()
        assert b == grant, f"balance {b} after granting twice with one id, expected {grant}"
        return f"granted {grant} credits twice with one id: balance {b}"

    def v3() -> str:
        uid = f"verify-use-{run}"
        first = vunly.usage.record(id=uid, customer=customer, meter=meter, quantity=per)
        again = vunly.usage.record(id=uid, customer=customer, meter=meter, quantity=per)
        b = balance()
        assert b == grant - 1, f"balance {b} after recording the same id twice, expected {grant - 1}"
        assert again.replayed and not first.replayed, "the second record was not reported as a replay"
        return f"1 credit ({per} {meter}) recorded twice with one id: balance {b}, second call reported as replay"

    def v4() -> str:
        before = balance()
        too_much = (before + 1) * per
        try:
            vunly.usage.record(id=f"verify-over-{run}", customer=customer, meter=meter, quantity=too_much)
        except LimitReached:
            pass
        else:
            raise AssertionError("usage above the balance was accepted")
        assert balance() == before, "a refused record changed the balance"
        return f"{too_much} {meter} against {before} credits: LimitReached (402), balance still {before}"

    def v5() -> str:
        down = Client(api_key=key, base_url="http://127.0.0.1:9", max_retries=0)
        try:
            down.entitlements.check(customer, "any", {"meter": meter, "quantity": 1})
        except Unavailable:
            return "with the API unreachable, entitlements.check raises Unavailable: your code must not run the work"
        raise AssertionError("an unreachable API did not raise")

    check("V1", "Customers are created idempotently", v1)
    check("V2", "A top-up with the same id is applied once", v2)
    check("V3", "Allowed usage is counted once; a replay does not double count", v3)
    check("V4", "Over-limit usage is rejected and nothing is counted", v4)
    check("V5", "An unreachable Vunly fails closed", v5)

    failed = [r for r in results if not r[2]]
    for id_, name, ok, detail in results:
        print(f"{'PASS' if ok else 'FAIL'} {id_} {name}\n     {detail}")
    summary = "Summary: " + ", ".join(f"{i} {'PASS' if ok else 'FAIL'}" for i, _, ok, _ in results)
    result = f"Result: {len(results) - len(failed)} of {len(results)} checks passed. Sandbox only; no real payments were made."
    print(f"\n{summary}\n{result}")
    lines = [
        "# Vunly sandbox verification",
        "",
        f"API: {_host(base)} · meter: {meter} · customer: {customer} · key: sandbox (not recorded)",
        "",
        *[f"- **{'PASS' if ok else 'FAIL'} {i}** {n}: {d}" for i, n, ok, d in results],
        "",
        result,
        "",
    ]
    Path(REPORT).write_text("\n".join(lines))
    return 1 if failed else 0


# ---- doctor ---------------------------------------------------------------------------------------------------------
def cmd_doctor(_: argparse.Namespace) -> int:
    rows: list[tuple[str, str, str]] = []  # (PASS|WARN|FAIL, name, detail)

    def add(status: str, name: str, detail: str) -> None:
        rows.append((status, name, detail))

    py = sys.version_info
    add("PASS" if py >= (3, 10) else "FAIL", "Python", f"{platform.python_version()} (needs 3.10 or newer); vunly {__version__}")

    raw_url = os.environ.get("VUNLY_API_URL", "")
    key = os.environ.get("VUNLY_API_KEY", "")
    if not raw_url:
        add("FAIL", "VUNLY_API_URL", "not set: copy the API address from the setup step")
    if not key:
        add("FAIL", "VUNLY_API_KEY", "not set: copy your sandbox key from the setup step (never write it into a file)")
    elif not key.startswith("vk_test_"):
        add("FAIL", "VUNLY_API_KEY", "is set but is not a sandbox key (vk_test_...): this tool only runs with a sandbox key")
    else:
        add("PASS", "VUNLY_API_KEY", "set, sandbox key (value not shown)")
    if raw_url:
        base = normalize_base_url(raw_url)
        note = " (the trailing /v1 is accepted and ignored)" if raw_url.strip().rstrip("/").lower().endswith("/v1") else ""
        add("PASS", "VUNLY_API_URL", f"{_host(base)}{note}")
        try:
            health = httpx.get(f"{base}/v1/health", timeout=10)
            detail = f"GET /v1/health answered HTTP {health.status_code}"
            if health.status_code == 404:
                detail += (
                    " (no such route): VUNLY_API_URL is probably not the API address; use the address from the setup step "
                    "(a trailing /v1 is accepted)"
                )
            add("PASS" if health.is_success else "FAIL", "API reachable", detail)
            date = health.headers.get("date")
            if date:
                from email.utils import parsedate_to_datetime
                from datetime import datetime, timezone

                try:
                    served = parsedate_to_datetime(date)
                    # A `Date` with a numeric `-0000` zone parses as naive ("unknown local offset"); treat it as UTC
                    # so the subtraction below is not offset-naive vs offset-aware (a TypeError that would crash doctor).
                    if served.tzinfo is None:
                        served = served.replace(tzinfo=timezone.utc)
                    skew = abs((datetime.now(timezone.utc) - served).total_seconds())
                    add(
                        "PASS" if skew < 60 else "WARN",
                        "Clock",
                        f"{skew:.0f} s from the API's clock" + ("" if skew < 60 else " (token lifetimes depend on it)"),
                    )
                except (TypeError, ValueError):
                    add("WARN", "Clock", f"could not read the API's Date header ({date!r})")
        except httpx.HTTPError as exc:
            add("FAIL", "API reachable", f"cannot reach {_host(base)} ({type(exc).__name__}); check VUNLY_API_URL and the network")
            base = ""
        if base and key.startswith("vk_test_"):
            for scope, path in (("plans:read", "/v1/meters"), ("customers:read", "/v1/customers?limit=1")):
                try:
                    r = _get(base, key, path)
                except httpx.HTTPError:
                    continue
                if r.status_code == 401:
                    add("FAIL", f"Key accepted ({scope})", "the API does not know this key (401): use the sandbox key from the setup step")
                elif r.status_code == 403:
                    add("WARN", f"Scope {scope}", "the key lacks it (403): `vunly verify` needs it")
                elif r.status_code == 404 and path == "/v1/meters":
                    add("FAIL", "API address", "answered 404 (no such route): VUNLY_API_URL is probably not the API address")
                elif r.is_success:
                    add("PASS", f"Scope {scope}", "ok")
                    if path == "/v1/meters":
                        try:
                            body = r.json()
                            meters = body.get("data") if isinstance(body, dict) else None  # .get: {} must not KeyError
                        except ValueError:
                            meters = None
                        if not isinstance(meters, list):
                            add("FAIL", "Meters", "the API answered /v1/meters with a body this tool did not understand")
                        else:
                            n = len(meters)
                            add("PASS" if n else "WARN", "Meters", f"{n} configured" + ("" if n else ": create one (or apply a recipe) before `vunly verify`"))
    width = max(len(n) for _, n, _ in rows)
    for status, name, detail in rows:
        print(f"{status:<4} {name:<{width}}  {detail}")
    bad = sum(s == "FAIL" for s, _, _ in rows)
    warn = sum(s == "WARN" for s, _, _ in rows)
    print(f"\nResult: {len(rows) - bad - warn} ok, {warn} warnings, {bad} problems.")
    return 1 if bad else 0


# ---- init -----------------------------------------------------------------------------------------------------------
ENV_EXAMPLE = """# Copy the values from the Vunly setup step into your SHELL (export ...) or your secret manager.
# Do not put the real key in this file or in any file you commit.
VUNLY_API_URL=
VUNLY_API_KEY=
"""


def cmd_init(_: argparse.Namespace) -> int:
    root = Path.cwd()
    example = root / ".env.example"
    made = []
    if not example.exists():
        example.write_text(ENV_EXAMPLE)
        made.append(".env.example (names only, no values)")
    ignore = root / ".gitignore"
    text = ignore.read_text() if ignore.exists() else ""
    # `.venv/` and `/.venv` are the same entry
    def norm(s: str) -> str:
        return s.removeprefix("/").removesuffix("/")

    present = {norm(line) for line in text.split()}
    # `.env.*` would also hide `.env.example`, which you commit: the `!` line (after it) puts it back.
    # A virtualenv and bytecode caches are never committed either, and `git add -A` would stage them.
    wanted = [p for p in (".env", ".env.*", "!.env.example", ".venv/", "__pycache__/") if norm(p) not in present]
    if wanted:
        ignore.write_text(text + ("" if text.endswith("\n") or not text else "\n") + "\n".join(wanted) + "\n")
        made.append(f".gitignore (added {', '.join(wanted)}; .env is ignored, .env.example stays committable)")
    print("vunly init: " + ("wrote " + "; ".join(made) if made else "nothing to change, already set up"))
    print(
        "\nNext:\n"
        "  1. In your shell:  export VUNLY_API_URL=<address from the setup step>  and  export VUNLY_API_KEY=<sandbox key>\n"
        "  2. vunly doctor    checks the environment, the address, the key and its scopes\n"
        "  3. vunly verify    runs the 5 sandbox checks and writes " + REPORT + "\n"
        "The key is read from the environment only: it is never written to a file by this command."
    )
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="vunly", description="Vunly sandbox tools: init, doctor, verify.")
    ap.add_argument("--version", action="version", version=f"vunly {__version__}")
    sub = ap.add_subparsers(dest="command", required=True)
    v = sub.add_parser("verify", help="run the 5 sandbox checks (creates one throwaway customer)")
    v.add_argument("--meter", help="the meter key; optional when the project has exactly one")
    v.set_defaults(fn=cmd_verify)
    sub.add_parser("doctor", help="check the environment, the address, the key and its scopes").set_defaults(fn=cmd_doctor)
    sub.add_parser("init", help="write .env.example and ignore .env; never writes a key").set_defaults(fn=cmd_init)
    # The key comes from VUNLY_API_KEY only; it is never a flag. If a user passes one anyway, argparse's default error
    # would echo its value to stderr — so parse leniently and report only the flag NAMES, never their values. A token
    # like `--api-key=vk_test_...` carries the secret after `=`; keep the flag name, drop everything after `=`, and
    # redact any bare token that looks like a key.
    args, extra = ap.parse_known_args(argv)
    if extra:

        def _safe(tok: str) -> str | None:
            if tok.startswith("-"):
                return tok.split("=", 1)[0] if "=" in tok else tok  # `--flag=value` -> `--flag`
            return None  # a bare value (its own token): never echo it

        names = " ".join(n for n in (_safe(t) for t in extra) if n) or "see --help"
        print(f"vunly: unrecognized arguments: {names} (the key is read from VUNLY_API_KEY, never a flag)", file=sys.stderr)
        return 2
    try:
        return args.fn(args)
    except SetupProblem as exc:
        print(f"vunly {args.command}: cannot run: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
