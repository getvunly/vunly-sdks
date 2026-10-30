//! A small HTTP service: ask Vunly whether the customer may do the work, do it, record the usage once.
//!
//! ```text
//! VUNLY_API_URL=http://127.0.0.1:8787 VUNLY_API_KEY=vk_test_... cargo run -- --port 8831
//! curl -s localhost:8831/generate -d '{"customer":"acme","prompt":"hello"}'
//! ```
//!
//! The service generates the idempotency id itself. `--trust-request-id` makes it use the caller's `X-Request-Id`
//! header instead (only for a caller you control: see `handle`).
//!
//! The sandbox project needs a meter called "tokens", a plan whose features include "generate" with the customer
//! subscribed to it, and a key with the scopes the onboarding wizard issues. The key is read from the environment and is
//! never printed: a `Client` prints as `Client { base_url: .., api_key: "<hidden>" }`.

use std::io::Read;
use std::process::exit;
use std::thread;

use tiny_http::{Header, Method, Request, Response, Server};
use vunly::serde_json::{json, Value};
use vunly::{CheckParams, Client, Error, Estimate, RecordParams};

const METER: &str = "tokens";
const FEATURE: &str = "generate";

fn main() {
    let (host, port) = listen_address();
    let (key, url) = match (std::env::var("VUNLY_API_KEY"), std::env::var("VUNLY_API_URL")) {
        (Ok(k), Ok(u)) => (k, u),
        _ => {
            eprintln!("set VUNLY_API_KEY and VUNLY_API_URL");
            exit(2);
        }
    };
    let client = match Client::new(key, url) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            exit(2);
        }
    };
    let server = Server::http((host.as_str(), port)).unwrap_or_else(|e| {
        eprintln!("cannot listen on {host}:{port}: {e}");
        exit(2);
    });
    let trust_request_id = std::env::args().any(|a| a == "--trust-request-id");
    println!("listening on http://{host}:{port} ({client:?}); trusting a client X-Request-Id: {trust_request_id}");
    for request in server.incoming_requests() {
        // the SDK blocks, so one thread per request; the client is cheap to clone and shares its connection pool
        let client = client.clone();
        thread::spawn(move || handle(&client, trust_request_id, request));
    }
}

fn listen_address() -> (String, u16) {
    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str, default: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
            .unwrap_or_else(|| default.to_owned())
    };
    (
        flag("--host", "127.0.0.1"),
        flag("--port", "8080").parse().unwrap_or(8080),
    )
}

fn handle(client: &Client, trust_request_id: bool, mut request: Request) {
    let (status, body) = if request.method() == &Method::Post && request.url() == "/generate" {
        let mut raw = String::new();
        let _ = request.as_reader().take(64 * 1024).read_to_string(&mut raw);
        // By default the service makes the idempotency id itself, one per request. A client-supplied header is only
        // used with --trust-request-id: re-sending a header then makes the SAME id come back, and the API (correctly)
        // answers a replay without recording the work again, so a client could run billable work for free by
        // re-sending one header. Trust it only when the client is your own gateway that guarantees one id per unit of work.
        let id = trust_request_id
            .then(|| {
                request
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("X-Request-Id"))
                    .map(|h| h.value.as_str().to_owned())
            })
            .flatten();
        generate(client, &raw, id)
    } else {
        (404, json!({"error": "not_found"}))
    };
    let json_header = Header::from_bytes("Content-Type", "application/json").expect("static header");
    let _ = request.respond(
        Response::from_string(body.to_string())
            .with_status_code(status)
            .with_header(json_header),
    );
}

fn generate(client: &Client, raw: &str, request_id: Option<String>) -> (u16, Value) {
    let Ok(input) = vunly::serde_json::from_str::<Value>(raw) else {
        return (400, json!({"error": "invalid_json"}));
    };
    let (Some(customer), Some(prompt)) = (input["customer"].as_str(), input["prompt"].as_str()) else {
        return (400, json!({"error": "customer and prompt are required"}));
    };
    let id = request_id.unwrap_or_else(new_request_id);
    let tokens = (prompt.chars().count() as i64 * 10).max(1);

    // 1. Ask first. A "no" and an error both mean: do not run the work (fail closed).
    match client
        .entitlements()
        .check(CheckParams::new(customer, FEATURE).estimate(Estimate::new(METER, tokens)))
    {
        Ok(decision) if decision.allowed => {}
        Ok(decision) => return (403, json!({"error": "not_allowed", "reason": decision.reason})),
        Err(e) => return failure(&e),
    }

    // 2. The work. A real service calls its model here.
    let output = format!("echo: {prompt}");

    // 3. Record it once. A LimitReached here means the credits were not there when it counted (the check is only
    // advisory); nothing was debited.
    match client
        .usage()
        .record(RecordParams::new(id.as_str(), customer, METER, tokens))
    {
        Ok(usage) => (
            200,
            json!({"output": output, "id": id, "creditsRemaining": usage.remaining, "replayed": usage.replayed}),
        ),
        Err(e) => failure(&e),
    }
}

/// What each class of SDK error means to this service's callers. Anything not understood fails closed (503): when Vunly
/// cannot be reached, or answers something unexpected, the work is not done and the caller may try again.
fn failure(e: &Error) -> (u16, Value) {
    match e {
        Error::LimitReached(a) => (402, json!({"error": "insufficient_credits", "requestId": a.request_id})),
        // the customer (or the meter, or the plan) does not exist in the Vunly project: that is a 404, not an outage
        Error::InvalidRequest(a) if a.status == 404 => (404, json!({"error": "not_found", "code": e.code()})),
        Error::InvalidRequest(_) => (400, json!({"error": "invalid_request", "code": e.code()})),
        // this service's own key is wrong or lacks a scope: a deployment problem, not the caller's
        Error::Auth(_) => (502, json!({"error": "billing_misconfigured", "code": e.code()})),
        _ => (503, json!({"error": "billing_unavailable", "code": e.code()})),
    }
}

/// One fresh id per request: time, process id and a counter, so two requests never share one.
fn new_request_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("req_{nanos:x}_{:x}_{n:x}", std::process::id())
}
