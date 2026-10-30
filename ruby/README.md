# vunly (Ruby)

Ruby client for the Vunly sandbox API: record usage, check entitlements, grant credits. **No runtime dependencies** (the Ruby standard library only), safe to share between threads, with a Rails controller concern. Sandbox only: no real payments.

> The gem name `vunly` is not reserved on RubyGems. Do not run `gem install vunly`: point your `Gemfile` at the clone.

## Install

**Install Ruby from this repository.** The repository is public, packages are not published to registries. Clone the repository, then follow the steps below from the `ruby/` folder.

In your project, add a `path:` line to the `Gemfile` and run `bundle install`. `/path/to/vunly-sdks/ruby` is the `ruby/` folder of the clone, the folder that holds this README:

```ruby
source "https://rubygems.org"
gem "vunly", path: "/path/to/vunly-sdks/ruby"
```

```sh
bundle install
```

The gem has no runtime dependency, so Bundler fetches nothing for it.

This library is not published to a package registry, so there is no registry command.

## Before you run it: the sandbox

The program below assumes, in your sandbox project: a meter `tokens` (for example 1000 tokens per credit), a plan `starter` that includes the feature `generate` and some credits, and a sandbox API key (`vk_test_...`) with the scopes the onboarding wizard issues. The program creates the customer and puts it on the plan. Export `VUNLY_API_KEY` and `VUNLY_API_URL` (the API address from the setup step).

## Run it

```ruby
# frozen_string_literal: true

require "vunly"

# The key and the address come from your environment. Never write them into source, tests, logs or prompts.
client = Vunly::Client.new(api_key: ENV.fetch("VUNLY_API_KEY"), base_url: ENV.fetch("VUNLY_API_URL"))

client.customers.upsert("cus_demo_01")
client.plans.subscribe("cus_demo_01", "starter")

decision = client.entitlements.check(customer: "cus_demo_01", feature: "generate",
                                     estimate: { meter: "tokens", quantity: 1050 })
abort("not allowed: #{decision.reason}") unless decision.allowed? # a check that cannot be answered raises: fail closed

# ... do the work, then record it once. The id names this unit of work; a retry uses the SAME id.
result = client.usage.record(id: "req_2026_0001", customer: "cus_demo_01", meter: "tokens", quantity: 1050)
puts "remaining=#{result.remaining} replayed=#{result.replayed?}"
```

(The same file is `examples/quickstart.rb`.) Run it twice: the second time the same id answers `replayed=true` and debits nothing. Handle the outcomes you care about by class:

```ruby
begin
  client.usage.record(id: job_id, customer: "cus_demo_01", meter: "tokens", quantity: 1050)
rescue Vunly::LimitReachedError
  # 402: not enough credits, nothing was debited
rescue Vunly::UnavailableError
  # after the SDK's own retries: fail closed, and repeat later with the SAME id
end
```

`base_url:` is the API address. A trailing slash or `/v1` is accepted and means the same address; an address with credentials, a query or a fragment is refused. There is no default address. The API key may have a trailing newline (it is dropped); anything else that is not printable ASCII is refused, naming the setting and never the value.

## Calls

| Call                                                      | What it does                                                       |
| --------------------------------------------------------- | ------------------------------------------------------------------ |
| `customers.upsert(id, name: nil)`                         | Create a customer, or update it; repeating changes nothing         |
| `customers.get / list / each`                             | Read one customer, a page, or every customer lazily                |
| `entitlements.check(customer:, feature:, estimate: nil)`  | Ask whether the customer may do this before the work               |
| `usage.record(id:, customer:, meter:, quantity:)`         | Record usage once; a repeated `id` returns `replayed?` true        |
| `credits.balance(customer)`                               | `remaining`, `consumed` and `granted`                              |
| `credits.grant(id:, customer:, credits:, reason: nil)`    | Sandbox top-up, once per `id`                                      |
| `credits.ledger / each_ledger_entry`                      | A customer's ledger, a page or lazily                              |
| `plans.upsert_meter / upsert_plan / subscribe / cancel`   | Define a meter and a plan, put a customer on it, end it            |
| `customer_tokens.create(customer, ttl: nil) / revoke(id)` | A read-only token for one customer's widget, 30 seconds to 1 hour  |
| `plan_usage`                                              | The project's plan and this month's usage (limits are unconfirmed) |
| `Vunly.verify_webhook(raw_body, header, secrets)`         | Check a webhook delivery against the raw body and the secret       |

Credits are integers and the SDK never computes them: `quantity` is in the meter's own units (tokens, say) and the API converts it. A paging cursor that repeats stops the iteration with an `invalid_response` error instead of looping.

## Retries: only what is safe

A write carries its `id` as the `Idempotency-Key`, so the API applies it at most once. The client retries, with the **same id and body**, by operation class:

| Operation                                                   | After a lost answer, timeout, 5xx | After a 429 |
| ----------------------------------------------------------- | --------------------------------- | ----------- |
| reads, the entitlement check                                | yes                               | yes         |
| keyed mutations (`usage.record`, `credits.grant`)           | yes                               | yes         |
| naturally idempotent writes (upserts, deletes, `subscribe`) | yes                               | yes         |
| a new token each call (`customer_tokens.create`)            | **no**                            | yes         |

The Free plan's monthly volume stop (a 429 whose `details["reason"]` is `plan_included_volume`) is never retried. A `Retry-After` longer than 30 seconds, or too large to represent, is a very long wait: it is raised at once (`retry_after_seconds`, at most 86400) and never retried early. 402, 400, 401, 403, 404 and 409 are never retried, and a redirect is `UnavailableError` with code `unexpected_redirect` (never followed, never retried). The wait between retries is `min(8 s, 0.25 s x 2^(n-1))` with a random factor between 0.5 and 1.

Defaults: 10 seconds **per attempt as a whole** (connect, headers and body: a server that answers at once and drips its body is cut at the timeout), 2 retries (`timeout:` from 0.001 to 3600 seconds, `max_retries:`; `0` turns retries off). The response body is capped at 16 MB while it streams. No redirect is followed, and no proxy variable is read. When the retries run out you get `Vunly::UnavailableError`; it is still safe to try later with the same id.

## Errors

Every error is a `Vunly::Error` with `code`, `status` (0 when there was no response), `request_id` (quote it when you report a problem), `details` and `retry_after_seconds`. The message never contains your key, text the server sent is cleaned (control, bidirectional and zero-width characters removed) and bounded, and an error never has a `cause` (the HTTP library's exception is dropped on purpose).

| Class                        | When                                                                                |
| ---------------------------- | ----------------------------------------------------------------------------------- |
| `Vunly::AuthError`           | 401/403: unknown or revoked key, or a missing scope                                 |
| `Vunly::LimitReachedError`   | 402: too few credits; `balance` and `required`                                      |
| `Vunly::InvalidRequestError` | other 4xx, and an id that cannot be an Idempotency-Key (refused before any request) |
| `Vunly::UnavailableError`    | no answer, 429, 5xx, a redirect, or a malformed answer (`invalid_response`)         |

Rescue the class or match on `error.code`; never on the message. `error.retryable?` says whether trying again later with the same id can help.

## Rails

The gem does not depend on Rails. When Rails is loaded, `require "vunly"` (from the Gemfile) also loads a Railtie, and `Vunly::Metered` is available as a controller concern.

```ruby
# config/initializers/vunly.rb
Vunly.configure do |config|
  config.api_key  = ENV.fetch("VUNLY_API_KEY")  # a sandbox key, vk_test_...
  config.base_url = ENV.fetch("VUNLY_API_URL")
end
```

```ruby
class GenerateController < ApplicationController
  include Vunly::Metered

  # check BEFORE the action (fail closed), record ONCE after it
  metered feature: "generate", meter: "tokens", only: :create,
          customer: ->(controller) { controller.params.require(:customer) }

  def create
    output = MyModel.run(params.require(:prompt))
    vunly_quantity output.token_count # what the work used, in the meter's units
    render json: { output: output.text }
  end
end
```

- Not entitled: 403 (`no_active_plan`, `feature_not_in_plan`) or 402 (`insufficient_credits`), and the action does not run.
- The check cannot be answered: **503, the action does not run** (fail closed).
- Not enough credits at record time (nothing was debited): 402; the usage could not be recorded: 503 with the work's answer withheld.
- To put the outcome in your own answer, call `vunly_record!(quantity)` in the action; it returns the `UsageResult` (`remaining`, `replayed?`).
- **The idempotency id is generated on the server for each request.** A header the client sends (`X-Request-Id`) is **not** trusted by default: whoever can repeat the header could make billable work run again without a charge. Name the unit of work yourself with `id: ->(c) { c.params[:order_id] }`, or, for clients you trust, opt in with `trust_request_id: true`: the id is then derived from the header and bound to the customer, and a client that retries with the same header is not billed twice.
- Vunly refusing the request as malformed (an unknown customer, a bad id) is answered 404 or 400 with the API's code and no detail; your own key being refused is a generic 502 (the cause goes to the log).
- Every answer is a method you can override in the controller (`vunly_denied`, `vunly_limit_reached`, `vunly_unavailable`, `vunly_invalid_request`, `vunly_auth_failed`).

`examples/rails` is a complete minimal API-only application (initializer and controller).

## Webhooks

```ruby
event = Vunly.verify_webhook(request.raw_post, request.headers["Vunly-Signature"], [ENV.fetch("VUNLY_WEBHOOK_SECRET")])
```

Pass the **raw** request body, not re-encoded JSON. Pass two secrets while you rotate. Empty or blank secrets and an empty list are refused (`ArgumentError`). On a `Vunly::WebhookError` answer 400 and do not process the payload.

## Secrets never print

The key is held in an object whose `inspect`, `to_s`, `pp`, `to_json`, `Marshal.dump` and `YAML.dump` say `<hidden>` or refuse, so a client or a configuration printed by mistake shows no key. A `CustomerToken` shows its id only; read the token on purpose with `.token`.

## Versions verified

Ruby 3.3.13 and 4.0.9 (the gem requires Ruby 3.3 or newer; other Ruby releases were not run). The Rails example was verified on Rails 8.1.4 (Puma 8.0.2). The API major is `/v1`.

## Tests

The tests use a local server and synthetic keys: they need no Vunly API and no key. They use the minitest that ships with Ruby, so they need no gem. From the `ruby/` folder:

```sh
ruby -Ilib -Itest -e 'Dir["test/*_test.rb"].sort.each { |f| require "./#{f}" }'
```

## Known limitations

Status: **Implemented**. These are the limitations recorded for this SDK. A limitation does not mean the SDK is withdrawn; read it before you use the SDK.

- The gem name `vunly` is a placeholder and is not reserved on RubyGems; install it from a checkout, never `gem install vunly`.
- Ruby 3.3 receives security fixes only until 2027-03-31; the floor moves to 3.4 then.
- Only Rails 8.1.4 was run; Ruby 3.4 was not run.
- Webhook endpoint management, the delivery log, recipes, activity, exports, credit packs and billing calls (diagnostics and reconciliation) are available through the REST API; this SDK does not wrap them. Changing a plan, cancelling a pending plan change, reading a customer's own usage with a customer token and the credits usage summary are not wrapped either.
- Run on Linux only; Windows and macOS were not run.
