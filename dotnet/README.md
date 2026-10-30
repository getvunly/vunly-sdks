# Vunly for .NET

.NET client for the Vunly sandbox API: record usage, check entitlements, grant credits. **.NET 8 and .NET 10** (both LTS; tested on both), **no dependencies** beyond the base class library, async with `CancellationToken`, nullable annotations, immutable records. Sandbox only: no real payments.

> **Not published.** The package ids `Vunly` and `Vunly.Extensions.DependencyInjection` are not reserved on nuget.org: **anyone could register them there**. Do not run a bare `dotnet add package Vunly` against nuget.org (you could install somebody else's package). Install the packages you built yourself from a local folder, as below.
>
> .NET 8 reaches the end of Microsoft's support on 2026-11-10; the `net8.0` target is removed in the first release after that date.

## Install

**Install .NET from this repository.** The repository is public, packages are not published to registries. Clone the repository, then follow the steps below from the `dotnet/` folder.

1. Build the packages from the clone into a folder (`.NET 10 SDK`; it produces `Vunly.0.1.0.nupkg` and `Vunly.Extensions.DependencyInjection.0.1.0.nupkg`):

```sh
cd dotnet
dotnet pack src/Vunly -c Release -o ~/vunly-packages
dotnet pack src/Vunly.Extensions.DependencyInjection -c Release -o ~/vunly-packages
```

2. In your project, put a `nuget.config` next to the `.csproj`. It adds the folder as a package source **and maps the ids `Vunly*` to it alone**, so a package of that name on nuget.org is never considered (dependency confusion). Everything else (`Microsoft.Extensions.*`) still comes from nuget.org:

```xml
<?xml version="1.0" encoding="utf-8"?>
<configuration>
  <packageSources>
    <clear />
    <add key="vunly-local" value="/home/you/vunly-packages" />
    <add key="nuget.org" value="https://api.nuget.org/v3/index.json" />
  </packageSources>
  <packageSourceMapping>
    <packageSource key="vunly-local"><package pattern="Vunly*" /></packageSource>
    <packageSource key="nuget.org"><package pattern="*" /></packageSource>
  </packageSourceMapping>
</configuration>
```

3. Add the packages, naming the version (the DI package is `AddVunly()` for ASP.NET Core and the generic host):

```sh
dotnet add package Vunly --version 0.1.0
dotnet add package Vunly.Extensions.DependencyInjection --version 0.1.0
```

`dotnet nuget add source <folder> --name vunly-local` writes the source line for you, but not the source mapping: add the `packageSourceMapping` block by hand. Without it the folder and nuget.org compete for the id.

This library is not published to a package registry, so there is no registry command.

## Sandbox prerequisites

A call that records usage needs the project to know what it records and who pays. In the sandbox, before the first `RecordAsync`:

- a **meter** (`tokens` in the examples) that says how many units one credit is worth: `Plans.UpsertMeterAsync("tokens", 1000)`;
- a **feature** (`generate`) that the entitlement check asks about, listed on a **plan** that uses that meter and includes credits: `Plans.UpsertPlanAsync(new PlanSpec("starter", "Starter", 100, "tokens", new[] { "generate" }))`;
- the **customer**: `Customers.UpsertAsync("cus_demo_01")`;
- the customer's **subscription** to that plan: `Plans.SubscribeAsync("cus_demo_01", "starter")`;
- **credits**: the plan's included credits arrive with the subscription; add more with `Credits.GrantAsync("grant_2026_0001", "cus_demo_01", 500)`.

Without them `CheckAsync` answers "not allowed" (no plan or feature) and `RecordAsync` fails with a 4xx (an unknown meter or customer) or a `LimitReachedException` (no credits). Your key needs the scopes for what you call: defining meters and plans needs a key with the admin scopes (do it once, from the console or a setup script); the key the setup wizard makes covers customers, subscriptions, credit grants, usage, balances and entitlement checks, which is what the service itself calls.

## Use

```csharp
using Vunly;

// The key comes from your configuration or secret store. Never write it into source, tests, logs or prompts.
var vunly = new VunlyClient(new VunlyClientOptions
{
    ApiKey = Environment.GetEnvironmentVariable("VUNLY_API_KEY"),
    BaseUrl = Environment.GetEnvironmentVariable("VUNLY_API_URL"),
});

await vunly.Customers.UpsertAsync("cus_demo_01");

var decision = await vunly.Entitlements.CheckAsync("cus_demo_01", "generate", new Estimate("tokens", 1050));
if (!decision.Allowed)
{
    throw new InvalidOperationException(decision.Reason); // do not run the work
}

// ... do the work, then record it once. The id names this unit of work; retries use the SAME id.
try
{
    var result = await vunly.Usage.RecordAsync("req_2026_0001", "cus_demo_01", "tokens", 1050);
    Console.WriteLine($"{result.Remaining} {result.Replayed}");
}
catch (LimitReachedException)
{
    // 402: not enough credits, nothing was debited
}
catch (UnavailableException)
{
    // the SDK already retried; fail closed: do not run the work
}
```

`BaseUrl` is the API address. A trailing slash or `/v1` is accepted and means the same address.

### ASP.NET Core and dependency injection

```csharp
builder.Services.AddVunly(o =>
{
    o.ApiKey = builder.Configuration["VUNLY_API_KEY"];
    o.BaseUrl = builder.Configuration["VUNLY_API_URL"];
});
// then take VunlyClient as a constructor or minimal-API parameter
```

`AddVunly` registers one shared `VunlyClient` over an `IHttpClientFactory` client that never follows redirects, and validates the options when the host starts, so a missing key stops the application at boot. A complete minimal-API application is in [`examples/AspNetCore`](examples/AspNetCore). It validates the body (a bad request is a 4xx, an unknown customer a 404, a refused service key a generic 502) and **generates the idempotency id itself**: the work runs before usage is recorded, so taking the id from a caller's `X-Request-Id` would let a repeated header run the work again and only replay the record (billable work for free). Trusting the caller's header is an explicit option of the example (`VUNLY_TRUST_CLIENT_REQUEST_ID=true`), and it then also returns the earlier answer instead of running the work again.

## Calls

Every call is `async`, ends in `Async` and takes an optional `CancellationToken` last.

| Call                                                                      | What it does                                                       |
| ------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| `Customers.UpsertAsync(id[, name])`                                       | Create a customer, or update it; repeating changes nothing         |
| `Customers.GetAsync / ListAsync / ListAllAsync`                           | Read one customer, a page, or every customer as `IAsyncEnumerable` |
| `Entitlements.CheckAsync(customer, feature[, estimate])`                  | Ask whether the customer may do this before the work               |
| `Usage.RecordAsync(id, customer, meter, quantity)`                        | Record usage once; a repeated `id` returns `Replayed == true`      |
| `Credits.GetBalanceAsync(customer)`                                       | `Remaining`, `Consumed` and `Granted`                              |
| `Credits.GrantAsync(id, customer, credits[, reason])`                     | Sandbox top-up, once per `id`                                      |
| `Credits.GetLedgerAsync / GetLedgerAllAsync`                              | A customer's ledger, a page or lazily                              |
| `Plans.UpsertMeterAsync / UpsertPlanAsync / SubscribeAsync / CancelAsync` | Define a meter and a plan, put a customer on it, end it            |
| `CustomerTokens.CreateAsync(customer[, ttl]) / RevokeAsync(id)`           | A read-only token for one customer's widget, 30 s to 1 hour        |
| `GetPlanUsageAsync()`                                                     | The project's plan and this month's usage (limits unconfirmed)     |
| `Webhooks.Verify(payload, header, secrets)`                               | Check a webhook delivery against the raw body and the secret       |

## Retries: only what is safe

A write carries its `id` as the `Idempotency-Key`, so the API applies it at most once. The client retries, with the **same id**, by operation class:

| Operation                                                        | After a lost answer, timeout, 5xx | After a 429 |
| ---------------------------------------------------------------- | --------------------------------- | ----------- |
| reads, the entitlement check                                     | yes                               | yes         |
| keyed mutations (`RecordAsync`, `GrantAsync`)                    | yes                               | yes         |
| naturally idempotent writes (upserts, deletes, `SubscribeAsync`) | yes                               | yes         |
| a new token each call (`CustomerTokens.CreateAsync`)             | **no**                            | yes         |

The Free plan's monthly volume stop (a 429 whose details say `reason=plan_included_volume`) is never retried. `Retry-After` is read like in every Vunly SDK: a number of seconds up to 30 is the wait; a number above 30, or one too large to represent (`1e12`, `Infinity`, hundreds of digits), is a very long wait: the call fails at once with `UnavailableException` and `RetryAfter` set (capped at one day), and is never retried early; `0`, an HTTP date or anything else that is not a number means no directive, and the normal jittered backoff applies. 402, 400, 401, 403, 404 and 409 are never retried. A redirect (3xx) is never followed and never retried: it is `UnavailableException` with code `unexpected_redirect` (check `BaseUrl`).

Defaults: 10 s per attempt (above zero, at most 10 minutes), 2 retries (`Timeout`, `MaxRetries` in `VunlyClientOptions`; `0` turns retries off, at most 10). `MaxRetries = 0` means one request on the wire: a body-less request carries an empty body so that `SocketsHttpHandler` does not repeat it by itself (it repeats a body-less request up to 3 times when a connection closes before an answer; with retries on, that replay of a read stays, within the attempt's time limit). When the retries run out you get `UnavailableException`; it is still safe to try later with the same id.

One final line break (LF or CRLF) of `ApiKey` is dropped; a space, tab, NUL, lone CR, control or non-ASCII character in it, at either end or inside, is refused, naming the setting, never the value. `BaseUrl` must be the address only: credentials (`user:password@`), a query string and a fragment are refused. With your own `HttpClient`, its handler must not follow redirects; if it does, the SDK refuses an answer that came from another host than `BaseUrl` (`unexpected_redirect`), but the request, with the key, has already gone to that host: configure the handler correctly. **Cancelling the token** stops a call at once and throws `OperationCanceledException`, as every .NET API does; that is your decision, not an outage.

## Exceptions

All extend `VunlyException` (`Code`, `Status`, `RequestId` to quote when you report a problem, `Details`, `RetryAfter`). The message never contains your key. Text that came from the server is data: in `Message`, `Code` and `RequestId` control characters (LF, CR, ESC, DEL, C1), bidirectional and zero-width characters become spaces, and the lengths are bounded (500, 64 and 128 characters), so they are safe in a log line.

| Class                     | When                                                                                                                                                                   |
| ------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `AuthException`           | 401/403: unknown or revoked key, or a missing scope                                                                                                                    |
| `LimitReachedException`   | 402: too few credits; `Balance` and `Required`                                                                                                                         |
| `InvalidRequestException` | other 4xx, and an id that is null or cannot be an Idempotency-Key (refused before any request)                                                                         |
| `UnavailableException`    | no answer, 429, 5xx after retries, a redirect (`unexpected_redirect`), or a malformed answer (`invalid_response`, also a list whose next-page cursor does not advance) |

Match on the type or on `Code`; never on the message.

## Webhooks

```csharp
var evt = Webhooks.Verify(rawBodyBytes, Request.Headers["Vunly-Signature"], new[] { secret });
```

Pass the **raw** request body, not re-serialised JSON. Pass two secrets while you rotate. On a `WebhookException` answer 400 and do not process the payload. An empty list, or an empty or blank secret in it, is a configuration error: `Verify` throws `ArgumentException` before it looks at the delivery (a signature made with an empty key can be computed by anyone). A payload over 1 MiB or nested deeper than 32 levels is `invalid_payload`.

## Supported .NET versions

**.NET 8 and .NET 10, both LTS** (.NET 9 is not LTS and is not verified). Unit tests run on the .NET 8.0.427 and .NET 10.0.403 SDKs; the ASP.NET Core example on .NET 10. Linux only: Windows and macOS were not run. .NET Framework and .NET Standard are not supported.

## Tests

The tests use an in-process HTTP listener and synthetic keys: they need no Vunly API and no key. From the `dotnet/` folder, on the .NET 10 SDK (use `net8.0` on the .NET 8 SDK):

```sh
cd tests/Vunly.Tests
dotnet test -f net10.0 -p:TargetFrameworks=net10.0
```

## Known limitations

Status: **Implemented**. These are the limitations recorded for this SDK. A limitation does not mean the SDK is withdrawn; read it before you use the SDK.

- .NET 8 reaches the end of Microsoft's support on 2026-11-10; .NET 10 is supported until 2028-11-14.
- The package ids are placeholders and are not reserved on nuget.org; install from the local folder procedure in the README, never a bare `dotnet add package`.
- Webhook endpoint management, the delivery log, recipes, activity, exports, credit packs and billing calls (diagnostics and reconciliation) are available through the REST API; this SDK does not wrap them. Changing a plan, cancelling a pending plan change, reading a customer's own usage with a customer token and the credits usage summary are not wrapped either.
- Run on Linux only; Windows and macOS were not run. Native AOT publishing was not tested and the package is not signed.
