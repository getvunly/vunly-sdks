# @vunly/sdk

Minimal TypeScript client for the Vunly sandbox API. `fetch` only, zero dependencies, Node >= 22. Sandbox only:
no real payments.

## Install

**Install TypeScript from this repository.** The repository is public, packages are not published to registries. Clone the repository, then follow the steps below from the `typescript/` folder. They were run from a fresh clone on Node 22.

```sh
cd typescript
npm install          # the development tools only (TypeScript); the SDK has no dependencies
npm run pack:local   # builds the package and prints the path of out/vunly-sdk-<version>.tgz
```

Then, in your own project, install the file by its path:

```sh
npm install /path/to/vunly-sdks/typescript/out/vunly-sdk-<version>.tgz
```

This library is not published to a package registry, so there is no registry command.

The package is ESM only: use `import`, not `require`. Run the tests of this SDK with `npm test` in the `typescript/` folder (Node 22 or newer; no network and no key are needed).

## Command line

The package installs the `vunly` command:

```sh
npx vunly init      # writes .env.example (names only) and the ignore rules for .env; never writes a key
npx vunly doctor    # checks Node, the environment, the API address, the key and its scopes
npx vunly verify --meter <your-meter-key>
```

`vunly-verify` is the same as `vunly verify`. Exit codes of `verify`: 0 everything passed, 1 a check failed, 2 the command could not run (a missing variable, a key that is not a sandbox key, an unknown meter). `doctor` reports each problem, a missing variable included, as a failed check: 0 nothing failed, 1 something failed.

## Use

```ts
import { createClient, LimitReached, Unavailable } from "@vunly/sdk";

// The key comes from the environment. Never write it into source, tests, logs or prompts.
const vunly = createClient({
  apiKey: process.env.VUNLY_API_KEY!,
  baseUrl: process.env.VUNLY_API_URL!,
});

await vunly.customers.upsert({ id: "cus_demo_01" });

const decision = await vunly.entitlements.check({
  customer: "cus_demo_01",
  feature: "generate",
  estimate: { meter: "tokens", quantity: 1050 },
});
if (!decision.allowed) throw new Error(decision.reason); // do not run the work

// ... do the work, then record it once. The id must be stable for this unit of work (retry with the SAME id).
await vunly.usage.record({
  id: "req_2026_0001",
  customer: "cus_demo_01",
  meter: "tokens",
  quantity: 1050,
});

const { remaining, consumed } = await vunly.credits.balance({ customer: "cus_demo_01" });
```

## Verify your integration

With the SDK installed, run this from your app folder. It needs only the SDK and your sandbox key:

```sh
export VUNLY_API_URL=<the API address from the setup step; with or without a trailing /v1>
export VUNLY_API_KEY=<your sandbox key, vk_test_...>
npx vunly-verify --meter <your-meter-key>
```

`--meter` can be left out when the project has exactly one meter. It creates one throwaway customer, tops it up with sandbox credits and checks: customers are created once, a top-up with one id applies once, usage is counted once and a replay does not double count, over-limit usage is rejected with nothing counted, and with Vunly unreachable the entitlement check fails with `Unavailable` instead of answering. It tests your Vunly setup with a throwaway customer, not your own routes. It prints one PASS or FAIL line per check, writes `vunly-verify-report.md` (the key is never written), and exits 0 when all pass, 1 when a check fails, 2 when it could not run (missing variable, a key that is not a sandbox key, an unknown meter). Sandbox only; no real payments.

## Timeouts and retries

A custom `fetch` (the `fetch` option) must respect `redirect: "manual"`, which the SDK sets on every request: the SDK never follows a redirect and never sends your key to a redirect target. If your `fetch` follows redirects anyway (`response.redirected` is true), the SDK raises `Unavailable` with code `unexpected_redirect` instead of using the answer; note that your function has already sent the key to the target by then.

Defaults: a 10 second timeout per attempt (`timeoutMs`) and 2 retries (`maxRetries`; `0` turns them off). Retries use the same id. The SDK retries network failures, timeouts, `429` (after `Retry-After`, up to 30 seconds) and `500`, `502`, `503`, `504`; it never retries another `4xx`: `402 plan_limit_reached` (`PlanLimitReached`) is not retried before the next UTC month or a plan change, and `402 insufficient_credits` (`LimitReached`) is raised at once.

```ts
const vunly = createClient({ apiKey, baseUrl, timeoutMs: 5_000, maxRetries: 3 });
```

## Calls

| Call                                                                  | Purpose                                                                                   |
| --------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| `createClient({ apiKey, baseUrl })`                                   | Reads nothing from disk; you pass the key from your environment                           |
| `customers.upsert({ id, name? })`                                     | Idempotent create-or-get by your own customer id                                          |
| `entitlements.check({ customer, feature, estimate? })`                | `{ allowed, reason, remaining, required, plan }`. Fail closed on `Unavailable`            |
| `usage.record({ id, customer, meter, quantity })`                     | Idempotent by `id`. Debits credits (rounded up per request)                               |
| `usage.recordBatch([{ id, customer, meter, quantity, occurredAt? }])` | 1 to 100 events in one request; one result per event, in order (see "Batches")            |
| `usage.batcher(options?)`                                             | A buffer in front of `recordBatch`: `add`, `flush`, `close` (see "Batches")               |
| `credits.balance({ customer })`                                       | `{ remaining, consumed, granted }`                                                        |
| `credits.usage({ customer })`                                         | Current-period usage per meter, plan and balance                                          |
| `credits.grant({ id, customer, credits })`                            | Sandbox top-up (needs `credits:grant`)                                                    |
| `plans.upsertMeter / upsert / subscribe / cancel`                     | Project setup (needs `plans:write`, `subscriptions:write`)                                |
| `webhooks.endpoints.*`, `webhooks.deliveries.*`, `verifyWebhook(...)` | Outbound webhook setup, delivery log, and receiving/verifying events (see Webhooks below) |
| `recipes.list / apply`                                                | Versioned meter+plan templates; `apply` is idempotent (`plans:read` / `plans:write`)      |

## Batches

`usage.recordBatch(events)` sends 1 to 100 events in ONE request and returns one result per event, in order. A request that is
answered is not an error even when some events are not recorded, so read each `status`. Only `unavailable` is sent again:

| Status                 | Recorded              | Send again?                                                                               |
| ---------------------- | --------------------- | ----------------------------------------------------------------------------------------- |
| `accepted`             | yes                   | no                                                                                        |
| `duplicate`            | already, charged once | no                                                                                        |
| `unavailable`          | **no**                | **yes**, with the same `id` (the batcher does it for you)                                 |
| `plan_limit_reached`   | no                    | no: your plan's monthly volume is used up; not before the next UTC month or a plan change |
| `too_late`             | no                    | no: older than 24 hours, final                                                            |
| `insufficient_credits` | no                    | no: your customer's balance is too low (not your plan's limits)                           |
| `meter_not_found`      | no                    | no: create the meter first                                                                |
| `error`                | no                    | no                                                                                        |

`customer` is the Vunly `cus_...` id that `customers.upsert` returns, not your
own id. `occurredAt` is when the event happened; if you leave it out the SDK stamps the time you handed the event over and keeps
that stamp on every resend.

`usage.batcher()` does the buffering for you:

```ts
const batcher = vunly.usage.batcher({ maxBatchSize: 100, flushIntervalMs: 1000 });
const outcome = await batcher.add({
  id: "req_2026_0001",
  customer: "cus_demo_01",
  meter: "tokens",
  quantity: 1050,
});
// outcome.status is the final status of THIS event, once its batch has been sent
await batcher.close(); // flushes what is left; call it before the process ends
```

It sends up to 100 events at a time and at least every `flushIntervalMs`, sends `unavailable` events again with the same ids
(waiting at least the `Retry-After`, then backing off, at most `maxAttempts` sends, default 5; after that the event's status is
`unavailable` and it is not recorded), and holds at most `maxPending` events (default 10,000): past that `add` rejects with
`Unavailable` instead of queueing without limit. `add` rejects for an invalid event (nothing queued) and when the request as a
whole fails for a reason a resend cannot fix (`AuthError`, a frozen project). Keep `usage.record` for a one-off call that needs
its answer in the request.

**Many small batches.** The batch route also has a ceiling on how many _requests_ one project may send, separate from the plan's
volume: where it is on (today, for allow-listed sandbox projects) it is 1,800 requests per 60 seconds per project and environment, counted
across every process that uses the project's keys. At 100 events per request that is far above any plan; at one event per
request it is 30 events a second. So send full batches: one `usage.batcher()` per process (not one per request or per
customer), a `flushIntervalMs` of a second or more, and let it fill. If your workers are many and small, give each one a
batcher, not a direct `recordBatch` per event. A refusal for this limit is a `429` with `Retry-After: 60`, longer than the 30
seconds the SDK waits, so the call raises `Unavailable`; the batcher sends those events again with the same ids, and a direct
`recordBatch` caller does the same after the wait. Nothing was recorded by a `429`.

**The batch route is not available everywhere yet.** It is off unless the deployment turned it on for your project (today only for allow-listed sandbox projects); until then `recordBatch` throws `InvalidRequest` (`not_found`, 404) and `usage.record` is the way to send usage.

## Ids

- **Idempotency id.** `usage.record` and `credits.grant` take an `id` that is sent as the `Idempotency-Key`: 8 to 128
  characters of `A-Za-z0-9_.:-`. Anything else (for example `q-1`) throws `InvalidRequest` before any request is sent, so
  validate or generate it (`req-<uuid>`) before you run the work. Use a stable id derived from the unit of work and
  reuse it on retry; the same id with a different quantity or customer is `InvalidRequest` (`idempotency_conflict`).
- **Customer ids.** `customers.upsert` returns Vunly's own `id` (`cus_<hex>`) and your `externalId`. You do not need
  to store the `cus_<hex>` id: every call that takes a `customer` accepts your own id (the one you upserted) or the
  `cus_<hex>` id.
- **Units.** `quantity` is in the meter's units (for example tokens), not credits. The meter converts it, rounded up
  per request: 1050 tokens at 1000 per credit is 2 credits.

## Concurrency: `check` is advisory, `record` is final

`entitlements.check` reads the balance and reserves nothing. Many concurrent requests can all pass it when only a few
have credits left; the rest then get `LimitReached` (402) from `usage.record`. The balance never goes below zero, and
this is by design. Treat a 402 from `record` as final: the credits were not there. The work already ran by then, so
return 402 to your caller (do not return the result), log it, and do not retry with a new id. Holding credits before
the work (reserve and commit) is not part of this SDK yet.

## Errors

All extend `VunlyError` and carry a stable `code` and a `requestId`. Messages never contain your key.

| Class              | When                                                                                                                               | What to do                                                                                                                                                                                                                               |
| ------------------ | ---------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `AuthError`        | 401/403: wrong, revoked or under-scoped key                                                                                        | Stop. Do not look for another key; ask for the right scope                                                                                                                                                                               |
| `LimitReached`     | 402 `insufficient_credits`: **your customer's** credit balance is too low. Not your own plan's limits (that is `PlanLimitReached`) | Do not run the work. Top up the customer's credits or move the customer to a plan with more credits                                                                                                                                      |
| `PlanLimitReached` | 402 `plan_limit_reached`: the plan's monthly volume is used up                                                                     | **Do not retry** (no `Retry-After`; nothing recorded). It clears at `error.details.resetsAt` or on a higher plan. On Free's single-event route `error.details.alternative` is the batch route: use `usage.recordBatch` / `usage.batcher` |
| `ProjectFrozen`    | 403 `project_frozen`: the project is read-only (beyond the plan's project limit)                                                   | Not a key problem and not retryable: upgrade or choose which projects stay active. Reads still work                                                                                                                                      |
| `InvalidRequest`   | 400/404/409: bad input, unknown id or meter (`meter_not_found`), reused id                                                         | Fix the request. For `meter_not_found`, create the meter first (`plans.upsertMeter` or a recipe)                                                                                                                                         |
| `Unavailable`      | Network failure, 429 (a real rate limit) or 5xx                                                                                    | Fail closed on `check`; retry `record` with the same `id`. On 429 wait `error.details.retryAfterSeconds` first                                                                                                                           |

## Webhooks: receiving events

Register an endpoint (`webhooks.endpoints.create({ url, events? })`, key scope `webhooks:write`); the signing `secret`
is returned once, so put it in your environment. Vunly then POSTs signed JSON events: `credits.low`,
`credits.exhausted`, `usage.limit_reached`, `subscription.activated` (and `webhook.test` from
`webhooks.endpoints.test`).

```ts
import { verifyWebhook } from "@vunly/sdk";

export async function handle(request: Request): Promise<Response> {
  const payload = await request.text(); // the RAW body, before any JSON parsing
  let event;
  try {
    event = await verifyWebhook({
      payload,
      signature: request.headers.get("vunly-signature"),
      secret: process.env.VUNLY_WEBHOOK_SECRET!,
    });
  } catch {
    return new Response(null, { status: 400 }); // bad signature or stale timestamp
  }
  // Duplicates are normal: insert event.id under a unique key and stop if it already exists.
  if (!(await markProcessed(event.id))) return new Response(null, { status: 200 });
  queueMicrotask(() => doTheWork(event)); // answer fast, work after
  return new Response(null, { status: 200 });
}
```

Rules that keep you correct:

1. **Verify the raw body** and the timestamp (`verifyWebhook` rejects more than 5 minutes of skew). Pass both secrets while rotating.
2. **Answer any 2xx quickly.** Anything else, a redirect or a timeout is a failed attempt.
3. **Expect duplicates and any order.** Failures retry after 30 s, 1, 2, 4, 8, 16 and 32 min (8 attempts), and you can redeliver from
   the log. The event `id` is the same every time: dedupe on it, never on arrival.
4. **Look at the delivery log** (`webhooks.deliveries.list / get`) when something is missing: it shows each attempt's status code, error and duration.

## Known limitations

Status: **Beta**. These are the limitations recorded for this SDK. A limitation does not mean the SDK is withdrawn; read it before you use the SDK.

- Proxy settings and custom certificate authorities are not verified.
- Run on Linux only; Windows and macOS were not run.

## License

Apache-2.0 (see `LICENSE` and `NOTICE`). The Vunly name and logo are trademarks and are not licensed by it.
