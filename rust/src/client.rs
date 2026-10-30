//! The client: configuration, the single request path and the retry rules of conformance/README.md.

use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::error::{ApiError, ConfigError, Error};
use crate::json::{self, Read, Shape};
use crate::VERSION;

/// The largest response body the SDK reads, in bytes. Over it a 2xx is `invalid_response` and an error status keeps its
/// class by status. Reading stops at the cap: the SDK never holds more than the cap plus one 16 KiB chunk.
pub const MAX_BODY_BYTES: usize = 16 << 20;
/// The longest `Retry-After` the SDK waits for. Beyond it the error is returned with `retry_after` set.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);

const CHUNK: usize = 16 * 1024;
const SLEEP_SLICE: Duration = Duration::from_millis(25);
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;
const MAX_MESSAGE_CHARS: usize = 500;
const MAX_REQUEST_ID_CHARS: usize = 128;
const MAX_CODE_CHARS: usize = 64;
const MAX_JSON_VALUES: usize = 200_000;

/// Holds the API key and never prints it, however it is formatted.
#[derive(Clone)]
pub(crate) struct Secret(String);

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<hidden>")
    }
}

/// Cancels the calls of every client made with [`Client::with_cancel_token`]. Cheap to clone; all clones are one token.
///
/// Cancellation is checked before each attempt and while the SDK waits between retries (every 25 ms). A request already
/// on the wire ends at its own timeout. A cancelled call returns [`Error::Unavailable`] with code `"cancelled"`,
/// which is not retryable.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A new token that has not been cancelled.
    pub fn new() -> CancelToken {
        CancelToken::default()
    }

    /// Cancels every call bound to this token.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Has [`CancelToken::cancel`] been called?
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Builds a [`Client`]. The key and the base URL are required; neither is read from the environment or from disk.
#[derive(Clone)]
pub struct ClientBuilder {
    api_key: Secret,
    base_url: Option<String>,
    timeout: Duration,
    total_timeout: Option<Duration>,
    max_retries: u32,
    retry_base: Duration,
    allow_insecure: bool,
}

impl fmt::Debug for ClientBuilder {
    /// Neither the key nor the address as typed is shown: an address that was refused may carry credentials.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("api_key", &"<hidden>")
            .field("base_url", &self.base_url.as_ref().map(|_| "<set>"))
            .field("timeout", &self.timeout)
            .field("total_timeout", &self.total_timeout)
            .field("max_retries", &self.max_retries)
            .finish()
    }
}

/// Timeouts outside this range are refused at build time: a smaller one cannot complete a request, a larger one is a hang.
const TIMEOUT_RANGE: std::ops::RangeInclusive<Duration> = Duration::from_millis(10)..=Duration::from_secs(600);
const TOTAL_TIMEOUT_MAX: Duration = Duration::from_secs(3600);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

impl ClientBuilder {
    /// The API address, for example `"https://host"` or `"http://127.0.0.1:8787"`. A trailing slash or `/v1` is
    /// accepted and ignored. Required: the SDK has no default address. Credentials in the URL are refused.
    pub fn base_url(mut self, url: impl Into<String>) -> ClientBuilder {
        self.base_url = Some(url.into());
        self
    }

    /// Allows plain `http://` to a host that is not this machine. Off by default: the key travels in a header, so
    /// anything but `localhost`, `127.0.0.0/8` and `::1` must be `https://`. Only for a test setup you control.
    pub fn allow_insecure(mut self, allow: bool) -> ClientBuilder {
        self.allow_insecure = allow;
        self
    }

    /// The time ONE attempt may take as a whole: connecting, sending, the headers and the whole body. A server that
    /// answers at once and then drips its body is cut at this timeout. Default 10 seconds.
    pub fn timeout(mut self, timeout: Duration) -> ClientBuilder {
        self.timeout = timeout;
        self
    }

    /// An optional deadline for the whole call, retries and waits included. Default: none.
    pub fn total_timeout(mut self, total: Duration) -> ClientBuilder {
        self.total_timeout = Some(total);
        self
    }

    /// How many times a retryable failure is repeated (default 2; `0` turns retries off).
    pub fn max_retries(mut self, n: u32) -> ClientBuilder {
        self.max_retries = n;
        self
    }

    /// The first backoff before a retry (default 250 ms; doubled each retry, capped at 8 s, then multiplied by a random
    /// factor between 0.5 and 1). Mostly for tests.
    pub fn retry_backoff(mut self, base: Duration) -> ClientBuilder {
        self.retry_base = base;
        self
    }

    /// Builds the client.
    pub fn build(self) -> Result<Client, ConfigError> {
        // Only a trailing line ending (LF or CRLF, as `echo` or a secrets file leaves) is stripped. A space, tab, NUL or a
        // lone CR at either end is a mistake in the key, not whitespace to forgive: it is refused below.
        let key = self.api_key.0.as_str();
        let key = key
            .strip_suffix('\n')
            .map_or(key, |k| k.strip_suffix('\r').unwrap_or(k));
        if key.is_empty() {
            return Err(ConfigError::EmptyApiKey);
        }
        if !key.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
            return Err(ConfigError::InvalidApiKey);
        }
        let base_url = normalize_base_url(self.base_url.as_deref().unwrap_or(""), self.allow_insecure)?;
        if !TIMEOUT_RANGE.contains(&self.timeout)
            || self
                .total_timeout
                .is_some_and(|t| t < *TIMEOUT_RANGE.start() || t > TOTAL_TIMEOUT_MAX)
            || self.retry_base > BACKOFF_MAX
        {
            return Err(ConfigError::InvalidTimeout);
        }
        let user_agent = format!(
            "vunly-rust/{VERSION} (rust {}/{})",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        // Everything that could surprise a money-related client is switched off on purpose:
        //  - redirects: a 3xx is returned as it is, so the key never follows a redirect to another host;
        //  - status-as-error: the SDK classifies statuses itself;
        //  - proxies from the environment: the SDK reads no environment variable the caller did not ask for.
        // The library does not replay a request by itself (ureq has no automatic retry); only this client retries.
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .proxy(None)
            .user_agent(user_agent.as_str())
            .build()
            .into();
        Ok(Client {
            inner: Arc::new(Inner {
                api_key: Secret(key.to_owned()),
                base_url,
                agent,
                timeout: self.timeout,
                total_timeout: self.total_timeout,
                max_retries: self.max_retries,
                retry_base: self.retry_base,
                user_agent,
            }),
            cancel: None,
        })
    }
}

struct Inner {
    api_key: Secret,
    base_url: String,
    agent: ureq::Agent,
    timeout: Duration,
    total_timeout: Option<Duration>,
    max_retries: u32,
    retry_base: Duration,
    user_agent: String,
}

/// Talks to one Vunly API. `Client` is `Send + Sync` and cheap to clone (clones share one connection pool): create it
/// once and share it between threads.
///
/// The loop every integration is built from:
///
/// ```no_run
/// use vunly::{CheckParams, Client, Estimate, RecordParams};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let client = Client::new(std::env::var("VUNLY_API_KEY")?, std::env::var("VUNLY_API_URL")?)?;
/// let decision = client.entitlements().check(
///     CheckParams::new("acme", "generate").estimate(Estimate::new("tokens", 1050)),
/// );
/// if !matches!(decision, Ok(d) if d.allowed) {
///     return Ok(()); // a failed check is a "no": fail closed
/// }
/// // ... run the work, then record it once under an id that names this unit of work ...
/// let res = client.usage().record(RecordParams::new("req_2026_0001", "acme", "tokens", 1050))?;
/// println!("{} credits left (replayed: {})", res.remaining, res.replayed);
/// # Ok(()) }
/// ```
///
/// The check is advisory and fails closed; the record is final. The id names the unit of work, is sent as the
/// `Idempotency-Key`, and is never generated for you: retry with the SAME one.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
    cancel: Option<CancelToken>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.inner.base_url)
            .field("api_key", &"<hidden>")
            .field("timeout", &self.inner.timeout)
            .field("max_retries", &self.inner.max_retries)
            .finish()
    }
}

impl Client {
    /// Starts a [`ClientBuilder`]. Read the key from your environment; never write it into source, tests or logs.
    pub fn builder(api_key: impl Into<String>) -> ClientBuilder {
        ClientBuilder {
            api_key: Secret(api_key.into()),
            base_url: None,
            timeout: Duration::from_secs(10),
            total_timeout: None,
            max_retries: 2,
            retry_base: Duration::from_millis(250),
            allow_insecure: false,
        }
    }

    /// A client with the defaults: 10 s per attempt, 2 retries. Same as `Client::builder(key).base_url(url).build()`.
    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>) -> Result<Client, ConfigError> {
        Client::builder(api_key).base_url(base_url).build()
    }

    /// The same client (and connection pool), bound to `token`: cancelling the token stops its calls.
    pub fn with_cancel_token(&self, token: &CancelToken) -> Client {
        Client {
            inner: Arc::clone(&self.inner),
            cancel: Some(token.clone()),
        }
    }

    /// The normalized base URL (no trailing slash, no `/v1`).
    pub fn base_url(&self) -> &str {
        &self.inner.base_url
    }

    /// The `User-Agent` this client sends.
    pub fn user_agent(&self) -> &str {
        &self.inner.user_agent
    }

    fn cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(CancelToken::is_cancelled)
    }

    /// Sleeps `d` in short slices so a cancel is noticed. Returns `false` when cancelled.
    fn sleep(&self, d: Duration) -> bool {
        let end = Instant::now() + d;
        loop {
            if self.cancelled() {
                return false;
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            thread::sleep(left.min(SLEEP_SLICE));
        }
    }
}

/// Returns the address without a trailing slash and without a trailing `/v1`: the SDK adds `/v1/...` itself.
pub(crate) fn normalize_base_url(url: &str, allow_insecure: bool) -> Result<String, ConfigError> {
    let url = url.trim();
    if url.is_empty() {
        return Err(ConfigError::EmptyBaseUrl);
    }
    // Only ASCII: an internationalised host must be given as punycode. This also keeps the byte slicing below on
    // character boundaries (a multibyte character there used to panic).
    if !url.is_ascii() {
        return Err(ConfigError::InvalidBaseUrl);
    }
    let mut out = url.trim_end_matches('/').to_owned();
    if out.len() >= 3 && out[out.len() - 3..].eq_ignore_ascii_case("/v1") {
        out.truncate(out.len() - 3);
    }
    let out = out.trim_end_matches('/').to_owned();
    let (secure, rest) = if let Some(r) = strip_scheme(&out, "http://") {
        (false, r)
    } else if let Some(r) = strip_scheme(&out, "https://") {
        (true, r)
    } else {
        return Err(ConfigError::InvalidBaseUrl);
    };
    let authority = rest.split('/').next().unwrap_or("");
    let bad = authority.is_empty()
        || authority.contains('@')
        || authority
            .chars()
            .any(|c| c.is_whitespace() || c == '?' || c == '#' || c == '\\')
        || rest.contains(['?', '#'])
        || out.chars().any(|c| c.is_control() || c.is_whitespace());
    if bad {
        return Err(ConfigError::InvalidBaseUrl);
    }
    let host = host_of(authority).ok_or(ConfigError::InvalidBaseUrl)?;
    // plain http is for this machine only (a local sandbox or a test); anything else needs https, or an explicit opt-in
    if !secure && !allow_insecure && !is_loopback(host) {
        return Err(ConfigError::InsecureBaseUrl);
    }
    Ok(out)
}

/// The host of an authority (`host`, `host:port`, `[v6]`, `[v6]:port`), or `None` when it is malformed: more than one
/// colon outside brackets, an unclosed bracket, an IPv6 address that does not parse, an empty or non-numeric port, a port
/// of 0 or above 65535, a host with characters a name cannot have.
fn host_of(authority: &str) -> Option<&str> {
    let (host, port) = if let Some(inner) = authority.strip_prefix('[') {
        let (host, after) = inner.split_once(']')?;
        host.parse::<std::net::Ipv6Addr>().ok()?;
        (
            host,
            if after.is_empty() {
                None
            } else {
                Some(after.strip_prefix(':')?)
            },
        )
    } else {
        match authority.split_once(':') {
            None => (authority, None),
            Some((h, p)) if !p.contains(':') => (h, Some(p)),
            Some(_) => return None,
        }
    };
    if let Some(p) = port {
        if p.is_empty()
            || p.len() > 5
            || !p.bytes().all(|b| b.is_ascii_digit())
            || !(1..=65535).contains(&p.parse::<u32>().ok()?)
        {
            return None;
        }
    }
    let name_ok = !host.is_empty()
        && (host.contains(':') // a bracketed IPv6 address, parsed above
            || host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')));
    name_ok.then_some(host)
}

/// `localhost`, `127.0.0.0/8` and `::1`.
fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<std::net::Ipv4Addr>().is_ok_and(|a| a.is_loopback())
        || host.parse::<std::net::Ipv6Addr>().is_ok_and(|a| a.is_loopback())
}

fn strip_scheme<'a>(s: &'a str, scheme: &str) -> Option<&'a str> {
    let head = s.get(..scheme.len())?; // None, not a panic, when the cut is inside a multibyte character
    head.eq_ignore_ascii_case(scheme).then(|| &s[scheme.len()..])
}

// ---- requests ------------------------------------------------------------------------------------------------------

/// The operation class of conformance/README.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    /// Reads and the entitlement check.
    R,
    /// Keyed mutations (same id on retry).
    K,
    /// Naturally idempotent writes: upserts, deletes.
    N,
    /// Everything else: never repeated after an ambiguous failure.
    X,
}

pub(crate) struct Request<'a> {
    pub method: &'static str,
    pub path: String,
    pub query: Vec<(&'static str, String)>,
    pub body: Option<Value>,
    pub idem: Option<&'a str>,
    pub class: Class,
    /// The fields a successful body must carry. Empty: the body is not read as JSON.
    pub shape: Shape,
}

impl<'a> Request<'a> {
    pub fn new(method: &'static str, path: String, class: Class) -> Request<'a> {
        Request {
            method,
            path,
            query: Vec::new(),
            body: None,
            idem: None,
            class,
            shape: &[],
        }
    }
    pub fn body(mut self, body: Value) -> Self {
        self.body = Some(body);
        self
    }
    pub fn idem(mut self, id: &'a str) -> Self {
        self.idem = Some(id);
        self
    }
    pub fn shape(mut self, shape: Shape) -> Self {
        self.shape = shape;
        self
    }
    pub fn query(mut self, query: Vec<(&'static str, String)>) -> Self {
        self.query = query;
        self
    }
}

#[derive(Debug)]
pub(crate) struct Reply {
    pub request_id: Option<String>,
    pub replayed: bool,
    pub plan_overage: bool,
    pub body: Value,
}

/// Refuses, before any request, an id that could not be an `Idempotency-Key`: the same error the API would give.
pub(crate) fn check_id(id: &str) -> Result<(), Error> {
    let good = (8..=128).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'));
    if good {
        return Ok(());
    }
    Err(Error::from_status(ApiError {
        code: "validation_error".into(),
        status: 400,
        message: "id must be 8 to 128 characters of A-Za-z0-9_.:- (it is sent as the Idempotency-Key)".into(),
        request_id: None,
        details: Some(serde_json::json!({"fields": {"id": "8-128 chars of A-Za-z0-9_.:-"}})),
        retry_after: None,
    }))
}

/// The most `retry_after` ever reports: a number above it (or too large to represent) is reported as this.
const REPORT_CAP_SECONDS: f64 = 86_400.0;

/// Reads a `Retry-After` header per conformance/README.md (vectors:
/// conformance/retry-after-vectors.json). Only a plain decimal number of seconds counts: digits, an optional fraction, an
/// optional exponent, an optional leading `+`, surrounding spaces (and `inf`/`infinity`, which is a too-long wait). Anything else (an HTTP date, `abc`, hex, a suffix, a sign, `NaN`) is `None`:
/// no directive. A number too large to hold (`1e999`, 400 digits) is a very long wait, never an absent header: it is
/// returned as the cap, so the caller surfaces the error at once instead of retrying early.
pub(crate) fn parse_retry_after(value: &str) -> Option<Duration> {
    let v = value.trim_matches(|c: char| c.is_ascii_whitespace());
    // an optional leading plus; `inf` / `infinity` in any case is a wait too long to represent
    let v = v.strip_prefix('+').unwrap_or(v);
    if v.eq_ignore_ascii_case("inf") || v.eq_ignore_ascii_case("infinity") {
        return Duration::try_from_secs_f64(REPORT_CAP_SECONDS).ok();
    }
    let b = v.as_bytes();
    let digits = |from: usize| b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    // mantissa: digits, digits ".", digits "." digits, or "." digits ("5", "5.", "5.5", ".5"); a lone "." is nothing
    let int_digits = digits(0);
    let mut i = int_digits;
    if b.get(i) == Some(&b'.') {
        let frac_digits = digits(i + 1);
        if int_digits == 0 && frac_digits == 0 {
            return None;
        }
        i += 1 + frac_digits;
    } else if int_digits == 0 {
        return None;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let n = digits(j);
        if n == 0 {
            return None;
        }
        i = j + n;
    }
    if i != b.len() {
        return None;
    }
    // Rust's float parser saturates to infinity instead of failing on overflow; both mean "too long to wait".
    let secs = v.parse::<f64>().unwrap_or(f64::INFINITY);
    Duration::try_from_secs_f64(secs.min(REPORT_CAP_SECONDS)).ok()
}

/// A random factor in `[0.5, 1.0)`; `RandomState` is seeded by the operating system, so no `rand` dependency is needed.
fn jitter() -> f64 {
    let n = RandomState::new().build_hasher().finish();
    0.5 + (n >> 11) as f64 / (1u64 << 53) as f64 / 2.0
}

/// How long to wait before retry number `attempt` (1-based), or `None` when the server asked for longer than the cap.
pub(crate) fn retry_delay(
    attempt: u32,
    retry_after: Option<Duration>,
    base: Duration,
    factor: f64,
) -> Option<Duration> {
    if let Some(ra) = retry_after.filter(|d| !d.is_zero()) {
        return (ra <= MAX_RETRY_AFTER).then_some(ra);
    }
    let doubled = base.saturating_mul(1u32.checked_shl(attempt.saturating_sub(1)).unwrap_or(u32::MAX));
    Some(doubled.min(Duration::from_secs(8)).mul_f64(factor))
}

/// The retry classes: R, K and N are repeated after a lost answer, a timeout or a 5xx; X only after a 429, which was
/// refused before it ran. The plan-volume stop is never repeated. Anything else is final.
pub(crate) fn may_retry(class: Class, err: &Error) -> bool {
    let Error::Unavailable(e) = err else { return false };
    if e.code == "cancelled" || err.is_plan_stop() {
        return false;
    }
    match e.status {
        0 | 429 | 500 | 502 | 503 | 504 => {}
        _ => return false, // invalid_response and anything else: the status is not a retryable one
    }
    !(class == Class::X && e.status != 429)
}

enum ReadFail {
    TooBig,
    Broken,
}

/// Reads the body, never holding more than the cap plus one chunk, and gives up at the deadline.
fn read_capped(reader: &mut impl std::io::Read, deadline: Instant, cap: usize) -> Result<Vec<u8>, ReadFail> {
    let mut out = Vec::new();
    let mut chunk = [0u8; CHUNK];
    loop {
        if Instant::now() >= deadline {
            return Err(ReadFail::Broken);
        }
        match reader.read(&mut chunk) {
            Ok(0) => return Ok(out),
            Ok(n) => {
                if out.len() + n > cap {
                    return Err(ReadFail::TooBig);
                }
                out.extend_from_slice(&chunk[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Err(ReadFail::Broken),
        }
    }
}

fn fallback_code(status: u16) -> &'static str {
    match status {
        401 | 403 => "unauthenticated",
        402 => "insufficient_credits",
        429 => "rate_limited",
        500..=599 => "internal_error",
        _ => "validation_error",
    }
}

impl Client {
    /// Sends one request with the retry rules of its class and returns the decoded, shape-checked body.
    pub(crate) fn send(&self, r: &Request<'_>) -> Result<Reply, Error> {
        if let Some(id) = r.idem {
            check_id(id)?;
        }
        let payload = r.body.as_ref().map(|b| serde_json::to_vec(b).unwrap_or_default());
        let mut url = format!("{}{}", self.inner.base_url, r.path);
        for (i, (k, v)) in r.query.iter().enumerate() {
            url.push(if i == 0 { '?' } else { '&' });
            url.push_str(&format!("{k}={}", json::escape(v)));
        }
        let deadline = self.inner.total_timeout.map(|t| Instant::now() + t);
        let mut attempt = 1u32;
        loop {
            if self.cancelled() {
                return Err(cancelled());
            }
            let timeout = match deadline {
                Some(d) => {
                    let left = d.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(Error::unavailable("unavailable", "the call's total timeout ran out"));
                    }
                    left.min(self.inner.timeout)
                }
                None => self.inner.timeout,
            };
            let err = match self.once(r, &url, payload.as_deref(), timeout) {
                Ok(reply) => return Ok(reply),
                Err(e) => e,
            };
            if attempt > self.inner.max_retries || !may_retry(r.class, &err) {
                return Err(err);
            }
            let Some(wait) = retry_delay(attempt, err.retry_after(), self.inner.retry_base, jitter()) else {
                return Err(err);
            };
            if deadline.is_some_and(|d| Instant::now() + wait >= d) {
                return Err(err);
            }
            if !self.sleep(wait) {
                return Err(cancelled());
            }
            attempt += 1;
        }
    }

    fn once(&self, r: &Request<'_>, url: &str, payload: Option<&[u8]>, timeout: Duration) -> Result<Reply, Error> {
        let started = Instant::now();
        let inner = &*self.inner;
        let mut auth = ureq::http::HeaderValue::from_str(&format!("Bearer {}", inner.api_key.0))
            .map_err(|_| Error::unavailable("unavailable", "cannot build the request"))?;
        auth.set_sensitive(true);
        let mut builder = ureq::http::Request::builder()
            .method(r.method)
            .uri(url)
            .header("Authorization", auth)
            .header("Accept", "application/json");
        if let Some(id) = r.idem {
            builder = builder.header("Idempotency-Key", id);
        }
        let build_err = |_| Error::unavailable("unavailable", "cannot build the request");
        let result = match payload {
            Some(p) => {
                let req = builder
                    .header("Content-Type", "application/json")
                    .body(p.to_vec())
                    .map_err(build_err)?;
                self.run(req, timeout)
            }
            None => {
                let req = builder.body(()).map_err(build_err)?;
                self.run(req, timeout)
            }
        };
        let mut resp = match result {
            Ok(resp) => resp,
            // Deliberately drop the underlying error: some transports put the URL or headers in it.
            Err(ureq::Error::Timeout(_)) => return Err(self.timed_out(timeout)),
            Err(_) => return Err(Error::unavailable("unavailable", "could not reach the Vunly API")),
        };
        let status = resp.status().as_u16();
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let request_id = header("request-id")
            .map(|r| self.scrub(&r, MAX_REQUEST_ID_CHARS))
            .filter(|r| !r.is_empty());
        let replayed = header("idempotent-replayed").as_deref() == Some("true");
        let plan_overage = header("vunly-plan-overage").as_deref() == Some("true");
        let retry_after = header("retry-after").and_then(|v| parse_retry_after(&v));
        let declared = header("content-length").and_then(|v| v.trim().parse::<u64>().ok());
        // A success body may be large (a page of results). An ERROR body is only read up to 64 KiB: the status decides the
        // class, and the code, message and request id sit at the start of a body that is a few hundred bytes. Over the
        // limit the body is ignored and the status alone classifies the error. This also keeps the error path cheap in
        // memory (a large error body used to cost a multiple of its size).
        let cap = if (200..300).contains(&status) {
            MAX_BODY_BYTES
        } else {
            MAX_ERROR_BODY_BYTES
        };
        let raw = if declared.is_some_and(|n| n > cap as u64) {
            Err(ReadFail::TooBig)
        } else {
            read_capped(&mut resp.body_mut().as_reader(), started + timeout, cap)
        };
        if (200..300).contains(&status) {
            let raw = match raw {
                Ok(raw) => raw,
                Err(ReadFail::TooBig) => {
                    return Err(Error::invalid_response(
                        status,
                        request_id,
                        "the body is larger than 16 MB",
                    ))
                }
                // the answer was lost on the way: an unknown outcome, retried like any lost answer
                Err(ReadFail::Broken) => return Err(self.timed_out_or_broken(started, timeout)),
            };
            let body = if r.shape.is_empty() {
                Value::Null
            } else {
                let value = parse_json(&raw).ok_or_else(|| {
                    Error::invalid_response(status, request_id.clone(), "the body is not JSON or is too complex")
                })?;
                if let Some(problem) = json::shape_problem(&value, r.shape) {
                    return Err(Error::invalid_response(status, request_id, &problem));
                }
                value
            };
            return Ok(Reply {
                request_id,
                replayed,
                plan_overage,
                body,
            });
        }
        // an error status keeps its class by status, whatever happened to its body
        Err(self.error_from_response(status, request_id, retry_after, raw.ok().as_deref()))
    }

    fn run<B: ureq::AsSendBody>(
        &self,
        req: ureq::http::Request<B>,
        timeout: Duration,
    ) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
        // one bound for the whole attempt: connect, send, headers AND body (ureq's global timeout keeps counting while
        // the body is read), so a server that answers at once and drips the body is cut at the timeout
        let req = self
            .inner
            .agent
            .configure_request(req)
            .timeout_global(Some(timeout))
            .build();
        self.inner.agent.run(req)
    }

    fn timed_out(&self, timeout: Duration) -> Error {
        Error::unavailable(
            "unavailable",
            format!("the Vunly API did not answer within {}", fmt_duration(timeout)),
        )
    }

    fn timed_out_or_broken(&self, started: Instant, timeout: Duration) -> Error {
        if started.elapsed() >= timeout {
            self.timed_out(timeout)
        } else {
            Error::unavailable("unavailable", "the connection broke while the answer was being read")
        }
    }

    fn error_from_response(
        &self,
        status: u16,
        header_request_id: Option<String>,
        retry_after: Option<Duration>,
        raw: Option<&[u8]>,
    ) -> Error {
        // an error with no body (a proxy, an empty 5xx) is still classified by its status
        if (300..400).contains(&status) {
            // Never followed (the key would go to another host), never retried, and not the caller's mistake in the
            // request: the configured address is wrong or a proxy is in the way. The Location is not repeated.
            return Error::Unavailable(Box::new(ApiError {
                code: "unexpected_redirect".into(),
                status,
                message: format!(
                    "the API answered {status}, a redirect, which is never followed (check the API address you configured)"
                ),
                request_id: header_request_id.map(|r| self.scrub(&r, MAX_REQUEST_ID_CHARS)),
                details: None,
                retry_after: None,
            }));
        }
        let mut envelope: Value = raw.and_then(parse_json).unwrap_or(Value::Null);
        // `details` is moved out of the parsed body and cleaned by value: no second copy of the tree is made
        let details = envelope
            .get_mut("error")
            .and_then(|e| e.get_mut("details"))
            .map(Value::take)
            .filter(Value::is_object)
            .map(|d| self.scrub_value(d));
        let e = envelope.child("error");
        let code = e
            .opt_string("code")
            .map(|c| self.scrub(&c, MAX_CODE_CHARS))
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| fallback_code(status).to_owned());
        let mut message = e
            .opt_string("message")
            .map(|m| self.scrub(&m, MAX_MESSAGE_CHARS))
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| format!("request failed with status {status}"));
        if status == 404 && message.to_lowercase().contains("no such route") {
            message.push_str(
                " (check the API address you configured: it should be the address only, for example https://host; a trailing /v1 is accepted)",
            );
        }
        let request_id = e
            .opt_string("requestId")
            .or(header_request_id)
            .map(|r| self.scrub(&r, MAX_REQUEST_ID_CHARS))
            .filter(|r| !r.is_empty());
        Error::from_status(ApiError {
            code,
            status,
            message,
            request_id,
            details,
            retry_after,
        })
    }

    /// Text from the server that people will read (logs, terminals): cleaned FIRST (so a key with a zero-width character
    /// in it is whole again), then the key is replaced, and only then is the length bounded (so a cut can never leave
    /// half a key behind).
    fn scrub(&self, text: &str, max_chars: usize) -> String {
        let key = self.inner.api_key.0.as_str();
        // The key is looked for in the text with every character that is not printable ASCII taken out, so ANY invisible
        // or exotic character inserted into it (not only the ones a denylist knows) cannot hide it; the whole stretch,
        // inserted characters included, is replaced. Cleaning and the length bound come after.
        let redacted = redact_key(text, key);
        bound(&clean(&redacted).replace(key, "<hidden>"), max_chars)
    }

    /// The same for every string (and key) inside an error's `details`, consuming the tree (no copy).
    fn scrub_value(&self, v: Value) -> Value {
        match v {
            Value::String(s) => Value::String(self.scrub(&s, MAX_MESSAGE_CHARS)),
            Value::Array(a) => Value::Array(a.into_iter().map(|e| self.scrub_value(e)).collect()),
            Value::Object(o) => Value::Object(
                o.into_iter()
                    .map(|(k, e)| (self.scrub(&k, MAX_CODE_CHARS), self.scrub_value(e)))
                    .collect(),
            ),
            other => other,
        }
    }
}

/// Replaces every occurrence of `key` in `text` by `<hidden>`, even when characters that are not printable ASCII (any
/// character at all: zero-width, default-ignorable, tags, controls, combining marks, spaces) were inserted between its
/// characters. Matching is on the printable-ASCII skeleton of the text; a match covers the original characters from its
/// first to its last matched character, everything in between included.
fn redact_key(text: &str, key: &str) -> String {
    if key.is_empty() {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    // (index in `chars`, byte) for each printable ASCII non-space character
    let skeleton: Vec<(usize, u8)> = chars
        .iter()
        .enumerate()
        .filter(|(_, c)| ('\u{21}'..='\u{7e}').contains(*c))
        .map(|(i, c)| (i, *c as u8))
        .collect();
    let k = key.as_bytes();
    if skeleton.len() < k.len() {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let (mut copied, mut s) = (0usize, 0usize);
    while s + k.len() <= skeleton.len() {
        if skeleton[s..s + k.len()].iter().map(|(_, b)| *b).eq(k.iter().copied()) {
            let (first, last) = (skeleton[s].0, skeleton[s + k.len() - 1].0);
            out.extend(&chars[copied..first]);
            out.push_str("<hidden>");
            copied = last + 1;
            s += k.len();
        } else {
            s += 1;
        }
    }
    out.extend(&chars[copied..]);
    out
}

/// Drops what can forge or hide log lines: control characters, DEL, the C1 range, BiDi overrides and isolates, and
/// zero-width characters.
fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !(c.is_control()
                || matches!(*c,
                    '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}'
                    | '\u{2066}'..='\u{206F}' | '\u{FEFF}' | '\u{00AD}' | '\u{061C}' | '\u{180E}'
                    | '\u{FFF9}'..='\u{FFFB}'))
        })
        .collect()
}

/// Bounds the length (an ellipsis marks a cut) and trims.
fn bound(s: &str, max_chars: usize) -> String {
    let mut out = s.trim().to_owned();
    if out.chars().count() > max_chars {
        out = out.chars().take(max_chars).collect::<String>() + "...";
    }
    out
}

/// Parses a response body. serde_json stops at a nesting depth of 128 with an error (no stack overflow). Besides that,
/// a body with more than `MAX_JSON_VALUES` values is refused before the tree is built: a tree costs about ten times the
/// text (a 13 MB object of 990 000 keys is some 130 MB of memory), so the value count, not the byte count, is what bounds
/// the memory. Separators inside strings are not counted, so a legitimate long string is never refused for its commas.
fn parse_json(raw: &[u8]) -> Option<Value> {
    let (mut values, mut in_string, mut escaped) = (0usize, false, false);
    for &b in raw {
        if in_string {
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
        } else {
            match b {
                b'"' => in_string = true,
                b',' | b'[' | b'{' => {
                    values += 1;
                    if values > MAX_JSON_VALUES {
                        return None;
                    }
                }
                _ => {}
            }
        }
    }
    serde_json::from_slice(raw).ok()
}

fn cancelled() -> Error {
    Error::unavailable("cancelled", "the call was cancelled")
}

fn fmt_duration(d: Duration) -> String {
    if d.subsec_nanos() == 0 {
        format!("{}s", d.as_secs())
    } else {
        format!("{}ms", d.as_millis())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_is_normalised() {
        for (input, want) in [
            ("https://host", "https://host"),
            ("https://host/", "https://host"),
            ("https://host/v1", "https://host"),
            ("https://host/V1/", "https://host"),
            ("  http://127.0.0.1:8787/v1  ", "http://127.0.0.1:8787"),
            ("https://host/base/v1", "https://host/base"),
        ] {
            assert_eq!(normalize_base_url(input, false).as_deref(), Ok(want), "{input}");
        }
        for loopback in [
            "http://localhost",
            "http://LOCALHOST:8787",
            "http://127.0.0.1",
            "http://127.8.9.10:1",
            "http://[::1]:8787",
            "http://[::1]",
        ] {
            assert!(normalize_base_url(loopback, false).is_ok(), "{loopback}");
        }
        assert!(normalize_base_url("https://example.com:8443/v1", false).is_ok());
        assert!(normalize_base_url("https://[2001:db8::1]:443", false).is_ok());
    }

    #[test]
    fn plain_http_is_for_this_machine_unless_allowed() {
        for remote in [
            "http://example.com",
            "http://10.0.0.1",
            "http://[2001:db8::1]",
            "http://localhost.evil.test",
            "http://128.0.0.1",
            "http://2130706433",
        ] {
            assert_eq!(
                normalize_base_url(remote, false),
                Err(ConfigError::InsecureBaseUrl),
                "{remote}"
            );
            assert!(normalize_base_url(remote, true).is_ok(), "{remote} with allow_insecure");
        }
    }

    #[test]
    fn ports_and_hosts_are_validated() {
        for bad in [
            "http://127.0.0.1:abc",
            "http://127.0.0.1:99999",
            "http://127.0.0.1:65536",
            "http://127.0.0.1:0",
            "http://127.0.0.1:",
            "https://host:",
            "http://127.0.0.1:-1",
            "http://127.0.0.1:+1",
            "http://127.0.0.1:0080a",
            "http://[::1",
            "http://[::1]x",
            "http://[::1]:",
            "http://[::1]:0",
            "http://[zz::1]",
            "http://[]",
            "https://h:80:90",
            "https://h:80:",
            "http://::1",
            "https://ho_st!:80",
            "https://a b",
            "https://.:80x",
        ] {
            // checked with allow_insecure so that only the structure decides
            assert_eq!(normalize_base_url(bad, true), Err(ConfigError::InvalidBaseUrl), "{bad}");
        }
        assert!(normalize_base_url("https://h:1", false).is_ok());
        assert!(normalize_base_url("https://h:65535", false).is_ok());
    }

    /// The key is found whatever invisible Unicode is inserted into it (a denylist cannot know them all).
    #[test]
    fn the_key_is_redacted_whatever_is_inserted_into_it() {
        let key = "vk_test_synthetic_key_for_unit_tests_000";
        let c = Client::new(key, "http://127.0.0.1:1").unwrap();
        let fixed = [
            '\u{34f}',
            '\u{3164}',
            '\u{fe0f}',
            '\u{e0001}',
            '\u{200b}',
            '\u{feff}',
            '\u{115f}',
            '\u{1160}',
            '\u{ffa0}',
            '\u{17b4}',
            '\u{180b}',
            '\u{e0020}',
            '\u{e007f}',
            '\u{0}',
            '\u{7f}',
            '\u{85}',
            '\u{a0}',
            '\u{2028}',
            '\u{300}',
            ' ',
            '\u{3000}',
            '\u{202e}',
            '\u{2066}',
            '\u{1d173}',
            '\u{e0100}',
            '\u{1f600}',
            'é',
        ];
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let random_char = |n: u64| -> char {
            // anywhere in Unicode, not only the listed ones
            let mut cp = (n % 0x11_0000) as u32;
            while char::from_u32(cp).is_none_or(|ch| ch.is_ascii_graphic()) {
                cp = (cp + 0x1f) % 0x11_0000;
            }
            char::from_u32(cp).unwrap()
        };
        let probes = [key, &key[..20], &key[key.len() - 20..]];
        for round in 0..2000 {
            let mut text = String::from("prefix ");
            for ch in key.chars() {
                text.push(ch);
                let n = next();
                match n % 4 {
                    0 => {}
                    1 => text.push(fixed[(n >> 8) as usize % fixed.len()]),
                    2 => text.push(random_char(n >> 8)),
                    _ => {
                        text.push(random_char(n >> 8));
                        text.push(fixed[(n >> 40) as usize % fixed.len()]);
                    }
                }
            }
            text.push_str(" suffix");
            let shown = c.scrub(&text, 500);
            for p in probes {
                assert!(
                    !shown.contains(p),
                    "round {round}: {p} survived in {shown:?} (from {text:?})"
                );
            }
            assert!(
                shown.contains("<hidden>") && shown.starts_with("prefix") && shown.ends_with("suffix"),
                "{shown:?}"
            );
        }
        // each of the four named by the review, on its own
        for ch in ['\u{34f}', '\u{3164}', '\u{fe0f}', '\u{e0001}'] {
            let text: String = key.chars().flat_map(|k| [k, ch]).collect();
            assert!(!c.scrub(&text, 500).contains("vk_test"), "{ch:?}");
        }
        // two keys in one text, and the key at the very start and end
        let two = format!("{key} and {key}");
        assert_eq!(c.scrub(&two, 500), "<hidden> and <hidden>");
    }

    #[test]
    fn bad_base_urls_are_refused_without_echoing_them() {
        assert_eq!(normalize_base_url("", false), Err(ConfigError::EmptyBaseUrl));
        for bad in [
            "host",
            "ftp://host",
            "http://",
            "http://user:pass@host",
            "https://host?x=1",
            "https://host#frag",
            "http://ho st",
            "https:///path",
            // multibyte and odd forms used to panic on a byte slice; now they are refused
            "http://host\u{1F600}",
            "http://\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}",
            "https://h\u{e9}",
            "\u{e9}",
            "ht\u{e9}tp://host",
            "http:/\u{e9}",
            "https://host/\u{e9}/v1",
            "http://host/v\u{e9}",
            "\u{202e}http://host",
            "http://host/\u{1F600}1",
            "\u{1F600}",
            "http://",
            "https://",
            "/v1",
        ] {
            assert_eq!(normalize_base_url(bad, true), Err(ConfigError::InvalidBaseUrl), "{bad}");
        }
    }

    #[test]
    fn idempotency_ids_are_validated_locally() {
        for good in ["req_2026_0001", "a:b.c-d_e1", &"x".repeat(128), "12345678"] {
            assert!(check_id(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "short",
            &"x".repeat(129),
            "has space 1234",
            "emoji-é-12345",
            "slash/1234567",
            "new\nline1234",
        ] {
            let err = check_id(bad).unwrap_err();
            assert_eq!((err.status(), err.code()), (400, "validation_error"));
            assert!(matches!(err, Error::InvalidRequest(_)));
            assert!(err.details().unwrap()["fields"]["id"].is_string());
        }
    }

    #[test]
    fn backoff_is_exponential_capped_and_jittered() {
        let base = Duration::from_millis(250);
        assert_eq!(retry_delay(1, None, base, 1.0), Some(Duration::from_millis(250)));
        assert_eq!(retry_delay(2, None, base, 1.0), Some(Duration::from_millis(500)));
        assert_eq!(retry_delay(3, None, base, 1.0), Some(Duration::from_secs(1)));
        assert_eq!(retry_delay(10, None, base, 1.0), Some(Duration::from_secs(8)));
        assert_eq!(retry_delay(99, None, base, 1.0), Some(Duration::from_secs(8)));
        assert_eq!(retry_delay(1, None, base, 0.5), Some(Duration::from_millis(125)));
        for _ in 0..200 {
            let j = jitter();
            assert!((0.5..1.0).contains(&j), "{j}");
        }
    }

    /// Every vector of the contract's Retry-After vectors (shared by all SDKs). `tests/retry-after-vectors.json` is this
    /// crate's copy of `conformance/retry-after-vectors.json`; a repository check fails when they differ.
    #[test]
    fn retry_after_vectors_of_the_contract() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/retry-after-vectors.json");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let file: Value = serde_json::from_str(&text).unwrap();
        let (cap, report) = (
            file["capSeconds"].as_f64().unwrap(),
            file["reportCapSeconds"].as_f64().unwrap(),
        );
        assert_eq!((cap, report), (MAX_RETRY_AFTER.as_secs_f64(), REPORT_CAP_SECONDS));
        let vectors = file["vectors"].as_array().unwrap();
        assert!(vectors.len() >= 39);
        for v in vectors {
            let header = v["header"].as_str().unwrap();
            let parsed = parse_retry_after(header);
            match v["kind"].as_str().unwrap() {
                "ignored" => assert_eq!(parsed, None, "{header:?} must be ignored"),
                kind => {
                    let want = v["seconds"].as_f64().unwrap();
                    let got = parsed
                        .unwrap_or_else(|| panic!("{header:?} must be read"))
                        .as_secs_f64();
                    assert!((got - want).abs() < 1e-9, "{header:?}: {got} != {want}");
                    // what the retry loop does with it: wait up to the cap, otherwise surface at once (never retry early)
                    let delay = retry_delay(1, parsed, Duration::from_millis(250), 1.0);
                    if kind == "surface" {
                        assert_eq!(delay, None, "{header:?} must not be waited on");
                    } else if want > 0.0 {
                        assert_eq!(delay, Some(Duration::from_secs_f64(want)), "{header:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn retry_after_wins_up_to_the_cap() {
        let base = Duration::from_millis(250);
        assert_eq!(
            retry_delay(1, Some(Duration::from_secs(2)), base, 0.7),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            retry_delay(1, Some(Duration::from_secs(30)), base, 0.7),
            Some(Duration::from_secs(30))
        );
        assert_eq!(retry_delay(1, Some(Duration::from_secs(31)), base, 0.7), None);
        // a zero Retry-After is no instruction
        assert_eq!(
            retry_delay(1, Some(Duration::ZERO), base, 1.0),
            Some(Duration::from_millis(250))
        );
    }

    fn unavailable(status: u16, code: &str, details: Option<Value>) -> Error {
        Error::from_status(ApiError {
            code: code.into(),
            status,
            message: "m".into(),
            request_id: None,
            details,
            retry_after: None,
        })
    }

    #[test]
    fn retry_rules_by_class() {
        use Class::*;
        for class in [R, K, N] {
            for status in [0, 429, 500, 502, 503, 504] {
                assert!(may_retry(class, &unavailable(status, "x", None)), "{class:?} {status}");
            }
            for status in [400, 401, 402, 403, 404, 409] {
                assert!(!may_retry(class, &unavailable(status, "x", None)), "{class:?} {status}");
            }
            assert!(!may_retry(class, &Error::invalid_response(200, None, "x")));
            assert!(!may_retry(class, &unavailable(501, "x", None)));
        }
        // class X: only after a 429
        assert!(may_retry(X, &unavailable(429, "rate_limited", None)));
        for status in [0, 500, 502, 503, 504] {
            assert!(!may_retry(X, &unavailable(status, "x", None)), "X {status}");
        }
        // the plan-volume stop is a 429 that is never repeated, in any class
        let stop = serde_json::json!({"reason": "plan_included_volume"});
        for class in [R, K, N, X] {
            assert!(!may_retry(class, &unavailable(429, "rate_limited", Some(stop.clone()))));
        }
        assert!(!may_retry(R, &cancelled()));
    }
}
