//! The shared webhook vectors (`tests/webhook-vectors.json`, an identical copy in every SDK). The signatures were computed
//! outside any SDK. "expect" is "ok", a `WebhookError` reason, or "config" (`InvalidSecret`, raised before the signature is
//! looked at). A null header is the empty string here: the function takes a `&str`, and both mean a missing header.

use std::time::{Duration, UNIX_EPOCH};

use vunly::serde_json::Value;
use vunly::{verify_webhook_at, WebhookError};

#[test]
fn every_shared_webhook_vector_gets_its_answer() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/webhook-vectors.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let file: Value = vunly::serde_json::from_str(&text).unwrap();
    let now = UNIX_EPOCH + Duration::from_secs(file["now"].as_u64().unwrap());
    let vectors = file["vectors"].as_array().unwrap();
    assert!(vectors.len() >= 20, "only {} vectors", vectors.len());
    let mut failures = Vec::new();
    for v in vectors {
        let name = v["name"].as_str().unwrap();
        let secrets: Vec<&str> = v["secrets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap())
            .collect();
        let got = match verify_webhook_at(
            v["payload"].as_str().unwrap().as_bytes(),
            v["header"].as_str().unwrap_or(""),
            &secrets,
            now,
        ) {
            Ok(_) => "ok",
            Err(WebhookError::InvalidSecret) => "config",
            Err(e) => e.reason(),
        };
        let want = v["expect"].as_str().unwrap();
        if got != want {
            failures.push(format!("{name}: got {got}, want {want}"));
        }
    }
    assert!(
        failures.is_empty(),
        "shared webhook vectors fail:\n{}",
        failures.join("\n")
    );
}
