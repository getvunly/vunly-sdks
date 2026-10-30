//! What the HTTP library writes to the `log` facade while the client runs: the API key must not be in it.
//! Own test binary, so the global logger is this one.

mod common;

use std::sync::Mutex;
use std::time::Duration;

use common::{Action, Server, KEY};
use vunly::Client;

static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// The logger and its level are process-global: the two tests take turns.
static TURN: Mutex<()> = Mutex::new(());

struct Capture;
impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, r: &log::Record<'_>) {
        LINES.lock().unwrap().push(format!("{} {}", r.level(), r.args()));
    }
    fn flush(&self) {}
}

fn run_and_capture(level: log::LevelFilter) -> Vec<String> {
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let _ = log::set_logger(&Capture);
    log::set_max_level(level);
    LINES.lock().unwrap().clear();
    let s = Server::start(|_, _| Action::json(200, r#"{"credits":5,"consumed":1,"granted":6}"#));
    let c = Client::builder(KEY)
        .base_url(s.url.clone())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    c.credits().balance("cus_1").unwrap();
    let lines = LINES.lock().unwrap().clone();
    log::set_max_level(log::LevelFilter::Off);
    lines
}

#[test]
fn the_key_is_not_in_the_debug_log() {
    let lines = run_and_capture(log::LevelFilter::Debug);
    assert!(
        lines.iter().any(|l| l.contains("HEADERS ARE REDACTED")),
        "the debug request dump was expected with the sensitive header redacted: {lines:?}"
    );
    let joined = lines.join("\n");
    assert!(!joined.contains(KEY) && !joined.contains("vk_test"), "{joined}");
}

/// Documents a limit of the HTTP library rather than a guarantee: ureq-proto 0.6.5 hex-dumps the serialized request, headers
/// included, in 16-byte rows at TRACE (`util.rs` `log_data`), before any transport sees it. The header cannot be kept out of
/// that dump without leaving it off the wire, so the README tells callers never to enable TRACE for `ureq_proto` in a deployed program.
/// This test pins the observation so a dependency update that changes it is noticed (and the README warning can go).
#[test]
fn trace_of_the_http_library_is_known_to_dump_the_request_bytes() {
    let lines = run_and_capture(log::LevelFilter::Trace);
    let ascii: String = lines
        .iter()
        .filter(|l| l.starts_with("TRACE"))
        .filter_map(|l| l.rsplit(' ').next())
        .collect();
    eprintln!("TRACE ascii columns: {ascii}");
    assert!(
        ascii.contains(&KEY[..16]),
        "TRACE no longer dumps the key: update the README warning and make this an absence test"
    );
}
