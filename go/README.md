# github.com/getvunly/vunly-sdks/go

Go client for the Vunly sandbox API: record usage, check entitlements, grant credits. Go 1.26 or newer, **no third-party dependencies**, safe for concurrent use. Sandbox only: no real payments.

## Install

The module path is `github.com/getvunly/vunly-sdks/go`: the address of this repository plus the `go/` folder.

**Install Go from this repository.** The repository is public, packages are not published to registries. Clone the repository, then follow the steps below from the `go/` folder. They were run from a fresh clone on Go 1.27.

In your own module (the directory with your `go.mod`), point the module path at the clone. `/path/to/vunly-sdks/go` is the `go/` folder of the clone, the folder that holds this README:

```sh
go mod edit -require github.com/getvunly/vunly-sdks/go@v0.0.0 -replace github.com/getvunly/vunly-sdks/go=/path/to/vunly-sdks/go
go build ./...
```

The version after `@` is only a label: with a `replace` to a directory Go uses the files there. `go mod tidy` also works, offline, because the SDK has no dependencies. Then `import vunly "github.com/getvunly/vunly-sdks/go"`.

Go fetches modules from the repository itself, so this line is the other way in. It works once the repository is public (it was not run before then):

```sh
go get github.com/getvunly/vunly-sdks/go
```

This library is not published to a package registry, so there is no registry command.

## Use

```go
package main

import (
	"context"
	"errors"
	"fmt"
	"os"

	vunly "github.com/getvunly/vunly-sdks/go"
)

func main() {
	ctx := context.Background()

	// The key comes from the environment. Never write it into source, tests, logs or prompts.
	client, err := vunly.New(os.Getenv("VUNLY_API_KEY"), vunly.WithBaseURL(os.Getenv("VUNLY_API_URL")))
	if err != nil {
		panic(err)
	}

	if _, err := client.Customers.Upsert(ctx, "cus_demo_01", nil); err != nil {
		panic(err)
	}

	decision, err := client.Entitlements.Check(ctx, vunly.CheckParams{
		Customer: "cus_demo_01", Feature: "generate",
		Estimate: &vunly.Estimate{Meter: "tokens", Quantity: 1050},
	})
	if err != nil {
		fmt.Fprintln(os.Stderr, "cannot decide, so the work does not run:", err) // fail closed
		return
	}
	if !decision.Allowed {
		// "no_active_plan", "feature_not_in_plan" or "insufficient_credits": a decision, not a failure
		fmt.Println("not allowed:", decision.Reason)
		return
	}

	// ... do the work, then record it once. The ID names this unit of work; retries use the SAME ID.
	res, err := client.Usage.Record(ctx, vunly.RecordParams{ID: "req_2026_0001", Customer: "cus_demo_01", Meter: "tokens", Quantity: 1050})
	var limit *vunly.LimitReachedError
	switch {
	case errors.As(err, &limit):
		// 402: not enough credits, nothing was debited
	case err != nil:
		// *vunly.UnavailableError after the SDK's own retries: fail closed
	default:
		fmt.Println(res.Remaining, res.Replayed)
	}
}
```

`WithBaseURL` is the API address. A trailing slash or `/v1` is accepted and means the same address.

`WithHTTPClient` lets you bring your own `*http.Client` (proxy, transport). The SDK uses a copy of it, so your `Transport`, `Jar` and `Timeout` apply and your own client is never modified. On the copy, redirects are never followed, whatever your client's `CheckRedirect` says: a followed redirect would send your key to wherever it points. Every 3xx answer ends in `unexpected_redirect`.

**This runs against a project that is already set up.** On a fresh project the check says `not allowed: no_active_plan`, or the API answers `meter_not_found`. Before the first run the project needs: a meter named `tokens` (`Plans.UpsertMeter`), a plan whose features include `generate` (`Plans.UpsertPlan`), the customer subscribed to that plan (`Plans.Subscribe`) and credits (the plan's included credits, or `Credits.Grant`). The meter and the plan are created with an admin key; the key the onboarding wizard issues to an application can subscribe, check, record and grant but cannot create a plan. [`examples/quickstart`](examples/quickstart) does the customer, subscription and grant steps and prints every decision.

## Calls

| Call                                                | What it does                                                                                                           |
| --------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `Customers.Upsert(ctx, id, *CustomerParams)`        | Create a customer, or update it; repeating changes nothing                                                             |
| `Customers.Get / List / All`                        | Read one customer, a page, or every customer lazily                                                                    |
| `Entitlements.Check(ctx, CheckParams)`              | Ask whether the customer may do this before the work                                                                   |
| `Usage.Record(ctx, RecordParams)`                   | Record usage once; a repeated `ID` returns `Replayed: true`                                                            |
| `Credits.Balance(ctx, customer)`                    | `Remaining`, `Consumed` and `Granted`                                                                                  |
| `Credits.Grant(ctx, GrantParams)`                   | Sandbox top-up, once per `ID`                                                                                          |
| `Credits.Ledger / AllLedger`                        | A customer's ledger, a page or lazily (needs the `ledger:read` scope, which the onboarding wizard's key does not have) |
| `Plans.UpsertMeter / UpsertPlan / Subscribe/Cancel` | Define a meter and a plan, put a customer on it, end it                                                                |
| `CustomerTokens.Create / Revoke`                    | A read-only token for one customer's widget, 30 seconds to 1 hour                                                      |
| `PlanUsage(ctx)`                                    | The project's plan and this month's usage (limits are unconfirmed)                                                     |
| `VerifyWebhook(payload, header, secrets)`           | Check a webhook delivery against the raw body and the signing secret                                                   |

## Retries: only what is safe

A **keyed** write (`Usage.Record`, `Credits.Grant`) carries its `ID` as the `Idempotency-Key`, so the API applies it at most once; an empty or malformed `ID` is refused before any request. Upserts, deletes and `Subscribe` carry no key: repeating them changes nothing, so they are safe to repeat without one. The client retries, with the **same ID**, by operation class:

| Operation                                                   | After a lost answer, timeout, 5xx | After a 429 |
| ----------------------------------------------------------- | --------------------------------- | ----------- |
| reads, the entitlement check                                | yes                               | yes         |
| keyed mutations (`Usage.Record`, `Credits.Grant`)           | yes                               | yes         |
| naturally idempotent writes (upserts, deletes, `Subscribe`) | yes                               | yes         |
| a new token each call (`CustomerTokens.Create`)             | **no**                            | yes         |

The Free plan's monthly volume stop (a 429 whose `Details["reason"]` is `plan_included_volume`) is never retried. A `Retry-After` longer than 30 seconds is returned, not waited on. 402, 400, 401, 403, 404 and 409 are never retried.

Defaults: 10 s per attempt, 2 retries (`WithTimeout`, `WithMaxRetries`; `0` turns retries off). When they run out you get `*UnavailableError`; it is still safe to try later with the same ID. Cancel the context to stop a call at once: a cancel returns the context's own error (`errors.Is(err, context.Canceled)`), because it is your decision. A context **deadline** that expires first is "no answer in time": an `*UnavailableError` (so your fail-closed branch sees it) that still satisfies `errors.Is(err, context.DeadlineExceeded)`.

## Errors

Every error wraps `*APIError` (`Code`, `Status`, `RequestID` to quote when you report a problem, `Details`, `RetryAfter`). The message never contains your key. Text the server sends (message, code, request id, the strings in details) is cleaned before you see it: control, bidirectional and zero-width characters removed, an echo of your key replaced by `[redacted]`, and bounded to 500 (message and detail strings), 64 (code) and 128 (request id) characters.

| Type                   | When                                                                                                                                                                                                                                 |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `*AuthError`           | 401/403: unknown or revoked key, or a missing scope                                                                                                                                                                                  |
| `*LimitReachedError`   | 402: too few credits; `Balance()` and `Required()`                                                                                                                                                                                   |
| `*InvalidRequestError` | other 4xx, and an ID that cannot be an Idempotency-Key (refused before any request)                                                                                                                                                  |
| `*UnavailableError`    | no answer, 429, 5xx after retries, a malformed answer (`invalid_response`), or a redirect (`unexpected_redirect`: the base URL is probably wrong; the SDK never follows one, it would send your key elsewhere, and never retries it) |

Match with `errors.As` or `vunly.IsLimitReached(err)` and friends; never on the message.

## Webhooks

```go
event, err := vunly.VerifyWebhook(rawBody, r.Header.Get("Vunly-Signature"), []string{secret})
```

Pass the **raw** request body, not re-encoded JSON. Pass two secrets while you rotate. On a `*WebhookError` answer 400 and do not process the payload.

## Supported Go versions

The two latest Go releases. This version is verified on Go 1.26 and Go 1.27 (`go.mod` says `go 1.26`).

## Tests

The tests use `httptest` servers and synthetic keys: they need no Vunly API and no key. From the `go/` folder:

```sh
go test ./...
```

## Known limitations

Status: **Beta**. These are the limitations recorded for this SDK. A limitation does not mean the SDK is withdrawn; read it before you use the SDK.

- Webhook endpoint management, the delivery log, recipes, activity, exports, credit packs and billing calls (diagnostics and reconciliation) are available through the REST API; this SDK does not wrap them. Changing a plan, cancelling a pending plan change, reading a customer's own usage with a customer token and the credits usage summary are not wrapped either. There is no command line tool.
- Run on Linux only; Windows and macOS were not run.
