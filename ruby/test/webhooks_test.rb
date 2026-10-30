# frozen_string_literal: true

require_relative "test_helper"
require "openssl"

class WebhooksTest < Minitest::Test
  NOW = Time.at(1_800_000_000)
  PAYLOAD = '{"id":"evt_1","type":"credits.low","createdAt":"2026-01-01T00:00:00Z","data":{"a":1}}'

  def sign(secret, timestamp, payload = PAYLOAD)
    digest = OpenSSL::HMAC.hexdigest("SHA256", secret, "#{timestamp}.#{payload}")
    "t=#{timestamp},v1=#{digest}"
  end

  def reason(header: sign("s", NOW.to_i), payload: PAYLOAD, secrets: ["s"])
    Vunly.verify_webhook(payload, header, secrets, now: NOW)
    nil
  rescue Vunly::WebhookError => e
    e.reason
  end

  def test_a_vector_computed_outside_ruby_verifies
    # HMAC-SHA256("whsec_synthetic_vector", "1798761600." + body), computed with Node's crypto module
    body = '{"id":"evt_1","type":"usage.recorded","createdAt":"2026-10-28T10:00:00.000Z","data":{"n":1}}'
    header = "t=1798761600,v1=de4a8312482f941807a6e5b1fb10c48e82391674d163ed3f5b0187930b5b86f0"
    event = Vunly::Webhooks.verify(body, header, "whsec_synthetic_vector", now: Time.at(1_798_761_600 + 10))
    assert_equal ["evt_1", "usage.recorded", { "n" => 1 }], [event.id, event.type, event.data]
    assert_equal Time.utc(2026, 10, 28, 10), event.created_at
  end

  def test_a_valid_delivery_verifies_and_two_secrets_are_accepted_while_rotating
    event = Vunly.verify_webhook(PAYLOAD, sign("old", NOW.to_i), %w[new old], now: NOW)
    assert_equal ["evt_1", "credits.low"], [event.id, event.type]
  end

  def test_the_timestamp_may_be_off_by_five_minutes_either_way_but_no_more
    assert_nil reason(header: sign("s", NOW.to_i - 300))
    assert_nil reason(header: sign("s", NOW.to_i + 300))
    assert_equal "stale_timestamp", reason(header: sign("s", NOW.to_i - 301))
    assert_equal "stale_timestamp", reason(header: sign("s", NOW.to_i + 301))
  end

  def test_each_way_to_fail
    assert_equal "missing_header", reason(header: "")
    assert_equal "missing_header", reason(header: nil)
    assert_equal "missing_header", reason(header: "   ")
    assert_equal "malformed_header", reason(header: "garbage")
    assert_equal "malformed_header", reason(header: "t=abc,v1=00")
    assert_equal "malformed_header", reason(header: "t=#{NOW.to_i}")
    assert_equal "no_matching_signature", reason(header: sign("other", NOW.to_i))
    # a v1 that is not 64 hex characters is not a signature (the shared vector "v1 is not 64 hex characters")
    assert_equal "malformed_header", reason(header: "t=#{NOW.to_i},v1=nothex")
    assert_equal "malformed_header", reason(header: "t=#{NOW.to_i},v1=00")
    assert_equal "malformed_header", reason(header: "t=#{NOW.to_i},v1=")
    assert_equal "no_matching_signature", reason(payload: "#{PAYLOAD} ")
  end

  def test_an_invalid_payload_with_a_good_signature
    ["{\"nope\":true}", "not json", "[]", '{"id":"","type":"x"}', '{"id":"a","type":3}'].each do |bad|
      assert_equal "invalid_payload", reason(header: sign("s", NOW.to_i, bad), payload: bad), bad
    end
  end

  def test_the_raw_bytes_are_what_is_signed
    payload = "{\"id\":\"e\",\"type\":\"t\",\"data\":\"café\"}".b
    assert_nil reason(header: sign("s", NOW.to_i, payload), payload: payload)
    assert_equal "no_matching_signature", reason(header: sign("s", NOW.to_i, payload), payload: payload.sub("e", "E"))
  end

  def test_a_second_v1_signature_is_enough
    good = sign("s", NOW.to_i).split("v1=").last
    assert_nil reason(header: "t=#{NOW.to_i},v1=#{'0' * 64},v1=#{good}")
  end

  def test_the_payload_must_be_a_string
    assert_raises(ArgumentError) { Vunly.verify_webhook({ "id" => "x" }, "t=1,v1=00", ["s"]) }
  end

  def test_the_error_message_does_not_contain_the_secret_or_the_payload
    error = assert_raises(Vunly::WebhookError) { Vunly.verify_webhook(PAYLOAD, sign("other", NOW.to_i), ["whsec_synthetic_x"], now: NOW) }
    refute_includes error.message, "whsec_synthetic_x"
    refute_includes error.message, "evt_1"
  end
end
