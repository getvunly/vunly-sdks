# frozen_string_literal: true

require "json"
require "time"
require "uri"

require_relative "vunly/version"
require_relative "vunly/undumpable"
require_relative "vunly/secret"
require_relative "vunly/errors"
require_relative "vunly/models"
require_relative "vunly/bounded_http"
require_relative "vunly/transport"
require_relative "vunly/resources"
require_relative "vunly/client"
require_relative "vunly/webhooks"
require_relative "vunly/configuration"

# The Ruby client for the Vunly API: customers, credits, usage, entitlement checks, plans and webhooks. Sandbox only for
# now. No runtime dependency: the Ruby standard library only (+net/http+, +json+, +openssl+).
#
# The loop every integration is built from:
#
#   client = Vunly::Client.new(api_key: ENV.fetch("VUNLY_API_KEY"), base_url: ENV.fetch("VUNLY_API_URL"))
#   decision = client.entitlements.check(customer: "acme", feature: "generate")
#   raise "not allowed" unless decision.allowed? # a failed check raises: fail closed
#   # ... run the work ...
#   client.usage.record(id: request_id, customer: "acme", meter: "tokens", quantity: 1050)
#
# The check is advisory and fails closed; the record is final. The +id+ names the unit of work (your request or job id),
# is sent as the Idempotency-Key, and is never generated for you: retry with the SAME one.
module Vunly
end

# Rails: a Railtie (+config.vunly+) and a controller concern ({Vunly::Metered}). Loaded only when Rails is.
require_relative "vunly/railtie" if defined?(Rails::Railtie)
