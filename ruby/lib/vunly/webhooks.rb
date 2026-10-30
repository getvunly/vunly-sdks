# frozen_string_literal: true

require "openssl"

module Vunly
  # Verification of webhook deliveries (no HTTP involved).
  module Webhooks
    # How far a signed timestamp may be from now (either way), in seconds, before it counts as stale.
    TOLERANCE = 300
    # The longest +Vunly-Signature+ header considered (bytes); a real one is a couple of hundred.
    MAX_HEADER = 4096
    # How many +v1=+ signatures are considered (two while rotating is the normal most).
    MAX_SIGNATURES = 10
    # How many signing secrets one call accepts.
    MAX_SECRETS = 10

    module_function

    # Checks a delivery. +payload+ is the RAW request body (the exact bytes, not re-encoded JSON), +header+ is the
    # +Vunly-Signature+ header and +secrets+ are the endpoint's signing secrets: pass two while rotating, a match with any
    # one is enough. The signature is HMAC-SHA256 over +"<t>." + payload+, compared in constant time.
    #
    # In a Rails controller, +payload+ is +request.raw_post+.
    #
    # @param payload [String] the raw body
    # @param header [String, nil] the +Vunly-Signature+ header, +t=<unix>,v1=<hex>+
    # @param secrets [String, Array<String>] the endpoint's signing secret(s)
    # @param now [Time, Integer] the clock, for tests
    # @return [WebhookEvent]
    # @raise [WebhookError] with a +reason+ of +missing_header+, +malformed_header+, +stale_timestamp+,
    #   +no_matching_signature+ or +invalid_payload+; answer 400 and do not process the payload
    def verify(payload, header, secrets, now: Time.now)
      raise ArgumentError, "payload must be the raw request body (a String)" unless payload.is_a?(String)

      secrets = check_secrets!(secrets)
      raise WebhookError, "missing_header" if header.nil?
      # whatever arrives as a header is a verification failure, never an exception of another kind
      raise WebhookError, "malformed_header" unless header.is_a?(String)

      header = header.b # bytes: an invalid or foreign encoding cannot make a string operation raise
      raise WebhookError, "missing_header" if header.strip.empty?
      raise WebhookError, "malformed_header" if header.bytesize > MAX_HEADER

      timestamp, signatures = parse_header(header)
      raise WebhookError, "malformed_header" if timestamp.nil? || signatures.empty?
      raise WebhookError, "stale_timestamp" if (now.to_i - timestamp.to_i).abs > TOLERANCE

      signed = "#{timestamp}.".b + payload.b
      raise WebhookError, "no_matching_signature" unless matches?(signed, signatures, secrets)

      event(payload)
    end

    # A blank secret would verify a signature anyone can compute, so it is refused, as is an empty list.
    #
    # @api private
    # @return [Array<String>]
    def check_secrets!(secrets)
      list = Array(secrets)
      # a secret needs at least one printable ASCII character that is not a space (NBSP or U+3000 alone is not one)
      usable = ->(s) { s.is_a?(String) && s.b.match?(/[\x21-\x7e]/n) }
      return list if list.size.between?(1, MAX_SECRETS) && list.all?(&usable)

      raise ArgumentError,
            "secrets must be 1 to #{MAX_SECRETS} Strings, each with a printable character (the endpoint's signing secrets)"
    end

    # @api private
    # @return [Array(String, Array<String>)] the timestamp (or nil) and the v1 signatures of a +Vunly-Signature+ header
    def parse_header(header)
      timestamp = nil
      signatures = []
      header.split(",").each do |part|
        key, value = part.strip.split("=", 2)
        next if value.nil?

        case key
        when "t" then timestamp = value
        # a v1 that is not 64 hex characters is not a signature: the header is malformed
        when "v1" then signatures << value if signatures.size < MAX_SIGNATURES && /\A[0-9a-f]{64}\z/.match?(value)
        end
      end
      [/\A-?\d+\z/.match?(timestamp.to_s) ? timestamp : nil, signatures]
    end

    # @api private
    # @return [Boolean] whether any secret signs +signed+ to any of the signatures, compared in constant time
    def matches?(signed, signatures, secrets)
      wanted = secrets.map { |secret| OpenSSL::HMAC.digest("SHA256", secret.to_s, signed) }
      # every candidate is compared (no early exit on a match), each in constant time
      signatures.map do |hex|
        next false unless /\A(\h\h)+\z/.match?(hex)

        got = [hex].pack("H*")
        wanted.map { |w| w.bytesize == got.bytesize && OpenSSL.fixed_length_secure_compare(w, got) }.any?
      end.any?
    end

    # @api private
    # @return [WebhookEvent]
    def event(payload)
      data = JSON.parse(payload.dup.force_encoding(Encoding::UTF_8))
      unless data.is_a?(Hash) && data["id"].is_a?(String) && !data["id"].empty? &&
             data["type"].is_a?(String) && !data["type"].empty?
        raise WebhookError, "invalid_payload"
      end

      WebhookEvent.new(id: data["id"], type: data["type"], created_at: created_at(data["createdAt"]), data: data["data"])
    rescue JSON::ParserError, EncodingError
      raise WebhookError, "invalid_payload"
    end

    # @api private
    # @return [Time, nil]
    def created_at(value)
      value.is_a?(String) ? Time.iso8601(value).utc : nil
    rescue ArgumentError
      nil
    end
  end

  # Shorthand for {Webhooks.verify}.
  #
  # @see Webhooks.verify
  def self.verify_webhook(...)
    Webhooks.verify(...)
  end
end
