# frozen_string_literal: true

# Two metered actions. The concern checks the entitlement BEFORE the action (fail closed) and records the usage ONCE.
#
# /generate gets its idempotency id from the SERVER (the default): an X-Request-Id sent by the client is never trusted, so
# repeating a request is a new, billed request. /summarize trusts the header on purpose (trust_request_id), bound to the
# customer: a client that retries with the same X-Request-Id is not billed twice. Choose per endpoint, knowing your clients.
class GenerateController < ApplicationController
  include Vunly::Metered

  TOKENS_PER_REQUEST = 1000 # a real application reports what the work actually used

  metered feature: "generate", meter: "tokens", only: :create,
          customer: ->(controller) { controller.params.require(:customer) }

  metered feature: "generate", meter: "tokens", only: :summarize, trust_request_id: true,
          customer: ->(controller) { controller.params.require(:customer) }

  # The action records itself, to put the outcome in its own answer.
  def create
    output = "echo: #{params.require(:prompt)}" # the work
    result = vunly_record!(TOKENS_PER_REQUEST)
    render json: { output: output, replayed: result.replayed?, creditsRemaining: result.remaining }
  end

  # The action only says how much it used; the concern records after it and sets the Vunly-Replayed header.
  def summarize
    summary = params.require(:prompt).to_s.split.first(3).join(" ")
    vunly_quantity TOKENS_PER_REQUEST
    render json: { summary: summary }
  end
end
