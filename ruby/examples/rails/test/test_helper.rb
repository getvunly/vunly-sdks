# frozen_string_literal: true

ENV["RAILS_ENV"] = "test"
# synthetic values: the tests replace the client with a fake and never reach a server
ENV["VUNLY_API_KEY"] ||= "vk_test_synthetic_example_test_0001"
ENV["VUNLY_API_URL"] ||= "http://127.0.0.1:1"

require_relative "../config/environment"
require "rails/test_help"

# Stands in for Vunly::Client: answers the check and the record the way a test says, and remembers what it was asked.
class FakeVunly
  Decision = Struct.new(:allowed, :reason) do
    def allowed? = allowed
  end

  attr_reader :checks, :records

  def initialize(allowed: true, reason: "ok", check_error: nil, record_error: nil, replayed: false)
    @decision = Decision.new(allowed, reason)
    @check_error = check_error
    @record_error = record_error
    @replayed = replayed
    @checks = []
    @records = []
  end

  def entitlements = self
  def usage = self

  def check(**args)
    @checks << args
    raise @check_error if @check_error

    @decision
  end

  def record(**args)
    @records << args
    raise @record_error if @record_error

    Vunly::UsageResult.new(event: nil, remaining: 99, replayed: @replayed, plan_overage: false, request_id: nil)
  end
end
