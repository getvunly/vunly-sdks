//! # vunly
//!
//! Rust client for the Vunly API: customers, credits, usage, entitlement checks, plans, customer tokens and webhook
//! verification. Sandbox only for now. **Blocking** (no async runtime), `Send + Sync`, no `unsafe`.
//!
//! The loop every integration is built from:
//!
//! ```no_run
//! use vunly::{CheckParams, Client, Estimate, RecordParams};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // The key comes from the environment. Never write it into source, tests, logs or prompts.
//! let client = Client::new(std::env::var("VUNLY_API_KEY")?, std::env::var("VUNLY_API_URL")?)?;
//!
//! let decision = client
//!     .entitlements()
//!     .check(CheckParams::new("cus_demo_01", "generate").estimate(Estimate::new("tokens", 1050)));
//! if !matches!(decision, Ok(d) if d.allowed) {
//!     return Ok(()); // fail closed: an error is a "no"
//! }
//!
//! // ... do the work, then record it once. The id names this unit of work; retries use the SAME id.
//! match client.usage().record(RecordParams::new("req_2026_0001", "cus_demo_01", "tokens", 1050)) {
//!     Ok(res) => println!("{} credits left, replayed: {}", res.remaining, res.replayed),
//!     Err(vunly::Error::LimitReached(_)) => { /* 402: nothing was debited */ }
//!     Err(e) => return Err(e.into()), // Unavailable after the SDK's own retries: fail closed
//! }
//! # Ok(()) }
//! ```
//!
//! ## Using it from async code
//!
//! The client blocks the calling thread. From an async runtime call it where blocking is allowed, for example with
//! Tokio's `spawn_blocking`; the client is cheap to clone (clones share one connection pool):
//!
//! ```ignore
//! let client = client.clone();
//! let res = tokio::task::spawn_blocking(move || client.usage().record(params)).await??;
//! ```
//!
//! ## Rules the SDK keeps
//!
//! * One attempt is bounded as a whole (connect, headers and body) by [`ClientBuilder::timeout`].
//! * Retries follow the operation class: reads, the entitlement check, keyed writes and upserts are repeated (keyed
//!   writes with the SAME id) after a lost answer, a timeout, a 429 or a 502/503/504/500; mutations without a key
//!   ([`CustomerTokens::create`]) only after a 429. The plan-volume stop is never repeated.
//! * Redirects are never followed; the key goes only to the configured host.
//! * A response body is read up to [`MAX_BODY_BYTES`] (16 MiB).
//! * The key never appears in an error, a `Debug` or a `Display`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod client;
mod error;
mod json;
mod services;
mod types;
mod webhooks;

pub use client::{CancelToken, Client, ClientBuilder, MAX_BODY_BYTES, MAX_RETRY_AFTER};
pub use error::{ApiError, ConfigError, Error, ErrorClass, WebhookError};
pub use services::{Credits, CustomerTokens, Customers, Entitlements, PageIter, Plans, Usage};
pub use types::{
    Balance, CheckParams, Customer, CustomerToken, Entitlement, Estimate, GrantParams, LedgerEntry, ListOptions, Page,
    Period, Plan, PlanParams, PlanRef, PlanUsage, RecordParams, Subscription, UnconfirmedLimits, UsageResult,
};
pub use webhooks::{verify_webhook, verify_webhook_at, WebhookEvent, WEBHOOK_TOLERANCE};

/// Re-exported because [`ApiError::details`] and [`WebhookEvent::data`] are `serde_json` values.
pub use serde_json;

/// This SDK's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The API major this SDK speaks.
pub const API_VERSION: &str = "v1";

/// The README's code blocks are compiled as doc-tests, so what it shows always builds.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Client>();
    assert_send_sync::<Error>();
    assert_send_sync::<CancelToken>();
};
