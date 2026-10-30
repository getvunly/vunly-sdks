//! The decode bound under a small memory budget. A file of its own so that the process contains nothing else: the
//! peak-memory reading (`VmHWM`, Linux) is then this test's alone.

mod common;

use std::time::Duration;

use common::{Action, Server, KEY};
use vunly::{Client, ErrorClass};

/// Peak resident memory of this process, in bytes (Linux; `None` elsewhere).
fn peak_rss() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb: u64 = status
        .lines()
        .find(|l| l.starts_with("VmHWM:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some(kb * 1024)
}

#[test]
fn the_decode_bound_holds_under_small_memory_for_the_shapes_that_killed_the_php_sdk() {
    // 2.4 MB of tiny containers: if a tree were built, `[[1],[1],...]` is 600 000 arrays and `[{"a":1},...]` 300 000 maps
    let arrays = format!("[{}[1]]", "[1],".repeat(600_000));
    let objects = format!("[{}{{\"a\":1}}]", "{\"a\":1},".repeat(300_000));
    assert!((2_400_000..2_500_000).contains(&arrays.len()), "{}", arrays.len());
    assert!((2_400_000..2_500_000).contains(&objects.len()), "{}", objects.len());
    let before = peak_rss();
    for body in [arrays, objects] {
        let s = Server::start(move |_, _| Action::json(200, &body));
        let c = Client::builder(KEY)
            .base_url(s.url.clone())
            .max_retries(0)
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap();
        let err = c.credits().balance("cus_1").unwrap_err();
        assert_eq!(
            (err.class(), err.code(), err.status()),
            (ErrorClass::Unavailable, "invalid_response", 200)
        );
    }
    // An ERROR body is read up to 64 KiB only (the status classifies; the code and request id sit at the start of a
    // small body). A 781 KB error body whose `details` is a big tree used to cost over 100 MB on the error path.
    let items = "\"abcdefg\",".repeat(78_000);
    let big_error = format!(
        r#"{{"error":{{"code":"custom_code","message":"m","requestId":"r","details":{{"list":[{items}"z"]}}}}}}"#
    );
    assert!((780_000..790_000).contains(&big_error.len()), "{}", big_error.len());
    let s = Server::start(move |_, _| Action::json(500, &big_error));
    let c = Client::builder(KEY)
        .base_url(s.url.clone())
        .max_retries(0)
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap();
    let err = c.credits().balance("cus_1").unwrap_err();
    assert_eq!((err.class(), err.status()), (ErrorClass::Unavailable, 500));
    // the oversized body is ignored: the code is the one the status implies, not the one in the unread body
    assert_eq!(err.code(), "internal_error");
    // while a body under the limit is used in full
    let small = r#"{"error":{"code":"custom_code","message":"m","requestId":"r","details":{"k":1}}}"#;
    let s = Server::start(move |_, _| Action::json(500, small));
    let c = Client::builder(KEY)
        .base_url(s.url.clone())
        .max_retries(0)
        .build()
        .unwrap();
    let err = c.credits().balance("cus_1").unwrap_err();
    assert_eq!((err.code(), err.request_id()), ("custom_code", Some("r")));
    assert_eq!(err.details().unwrap()["k"], 1);

    let (Some(before), Some(after)) = (before, peak_rss()) else {
        return; // no /proc: the error assertions above still hold
    };
    // the bodies, the server's copy and a few buffers are tens of MB at most; a built tree is hundreds
    let grew = after.saturating_sub(before);
    assert!(grew < 48 * 1024 * 1024, "peak memory grew by {} MB", grew >> 20);
}
