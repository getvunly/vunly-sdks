# frozen_string_literal: true

require_relative "test_helper"
require "yaml"
require "psych"
require "pp"

# A server (or a caller) that tries to exhaust memory, escape a path, or get a secret written out.
class HostileTest < Minitest::Test
  include TestSupport

  FLOOD_LIMIT = 64 * 1024 * 1024 # what a handler is willing to send before it gives up
  BLOCK = ("A" * 65_535) << "\n"

  def peak_rss_kb
    File.read("/proc/self/status")[/VmHWM:\s+(\d+)/, 1].to_i
  end

  # Each shape floods one part of the response that Net::HTTP buffers without a limit.
  SHAPES = {
    "an endless status line" => ->(io, _i) { io.write("HTTP/1.1 200 ") },
    "an endless header line" => ->(io, _i) { io.write("HTTP/1.1 200 OK\r\nX-Endless: ") },
    "an endless chunk-size line" => ->(io, _i) { io.write("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1;") },
    "endless distinct headers" => ->(io, i) { io.write("HTTP/1.1 200 OK\r\n") if i.zero? }
  }.freeze

  def flood(shape, state)
    lambda do |io, _req|
      SHAPES.fetch(shape).call(io, 0)
      i = 0
      while state[:sent] < FLOOD_LIMIT
        io.write(shape == "endless distinct headers" ? "X-H#{i}: #{'v' * 1000}\r\n" : "A" * 65_535)
        state[:sent] += shape == "endless distinct headers" ? 1010 : 65_535
        i += 1
      end
    end
  end

  def test_a_server_cannot_make_the_client_buffer_an_endless_line_or_endless_headers
    before = peak_rss_kb
    SHAPES.each_key do |shape|
      state = { sent: 0 }
      with_handler(flood(shape, state)) do |server|
        client, = new_client(server, max_retries: 0, timeout: 30)
        error = assert_raises(Vunly::UnavailableError, shape) { client.customers.get("acme") }
        assert_equal 0, error.status, shape
        sleep 0.2
        assert_operator state[:sent], :<, 16 * 1024 * 1024,
                        "#{shape}: the client must stop reading near its limit, the server got to #{state[:sent]} bytes"
      end
    end
    assert_operator peak_rss_kb - before, :<, 96 * 1024, "peak memory grew by #{peak_rss_kb - before} kB"
  end

  def test_a_normal_response_with_ordinary_headers_and_chunks_still_works
    handler = lambda do |io, _req|
      body = JSON.generate(customer_json)
      head = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n"
      many = (1..30).map { |i| "X-Extra-#{i}: #{'v' * 200}\r\n" }.join
      io.write("#{head}#{many}\r\n#{body.bytesize.to_s(16)}\r\n#{body}\r\n0\r\n\r\n")
    end
    with_handler(handler) do |server|
      client, = new_client(server)
      assert_equal "acme", client.customers.get("acme").external_id
    end
  end

  # ---- path segments ----------------------------------------------------------------------------------------------

  def test_dot_dot_dot_and_empty_ids_are_refused_locally_as_invalid_request_with_fields_id
    with_server(FakeServer.json(200, customer_json)) do |server|
      client, = new_client(server)
      ["..", ".", "", nil, 5, ["a"], "x" * 513].each do |bad|
        calls = [-> { client.credits.balance(bad) }, lambda {
          client.credits.grant(id: "grant-0001-abcd", customer: bad, credits: 1)
        },
                 -> { client.customers.get(bad) }, -> { client.customer_tokens.revoke(bad) },
                 -> { client.plans.subscribe(bad, "p") }, -> { client.credits.ledger(bad) }]
        calls.each do |call|
          error = assert_raises(Vunly::InvalidRequestError, bad.inspect) { call.call }
          assert error.details.dig("fields", "id"), bad.inspect
          assert_equal [400, "validation_error"], [error.status, error.code]
        end
      end
      assert_equal 0, server.hits
    end
  end

  def test_an_id_with_dots_inside_or_slashes_is_encoded_not_navigated
    with_server(FakeServer.json(200, customer_json)) do |server|
      client, = new_client(server)
      ["a.b", "...", "..a", "a/../b", "%2e%2e"].each { |id| client.customers.get(id) }
      assert_equal ["/v1/customers/a.b", "/v1/customers/...", "/v1/customers/..a", "/v1/customers/a%2F..%2Fb",
                    "/v1/customers/%252e%252e"], server.requests.map(&:path)
    end
  end

  # ---- nothing serialises a secret ---------------------------------------------------------------------------------

  def secret_holders
    key = "vk_test_synthetic_yaml_0000000001"
    client = Vunly::Client.new(api_key: key, base_url: "http://127.0.0.1:1")
    config = Vunly::Configuration.new.tap { |c| c.api_key = key }
    token = Vunly::CustomerToken.new(id: "t", token: "vct_synthetic_yaml", customer_id: "c", expires_at: Time.now)
    transport = client.instance_variable_get(:@transport)
    {
      "client" => client, "configuration" => config, "customer token" => token, "transport" => transport,
      "secret" => transport.instance_variable_get(:@api_key), "a resource" => client.usage,
      "customers" => client.customers, "credits" => client.credits, "plans" => client.plans,
      "entitlements" => client.entitlements, "customer_tokens" => client.customer_tokens,
      "an array holding the client" => [client], "a hash holding the token" => { token: token }
    }
  end

  def test_yaml_and_marshal_never_write_a_key_or_a_token
    secret_holders.each do |name, object|
      [-> { YAML.dump(object) }, -> { object.to_yaml }, -> { Psych.dump(object) }, -> { Psych.safe_dump(object) },
       -> { Marshal.dump(object) }].each do |dump|
        out = nil
        begin
          out = dump.call
        rescue TypeError, Psych::Exception
          next # refused: good
        end
        refute_match(/synthetic_yaml/, out.to_s, "#{name} was written out")
      end
    end
  end

  def test_every_dump_of_a_secret_holder_refuses_rather_than_hides
    secret_holders.except("an array holding the client", "a hash holding the token").each do |name, object|
      assert_raises(TypeError, name) { YAML.dump(object) }
      assert_raises(TypeError, name) { Marshal.dump(object) }
    end
  end

  def test_the_texts_a_program_may_print_hold_neither_key_nor_token
    secret_holders.each do |name, object|
      texts = [object.inspect, object.to_s, object.pretty_inspect]
      texts << object.to_json if object.respond_to?(:to_json)
      texts.each { |text| refute_match(/synthetic_yaml/, text, "#{name} printed a secret") }
    end
  end

  # ---- parsing -----------------------------------------------------------------------------------------------------

  def test_a_huge_integer_is_refused_before_it_is_parsed
    body = "{\"credits\": #{'9' * 1_000_000}, \"consumed\": 0, \"granted\": 0}"
    with_server(FakeServer.json(200, body)) do |server|
      client, = new_client(server)
      started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
      error = assert_raises(Vunly::UnavailableError) { client.credits.balance("acme") }
      assert_equal "invalid_response", error.code
      assert_operator Process.clock_gettime(Process::CLOCK_MONOTONIC) - started, :<, 5
    end
  end

  # ---- webhooks ----------------------------------------------------------------------------------------------------

  def test_malformed_webhook_headers_are_webhook_errors_never_other_exceptions
    payload = '{"id":"e","type":"t"}'
    headers = [123, :sym, ["t=1"], "\xff\xfe=1,t=\xc3", "t=1,v1=\xff".b, "t=#{'9' * 5000}", "é=ü", "t=1\u0000",
               "v1=#{'ab' * 3000}", "t=#{Time.now.to_i},v1=zz", "\x00", "t=,v1=", ",,,"]
    headers.each do |header|
      error = assert_raises(Vunly::WebhookError, header.inspect[0, 40]) { Vunly.verify_webhook(payload, header, ["s3cret-synthetic"]) }
      assert_includes %w[missing_header malformed_header stale_timestamp no_matching_signature], error.reason
    end
  end

  def test_a_header_with_a_million_signatures_is_cheap_and_only_the_first_few_count
    ts = Time.now.to_i
    payload = '{"id":"e","type":"t"}'
    good = OpenSSL::HMAC.hexdigest("SHA256", "s3cret-synthetic", "#{ts}.#{payload}")
    many = (["v1=#{'0' * 64}"] * 1_000_000).join(",")
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    assert_raises(Vunly::WebhookError) { Vunly.verify_webhook(payload, "t=#{ts},#{many}", ["s3cret-synthetic"]) }
    assert_operator Process.clock_gettime(Process::CLOCK_MONOTONIC) - started, :<, 2
    assert Vunly.verify_webhook(payload, "t=#{ts},v1=#{good}", ["s3cret-synthetic"])
    # a valid signature after more than ten others is not considered
    late = ((["v1=#{'0' * 64}"] * 10) + ["v1=#{good}"]).join(",")
    assert_raises(Vunly::WebhookError) { Vunly.verify_webhook(payload, "t=#{ts},#{late}", ["s3cret-synthetic"]) }
  end

  def test_secrets_without_a_printable_character_are_refused
    header = "t=#{Time.now.to_i},v1=#{'0' * 64}"
    [" ", "　　", "  \t", "​", ["real-synthetic", " "], ["x"] * 11].each do |secrets|
      assert_raises(ArgumentError, secrets.inspect) { Vunly.verify_webhook("{}", header, secrets) }
    end
  end

  # ---- URLs, details -----------------------------------------------------------------------------------------------

  def test_empty_userinfo_and_impossible_ports_are_refused
    ["http://@host", "https://@host:443", "http://:@host", "http://host:0", "http://host:99999",
     "http://host:65536"].each do |url|
      assert_raises(Vunly::ConfigurationError, url) { Vunly::Client.new(api_key: SYNTHETIC_KEY, base_url: url) }
    end
    assert_equal "http://host:65535", Vunly::Client.new(api_key: SYNTHETIC_KEY, base_url: "http://host:65535").base_url
  end

  def test_error_details_are_bounded_and_cleaned
    nasty = { "reason" => "ok‮", "deep" => { "a" => { "b" => { "c" => { "d" => { "e" => { "f" => 1 } } } } } },
              "list" => Array.new(500) { |i| "x#{i}" }, "s" => "z" * 5000 }
    (1..100).each { |i| nasty["k#{i}"] = i }
    with_server(FakeServer.error(409, "conflict", "m", nasty)) do |server|
      client, = new_client(server)
      error = assert_raises(Vunly::InvalidRequestError) { client.customers.get("acme") }
      details = error.details
      assert_operator details.size, :<=, 50
      assert_equal "ok", details["reason"]
      assert_operator details["list"].size, :<=, 50
      assert_operator details["s"].length, :<=, 500
      assert_nil details.dig("deep", "a", "b", "c", "d", "e")
    end
  end
end
