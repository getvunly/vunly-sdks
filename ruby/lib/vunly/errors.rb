# frozen_string_literal: true

module Vunly
  # Raised when the client is configured wrongly (an empty key, a base URL that is not an address). The message names the
  # setting and never its value.
  class ConfigurationError < ArgumentError; end

  # Base class of every failure of a call. Rescue one of the four subclasses ({AuthError}, {LimitReachedError},
  # {InvalidRequestError}, {UnavailableError}) or match on {#code}, never on the message.
  #
  # The message never contains the API key, a customer token, a header or a URL with credentials, and an error never has
  # a +cause+: the exception of the HTTP library is dropped on purpose because some libraries put request headers in it.
  class Error < StandardError
    # @return [String] the API's stable machine code (+"insufficient_credits"+, +"rate_limited"+ ...) or one of the SDK's
    #   own: +"unavailable"+ (no answer) and +"invalid_response"+ (a 2xx whose body lacks a promised field)
    attr_reader :code

    # @return [Integer] the HTTP status, or +0+ when there was no response
    attr_reader :status

    # @return [String, nil] the API's +Request-Id+; quote it in a support request
    attr_reader :request_id

    # @return [Hash, nil] the API's structured detail (for example +balance+ and +required+ on a 402)
    attr_reader :details

    # @return [Float, nil] the server's +Retry-After+ in seconds, when it sent one
    attr_reader :retry_after_seconds

    # @return [String] the message as the API (or the SDK) put it, without the status and request id
    attr_reader :api_message

    # @param message [String]
    # @param code [String]
    # @param status [Integer]
    # @param request_id [String, nil]
    # @param details [Hash, nil]
    # @param retry_after_seconds [Float, nil]
    def initialize(message = "request failed", code: "unknown", status: 0, request_id: nil, details: nil,
                   retry_after_seconds: nil)
      @api_message = message
      @code = code
      @status = status
      @request_id = request_id
      @details = details
      @retry_after_seconds = retry_after_seconds
      super(compose(message))
    end

    # @return [Boolean] whether trying again later with the SAME id can help; false for everything but
    #   {UnavailableError}
    def retryable?
      false
    end

    # Builds the right subclass for a status.
    #
    # @api private
    # @return [Error]
    def self.for_status(status, code, message, request_id: nil, details: nil, retry_after_seconds: nil)
      klass =
        case status
        when 402 then LimitReachedError
        when 401, 403 then AuthError
        when 0, 429, 500..599 then UnavailableError
        else InvalidRequestError
        end
      klass.new(message, code: code, status: status, request_id: request_id, details: details,
                         retry_after_seconds: retry_after_seconds)
    end

    private

    def compose(message)
      text = "vunly: #{message}"
      text += @status.zero? ? " (#{@code})" : " (#{@status} #{@code})"
      text += " request #{@request_id}" if @request_id
      text
    end
  end

  # 401 or 403: a missing, wrong or revoked key, or a key without the scope. Not retryable.
  class AuthError < Error; end

  # 402: the customer has too few credits. Nothing was debited. Do not run the work. Not retryable.
  class LimitReachedError < Error
    # @return [Integer, nil] the customer's balance reported by the 402, when the API sent it
    def balance
      detail_integer("balance")
    end

    # @return [Integer, nil] the cost the refused request needed, when the API sent it
    def required
      detail_integer("required")
    end

    private

    def detail_integer(key)
      value = details.is_a?(Hash) ? details[key] : nil
      value.is_a?(Integer) ? value : nil
    end
  end

  # Every other 4xx (400, 404, 409) and any local validation failure: fix the request.
  class InvalidRequestError < Error; end

  # No answer (network, timeout), a 429, a 5xx that survived the SDK's own retries, or +invalid_response+. It is safe to
  # try again later with the SAME id. A 429 whose +details["reason"]+ is +"plan_included_volume"+ is the Free plan's
  # monthly volume stop, not a rate limit: it is never retried automatically ({#plan_stop?}).
  class UnavailableError < Error
    # @return [Boolean] true unless this is the plan-volume stop or a redirect (never followed, never worth repeating)
    def retryable?
      !plan_stop? && code != "unexpected_redirect"
    end

    # @return [Boolean] whether this is the Free plan's monthly volume stop (its +Retry-After+ is the time to the start
    #   of the next month)
    def plan_stop?
      status == 429 && details.is_a?(Hash) && details["reason"] == "plan_included_volume"
    end
  end

  # Raised by {Webhooks.verify}. Answer a failed verification with a 400 and do not process the payload.
  class WebhookError < StandardError
    # @return [String] one of +missing_header+, +malformed_header+, +stale_timestamp+, +no_matching_signature+ or
    #   +invalid_payload+
    attr_reader :reason

    # @param reason [String]
    def initialize(reason)
      @reason = reason
      super("vunly: webhook verification failed (#{reason})")
    end
  end
end
