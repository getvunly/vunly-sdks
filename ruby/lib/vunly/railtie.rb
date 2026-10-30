# frozen_string_literal: true

require "rails/railtie"
require_relative "metered"

module Vunly
  # Rails integration. Loaded automatically when Rails is present (+require "vunly"+ from the Gemfile).
  #
  # - +config.vunly+ is {Vunly.configuration}, so +config/environments/*.rb+ can set +config.vunly.timeout+.
  # - the controller concern {Metered} is available as +Vunly::Metered+ (it is not included anywhere for you).
  # - the API key is never written to the logs: nothing in the SDK logs, and +vunly_api_key+ is added to
  #   +filter_parameters+ for applications that pass it around in params.
  class Railtie < ::Rails::Railtie
    config.vunly = Vunly.configuration

    initializer "vunly.filter_parameters" do |app|
      app.config.filter_parameters |= %i[vunly_api_key]
    end
  end
end
