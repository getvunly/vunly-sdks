# frozen_string_literal: true

require_relative "../test_helper"

class GenerateTest < ActionDispatch::IntegrationTest
  REQUEST_ID = "req-example-0001"

  # A controller that forgets to say how much the work used.
  class ForgetfulController < ApplicationController
    include Vunly::Metered

    metered feature: "generate", meter: "tokens", customer: "acme"

    def index
      render json: { ok: true }
    end
  end

  teardown { Vunly.client = nil }

  def post_generate(path = "/generate", id: REQUEST_ID)
    post path, params: { customer: "acme", prompt: "hello world again" }, as: :json,
               headers: id ? { "X-Request-Id" => id } : {}
  end

  def fake(**options)
    FakeVunly.new(**options).tap { |client| Vunly.client = client }
  end

  test "an entitled request checks, runs the work, and records once with a server-generated id" do
    client = fake
    post_generate
    assert_response :success
    assert_equal "echo: hello world again", response.parsed_body["output"]
    assert_equal 99, response.parsed_body["creditsRemaining"]
    assert_equal [{ customer: "acme", feature: "generate", estimate: nil }], client.checks
    assert_equal 1, client.records.size
    record = client.records.first
    assert_match(/\Avunly_[0-9a-f-]{36}\z/, record[:id])
    assert_equal({ customer: "acme", meter: "tokens", quantity: 1000 }, record.except(:id))
  end

  test "a client-supplied X-Request-Id is NOT the idempotency id by default: sending it twice is two charges" do
    client = fake
    2.times { post_generate }
    ids = client.records.map { |r| r[:id] }
    assert_equal 2, ids.uniq.size, "the same header must not make the second request free"
    refute_includes ids.join, REQUEST_ID
  end

  test "the action that only sets the quantity is recorded after it, once, under an id bound to customer and header" do
    client = fake(replayed: true)
    post_generate("/summarize")
    assert_response :success
    assert_equal "hello world again", response.parsed_body["summary"]
    assert_equal "true", response.headers["Vunly-Replayed"]
    assert_equal 1, client.records.size
    assert_match(/\Areq_[0-9a-f]{60}\z/, client.records.first[:id])
  end

  test "with trust_request_id the same header and customer give the same id, another customer another id" do
    client = fake
    2.times { post_generate("/summarize") }
    post "/summarize", params: { customer: "other", prompt: "x" }, as: :json, headers: { "X-Request-Id" => REQUEST_ID }
    ids = client.records.map { |r| r[:id] }
    assert_equal ids[0], ids[1]
    refute_equal ids[0], ids[2]
  end

  test "a customer with no plan is refused before any work and nothing is recorded" do
    client = fake(allowed: false, reason: "no_active_plan")
    post_generate
    assert_response :forbidden
    assert_equal "no_active_plan", response.parsed_body.dig("error", "code")
    assert_nil response.parsed_body["output"]
    assert_empty client.records
  end

  test "a customer without credits is a 402 before any work" do
    client = fake(allowed: false, reason: "insufficient_credits")
    post_generate
    assert_response :payment_required
    assert_empty client.records
  end

  test "a check that cannot be answered fails closed: 503, no work, no record" do
    client = fake(check_error: Vunly::UnavailableError.new("down", code: "unavailable", status: 0))
    post_generate
    assert_response :service_unavailable
    assert_equal "vunly_unavailable", response.parsed_body.dig("error", "code")
    assert_nil response.parsed_body["output"]
    assert_empty client.records
  end

  test "no credits at record time answers 402 and replaces the prepared response" do
    fake(record_error: Vunly::LimitReachedError.new("no", code: "insufficient_credits", status: 402))
    post_generate("/summarize")
    assert_response :payment_required
    assert_equal "insufficient_credits", response.parsed_body.dig("error", "code")
    assert_nil response.parsed_body["summary"], "the work's answer is withheld"
  end

  test "a record that cannot be answered answers 503 and replaces the prepared response" do
    fake(record_error: Vunly::UnavailableError.new("down", code: "unavailable", status: 0, retry_after_seconds: 3))
    post_generate("/summarize")
    assert_response :service_unavailable
    assert_equal "3", response.headers["Retry-After"]
    assert_nil response.parsed_body["summary"]
  end

  test "an action that never says how much it used is a loud error, not a skipped record" do
    fake
    with_routing do |set|
      set.draw { get "/forgetful", to: "generate_test/forgetful#index" }
      assert_raises(Vunly::Metered::MissingQuantity) { get "/forgetful" }
    end
  end

  test "a caller error (an unknown customer, a bad id) is a 404 or 400 with the API's code and no detail, not a 500" do
    fake(check_error: Vunly::InvalidRequestError.new("No such customer SECRET-DETAIL", code: "not_found", status: 404))
    post_generate
    assert_response :not_found
    assert_equal "not_found", response.parsed_body.dig("error", "code")
    refute_includes response.body, "SECRET-DETAIL"

    fake(check_error: Vunly::InvalidRequestError.new("bad", code: "validation_error", status: 400))
    post_generate
    assert_response :bad_request
  end

  test "our own key being refused is a generic 502, never the cause" do
    fake(check_error: Vunly::AuthError.new("key vk_test_synthetic_leak refused", code: "unauthenticated", status: 401))
    post_generate
    assert_response :bad_gateway
    assert_equal "server_error", response.parsed_body.dig("error", "code")
    refute_includes response.body, "vk_test"
  end

  test "the Railtie exposes the configuration without the key" do
    assert_same Vunly.configuration, Rails.application.config.vunly
    refute_includes Rails.application.config.vunly.inspect, ENV.fetch("VUNLY_API_KEY")
  end
end
