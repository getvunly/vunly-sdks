# frozen_string_literal: true

module Vunly
  # Talks to one Vunly API. Safe to share between threads: it holds no per-request state, and every call opens its own
  # connection. Create it once.
  #
  # @example the loop every integration is built from
  #   client = Vunly::Client.new(api_key: ENV.fetch("VUNLY_API_KEY"), base_url: ENV.fetch("VUNLY_API_URL"))
  #   decision = client.entitlements.check(customer: "acme", feature: "generate")
  #   # do the work only if decision.allowed? (a failed check raises: fail closed), then:
  #   client.usage.record(id: request_id, customer: "acme", meter: "tokens", quantity: 1050)
  class Client
    include Undumpable

    # @return [String] the API address, normalised (no trailing slash, no +/v1+)
    attr_reader :base_url
    # @return [Resources::Customers]
    attr_reader :customers
    # @return [Resources::Credits]
    attr_reader :credits
    # @return [Resources::Usage]
    attr_reader :usage
    # @return [Resources::Entitlements]
    attr_reader :entitlements
    # @return [Resources::Plans]
    attr_reader :plans
    # @return [Resources::CustomerTokens]
    attr_reader :customer_tokens

    # @param api_key [String] required; neither this nor +base_url+ is read from the environment or from disk by the SDK
    # @param base_url [String] required; the API address, for example +https://host+ or +http://127.0.0.1:8787+. A trailing
    #   slash or +/v1+ is accepted and ignored. There is no default address.
    # @param timeout [Numeric] seconds for ONE attempt as a whole (connect, headers and body); default 10
    # @param max_retries [Integer] how many times a retryable failure is repeated; default 2, +0+ turns retries off
    # @param sleeper [#call, nil] waits a number of seconds between retries (default +Kernel#sleep+; replaced in tests)
    # @raise [ConfigurationError] when a setting is missing or wrong; the message names the setting, never the key
    def initialize(api_key:, base_url:, timeout: 10, max_retries: 2, sleeper: nil)
      key = Secret.new(clean_key(api_key))
      unless timeout.is_a?(Numeric) && timeout.to_f.finite? && timeout.between?(MIN_TIMEOUT, MAX_TIMEOUT)
        raise ConfigurationError, "timeout must be a number of seconds between #{MIN_TIMEOUT} and #{MAX_TIMEOUT}"
      end
      unless max_retries.is_a?(Integer) && !max_retries.negative?
        raise ConfigurationError, "max_retries must be a whole number, 0 or more"
      end

      @base_url = Transport.normalize_base_url(base_url)
      transport = Transport.new(api_key: key, base_url: @base_url, timeout: timeout, max_retries: max_retries,
                                sleeper: sleeper || ->(seconds) { sleep(seconds) },
                                user_agent: "vunly-ruby/#{VERSION} (ruby #{RUBY_VERSION} #{RUBY_PLATFORM})")
      @transport = transport
      @customers = Resources::Customers.new(transport)
      @credits = Resources::Credits.new(transport)
      @usage = Resources::Usage.new(transport)
      @entitlements = Resources::Entitlements.new(transport)
      @plans = Resources::Plans.new(transport)
      @customer_tokens = Resources::CustomerTokens.new(transport)
      @plan_usage_resource = Resources::PlanUsageReader.new(transport)
    end

    # The shortest per-attempt timeout accepted (seconds): below it is a mistake, not a setting.
    MIN_TIMEOUT = 0.001
    # The longest per-attempt timeout accepted (seconds).
    MAX_TIMEOUT = 3600

    # One trailing LF or CRLF, such as the newline of a key read from a file, is dropped; anything that is not printable
    # ASCII afterwards (a space or tab at either end, a NUL, a lone CR, a control character or a line break inside, a
    # non-ASCII byte) is refused, naming the setting and never the value. A CR LF can never become a second header.
    def clean_key(api_key)
      unless api_key.nil? || api_key.is_a?(String)
        raise ConfigurationError,
              "api_key must be a String (from your environment)"
      end

      raw = api_key.to_s
      valid = raw.valid_encoding?
      text = valid ? raw.sub(/\r?\n\z/, "") : raw # only a trailing LF or CRLF (a key read from a file) is dropped
      raise ConfigurationError, "api_key is empty (read it from your environment)" if text.empty?
      return text if valid && text.match?(/\A[\x21-\x7e]+\z/)

      raise ConfigurationError, "api_key must be one token of printable ASCII, without spaces or control characters"
    end
    private :clean_key

    # The project's plan and this month's recorded usage. Its limits are inside +unconfirmed_limits+ and are not a
    # guarantee (+confirmed+ is false).
    #
    # @return [PlanUsage]
    def plan_usage
      @plan_usage_resource.read
    end

    # Never contains the key.
    #
    # @return [String]
    def inspect
      "#<Vunly::Client base_url=#{base_url.inspect} api_key=#{Secret::HIDDEN}>"
    end
    alias to_s inspect

    # @param queue [PP] the pretty printer
    def pretty_print(queue)
      queue.text(inspect)
    end

    # @return [Hash] only the address
    def as_json(*)
      { "base_url" => base_url }
    end
  end
end
