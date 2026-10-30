//! The calls, grouped as the OpenAPI tags group them. Each group is a cheap borrow of a [`Client`].

use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::client::{Class, Client, Request};
use crate::error::{ApiError, Error};
use crate::json::{escape, Kind, Read, Shape};
use crate::types::{
    Balance, CheckParams, Customer, CustomerToken, Entitlement, GrantParams, LedgerEntry, ListOptions, Page, Plan,
    PlanParams, PlanUsage, RecordParams, Subscription, UsageResult,
};

const CUSTOMER: Shape = &[("id", Kind::Str), ("externalId", Kind::Str), ("createdAt", Kind::Str)];
const ENTRY: Shape = &[
    ("id", Kind::Str),
    ("customerId", Kind::Str),
    ("kind", Kind::Str),
    ("credits", Kind::Int),
    ("createdAt", Kind::Str),
];
const LIST: Shape = &[("data", Kind::Array), ("nextCursor", Kind::StrOpt)];

/// One URL path segment for a caller-supplied id: percent-encoded, and refused when empty (`/customers//balance` is not
/// the customer the caller meant). `.` and `..` are refused too: the API normalises `%2E` back to a dot segment and
/// answered for another customer, so they never reach the wire.
fn seg(value: &str, field: &str) -> Result<String, Error> {
    let problem = if value.is_empty() {
        Some("must not be empty")
    } else if value == "." || value == ".." {
        Some("must not be '.' or '..'")
    } else {
        None
    };
    if let Some(problem) = problem {
        return Err(Error::from_status(ApiError {
            code: "validation_error".into(),
            status: 400,
            message: format!("{field} {problem}"),
            request_id: None,
            details: Some(json!({"fields": {field: problem}})),
            retry_after: None,
        }));
    }
    Ok(escape(value))
}

fn page_query(o: &ListOptions) -> Vec<(&'static str, String)> {
    let mut q = Vec::new();
    if let Some(limit) = o.limit {
        q.push(("limit", limit.to_string()));
    }
    if let Some(cursor) = &o.cursor {
        q.push(("cursor", cursor.clone()));
    }
    q
}

fn page_of<T>(body: &Value, item: fn(&Value) -> T) -> Page<T> {
    let data = body
        .get("data")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(item).collect())
        .unwrap_or_default();
    Page {
        data,
        next_cursor: body.opt_string("nextCursor").filter(|c| !c.is_empty()),
    }
}

impl Client {
    /// Creates and reads the customers you bill.
    pub fn customers(&self) -> Customers<'_> {
        Customers { c: self }
    }
    /// Grants credits and reads balances and the ledger.
    pub fn credits(&self) -> Credits<'_> {
        Credits { c: self }
    }
    /// Records usage events.
    pub fn usage(&self) -> Usage<'_> {
        Usage { c: self }
    }
    /// Asks whether a customer may do something now.
    pub fn entitlements(&self) -> Entitlements<'_> {
        Entitlements { c: self }
    }
    /// Manages meters, plans and subscriptions.
    pub fn plans(&self) -> Plans<'_> {
        Plans { c: self }
    }
    /// Mints short-lived read tokens for one customer, for a browser.
    pub fn customer_tokens(&self) -> CustomerTokens<'_> {
        CustomerTokens { c: self }
    }

    /// The project's plan and this month's recorded usage. The limits are inside
    /// [`PlanUsage::unconfirmed_limits`] and are **not a guarantee** (`confirmed` is `false`).
    pub fn plan_usage(&self) -> Result<PlanUsage, Error> {
        const SHAPE: Shape = &[
            ("plan.key", Kind::Str),
            ("period.start", Kind::Str),
            ("usage.events", Kind::Int),
            ("unconfirmedLimits.confirmed", Kind::Bool),
        ];
        let r = self.send(&Request::new("GET", "/v1/plan-usage".into(), Class::R).shape(SHAPE))?;
        Ok(PlanUsage::from_json(&r.body))
    }
}

// ---- pagination ----------------------------------------------------------------------------------------------------

type Fetch<'a, T> = Box<dyn FnMut(Option<String>) -> Result<Page<T>, Error> + Send + 'a>;

/// An iterator over every item of a list. It fetches one page at a time and only when you ask for the next item
/// (never ahead), and ends after the first error.
pub struct PageIter<'a, T> {
    fetch: Fetch<'a, T>,
    buffer: VecDeque<T>,
    cursor: Option<String>,
    started: bool,
    done: bool,
    /// Every cursor handed out so far: a repeat means the server is looping, and the iterator must not follow it.
    seen: HashSet<String>,
    looped: bool,
    empty_streak: u32,
}

/// Consecutive empty pages that each promise another one before an iterator gives up.
const MAX_EMPTY_PAGES: u32 = 3;

impl<'a, T> PageIter<'a, T> {
    fn new(fetch: impl FnMut(Option<String>) -> Result<Page<T>, Error> + Send + 'a) -> Self {
        PageIter {
            fetch: Box::new(fetch),
            buffer: VecDeque::new(),
            cursor: None,
            started: false,
            done: false,
            seen: HashSet::new(),
            looped: false,
            empty_streak: 0,
        }
    }
}

impl<T> Iterator for PageIter<'_, T> {
    type Item = Result<T, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(item) = self.buffer.pop_front() {
                return Some(Ok(item));
            }
            if std::mem::take(&mut self.looped) {
                return Some(Err(Error::invalid_response(
                    200,
                    None,
                    "the list does not end (a repeated cursor, or empty pages that each promise another)",
                )));
            }
            if self.done || (self.started && self.cursor.is_none()) {
                return None;
            }
            self.started = true;
            match (self.fetch)(self.cursor.take()) {
                Ok(page) => {
                    // Pages that are empty yet promise another one, three in a row, are a server that will never finish
                    // (a fresh cursor each time is not a repeat): stop with the same error. A page with data resets it.
                    if page.data.is_empty() && page.next_cursor.is_some() {
                        self.empty_streak += 1;
                    } else {
                        self.empty_streak = 0;
                    }
                    self.buffer = page.data.into();
                    // a cursor seen before never advances: this page's items are delivered, then the loop is
                    // reported instead of followed (the iterator would otherwise never end)
                    let repeated = page.next_cursor.as_ref().is_some_and(|c| !self.seen.insert(c.clone()));
                    if repeated || self.empty_streak >= MAX_EMPTY_PAGES {
                        self.done = true;
                        self.looped = true;
                    } else {
                        self.cursor = page.next_cursor;
                    }
                }
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            }
        }
    }
}

// ---- customers -----------------------------------------------------------------------------------------------------

/// Creates and reads the customers you bill. Get it with [`Client::customers`].
pub struct Customers<'a> {
    c: &'a Client,
}

impl<'a> Customers<'a> {
    /// Creates the customer or returns the existing one for this id (your own id). Asking twice leaves one customer.
    /// `name` is shown in the console.
    pub fn upsert(&self, id: &str, name: Option<&str>) -> Result<Customer, Error> {
        let mut body = Map::new();
        body.insert("externalId".into(), json!(id));
        if let Some(name) = name.filter(|n| !n.is_empty()) {
            body.insert("name".into(), json!(name));
        }
        let req = Request::new("POST", "/v1/customers".into(), Class::N)
            .body(Value::Object(body))
            .shape(CUSTOMER);
        Ok(Customer::from_json(&self.c.send(&req)?.body))
    }

    /// Returns one customer, by your id or Vunly's. A customer that does not exist is an
    /// [`Error::InvalidRequest`] with code `"not_found"`.
    pub fn get(&self, customer: &str) -> Result<Customer, Error> {
        let req =
            Request::new("GET", format!("/v1/customers/{}", seg(customer, "customer")?), Class::R).shape(CUSTOMER);
        Ok(Customer::from_json(&self.c.send(&req)?.body))
    }

    /// One page of customers, newest first.
    pub fn list(&self, options: &ListOptions) -> Result<Page<Customer>, Error> {
        let req = Request::new("GET", "/v1/customers".into(), Class::R)
            .query(page_query(options))
            .shape(LIST);
        Ok(page_of(&self.c.send(&req)?.body, Customer::from_json))
    }

    /// Every customer, one page at a time and only as you consume them. `limit` is the page size (1 to 100).
    pub fn iter(&self, limit: Option<u32>) -> PageIter<'a, Customer> {
        let c = self.c;
        PageIter::new(move |cursor| c.customers().list(&ListOptions { limit, cursor }))
    }
}

// ---- credits -------------------------------------------------------------------------------------------------------

/// Grants credits and reads balances and the ledger. Get it with [`Client::credits`].
pub struct Credits<'a> {
    c: &'a Client,
}

impl<'a> Credits<'a> {
    /// Adds credits (a sandbox top-up). Repeating the same id grants once and returns the original entry.
    pub fn grant(&self, p: GrantParams) -> Result<LedgerEntry, Error> {
        let mut body = Map::new();
        body.insert("credits".into(), json!(p.credits));
        if let Some(reason) = p.reason.as_deref().filter(|r| !r.is_empty()) {
            body.insert("reason".into(), json!(reason));
        }
        let req = Request::new(
            "POST",
            format!("/v1/customers/{}/credit-grants", seg(&p.customer, "customer")?),
            Class::K,
        )
        .body(Value::Object(body))
        .idem(&p.id)
        .shape(ENTRY);
        Ok(LedgerEntry::from_json(&self.c.send(&req)?.body))
    }

    /// A customer's credits.
    pub fn balance(&self, customer: &str) -> Result<Balance, Error> {
        const SHAPE: Shape = &[("credits", Kind::Int), ("consumed", Kind::Int), ("granted", Kind::Int)];
        let req = Request::new(
            "GET",
            format!("/v1/customers/{}/balance", seg(customer, "customer")?),
            Class::R,
        )
        .shape(SHAPE);
        let r = self.c.send(&req)?;
        Ok(Balance {
            remaining: r.body.int("credits"),
            consumed: r.body.int("consumed"),
            granted: r.body.int("granted"),
        })
    }

    /// One page of a customer's ledger entries, newest first.
    pub fn ledger(&self, customer: &str, options: &ListOptions) -> Result<Page<LedgerEntry>, Error> {
        let req = Request::new(
            "GET",
            format!("/v1/customers/{}/ledger", seg(customer, "customer")?),
            Class::R,
        )
        .query(page_query(options))
        .shape(LIST);
        Ok(page_of(&self.c.send(&req)?.body, LedgerEntry::from_json))
    }

    /// A customer's whole ledger, one page at a time, only as you consume it.
    pub fn iter_ledger(&self, customer: &str, limit: Option<u32>) -> PageIter<'a, LedgerEntry> {
        let c = self.c;
        let customer = customer.to_owned();
        PageIter::new(move |cursor| c.credits().ledger(&customer, &ListOptions { limit, cursor }))
    }
}

// ---- usage ---------------------------------------------------------------------------------------------------------

/// Records usage events. Get it with [`Client::usage`].
pub struct Usage<'a> {
    c: &'a Client,
}

impl Usage<'_> {
    /// Records usage once and debits credits. It is final: [`Error::LimitReached`] means the credits were not there when
    /// you recorded, even if a check said yes earlier; nothing was debited. A repeat of the same id returns the
    /// original result with `replayed == true`.
    pub fn record(&self, p: RecordParams) -> Result<UsageResult, Error> {
        const SHAPE: Shape = &[
            ("balance", Kind::Int),
            ("event.id", Kind::Str),
            ("event.customerId", Kind::Str),
            ("event.kind", Kind::Str),
            ("event.credits", Kind::Int),
            ("event.createdAt", Kind::Str),
        ];
        let req = Request::new("POST", "/v1/usage-events".into(), Class::K)
            .body(json!({"customerId": p.customer, "meter": p.meter, "quantity": p.quantity}))
            .idem(&p.id)
            .shape(SHAPE);
        let r = self.c.send(&req)?;
        Ok(UsageResult {
            event: LedgerEntry::from_json(r.body.child("event")),
            remaining: r.body.int("balance"),
            replayed: r.replayed,
            plan_overage: r.plan_overage && !r.replayed,
            request_id: r.request_id,
        })
    }
}

// ---- entitlements --------------------------------------------------------------------------------------------------

/// Asks whether a customer may do something now. Get it with [`Client::entitlements`].
pub struct Entitlements<'a> {
    c: &'a Client,
}

impl Entitlements<'_> {
    /// Read-only and advisory. If it returns an error (an [`Error::Unavailable`] after retries), treat that as "no":
    /// fail closed, do not run the work. It never returns `allowed: true` by itself.
    pub fn check(&self, p: CheckParams) -> Result<Entitlement, Error> {
        const SHAPE: Shape = &[
            ("allowed", Kind::Bool),
            ("reason", Kind::Str),
            ("remaining", Kind::Int),
            ("required", Kind::Int),
            ("plan", Kind::StrOpt),
        ];
        let mut body = Map::new();
        body.insert("customerId".into(), json!(p.customer));
        body.insert("feature".into(), json!(p.feature));
        if let Some(e) = &p.estimate {
            body.insert("estimate".into(), json!({"meter": e.meter, "quantity": e.quantity}));
        }
        let req = Request::new("POST", "/v1/entitlements/check".into(), Class::R)
            .body(Value::Object(body))
            .shape(SHAPE);
        Ok(Entitlement::from_json(&self.c.send(&req)?.body))
    }
}

// ---- plans ---------------------------------------------------------------------------------------------------------

/// Manages meters, plans and subscriptions. Get it with [`Client::plans`].
pub struct Plans<'a> {
    c: &'a Client,
}

impl Plans<'_> {
    /// Creates or updates a meter: how many units of usage cost one credit.
    pub fn upsert_meter(&self, key: &str, units_per_credit: i64) -> Result<(), Error> {
        let req = Request::new("POST", "/v1/meters".into(), Class::N)
            .body(json!({"key": key, "unitsPerCredit": units_per_credit}));
        self.c.send(&req).map(|_| ())
    }

    /// Creates or updates a plan.
    pub fn upsert_plan(&self, p: PlanParams) -> Result<Plan, Error> {
        const SHAPE: Shape = &[
            ("key", Kind::Str),
            ("name", Kind::Str),
            ("includedCredits", Kind::Int),
            ("meter", Kind::Str),
            ("features", Kind::Array),
        ];
        let req = Request::new("POST", "/v1/plans".into(), Class::N)
            .body(json!({
                "key": p.key, "name": p.name, "includedCredits": p.included_credits,
                "meter": p.meter, "features": p.features,
            }))
            .shape(SHAPE);
        Ok(Plan::from_json(&self.c.send(&req)?.body))
    }

    /// Activates a plan for the customer. The same plan again returns the existing subscription and grants nothing more;
    /// another plan while one is active is an [`Error::InvalidRequest`] with code `"subscription_exists"`.
    pub fn subscribe(&self, customer: &str, plan: &str) -> Result<Subscription, Error> {
        const SHAPE: Shape = &[
            ("id", Kind::Str),
            ("customerId", Kind::Str),
            ("status", Kind::Str),
            ("plan.key", Kind::Str),
        ];
        let req = Request::new(
            "POST",
            format!("/v1/customers/{}/subscription", seg(customer, "customer")?),
            Class::N,
        )
        .body(json!({"plan": plan}))
        .shape(SHAPE);
        Ok(Subscription::from_json(&self.c.send(&req)?.body))
    }

    /// Cancels the customer's active subscription.
    pub fn cancel(&self, customer: &str) -> Result<(), Error> {
        let req = Request::new(
            "DELETE",
            format!("/v1/customers/{}/subscription", seg(customer, "customer")?),
            Class::N,
        );
        self.c.send(&req).map(|_| ())
    }
}

// ---- customer tokens -----------------------------------------------------------------------------------------------

/// Mints short-lived read tokens for one customer, for a browser. Get it with [`Client::customer_tokens`].
pub struct CustomerTokens<'a> {
    c: &'a Client,
}

impl CustomerTokens<'_> {
    /// Mints a token for ONE customer. `ttl` is 30 seconds to 1 hour (`None` means 15 minutes). Every call mints a new
    /// token and has no idempotency key, so after a lost answer the SDK does NOT repeat it: it returns an
    /// [`Error::Unavailable`] and you decide (it may exist; revoke it by listing, or let it expire).
    pub fn create(&self, customer: &str, ttl: Option<Duration>) -> Result<CustomerToken, Error> {
        const SHAPE: Shape = &[
            ("id", Kind::Str),
            ("token", Kind::Str),
            ("customerId", Kind::Str),
            ("expiresAt", Kind::Str),
        ];
        seg(customer, "customer")?; // not a path segment here, but an empty id is refused before any request all the same
        let mut body = Map::new();
        body.insert("customerId".into(), json!(customer));
        if let Some(ttl) = ttl.filter(|t| !t.is_zero()) {
            body.insert("ttlSeconds".into(), json!(ttl.as_secs()));
        }
        let req = Request::new("POST", "/v1/customer-tokens".into(), Class::X)
            .body(Value::Object(body))
            .shape(SHAPE);
        Ok(CustomerToken::from_json(&self.c.send(&req)?.body))
    }

    /// Revokes a token by its id. Idempotent.
    pub fn revoke(&self, id: &str) -> Result<(), Error> {
        let req = Request::new("DELETE", format!("/v1/customer-tokens/{}", seg(id, "id")?), Class::N);
        self.c.send(&req).map(|_| ())
    }
}
