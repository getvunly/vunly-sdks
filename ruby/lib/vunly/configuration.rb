# frozen_string_literal: true

module Vunly
  # The settings of the shared client behind {Vunly.client}: the way to configure Vunly once, in a Rails initializer or at
  # boot. The SDK never reads these from the environment itself; your initializer does, explicitly.
  #
  # @example config/initializers/vunly.rb
  #   Vunly.configure do |config|
  #     config.api_key  = ENV.fetch("VUNLY_API_KEY")
  #     config.base_url = ENV.fetch("VUNLY_API_URL")
  #   end
  class Configuration
    include Undumpable

    # @return [String, nil] the API address (required)
    attr_accessor :base_url
    # @return [Numeric] seconds for one attempt as a whole; default 10
    attr_accessor :timeout
    # @return [Integer] retries of a retryable failure; default 2, 0 turns them off
    attr_accessor :max_retries

    def initialize
      reset
    end

    # Back to the defaults and no key (tests).
    #
    # @return [void]
    def reset
      @api_key = nil
      @base_url = nil
      @timeout = 10
      @max_retries = 2
    end

    # @param value [String, nil] the API key (a sandbox key, +vk_test_...+)
    def api_key=(value)
      @api_key = value.nil? ? nil : Secret.new(value)
    end

    # @return [String, nil] the API key
    def api_key
      @api_key&.reveal
    end

    # Never contains the key.
    #
    # @return [String]
    def inspect
      "#<Vunly::Configuration base_url=#{base_url.inspect} timeout=#{timeout.inspect} " \
        "max_retries=#{max_retries.inspect} api_key=#{@api_key ? Secret::HIDDEN : 'nil'}>"
    end
    alias to_s inspect

    # @param queue [PP] the pretty printer
    def pretty_print(queue)
      queue.text(inspect)
    end

    # @return [Client] a new client from these settings
    # @raise [ConfigurationError] when the key or the address is missing
    def build_client
      Client.new(api_key: api_key, base_url: base_url, timeout: timeout, max_retries: max_retries)
    end
  end

  @configuration = Configuration.new
  @client = nil
  @lock = Mutex.new

  class << self
    # @return [Configuration] the settings of the shared client
    attr_reader :configuration

    # Yields the {Configuration}; a client built before is discarded so the next {client} call uses the new settings.
    #
    # @yieldparam config [Configuration]
    # @return [Configuration]
    def configure
      yield configuration
      @lock.synchronize { @client = nil }
      configuration
    end

    # The shared client, built on first use from {configuration}. Share it: it is safe for concurrent use.
    #
    # @return [Client]
    # @raise [ConfigurationError] when the key or the address was never set
    def client
      @lock.synchronize { @client ||= configuration.build_client }
    end

    # Replaces the shared client (tests: pass a fake).
    #
    # @param client [Client, #entitlements]
    def client=(client)
      @lock.synchronize { @client = client }
    end

    # Forgets the settings and the shared client.
    def reset!
      @lock.synchronize do
        @configuration.reset # in place: +config.vunly+ in Rails holds this same object
        @client = nil
      end
    end
  end
end
