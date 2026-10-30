# vunly (Python)

Python client for the Vunly sandbox API: record usage, check entitlements, grant credits. One dependency (`httpx`), typed, synchronous and asyncio. Sandbox only: no real payments.

## Install

**Install Python from this repository.** The repository is public, packages are not published to registries. Clone the repository, then follow the steps below from the `python/` folder. They were run from a fresh clone on Python 3.12.

```sh
cd python
python3 -m venv .venv && . .venv/bin/activate   # optional, but keeps your system Python clean
pip install .                                   # pip fetches httpx, the one dependency
```

This library is not published to a package registry, so there is no registry command.

`pip` builds the package from the folder with `setuptools`, so it needs access to a Python package index for `httpx` and the build tool; the `vunly` package itself comes from the clone.

## Use

```python
import os
from vunly import Client, LimitReached, Unavailable

# The key comes from the environment. Never write it into source, tests, logs or prompts.
vunly = Client(api_key=os.environ["VUNLY_API_KEY"], base_url=os.environ["VUNLY_API_URL"])

vunly.customers.upsert("cus_demo_01")

decision = vunly.entitlements.check("cus_demo_01", "generate", {"meter": "tokens", "quantity": 1050})
if not decision.allowed:
    raise RuntimeError(decision.reason)  # do not run the work

# ... do the work, then record it once. The id names this unit of work; retries use the SAME id.
try:
    result = vunly.usage.record(id="req_2026_0001", customer="cus_demo_01", meter="tokens", quantity=1050)
    print(result.remaining, result.replayed)
except LimitReached as e:
    ...  # 402: not enough credits, nothing was debited
except Unavailable:
    ...  # the SDK already retried; fail closed: do not run the work
```

Asyncio: `from vunly import AsyncClient`, `async with AsyncClient(...) as vunly:` and `await` every call. Same results, same errors.

`base_url` is the API address. A trailing slash or `/v1` is accepted and means the same address.

## Calls

`AsyncClient` has the same calls; `await` each one.

| Call                                                         | What it does                                                |
| ------------------------------------------------------------ | ----------------------------------------------------------- |
| `customers.upsert(id, name=None)`                            | Create a customer, or update it; repeating changes nothing  |
| `entitlements.check(customer, feature, estimate=None)`       | Ask whether the customer may do this before the work        |
| `usage.record(id, customer, meter, quantity)`                | Record usage once; a repeated `id` returns `replayed: true` |
| `credits.balance(customer)`                                  | `remaining`, `consumed` and `granted`                       |
| `credits.grant(id, customer, credits, reason=None)`          | Sandbox top-up, once per `id`                               |
| `credits.usage(customer)`                                    | This period's consumption per meter, with plan and balance  |
| `plans.upsert_meter(key, units_per_credit)`                  | Define a meter and how many units one credit covers         |
| `plans.upsert(key, name, included_credits, meter, features)` | Define a plan                                               |
| `plans.subscribe(customer, plan)`                            | Put a customer on a plan                                    |
| `plans.cancel(customer)`                                     | End the customer's plan                                     |
| `customer_tokens.create(customer, ttl_seconds=None)`         | A read-only token for one customer's widget, 30 to 3600 s   |
| `customer_tokens.revoke(id)`                                 | Revoke a token before it expires                            |

## Webhooks: verify a delivery

`verify_webhook(payload, signature_header, secrets, now=None)` checks the `Vunly-Signature` header (`t=<unix>,v1=<hex>`) against the **raw** request body, before you parse it. It returns a `WebhookEvent` (`id`, `type`, `created_at`, `data`) or raises `WebhookError` with a `reason` (`missing_header`, `malformed_header`, `stale_timestamp`, `no_matching_signature` or `invalid_payload`): answer 400 and do not process the payload.

```python
import os
from vunly import WebhookError, verify_webhook

def handle(raw_body: bytes, headers: dict[str, str]):
    try:
        event = verify_webhook(raw_body, headers.get("Vunly-Signature"), os.environ["VUNLY_WEBHOOK_SECRET"])
    except WebhookError:
        return 400
    if already_processed(event.id):  # the same event can arrive more than once
        return 200
    ...
```

- Pass the exact bytes received (a `str` is encoded as UTF-8), not JSON that was parsed and written out again.
- While you rotate a secret, pass both: `secrets=[new, old]`. A match with any one is enough.
- The signature is HMAC-SHA256 over `"<t>." + body`, compared in constant time. A timestamp more than five minutes from now, in either direction, is rejected.
- A missing, empty or blank secret (or an empty list) raises `ValueError` before the signature is looked at, because an empty key signs anything: it is a configuration problem, not a bad delivery.

## Retries: only what is safe

A write carries its id as the `Idempotency-Key`, so the API applies it at most once. The client retries, with the **same id**:

| What happened                            | Retried? | Wait                                                     |
| ---------------------------------------- | -------- | -------------------------------------------------------- |
| network failure, timeout                 | yes      | exponential backoff with jitter                          |
| `429`                                    | yes      | `Retry-After` seconds (longer than 30 s: raises instead) |
| `500`, `502`, `503`, `504`               | yes      | exponential backoff with jitter                          |
| `402`, `400`, `401`, `403`, `404`, `409` | **no**   | raised at once                                           |

Defaults: timeout 10 s, 2 retries (`max_retries=`, or `retry=RetryPolicy(...)`, `0` turns retries off). When retries run out you get `Unavailable`; it is still safe to retry later with the same id.

## Errors

All derive from `VunlyError` with `code`, `status`, `request_id` (quote it in support requests), `details`. The message never contains your key.

| Class            | When                                                                                                                                                    |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `AuthError`      | 401/403: unknown or revoked key, or a missing scope                                                                                                     |
| `LimitReached`   | 402: too few credits; `details` has `balance` and `required`                                                                                            |
| `InvalidRequest` | 400/404/409: bad fields, unknown customer or meter, an id reused with other data. An id that cannot be an Idempotency-Key is refused before any request |
| `Unavailable`    | network failure, timeout, 429, 5xx after the retries, or a 3xx (`unexpected_redirect`: wrong base URL, never followed or retried)                       |

## The `vunly` command

Installing the package also installs a `vunly` command, for projects without Node:

```sh
vunly init      # writes .env.example (names only) and ignores .env; never writes a key
vunly doctor    # checks Python, the environment, the API address, the key and its scopes
vunly verify    # the 5 sandbox checks (add --meter <key> if the project has several); writes vunly-verify-report.md
```

The key is read from `VUNLY_API_KEY` only and is never printed, written or accepted as an argument. Exit codes of `verify`: `0` everything passed, `1` a check failed, `2` the command could not run (a missing variable, a key that is not a sandbox key, a wrong address, an unknown meter). `doctor` reports each problem, a missing variable included, as a failed check: `0` nothing failed, `1` something failed.

## Tests

The tests use a fake transport and synthetic keys: they need no Vunly API and no key. From the `python/` folder:

```sh
python3 -m venv .venv
.venv/bin/pip install -e ".[test]"
.venv/bin/python -m pytest
```

## Known limitations

Status: **Implemented**. These are the limitations recorded for this SDK. A limitation does not mean the SDK is withdrawn; read it before you use the SDK.

- Webhook endpoint management, the delivery log, recipes, activity, exports, credit packs and billing calls (diagnostics and reconciliation) are available through the REST API; this SDK does not wrap them. Changing a plan, cancelling a pending plan change and reading a customer's own usage with a customer token are not wrapped either.
- Proxy settings and custom certificate authorities are not verified.
- Run on Linux only; Windows and macOS were not run.

## License

Apache-2.0 (see `LICENSE` and `NOTICE`). The Vunly name and logo are trademarks and are not licensed by it.
