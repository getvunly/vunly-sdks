# frozen_string_literal: true

require "securerandom"
require "openssl"
require "active_support/concern"
require "active_support/core_ext/class/attribute"

module Vunly
  # A controller concern for the loop every integration is built from: ask first, do the work, record once.
  #
  #   class GenerateController < ApplicationController
  #     include Vunly::Metered
  #
  #     metered feature: "generate", meter: "tokens", only: :create,
  #             customer: ->(c) { c.params.require(:customer) }
  #
  #     def create
  #       output = Model.run(params[:prompt])
  #       vunly_quantity output.token_count # what the work used, in the meter's own units
  #       render json: { output: output.text }
  #     end
  #   end
  #
  # +metered+ adds one +around_action+ that
  #
  # 1. checks the entitlement before the action. A check that cannot be answered ({UnavailableError}) FAILS CLOSED: the
  #    action does not run and the answer is 503. A "no" is 403 (+no_active_plan+, +feature_not_in_plan+) or 402
  #    (+insufficient_credits+).
  # 2. runs the action.
  # 3. if the action answered with a status below 400 and has not recorded yet, records the usage once, with an idempotency
  #    id generated on the server for the request (see +id:+ and +trust_request_id:+ of +metered+; a header the
  #    client sends is NOT trusted unless you say so). The quantity is
  #    what the action set with {#vunly_quantity}, or the +quantity:+ option. If neither is there, {MissingQuantity} is
  #    raised: usage is never silently skipped.
  #
  # An action that wants the result in its own answer calls {#vunly_record!} itself (it records at once and returns the
  # {UsageResult}); the around action then does not record again.
  #
  # A {LimitReachedError} (nothing was debited) is answered 402 and an {UnavailableError} 503, also when they come after the
  # action has prepared its response: the response is replaced. Each answer is a method you can override in the controller
  # ({#vunly_denied}, {#vunly_limit_reached}, {#vunly_unavailable}, {#vunly_invalid_request}, {#vunly_auth_failed}).
  # A request
  # Vunly refuses as malformed ({InvalidRequestError}: an unknown customer, an id that is not an id) is the caller's mistake:
  # 404 or 400 with the API's code and no detail. {AuthError} (our key is wrong or lacks a scope) is OUR problem: 502 with a
  # generic body, and the cause goes to the log, not to the caller.
  module Metered
    extend ActiveSupport::Concern

    # The action finished with a success but no quantity was given to record.
    class MissingQuantity < StandardError
      def initialize(message = "the action succeeded but no quantity was set (call vunly_quantity or pass quantity:)")
        super
      end
    end

    included do
      class_attribute :vunly_options, instance_accessor: false, default: nil
      rescue_from Vunly::LimitReachedError, with: :vunly_limit_reached
      rescue_from Vunly::UnavailableError, with: :vunly_unavailable
      rescue_from Vunly::InvalidRequestError, with: :vunly_invalid_request
      rescue_from Vunly::AuthError, with: :vunly_auth_failed
    end

    class_methods do
      # Meters the actions this controller (or +only:+/+except:+) serves.
      #
      # @param feature [String, Symbol, Proc] the feature to check; a Proc/lambda is called with the controller
      # @param meter [String, Symbol, Proc] the meter to record against
      # @param customer [String, Symbol, Proc] whose usage this is: a String, the NAME of a controller method, or a lambda
      #   called with the controller
      # @param quantity [Integer, Symbol, Proc, nil] used when the action did not call {#vunly_quantity}
      # @param id [String, Symbol, Proc, nil] the idempotency id, naming the unit of work (a job or order id you control);
      #   8 to 128 characters of +A-Za-z0-9_.:-+. Default: generated on the server for each request.
      # @param trust_request_id [Boolean] derive the id from the +X-Request-Id+ header the client sent and the customer,
      #   so a client that retries a request with the SAME header is not billed twice. Off by default: whoever can send the
      #   header can repeat it, and the work would run again with no charge. Turn it on only for clients you trust.
      # @param estimate [Hash, Symbol, Proc, nil] the usage the work is expected to need, for the check
      # @param filters [Hash] +only:+, +except:+, +if:+, +unless:+ as for any callback
      # @return [void]
      def metered(feature:, meter:, customer:, quantity: nil, id: nil, trust_request_id: false, estimate: nil, **filters)
        options = { feature: feature, meter: meter, customer: customer, quantity: quantity, id: id,
                    trust_request_id: trust_request_id, estimate: estimate }.freeze
        self.vunly_options = options
        around_action(**filters) { |controller, action| controller.send(:vunly_meter, options, action) }
      end
    end

    private

    # The shared client; override to inject another.
    #
    # @return [Client]
    def vunly_client
      Vunly.client
    end

    # Call from the action: how much the work used, in the METER's units (for example tokens), not credits. The usage is
    # recorded after the action, once.
    #
    # @param quantity [Integer]
    # @return [Integer]
    def vunly_quantity(quantity)
      @vunly_quantity = quantity
    end

    # Records the usage now and returns the result, so the action can put +remaining+ or +replayed?+ in its answer. Raises
    # {LimitReachedError} or {UnavailableError}; the concern answers 402 or 503 for them.
    #
    # @param quantity [Integer, nil] defaults to {#vunly_quantity}, then to the +quantity:+ option
    # @return [UsageResult]
    def vunly_record!(quantity = nil)
      options = @vunly_options or raise ArgumentError, "vunly_record! needs a controller that declares `metered`"
      amount = quantity || @vunly_quantity || vunly_resolve(options[:quantity])
      raise MissingQuantity if amount.nil?

      @vunly_result = vunly_client.usage.record(
        id: vunly_idempotency_id(options), customer: @vunly_customer, meter: vunly_name(options[:meter]), quantity: amount
      )
      response.set_header("Vunly-Replayed", @vunly_result.replayed.to_s)
      @vunly_result
    end

    # The idempotency id of this request's record, fixed for the whole request. By default it is GENERATED HERE, never taken
    # from the client: a client-supplied header cannot make billable work run again without a charge. +id:+ names the unit
    # of work yourself (a job id, an order id); +trust_request_id: true+ derives it from +X-Request-Id+ and the customer.
    def vunly_idempotency_id(options)
      @vunly_idempotency_id ||=
        if options[:id]
          vunly_resolve(options[:id])
        elsif options[:trust_request_id]
          # a hash of customer and header: 64 characters of the allowed alphabet, and one customer cannot reuse another's
          "req_#{OpenSSL::Digest::SHA256.hexdigest("#{@vunly_customer}\0#{request.request_id}")[0, 60]}"
        else
          "vunly_#{SecureRandom.uuid}"
        end
    end

    def vunly_meter(options, action)
      @vunly_options = options
      @vunly_quantity = @vunly_result = @vunly_idempotency_id = nil
      @vunly_customer = vunly_resolve(options[:customer])
      return unless vunly_allowed?(options)

      action.call
      return if response.status >= 400 || @vunly_result

      vunly_record!
    end

    # The check, answered here so that an action that was not allowed never runs.
    def vunly_allowed?(options)
      decision = vunly_client.entitlements.check(customer: @vunly_customer, feature: vunly_name(options[:feature]),
                                                 estimate: vunly_resolve(options[:estimate]))
      return true if decision.allowed?

      vunly_denied(decision)
      false
    rescue UnavailableError => e
      vunly_unavailable(e)
      false
    end

    # A feature or a meter: a String or Symbol is its name; a Proc is called with the controller.
    def vunly_name(value)
      value.is_a?(Proc) ? value.call(self).to_s : value.to_s
    end

    # A String is itself, a Symbol names a controller method, a Proc is called with the controller.
    def vunly_resolve(value)
      case value
      when Symbol then send(value)
      when Proc then value.call(self)
      else value
      end
    end

    # The check said no. Override to render HTML, redirect, and so on.
    def vunly_denied(decision)
      status = decision.reason == "insufficient_credits" ? 402 : 403
      vunly_respond(status, decision.reason, "this customer may not do that now")
    end

    # There were no credits at record time (nothing was debited).
    def vunly_limit_reached(_error)
      vunly_respond(402, "insufficient_credits", "not enough credits; nothing was debited")
    end

    # The check or the record could not be answered. A check fails closed; a record that was lost is safe to repeat with
    # the same X-Request-Id.
    def vunly_unavailable(error)
      vunly_respond(503, "vunly_unavailable", "usage could not be checked or recorded; try again",
                    retry_after: error.retry_after_seconds)
    end

    # Vunly refused the request as malformed or about something that does not exist: the caller's mistake.
    def vunly_invalid_request(error)
      vunly_respond(error.status == 404 ? 404 : 400, error.code, "the request was not accepted")
    end

    # Our key was refused: not the caller's business. The cause is logged (its message never holds the key).
    def vunly_auth_failed(error)
      ::Rails.logger&.error("vunly: #{error.message}")
      vunly_respond(502, "server_error", "the service could not be reached on your behalf")
    end

    # Renders the answer, or REPLACES one the action already prepared (nothing has been sent yet; +render+ would raise a
    # double render).
    def vunly_respond(status, code, message, retry_after: nil)
      response.reset_body! if performed?
      response.set_header("Retry-After", retry_after.ceil.to_s) if retry_after
      payload = { error: { code: code, message: message } }
      if performed?
        response.status = status
        response.content_type = "application/json"
        response.body = JSON.generate(payload)
      else
        render json: payload, status: status
      end
    end
  end
end
