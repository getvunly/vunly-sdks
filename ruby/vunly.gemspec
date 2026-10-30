# frozen_string_literal: true

require_relative "lib/vunly/version"

# This gem is not published to a package registry: use it from a Gemfile with a `path:` line.
Gem::Specification.new do |spec|
  spec.name = "vunly"
  spec.version = Vunly::VERSION
  spec.authors = ["Vunly"]

  spec.summary = "Ruby client for the Vunly API: record usage, check entitlements, grant credits (sandbox)."
  spec.description = "A small client for the Vunly API with no runtime dependency: customers, credits, usage events, " \
                     "entitlement checks, plans and webhook verification. Retries by operation class with the same " \
                     "idempotency id, typed errors, a hard per-attempt deadline, and a Rails controller concern. " \
                     "Sandbox only: no real payments."
  spec.homepage = "https://github.com/getvunly/vunly-sdks"
  spec.license = "Apache-2.0"
  spec.required_ruby_version = ">= 3.3"

  spec.metadata = {
    "homepage_uri" => spec.homepage,
    "source_code_uri" => "https://github.com/getvunly/vunly-sdks/tree/main/ruby",
    "changelog_uri" => "https://github.com/getvunly/vunly-sdks/blob/main/ruby/CHANGELOG.md",
    "rubygems_mfa_required" => "true"
  }

  spec.files = Dir["lib/**/*.rb"] + %w[LICENSE NOTICE README.md CHANGELOG.md]
  spec.require_paths = ["lib"]
  spec.extra_rdoc_files = %w[README.md CHANGELOG.md]
end
