# frozen_string_literal: true

require_relative "test_helper"

# Regression tests for hardening cases (malformed input, unsafe responses, local validation), one test per case.
class HardeningTest < Minitest::Test
  include TestSupport

  # ---- the key ----------------------------------------------------------------------------------------------------

  def test_only_a_trailing_lf_or_crlf_is_dropped_from_the_key_and_never_sent
    with_server(FakeServer.json(200, customer_json)) do |server|
      ["#{SYNTHETIC_KEY}\n", "#{SYNTHETIC_KEY}\r\n"].each do |key|
        client = Vunly::Client.new(api_key: key, base_url: server.url)
        client.customers.get("acme")
      end
      assert_equal ["Bearer #{SYNTHETIC_KEY}"], server.requests.map { |r| r.headers["authorization"] }.uniq
    end
  end

  def test_spaces_tabs_nul_and_a_lone_cr_at_either_end_of_the_key_are_refused
    [" #{SYNTHETIC_KEY}", "#{SYNTHETIC_KEY} ", "\t#{SYNTHETIC_KEY}", "#{SYNTHETIC_KEY}\t", "\0#{SYNTHETIC_KEY}",
     "#{SYNTHETIC_KEY}\0", "#{SYNTHETIC_KEY}\r", "\r#{SYNTHETIC_KEY}", "\n#{SYNTHETIC_KEY}", "#{SYNTHETIC_KEY}\n\n",
     "#{SYNTHETIC_KEY} \n"].each do |key|
      error = assert_raises(Vunly::ConfigurationError, key.inspect) do
        Vunly::Client.new(api_key: key, base_url: "http://127.0.0.1:1")
      end
      refute_includes error.message, "synthetic"
    end
  end

  def test_a_key_with_control_characters_or_non_ascii_is_refused_without_repeating_it
    ["vk_test_synthetic_a\r\nX-Injected: 1", "vk_test_synthetic\u0000a", "vk_test_synthetic_é", "vk_test synthetic",
     "vk_test_synthetic‮a", "vk_test_synthetic_\xff".dup.force_encoding("UTF-8")].each do |key|
      error = assert_raises(Vunly::ConfigurationError) { Vunly::Client.new(api_key: key, base_url: "http://127.0.0.1:1") }
      assert_includes error.message, "api_key"
      refute_includes error.message, "synthetic"
      assert_nil error.cause
    end
    assert_raises(Vunly::ConfigurationError) { Vunly::Client.new(api_key: 12_345, base_url: "http://127.0.0.1:1") }
  end

  def test_a_crlf_in_the_key_cannot_inject_a_header
    with_server(FakeServer.json(200, customer_json)) do |server|
      assert_raises(Vunly::ConfigurationError) do
        Vunly::Client.new(api_key: "vk_test_synthetic_x\r\nX-Injected: yes", base_url: server.url)
      end
      assert_equal 0, server.hits
    end
  end

  # ---- timeouts ---------------------------------------------------------------------------------------------------

  def test_timeout_values_that_are_not_sane_are_refused
    [0, -1, Float::NAN, Float::INFINITY, -Float::INFINITY, 1e-9, 0.0001, 3601, 1e9, "5", nil, :five].each do |value|
      assert_raises(Vunly::ConfigurationError, value.inspect) do
        Vunly::Client.new(api_key: SYNTHETIC_KEY, base_url: "http://h", timeout: value)
      end
    end
    [0.001, 0.5, 10, 3600, Rational(1, 2)].each do |value|
      Vunly::Client.new(api_key: SYNTHETIC_KEY, base_url: "http://h", timeout: value)
    end
  end

  # ---- the base url -----------------------------------------------------------------------------------------------

  def test_the_base_url_is_never_printed_with_credentials_because_it_is_never_accepted_with_them
    error = assert_raises(Vunly::ConfigurationError) do
      Vunly::Client.new(api_key: SYNTHETIC_KEY, base_url: "https://user:pw-synthetic@host/v1")
    end
    refute_includes error.message, "pw-synthetic"
    %w[https://host#frag https://host?token=1 http:// https:///x ftp://h].each do |url|
      assert_raises(Vunly::ConfigurationError, url) { Vunly::Client.new(api_key: SYNTHETIC_KEY, base_url: url) }
    end
  end

  # ---- what a server says in an error ----------------------------------------------------------------------------

  def test_server_text_is_sanitised_and_bounded_before_it_reaches_a_message
    nasty = "bad\u0000\e[31m‮evil​\u0085\u007f\r\nINJECTED line#{'x' * 2000}"
    with_server(FakeServer.json(409, { error: { code: "#{'c' * 200}‮", message: nasty, requestId: "r\n#{'q' * 500}" } },
                                "Request-Id" => "h\u0000#{'z' * 500}")) do |server|
      client, = new_client(server)
      error = assert_raises(Vunly::InvalidRequestError) { client.customers.get("acme") }
      assert_operator error.api_message.length, :<=, 500
      assert_operator error.code.length, :<=, 64
      assert_operator error.request_id.length, :<=, 128
      [error.message, error.api_message, error.code, error.request_id].each do |text|
        refute_match(/[\p{Cc}\p{Cf}]/, text)
      end
      refute_includes error.message, "\n"
    end
  end

  def test_the_request_id_header_is_sanitised_too
    with_server(FakeServer.json(200, usage_json, "Request-Id" => "ok​-id‮#{'k' * 300}")) do |server|
      client, = new_client(server)
      result = client.usage.record(id: "use-0001-abcd", customer: "acme", meter: "tokens", quantity: 1)
      assert_operator result.request_id.length, :<=, 128
      refute_match(/[\p{Cc}\p{Cf}]/, result.request_id)
    end
  end

  # ---- redirects --------------------------------------------------------------------------------------------------

  def test_a_redirect_is_unavailable_unexpected_redirect_and_is_never_retried
    [301, 302, 303, 307, 308].each do |status|
      head = "HTTP/1.1 #{status} R\r\nLocation: http://127.0.0.1:1/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
      with_handler(->(io, _req) { io.write(head) }) do |server|
        client, sleeps = new_client(server)
        error = assert_raises(Vunly::UnavailableError, status.to_s) { client.customers.get("acme") }
        assert_equal ["unexpected_redirect", status], [error.code, error.status]
        refute error.retryable?
        refute_equal "validation_error", error.code
        assert_equal 1, server.hits
        assert_empty sleeps
      end
    end
  end

  # ---- pagination -------------------------------------------------------------------------------------------------

  def test_a_repeated_or_non_advancing_cursor_fails_instead_of_looping
    same = FakeServer.json(200, { data: [customer_json], nextCursor: "same" })
    with_server(same) do |server|
      client, = new_client(server)
      error = assert_raises(Vunly::UnavailableError) { client.customers.each(limit: 1).take(50) }
      assert_equal "invalid_response", error.code
      assert_operator server.hits, :<=, 3
    end
    cycle = [FakeServer.json(200, { data: [customer_json], nextCursor: "a" }),
             FakeServer.json(200, { data: [customer_json], nextCursor: "b" }),
             FakeServer.json(200, { data: [customer_json], nextCursor: "a" })]
    with_server(*cycle) do |server|
      client, = new_client(server)
      assert_raises(Vunly::UnavailableError) { client.credits.each_ledger_entry("acme").first(50) }
      assert_equal 3, server.hits
    end
  end

  def test_three_empty_pages_in_a_row_that_each_promise_another_fail_with_fresh_cursors_every_time
    steps = %w[a b c d e].map { |c| FakeServer.json(200, { data: [], nextCursor: c }) }
    with_server(*steps) do |server|
      client, = new_client(server)
      error = assert_raises(Vunly::UnavailableError) { client.customers.each.to_a }
      assert_equal "invalid_response", error.code
      assert_equal 3, server.hits
    end
  end

  def test_an_empty_page_between_full_ones_is_fine
    page = ->(items, cursor) { FakeServer.json(200, { data: items, nextCursor: cursor }) }
    pages = [page.call([], "a"), page.call([customer_json], "b"), page.call([], "c"), page.call([customer_json], nil)]
    with_server(*pages) do |server|
      client, = new_client(server)
      assert_equal 2, client.customers.each.to_a.size
    end
  end

  # ---- JSON bounds ------------------------------------------------------------------------------------------------

  def test_small_bodies_of_tiny_nested_values_end_in_an_sdk_error
    {
      "arrays" => "[#{'[1],' * 600_000}[1]]", # about 2.4 MB, 600k arrays
      "objects" => "[#{'{"a":1},' * 250_000}{\"a\":1}]", # about 2.2 MB
      "flat" => "[#{'1,' * 1_000_000}1]" # one array, a million elements
    }.each do |shape, body|
      assert_operator body.bytesize, :<, 3 * 1024 * 1024, shape
      with_server(FakeServer.json(200, body)) do |server|
        client, = new_client(server)
        error = assert_raises(Vunly::UnavailableError, shape) { client.customers.list }
        assert_equal "invalid_response", error.code, shape
      end
    end
  end

  def test_a_body_just_under_the_cap_made_of_empty_arrays_ends_in_an_sdk_error
    body = "[#{'[],' * 5_000_000}[]]" # about 15 MB, millions of containers
    assert_operator body.bytesize, :<, 16 * 1024 * 1024
    with_server(FakeServer.json(200, body)) do |server|
      client, = new_client(server)
      error = assert_raises(Vunly::UnavailableError) { client.customers.list }
      assert_equal "invalid_response", error.code
    end
  end

  def test_deep_nesting_is_an_sdk_error_not_a_nesting_error
    body = "#{'[' * 5000}#{']' * 5000}"
    with_server(FakeServer.json(200, body), FakeServer.json(200, "{\"data\":#{body},\"nextCursor\":null}")) do |server|
      client, = new_client(server)
      assert_equal "invalid_response", assert_raises(Vunly::UnavailableError) { client.customers.list }.code
      assert_equal "invalid_response", assert_raises(Vunly::UnavailableError) { client.customers.list }.code
    end
  end

  def test_an_error_body_of_deep_nesting_still_keeps_its_class
    with_server(FakeServer.json(404, "#{'[' * 5000}#{']' * 5000}")) do |server|
      client, = new_client(server)
      error = assert_raises(Vunly::InvalidRequestError) { client.customers.get("acme") }
      assert_equal 404, error.status
    end
  end

  # ---- webhooks ---------------------------------------------------------------------------------------------------

  def test_blank_or_missing_webhook_secrets_are_refused_not_used
    header = "t=#{Time.now.to_i},v1=#{'0' * 64}"
    [[], "", "  ", [""], ["real", ""], ["real", nil], [" \n"], nil, [1]].each do |secrets|
      assert_raises(ArgumentError, secrets.inspect) { Vunly.verify_webhook("{}", header, secrets) }
    end
  end

  def test_a_signature_made_with_an_empty_key_is_not_accepted_for_a_real_secret
    payload = '{"id":"e","type":"t"}'
    ts = Time.now.to_i
    forged = OpenSSL::HMAC.hexdigest("SHA256", "", "#{ts}.#{payload}")
    error = assert_raises(Vunly::WebhookError) { Vunly.verify_webhook(payload, "t=#{ts},v1=#{forged}", ["s3cret-synthetic"]) }
    assert_equal "no_matching_signature", error.reason
  end

  # ---- idempotency ------------------------------------------------------------------------------------------------

  def test_a_nil_id_on_a_keyed_mutation_is_refused_locally_naming_the_field
    with_server(FakeServer.json(200, usage_json)) do |server|
      client, = new_client(server)
      error = assert_raises(Vunly::InvalidRequestError) do
        client.usage.record(id: nil, customer: "acme", meter: "tokens", quantity: 1)
      end
      assert_equal 400, error.status
      assert error.details.dig("fields", "id")
      assert_equal 0, server.hits
    end
  end
end
