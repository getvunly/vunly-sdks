//! Webhook signature verification (HMAC-SHA256, 300 s tolerance). No HTTP is involved.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ring::hmac;
use serde_json::Value;

use crate::error::WebhookError;
use crate::json::Read;

/// How far a signed timestamp may be from now (either way) before it counts as stale.
pub const WEBHOOK_TOLERANCE: Duration = Duration::from_secs(300);

/// A verified webhook delivery.
#[derive(Debug, Clone, PartialEq)]
pub struct WebhookEvent {
    /// The event's id.
    pub id: String,
    /// The event type, for example `"credits.low"` (the JSON field `type`).
    pub event_type: String,
    /// RFC 3339, when the payload carries it.
    pub created_at: Option<String>,
    /// The event's data, as sent.
    pub data: Value,
}

/// Checks a delivery against the current time. See [`verify_webhook_at`].
pub fn verify_webhook(payload: &[u8], header: &str, secrets: &[&str]) -> Result<WebhookEvent, WebhookError> {
    verify_webhook_at(payload, header, secrets, SystemTime::now())
}

/// Checks a delivery. `payload` is the RAW request body (the exact bytes, not re-encoded JSON), `header` is the
/// `Vunly-Signature` header (`t=<unix>,v1=<hex>`) and `secrets` are the endpoint's signing secrets: pass two while
/// rotating, a match with any one is enough. The signature is HMAC-SHA256 over `"<t>." + payload`, compared in constant
/// time; a timestamp more than five minutes from `now` in either direction is refused.
///
/// Answer a failed verification with a 400 and do not process the payload.
pub fn verify_webhook_at(
    payload: &[u8],
    header: &str,
    secrets: &[&str],
    now: SystemTime,
) -> Result<WebhookEvent, WebhookError> {
    // an empty or blank secret would make the signature forgeable by anyone; refuse it before looking at anything else
    // A secret must hold at least one printable, non-space ASCII character: a secret of NULs, zero-width characters or a
    // BOM is blank to a person and as good as empty.
    let blank = |s: &&str| !s.chars().any(|c| ('\u{21}'..='\u{7e}').contains(&c));
    if secrets.is_empty() || secrets.iter().any(blank) {
        return Err(WebhookError::InvalidSecret);
    }
    if header.trim().is_empty() {
        return Err(WebhookError::MissingHeader);
    }
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for part in header.split(',') {
        let Some((k, v)) = part.trim().split_once('=') else {
            continue;
        };
        match k {
            "t" => timestamp = Some(v),
            // a v1 that is not 64 hex characters is not a signature: the header is malformed
            "v1" if v.len() == 64 && v.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) => signatures.push(v),
            _ => {}
        }
    }
    let ts = timestamp.ok_or(WebhookError::MalformedHeader)?;
    let t: i64 = ts.parse().map_err(|_| WebhookError::MalformedHeader)?;
    if signatures.is_empty() {
        return Err(WebhookError::MalformedHeader);
    }
    let now_secs = match now.duration_since(UNIX_EPOCH) {
        Ok(d) => i128::from(d.as_secs()),
        Err(e) => -i128::from(e.duration().as_secs()),
    };
    if (now_secs - i128::from(t)).unsigned_abs() > u128::from(WEBHOOK_TOLERANCE.as_secs()) {
        return Err(WebhookError::StaleTimestamp);
    }
    let mut signed = Vec::with_capacity(ts.len() + 1 + payload.len());
    signed.extend_from_slice(ts.as_bytes());
    signed.push(b'.');
    signed.extend_from_slice(payload);
    let mut matched = false;
    for secret in secrets {
        let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
        for sig in &signatures {
            // hmac::verify compares in constant time
            if let Some(bytes) = decode_hex(sig) {
                matched |= hmac::verify(&key, &signed, &bytes).is_ok();
            }
        }
    }
    if !matched {
        return Err(WebhookError::NoMatchingSignature);
    }
    let value: Value = serde_json::from_slice(payload).map_err(|_| WebhookError::InvalidPayload)?;
    let (id, event_type) = (value.string("id"), value.string("type"));
    if id.is_empty() || event_type.is_empty() {
        return Err(WebhookError::InvalidPayload);
    }
    Ok(WebhookEvent {
        id,
        event_type,
        created_at: value.opt_string("createdAt"),
        data: value.get("data").cloned().unwrap_or(Value::Null),
    })
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    s.as_bytes()
        .chunks(2)
        .map(|p| {
            let hi = (p[0] as char).to_digit(16)?;
            let lo = (p[1] as char).to_digit(16)?;
            Some((hi * 16 + lo) as u8)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "whsec_synthetic_test_secret_0001";
    const BODY: &str =
        r#"{"id":"evt_1","type":"credits.low","createdAt":"2026-10-28T00:00:00Z","data":{"customerId":"cus_1"}}"#;

    fn sign(secret: &str, t: i64, body: &str) -> String {
        let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
        let tag = hmac::sign(&key, format!("{t}.{body}").as_bytes());
        tag.as_ref().iter().map(|b| format!("{b:02x}")).collect()
    }

    fn at(t: i64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(t as u64)
    }

    #[test]
    fn a_good_signature_verifies() {
        let t = 1_800_000_000;
        let header = format!("t={t},v1={}", sign(SECRET, t, BODY));
        let ev = verify_webhook_at(BODY.as_bytes(), &header, &[SECRET], at(t + 10)).unwrap();
        assert_eq!((ev.id.as_str(), ev.event_type.as_str()), ("evt_1", "credits.low"));
        assert_eq!(ev.data["customerId"], "cus_1");
    }

    #[test]
    fn rotation_any_secret_and_any_signature_may_match() {
        let t = 1_800_000_000;
        let header = format!("t={t},v1={},v1={}", "00".repeat(32), sign(SECRET, t, BODY));
        assert!(verify_webhook_at(BODY.as_bytes(), &header, &["whsec_synthetic_b", SECRET], at(t)).is_ok());
    }

    #[test]
    fn every_refusal_has_its_reason() {
        let t = 1_800_000_000;
        let good = sign(SECRET, t, BODY);
        let check = |h: &str, body: &str, secrets: &[&str], now: i64| {
            verify_webhook_at(body.as_bytes(), h, secrets, at(now)).unwrap_err()
        };
        assert_eq!(check("", BODY, &[SECRET], t), WebhookError::MissingHeader);
        assert_eq!(check("   ", BODY, &[SECRET], t), WebhookError::MissingHeader);
        assert_eq!(check("garbage", BODY, &[SECRET], t), WebhookError::MalformedHeader);
        assert_eq!(
            check(&format!("t=abc,v1={good}"), BODY, &[SECRET], t),
            WebhookError::MalformedHeader
        );
        assert_eq!(
            check(&format!("t={t}"), BODY, &[SECRET], t),
            WebhookError::MalformedHeader
        );
        assert_eq!(
            check(&format!("v1={good}"), BODY, &[SECRET], t),
            WebhookError::MalformedHeader
        );
        let header = format!("t={t},v1={good}");
        assert_eq!(check(&header, BODY, &[SECRET], t + 301), WebhookError::StaleTimestamp);
        assert_eq!(check(&header, BODY, &[SECRET], t - 301), WebhookError::StaleTimestamp);
        assert!(verify_webhook_at(BODY.as_bytes(), &header, &[SECRET], at(t + 300)).is_ok());
        assert!(verify_webhook_at(BODY.as_bytes(), &header, &[SECRET], at(t - 300)).is_ok());
        assert_eq!(check(&header, BODY, &["wrong"], t), WebhookError::NoMatchingSignature);
        // no secret, or a blank one anywhere in the rotation list, is refused (a blank HMAC key is forgeable by anyone)
        for secrets in [
            &[][..],
            &[""][..],
            &["  "][..],
            &[SECRET, ""][..],
            &["\n", SECRET][..],
            &["\0\0\0"][..],
            &["\u{200b}\u{200b}"][..],
            &["\u{feff}"][..],
            &[SECRET, "\0 \u{200d}\u{feff}"][..],
            &["\u{3164}\u{e0001}"][..],
        ] {
            assert_eq!(
                check(&header, BODY, secrets, t),
                WebhookError::InvalidSecret,
                "{secrets:?}"
            );
        }
        let forged = format!("t={t},v1={}", sign("", t, BODY));
        assert_eq!(check(&forged, BODY, &[""], t), WebhookError::InvalidSecret);
        // the body is signed as raw bytes: one changed byte, or a re-encoded body, does not verify
        assert_eq!(
            check(&header, &BODY.replace("evt_1", "evt_2"), &[SECRET], t),
            WebhookError::NoMatchingSignature
        );
        assert_eq!(
            check(&header, &format!("{BODY} "), &[SECRET], t),
            WebhookError::NoMatchingSignature
        );
        // a v1 that is not 64 hex characters is not a signature (the shared vector "v1 is not 64 hex characters")
        assert_eq!(
            check(&format!("t={t},v1=zz{}", &good[2..]), BODY, &[SECRET], t),
            WebhookError::MalformedHeader
        );
        assert_eq!(
            check(&format!("t={t},v1=abc"), BODY, &[SECRET], t),
            WebhookError::MalformedHeader
        );
    }

    #[test]
    fn a_signed_body_that_is_not_an_event_is_invalid() {
        let t = 1_800_000_000;
        for body in ["not json", "{}", r#"{"id":"e"}"#, r#"{"type":"x"}"#, "[1]"] {
            let header = format!("t={t},v1={}", sign(SECRET, t, body));
            assert_eq!(
                verify_webhook_at(body.as_bytes(), &header, &[SECRET], at(t)).unwrap_err(),
                WebhookError::InvalidPayload,
                "{body}"
            );
        }
    }

    #[test]
    fn hmac_sha256_matches_rfc_4231_test_case_2() {
        // key "Jefe", data "what do ya want for nothing?"
        let key = hmac::Key::new(hmac::HMAC_SHA256, b"Jefe");
        let tag = hmac::sign(&key, b"what do ya want for nothing?");
        let hex: String = tag.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    #[test]
    fn hex_decoding_rejects_junk() {
        assert_eq!(decode_hex("00ff"), Some(vec![0, 255]));
        assert_eq!(decode_hex("0"), None);
        assert_eq!(decode_hex("0g"), None);
        assert_eq!(decode_hex("é0"), None);
    }
}
