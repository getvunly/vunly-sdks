//! The values the API returns and the parameters of the calls that take more than one or two arguments.
//!
//! Timestamps are kept as the RFC 3339 strings the API sent (the crate has no date dependency). Credits are whole
//! numbers (`i64`) exactly as the API reports them: the SDK never computes, rounds or guesses them.

use std::fmt;

use serde_json::Value;

use crate::json::Read;

/// A customer you bill. `id` is Vunly's own id (`cus_...`); you never have to store it: every call accepts your own
/// `external_id` instead.
#[derive(Debug, Clone, PartialEq)]
pub struct Customer {
    /// Vunly's id.
    pub id: String,
    /// Your id for the customer.
    pub external_id: String,
    /// Shown in the console; `None` when none was given.
    pub name: Option<String>,
    /// RFC 3339.
    pub created_at: String,
}

impl Customer {
    pub(crate) fn from_json(v: &Value) -> Customer {
        Customer {
            id: v.string("id"),
            external_id: v.string("externalId"),
            name: v.opt_string("name"),
            created_at: v.string("createdAt"),
        }
    }
}

/// One line of a customer's ledger. `credits` is signed: positive for a grant, negative for a debit.
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerEntry {
    /// The entry's id.
    pub id: String,
    /// Vunly's customer id.
    pub customer_id: String,
    /// `"grant"` or `"debit"` (an unknown kind is kept as the raw string).
    pub kind: String,
    /// Signed credits.
    pub credits: i64,
    /// The meter of a debit; `None` for a grant.
    pub meter: Option<String>,
    /// The quantity of a debit, in the meter's units; `None` for a grant.
    pub quantity: Option<i64>,
    /// The reason given with a grant, if any.
    pub reason: Option<String>,
    /// The idempotency key the entry was created under, if any.
    pub idempotency_key: Option<String>,
    /// RFC 3339.
    pub created_at: String,
}

impl LedgerEntry {
    pub(crate) fn from_json(v: &Value) -> LedgerEntry {
        LedgerEntry {
            id: v.string("id"),
            customer_id: v.string("customerId"),
            kind: v.string("kind"),
            credits: v.int("credits"),
            meter: v.opt_string("meter"),
            quantity: v.opt_int("quantity"),
            reason: v.opt_string("reason"),
            idempotency_key: v.opt_string("idempotencyKey"),
            created_at: v.string("createdAt"),
        }
    }
}

/// The outcome of [`Usage::record`](crate::Usage::record).
#[derive(Debug, Clone, PartialEq)]
pub struct UsageResult {
    /// The ledger entry.
    pub event: LedgerEntry,
    /// The credits left after this event.
    pub remaining: i64,
    /// `true` when this id had already been recorded: nothing was debited again. A replay is a success.
    pub replayed: bool,
    /// `true` when the project is on a paid plan and this event is past the plan's included monthly volume. Paid plans
    /// keep recording; this lets you notice. Always `false` for a replay.
    pub plan_overage: bool,
    /// The API's `Request-Id`.
    pub request_id: Option<String>,
}

/// A customer's credits. `remaining = granted - consumed` and is never negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Balance {
    /// Credits left.
    pub remaining: i64,
    /// Credits spent.
    pub consumed: i64,
    /// Credits granted.
    pub granted: i64,
}

/// The usage a check should price in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Estimate {
    /// The meter's key.
    pub meter: String,
    /// In the meter's own units (for example tokens), not credits.
    pub quantity: i64,
}

impl Estimate {
    /// An estimate of `quantity` units of `meter`.
    pub fn new(meter: impl Into<String>, quantity: i64) -> Estimate {
        Estimate {
            meter: meter.into(),
            quantity,
        }
    }
}

/// The answer of [`Entitlements::check`](crate::Entitlements::check). It is advisory and reserves nothing: the
/// authoritative limit is the debit made by [`Usage::record`](crate::Usage::record). Unless `allowed` is true, do not
/// run the work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entitlement {
    /// May the work run now?
    pub allowed: bool,
    /// `"ok"`, `"no_active_plan"`, `"feature_not_in_plan"` or `"insufficient_credits"` (others are kept as sent).
    pub reason: String,
    /// Credits left.
    pub remaining: i64,
    /// Credits the estimate needs.
    pub required: i64,
    /// The customer's plan key, `None` when it has none.
    pub plan: Option<String>,
}

impl Entitlement {
    pub(crate) fn from_json(v: &Value) -> Entitlement {
        Entitlement {
            allowed: v.boolean("allowed"),
            reason: v.string("reason"),
            remaining: v.int("remaining"),
            required: v.int("required"),
            plan: v.opt_string("plan"),
        }
    }
}

/// A plan of the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The plan's key.
    pub key: String,
    /// Its display name.
    pub name: String,
    /// Credits granted per period.
    pub included_credits: i64,
    /// The meter the plan uses.
    pub meter: String,
    /// Feature keys the plan includes.
    pub features: Vec<String>,
    /// RFC 3339, when the API sent it.
    pub updated_at: Option<String>,
}

impl Plan {
    pub(crate) fn from_json(v: &Value) -> Plan {
        Plan {
            key: v.string("key"),
            name: v.string("name"),
            included_credits: v.int("includedCredits"),
            meter: v.string("meter"),
            features: v.strings("features"),
            updated_at: v.opt_string("updatedAt"),
        }
    }
}

/// A customer's plan. `status` is `"active"` or `"canceled"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscription {
    /// The subscription's id.
    pub id: String,
    /// Vunly's customer id.
    pub customer_id: String,
    /// The plan.
    pub plan: Plan,
    /// `"active"` or `"canceled"` (others are kept as sent).
    pub status: String,
    /// RFC 3339, when the API sent it.
    pub period_start: Option<String>,
    /// RFC 3339, when the API sent it.
    pub period_end: Option<String>,
    /// RFC 3339, when the API sent it.
    pub created_at: Option<String>,
}

impl Subscription {
    pub(crate) fn from_json(v: &Value) -> Subscription {
        Subscription {
            id: v.string("id"),
            customer_id: v.string("customerId"),
            plan: Plan::from_json(v.child("plan")),
            status: v.string("status"),
            period_start: v.opt_string("periodStart"),
            period_end: v.opt_string("periodEnd"),
            created_at: v.opt_string("createdAt"),
        }
    }
}

/// A short-lived read token for ONE customer, for the usage widget in a browser. It is shown once; a browser may hold
/// it, never an API key. `Debug` shows only its id; the secret is read with [`CustomerToken::token`].
#[derive(Clone, PartialEq, Eq)]
pub struct CustomerToken {
    /// The token's id (use it to revoke the token).
    pub id: String,
    token: String,
    /// Vunly's customer id.
    pub customer_id: String,
    /// RFC 3339.
    pub expires_at: String,
}

impl CustomerToken {
    pub(crate) fn from_json(v: &Value) -> CustomerToken {
        CustomerToken {
            id: v.string("id"),
            token: v.string("token"),
            customer_id: v.string("customerId"),
            expires_at: v.string("expiresAt"),
        }
    }

    /// The token itself. Hand it to the browser; never log it.
    pub fn token(&self) -> &str {
        &self.token
    }
}

impl fmt::Debug for CustomerToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CustomerToken")
            .field("id", &self.id)
            .field("customer_id", &self.customer_id)
            .field("expires_at", &self.expires_at)
            .field("token", &"<hidden>")
            .finish()
    }
}

/// The project's plan reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRef {
    /// The plan's key.
    pub key: String,
    /// Its display name.
    pub name: String,
}

/// The billing period of [`PlanUsage`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Period {
    /// RFC 3339.
    pub start: String,
    /// RFC 3339, when the API sent it.
    pub end: Option<String>,
}

/// Limits the API reports as a working hypothesis: **not approved and not a guarantee** (`confirmed` is `false`). Do
/// not show them to your customers as a quota, a price or a commitment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnconfirmedLimits {
    /// Always `false` today: these numbers are not a commitment.
    pub confirmed: bool,
    /// The API's own caveat.
    pub note: String,
    /// The hypothesised monthly event volume, if any.
    pub included_monthly_events: Option<i64>,
    /// The hypothesised activity retention, if any.
    pub activity_retention_days: Option<i64>,
}

/// The project's plan and this month's recorded usage, with its limits kept inside [`UnconfirmedLimits`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanUsage {
    /// The project's plan.
    pub plan: PlanRef,
    /// The current period.
    pub period: Period,
    /// Events recorded this period.
    pub events: i64,
    /// The limits the API reports, flagged `confirmed: false`. Not a guarantee.
    pub unconfirmed_limits: UnconfirmedLimits,
}

impl PlanUsage {
    pub(crate) fn from_json(v: &Value) -> PlanUsage {
        let plan = v.child("plan");
        let period = v.child("period");
        let limits = v.child("unconfirmedLimits");
        PlanUsage {
            plan: PlanRef {
                key: plan.string("key"),
                name: plan.string("name"),
            },
            period: Period {
                start: period.string("start"),
                end: period.opt_string("end"),
            },
            events: v.child("usage").int("events"),
            unconfirmed_limits: UnconfirmedLimits {
                confirmed: limits.boolean("confirmed"),
                note: limits.string("note"),
                included_monthly_events: limits.opt_int("includedMonthlyEvents"),
                activity_retention_days: limits.opt_int("activityRetentionDays"),
            },
        }
    }
}

/// One page of a list. `next_cursor` is opaque: pass it back as [`ListOptions::cursor`]. It is `None` on the last page.
#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    /// The items.
    pub data: Vec<T>,
    /// Opaque; never parse or build one.
    pub next_cursor: Option<String>,
}

impl<T> Page<T> {
    /// Does another page follow?
    pub fn has_more(&self) -> bool {
        self.next_cursor.is_some()
    }
}

/// Pages a list. `limit` is 1 to 100 (`None` means the API's default).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListOptions {
    /// Page size, 1 to 100.
    pub limit: Option<u32>,
    /// The `next_cursor` of the previous page.
    pub cursor: Option<String>,
}

/// The fields of [`Credits::grant`](crate::Credits::grant).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantParams {
    /// The idempotency key: 8 to 128 characters of `A-Za-z0-9_.:-`. The same id grants once.
    pub id: String,
    /// Your id or Vunly's.
    pub customer: String,
    /// Whole credits.
    pub credits: i64,
    /// Optional, shown in the ledger.
    pub reason: Option<String>,
}

impl GrantParams {
    /// A grant of `credits` to `customer`, applied at most once per `id`.
    pub fn new(id: impl Into<String>, customer: impl Into<String>, credits: i64) -> GrantParams {
        GrantParams {
            id: id.into(),
            customer: customer.into(),
            credits,
            reason: None,
        }
    }

    /// Adds a reason.
    pub fn reason(mut self, reason: impl Into<String>) -> GrantParams {
        self.reason = Some(reason.into());
        self
    }
}

/// The fields of [`Usage::record`](crate::Usage::record). The id is required and is never generated for you.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordParams {
    /// Names the unit of work (your request or job id) and is sent as the `Idempotency-Key`: 8 to 128 characters of
    /// `A-Za-z0-9_.:-`. Retry with the SAME id; the API applies it at most once.
    pub id: String,
    /// Your id or Vunly's.
    pub customer: String,
    /// The meter's key.
    pub meter: String,
    /// In the meter's own units (for example tokens), not credits: the meter converts it, rounded up per request.
    pub quantity: i64,
}

impl RecordParams {
    /// Usage of `quantity` units of `meter` by `customer`, recorded at most once per `id`.
    pub fn new(
        id: impl Into<String>,
        customer: impl Into<String>,
        meter: impl Into<String>,
        quantity: i64,
    ) -> RecordParams {
        RecordParams {
            id: id.into(),
            customer: customer.into(),
            meter: meter.into(),
            quantity,
        }
    }
}

/// The fields of [`Entitlements::check`](crate::Entitlements::check).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckParams {
    /// Your id or Vunly's.
    pub customer: String,
    /// The feature key.
    pub feature: String,
    /// Optional: the usage the work is expected to need.
    pub estimate: Option<Estimate>,
}

impl CheckParams {
    /// Asks whether `customer` may use `feature`.
    pub fn new(customer: impl Into<String>, feature: impl Into<String>) -> CheckParams {
        CheckParams {
            customer: customer.into(),
            feature: feature.into(),
            estimate: None,
        }
    }

    /// Prices an estimate in.
    pub fn estimate(mut self, estimate: Estimate) -> CheckParams {
        self.estimate = Some(estimate);
        self
    }
}

/// The fields of [`Plans::upsert_plan`](crate::Plans::upsert_plan).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanParams {
    /// The plan's key.
    pub key: String,
    /// Display name.
    pub name: String,
    /// Credits granted per period.
    pub included_credits: i64,
    /// The meter the plan uses.
    pub meter: String,
    /// Feature keys the plan includes.
    pub features: Vec<String>,
}
