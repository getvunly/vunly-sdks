# frozen_string_literal: true

require "vunly"

# The key and the address come from your environment. Never write them into source, tests, logs or prompts.
client = Vunly::Client.new(api_key: ENV.fetch("VUNLY_API_KEY"), base_url: ENV.fetch("VUNLY_API_URL"))

client.customers.upsert("cus_demo_01")
client.plans.subscribe("cus_demo_01", "starter")

decision = client.entitlements.check(customer: "cus_demo_01", feature: "generate",
                                     estimate: { meter: "tokens", quantity: 1050 })
abort("not allowed: #{decision.reason}") unless decision.allowed? # a check that cannot be answered raises: fail closed

# ... do the work, then record it once. The id names this unit of work; a retry uses the SAME id.
result = client.usage.record(id: "req_2026_0001", customer: "cus_demo_01", meter: "tokens", quantity: 1050)
puts "remaining=#{result.remaining} replayed=#{result.replayed?}"
