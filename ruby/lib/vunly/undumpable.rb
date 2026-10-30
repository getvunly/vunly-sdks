# frozen_string_literal: true

module Vunly
  # Included by every object that holds, or can reach, the API key or a customer token. Such an object is never
  # serialised: +Marshal.dump+, +YAML.dump+ / +to_yaml+ (Psych walks instance variables unless told otherwise) and
  # +Psych.safe_dump+ all fail instead of writing the secret.
  #
  # @api private
  module Undumpable
    # @raise [TypeError] always
    def marshal_dump
      raise TypeError, "#{self.class} holds a secret and cannot be dumped"
    end

    # Psych calls this to build the YAML node of an object.
    #
    # @raise [TypeError] always
    def encode_with(_coder)
      raise TypeError, "#{self.class} holds a secret and cannot be dumped"
    end
  end
end
