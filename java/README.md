# vunly (Java)

Java client for the Vunly sandbox API: record usage, check entitlements, grant credits. Java 17 or newer (tested on 17, 21 and 25), **no runtime dependencies**, immutable and thread-safe. Sandbox only: no real payments.

## Install

**Install Java from this repository.** The repository is public, packages are not published to registries. Clone the repository, then follow the steps below from the `java/` folder. They were run from a fresh clone on Java 17.

Build it and install it into your local Maven repository (Maven 3.9 and a JDK 17 or newer):

```sh
cd java
mvn -B -ntp install -DskipTests
```

Then depend on it in your own `pom.xml`. The coordinates are local ones: they exist in your local repository after the step above and nowhere else.

```xml
<dependency>
  <groupId>com.vunly</groupId>
  <artifactId>vunly</artifactId>
  <version>0.1.0</version>
</dependency>
```

This library is not published to a package registry, so there is no registry command.

## Use

This program compiles and runs as it stands (`Main.java`, with the library on the class path):

```java
import com.vunly.*;

public class Main {
  public static void main(String[] args) {
    // The key comes from the environment. Never write it into source, tests, logs or prompts.
    // A trailing newline (a secret mounted from a file) is trimmed for you.
    Vunly vunly = Vunly.builder()
        .apiKey(System.getenv("VUNLY_API_KEY"))
        .baseUrl(System.getenv("VUNLY_API_URL"))
        .build();

    vunly.customers().upsert("cus_demo_01");

    Entitlement decision = vunly.entitlements().check("cus_demo_01", "generate", new Estimate("tokens", 1050));
    if (!decision.allowed()) {
      throw new IllegalStateException(decision.reason()); // do not run the work
    }

    // ... do the work, then record it once. The id names this unit of work; retries use the SAME id.
    try {
      UsageResult result = vunly.usage().record("req_2026_0001", "cus_demo_01", "tokens", 1050);
      System.out.println(result.remaining() + " " + result.replayed());
    } catch (LimitReachedException e) {
      // 402: not enough credits, nothing was debited
    } catch (UnavailableException e) {
      // the SDK already retried; fail closed: do not run the work
    }
  }
}
```

**What the sandbox project needs first**, or the program stops at the check or answers 402: a meter with the key `tokens`, a plan that includes the feature `generate` with that meter, the customer on that plan, and credits granted. The guided setup in the console does all four (the AI subscription recipe); with the REST API they are `plans().upsertMeter`, `plans().upsertPlan`, `plans().subscribe` and `credits().grant`.

`baseUrl` is the API address. A trailing slash or `/v1` is accepted and means the same address. It must be the address only (no credentials, query or fragment); the SDK refuses anything else.

A complete Spring Boot application is in [`examples/spring-boot`](examples/spring-boot): the client is one `@Bean` built from `VUNLY_API_URL` and `VUNLY_API_KEY`, and a controller asks, works and records. The controller issues the idempotency id itself: it does **not** use a caller's `X-Request-Id` by default, because a replay of an id does not charge again, so a caller who re-sends the same header would get the work done again for free (set `vunly.trust-request-id=true` only for an id you issue yourself, never one a client can choose).

## Calls

| Call                                                    | What it does                                                   |
| ------------------------------------------------------- | -------------------------------------------------------------- |
| `customers().upsert(id[, name])`                        | Create a customer, or update it; repeating changes nothing     |
| `customers().get / list / listAll`                      | Read one customer, a page, or every customer lazily            |
| `entitlements().check(customer, feature[, estimate])`   | Ask whether the customer may do this before the work           |
| `usage().record(id, customer, meter, quantity)`         | Record usage once; a repeated `id` returns `replayed: true`    |
| `credits().balance(customer)`                           | `remaining`, `consumed` and `granted`                          |
| `credits().grant(id, customer, credits[, reason])`      | Sandbox top-up, once per `id`                                  |
| `credits().ledger / ledgerAll`                          | A customer's ledger, a page or lazily                          |
| `plans().upsertMeter / upsertPlan / subscribe / cancel` | Define a meter and a plan, put a customer on it, end it        |
| `customerTokens().create(customer[, ttl]) / revoke(id)` | A read-only token for one customer's widget, 30 s to 1 hour    |
| `planUsage()`                                           | The project's plan and this month's usage (limits unconfirmed) |
| `Webhooks.verify(payload, header, secrets)`             | Check a webhook delivery against the raw body and the secret   |

## Retries: only what is safe

A write carries its `id` as the `Idempotency-Key`, so the API applies it at most once. The client retries, with the **same id**, by operation class:

| Operation                                                   | After a lost answer, timeout, 5xx | After a 429 |
| ----------------------------------------------------------- | --------------------------------- | ----------- |
| reads, the entitlement check                                | yes                               | yes         |
| keyed mutations (`usage().record`, `credits().grant`)       | yes                               | yes         |
| naturally idempotent writes (upserts, deletes, `subscribe`) | yes                               | yes         |
| a new token each call (`customerTokens().create`)           | **no**                            | yes         |

The Free plan's monthly volume stop (a 429 whose details say `reason=plan_included_volume`) is never retried. A `Retry-After` longer than 30 seconds is surfaced, not waited on. 402, 400, 401, 403, 404 and 409 are never retried.

A response is read up to 16 MB, then refused. **Memory:** the JSON is parsed into Java objects, which take about 22 times the size of the body for deeply nested tiny arrays (measured LOCAL: a 10.4 MB body of `[[1],[1],…]` added 230 MB of heap). The worst case for the 16 MB cap is therefore about 350 to 430 MB of heap; give the process more than that, or a heap smaller than that can end in an `OutOfMemoryError` that the SDK cannot turn into an exception. A normal API answer is a few kilobytes.

Defaults: 10 s per attempt, 2 retries (`.timeout(Duration)`, `.maxRetries(int)`; `0` turns retries off). When they run out you get `UnavailableException`; it is still safe to try later with the same id. Interrupting the thread stops a call at once (`UnavailableException` with code `cancelled`, interrupt flag restored).

## Exceptions

Unchecked, all extending `VunlyException` (`code()`, `status()`, `requestId()` to quote when you report a problem, `details()`, `retryAfter()`). The message never contains your key: a key that cannot be sent in a header is refused by `build()` with a message that does not quote it, and the message, code, request id and every string in the details a server sends back are cleaned of control, bidirectional and zero-width characters, bounded, and any echo of your key is replaced by `[redacted]` before they are put in an exception. Every redirect (3xx) is never followed and never retried, whatever its body; it is an `UnavailableException` with code `unexpected_redirect` (check the address you configured).

**Do not turn on the JDK's HTTP logging in a deployed program.** `-Djdk.httpclient.HttpClient.log=headers` (or `all`) makes the JDK itself print every request header, including `Authorization: Bearer <your key>`, to its `jdk.internal.net.http` loggers, and the JDK offers no way for a library to keep one header out of it. The level `requests` did not print the key on the JDKs tested (17.0.20.1 and 25.0.4.1); do not rely on that, it is JDK behaviour and not something the SDK controls. The SDK cannot prevent this; leave the option off, or keep that logger's output out of shared logs. A custom `HttpClient` must be built with `followRedirects(NEVER)` or `build()` refuses it.

| Class                     | When                                                                                        |
| ------------------------- | ------------------------------------------------------------------------------------------- |
| `AuthException`           | 401/403: unknown or revoked key, or a missing scope                                         |
| `LimitReachedException`   | 402: too few credits; `balance()` and `required()`                                          |
| `InvalidRequestException` | other 4xx, and an id that cannot be an Idempotency-Key (refused before any request)         |
| `UnavailableException`    | no answer, 429, 5xx after retries, a malformed answer (`invalid_response`), or interruption |

Match on the class or on `code()`; never on the message.

## Webhooks

```java
Webhooks.Event event = Webhooks.verify(rawBody, request.getHeader("Vunly-Signature"), List.of(secret));
```

Pass the **raw** request body bytes, not re-encoded JSON. Pass two secrets while you rotate. On a `Webhooks.WebhookException` answer 400 and do not process the payload.

## Supported Java versions

LTS releases only: **17 (the floor), 21 and 25**.

## Tests

The tests use an in-process HTTP server and synthetic keys: they need no Vunly API and no key. From the `java/` folder:

```sh
mvn -B -ntp clean test
```

## Known limitations

Status: **Implemented**. These are the limitations recorded for this SDK. A limitation does not mean the SDK is withdrawn; read it before you use the SDK.

- With -Djdk.httpclient.HttpClient.log=headers or =all the JDK client prints the Authorization header, including the key (not with =requests). Do not enable those logging levels.
- Calls block the calling thread; there is no async API.
- Webhook endpoint management, the delivery log, recipes, activity, exports, credit packs and billing calls (diagnostics and reconciliation) are available through the REST API; this SDK does not wrap them. Changing a plan, cancelling a pending plan change, reading a customer's own usage with a customer token and the credits usage summary are not wrapped either.
- Run on Linux only; Windows and macOS were not run. JDKs 22 to 24 and 26 were not run.
