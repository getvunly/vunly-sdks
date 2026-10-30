# frozen_string_literal: true

require_relative "test_helper"

class ClientTest < Minitest::Test
  include TestSupport

  # ---- construction ------------------------------------------------------------------------------------------------

  def test_an_empty_key_fails_at_construction_and_names_the_setting
    ["", "   ", nil].each do |key|
      error = assert_raises(Vunly::ConfigurationError) { Vunly::Client.new(api_key: key, base_url: "http://127.0.0.1:1") }
      assert_includes error.message, "api_key"
    end
  end

  def test_a_malformed_key_is_refused_without_repeating_it
    bad = "vk_test_synthetic_bad\nX-Injected: 1"
    error = assert_raises(Vunly::ConfigurationError) { Vunly::Client.new(api_key: bad, base_url: "http://127.0.0.1:1") }
    refute_includes error.message, "vk_test_synthetic"
    assert_nil error.cause
  end

  def test_the_base_url_is_required_and_has_no_default
    error = assert_raises(Vunly::ConfigurationError) { Vunly::Client.new(api_key: SYNTHETIC_KEY, base_url: nil) }
    assert_includes error.message, "base_url"
  end

  def test_the_base_url_is_normalised
    {
      "http://127.0.0.1:8787" => "http://127.0.0.1:8787",
      "http://127.0.0.1:8787/" => "http://127.0.0.1:8787",
      "http://127.0.0.1:8787/v1" => "http://127.0.0.1:8787",
      "http://127.0.0.1:8787/v1/" => "http://127.0.0.1:8787",
      "https://api.example.com/V1" => "https://api.example.com",
      "  https://api.example.com//  " => "https://api.example.com",
      "https://example.com/gateway/v1" => "https://example.com/gateway"
    }.each do |given, want|
      assert_equal want, Vunly::Client.new(api_key: SYNTHETIC_KEY, base_url: given).base_url, given
    end
  end

  def test_a_base_url_that_is_not_an_address_is_refused_and_never_repeated
    ["ftp://host", "not a url", "/relative", "http://", "https://user:hunter2synthetic@host",
     "https://host?x=1"].each do |url|
      error = assert_raises(Vunly::ConfigurationError, url) { Vunly::Client.new(api_key: SYNTHETIC_KEY, base_url: url) }
      refute_includes error.message, "hunter2synthetic"
      refute_includes error.message, url unless url.size < 6
      assert_nil error.cause
    end
  end

  def test_timeout_and_retries_are_validated
    assert_raises(Vunly::ConfigurationError) { Vunly::Client.new(api_key: SYNTHETIC_KEY, base_url: "http://h", timeout: 0) }
    assert_raises(Vunly::ConfigurationError) { Vunly::Client.new(api_key: SYNTHETIC_KEY, base_url: "http://h", max_retries: -1) }
    assert_raises(Vunly::ConfigurationError) { Vunly::Client.new(api_key: SYNTHETIC_KEY, base_url: "http://h", max_retries: 1.5) }
  end

  # ---- the key never prints ----------------------------------------------------------------------------------------

  def test_the_key_never_appears_in_any_dump_of_the_client
    client = Vunly::Client.new(api_key: SYNTHETIC_KEY, base_url: "http://127.0.0.1:1")
    require "pp"
    outputs = [client.inspect, client.to_s, client.pretty_inspect, client.to_json, "client: #{client}", client.as_json.to_s]
    outputs.each { |text| refute_includes text, "synthetic_ruby_sdk", text }
    assert_raises(TypeError) { Marshal.dump(client) }
    secret = client.instance_variable_get(:@transport).instance_variable_get(:@api_key)
    [secret.inspect, secret.to_s, secret.to_json, secret.pretty_inspect].each { |text| refute_includes text, "synthetic" }
    assert_raises(TypeError) { Marshal.dump(secret) }
  end

  def test_the_configuration_never_prints_the_key
    config = Vunly::Configuration.new
    config.api_key = SYNTHETIC_KEY
    config.base_url = "http://127.0.0.1:1"
    require "pp"
    [config.inspect, config.to_s, config.pretty_inspect].each { |text| refute_includes text, "synthetic" }
    assert_equal SYNTHETIC_KEY, config.api_key
    assert_raises(TypeError) { Marshal.dump(config) }
  end

  def test_a_customer_token_never_prints_its_token
    token = Vunly::CustomerToken.new(id: "tok_1", token: "vct_synthetic_val", customer_id: "cus_1",
                                     expires_at: Time.utc(2026, 10, 28))
    require "pp"
    [token.inspect, token.to_s, token.pretty_inspect, token.to_h.to_s, token.to_json, token.as_json.to_s].each do |text|
      refute_includes text, "synthetic_val", text
    end
    assert_includes token.inspect, "tok_1"
    assert_equal "vct_synthetic_val", token.token
    assert_raises(TypeError) { Marshal.dump(token) }
  end

  # ---- requests ----------------------------------------------------------------------------------------------------

  def test_a_request_carries_the_key_the_user_agent_and_json_and_no_environment_headers
    with_server(FakeServer.json(200, customer_json)) do |server|
      client, = new_client(server)
      customer = client.customers.upsert("acme", name: "Acme")
      assert_equal "acme", customer.external_id
      assert_equal Time.utc(2026, 10, 28, 10), customer.created_at
      req = server.requests.fetch(0)
      assert_equal ["POST", "/v1/customers"], [req.verb, req.path]
      assert_equal "Bearer #{SYNTHETIC_KEY}", req.headers["authorization"]
      assert_match(%r{\Avunly-ruby/#{Regexp.escape(Vunly::VERSION)} \(ruby #{Regexp.escape(RUBY_VERSION)} },
                   req.headers["user-agent"])
      assert_equal "application/json", req.headers["content-type"]
      assert_equal "identity", req.headers["accept-encoding"]
      assert_equal({ "externalId" => "acme", "name" => "Acme" }, JSON.parse(req.body))
      refute req.headers.key?("vunly-project")
      refute req.headers.key?("vunly-environment")
      refute req.headers.key?("idempotency-key")
    end
  end

  def test_the_proxy_environment_is_not_read
    with_server(FakeServer.json(200, customer_json)) do |server|
      old = ENV.to_h.slice("http_proxy", "HTTP_PROXY")
      ENV["http_proxy"] = ENV["HTTP_PROXY"] = "http://127.0.0.1:1"
      begin
        client, = new_client(server)
        assert_equal "acme", client.customers.get("acme").external_id
      ensure
        ENV.delete("http_proxy")
        ENV.delete("HTTP_PROXY")
        old.each { |k, v| ENV[k] = v }
      end
    end
  end

  def test_a_path_value_is_escaped
    with_server(FakeServer.json(200, customer_json)) do |server|
      client, = new_client(server)
      client.customers.get("a b/c?d")
      assert_equal "/v1/customers/a%20b%2Fc%3Fd", server.requests.fetch(0).path
    end
  end

  def test_a_missing_path_value_is_refused_locally
    with_server(FakeServer.json(200, customer_json)) do |server|
      client, = new_client(server)
      assert_raises(Vunly::InvalidRequestError) { client.customers.get(nil) }
      assert_raises(Vunly::InvalidRequestError) { client.credits.balance("") }
      assert_equal 0, server.hits
    end
  end

  def test_usage_record_returns_the_event_the_remaining_credits_and_the_replay_flag
    with_server(FakeServer.json(200, usage_json, "Idempotent-Replayed" => "true", "Request-Id" => "req_hdr")) do |server|
      client, = new_client(server)
      result = client.usage.record(id: "use-0001-abcd", customer: "acme", meter: "tokens", quantity: 3000)
      assert_equal 97, result.remaining
      assert result.replayed?
      refute result.plan_overage?, "a replay is never an overage"
      assert_equal(-3, result.event.credits)
      assert_equal "req_hdr", result.request_id
      req = server.requests.fetch(0)
      assert_equal "use-0001-abcd", req.headers["idempotency-key"]
      assert_equal({ "customerId" => "acme", "meter" => "tokens", "quantity" => 3000 }, JSON.parse(req.body))
    end
  end

  def test_plan_overage_is_surfaced
    with_server(FakeServer.json(200, usage_json, "Vunly-Plan-Overage" => "true")) do |server|
      client, = new_client(server)
      result = client.usage.record(id: "use-0001-abcd", customer: "acme", meter: "tokens", quantity: 1)
      assert result.plan_overage?
      refute result.replayed?
    end
  end

  def test_the_core_calls
    steps = [
      FakeServer.json(200, { credits: 50, consumed: 0, granted: 50 }),
      FakeServer.json(200, entry_json(kind: "grant", credits: 50, meter: nil, quantity: nil)),
      FakeServer.json(200, { allowed: true, reason: "ok", remaining: 50, required: 1, plan: nil }),
      FakeServer.json(200, plan_json),
      FakeServer.json(200, subscription_json),
      FakeServer.json(204, ""),
      FakeServer.json(200, token_json),
      FakeServer.json(204, ""),
      FakeServer.json(200, plan_usage_json),
      FakeServer.json(204, "")
    ]
    with_server(*steps) do |server|
      client, = new_client(server)
      balance = client.credits.balance("acme")
      assert_equal [50, 0, 50], [balance.remaining, balance.consumed, balance.granted]
      grant = client.credits.grant(id: "grant-0001-abcd", customer: "acme", credits: 50, reason: "top-up")
      assert_equal ["grant", 50, nil], [grant.kind, grant.credits, grant.meter]
      decision = client.entitlements.check(customer: "acme", feature: "generate",
                                           estimate: { meter: "tokens", quantity: 1050 })
      assert decision.allowed?
      assert_nil decision.plan
      plan = client.plans.upsert_plan(key: "starter", name: "Starter", included_credits: 100, meter: "tokens",
                                      features: ["generate"])
      assert_equal "starter", plan.key
      sub = client.plans.subscribe("acme", "starter")
      assert_equal %w[active starter], [sub.status, sub.plan.key]
      assert_nil client.plans.upsert_meter("tokens", 1000)
      token = client.customer_tokens.create("acme", ttl: 900)
      assert_equal "vct_synthetic_x", token.token
      assert_nil client.customer_tokens.revoke("tok_1")
      usage = client.plan_usage
      assert_equal false, usage.unconfirmed_limits.confirmed
      assert_equal 4, usage.usage.events
      assert_nil usage.unconfirmed_limits.activity_retention_days
      assert_nil client.plans.cancel("acme")

      paths = server.requests.map { |r| "#{r.verb} #{r.path}" }
      assert_equal ["GET /v1/customers/acme/balance", "POST /v1/customers/acme/credit-grants",
                    "POST /v1/entitlements/check", "POST /v1/plans", "POST /v1/customers/acme/subscription",
                    "POST /v1/meters", "POST /v1/customer-tokens", "DELETE /v1/customer-tokens/tok_1",
                    "GET /v1/plan-usage", "DELETE /v1/customers/acme/subscription"], paths
      assert_equal({ "customerId" => "acme", "feature" => "generate",
                     "estimate" => { "meter" => "tokens", "quantity" => 1050 } }, JSON.parse(server.requests[2].body))
      assert_equal 900, JSON.parse(server.requests[6].body)["ttlSeconds"]
    end
  end

  # ---- errors ------------------------------------------------------------------------------------------------------

  def test_every_status_is_classified
    {
      400 => Vunly::InvalidRequestError, 404 => Vunly::InvalidRequestError, 409 => Vunly::InvalidRequestError,
      401 => Vunly::AuthError, 403 => Vunly::AuthError, 402 => Vunly::LimitReachedError,
      429 => Vunly::UnavailableError, 500 => Vunly::UnavailableError, 503 => Vunly::UnavailableError
    }.each do |status, klass|
      with_server(FakeServer.error(status, "some_code", "msg")) do |server|
        client, = new_client(server, max_retries: 0)
        error = assert_raises(klass, "status #{status}") { client.credits.balance("acme") }
        assert_equal [status, "some_code", "req_test"], [error.status, error.code, error.request_id]
        assert_nil error.cause
      end
    end
  end

  def test_a_limit_reached_error_carries_balance_and_required
    with_server(FakeServer.error(402, "insufficient_credits", "no", { "balance" => 2, "required" => 5 })) do |server|
      client, = new_client(server)
      error = assert_raises(Vunly::LimitReachedError) do
        client.usage.record(id: "use-0001-abcd", customer: "acme", meter: "tokens", quantity: 9000)
      end
      assert_equal [2, 5], [error.balance, error.required]
      refute error.retryable?
      assert_equal 1, server.hits, "a 402 is a final answer"
    end
  end

  def test_an_error_without_a_json_body_is_classified_by_its_status
    {
      502 => ["internal_error", Vunly::UnavailableError], 401 => ["unauthenticated", Vunly::AuthError],
      400 => ["validation_error", Vunly::InvalidRequestError]
    }.each do |status, (code, klass)|
      with_server(FakeServer.json(status, "<html>proxy</html>", "Content-Type" => "text/html")) do |server|
        client, = new_client(server, max_retries: 0)
        error = assert_raises(klass) { client.customers.get("acme") }
        assert_equal [status, code], [error.status, error.code]
      end
    end
  end

  def test_an_unknown_error_code_is_handled_as_the_generic_error_of_its_status
    with_server(FakeServer.error(409, "a_code_added_in_2027")) do |server|
      client, = new_client(server)
      error = assert_raises(Vunly::InvalidRequestError) { client.customers.get("acme") }
      assert_equal "a_code_added_in_2027", error.code
    end
  end

  def test_a_wrong_address_gets_a_hint
    with_server(FakeServer.error(404, "not_found", "No such route: GET /v1/v1/customers")) do |server|
      client, = new_client(server)
      error = assert_raises(Vunly::InvalidRequestError) { client.customers.get("acme") }
      assert_includes error.message, "check the API address"
    end
  end

  def test_the_error_message_has_status_code_and_request_id_but_no_key
    with_server(FakeServer.error(403, "forbidden", "missing scope")) do |server|
      client, = new_client(server)
      error = assert_raises(Vunly::AuthError) { client.customers.get("acme") }
      assert_equal "vunly: missing scope (403 forbidden) request req_test", error.message
      refute_includes error.full_message, SYNTHETIC_KEY
    end
  end

  # ---- idempotency ids ---------------------------------------------------------------------------------------------

  def test_a_bad_idempotency_id_is_refused_before_any_request
    with_server(FakeServer.json(200, usage_json)) do |server|
      client, = new_client(server)
      ["short", "x" * 129, "has space in it", "ünïcode-id-0001", "ok-id-1234\n", "ok-id-1234\nX-Evil: 1", "", nil,
       12_345_678].each do |id|
        error = assert_raises(Vunly::InvalidRequestError, id.inspect) do
          client.usage.record(id: id, customer: "acme", meter: "tokens", quantity: 1)
        end
        assert_equal [400, "validation_error"], [error.status, error.code]
        assert_equal "8-128 chars of A-Za-z0-9_.:-", error.details.dig("fields", "id")
      end
      error = assert_raises(Vunly::InvalidRequestError) { client.credits.grant(id: "bad", customer: "acme", credits: 1) }
      assert_equal 400, error.status
      assert_equal 0, server.hits, "nothing was sent"
    end
  end

  def test_the_boundary_ids_are_accepted
    with_server(FakeServer.json(200, usage_json)) do |server|
      client, = new_client(server)
      ["a" * 8, "A" * 128, "req_2026.10:28-abcd"].each do |id|
        client.usage.record(id: id, customer: "acme", meter: "tokens", quantity: 1)
      end
      assert_equal 3, server.hits
    end
  end

  # ---- retries by operation class ----------------------------------------------------------------------------------

  def test_a_read_is_retried_after_a_503_and_after_a_lost_answer
    with_server(FakeServer.error(503, "unavailable"), FakeServer.drop, FakeServer.json(200, customer_json)) do |server|
      client, sleeps = new_client(server)
      assert_equal "acme", client.customers.get("acme").external_id
      assert_equal 3, server.hits
      assert_equal 2, sleeps.size
    end
  end

  def test_a_keyed_mutation_is_retried_with_the_same_id_and_body
    with_server(FakeServer.error(500, "internal_error"), FakeServer.drop, FakeServer.json(200, usage_json)) do |server|
      client, = new_client(server)
      client.usage.record(id: "use-0001-abcd", customer: "acme", meter: "tokens", quantity: 3000)
      reqs = server.requests
      assert_equal 3, reqs.size
      assert_equal ["use-0001-abcd"], reqs.map { |r| r.headers["idempotency-key"] }.uniq
      assert_equal 1, reqs.map(&:body).uniq.size
    end
  end

  def test_a_naturally_idempotent_write_is_retried
    with_server(FakeServer.error(502, "bad_gateway"), FakeServer.json(200, customer_json)) do |server|
      client, = new_client(server)
      client.customers.upsert("acme")
      assert_equal 2, server.hits
    end
  end

  def test_an_unkeyed_non_idempotent_post_is_not_repeated_after_an_ambiguous_failure
    [FakeServer.drop, FakeServer.error(500, "internal_error"), FakeServer.error(503, "unavailable")].each do |first|
      with_server(first,
                  FakeServer.json(200,
                                  { id: "t", token: "x", customerId: "c", expiresAt: "2026-10-28T10:00:00Z" })) do |server|
        client, sleeps = new_client(server)
        assert_raises(Vunly::UnavailableError) { client.customer_tokens.create("acme") }
        assert_equal 1, server.hits, "a second token would be a different outcome"
        assert_empty sleeps
      end
    end
  end

  def test_an_unkeyed_non_idempotent_post_is_repeated_after_a_429
    ok = FakeServer.json(200, { id: "t", token: "vct_synthetic_x", customerId: "c", expiresAt: "2026-10-28T10:00:00Z" })
    with_server(FakeServer.error(429, "rate_limited", "slow", nil, "Retry-After" => "2"), ok) do |server|
      client, sleeps = new_client(server)
      assert_equal "t", client.customer_tokens.create("acme").id
      assert_equal 2, server.hits
      assert_equal [2.0], sleeps
    end
  end

  def test_the_plan_volume_stop_is_never_repeated
    stop = FakeServer.error(429, "rate_limited", "plan limit", { "reason" => "plan_included_volume" }, "Retry-After" => "5")
    with_server(stop, FakeServer.json(200, usage_json)) do |server|
      client, sleeps = new_client(server)
      error = assert_raises(Vunly::UnavailableError) do
        client.usage.record(id: "use-0001-abcd", customer: "acme", meter: "tokens", quantity: 1)
      end
      assert error.plan_stop?
      refute error.retryable?
      assert_equal 429, error.status
      assert_equal 1, server.hits
      assert_empty sleeps
    end
  end

  def test_final_answers_are_never_retried
    [400, 401, 403, 404, 409, 402, 501].each do |status|
      with_server(FakeServer.error(status, "x"), FakeServer.json(200, customer_json)) do |server|
        client, = new_client(server)
        assert_raises(Vunly::Error) { client.customers.get("acme") }
        assert_equal 1, server.hits, "status #{status}"
      end
    end
  end

  def test_retries_stop_at_the_configured_maximum
    with_server(FakeServer.error(503, "unavailable")) do |server|
      client, = new_client(server)
      assert_raises(Vunly::UnavailableError) { client.customers.get("acme") }
      assert_equal 3, server.hits, "1 attempt + 2 retries by default"
    end
    with_server(FakeServer.error(503, "unavailable")) do |server|
      client, = new_client(server, max_retries: 0)
      assert_raises(Vunly::UnavailableError) { client.customers.get("acme") }
      assert_equal 1, server.hits
    end
    with_server(FakeServer.error(503, "unavailable")) do |server|
      client, = new_client(server, max_retries: 4)
      assert_raises(Vunly::UnavailableError) { client.customers.get("acme") }
      assert_equal 5, server.hits
    end
  end

  def test_retry_after_is_honoured_up_to_30_seconds_then_raised
    with_server(FakeServer.error(429, "rate_limited", "slow", nil, "Retry-After" => "7"),
                FakeServer.json(200, customer_json)) do |server|
      client, sleeps = new_client(server)
      client.customers.get("acme")
      assert_equal [7.0], sleeps
    end
    with_server(FakeServer.error(429, "rate_limited", "slow", nil, "Retry-After" => "30"),
                FakeServer.json(200, customer_json)) do |server|
      client, sleeps = new_client(server)
      client.customers.get("acme")
      assert_equal [30.0], sleeps
    end
    with_server(FakeServer.error(429, "rate_limited", "slow", nil, "Retry-After" => "3600")) do |server|
      client, sleeps = new_client(server)
      error = assert_raises(Vunly::UnavailableError) { client.customers.get("acme") }
      assert_equal 3600.0, error.retry_after_seconds
      assert_equal 1, server.hits
      assert_empty sleeps, "an absurd Retry-After is not waited for"
    end
  end

  def test_backoff_is_jittered_and_capped
    with_server(FakeServer.error(503, "unavailable")) do |server|
      client, sleeps = new_client(server, max_retries: 6)
      assert_raises(Vunly::UnavailableError) { client.customers.get("acme") }
      bases = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0]
      assert_equal 6, sleeps.size
      sleeps.zip(bases).each_with_index do |(wait, base), i|
        assert_operator wait, :>=, base * 0.5, "retry #{i + 1}"
        assert_operator wait, :<=, base, "retry #{i + 1}"
      end
    end
  end

  def test_retry_after_zero_falls_back_to_the_backoff
    with_server(FakeServer.error(503, "unavailable", "x", nil, "Retry-After" => "0"),
                FakeServer.json(200, customer_json)) do |server|
      client, sleeps = new_client(server)
      client.customers.get("acme")
      assert_operator sleeps.first, :<=, 0.25
    end
  end

  def test_a_malformed_retry_after_is_ignored
    ["soon", "Wed, 21 Oct 2026 07:28:00 GMT", "-5", "NaN", "0x1p3", "5s"].each do |value|
      with_server(FakeServer.error(503, "unavailable", "x", nil, "Retry-After" => value),
                  FakeServer.json(200, customer_json)) do |server|
        client, sleeps = new_client(server)
        client.customers.get("acme")
        assert_operator sleeps.first, :<=, 0.25, value
      end
    end
  end

  def test_checking_an_entitlement_fails_closed_when_the_check_cannot_be_answered
    with_server(FakeServer.error(503, "unavailable")) do |server|
      client, = new_client(server)
      assert_raises(Vunly::UnavailableError) { client.entitlements.check(customer: "acme", feature: "generate") }
      assert_equal 3, server.hits, "the check is a read: retried, then it raises, never allowed: true"
    end
  end

  # ---- malformed answers -------------------------------------------------------------------------------------------

  def test_a_2xx_that_lacks_a_promised_field_is_unavailable_invalid_response
    bad_bodies = [
      "", "not json", "[]", "\"string\"", "{}", '{"credits": 1}', '{"credits": 1.5, "consumed": 0, "granted": 1}',
      '{"credits": "1", "consumed": 0, "granted": 1}', '{"credits": null, "consumed": 0, "granted": 1}',
      '{"credits": 99999999999999999999, "consumed": 0, "granted": 1}', "\xff\xfe"
    ]
    bad_bodies.each do |body|
      with_server(FakeServer.json(200, body)) do |server|
        client, = new_client(server)
        error = assert_raises(Vunly::UnavailableError, body.inspect) { client.credits.balance("acme") }
        assert_equal ["invalid_response", 200], [error.code, error.status], body.inspect
        assert_equal 1, server.hits, "invalid_response is not retried automatically"
      end
    end
  end

  def test_a_malformed_timestamp_is_invalid_response_not_a_bare_exception
    with_server(FakeServer.json(200, customer_json(createdAt: "yesterday-ish"))) do |server|
      client, = new_client(server)
      error = assert_raises(Vunly::UnavailableError) { client.customers.get("acme") }
      assert_equal "invalid_response", error.code
    end
  end

  def test_unknown_fields_and_unknown_enum_values_are_ignored
    body = entry_json(kind: "adjustment_added_in_2027", surprise: { nested: [1, 2, 3] })
    with_server(FakeServer.json(200, { data: [body], nextCursor: nil, extra: true })) do |server|
      client, = new_client(server)
      page = client.credits.ledger("acme")
      assert_equal "adjustment_added_in_2027", page.data.first.kind
      refute_predicate page, :more?
    end
  end

  # ---- pagination --------------------------------------------------------------------------------------------------

  def test_iteration_fetches_one_page_per_step_and_only_when_consumed
    pages = [
      FakeServer.json(200, { data: [customer_json(id: "c1"), customer_json(id: "c2")], nextCursor: "cur_1" }),
      FakeServer.json(200, { data: [customer_json(id: "c3"), customer_json(id: "c4")], nextCursor: "cur_2" }),
      FakeServer.json(200, { data: [customer_json(id: "c5")], nextCursor: nil })
    ]
    with_server(*pages) do |server|
      client, = new_client(server)
      assert_equal %w[c1 c2 c3], client.customers.each(limit: 2).first(3).map(&:id)
      assert_equal 2, server.hits, "three items need two pages, not three"
      assert_equal "/v1/customers?limit=2", server.requests[0].path
      assert_equal "/v1/customers?limit=2&cursor=cur_1", server.requests[1].path
    end
    with_server(*pages) do |server|
      client, = new_client(server)
      enum = client.customers.each(limit: 2)
      assert_equal 0, server.hits, "nothing is fetched until the caller consumes"
      assert_equal %w[c1 c2 c3 c4 c5], enum.map(&:id)
      assert_equal 3, server.hits
    end
  end

  def test_an_empty_cursor_means_the_last_page
    with_server(FakeServer.json(200, { data: [], nextCursor: "" })) do |server|
      client, = new_client(server)
      refute_predicate client.customers.list, :more?
      assert_empty client.customers.each.to_a
    end
  end

  def test_the_ledger_iterator_follows_cursors
    pages = [FakeServer.json(200, { data: [entry_json(id: "l1")], nextCursor: "n" }),
             FakeServer.json(200, { data: [entry_json(id: "l2")], nextCursor: nil })]
    with_server(*pages) do |server|
      client, = new_client(server)
      assert_equal %w[l1 l2], client.credits.each_ledger_entry("acme", limit: 1).map(&:id)
      assert_equal "/v1/customers/acme/ledger?limit=1&cursor=n", server.requests[1].path
    end
  end

  def test_an_error_while_paging_surfaces_on_the_step_that_hit_it
    pages = [FakeServer.json(200, { data: [customer_json], nextCursor: "n" }), FakeServer.error(400, "validation_error")]
    with_server(*pages) do |server|
      client, = new_client(server)
      seen = []
      assert_raises(Vunly::InvalidRequestError) { client.customers.each { |c| seen << c.id } }
      assert_equal ["cus_1"], seen
    end
  end

  # ---- the shared client and the thread safety ---------------------------------------------------------------------

  def test_one_client_is_safe_to_share_between_threads
    with_server(FakeServer.json(200, customer_json)) do |server|
      client, = new_client(server)
      results = Array.new(8) { Thread.new { Array.new(5) { client.customers.get("acme").external_id } } }.flat_map(&:value)
      assert_equal ["acme"] * 40, results
      assert_equal 40, server.hits
    end
  end

  def test_vunly_client_is_built_from_the_configuration_and_rebuilt_on_configure
    Vunly.reset!
    assert_raises(Vunly::ConfigurationError) { Vunly.client }
    Vunly.configure do |c|
      c.api_key = SYNTHETIC_KEY
      c.base_url = "http://127.0.0.1:9/v1"
      c.max_retries = 0
    end
    first = Vunly.client
    assert_same first, Vunly.client
    assert_equal "http://127.0.0.1:9", first.base_url
    Vunly.configure { |c| c.base_url = "http://127.0.0.1:10" }
    refute_same first, Vunly.client
  ensure
    Vunly.reset!
  end

  def test_the_watchdog_threads_do_not_leak
    with_server(FakeServer.json(200, customer_json)) do |server|
      client, = new_client(server)
      before = Thread.list.size
      20.times { client.customers.get("acme") }
      assert_operator Thread.list.size, :<=, before + 1
    end
  end
end
