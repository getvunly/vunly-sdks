//! The calls: what each sends and how each answer is read, against a scripted local server.

mod common;

use std::time::Duration;

use common::{Action, Server, KEY};
use vunly::{
    CheckParams, Client, ConfigError, Error, ErrorClass, Estimate, GrantParams, ListOptions, PlanParams, RecordParams,
};

fn client(s: &Server) -> Client {
    Client::builder(KEY)
        .base_url(s.url.clone())
        .max_retries(0)
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

const ENTRY: &str = r#"{"id":"led_1","customerId":"cus_1","kind":"grant","credits":100,"reason":"top-up","idempotencyKey":"grant_12345","createdAt":"2026-10-28T00:00:00Z"}"#;
const USAGE: &str = r#"{"event":{"id":"evt_1","customerId":"cus_1","kind":"debit","credits":-2,"meter":"tokens","quantity":1050,"createdAt":"2026-10-28T00:00:00Z"},"balance":98}"#;

#[test]
fn the_client_needs_a_key_and_an_address() {
    assert_eq!(Client::new("", "http://x").unwrap_err(), ConfigError::EmptyApiKey);
    assert_eq!(Client::new("\n", "http://x").unwrap_err(), ConfigError::EmptyApiKey);
    assert_eq!(Client::new("\r\n", "http://x").unwrap_err(), ConfigError::EmptyApiKey);
    assert_eq!(Client::new("   ", "http://x").unwrap_err(), ConfigError::InvalidApiKey);
    assert_eq!(
        Client::new("vk_test_synthetic_a\nb", "http://x").unwrap_err(),
        ConfigError::InvalidApiKey
    );
    assert_eq!(Client::new(KEY, "").unwrap_err(), ConfigError::EmptyBaseUrl);
    assert_eq!(Client::new(KEY, "host").unwrap_err(), ConfigError::InvalidBaseUrl);
    // non-ASCII addresses used to panic on a byte slice; they are an error now (an IDN host is given as punycode)
    for url in [
        "http://host\u{1F600}",
        "http://\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}",
        "\u{e9}\u{e9}\u{e9}",
        "http\u{e9}://x",
        "https://h/v\u{e9}",
        "\u{1F600}\u{1F600}",
    ] {
        assert_eq!(
            Client::new(KEY, url).unwrap_err(),
            ConfigError::InvalidBaseUrl,
            "{url:?}"
        );
    }
    assert!(Client::new(KEY, "https://xn--bcher-kva.example").is_ok());
    assert_eq!(Client::builder(KEY).build().unwrap_err(), ConfigError::EmptyBaseUrl);
    assert_eq!(
        Client::builder(KEY)
            .base_url("http://127.0.0.1:1")
            .timeout(Duration::ZERO)
            .build()
            .unwrap_err(),
        ConfigError::InvalidTimeout
    );
    let c = Client::new(KEY, "https://api.example.test/v1/").unwrap();
    assert_eq!(c.base_url(), "https://api.example.test");
    let shown = format!("{c:?}");
    assert!(shown.contains("<hidden>") && !shown.contains(KEY), "{shown}");
}

#[test]
fn record_sends_the_key_the_id_and_reports_overage_and_replay() {
    let s = Server::start(|i, _| match i {
        0 => Action::json_with(200, &[("Request-Id", "req_a"), ("Vunly-Plan-Overage", "true")], USAGE),
        _ => Action::json_with(
            200,
            &[
                ("Request-Id", "req_b"),
                ("Idempotent-Replayed", "true"),
                ("Vunly-Plan-Overage", "true"),
            ],
            USAGE,
        ),
    });
    let c = client(&s);
    let p = RecordParams::new("req_2026_0001", "cus_1", "tokens", 1050);
    let first = c.usage().record(p.clone()).unwrap();
    assert_eq!((first.remaining, first.replayed, first.plan_overage), (98, false, true));
    assert_eq!(first.request_id.as_deref(), Some("req_a"));
    assert_eq!(
        (
            first.event.credits,
            first.event.kind.as_str(),
            first.event.meter.as_deref(),
            first.event.quantity
        ),
        (-2, "debit", Some("tokens"), Some(1050))
    );
    let again = c.usage().record(p).unwrap();
    assert!(
        again.replayed && !again.plan_overage,
        "a replay is a success and never an overage"
    );
}

#[test]
fn a_bad_id_is_refused_before_any_request() {
    let s = Server::start(|_, _| Action::json(200, USAGE));
    let c = client(&s);
    for id in ["", "short", "has space in it", &"x".repeat(129)] {
        let err = c
            .usage()
            .record(RecordParams::new(id, "cus_1", "tokens", 1))
            .unwrap_err();
        assert_eq!(
            (err.class(), err.status(), err.code()),
            (ErrorClass::InvalidRequest, 400, "validation_error")
        );
        assert!(err.details().unwrap()["fields"]["id"].is_string());
        let err = c.credits().grant(GrantParams::new(id, "cus_1", 5)).unwrap_err();
        assert_eq!(err.code(), "validation_error");
    }
    assert_eq!(s.settled_count(), 0, "a request was sent with a bad id");
}

#[test]
fn limit_reached_carries_the_balance_and_is_not_retried() {
    let s = Server::start(|_, _| {
        Action::json(
            402,
            r#"{"error":{"code":"insufficient_credits","message":"not enough","requestId":"req_9","details":{"balance":3,"required":10}}}"#,
        )
    });
    let c = Client::builder(KEY)
        .base_url(s.url.clone())
        .max_retries(2)
        .retry_backoff(Duration::from_millis(5))
        .build()
        .unwrap();
    let err = c
        .usage()
        .record(RecordParams::new("req_12345678", "cus_1", "tokens", 10))
        .unwrap_err();
    assert!(matches!(err, Error::LimitReached(_)));
    assert_eq!((err.balance(), err.required()), (Some(3), Some(10)));
    assert_eq!(
        (err.status(), err.code(), err.request_id()),
        (402, "insufficient_credits", Some("req_9"))
    );
    assert!(!err.is_retryable());
    assert_eq!(s.count(), 1);
    assert_eq!(
        err.to_string(),
        "vunly: not enough (402 insufficient_credits) request req_9"
    );
}

#[test]
fn error_bodies_without_json_are_classified_by_status() {
    for (status, class, code) in [
        (401, ErrorClass::Auth, "unauthenticated"),
        (403, ErrorClass::Auth, "unauthenticated"),
        (402, ErrorClass::LimitReached, "insufficient_credits"),
        (404, ErrorClass::InvalidRequest, "validation_error"),
        (429, ErrorClass::Unavailable, "rate_limited"),
        (502, ErrorClass::Unavailable, "internal_error"),
    ] {
        let s = Server::start(move |_, _| Action::json(status, "<html>proxy</html>"));
        let err = client(&s).customers().get("cus_1").unwrap_err();
        assert_eq!((err.class(), err.status(), err.code()), (class, status, code));
    }
}

#[test]
fn an_unknown_error_code_is_the_generic_error_of_its_status() {
    let s = Server::start(|_, _| Action::error(409, "brand_new_code", "conflict"));
    let err = client(&s).customers().get("x").unwrap_err();
    assert_eq!(
        (err.class(), err.code()),
        (ErrorClass::InvalidRequest, "brand_new_code")
    );
}

#[test]
fn a_wrong_address_gets_a_hint() {
    let s = Server::start(|_, _| Action::error(404, "not_found", "No such route"));
    let err = client(&s).customers().get("x").unwrap_err();
    assert!(err.message().contains("check the API address"), "{err}");
}

#[test]
fn unknown_response_fields_are_ignored() {
    let s = Server::start(|_, _| {
        Action::json(
            200,
            r#"{"credits":5,"consumed":1,"granted":6,"somethingNew":{"a":[1,2]},"kind":"future"}"#,
        )
    });
    assert_eq!(client(&s).credits().balance("c").unwrap().remaining, 5);
}

#[test]
fn big_integers_stay_exact() {
    let s = Server::start(|_, _| {
        Action::json(
            200,
            r#"{"credits":9007199254740993,"consumed":0,"granted":9007199254740993}"#,
        )
    });
    assert_eq!(
        client(&s).credits().balance("c").unwrap().remaining,
        9_007_199_254_740_993
    );
}

#[test]
fn customers_upsert_get_list_and_the_lazy_iterator() {
    let customer = |n: u32| {
        format!(r#"{{"id":"cus_{n}","externalId":"ext_{n}","name":"N{n}","createdAt":"2026-10-28T00:00:00Z"}}"#)
    };
    let page = move |items: &[u32], next: Option<&str>| {
        let data: Vec<String> = items.iter().map(|n| customer(*n)).collect();
        let next = next.map_or("null".to_owned(), |c| format!("\"{c}\""));
        format!(r#"{{"data":[{}],"nextCursor":{next}}}"#, data.join(","))
    };
    let s = Server::start(move |_, req| {
        if req.method == "POST" {
            return Action::json(200, &customer(1));
        }
        if req.target.contains("cursor=c2") {
            Action::json(200, &page(&[5], None))
        } else if req.target.contains("cursor=c1") {
            Action::json(200, &page(&[3, 4], Some("c2")))
        } else if req.target.starts_with("/v1/customers?") || req.target == "/v1/customers" {
            Action::json(200, &page(&[1, 2], Some("c1")))
        } else {
            Action::json(200, &customer(7))
        }
    });
    let c = client(&s);
    let up = c.customers().upsert("ext_1", Some("N1")).unwrap();
    assert_eq!(
        (up.id.as_str(), up.external_id.as_str(), up.name.as_deref()),
        ("cus_1", "ext_1", Some("N1"))
    );
    assert_eq!(s.seen()[0].body, r#"{"externalId":"ext_1","name":"N1"}"#);
    c.customers().upsert("ext_2", None).unwrap();
    assert_eq!(s.seen()[1].body, r#"{"externalId":"ext_2"}"#);
    assert_eq!(c.customers().get("cus_7").unwrap().id, "cus_7");

    let before = s.count();
    let p = c
        .customers()
        .list(&ListOptions {
            limit: Some(2),
            cursor: None,
        })
        .unwrap();
    assert_eq!(
        (p.data.len(), p.next_cursor.as_deref(), p.has_more()),
        (2, Some("c1"), true)
    );
    assert!(s.seen().last().unwrap().target.contains("limit=2"));

    // lazy: taking two items fetches one page; draining fetches all three
    let before_iter = s.count();
    let two: Vec<_> = c.customers().iter(Some(2)).take(2).collect::<Result<_, _>>().unwrap();
    assert_eq!(two.len(), 2);
    assert_eq!(s.count() - before_iter, 1, "the iterator fetched ahead");
    let before_all = s.count();
    let all: Vec<_> = c.customers().iter(Some(2)).collect::<Result<_, _>>().unwrap();
    assert_eq!(
        all.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(),
        ["cus_1", "cus_2", "cus_3", "cus_4", "cus_5"]
    );
    assert_eq!(s.count() - before_all, 3);
    assert!(before > 0);
}

#[test]
fn the_iterator_ends_after_the_first_error() {
    let s = Server::start(|i, _| {
        if i == 0 {
            Action::json(
                200,
                r#"{"data":[{"id":"l1","customerId":"c","kind":"grant","credits":1,"createdAt":"t"}],"nextCursor":"n"}"#,
            )
        } else {
            Action::error(500, "internal_error", "boom")
        }
    });
    let c = client(&s);
    let mut it = c.credits().iter_ledger("cus_1", Some(1));
    assert!(it.next().unwrap().is_ok());
    assert!(it.next().unwrap().is_err());
    assert!(it.next().is_none());
    assert_eq!(s.count(), 2);
}

#[test]
fn path_segments_are_escaped_and_the_v1_suffix_is_accepted() {
    let s = Server::start(|_, _| Action::json(200, r#"{"credits":1,"consumed":0,"granted":1}"#));
    let c = Client::builder(KEY).base_url(format!("{}/v1/", s.url)).build().unwrap();
    c.credits().balance("a b/c?d").unwrap();
    assert_eq!(s.seen()[0].target, "/v1/customers/a%20b%2Fc%3Fd/balance");
}

#[test]
fn grant_balance_and_entitlement_check() {
    let s = Server::start(|_, req| match req.target.as_str() {
        "/v1/customers/cus_1/credit-grants" => Action::json(201, ENTRY),
        "/v1/entitlements/check" => Action::json(
            200,
            r#"{"allowed":true,"reason":"ok","remaining":98,"required":2,"plan":null}"#,
        ),
        _ => Action::json(404, ""),
    });
    let c = client(&s);
    let g = c
        .credits()
        .grant(GrantParams::new("grant_12345", "cus_1", 100).reason("top-up"))
        .unwrap();
    assert_eq!(
        (g.credits, g.reason.as_deref(), g.idempotency_key.as_deref()),
        (100, Some("top-up"), Some("grant_12345"))
    );
    assert_eq!(s.seen()[0].body, r#"{"credits":100,"reason":"top-up"}"#);
    assert_eq!(s.seen()[0].header("idempotency-key"), Some("grant_12345"));

    let e = c
        .entitlements()
        .check(CheckParams::new("cus_1", "generate").estimate(Estimate::new("tokens", 1050)))
        .unwrap();
    assert!(e.allowed && e.plan.is_none() && e.required == 2);
    assert_eq!(
        s.seen()[1].body,
        r#"{"customerId":"cus_1","estimate":{"meter":"tokens","quantity":1050},"feature":"generate"}"#
    );
}

#[test]
fn the_entitlement_check_fails_closed() {
    let s = Server::start(|_, _| Action::error(503, "unavailable", "down"));
    let c = Client::builder(KEY)
        .base_url(s.url.clone())
        .max_retries(1)
        .retry_backoff(Duration::from_millis(5))
        .build()
        .unwrap();
    let r = c.entitlements().check(CheckParams::new("cus_1", "generate"));
    assert!(matches!(r, Err(Error::Unavailable(_))));
    assert_eq!(s.count(), 2, "the check is a read: it is retried");
}

#[test]
fn plans_subscriptions_and_meters() {
    let s = Server::start(|_, req| match req.target.as_str() {
        "/v1/meters" => Action::json(200, "{}"),
        "/v1/plans" => Action::json(
            200,
            r#"{"key":"starter","name":"Starter","includedCredits":100,"meter":"tokens","features":["generate"],"updatedAt":"t"}"#,
        ),
        t if t.ends_with("/subscription") && req.method == "POST" => Action::json(
            200,
            r#"{"id":"sub_1","customerId":"cus_1","status":"active","plan":{"key":"starter","name":"Starter","includedCredits":100,"meter":"tokens","features":["generate"]},"periodStart":"a","periodEnd":"b"}"#,
        ),
        _ => Action::json(204, ""),
    });
    let c = client(&s);
    c.plans().upsert_meter("tokens", 1000).unwrap();
    assert_eq!(s.seen()[0].body, r#"{"key":"tokens","unitsPerCredit":1000}"#);
    let plan = c
        .plans()
        .upsert_plan(PlanParams {
            key: "starter".into(),
            name: "Starter".into(),
            included_credits: 100,
            meter: "tokens".into(),
            features: vec!["generate".into()],
        })
        .unwrap();
    assert_eq!(
        (plan.included_credits, plan.features.as_slice()),
        (100, ["generate".to_owned()].as_slice())
    );
    let sub = c.plans().subscribe("cus_1", "starter").unwrap();
    assert_eq!((sub.status.as_str(), sub.plan.key.as_str()), ("active", "starter"));
    c.plans().cancel("cus_1").unwrap();
    let last = s.seen().pop().unwrap();
    assert_eq!(
        (last.method.as_str(), last.target.as_str()),
        ("DELETE", "/v1/customers/cus_1/subscription")
    );
}

#[test]
fn customer_tokens_are_never_printed() {
    let s = Server::start(|_, req| {
        if req.method == "DELETE" {
            Action::json(204, "")
        } else {
            Action::json(
                200,
                r#"{"id":"tok_1","token":"ct_synthetic_val_1","customerId":"cus_1","expiresAt":"2026-10-28T01:00:00Z"}"#,
            )
        }
    });
    let c = client(&s);
    let t = c
        .customer_tokens()
        .create("cus_1", Some(Duration::from_secs(600)))
        .unwrap();
    assert_eq!(s.seen()[0].body, r#"{"customerId":"cus_1","ttlSeconds":600}"#);
    assert_eq!(t.token(), "ct_synthetic_val_1");
    let shown = format!("{t:?} {t:#?}");
    assert!(
        !shown.contains("ct_synthetic_val_1") && shown.contains("tok_1"),
        "{shown}"
    );
    c.customer_tokens().revoke(&t.id).unwrap();
}

#[test]
fn a_repeated_cursor_fails_instead_of_looping() {
    // every page says "there is more: c1", forever
    let s = Server::start(|_, _| {
        Action::json(
            200,
            r#"{"data":[{"id":"cus_1","externalId":"e","createdAt":"t"}],"nextCursor":"c1"}"#,
        )
    });
    let c = client(&s);
    let results: Vec<_> = c.customers().iter(Some(1)).take(10).collect();
    // first page: its item; second page repeats the cursor: its item, then the error, then the end
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 2);
    let err = results.iter().find_map(|r| r.as_ref().err()).expect("an error");
    assert_eq!((err.class(), err.code()), (ErrorClass::Unavailable, "invalid_response"));
    assert_eq!(results.len(), 3, "the iterator went on after the loop");
    assert_eq!(s.count(), 2);
}

#[test]
fn empty_pages_that_promise_more_end_in_invalid_response() {
    // every page is empty and carries a FRESH cursor: not a repeat, but it never ends
    // (it does end after 100 pages, so that a broken iterator fails the assertions instead of hanging the test)
    let s = Server::start(|i, _| {
        if i < 100 {
            Action::json(200, &format!(r#"{{"data":[],"nextCursor":"fresh{i}"}}"#))
        } else {
            Action::json(200, r#"{"data":[],"nextCursor":null}"#)
        }
    });
    let c = client(&s);
    let results: Vec<_> = c.customers().iter(Some(1)).take(50).collect();
    assert_eq!(results.len(), 1, "{results:?}");
    let err = results[0].as_ref().unwrap_err();
    assert_eq!((err.class(), err.code()), (ErrorClass::Unavailable, "invalid_response"));
    assert_eq!(s.count(), 3, "three empty pages, then it stopped");

    // an empty page between pages with data is fine: the streak resets
    let s = Server::start(|i, _| {
        let item = r#"{"id":"cus_1","externalId":"e","createdAt":"t"}"#;
        match i {
            0 | 2 | 4 => Action::json(200, &format!(r#"{{"data":[],"nextCursor":"n{i}"}}"#)),
            1 | 3 => Action::json(200, &format!(r#"{{"data":[{item}],"nextCursor":"n{i}"}}"#)),
            _ => Action::json(200, &format!(r#"{{"data":[{item}],"nextCursor":null}}"#)),
        }
    });
    let all: Vec<_> = client(&s).customers().iter(Some(1)).collect();
    assert_eq!(all.len(), 3);
    assert!(all.iter().all(Result::is_ok));
}

#[test]
fn the_key_is_trimmed_checked_and_cannot_inject_headers() {
    let s = Server::start(|_, _| Action::json(200, r#"{"credits":1,"consumed":0,"granted":1}"#));
    // a trailing newline from `echo` or a secrets file is trimmed
    let key = format!("{KEY}\r\n");
    let c = Client::builder(key).base_url(s.url.clone()).build().unwrap();
    c.credits().balance("c").unwrap();
    assert_eq!(
        s.seen()[0].header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    // control characters inside, non-ASCII, a space, DEL: refused naming the setting, never the value
    for bad in [
        // only ONE trailing LF or CRLF is stripped; space, tab, NUL and a lone CR at either end are refused
        format!(" {KEY}"),
        format!("{KEY} "),
        format!("\t{KEY}"),
        format!("{KEY}\t"),
        format!("{KEY}\r"),
        format!("\r{KEY}"),
        format!("\n{KEY}"),
        format!("{KEY}\n\n"),
        format!("{KEY}\r\r\n"),
        format!("{KEY}\r\n "),
        format!("\0{KEY}"),
        format!("{KEY}\r\nX-Injected: 1"),
        format!("{KEY}\nX-Injected: 1"),
        format!("{KEY} more"),
        format!("{KEY}\u{7f}"),
        format!("{KEY}\u{e9}"),
        format!("{KEY}\0"),
        format!("\u{202e}{KEY}"),
    ] {
        let err = Client::builder(bad.clone())
            .base_url(s.url.clone())
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::InvalidApiKey, "{bad:?}");
        assert!(!err.to_string().contains("vk_test") && !format!("{err:?}").contains("vk_test"));
    }
    assert_eq!(s.count(), 1);
}

#[test]
fn ports_ipv6_and_plain_http_are_checked_at_build() {
    for url in [
        "http://127.0.0.1:abc",
        "http://127.0.0.1:99999",
        "http://127.0.0.1:0",
        "http://127.0.0.1:",
        "http://[::1",
        "https://h:80:90",
    ] {
        // with and without allow_insecure: the shape is wrong either way
        for insecure in [false, true] {
            let r = Client::builder(KEY).base_url(url).allow_insecure(insecure).build();
            assert_eq!(r.unwrap_err(), ConfigError::InvalidBaseUrl, "{url} ({insecure})");
        }
    }
    // plain http: this machine only, unless it is allowed on purpose
    for ok in [
        "http://localhost:8787",
        "http://127.0.0.1:1",
        "http://[::1]:8787",
        "https://api.example.test",
    ] {
        assert!(Client::new(KEY, ok).is_ok(), "{ok}");
    }
    let remote = Client::new(KEY, "http://api.example.test").unwrap_err();
    assert_eq!(remote, ConfigError::InsecureBaseUrl);
    assert!(!remote.to_string().contains("example.test"));
    assert!(Client::builder(KEY)
        .base_url("http://api.example.test")
        .allow_insecure(true)
        .build()
        .is_ok());
}

#[test]
fn a_refused_base_url_is_never_printed() {
    for url in [
        "http://user:hunter2@host",
        "https://host?token=hunter2",
        "https://host#hunter2",
        "hunter2",
    ] {
        let b = Client::builder(KEY).base_url(url);
        let shown = format!("{b:?}");
        assert!(!shown.contains("hunter2") && !shown.contains(KEY), "{shown}");
        let err = b.build().unwrap_err();
        assert_eq!(err, ConfigError::InvalidBaseUrl);
        assert!(!format!("{err} {err:?}").contains("hunter2"));
    }
}

#[test]
fn timeouts_outside_their_range_are_refused() {
    let build = |t: Option<Duration>, total: Option<Duration>, backoff: Option<Duration>| {
        let mut b = Client::builder(KEY).base_url("http://127.0.0.1:1");
        if let Some(t) = t {
            b = b.timeout(t);
        }
        if let Some(t) = total {
            b = b.total_timeout(t);
        }
        if let Some(t) = backoff {
            b = b.retry_backoff(t);
        }
        b.build()
    };
    let ms = Duration::from_millis;
    let s = Duration::from_secs;
    for bad in [Duration::ZERO, ms(1), ms(9), s(601), Duration::MAX] {
        assert_eq!(
            build(Some(bad), None, None).unwrap_err(),
            ConfigError::InvalidTimeout,
            "timeout {bad:?}"
        );
    }
    for bad in [Duration::ZERO, ms(1), ms(9), Duration::MAX] {
        assert_eq!(
            build(None, Some(bad), None).unwrap_err(),
            ConfigError::InvalidTimeout,
            "total {bad:?}"
        );
    }
    assert_eq!(
        build(None, Some(s(3601)), None).unwrap_err(),
        ConfigError::InvalidTimeout
    );
    assert_eq!(build(None, None, Some(s(61))).unwrap_err(), ConfigError::InvalidTimeout);
    assert_eq!(
        build(None, None, Some(Duration::MAX)).unwrap_err(),
        ConfigError::InvalidTimeout
    );
    for good in [ms(10), s(10), s(600)] {
        assert!(build(Some(good), None, None).is_ok());
    }
    assert!(build(None, Some(s(3600)), Some(s(60))).is_ok());
    assert!(
        build(None, None, Some(Duration::ZERO)).is_ok(),
        "no backoff is allowed (tests)"
    );
}

#[test]
fn a_missing_id_cannot_be_expressed_and_an_empty_one_is_refused() {
    // `RecordParams::id` is a `String`: there is no null to pass. The nearest mistake is the empty string.
    let s = Server::start(|_, _| Action::json(200, USAGE));
    let err = client(&s)
        .usage()
        .record(RecordParams::new("", "cus_1", "tokens", 1))
        .unwrap_err();
    assert_eq!((err.status(), err.code()), (400, "validation_error"));
    assert!(err.details().unwrap()["fields"]["id"].is_string());
    assert_eq!(s.settled_count(), 0);
}

#[test]
fn plan_usage_keeps_the_unconfirmed_flag_attached() {
    let s = Server::start(|_, _| {
        Action::json(
            200,
            r#"{"plan":{"key":"free","name":"Free"},"period":{"start":"2026-10-01T00:00:00Z","end":"2026-11-01T00:00:00Z"},"usage":{"events":7},"unconfirmedLimits":{"confirmed":false,"note":"hypothesis","includedMonthlyEvents":1000,"activityRetentionDays":null}}"#,
        )
    });
    let u = client(&s).plan_usage().unwrap();
    assert_eq!((u.plan.key.as_str(), u.events), ("free", 7));
    assert!(!u.unconfirmed_limits.confirmed);
    assert_eq!(
        (
            u.unconfirmed_limits.included_monthly_events,
            u.unconfirmed_limits.activity_retention_days
        ),
        (Some(1000), None)
    );
}
