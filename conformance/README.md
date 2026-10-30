# Conformance material

This folder holds what the eight SDKs share.

## Retry rules

Every SDK sorts its calls into four operation classes and retries by class, always with the same idempotency id:

| Class | Calls                                                                                      | After a lost answer, a timeout or a 5xx | After a `429`                    |
| ----- | ------------------------------------------------------------------------------------------ | --------------------------------------- | -------------------------------- |
| R     | reads and the entitlement check                                                            | repeated                                | repeated                         |
| N     | naturally idempotent writes (upserts, deletes, subscribing a customer to a plan)           | repeated                                | repeated                         |
| K     | keyed mutations (for example recording usage, granting credits), repeated with the same id | repeated                                | repeated                         |
| X     | a mutation with no key that is not idempotent (creating a customer token)                  | never: the caller decides               | repeated (refused before it ran) |

- A `429` whose `details.reason` is `plan_included_volume` is never repeated and never waited on.
- `402` and every other `4xx` apart from `429` are never retried.
- A `3xx` is `unexpected_redirect`, class `Unavailable`, never retried (and never followed: it would send the key to another address). The API itself never answers one, so no scenario can express it; each SDK pins it in its own unit tests.
- By default a call is retried twice. An id that cannot be an idempotency key (wrong length or characters) is refused before any request, as an `InvalidRequest` with status `400` and code `validation_error`.
- `Retry-After` is read as plain seconds: ASCII digits with an optional fraction and exponent, an optional `+`, or `Inf` or `Infinity`. Up to 30 seconds is waited. A larger or unrepresentable value is a very long wait: it is surfaced at once, reported capped at one day, and never retried early. `0` means no directive (the normal backoff). An HTTP date, hexadecimal, a suffix, a minus sign or anything else means no header. The exact cases are the vectors in `conformance/retry-after-vectors.json` (each SDK carries an identical copy in its tests).

The four error classes are `AuthError` (`401`, `403`), `LimitReached` (`402`), `Unavailable` (`429`, `5xx`, `3xx` and network failures) and `InvalidRequest` (every other `4xx`).

## `retry-after-vectors.json`

How every SDK reads a `Retry-After` header. Each SDK keeps an identical copy next to its own tests and runs every vector, so running an SDK's tests (see the root README) runs this suite. This repository's CI checks that the eight copies are identical to this file.

## `scenarios/`

Language-neutral descriptions, in JSON, of how the Vunly API and every SDK must behave: authentication, usage, credits, entitlements, errors, idempotency, uncertain outcomes, concurrency, timeouts and rate limits, retry classes, pagination and plan usage.

A scenario is run against the Vunly API, with a test harness that can drop a response on purpose so an SDK's retry behaviour can be observed. The API and that harness are not part of this repository, so **you cannot run the scenarios from here**. They are published so you can read what each SDK promises. What you can run from this repository alone is each SDK's own test suite and the shared Retry-After vectors.

## Scenario file

```json
{
  "id": "usage",
  "title": "one line",
  "setup": ["step"],
  "steps": ["step"]
}
```

`setup` runs first with an administrative credential; `steps` run with the application credential unless a step says otherwise. A step names an SDK-level operation (`customers.upsert`, `credits.grant`, `credits.balance`, `usage.record`, `entitlements.check`, `plans.subscribe`, `customers.get`, `customers.list`, `credits.ledger`, `plan.usage`, `tokens.create`) with an input, what to expect (a success matched as a subset, or an error class, status and code), and optionally a value to keep for later steps. `{{run}}` stands for a short random string per run, so repeated runs never collide.
