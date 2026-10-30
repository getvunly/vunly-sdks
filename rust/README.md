# vunly (Rust)

Rust client for the Vunly sandbox API: record usage, check entitlements, grant credits. **Blocking** (no async runtime), `Send + Sync`, no `unsafe`. Sandbox only: no real payments.

> Nothing is published, and the crate name `vunly` is a placeholder that is not reserved on crates.io. Do not add it with `cargo add`: use a `path` dependency on the clone.

## Install

**Install Rust from this repository.** The repository is public, packages are not published to registries. Clone the repository, then follow the steps below from the `rust/` folder. They were run from a fresh clone on Rust 1.99.1.

```toml
# Cargo.toml of your service: a path to the rust/ folder of the clone ...
[dependencies]
vunly = { path = "../vunly-sdks/rust" }
```

... or from the archive `cargo package` builds (`cargo package --manifest-path rust/Cargo.toml` writes `rust/target/package/vunly-0.1.0.crate`):

```sh
tar -xzf vunly-0.1.0.crate -C vendor/        # gives vendor/vunly-0.1.0/
# then, in your Cargo.toml: vunly = { path = "vendor/vunly-0.1.0" }
```

This library is not published to a package registry, so there is no registry command.

## Prerequisites in the sandbox project

The example below needs, in the sandbox project it talks to: a meter with the key `tokens`; a plan that includes the feature `generate` (and that meter); the customer subscribed to that plan; credits on the customer (the plan's included credits, or a grant); and an API key with the scopes the onboarding wizard issues. Set `VUNLY_API_KEY` (a `vk_test_...` key) and `VUNLY_API_URL` (the address from the setup step). The setup, once, with a sandbox key that has every scope (`VUNLY_ADMIN_KEY`; the service itself should run with the narrower key).

```sh
# setup-recipe
export CUSTOMER=${CUSTOMER:-cus_demo_01}   # your own id for the customer
api() { curl -fsS -X POST "$VUNLY_API_URL$1" -H "Authorization: Bearer $VUNLY_ADMIN_KEY" \
  -H 'Content-Type: application/json' ${3:+-H "Idempotency-Key: $3"} -d "$2"; echo; }
api /v1/meters '{"key":"tokens","unitsPerCredit":1000}'
api /v1/plans '{"key":"starter","name":"Starter","includedCredits":100,"meter":"tokens","features":["generate"]}'
api /v1/customers "{\"externalId\":\"$CUSTOMER\"}"
api /v1/customers/$CUSTOMER/subscription '{"plan":"starter"}'
api /v1/customers/$CUSTOMER/credit-grants '{"credits":100,"reason":"sandbox top-up"}' "grant_${CUSTOMER}_1"
```

## Use

```rust,no_run
use vunly::{CheckParams, Client, Error, Estimate, RecordParams};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The key comes from the environment. Never write it into source, tests, logs or prompts.
    let client = Client::new(std::env::var("VUNLY_API_KEY")?, std::env::var("VUNLY_API_URL")?)?;

    client.customers().upsert("cus_demo_01", None)?;

    let decision = client
        .entitlements()
        .check(CheckParams::new("cus_demo_01", "generate").estimate(Estimate::new("tokens", 1050)));
    if !matches!(decision, Ok(d) if d.allowed) {
        return Ok(()); // fail closed: an error is a "no"
    }

    // ... do the work, then record it once. The id names this unit of work; retries use the SAME id.
    match client.usage().record(RecordParams::new("req_2026_0001", "cus_demo_01", "tokens", 1050)) {
        Ok(res) => println!("{} credits left, replayed: {}", res.remaining, res.replayed),
        Err(Error::LimitReached(_)) => { /* 402: not enough credits, nothing was debited */ }
        Err(e) => return Err(e.into()), // Unavailable after the SDK's own retries: fail closed
    }
    Ok(())
}
```

`Client::new(key, url)`'s `url` is the API address. A trailing slash or `/v1` is accepted and means the same address. The SDK has no default address and reads no environment variable itself.

## Calls

| Call                                                      | What it does                                                         |
| --------------------------------------------------------- | -------------------------------------------------------------------- |
| `customers().upsert(id, name)`                            | Create a customer, or update it; repeating changes nothing           |
| `customers().get / list / iter`                           | Read one customer, a page, or every customer lazily                  |
| `entitlements().check(CheckParams)`                       | Ask whether the customer may do this before the work                 |
| `usage().record(RecordParams)`                            | Record usage once; a repeated id returns `replayed: true`            |
| `credits().balance(customer)`                             | `remaining`, `consumed` and `granted`                                |
| `credits().grant(GrantParams)`                            | Sandbox top-up, once per id                                          |
| `credits().ledger / iter_ledger`                          | A customer's ledger, a page or lazily                                |
| `plans().upsert_meter / upsert_plan / subscribe / cancel` | Define a meter and a plan, put a customer on it, end it              |
| `customer_tokens().create / revoke`                       | A read-only token for one customer's widget, 30 seconds to 1 hour    |
| `plan_usage()`                                            | The project's plan and this month's usage (limits are unconfirmed)   |
| `verify_webhook(payload, header, secrets)`                | Check a webhook delivery against the raw body and the signing secret |

## Blocking, and what to do in async code

The client blocks the calling thread, on purpose: it keeps the dependency tree small (about 30 crates with HTTPS, none of them an async runtime). From async code call it where blocking is allowed, for example with Tokio's `spawn_blocking`; the client is cheap to clone and clones share one connection pool:

```rust,ignore
let client = client.clone();
let res = tokio::task::spawn_blocking(move || client.usage().record(params)).await??;
```

## Retries: only what is safe

A write carries its id as the `Idempotency-Key`, so the API applies it at most once. The client retries, with the **same id**, by operation class:

| Operation                                                           | After a lost answer, timeout, 5xx | After a 429 |
| ------------------------------------------------------------------- | --------------------------------- | ----------- |
| reads, the entitlement check                                        | yes                               | yes         |
| keyed mutations (`usage().record`, `credits().grant`)               | yes                               | yes         |
| naturally idempotent writes (upserts, deletes, `plans().subscribe`) | yes                               | yes         |
| a new token each call (`customer_tokens().create`)                  | **no**                            | yes         |

The Free plan's monthly volume stop (a 429 whose `details.reason` is `plan_included_volume`, `Error::is_plan_stop`) is never retried. A `Retry-After` longer than 30 seconds is returned (`Error::retry_after`), not waited on. 402, 400, 401, 403, 404 and 409 are never retried. The library underneath never repeats a request by itself.

Defaults: 10 s per attempt, as a whole (connecting, headers and the whole body), 2 retries (`ClientBuilder::timeout`, `max_retries`; `0` turns retries off), an optional `total_timeout` for the call with its retries. When they run out you get `Error::Unavailable`; it is still safe to try later with the same id. `Client::with_cancel_token` stops a call during its retry wait. A response body over 16 MiB is refused, and so is one with more than 200 000 values (a decoded tree costs about ten times the text, and the cost is per array or object, not per value: a body of many small containers is the expensive shape). An error response body is read up to 64 KiB only; past that the status alone classifies the error. Budget roughly 60 to 100 MB of extra memory for decoding the largest accepted success body if you run under a tight memory limit; real answers are a few KB. A list iterator stops with `invalid_response` on a repeated cursor and on three consecutive empty pages that each promise another. Plain `http://` is accepted only for `localhost`, `127.0.0.0/8` and `::1` (`ClientBuilder::allow_insecure` for a test setup you control).

## Errors

`vunly::Error` is an enum implementing `std::error::Error`. Every variant carries an `ApiError` (`code`, `status`, `request_id` to quote when you report a problem, `details`, `retry_after`). Neither `Display` nor `Debug` contains your key.

Customer and token ids that are `.` or `..` (or empty) are refused before any request with a `validation_error` (400) that names the field: the API treats an encoded dot segment like a real one and would answer for another resource.

**Logging warning.** If your program installs a `log` implementation, do not enable TRACE for the HTTP library (`ureq_proto`, or a global `trace` level) in a deployed program. At DEBUG the Authorization header is redacted, but `ureq-proto` 0.6.5 hex-dumps the serialized request, including `authorization: Bearer <your key>`, at TRACE, before the SDK or any transport can intervene; the SDK cannot keep the header out of that dump. `tests/logging.rs` pins both facts.

| Variant                 | When                                                                                      |
| ----------------------- | ----------------------------------------------------------------------------------------- |
| `Error::Auth`           | 401/403: unknown or revoked key, or a missing scope                                       |
| `Error::LimitReached`   | 402: too few credits; `balance()` and `required()`                                        |
| `Error::InvalidRequest` | other 4xx, and an id that cannot be an Idempotency-Key (refused before any request)       |
| `Error::Unavailable`    | no answer, 429, 5xx after retries, or a malformed answer (`code() == "invalid_response"`) |

Match on the variant (or `Error::class()`); never on the message. `ConfigError` is returned when a client cannot be built; its messages name the setting, never its value.

## Webhooks

```rust,ignore
let event = vunly::verify_webhook(raw_body, &signature_header, &[&secret])?;
```

Pass the **raw** request body (the exact bytes, not re-encoded JSON). Pass two secrets while you rotate. On a `WebhookError` answer 400 and do not process the payload.

## Supported Rust versions

Verified on Rust 1.85.1 and 1.99.1, and on no other version. The minimum supported version is 1.85 (set by the HTTP client dependency); versions in between were not run. A newer minimum is a breaking change before 1.0. HTTPS uses rustls with bundled root certificates (no system certificate store).

## Tests

The tests use local servers and synthetic keys: they need no Vunly API and no key. From the `rust/` folder (the first run downloads the crates the tests need, which `Cargo.lock` pins):

```sh
cargo test --locked
```

## Example

`examples/http-service` is a small HTTP service (about 100 lines on `tiny_http`) that checks the entitlement before the work, records usage after it and never prints the key. It generates the idempotency id itself for every request: a client-supplied `X-Request-Id` is used only with `--trust-request-id`, because re-sending the same header would otherwise make billable work run again without a second charge.

## Known limitations

Status: **Implemented**. These are the limitations recorded for this SDK. A limitation does not mean the SDK is withdrawn; read it before you use the SDK.

- At TRACE level the HTTP library (ureq-proto) prints the request, including the Authorization header. Do not enable TRACE logging.
- Calls block the calling thread; there is no async API. Proxy and custom certificate authority options are not exposed.
- Webhook endpoint management, the delivery log, recipes, activity, exports, credit packs and billing calls (diagnostics and reconciliation) are available through the REST API; this SDK does not wrap them. Changing a plan, cancelling a pending plan change, reading a customer's own usage with a customer token and the credits usage summary are not wrapped either.
- Run on Linux only; Windows and macOS were not run.
