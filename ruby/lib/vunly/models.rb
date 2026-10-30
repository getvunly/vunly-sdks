# frozen_string_literal: true

module Vunly
  # A response field that cannot be turned into the SDK's type. The transport turns it into an +invalid_response+
  # {UnavailableError}; callers never see this class.
  #
  # @api private
  class MalformedResponse < StandardError; end

  # Readers for the API's JSON objects. Unknown fields and unknown enum values are ignored or kept as the raw string
  # (within +/v1+ the API only adds).
  #
  # @api private
  module Wire
    module_function

    def time(value)
      return nil if value.nil?
      raise MalformedResponse, "a timestamp is not a string" unless value.is_a?(String)

      Time.iso8601(value).utc
    rescue ArgumentError
      raise MalformedResponse, "a timestamp is not ISO 8601"
    end

    def int(value)
      value.is_a?(Integer) ? value : nil
    end

    def str(value)
      value.is_a?(String) ? value : nil
    end
  end

  # A customer you bill. +id+ is Vunly's own id (+cus_...+); you never have to store it: every call accepts your
  # +external_id+ instead.
  #
  # @!attribute [r] id
  #   @return [String]
  # @!attribute [r] external_id
  #   @return [String] your own id for the customer
  # @!attribute [r] name
  #   @return [String, nil]
  # @!attribute [r] created_at
  #   @return [Time]
  Customer = Data.define(:id, :external_id, :name, :created_at) do
    # @api private
    def self.from_hash(hash)
      new(id: hash["id"], external_id: hash["externalId"], name: Wire.str(hash["name"]),
          created_at: Wire.time(hash["createdAt"]))
    end
  end

  # One line of a customer's ledger. +credits+ is signed: positive for a grant, negative for a debit.
  #
  # @!attribute [r] kind
  #   @return [String] +"grant"+ or +"debit"+ (other values are kept as the raw string)
  # @!attribute [r] meter
  #   @return [String, nil] nil for a grant
  # @!attribute [r] quantity
  #   @return [Integer, nil] nil for a grant
  # @!attribute [r] reason
  #   @return [String, nil]
  # @!attribute [r] idempotency_key
  #   @return [String, nil]
  LedgerEntry = Data.define(:id, :customer_id, :kind, :credits, :meter, :quantity, :reason, :idempotency_key,
                            :created_at) do
    # @api private
    def self.from_hash(hash)
      new(id: hash["id"], customer_id: hash["customerId"], kind: hash["kind"], credits: hash["credits"],
          meter: Wire.str(hash["meter"]), quantity: Wire.int(hash["quantity"]), reason: Wire.str(hash["reason"]),
          idempotency_key: Wire.str(hash["idempotencyKey"]), created_at: Wire.time(hash["createdAt"]))
    end
  end

  # The outcome of {Resources::Usage#record}.
  #
  # @!attribute [r] event
  #   @return [LedgerEntry]
  # @!attribute [r] remaining
  #   @return [Integer] the credits left after this event
  # @!attribute [r] replayed
  #   @return [Boolean] true when this id had already been recorded: nothing was debited again. A replay is a success.
  # @!attribute [r] plan_overage
  #   @return [Boolean] true when the project is on a paid plan and this event is past the plan's included monthly
  #     volume. Paid plans keep recording; this lets you notice. Always false for a replay.
  # @!attribute [r] request_id
  #   @return [String, nil] the API's +Request-Id+
  UsageResult = Data.define(:event, :remaining, :replayed, :plan_overage, :request_id) do
    alias_method :replayed?, :replayed
    alias_method :plan_overage?, :plan_overage
  end

  # A customer's credits. +remaining = granted - consumed+ and is never negative.
  #
  # @!attribute [r] remaining
  #   @return [Integer]
  # @!attribute [r] consumed
  #   @return [Integer]
  # @!attribute [r] granted
  #   @return [Integer]
  Balance = Data.define(:remaining, :consumed, :granted) do
    # @api private
    def self.from_hash(hash)
      new(remaining: hash["credits"], consumed: hash["consumed"], granted: hash["granted"])
    end
  end

  # The answer of {Resources::Entitlements#check}. It is advisory and reserves nothing: the authoritative limit is the
  # debit made by {Resources::Usage#record}. Unless +allowed+ is true, do not run the work.
  #
  # @!attribute [r] allowed
  #   @return [Boolean]
  # @!attribute [r] reason
  #   @return [String] +"ok"+, +"no_active_plan"+, +"feature_not_in_plan"+ or +"insufficient_credits"+ (an unknown value is
  #     kept as the raw string)
  # @!attribute [r] remaining
  #   @return [Integer]
  # @!attribute [r] required
  #   @return [Integer]
  # @!attribute [r] plan
  #   @return [String, nil] nil when the customer has none
  Entitlement = Data.define(:allowed, :reason, :remaining, :required, :plan) do
    alias_method :allowed?, :allowed

    # @api private
    def self.from_hash(hash)
      new(allowed: hash["allowed"], reason: hash["reason"], remaining: hash["remaining"],
          required: hash["required"], plan: Wire.str(hash["plan"]))
    end
  end

  # A plan of the project.
  #
  # @!attribute [r] features
  #   @return [Array<String>]
  Plan = Data.define(:key, :name, :included_credits, :meter, :features, :updated_at) do
    # @api private
    def self.from_hash(hash)
      new(key: hash["key"], name: hash["name"], included_credits: hash["includedCredits"], meter: hash["meter"],
          features: Array(hash["features"]).grep(String), updated_at: Wire.time(hash["updatedAt"]))
    end
  end

  # A customer's plan.
  #
  # @!attribute [r] plan
  #   @return [Plan]
  # @!attribute [r] status
  #   @return [String] +"active"+ or +"canceled"+ (other values are kept as the raw string)
  Subscription = Data.define(:id, :customer_id, :plan, :status, :period_start, :period_end, :created_at) do
    # @api private
    def self.from_hash(hash)
      new(id: hash["id"], customer_id: hash["customerId"], plan: Plan.from_hash(hash["plan"]), status: hash["status"],
          period_start: Wire.time(hash["periodStart"]), period_end: Wire.time(hash["periodEnd"]),
          created_at: Wire.time(hash["createdAt"]))
    end
  end

  # A short-lived read token for ONE customer, for the usage widget in a browser. The API shows it once. A browser may
  # hold it, never an API key.
  #
  # The token is a secret: +inspect+, +to_s+, +pp+, +to_h+, +to_json+, +as_json+ and +Marshal.dump+ never include it.
  # Read it on purpose with {#token}.
  class CustomerToken
    include Undumpable

    # @return [String]
    attr_reader :id
    # @return [String]
    attr_reader :customer_id
    # @return [Time]
    attr_reader :expires_at

    # @api private
    def self.from_hash(hash)
      new(id: hash["id"], token: hash["token"], customer_id: hash["customerId"], expires_at: Wire.time(hash["expiresAt"]))
    end

    # @param id [String]
    # @param token [String]
    # @param customer_id [String]
    # @param expires_at [Time]
    def initialize(id:, token:, customer_id:, expires_at:)
      @id = id
      @secret = Secret.new(token)
      @customer_id = customer_id
      @expires_at = expires_at
    end

    # @return [String] the token itself. Hand it to the browser; do not log it.
    def token
      @secret.reveal
    end

    # @return [Hash] the fields except the token
    def to_h
      { id: id, customer_id: customer_id, expires_at: expires_at }
    end

    # @return [Hash]
    def as_json(*)
      { "id" => id, "customerId" => customer_id, "expiresAt" => expires_at&.iso8601(3) }
    end

    # @return [String]
    def to_json(*)
      as_json.to_json(*)
    end

    # @return [String]
    def inspect
      "#<Vunly::CustomerToken id=#{id.inspect} customer_id=#{customer_id.inspect} expires_at=#{expires_at.inspect} " \
        "token=#{Secret::HIDDEN}>"
    end
    alias to_s inspect

    # @param queue [PP] the pretty printer
    def pretty_print(queue)
      queue.text(inspect)
    end
  end

  # The plan a project is on.
  #
  # @!attribute [r] key
  #   @return [String]
  # @!attribute [r] name
  #   @return [String, nil]
  PlanRef = Data.define(:key, :name)

  # The project's plan and this month's recorded usage.
  #
  # +unconfirmed_limits+ is a working hypothesis, not approved and not a guarantee: it carries +confirmed == false+.
  # Do not show it to your customers as a quota, a price or a commitment.
  class PlanUsage
    # The billing month.
    Period = Data.define(:start, :end)
    # What was recorded in it.
    Usage = Data.define(:events)
    # The limits the project is currently working with; +confirmed+ is false.
    UnconfirmedLimits = Data.define(:confirmed, :note, :included_monthly_events, :activity_retention_days)

    # @return [PlanRef]
    attr_reader :plan
    # @return [Period]
    attr_reader :period
    # @return [Usage]
    attr_reader :usage
    # @return [UnconfirmedLimits]
    attr_reader :unconfirmed_limits

    # @api private
    def self.from_hash(hash)
      limits = hash["unconfirmedLimits"]
      new(
        plan: PlanRef.new(key: hash.dig("plan", "key"), name: Wire.str(hash.dig("plan", "name"))),
        period: Period.new(start: Wire.time(hash.dig("period", "start")), end: Wire.time(hash.dig("period", "end"))),
        usage: Usage.new(events: hash.dig("usage", "events")),
        unconfirmed_limits: UnconfirmedLimits.new(
          confirmed: limits["confirmed"], note: Wire.str(limits["note"]),
          included_monthly_events: Wire.int(limits["includedMonthlyEvents"]),
          activity_retention_days: Wire.int(limits["activityRetentionDays"])
        )
      )
    end

    # @api private
    def initialize(plan:, period:, usage:, unconfirmed_limits:)
      @plan = plan
      @period = period
      @usage = usage
      @unconfirmed_limits = unconfirmed_limits
    end

    # @return [Hash]
    def to_h
      { plan: plan, period: period, usage: usage, unconfirmed_limits: unconfirmed_limits }
    end
  end

  # A verified webhook delivery.
  #
  # @!attribute [r] data
  #   @return [Object] the parsed +data+ of the event
  WebhookEvent = Data.define(:id, :type, :created_at, :data)

  # One page of a list. +next_cursor+ is opaque: pass it back as +cursor:+. It is nil on the last page.
  #
  # @!attribute [r] data
  #   @return [Array] the items
  # @!attribute [r] next_cursor
  #   @return [String, nil]
  Page = Data.define(:data, :next_cursor) do
    # @return [Boolean] whether another page follows
    def more?
      !next_cursor.nil?
    end

    # Yields the items of this page.
    def each(&)
      data.each(&)
    end
  end
end
