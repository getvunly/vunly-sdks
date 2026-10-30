# frozen_string_literal: true

require "logger" # Rails 8.0 needs it loaded before active_support on some Rubies
require "rails"
require "action_controller/railtie"

Bundler.require(*Rails.groups) # requires "vunly", which loads its Railtie because Rails is present

module VunlyExample
  # A minimal API-only application: one controller, one initializer.
  class Application < Rails::Application
    config.api_only = true
    config.eager_load = false
    config.hosts.clear # the verifier calls it by IP address
    config.logger = ActiveSupport::Logger.new($stdout)
    config.log_level = :info
    config.secret_key_base = SecureRandom.hex(64) # the app has no sessions or cookies; Rails wants one anyway
    config.action_dispatch.show_exceptions = :none if Rails.env.test? # let a test see the exception itself
  end
end
