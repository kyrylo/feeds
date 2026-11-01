//! Rails signed session cookie compatibility. Verify the signature and purpose
//! before interpreting any metadata; derive the key once when creating a signer.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Datelike, Utc};
use hmac::{Hmac, KeyInit, Mac};
use pbkdf2::pbkdf2_hmac;
use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, utf8_percent_encode};
use serde_json::{Value, json};
use sha1::Sha1;
use sha2::Sha256;

const COOKIE_ESCAPE: &AsciiSet = &CONTROLS.add(b'+').add(b'/').add(b'=');

#[derive(Clone)]
pub struct SessionSigner {
    key: [u8; 64],
}

impl SessionSigner {
    pub fn new(secret_key_base: &[u8]) -> Self {
        let mut key = [0; 64];
        pbkdf2_hmac::<Sha256>(secret_key_base, b"signed cookie", 1000, &mut key);
        Self { key }
    }

    pub fn issue(&self, session_id: i64, now: DateTime<Utc>) -> Option<String> {
        if session_id <= 0 {
            return None;
        }
        let expires = now
            .with_year(now.year() + 20)
            .or_else(|| now.with_day(28)?.with_year(now.year() + 20))?;
        let metadata = json!({"_rails": {
            "message":STANDARD.encode(session_id.to_string()),
            "exp":expires.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "pur":"cookie.session_id"
        }});
        let encoded = STANDARD.encode(serde_json::to_vec(&metadata).ok()?);
        let mut mac = <Hmac<Sha1> as KeyInit>::new_from_slice(&self.key).ok()?;
        mac.update(encoded.as_bytes());
        let hex: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Some(utf8_percent_encode(&format!("{encoded}--{hex}"), COOKIE_ESCAPE).to_string())
    }

    pub fn verify(&self, cookie: &str, now: DateTime<Utc>) -> Option<i64> {
        if cookie.len() > 4096 {
            return None;
        }
        let decoded = percent_decode_str(cookie).decode_utf8().ok()?;
        let (encoded, signature) = decoded.rsplit_once("--")?;
        if signature.len() != 40 || !signature.is_ascii() {
            return None;
        }
        let bytes: Option<Vec<u8>> = signature
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
            .collect();
        let mut mac = <Hmac<Sha1> as KeyInit>::new_from_slice(&self.key).ok()?;
        mac.update(encoded.as_bytes());
        mac.verify_slice(&bytes?).ok()?;
        let envelope: Value = serde_json::from_slice(&STANDARD.decode(encoded).ok()?).ok()?;
        let metadata = envelope.get("_rails")?;
        if metadata.get("pur")?.as_str()? != "cookie.session_id" {
            return None;
        }
        if let Some(expiry) = metadata.get("exp").and_then(Value::as_str)
            && DateTime::parse_from_rfc3339(expiry).ok()? <= now
        {
            return None;
        }
        let payload: Value = if let Some(message) = metadata.get("message").and_then(Value::as_str)
        {
            serde_json::from_slice(&STANDARD.decode(message).ok()?).ok()?
        } else {
            metadata.get("data")?.clone()
        };
        let id = payload
            .as_i64()
            .or_else(|| payload.as_str()?.parse::<i64>().ok())?;
        (id > 0).then_some(id)
    }

    pub fn sign_message(&self, purpose: &str, value: &Value, expires: DateTime<Utc>) -> String {
        let payload = STANDARD.encode(
            json!({"purpose":purpose,"value":value,"expires":expires.timestamp()}).to_string(),
        );
        let mut mac =
            <Hmac<Sha256> as KeyInit>::new_from_slice(&self.key).expect("fixed HMAC key length");
        mac.update(payload.as_bytes());
        let signature = STANDARD.encode(mac.finalize().into_bytes());
        utf8_percent_encode(&format!("{payload}--{signature}"), COOKIE_ESCAPE).to_string()
    }

    pub fn verify_message(&self, purpose: &str, cookie: &str, now: DateTime<Utc>) -> Option<Value> {
        if cookie.len() > 8192 {
            return None;
        }
        let decoded = percent_decode_str(cookie).decode_utf8().ok()?;
        let (payload, signature) = decoded.rsplit_once("--")?;
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&self.key).ok()?;
        mac.update(payload.as_bytes());
        mac.verify_slice(&STANDARD.decode(signature).ok()?).ok()?;
        let data: Value = serde_json::from_slice(&STANDARD.decode(payload).ok()?).ok()?;
        if data["purpose"].as_str()? != purpose || data["expires"].as_i64()? <= now.timestamp() {
            return None;
        }
        data.get("value").cloned()
    }
}

pub fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key == name).then_some(value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_tampering_other_secrets_and_expired_cookies() {
        let signer = SessionSigner::new(b"synthetic-test-secret-never-a-real-credential");
        let now = DateTime::parse_from_rfc3339("2026-10-18T00:00:00Z")
            .unwrap()
            .to_utc();
        let cookie = signer.issue(123, now).unwrap();
        assert_eq!(signer.verify(&cookie, now), Some(123));
        assert!(
            SessionSigner::new(b"different-secret")
                .verify(&cookie, now)
                .is_none()
        );
        assert!(signer.verify(&format!("{cookie}0"), now).is_none());
        assert!(
            signer
                .verify(&cookie, now.with_year(2047).unwrap())
                .is_none()
        );
    }
}
