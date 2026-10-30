# vunly/vunly (PHP)

PHP client for the Vunly sandbox API: record usage, check entitlements, grant credits. `composer.json` allows PHP `^8.2`; the tests ran on **PHP 8.2, 8.3, 8.4 and 8.5** (no other version was run). **No dependencies beyond the `curl` and `json` extensions**, with a Laravel integration (service provider, facade, middleware) for Laravel 12 and 13. Sandbox only: no real payments.

> The package name `vunly/vunly` is not reserved on Packagist. Do not run `composer require vunly/vunly` against Packagist: use a Composer `path` repository that points at the clone (see "Install from source").

## Install from source

**Install PHP from this repository.** The repository is public, packages are not published to registries. Clone the repository, then follow the steps below from the `php/` folder.

A clone has no version tag, so Composer needs one given explicitly (a bare `"*"` does not resolve against a path repository under the default `minimum-stability`). In your project's `composer.json`:

```json
{
  "repositories": [
    {
      "type": "path",
      "url": "/path/to/vunly-sdks/php",
      "options": { "symlink": false, "versions": { "vunly/vunly": "0.1.0" } }
    }
  ],
  "require": { "vunly/vunly": "0.1.0" }
}
```

then `composer install` (or `composer update vunly/vunly`). The `url` is the `php/` folder of the clone, or an unpacked archive that `composer archive` makes from it.

This library is not published to a package registry, so there is no registry command.

## Before the first call: the sandbox project needs a few things

The SDK only talks to a project that already has them; the calls below fail with a clear API error (`meter_not_found`, a "no" from the check, `402`) until they exist:

1. A **meter** called `tokens` (`plans->upsertMeter('tokens', 1000)`: how many units cost one credit).
2. A **plan** whose features include `generate` and whose meter is `tokens` (`plans->upsertPlan(...)`). Creating meters and plans needs an admin key; the application's own key only needs the scopes the onboarding wizard issues.
3. The **customer** exists (`customers->upsert`) and is **subscribed** to the plan (`plans->subscribe`); a customer with no plan gets `allowed: false`, reason `no_active_plan`.
4. The customer has **credits** (a plan grants its included credits on subscribing; `credits->grant` tops up the sandbox). With none, `usage->record` is a `LimitReached` (402) and nothing is debited.

Use a sandbox key (`vk_test_...`).

## Use

```php
<?php
declare(strict_types=1);

use Vunly\Client;
use Vunly\LimitReached;
use Vunly\Models\Estimate;
use Vunly\Unavailable;

// The key and the address come from the environment. Never write the key into source, tests, logs or prompts.
$vunly = new Client(apiKey: getenv('VUNLY_API_KEY'), baseUrl: getenv('VUNLY_API_URL'));

$vunly->customers->upsert('cus_demo_01');

try {
    $decision = $vunly->entitlements->check('cus_demo_01', 'generate', new Estimate('tokens', 1050));
} catch (Unavailable) {
    return; // fail closed: a check that cannot be answered is a "no"
}
if (!$decision->allowed) {
    return; // do not run the work
}

// ... do the work, then record it once. The id names this unit of work; a retry uses the SAME id.
try {
    $result = $vunly->usage->record(id: 'req_2026_0001', customer: 'cus_demo_01', meter: 'tokens', quantity: 1050);
    echo $result->remaining, ' ', $result->replayed ? 'replayed' : 'recorded';
} catch (LimitReached $e) {
    // 402: not enough credits, nothing was debited
} catch (Unavailable $e) {
    // still unknown after the SDK's own retries: fail closed, and repeat later with the SAME id
}
```

`baseUrl` is the API address. A trailing slash or `/v1` is accepted and means the same address. The client is stateless between calls and safe to share. PHP is synchronous: a retry waits (at most 30 seconds), so use `maxRetries: 0` where a request must not wait.

## Calls

| Call                                                           | What it does                                                         |
| -------------------------------------------------------------- | -------------------------------------------------------------------- |
| `customers->upsert($id, $name)`                                | Create a customer, or update it; repeating changes nothing           |
| `customers->get() / list() / all()`                            | Read one customer, a page, or every customer lazily                  |
| `entitlements->check($customer, $feature, $estimate)`          | Ask whether the customer may do this before the work                 |
| `usage->record($id, $customer, $meter, $quantity)`             | Record usage once; a repeated `id` returns `replayed: true`          |
| `credits->balance($customer)`                                  | `remaining`, `consumed` and `granted`                                |
| `credits->grant($id, $customer, $credits, $reason)`            | Sandbox top-up, once per `id`                                        |
| `credits->ledger() / allLedger()`                              | A customer's ledger, a page or lazily                                |
| `plans->upsertMeter() / upsertPlan() / subscribe() / cancel()` | Define a meter and a plan, put a customer on it, end it              |
| `customerTokens->create($customer, $ttlSeconds) / revoke($id)` | A read-only token for one customer's widget, 30 seconds to 1 hour    |
| `planUsage()`                                                  | The project's plan and this month's usage (limits are unconfirmed)   |
| `Webhooks::verify($rawBody, $signatureHeader, $secrets)`       | Check a webhook delivery against the raw body and the signing secret |

## Retries: only what is safe

A write carries its `id` as the `Idempotency-Key`, so the API applies it at most once. The client retries, with the **same id**, by operation class:

| Operation                                                          | After a lost answer, timeout, 5xx | After a 429 |
| ------------------------------------------------------------------ | --------------------------------- | ----------- |
| reads, the entitlement check                                       | yes                               | yes         |
| keyed mutations (`usage->record`, `credits->grant`)                | yes                               | yes         |
| naturally idempotent writes (upserts, deletes, `plans->subscribe`) | yes                               | yes         |
| a new token each call (`customerTokens->create`)                   | **no**                            | yes         |

The Free plan's monthly volume stop (a 429 whose `details['reason']` is `plan_included_volume`) is never retried. A `Retry-After` longer than 30 seconds is raised, not waited on. 402, 400, 401, 403, 404 and 409 are never retried. cURL itself never repeats a request: the transport uses a fresh connection for every attempt.

Defaults: 10 s for one attempt (connect, headers and body together: a server that drips its body is cut at the timeout), 2 retries (`timeout:`, `maxRetries:`; `0` turns retries off). **Response size limits, for memory safety:** this PHP SDK reads **at most 1 MiB** of a successful answer (not the 16 MB some other SDKs read) and **at most 64 KB** of an error answer (only its status and a short error matter). A larger success is an `Unavailable` with code `invalid_response` and is not retried; a larger error keeps its class by its status. Only a body with at most **20,000 arrays and objects** that fits the free `memory_limit` by a conservative estimate is decoded. The limits are low because PHP needs 50 to 100 times the text for small nested arrays, and running out of memory is a fatal error nothing can catch, so nothing is read that could not then be decoded; real answers (a page of 100 records) are a few tens of kilobytes. The size limits are for bodies. Response headers have no cap of this SDK (libcurl's own built-in limits apply: 100 KB per header line, 300 KB in all, and a hostile head ends in `Unavailable`), and an endless chunked body is bounded only by the per-attempt timeout (memory stays flat meanwhile); no size bound is claimed for those two. Paging also stops with `invalid_response` on a repeated cursor or on the third consecutive empty page that still promises another. When retries run out you get `Unavailable`; it is still safe to try later with the same id.

## Errors

Every error extends `VunlyError` (`errorCode`, `status`, `requestId` to quote when you report a problem, `details`, `retryAfterSeconds`, `isRetryable()`). The message never contains your key, a token, a header or the address.

| Class            | When                                                                                      |
| ---------------- | ----------------------------------------------------------------------------------------- |
| `AuthError`      | 401/403: unknown or revoked key, or a missing scope                                       |
| `LimitReached`   | 402: too few credits; `balance()` and `required()`                                        |
| `InvalidRequest` | other 4xx, and an id that cannot be an Idempotency-Key (refused before any request)       |
| `Unavailable`    | no answer, 429, 5xx after retries, or a malformed answer (`errorCode` `invalid_response`) |

Catch the class, never the message. `Unavailable::isPlanStop()` tells the plan-volume stop from a rate limit.

## Secrets never print

The key lives in a `Secret` that keeps its value outside the object, in a static `WeakMap`. What each facility does with a client, a customer token or a `Secret`:

- `var_dump`, `print_r`, `echo`/string conversion: print `<hidden>` (the client prints its address and `<hidden>`).
- `var_export`, `json_encode`, an `(array)` cast and reflection: show no value at all (an empty or placeholder entry), never the key.
- `serialize`: **throws a `LogicException`** (`unserialize` does too); it does not produce a string, so a client cannot be written to a cache or a queue by accident. Hold the key in your configuration, not in a serialized object.

The constructor parameters that take the key and the address are marked `#[\SensitiveParameter]`, so a stack trace shows them redacted. Error messages never contain the key, a token, a header or the address, and text the server sends in an error (code, message, request id) is cleaned of control, bidi and zero-width characters and cut to a fixed length before it reaches a message or a log line. A customer token's value is read with `$token->token->reveal()` at the one place that hands it to the browser.

The key must be printable ASCII without spaces or control characters; one trailing line break, LF or CRLF (from an `.env` file or a shell), is removed and nothing else is (spaces, tabs, NUL, a lone CR, at either end, are refused), anything else is refused at construction with a message that names the setting and not its value. The address must be an absolute `http(s)` URL with a host and no credentials, query or fragment. The timeout must be between 0.05 and 600 seconds.

## Webhooks

```php
$event = \Vunly\Webhooks::verify($rawBody, $_SERVER['HTTP_VUNLY_SIGNATURE'] ?? '', [$secret]);
```

Pass the **raw** request body, not re-encoded JSON. `verify` accepts bodies up to **1 MiB** (a larger signed body is `invalid_payload`, for memory safety). Vunly's events are far smaller: they carry only an id, a type, a timestamp and a few ids and numbers, so a few hundred bytes to about 1 KB is expected (inferred, not measured, and not guaranteed by a server-side cap). Pass two secrets while you rotate. On a `WebhookError` answer 400 and do not process the payload. An empty or whitespace-only secret (an unset `WEBHOOK_SECRET` read as `''`), an empty list, or a non-string entry throws `InvalidArgumentException` instead of verifying: a signature made with an empty key can be computed by anyone, so such a configuration must fail loudly, not accept forged deliveries.

## Laravel

The package registers itself (Laravel package auto-discovery; nothing to add to `config/app.php`). Set `VUNLY_API_KEY` and `VUNLY_API_URL` in your environment; publish the config only if you want to change a default (`php artisan vendor:publish --tag=vunly-config`).

```php
use Vunly\Client;
use Vunly\Laravel\Facades\Vunly;

Vunly::credits()->balance('acme');                       // the facade forwards to the shared client
public function __construct(private Client $vunly) {}     // or inject it
```

The `vunly` route middleware checks before the work and records after it:

```php
Route::post('/generate', GenerateController::class)->middleware('vunly:generate,tokens,1000');
```

Before the controller it asks whether the customer may use `generate` (pricing in 1000 units of `tokens`); a "no", or any failure of the check, means the controller does not run (403, or 503 when Vunly cannot be reached). After it, for a response below 400, it records the usage once under an id the **server** makes for that request (the SDK's own retries inside the request reuse it), with the quantity the controller set in `$request->attributes->set('vunly.quantity', 1050)` or else the estimate. The response carries `X-Vunly-Credits-Remaining` and `X-Vunly-Replayed`.

**The caller's `X-Request-Id` header is ignored by default, on purpose.** The API charges nothing for an id it has already seen (it answers "replayed"), so if the header were the idempotency id, anyone could repeat the same header and get the work again for free. The cost of ignoring it: a client that retries after a lost response is charged for each attempt that ran. If you want client-driven idempotency, set `VUNLY_TRUST_REQUEST_ID=true` (config `vunly.trust_request_id`, default `false`). **Warning:** with it on, a repeated header runs the work again and is not charged, so enable it only for callers you authenticate. The id is then derived from the header bound to the customer, the feature and the meter (another customer or route cannot reuse it), but there is no time window: if your work is expensive, also expire the ids yourself.

The billed customer is the authenticated user's id by default; change it once with `MeterUsage::resolveCustomerUsing(fn (Request $r) => ...)`. A working application is in `examples/laravel`.

`php artisan config:cache` writes the resolved key into `bootstrap/cache/config.php`: keep that file out of version control and out of web-readable paths.

## Supported versions

The unit tests ran on PHP 8.2.35, 8.3.36, 8.4.27 and 8.5.12 (8.2 is the floor because `#[\SensitiveParameter]` keeps the key out of stack traces from 8.2 on; versions between patch releases, and older than 8.2, were not run). The Laravel integration is verified on Laravel 12 (PHP 8.2) and Laravel 13 (PHP 8.3, 8.4 and 8.5); Laravel 12 on PHP 8.3 or newer was not run.

## Tests

The tests use a local server and synthetic keys: they need no Vunly API and no key. From the `php/` folder (Composer resolves the test tools, which needs access to Packagist):

```sh
composer update --no-interaction
vendor/bin/phpunit --no-progress
```

## Known limitations

Status: **Implemented**. These are the limitations recorded for this SDK. A limitation does not mean the SDK is withdrawn; read it before you use the SDK.

- The Composer name `vunly/vunly` is a placeholder and is not on Packagist; use it from a local checkout with a path repository.
- Calls are synchronous; a wait for Retry-After blocks the PHP process. In a long-running worker such as Octane, the middleware's customer resolver is shared by the whole process.
- Webhook endpoint management, the delivery log, recipes, activity, exports, credit packs and billing calls (diagnostics and reconciliation) are available through the REST API; this SDK does not wrap them. Changing a plan, cancelling a pending plan change, reading a customer's own usage with a customer token and the credits usage summary are not wrapped either.
- Run on Linux only; Windows and macOS were not run.
