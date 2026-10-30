# frozen_string_literal: true

require_relative "test_helper"
require "openssl"

# The HTTP layer under hostile servers: a drip, a flood, a dropped connection, a redirect.
class TransportTest < Minitest::Test
  include TestSupport

  HEAD_200 = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n"

  def elapsed
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    yield
    Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
  end

  # ---- one attempt is bounded as a whole ---------------------------------------------------------------------------

  def test_a_server_that_answers_at_once_and_drips_its_body_is_cut_at_the_timeout
    drip = lambda do |io, _req|
      io.write("#{HEAD_200}Content-Length: 100000\r\n\r\n")
      1200.times do
        io.write("x")
        sleep 0.05 # always inside any per-read timeout: only a whole-attempt deadline stops this
      end
    end
    with_handler(drip) do |server|
      client, = new_client(server, timeout: 0.4, max_retries: 0)
      error = nil
      seconds = elapsed { error = assert_raises(Vunly::UnavailableError) { client.customers.get("acme") } }
      assert_operator seconds, :<, 20, "the attempt must end near its 0.4 s timeout, not when the 60 s drip ends"
      assert_equal ["unavailable", 0], [error.code, error.status]
      assert_includes error.message, "did not answer within 0.4 s"
    end
  end

  def test_a_server_that_drips_its_headers_is_cut_at_the_timeout
    drip = lambda do |io, _req|
      io.write("HTTP/1.1 200 OK\r\n")
      600.times do
        io.write("X")
        sleep 0.1
      end
    end
    with_handler(drip) do |server|
      client, = new_client(server, timeout: 0.6, max_retries: 0)
      seconds = elapsed { assert_raises(Vunly::UnavailableError) { client.customers.get("acme") } }
      assert_operator seconds, :<, 20 # the drip would last 60 s
    end
  end

  def test_a_server_that_never_answers_is_cut_at_the_timeout
    with_handler(->(_io, _req) { sleep 60 }) do |server|
      client, = new_client(server, timeout: 0.4, max_retries: 0)
      seconds = elapsed { assert_raises(Vunly::UnavailableError) { client.customers.get("acme") } }
      assert_operator seconds, :>=, 0.35
      assert_operator seconds, :<, 20
    end
  end

  def test_each_attempt_has_its_own_deadline_and_a_keyed_retry_reuses_the_id
    slow = ->(_io, _req) { sleep 60 }
    fast = FakeServer.json(200, usage_json, "Idempotent-Replayed" => "true")
    with_handler(->(io, req) { (req.n == 1 ? slow : fast).call(io, req) }) do |server|
      client, = new_client(server, timeout: 1.5)
      result = client.usage.record(id: "slow-0001-abcd", customer: "acme", meter: "tokens", quantity: 3000)
      assert result.replayed?
      assert_equal(%w[slow-0001-abcd slow-0001-abcd], server.requests.map { |r| r.headers["idempotency-key"] })
    end
  end

  # ---- the body cap -----------------------------------------------------------------------------------------------

  def flood(io, head, total:, chunked: false)
    io.write(head)
    block = "y" * 65_536
    sent = 0
    while sent < total
      io.write(chunked ? "10000\r\n#{block}\r\n" : block)
      sent += block.bytesize
      @sent = sent
    end
  end

  def test_a_declared_body_over_the_cap_is_refused_without_reading_it
    @sent = 0
    handler = ->(io, _req) { flood(io, "#{HEAD_200}Content-Length: #{17 * 1024 * 1024}\r\n\r\n", total: 64 * 1024 * 1024) }
    with_handler(handler) do |server|
      client, = new_client(server)
      error = assert_raises(Vunly::UnavailableError) { client.customers.get("acme") }
      assert_equal ["invalid_response", 200], [error.code, error.status]
      sleep 0.2
      assert_equal 1, server.hits, "invalid_response is not retried"
      assert_operator @sent, :<, 16 * 1024 * 1024, "the SDK must not read a body it already knows is too large"
    end
  end

  def test_a_streamed_body_without_a_length_is_cut_at_the_cap
    @sent = 0
    handler = ->(io, _req) { flood(io, "#{HEAD_200}\r\n", total: 200 * 1024 * 1024) }
    with_handler(handler) do |server|
      client, = new_client(server)
      error = assert_raises(Vunly::UnavailableError) { client.customers.get("acme") }
      assert_equal ["invalid_response", 200], [error.code, error.status]
      sleep 0.3
      assert_equal 1, server.hits
      assert_operator @sent, :<, 48 * 1024 * 1024, "the SDK must stop reading near the 16 MB cap (kernel buffers aside)"
    end
  end

  def test_a_chunked_body_with_a_huge_chunk_is_cut_at_the_cap
    @sent = 0
    handler = lambda do |io, _req|
      io.write("#{HEAD_200}Transfer-Encoding: chunked\r\n\r\n#{(200 * 1024 * 1024).to_s(16)}\r\n")
      block = "z" * 65_536
      sent = 0
      while sent < 200 * 1024 * 1024
        io.write(block)
        sent += block.bytesize
        @sent = sent
      end
    end
    with_handler(handler) do |server|
      client, = new_client(server)
      error = assert_raises(Vunly::UnavailableError) { client.customers.get("acme") }
      assert_equal "invalid_response", error.code
      sleep 0.3
      assert_operator @sent, :<, 48 * 1024 * 1024
    end
  end

  def test_an_error_status_with_a_huge_body_keeps_its_class
    @sent = 0
    handler = lambda do |io, _req|
      flood(io, "HTTP/1.1 404 Not Found\r\nConnection: close\r\nContent-Length: #{17 * 1024 * 1024}\r\n\r\n",
            total: 64 * 1024 * 1024)
    end
    with_handler(handler) do |server|
      client, = new_client(server)
      error = assert_raises(Vunly::InvalidRequestError) { client.customers.get("acme") }
      assert_equal [404, "validation_error"], [error.status, error.code]
      assert_equal 1, server.hits
    end
  end

  def test_a_large_valid_body_under_the_cap_is_accepted
    items = Array.new(2_000) { |i| customer_json(id: "cus_#{i}", name: "n" * 1_000) } # about 2 MB
    with_server(FakeServer.json(200, { data: items, nextCursor: nil })) do |server|
      client, = new_client(server)
      assert_equal 2_000, client.customers.list.data.size
    end
  end

  # ---- no replay by the library -----------------------------------------------------------------------------------

  def test_net_http_does_not_repeat_a_read_behind_the_sdks_back
    with_server(FakeServer.drop) do |server|
      client, = new_client(server, max_retries: 0)
      assert_raises(Vunly::UnavailableError) { client.customers.get("acme") }
      assert_equal 1, server.hits, "Net::HTTP repeats an idempotent request once unless max_retries is 0"
    end
  end

  def test_a_keyed_post_is_not_replayed_by_the_library_when_retries_are_off
    with_server(FakeServer.drop) do |server|
      client, = new_client(server, max_retries: 0)
      error = assert_raises(Vunly::UnavailableError) do
        client.usage.record(id: "use-0001-abcd", customer: "acme", meter: "tokens", quantity: 1)
      end
      assert_equal 0, error.status
      assert_equal 1, server.hits
    end
  end

  def test_a_delete_is_not_replayed_by_the_library_either
    with_server(FakeServer.drop) do |server|
      client, = new_client(server, max_retries: 0)
      assert_raises(Vunly::UnavailableError) { client.plans.cancel("acme") }
      assert_equal 1, server.hits
    end
  end

  def test_a_lost_response_to_a_keyed_post_is_retried_with_the_same_id_and_learns_the_original_result
    with_server(FakeServer.drop, FakeServer.json(200, usage_json, "Idempotent-Replayed" => "true")) do |server|
      client, = new_client(server)
      result = client.usage.record(id: "use-0001-abcd", customer: "acme", meter: "tokens", quantity: 3000)
      assert result.replayed?
      assert_equal(%w[use-0001-abcd use-0001-abcd], server.requests.map { |r| r.headers["idempotency-key"] })
    end
  end

  # ---- a body that breaks mid-read --------------------------------------------------------------------------------

  def short_body
    ->(io, _req) { io.write("#{HEAD_200}Content-Length: 100\r\n\r\n{\"id\":") }
  end

  def test_a_body_that_breaks_mid_read_is_unavailable_and_retried_like_a_lost_answer
    ok = FakeServer.json(200, customer_json)
    with_handler(->(io, req) { (req.n == 1 ? short_body : ok).call(io, req) }) do |server|
      client, = new_client(server)
      assert_equal "acme", client.customers.get("acme").external_id
      assert_equal 2, server.hits
    end
  end

  def test_a_short_body_is_an_error_not_a_short_success
    with_handler(short_body) do |server|
      client, = new_client(server, max_retries: 0)
      error = assert_raises(Vunly::UnavailableError) { client.customers.get("acme") }
      assert_equal ["unavailable", 0], [error.code, error.status]
    end
  end

  def test_a_truncated_chunked_body_is_unavailable
    chunked = ->(io, _req) { io.write("#{HEAD_200}Transfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n") }
    with_handler(chunked) do |server|
      client, = new_client(server, max_retries: 0)
      error = assert_raises(Vunly::UnavailableError) { client.customers.get("acme") }
      assert_equal 0, error.status
    end
  end

  def test_a_broken_body_of_an_unkeyed_post_is_not_repeated
    with_handler(short_body) do |server|
      client, = new_client(server)
      assert_raises(Vunly::UnavailableError) { client.customer_tokens.create("acme") }
      assert_equal 1, server.hits
    end
  end

  def test_an_error_status_with_a_broken_body_keeps_its_class
    broken = ->(io, _req) { io.write("HTTP/1.1 409 Conflict\r\nConnection: close\r\nContent-Length: 100\r\n\r\n{\"err") }
    with_handler(broken) do |server|
      client, = new_client(server)
      error = assert_raises(Vunly::InvalidRequestError) { client.customers.get("acme") }
      assert_equal 409, error.status
      assert_equal 1, server.hits
    end
  end

  def test_garbage_instead_of_http_is_unavailable_with_no_library_text
    with_handler(->(io, _req) { io.write("this is not http\r\n\r\n") }) do |server|
      client, = new_client(server, max_retries: 0)
      error = assert_raises(Vunly::UnavailableError) { client.customers.get("acme") }
      assert_equal "vunly: could not reach the Vunly API (unavailable)", error.message
      assert_nil error.cause
    end
  end

  # ---- redirects, protocol, errors ---------------------------------------------------------------------------------

  def test_a_redirect_is_not_followed_and_the_key_goes_nowhere_else
    with_server(FakeServer.json(200, customer_json)) do |elsewhere|
      with_handler(redirect_to("#{elsewhere.url}/v1/customers/acme", 302)) do |server|
        client, = new_client(server)
        error = assert_raises(Vunly::UnavailableError) { client.customers.get("acme") }
        assert_equal 302, error.status
        assert_equal 1, server.hits
      end
      with_handler(redirect_to("#{elsewhere.url}/v1/usage-events", 307)) do |server|
        client, = new_client(server)
        assert_raises(Vunly::UnavailableError) do
          client.usage.record(id: "use-0001-abcd", customer: "acme", meter: "tokens", quantity: 1)
        end
        assert_equal 1, server.hits
      end
      assert_equal 0, elsewhere.hits, "the Authorization header must never reach another host"
    end
  end

  def redirect_to(location, status)
    head = "HTTP/1.1 #{status} Redirect\r\nLocation: #{location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    ->(io, _req) { io.write(head) }
  end

  def test_cleartext_http_is_plain_http_1_1_without_upgrade_headers
    with_server(FakeServer.json(200, customer_json)) do |server|
      client, = new_client(server)
      client.customers.get("acme")
      headers = server.requests.fetch(0).headers
      refute headers.key?("upgrade")
      refute headers.key?("http2-settings")
      assert_equal "close", headers["connection"]
    end
  end

  def test_an_unreachable_server_is_unavailable_with_a_fixed_message
    port = TCPServer.open("127.0.0.1", 0) { |s| s.addr[1] } # a port nobody listens on
    client = Vunly::Client.new(api_key: SYNTHETIC_KEY, base_url: "http://127.0.0.1:#{port}", max_retries: 0)
    error = assert_raises(Vunly::UnavailableError) { client.customers.get("acme") }
    assert_equal "vunly: could not reach the Vunly API (unavailable)", error.message
    assert_nil error.cause
    refute_includes error.full_message, SYNTHETIC_KEY
    assert_equal 0, error.status
  end

  def test_the_backtrace_and_message_of_a_transport_failure_hold_no_header
    with_server(FakeServer.drop) do |server|
      client, = new_client(server, max_retries: 0)
      error = assert_raises(Vunly::UnavailableError) { client.customers.get("acme") }
      text = ([error.message, error.full_message] + error.backtrace).join("\n")
      refute_includes text, "Bearer"
      refute_includes text, SYNTHETIC_KEY
    end
  end

  def test_the_wait_between_retries_is_a_call_to_the_sleeper_and_a_raising_sleeper_stops_the_call
    with_server(FakeServer.error(503, "unavailable")) do |server|
      client = Vunly::Client.new(api_key: SYNTHETIC_KEY, base_url: server.url, sleeper: ->(_s) { raise Interrupt })
      assert_raises(Interrupt) { client.customers.get("acme") }
      assert_equal 1, server.hits
    end
  end

  # ---- TLS ---------------------------------------------------------------------------------------------------------

  def tls_server
    key = OpenSSL::PKey::RSA.new(2048)
    cert = OpenSSL::X509::Certificate.new
    cert.version = 2
    cert.serial = 1
    cert.subject = cert.issuer = OpenSSL::X509::Name.parse("/CN=127.0.0.1")
    cert.public_key = key.public_key
    cert.not_before = Time.now - 60
    cert.not_after = Time.now + 3600
    ef = OpenSSL::X509::ExtensionFactory.new(cert, cert)
    cert.add_extension(ef.create_extension("subjectAltName", "IP:127.0.0.1"))
    cert.add_extension(ef.create_extension("basicConstraints", "CA:TRUE", true))
    cert.sign(key, OpenSSL::Digest.new("SHA256"))
    ctx = OpenSSL::SSL::SSLContext.new
    ctx.cert = cert
    ctx.key = key
    tcp = TCPServer.new("127.0.0.1", 0)
    ssl = OpenSSL::SSL::SSLServer.new(tcp, ctx)
    hits = []
    thread = Thread.new do
      loop do
        io = ssl.accept
        hits << io.gets("\r\n")
        while (line = io.gets("\r\n")) && line != "\r\n"; end
        payload = JSON.generate(customer_json)
        io.write("HTTP/1.1 200 OK\r\nContent-Length: #{payload.bytesize}\r\nConnection: close\r\n\r\n#{payload}")
        io.close
      rescue OpenSSL::SSL::SSLError, IOError, SystemCallError
        next
      end
    end
    thread.report_on_exception = false
    [tcp.addr[1], cert, hits, lambda {
      thread.kill
      tcp.close
    }]
  end

  def test_https_verifies_the_peer_and_works_once_the_certificate_is_trusted
    port, cert, hits, stop = tls_server
    client = Vunly::Client.new(api_key: SYNTHETIC_KEY, base_url: "https://127.0.0.1:#{port}", max_retries: 0)
    error = assert_raises(Vunly::UnavailableError) { client.customers.get("acme") }
    assert_equal "vunly: could not reach the Vunly API (unavailable)", error.message
    assert_empty hits, "an untrusted certificate must be refused before any request is sent"

    # trust the test certificate in the process-wide default store (what Net::HTTP verifies against)
    OpenSSL::SSL::SSLContext::DEFAULT_CERT_STORE.add_cert(cert)
    assert_equal "acme", client.customers.get("acme").external_id
    assert_equal 1, hits.size
  ensure
    stop&.call
  end
end
