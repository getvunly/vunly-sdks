# frozen_string_literal: true

module Vunly
  # Holds a value that must never be printed (the API key, a customer token) and says +<hidden>+ however it is
  # formatted: +to_s+, +inspect+, +pp+, +to_json+, +as_json+ and +Marshal.dump+ (which refuses).
  #
  # @api private
  class Secret
    include Undumpable

    HIDDEN = "<hidden>"

    # @param value [String]
    def initialize(value)
      @value = value.to_s.dup.freeze
    end

    # @return [String] the secret itself; only the transport and an explicit reader call this
    def reveal
      @value
    end

    # @return [Boolean] true when there is nothing in it
    def empty?
      @value.strip.empty?
    end

    # @return [String]
    def to_s
      HIDDEN
    end

    # @return [String]
    def inspect
      HIDDEN
    end

    # @return [String]
    def to_json(*)
      HIDDEN.to_json
    end

    # @return [String]
    def as_json(*)
      HIDDEN
    end

    # @param queue [PP] the pretty printer
    def pretty_print(queue)
      queue.text(HIDDEN)
    end
  end
end
