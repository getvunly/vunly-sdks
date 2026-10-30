# frozen_string_literal: true

require_relative "test_helper"

# The shared Retry-After vectors (conformance/retry-after-vectors.json): every SDK runs the same ones.
class RetryAfterTest < Minitest::Test
  include TestSupport

  # a copy of conformance/retry-after-vectors.json (a repository check keeps the copies identical)
  VECTORS = JSON.parse(File.read(File.join(__dir__, "retry-after-vectors.json")))

  def test_the_file_is_the_one_this_test_expects
    assert_equal 30, VECTORS["capSeconds"]
    assert_equal 86_400, VECTORS["reportCapSeconds"]
    assert_operator VECTORS["vectors"].size, :>=, 40
  end

  def test_every_vector_is_read_as_the_contract_says
    VECTORS["vectors"].each do |vector|
      got = Vunly::Transport.parse_retry_after(vector["header"])
      label = vector["header"][0, 30].inspect
      case vector["kind"]
      when "ignored" then assert_nil got, label
      when "seconds"
        assert_in_delta vector["seconds"], got, 1e-9, label
        assert_operator got, :<=, VECTORS["capSeconds"], label
      when "surface"
        assert_in_delta vector["seconds"], got, 1e-9, label
        assert_operator got, :<=, VECTORS["reportCapSeconds"], label
      else flunk "unknown kind #{vector['kind']}"
      end
    end
  end

  def test_a_very_long_wait_is_surfaced_at_once_and_never_retried_early
    %w[1e999 99999999999 1e12 Infinity].push("9" * 400).each do |header|
      with_server(FakeServer.error(429, "rate_limited", "slow", nil, "Retry-After" => header),
                  FakeServer.json(200, customer_json)) do |server|
        client, sleeps = new_client(server)
        error = assert_raises(Vunly::UnavailableError, header[0, 20]) { client.customers.get("acme") }
        assert_equal 86_400.0, error.retry_after_seconds
        assert_equal 429, error.status
        assert_equal 1, server.hits, "no early retry for #{header[0, 20]}"
        assert_empty sleeps
      end
    end
  end

  def test_a_syntactically_bad_header_means_the_normal_backoff
    bad = FakeServer.error(503, "unavailable", "x", nil, "Retry-After" => "5s")
    with_server(bad, FakeServer.json(200, customer_json)) do |server|
      client, sleeps = new_client(server)
      client.customers.get("acme")
      assert_operator sleeps.first, :<=, 0.25
    end
  end
end
