# frozen_string_literal: true

module Vunly
  # The areas of the API, one small object each, reached from {Client}.
  module Resources
    # Shared plumbing of the resources.
    #
    # @api private
    class Base
      include Undumpable

      # How many empty pages in a row (each promising another) an iterator tolerates before it fails with invalid_response.
      MAX_EMPTY_PAGES = 3
      # The longest id (a customer, a token) put in a URL path.
      MAX_SEGMENT = 512

      # @param transport [Transport]
      def initialize(transport)
        @transport = transport
      end

      private

      # operation classes of conformance/README.md: :r reads, :k keyed mutations, :n naturally
      # idempotent writes, :x everything else (never repeated after an ambiguous failure)
      # Returns [result, meta]: the parsed body (checked against +shape+) turned into a model by +build+.
      def send_request(verb, path, operation_class, query: nil, body: nil, id: nil, shape: nil, build: nil)
        hash, meta = @transport.call(
          Transport::Request.new(verb: verb, path: path, query: query, body: body, idempotency_key: id,
                                 operation_class: operation_class, shape: shape)
        )
        [build && hash ? built(meta) { build.call(hash) } : hash, meta]
      end

      # A field that cannot be turned into its type (a timestamp that is not ISO 8601) is an invalid_response, never a
      # bare exception.
      def built(meta)
        yield
      rescue MalformedResponse
        raise Transport.invalid_response(meta.status, meta.request_id, "unexpected shape"), cause: nil
      end

      def segment(value, name)
        # "." and ".." would be resolved as path navigation by the server or a proxy ("/customers/../credit-grants"), and
        # an empty id would merge two routes: all refused locally, as the same error the API gives for a bad id
        unless value.is_a?(String) && !value.empty? && value.length <= MAX_SEGMENT && !%w[. ..].include?(value)
          raise InvalidRequestError.new(
            "#{name} must be a non-empty string of at most #{MAX_SEGMENT} characters, and not \".\" or \"..\"",
            code: "validation_error", status: 400,
            details: { "fields" => { "id" => "1-#{MAX_SEGMENT} characters, not . or .." } }
          )
        end

        value.b.gsub(/[^A-Za-z0-9\-._~]/n) { |c| format("%%%02X", c.ord) }
      end

      def page_query(limit, cursor)
        query = {}
        query["limit"] = Integer(limit) if limit
        query["cursor"] = cursor if cursor
        query
      end

      def page(path, model, limit:, cursor:)
        build = lambda do |hash|
          items = hash["data"].map do |item|
            raise MalformedResponse, "a list item is not an object" unless item.is_a?(Hash)

            model.from_hash(item)
          end
          cursor_value = hash["nextCursor"]
          Page.new(data: items.freeze, next_cursor: cursor_value.nil? || cursor_value.empty? ? nil : cursor_value)
        end
        send_request("GET", path, :r, query: page_query(limit, cursor), build: build,
                                      shape: { "data" => "array", "nextCursor" => "string?" }).first
      end

      # An Enumerator that fetches one page per step, only as the caller consumes it.
      def each_item
        Enumerator.new do |yielder|
          cursor = nil
          seen = {}
          empty_streak = 0
          loop do
            current = yield(cursor)
            current.data.each { |item| yielder << item }
            break unless current.more?

            # pages that are empty yet promise another, again and again, are a broken server (fresh cursors do not save it)
            empty_streak = current.data.empty? ? empty_streak + 1 : 0
            if empty_streak >= MAX_EMPTY_PAGES
              raise Transport.invalid_response(200, nil, "#{MAX_EMPTY_PAGES} empty pages in a row, each promising another")
            end

            # a cursor that repeats (or does not advance) would loop forever: the server is broken, fail closed
            if seen.key?(current.next_cursor) || current.next_cursor == cursor
              raise Transport.invalid_response(200, nil, "the pagination cursor did not advance")
            end

            seen[current.next_cursor] = true
            cursor = current.next_cursor
          end
        end
      end
    end

    # Creates and reads the customers you bill.
    class Customers < Base
      CUSTOMER_SHAPE = { "id" => "string", "externalId" => "string", "createdAt" => "string" }.freeze
      private_constant :CUSTOMER_SHAPE

      # Creates the customer or returns the existing one for this id (your own id). Asking twice leaves one customer.
      #
      # @param id [String] your own id for the customer
      # @param name [String, nil] shown in the console
      # @return [Customer]
      def upsert(id, name: nil)
        body = { "externalId" => id }
        body["name"] = name unless name.nil? || name.empty?
        send_request("POST", "/v1/customers", :n, body: body, shape: CUSTOMER_SHAPE,
                                                  build: Customer.method(:from_hash)).first
      end

      # Returns one customer, by your id or Vunly's.
      #
      # @param customer [String]
      # @return [Customer]
      # @raise [InvalidRequestError] with code +"not_found"+ when the customer does not exist
      def get(customer)
        send_request("GET", "/v1/customers/#{segment(customer, 'customer')}", :r, shape: CUSTOMER_SHAPE,
                                                                                  build: Customer.method(:from_hash)).first
      end

      # Returns one page of customers, newest first.
      #
      # @param limit [Integer, nil] 1 to 100 (nil means the API's default)
      # @param cursor [String, nil] the +next_cursor+ of the previous page; opaque
      # @return [Page<Customer>]
      def list(limit: nil, cursor: nil)
        page("/v1/customers", Customer, limit: limit, cursor: cursor)
      end

      # Iterates over every customer, fetching one page at a time and only as you consume them.
      #
      # @param limit [Integer, nil] the page size
      # @return [Enumerator<Customer>]
      def each(limit: nil, &block)
        enum = each_item { |cursor| list(limit: limit, cursor: cursor) }
        block ? enum.each(&block) : enum
      end
    end

    # Grants credits and reads balances and the ledger.
    class Credits < Base
      ENTRY_SHAPE = { "id" => "string", "customerId" => "string", "kind" => "string", "credits" => "int",
                      "createdAt" => "string" }.freeze
      private_constant :ENTRY_SHAPE

      # Adds credits (a sandbox top-up). Repeating the same +id+ grants once and returns the original entry.
      #
      # @param id [String] the Idempotency-Key, 8 to 128 characters of +A-Za-z0-9_.:-+; never generated for you
      # @param customer [String]
      # @param credits [Integer]
      # @param reason [String, nil]
      # @return [LedgerEntry]
      def grant(id:, customer:, credits:, reason: nil)
        body = { "credits" => credits }
        body["reason"] = reason if reason
        path = "/v1/customers/#{segment(customer, 'customer')}/credit-grants"
        send_request("POST", path, :k, body: body, id: id, shape: ENTRY_SHAPE, build: LedgerEntry.method(:from_hash)).first
      end

      # Returns a customer's credits.
      #
      # @param customer [String]
      # @return [Balance]
      def balance(customer)
        send_request("GET", "/v1/customers/#{segment(customer, 'customer')}/balance", :r,
                     shape: { "credits" => "int", "consumed" => "int", "granted" => "int" },
                     build: Balance.method(:from_hash)).first
      end

      # Returns one page of a customer's ledger entries, newest first.
      #
      # @param customer [String]
      # @param limit [Integer, nil]
      # @param cursor [String, nil]
      # @return [Page<LedgerEntry>]
      def ledger(customer, limit: nil, cursor: nil)
        page("/v1/customers/#{segment(customer, 'customer')}/ledger", LedgerEntry, limit: limit, cursor: cursor)
      end

      # Iterates over a customer's whole ledger, one page at a time, only as you consume it.
      #
      # @param customer [String]
      # @param limit [Integer, nil]
      # @return [Enumerator<LedgerEntry>]
      def each_ledger_entry(customer, limit: nil, &block)
        enum = each_item { |cursor| ledger(customer, limit: limit, cursor: cursor) }
        block ? enum.each(&block) : enum
      end
    end

    # Records usage events.
    class Usage < Base
      SHAPE = { "balance" => "int", "event.id" => "string", "event.customerId" => "string", "event.kind" => "string",
                "event.credits" => "int", "event.createdAt" => "string" }.freeze
      private_constant :SHAPE

      # Records usage once and debits credits. It is final: a {LimitReachedError} means the credits were not there when
      # you recorded, even if a check said yes earlier; nothing was debited. A repeat of the same +id+ returns the original
      # result with +replayed+ true.
      #
      # @param id [String] names the unit of work (your request or job id) and is sent as the Idempotency-Key: 8 to 128
      #   characters of +A-Za-z0-9_.:-+. Retry with the SAME id; the API applies it at most once. Never generated for you.
      # @param customer [String]
      # @param meter [String]
      # @param quantity [Integer] in the meter's own units (for example tokens), not credits: the project's meter converts
      #   it, rounded up per request
      # @return [UsageResult]
      def record(id:, customer:, meter:, quantity:)
        body = { "customerId" => customer, "meter" => meter, "quantity" => quantity }
        hash, meta = send_request("POST", "/v1/usage-events", :k, body: body, id: id, shape: SHAPE)
        built(meta) do
          UsageResult.new(event: LedgerEntry.from_hash(hash["event"]), remaining: hash["balance"],
                          replayed: meta.replayed, plan_overage: meta.plan_overage && !meta.replayed,
                          request_id: meta.request_id)
        end
      end
    end

    # Asks whether a customer may do something now.
    class Entitlements < Base
      SHAPE = { "allowed" => "bool", "reason" => "string", "remaining" => "int", "required" => "int",
                "plan" => "string?" }.freeze
      private_constant :SHAPE

      # Read-only and advisory. If it raises (an {UnavailableError} after retries), treat that as "no": fail closed, do not
      # run the work. It never returns +allowed+ true by itself.
      #
      # @param customer [String]
      # @param feature [String]
      # @param estimate [Hash, nil] the usage the work is expected to need: +{ meter: "tokens", quantity: 1050 }+
      # @return [Entitlement]
      def check(customer:, feature:, estimate: nil)
        body = { "customerId" => customer, "feature" => feature }
        if estimate
          body["estimate"] = { "meter" => estimate[:meter] || estimate["meter"],
                               "quantity" => estimate[:quantity] || estimate["quantity"] }
        end
        send_request("POST", "/v1/entitlements/check", :r, body: body, shape: SHAPE,
                                                           build: Entitlement.method(:from_hash)).first
      end
    end

    # Defines meters and plans and puts customers on them.
    class Plans < Base
      PLAN_SHAPE = { "key" => "string", "name" => "string", "includedCredits" => "int", "meter" => "string",
                     "features" => "array" }.freeze
      SUBSCRIPTION_SHAPE = { "id" => "string", "customerId" => "string", "status" => "string", "plan" => "object",
                             "plan.key" => "string", "plan.name" => "string", "plan.includedCredits" => "int",
                             "plan.meter" => "string", "plan.features" => "array" }.freeze
      private_constant :PLAN_SHAPE, :SUBSCRIPTION_SHAPE

      # Creates or updates a meter: how many units of usage cost one credit.
      #
      # @param key [String]
      # @param units_per_credit [Integer]
      # @return [nil]
      def upsert_meter(key, units_per_credit)
        send_request("POST", "/v1/meters", :n, body: { "key" => key, "unitsPerCredit" => units_per_credit })
        nil
      end

      # Creates or updates a plan.
      #
      # @param key [String]
      # @param name [String]
      # @param included_credits [Integer]
      # @param meter [String]
      # @param features [Array<String>]
      # @return [Plan]
      def upsert_plan(key:, name:, included_credits:, meter:, features: [])
        body = { "key" => key, "name" => name, "includedCredits" => included_credits, "meter" => meter,
                 "features" => features }
        send_request("POST", "/v1/plans", :n, body: body, shape: PLAN_SHAPE, build: Plan.method(:from_hash)).first
      end

      # Activates a plan for the customer. The same plan again returns the existing subscription and grants nothing more;
      # another plan while one is active is an {InvalidRequestError} with code +"subscription_exists"+.
      #
      # @param customer [String]
      # @param plan [String] the plan key
      # @return [Subscription]
      def subscribe(customer, plan)
        send_request("POST", "/v1/customers/#{segment(customer, 'customer')}/subscription", :n,
                     body: { "plan" => plan }, shape: SUBSCRIPTION_SHAPE, build: Subscription.method(:from_hash)).first
      end

      # Cancels the customer's active subscription.
      #
      # @param customer [String]
      # @return [nil]
      def cancel(customer)
        send_request("DELETE", "/v1/customers/#{segment(customer, 'customer')}/subscription", :n)
        nil
      end
    end

    # Reads the project's plan and this month's usage.
    #
    # @api private
    class PlanUsageReader < Base
      SHAPE = { "plan.key" => "string", "period.start" => "string", "usage.events" => "int",
                "unconfirmedLimits.confirmed" => "bool" }.freeze
      private_constant :SHAPE

      # @return [PlanUsage]
      def read
        send_request("GET", "/v1/plan-usage", :r, shape: SHAPE, build: PlanUsage.method(:from_hash)).first
      end
    end

    # Mints short-lived read tokens for one customer, for a browser.
    class CustomerTokens < Base
      SHAPE = { "id" => "string", "token" => "string", "customerId" => "string", "expiresAt" => "string" }.freeze
      private_constant :SHAPE

      # Mints a token for ONE customer. Every call mints a new token and has no idempotency key, so after a lost answer
      # the SDK does NOT repeat it: it raises {UnavailableError} and you decide.
      #
      # @param customer [String]
      # @param ttl [Numeric, nil] seconds, 30 to 3600 (nil means 15 minutes)
      # @return [CustomerToken]
      def create(customer, ttl: nil)
        body = { "customerId" => customer }
        body["ttlSeconds"] = ttl.to_i if ttl
        send_request("POST", "/v1/customer-tokens", :x, body: body, shape: SHAPE,
                                                        build: CustomerToken.method(:from_hash)).first
      end

      # Revokes a token by its id. Idempotent.
      #
      # @param id [String] the token's +id+ (not the token)
      # @return [nil]
      def revoke(id)
        send_request("DELETE", "/v1/customer-tokens/#{segment(id, 'id')}", :n)
        nil
      end
    end
  end
end
