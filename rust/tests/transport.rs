//! Transport behaviour against a scripted local server: bounds, caps, no replay, retries by class, timeouts.

mod common;

use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use common::{closed_port_url, Action, Server, KEY};
use vunly::{CancelToken, Client, Error, ErrorClass, RecordParams, MAX_BODY_BYTES};

const BALANCE: &str = r#"{"credits":5,"consumed":1,"granted":6}"#;
const USAGE: &str = r#"{"event":{"id":"evt_1","customerId":"cus_1","kind":"debit","credits":-1,"createdAt":"2026-10-28T00:00:00Z"},"balance":4}"#;
const TOKEN: &str =
    r#"{"id":"tok_1","token":"ct_synthetic_secret","customerId":"cus_1","expiresAt":"2026-10-28T01:00:00Z"}"#;

fn client(s: &Server, retries: u32) -> Client {
    Client::builder(KEY)
        .base_url(s.url.clone())
        .max_retries(retries)
        .retry_backoff(Duration::from_millis(5))
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

fn record(c: &Client) -> Result<vunly::UsageResult, Error> {
    c.usage()
        .record(RecordParams::new("req_12345678", "cus_1", "tokens", 10))
}

// ---- no silent replay ----------------------------------------------------------------------------------------------

#[test]
fn a_keyed_post_is_not_replayed_when_the_connection_drops() {
    let s = Server::start(|_, _| Action::Drop);
    let err = record(&client(&s, 0)).unwrap_err();
    assert_eq!(err.class(), ErrorClass::Unavailable);
    assert_eq!(err.status(), 0);
    assert_eq!(s.settled_count(), 1, "the HTTP library repeated the request on its own");
}

#[test]
fn a_keyed_post_is_not_replayed_on_a_pooled_connection_that_the_server_dropped() {
    // request 0 is answered (keep-alive), request 1 arrives on the same pooled connection and is dropped unanswered;
    // a library that replays on a fresh connection would show up as request 2.
    let s = Server::start(|i, _| match i {
        0 => Action::json(200, BALANCE),
        1 => Action::Drop,
        _ => Action::json(200, USAGE),
    });
    let c = client(&s, 0);
    c.credits().balance("cus_1").unwrap();
    let err = record(&c).unwrap_err();
    assert_eq!(err.class(), ErrorClass::Unavailable);
    assert_eq!(s.settled_count(), 2, "the HTTP library replayed a keyed POST");
    let seen = s.seen();
    assert_eq!(
        seen[0].conn, seen[1].conn,
        "the second request was meant to reuse the pooled connection"
    );
}

#[test]
fn an_unkeyed_post_is_not_replayed_on_a_pooled_connection_that_the_server_dropped() {
    let s = Server::start(|i, _| match i {
        0 => Action::json(200, BALANCE),
        1 => Action::Drop,
        _ => Action::json(200, TOKEN),
    });
    let c = client(&s, 2); // retries are ON: class X must still not be repeated after an ambiguous failure
    c.credits().balance("cus_1").unwrap();
    let err = c.customer_tokens().create("cus_1", None).unwrap_err();
    assert_eq!(err.class(), ErrorClass::Unavailable);
    assert_eq!(s.settled_count(), 2, "an unkeyed POST was repeated after a lost answer");
    let seen = s.seen();
    assert_eq!(
        seen[0].conn, seen[1].conn,
        "the second request was meant to reuse the pooled connection"
    );
}

#[test]
fn a_lost_answer_is_retried_with_the_same_id_and_body() {
    let s = Server::start(|i, _| {
        if i == 0 {
            Action::Drop
        } else {
            Action::json_with(200, &[("Idempotent-Replayed", "true")], USAGE)
        }
    });
    let res = record(&client(&s, 2)).unwrap();
    assert!(res.replayed);
    assert_eq!(res.remaining, 4);
    let seen = s.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].header("idempotency-key"), Some("req_12345678"));
    assert_eq!(seen[1].header("idempotency-key"), Some("req_12345678"));
    assert_eq!(seen[0].body, seen[1].body);
}

// ---- retries by class ----------------------------------------------------------------------------------------------

#[test]
fn reads_retry_server_errors_then_succeed() {
    let s = Server::start(|i, _| {
        if i < 2 {
            Action::error(503, "unavailable", "busy")
        } else {
            Action::json(200, BALANCE)
        }
    });
    let b = client(&s, 2).credits().balance("cus_1").unwrap();
    assert_eq!((b.remaining, b.consumed, b.granted), (5, 1, 6));
    assert_eq!(s.count(), 3);
}

#[test]
fn retries_are_bounded() {
    let s = Server::start(|_, _| Action::error(500, "internal_error", "boom"));
    let err = client(&s, 2).credits().balance("cus_1").unwrap_err();
    assert_eq!(
        (err.class(), err.status(), err.code()),
        (ErrorClass::Unavailable, 500, "internal_error")
    );
    assert_eq!(s.count(), 3, "1 attempt + 2 retries");
    let s0 = Server::start(|_, _| Action::error(500, "internal_error", "boom"));
    client(&s0, 0).credits().balance("cus_1").unwrap_err();
    assert_eq!(s0.count(), 1, "retries off");
}

#[test]
fn final_statuses_are_never_retried() {
    for (status, class) in [
        (400, ErrorClass::InvalidRequest),
        (401, ErrorClass::Auth),
        (402, ErrorClass::LimitReached),
        (403, ErrorClass::Auth),
        (404, ErrorClass::InvalidRequest),
        (409, ErrorClass::InvalidRequest),
        (501, ErrorClass::Unavailable),
    ] {
        let s = Server::start(move |_, _| Action::error(status, "some_code", "no"));
        let err = record(&client(&s, 2)).unwrap_err();
        assert_eq!((err.class(), err.status()), (class, status));
        assert_eq!(s.count(), 1, "status {status} was retried");
    }
}

#[test]
fn class_x_is_retried_only_after_429() {
    let s = Server::start(|i, _| {
        if i == 0 {
            Action::error(429, "rate_limited", "slow down")
        } else {
            Action::json(200, TOKEN)
        }
    });
    let t = client(&s, 2).customer_tokens().create("cus_1", None).unwrap();
    assert_eq!(t.token(), "ct_synthetic_secret");
    assert_eq!(s.count(), 2, "a 429 was refused before it ran: repeating is safe");

    for status in [500, 502, 503, 504] {
        let s = Server::start(move |_, _| Action::error(status, "unavailable", "x"));
        let err = client(&s, 2).customer_tokens().create("cus_1", None).unwrap_err();
        assert_eq!(err.class(), ErrorClass::Unavailable);
        assert_eq!(s.count(), 1, "class X was repeated after a {status}");
    }
    let s = Server::start(|_, _| Action::Drop);
    client(&s, 2).customer_tokens().create("cus_1", None).unwrap_err();
    assert_eq!(s.settled_count(), 1, "class X was repeated after a lost answer");
}

#[test]
fn the_plan_volume_stop_is_never_retried() {
    let body =
        r#"{"error":{"code":"rate_limited","message":"monthly volume","details":{"reason":"plan_included_volume"}}}"#;
    for retry_after in ["3", "0"] {
        let s = Server::start(move |_, _| Action::json_with(429, &[("Retry-After", retry_after)], body));
        let started = Instant::now();
        let err = record(&client(&s, 2)).unwrap_err();
        assert!(err.is_plan_stop() && !err.is_retryable());
        assert_eq!((err.class(), err.status()), (ErrorClass::Unavailable, 429));
        assert_eq!(s.count(), 1);
        assert!(started.elapsed() < Duration::from_secs(2), "it waited");
    }
}

#[test]
fn retry_after_is_honoured_up_to_the_cap() {
    let s = Server::start(|i, _| {
        if i == 0 {
            Action::json_with(
                429,
                &[("Retry-After", "1")],
                r#"{"error":{"code":"rate_limited","message":"slow"}}"#,
            )
        } else {
            Action::json(200, BALANCE)
        }
    });
    let started = Instant::now();
    client(&s, 2).credits().balance("cus_1").unwrap();
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_millis(950) && waited < Duration::from_secs(3),
        "{waited:?}"
    );

    // above the 30 s cap: surfaced at once, with retry_after set, instead of hanging the caller
    let s = Server::start(|_, _| {
        Action::json_with(
            429,
            &[("Retry-After", "31")],
            r#"{"error":{"code":"rate_limited","message":"slow"}}"#,
        )
    });
    let started = Instant::now();
    let err = client(&s, 2).credits().balance("cus_1").unwrap_err();
    assert_eq!(err.retry_after(), Some(Duration::from_secs(31)));
    assert!(err.is_retryable());
    assert_eq!(s.count(), 1);
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn a_malformed_success_is_invalid_response_and_not_retried() {
    for body in [
        "{}",
        "not json",
        "[]",
        r#"{"credits":"5","consumed":1,"granted":6}"#,
        r#"{"credits":5.5,"consumed":1,"granted":6}"#,
    ] {
        let s = Server::start(move |_, _| Action::json(200, body));
        let err = client(&s, 2).credits().balance("cus_1").unwrap_err();
        assert_eq!(
            (err.class(), err.code(), err.status()),
            (ErrorClass::Unavailable, "invalid_response", 200),
            "{body}"
        );
        assert_eq!(s.count(), 1);
    }
}

// ---- bounds ---------------------------------------------------------------------------------------------------------

#[test]
fn a_silent_server_is_cut_at_the_timeout() {
    let s = Server::start(|_, _| Action::Hang(Duration::from_secs(20)));
    let c = Client::builder(KEY)
        .base_url(s.url.clone())
        .max_retries(0)
        .timeout(Duration::from_millis(400))
        .build()
        .unwrap();
    let started = Instant::now();
    let err = c.credits().balance("cus_1").unwrap_err();
    let took = started.elapsed();
    assert_eq!(
        (err.class(), err.status(), err.code()),
        (ErrorClass::Unavailable, 0, "unavailable")
    );
    assert!(err.message().contains("did not answer within 400ms"), "{err}");
    assert!(
        took >= Duration::from_millis(350) && took < Duration::from_secs(3),
        "{took:?}"
    );
}

#[test]
fn one_attempt_is_bounded_as_a_whole_a_dripping_body_is_cut() {
    // headers at once, then one byte every 100 ms for 100 bytes: 10 s of dripping against a 500 ms attempt
    let s = Server::start(|_, _| Action::DripBody {
        declared: 100,
        interval: Duration::from_millis(100),
    });
    let c = Client::builder(KEY)
        .base_url(s.url.clone())
        .max_retries(0)
        .timeout(Duration::from_millis(500))
        .build()
        .unwrap();
    let started = Instant::now();
    let err = c.credits().balance("cus_1").unwrap_err();
    let took = started.elapsed();
    assert_eq!((err.class(), err.status()), (ErrorClass::Unavailable, 0));
    assert!(
        took < Duration::from_secs(2),
        "the drip was not cut at the timeout: {took:?}"
    );
}

#[test]
fn one_attempt_is_bounded_as_a_whole_dripping_headers_are_cut() {
    let s = Server::start(|_, _| Action::DripHeaders {
        interval: Duration::from_millis(100),
    });
    let c = Client::builder(KEY)
        .base_url(s.url.clone())
        .max_retries(0)
        .timeout(Duration::from_millis(500))
        .build()
        .unwrap();
    let started = Instant::now();
    let err = c.credits().balance("cus_1").unwrap_err();
    assert_eq!(err.class(), ErrorClass::Unavailable);
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn a_drip_is_retried_like_a_lost_answer() {
    let s = Server::start(|i, _| {
        if i == 0 {
            Action::DripBody {
                declared: 100,
                interval: Duration::from_millis(100),
            }
        } else {
            Action::json(200, BALANCE)
        }
    });
    let c = Client::builder(KEY)
        .base_url(s.url.clone())
        .retry_backoff(Duration::from_millis(5))
        .timeout(Duration::from_millis(400))
        .build()
        .unwrap();
    c.credits().balance("cus_1").unwrap();
    assert_eq!(s.count(), 2);
}

#[test]
fn the_total_timeout_bounds_the_call_with_its_retries() {
    let s = Server::start(|_, _| Action::Hang(Duration::from_secs(20)));
    let c = Client::builder(KEY)
        .base_url(s.url.clone())
        .timeout(Duration::from_secs(5))
        .total_timeout(Duration::from_millis(500))
        .retry_backoff(Duration::from_millis(5))
        .build()
        .unwrap();
    let started = Instant::now();
    c.credits().balance("cus_1").unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn a_success_body_over_the_cap_is_invalid_response_and_reading_stops() {
    let total = 64 * 1024 * 1024;
    let s = Server::start(move |_, _| Action::Stream { total });
    let err = client(&s, 2).credits().balance("cus_1").unwrap_err();
    assert_eq!(
        (err.class(), err.code(), err.status()),
        (ErrorClass::Unavailable, "invalid_response", 200)
    );
    assert!(err.message().contains("16 MB"));
    assert_eq!(s.count(), 1, "an oversized body was retried");
    thread::sleep(Duration::from_millis(300));
    let written = s.written.load(Ordering::SeqCst);
    // the client stopped reading at the cap: the server could only push what the socket buffers hold on top of it
    assert!(
        written < MAX_BODY_BYTES + 12 * 1024 * 1024,
        "the server wrote {written} bytes: the client kept reading"
    );
    assert!(
        written >= MAX_BODY_BYTES,
        "the cap was hit before it should have been: {written}"
    );
}

#[test]
fn a_declared_length_over_the_cap_is_refused_without_reading() {
    let declared = 100 * 1024 * 1024;
    let s = Server::start(move |_, _| Action::BigBody { status: 200, declared });
    let err = client(&s, 0).credits().balance("cus_1").unwrap_err();
    assert_eq!((err.code(), err.status()), ("invalid_response", 200));
    thread::sleep(Duration::from_millis(300));
    assert!(
        s.written.load(Ordering::SeqCst) < 16 * 1024 * 1024,
        "the client read a body it knew was over the cap"
    );
}

#[test]
fn an_error_status_with_a_huge_body_keeps_its_class_by_status() {
    for (status, class) in [
        (500, ErrorClass::Unavailable),
        (400, ErrorClass::InvalidRequest),
        (401, ErrorClass::Auth),
        (402, ErrorClass::LimitReached),
    ] {
        let s = Server::start(move |_, _| Action::BigBody {
            status,
            declared: 20 * 1024 * 1024,
        });
        let err = client(&s, 0).credits().balance("cus_1").unwrap_err();
        assert_eq!((err.class(), err.status()), (class, status));
        assert!(!err.message().is_empty());
    }
}

#[test]
fn a_body_that_breaks_mid_read_is_unavailable_and_retried_like_a_lost_answer() {
    let s = Server::start(|_, _| Action::Truncated {
        declared: 100,
        send: 10,
    });
    let err = client(&s, 2).credits().balance("cus_1").unwrap_err();
    assert_eq!(
        (err.class(), err.status(), err.code()),
        (ErrorClass::Unavailable, 0, "unavailable")
    );
    assert_eq!(s.count(), 3, "a read (class R) is repeated after a lost answer");

    let s = Server::start(|_, _| Action::Truncated {
        declared: 100,
        send: 10,
    });
    client(&s, 2).customer_tokens().create("cus_1", None).unwrap_err();
    assert_eq!(
        s.settled_count(),
        1,
        "class X must not be repeated after a broken answer"
    );

    let s = Server::start(|i, _| {
        if i == 0 {
            Action::Truncated {
                declared: 100,
                send: 10,
            }
        } else {
            Action::json_with(200, &[("Idempotent-Replayed", "true")], USAGE)
        }
    });
    assert!(record(&client(&s, 2)).unwrap().replayed);
    assert_eq!(s.count(), 2);
}

#[test]
fn a_broken_error_body_still_has_its_class() {
    // a 503 whose body is far over the cap: classified by status, and retried as a 5xx
    let s = Server::start(|_, _| Action::BigBody {
        status: 503,
        declared: 30 * 1024 * 1024,
    });
    let err = client(&s, 1).credits().balance("cus_1").unwrap_err();
    assert_eq!((err.class(), err.status()), (ErrorClass::Unavailable, 503));
    assert_eq!(s.count(), 2);
}

// ---- redirects, cancellation, secrecy -------------------------------------------------------------------------------

#[test]
fn redirects_are_not_followed_and_the_key_goes_nowhere_else() {
    let other = Server::start(|_, _| Action::json(200, BALANCE));
    let target = format!("{}/v1/customers/cus_1/balance", other.url);
    let s = Server::start(move |_, _| Action::json_with(302, &[("Location", &target)], ""));
    let err = client(&s, 2).credits().balance("cus_1").unwrap_err();
    // not the caller's request that is wrong (never `validation_error`), and never retried: the address is
    assert_eq!(
        (err.class(), err.status(), err.code()),
        (ErrorClass::Unavailable, 302, "unexpected_redirect")
    );
    assert!(!err.message().contains(&other.url), "the Location must not be repeated");
    assert_eq!(s.count(), 1);
    assert_eq!(other.settled_count(), 0, "the redirect was followed");
}

#[test]
fn cancelling_stops_the_wait_between_retries() {
    let s = Server::start(|_, _| Action::error(503, "unavailable", "busy"));
    let c = Client::builder(KEY)
        .base_url(s.url.clone())
        .retry_backoff(Duration::from_secs(60))
        .build()
        .unwrap();
    let token = CancelToken::new();
    let bound = c.with_cancel_token(&token);
    let t2 = token.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(150));
        t2.cancel();
    });
    let started = Instant::now();
    let err = bound.credits().balance("cus_1").unwrap_err();
    assert_eq!(err.code(), "cancelled");
    assert!(!err.is_retryable());
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(s.count(), 1);
    // an already-cancelled token sends nothing
    assert_eq!(bound.credits().balance("cus_1").unwrap_err().code(), "cancelled");
    assert_eq!(s.count(), 1);
}

#[test]
fn no_error_or_debug_text_contains_the_key() {
    // the server echoes the key back in its message, and also sends a normal error
    let echo =
        format!(r#"{{"error":{{"code":"unauthenticated","message":"bad key {KEY} given","requestId":"req_1"}}}}"#);
    let s = Server::start(move |_, _| Action::json(401, &echo));
    let c = client(&s, 0);
    let err = c.credits().balance("cus_1").unwrap_err();
    // a refused connection, and a base URL that tried to carry credentials
    let refused = Client::builder(KEY)
        .base_url(closed_port_url())
        .max_retries(0)
        .build()
        .unwrap()
        .credits()
        .balance("cus_1")
        .unwrap_err();
    let with_creds = Client::builder(KEY)
        .base_url("http://user:hunter2@127.0.0.1:1")
        .build()
        .unwrap_err();
    let builder = format!("{:?}", Client::builder(KEY));
    for text in [
        format!("{err}"),
        format!("{err:?}"),
        format!("{refused}"),
        format!("{refused:?}"),
        format!("{c:?}"),
        format!("{with_creds}"),
        format!("{with_creds:?}"),
        builder,
    ] {
        assert!(
            !text.contains(KEY) && !text.contains("hunter2") && !text.contains("vk_test"),
            "leaked: {text}"
        );
    }
    assert!(err.message().contains("<hidden>"));
    // the request had the key where it belongs
    assert_eq!(
        s.seen()[0].header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
}

#[test]
fn requests_carry_the_documented_headers_and_nothing_extra() {
    let s = Server::start(|_, _| Action::json(200, USAGE));
    record(&client(&s, 0)).unwrap();
    let r = &s.seen()[0];
    assert_eq!((r.method.as_str(), r.target.as_str()), ("POST", "/v1/usage-events"));
    assert!(r.header("user-agent").unwrap().starts_with("vunly-rust/"));
    assert_eq!(r.header("content-type"), Some("application/json"));
    assert_eq!(r.header("accept"), Some("application/json"));
    assert_eq!(r.header("idempotency-key"), Some("req_12345678"));
    assert!(r.header("vunly-project").is_none() && r.header("vunly-environment").is_none());
    assert!(
        r.header("accept-encoding").is_none(),
        "no compression feature: the body is plain"
    );
    assert_eq!(r.body, r#"{"customerId":"cus_1","meter":"tokens","quantity":10}"#);
}

#[test]
fn cleartext_http_to_a_local_api_works_on_one_reused_connection() {
    let s = Server::start(|_, _| Action::json(200, BALANCE));
    let c = client(&s, 0);
    for _ in 0..3 {
        assert_eq!(c.credits().balance("cus_1").unwrap().remaining, 5);
    }
    assert_eq!(s.count(), 3);
}

#[test]
fn a_client_is_shared_between_threads() {
    let s = Server::start(|_, _| Action::json(200, BALANCE));
    let c = client(&s, 0);
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let c = c.clone();
            thread::spawn(move || {
                (0..5)
                    .map(|_| c.credits().balance("cus_1").unwrap().remaining)
                    .sum::<i64>()
            })
        })
        .collect();
    let total: i64 = handles.into_iter().map(|h| h.join().unwrap()).sum();
    assert_eq!(total, 8 * 5 * 5);
}

#[test]
fn an_unrepresentable_retry_after_is_a_very_long_wait_not_an_absent_header() {
    let huge = "9".repeat(400);
    for header in ["1e999", "99999999999", "1e12", huge.as_str()] {
        let header = header.to_owned();
        let s = Server::start(move |_, _| {
            Action::json_with(
                429,
                &[("Retry-After", &header)],
                r#"{"error":{"code":"rate_limited","message":"slow"}}"#,
            )
        });
        let started = Instant::now();
        let err = client(&s, 2).credits().balance("cus_1").unwrap_err();
        assert_eq!(err.retry_after(), Some(Duration::from_secs(86_400)));
        assert_eq!((err.class(), err.status()), (ErrorClass::Unavailable, 429));
        assert_eq!(s.settled_count(), 1, "retried early");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}

// ---- hostile servers ------------------------------------------------------------------------------------------------

#[test]
fn server_supplied_text_is_cleaned_and_bounded() {
    let evil_message = format!(
        "line1\r\nFORGED: ok\u{1b}[31m\u{202e}rtl\u{200b}zw\u{85}c1\u{7f}del {}",
        "x".repeat(2000)
    );
    let body = vunly::serde_json::json!({"error": {
        "code": format!("co\u{202e}de\n{}", "c".repeat(200)),
        "message": evil_message,
        "requestId": format!("req\r\n{}", "r".repeat(500)),
    }})
    .to_string();
    let s = Server::start(move |_, _| Action::json(409, &body));
    let err = client(&s, 0).credits().balance("cus_1").unwrap_err();
    let hostile =
        |c: char| c.is_control() || ('\u{200b}'..='\u{200f}').contains(&c) || ('\u{202a}'..='\u{202e}').contains(&c);
    for text in [
        err.message().to_owned(),
        err.code().to_owned(),
        err.request_id().unwrap().to_owned(),
        err.to_string(),
    ] {
        assert!(!text.chars().any(hostile), "{text:?}");
    }
    assert!(err.message().chars().count() <= 503 && err.code().chars().count() <= 67);
    assert!(err.request_id().unwrap().chars().count() <= 131);
    assert!(err.message().starts_with("line1FORGED: ok"));
    // a request id from the header is cleaned and bounded too
    let s = Server::start(|_, _| Action::json_with(500, &[("Request-Id", &"h".repeat(400))], ""));
    let err = client(&s, 0).credits().balance("cus_1").unwrap_err();
    assert!(err.request_id().unwrap().chars().count() <= 131);
}

#[test]
fn a_huge_or_deep_json_body_ends_in_an_sdk_error_not_a_crash() {
    let deep = "[".repeat(2_000_000); // a recursive-descent parser overflows its stack on this
    let wide = format!("[{}1]", "1,".repeat(7_000_000)); // 14 MB, under the cap, millions of values
    let nested_object = format!("{}{}", r#"{"a":"#.repeat(100_000), "1");
    for body in [deep, wide, nested_object] {
        let s = Server::start(move |_, _| Action::json(200, &body));
        let err = client(&s, 2).credits().balance("cus_1").unwrap_err();
        assert_eq!(
            (err.class(), err.code(), err.status()),
            (ErrorClass::Unavailable, "invalid_response", 200)
        );
        assert_eq!(s.count(), 1, "not retried");
    }
    // as an error body it is ignored and the status still decides the class
    let deep = "[".repeat(2_000_000);
    let s = Server::start(move |_, _| Action::json(401, &deep));
    assert_eq!(
        client(&s, 0).credits().balance("cus_1").unwrap_err().class(),
        ErrorClass::Auth
    );
}

#[test]
fn the_value_bound_counts_tokens_outside_strings_only() {
    // a legitimate answer whose string holds 300 000 commas and brackets is NOT refused
    let note = ",[{".repeat(100_000);
    let body = format!(r#"{{"credits":5,"consumed":1,"granted":6,"note":"{note}"}}"#);
    let s = Server::start(move |_, _| Action::json(200, &body));
    assert_eq!(client(&s, 0).credits().balance("cus_1").unwrap().remaining, 5);
    // escaped quotes inside a string do not end it
    let body = format!(
        r#"{{"credits":5,"consumed":1,"granted":6,"note":"\"{}\\"}}"#,
        ",".repeat(300_000)
    );
    let s = Server::start(move |_, _| Action::json(200, &body));
    assert_eq!(client(&s, 0).credits().balance("cus_1").unwrap().remaining, 5);
    // 990 000 keys in one object: refused before the tree is built
    let keys: String = (0..990_000).map(|i| format!(r#""k{i}":1,"#)).collect();
    let body = format!(r#"{{{keys}"credits":5,"consumed":1,"granted":6}}"#);
    let s = Server::start(move |_, _| Action::json(200, &body));
    let err = client(&s, 0).credits().balance("cus_1").unwrap_err();
    assert_eq!((err.code(), err.status()), ("invalid_response", 200));
}

#[test]
fn the_key_never_reaches_an_error_through_any_server_supplied_field() {
    // the server echoes the key in the message, the code, the request id and the details, once whole and once with
    // zero-width / control characters inside it (cleaning must happen BEFORE the replacement), and at the length cut
    let zw: String = KEY.chars().flat_map(|c| [c, '\u{200b}']).collect();
    let ctl = format!("{}\u{7f}{}", &KEY[..8], &KEY[8..]);
    let pad = "p".repeat(500 - 3); // the key straddles the 500-character message bound
    let msg = format!("{pad}{KEY}");
    let body = vunly::serde_json::json!({"error": {
        "code": zw, "message": format!("bad {zw} and {ctl} and {KEY} {msg}"),
        "requestId": ctl.clone(),
        "details": {"echo": KEY, "zw": zw, (zw.clone()): [ctl.clone(), {"deep": KEY}]},
    }})
    .to_string();
    let s = Server::start(move |_, _| Action::json(409, &body));
    let err = client(&s, 0).credits().balance("cus_1").unwrap_err();
    let shown = format!("{err} | {err:?} | {:?} | {}", err.details(), err.code());
    let probes = [KEY, &KEY[..24], &KEY[KEY.len() - 24..]];
    for p in probes {
        assert!(
            !shown.contains(p),
            "a piece of the key ({p}) reached the error text: {shown}"
        );
    }
    assert!(shown.contains("<hidden>"));
    // the same through the Request-Id header
    let s = Server::start(|_, _| Action::json_with(500, &[("Request-Id", KEY)], ""));
    let err = client(&s, 0).credits().balance("cus_1").unwrap_err();
    assert!(!format!("{err} {err:?}").contains("vk_test"), "{err}");
}

#[test]
fn only_a_transient_unavailable_is_retryable() {
    let s = Server::start(|_, _| Action::json_with(302, &[("Location", "http://elsewhere.invalid/")], ""));
    let redirect = client(&s, 0).credits().balance("cus_1").unwrap_err();
    assert_eq!(redirect.code(), "unexpected_redirect");
    assert!(!redirect.is_retryable());
    let s = Server::start(|_, _| Action::json(200, "{}"));
    let invalid = client(&s, 0).credits().balance("cus_1").unwrap_err();
    assert_eq!(invalid.code(), "invalid_response");
    assert!(!invalid.is_retryable());
    let s = Server::start(|_, _| Action::error(503, "unavailable", "busy"));
    assert!(client(&s, 0).credits().balance("cus_1").unwrap_err().is_retryable());
}

#[test]
fn dot_segments_and_empty_ones_are_refused_before_any_request() {
    // the API normalises `%2E` back to a dot segment (GET /v1/customers/%2E/balance answered for another customer), so
    // `.` and `..` are refused locally, like the empty id; `...` is an ordinary id and is sent
    let s = Server::start(|_, _| Action::json(200, BALANCE));
    let c = client(&s, 0);
    c.credits().balance("...").unwrap();
    let targets: Vec<String> = s.seen().iter().map(|r| r.target.clone()).collect();
    assert_eq!(targets, ["/v1/customers/.../balance"]);
    for id in ["", ".", ".."] {
        for result in [
            c.credits().balance(id).map(|_| ()),
            c.customers().get(id).map(|_| ()),
            c.plans().subscribe(id, "p").map(|_| ()),
            c.plans().cancel(id).map(|_| ()),
            c.customer_tokens().revoke(id).map(|_| ()),
            c.customer_tokens().create(id, None).map(|_| ()),
            c.credits().ledger(id, &vunly::ListOptions::default()).map(|_| ()),
            c.credits()
                .grant(vunly::GrantParams::new("grant-0001", id, 5))
                .map(|_| ()),
        ] {
            let err = result.unwrap_err();
            assert_eq!((err.status(), err.code()), (400, "validation_error"), "id {id:?}");
            assert!(err.to_string().contains("must not be"), "{err}");
        }
    }
    assert_eq!(s.settled_count(), 1, "a dot or empty segment sent a request");
}

#[test]
fn an_empty_ledger_page_loop_also_ends() {
    let s = Server::start(|i, _| {
        if i < 100 {
            Action::json(200, &format!(r#"{{"data":[],"nextCursor":"f{i}"}}"#))
        } else {
            Action::json(200, r#"{"data":[],"nextCursor":null}"#)
        }
    });
    let c = client(&s, 0);
    let results: Vec<_> = c.credits().iter_ledger("cus_1", Some(1)).take(50).collect();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].as_ref().unwrap_err().code(), "invalid_response");
    assert_eq!(s.count(), 3);
}
