//! Errors: one enum with the four error classes of conformance/README.md, plus the configuration and webhook errors.
//!
//! Match on the variant (the class), never on the message. No error ever contains the API key, a header, a URL with
//! credentials, or the text of an underlying transport error.

use std::fmt;
use std::time::Duration;

use serde_json::Value;

/// What every failure of a call carries.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    /// The API's stable machine code (`"insufficient_credits"`, `"rate_limited"`, ...) or one of the SDK's own:
    /// `"unavailable"` (no answer), `"invalid_response"` (a 2xx whose body lacks a promised field), `"validation_error"`
    /// (an id refused locally) and `"cancelled"` (the call was cancelled through a [`CancelToken`](crate::CancelToken)).
    pub code: String,
    /// The HTTP status, or `0` when there was no response.
    pub status: u16,
    /// For people; may change. Never compare it.
    pub message: String,
    /// The API's `Request-Id`. Quote it in a support request.
    pub request_id: Option<String>,
    /// The API's structured detail, if any (for example `balance` and `required` on a 402).
    pub details: Option<Value>,
    /// The server's `Retry-After`, when it sent one.
    pub retry_after: Option<Duration>,
}

/// The four error classes of conformance/README.md, without their payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorClass {
    /// 401 and 403.
    Auth,
    /// 402.
    LimitReached,
    /// Every other 4xx and local validation failures.
    InvalidRequest,
    /// No answer, 429, 5xx and `invalid_response`.
    Unavailable,
}

/// A failed call. Match on the variant.
///
/// ```
/// # use vunly::Error;
/// # fn handle(err: Error) {
/// match err {
///     Error::LimitReached(_) => { /* 402: not enough credits, nothing was debited */ }
///     Error::Unavailable(_) => { /* unknown outcome: fail closed, retry later with the SAME id */ }
///     other => { eprintln!("{other}"); }
/// }
/// # }
/// ```
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Error {
    /// 401 or 403: a missing, wrong or revoked key, or a key without the scope. Not retryable.
    Auth(Box<ApiError>),
    /// 402: the customer has too few credits. Nothing was debited. Do not run the work. Not retryable.
    LimitReached(Box<ApiError>),
    /// Every other 4xx (400, 404, 409) and any local validation failure: fix the request.
    InvalidRequest(Box<ApiError>),
    /// No answer (network, timeout), a 429, a 5xx that survived the SDK's retries, or `invalid_response`.
    /// The call may or may not have been applied; retrying later with the SAME id is safe for keyed operations.
    Unavailable(Box<ApiError>),
}

impl Error {
    /// Classifies by status: 402 LimitReached, 401/403 Auth, 0/429/5xx Unavailable, everything else InvalidRequest.
    pub(crate) fn from_status(api: ApiError) -> Error {
        let api = Box::new(api);
        match api.status {
            402 => Error::LimitReached(api),
            401 | 403 => Error::Auth(api),
            0 | 429 | 500..=599 => Error::Unavailable(api),
            _ => Error::InvalidRequest(api),
        }
    }

    pub(crate) fn unavailable(code: &str, message: impl Into<String>) -> Error {
        Error::Unavailable(Box::new(ApiError {
            code: code.to_string(),
            status: 0,
            message: message.into(),
            request_id: None,
            details: None,
            retry_after: None,
        }))
    }

    pub(crate) fn invalid_response(status: u16, request_id: Option<String>, what: &str) -> Error {
        // Unavailable whatever the status: the call may have been applied, so retrying later with the SAME id is safe.
        Error::Unavailable(Box::new(ApiError {
            code: "invalid_response".into(),
            status,
            message: format!("the API answered {status} but the response is malformed ({what})"),
            request_id,
            details: None,
            retry_after: None,
        }))
    }

    /// The class of this error.
    pub fn class(&self) -> ErrorClass {
        match self {
            Error::Auth(_) => ErrorClass::Auth,
            Error::LimitReached(_) => ErrorClass::LimitReached,
            Error::InvalidRequest(_) => ErrorClass::InvalidRequest,
            Error::Unavailable(_) => ErrorClass::Unavailable,
        }
    }

    /// The details common to every class.
    pub fn api(&self) -> &ApiError {
        match self {
            Error::Auth(e) | Error::LimitReached(e) | Error::InvalidRequest(e) | Error::Unavailable(e) => e,
        }
    }

    /// The API's stable code, or one of the SDK's own (see [`ApiError::code`]).
    pub fn code(&self) -> &str {
        &self.api().code
    }

    /// The HTTP status, `0` when there was no response.
    pub fn status(&self) -> u16 {
        self.api().status
    }

    /// The message, for people. It never contains the key.
    pub fn message(&self) -> &str {
        &self.api().message
    }

    /// The API's `Request-Id`, when the API answered.
    pub fn request_id(&self) -> Option<&str> {
        self.api().request_id.as_deref()
    }

    /// The API's structured detail, if any.
    pub fn details(&self) -> Option<&Value> {
        self.api().details.as_ref()
    }

    /// The server's `Retry-After`, if it sent one.
    pub fn retry_after(&self) -> Option<Duration> {
        self.api().retry_after
    }

    /// `true` for an outcome that is unknown or temporary and that a later attempt (with the same id for keyed
    /// operations) can resolve: [`Error::Unavailable`] except the plan-volume stop and a cancelled call.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Error::Unavailable(e)
            if !matches!(e.code.as_str(), "cancelled" | "unexpected_redirect" | "invalid_response"))
            && !self.is_plan_stop()
    }

    /// `true` for the Free plan's monthly volume stop: a 429 whose `details.reason` is `plan_included_volume`. It is
    /// not a rate limit and is never retried by the SDK; `Retry-After` is the time to the start of the next month.
    pub fn is_plan_stop(&self) -> bool {
        let e = self.api();
        e.status == 429
            && e.details.as_ref().and_then(|d| d.get("reason")).and_then(Value::as_str) == Some("plan_included_volume")
    }

    /// On a 402: the customer's balance when the request was refused, if the API sent it.
    pub fn balance(&self) -> Option<i64> {
        self.detail_int("balance")
    }

    /// On a 402: the credits the refused request needed, if the API sent it.
    pub fn required(&self) -> Option<i64> {
        self.detail_int("required")
    }

    fn detail_int(&self, key: &str) -> Option<i64> {
        match self {
            Error::LimitReached(e) => e.details.as_ref()?.get(key)?.as_i64(),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let e = self.api();
        write!(f, "vunly: {}", e.message)?;
        if e.status != 0 {
            write!(f, " ({} {})", e.status, e.code)?;
        } else {
            write!(f, " ({})", e.code)?;
        }
        if let Some(id) = &e.request_id {
            write!(f, " request {id}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

/// A client that could not be built. The messages name the setting, never its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigError {
    /// The API key is empty. Read it from your environment (`VUNLY_API_KEY`).
    EmptyApiKey,
    /// The API key contains characters that cannot be sent in an HTTP header.
    InvalidApiKey,
    /// The base URL is empty. The SDK has no default address (`VUNLY_API_URL`).
    EmptyBaseUrl,
    /// The base URL is not an absolute `http://` or `https://` address without credentials, query or fragment.
    InvalidBaseUrl,
    /// The base URL is plain `http://` to a host that is not this machine (`localhost`, `127.0.0.0/8`, `::1`). The key
    /// travels in a header: use `https://`, or set `ClientBuilder::allow_insecure` for a test setup you control.
    InsecureBaseUrl,
    /// A timeout or backoff is outside its range (attempt timeout 10 ms to 10 min, total timeout up to 1 h, backoff up
    /// to 60 s).
    InvalidTimeout,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ConfigError::EmptyApiKey => "vunly: the API key is empty (read it from your environment)",
            ConfigError::InvalidApiKey => "vunly: the API key contains characters that cannot be sent in a header",
            ConfigError::EmptyBaseUrl => {
                "vunly: the base URL is empty (set it to the API address from the setup step)"
            }
            ConfigError::InvalidBaseUrl => {
                "vunly: the base URL must be an absolute http(s) address such as https://host, without credentials, query or fragment"
            }
            ConfigError::InsecureBaseUrl => {
                "vunly: the base URL is plain http to a host that is not this machine; use https (or allow_insecure for a test setup)"
            }
            ConfigError::InvalidTimeout => {
                "vunly: a timeout is out of range (attempt 10 ms to 10 min, total up to 1 h, backoff up to 60 s)"
            }
        })
    }
}

impl std::error::Error for ConfigError {}

/// Why a webhook delivery was refused. Answer a failed verification with a 400 and do not process the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WebhookError {
    /// The `Vunly-Signature` header is absent or empty.
    MissingHeader,
    /// The header is not `t=<unix>,v1=<hex>`.
    MalformedHeader,
    /// The timestamp is more than five minutes from now, in either direction.
    StaleTimestamp,
    /// No signature matches any of the secrets.
    NoMatchingSignature,
    /// The signature is good but the body is not an event.
    InvalidPayload,
    /// A signing secret is empty or only whitespace (checked first, also inside a rotation list): a programming error.
    InvalidSecret,
}

impl WebhookError {
    /// The reason as a stable lowercase string (the same words the other Vunly SDKs use).
    pub fn reason(&self) -> &'static str {
        match self {
            WebhookError::MissingHeader => "missing_header",
            WebhookError::MalformedHeader => "malformed_header",
            WebhookError::StaleTimestamp => "stale_timestamp",
            WebhookError::NoMatchingSignature => "no_matching_signature",
            WebhookError::InvalidPayload => "invalid_payload",
            WebhookError::InvalidSecret => "invalid_secret",
        }
    }
}

impl fmt::Display for WebhookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "vunly: webhook verification failed ({})", self.reason())
    }
}

impl std::error::Error for WebhookError {}
