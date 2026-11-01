//! Active Record signed IDs and password-reset tokens. These deliberately use
//! Rails' salts, metadata, purposes and digests so existing links survive cutover.
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use hmac::{Hmac, KeyInit, Mac};
use pbkdf2::pbkdf2_hmac;
use serde_json::{Value, json};
use sha1::Sha1;
use sha2::Sha256;

const RESET_PURPOSE: &str = "User\npassword_reset\n900";
const TRANSFER_PURPOSE: &str = "user/transfer";

pub struct RailsTokens {
    reset_key: [u8; 64],
    transfer_key: [u8; 64],
    stream_key: [u8; 64],
}

impl RailsTokens {
    pub fn new(secret: &[u8]) -> Self {
        let mut reset_key = [0; 64];
        let mut transfer_key = [0; 64];
        let mut stream_key = [0; 64];
        pbkdf2_hmac::<Sha256>(secret, b"active_record/token_for", 1000, &mut reset_key);
        pbkdf2_hmac::<Sha256>(secret, b"active_record/signed_id", 1000, &mut transfer_key);
        pbkdf2_hmac::<Sha256>(
            secret,
            b"turbo/signed_stream_verifier_key",
            1000,
            &mut stream_key,
        );
        Self {
            reset_key,
            transfer_key,
            stream_key,
        }
    }

    pub fn issue_stream(&self, name: &str) -> String {
        let payload = STANDARD.encode(serde_json::to_vec(name).unwrap());
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&self.stream_key).unwrap();
        mac.update(payload.as_bytes());
        format!("{payload}--{}", hex(&mac.finalize().into_bytes()))
    }

    pub fn verify_stream(&self, token: &str) -> Option<String> {
        let (encoded, signature) = split(token, 64)?;
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&self.stream_key).ok()?;
        mac.update(encoded.as_bytes());
        mac.verify_slice(&signature).ok()?;
        serde_json::from_slice(&STANDARD.decode(encoded).ok()?).ok()
    }

    pub fn issue_reset(&self, user_id: i64, digest: &str, now: DateTime<Utc>) -> Option<String> {
        let salt = password_salt_tail(digest)?;
        let payload = envelope(
            json!([user_id, salt]),
            RESET_PURPOSE,
            now + Duration::minutes(15),
        );
        let mut mac = <Hmac<Sha1> as KeyInit>::new_from_slice(&self.reset_key).ok()?;
        mac.update(payload.as_bytes());
        Some(format!("{payload}--{}", hex(&mac.finalize().into_bytes())))
    }

    pub fn reset_claims(&self, token: &str, now: DateTime<Utc>) -> Option<(i64, String)> {
        let (encoded, signature) = split(token, 40)?;
        let mut mac = <Hmac<Sha1> as KeyInit>::new_from_slice(&self.reset_key).ok()?;
        mac.update(encoded.as_bytes());
        mac.verify_slice(&signature).ok()?;
        let value = payload(encoded, RESET_PURPOSE, now)?;
        let values = value.as_array()?;
        if values.len() != 2 {
            return None;
        }
        Some((values[0].as_i64()?, values[1].as_str()?.to_owned()))
    }

    pub fn issue_transfer(&self, user_id: i64, now: DateTime<Utc>) -> String {
        let payload = envelope(json!(user_id), TRANSFER_PURPOSE, now + Duration::hours(4));
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&self.transfer_key).unwrap();
        mac.update(payload.as_bytes());
        format!("{payload}--{}", hex(&mac.finalize().into_bytes()))
    }

    pub fn verify_transfer(&self, token: &str, now: DateTime<Utc>) -> Option<i64> {
        let (encoded, signature) = split(token, 64)?;
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&self.transfer_key).ok()?;
        mac.update(encoded.as_bytes());
        mac.verify_slice(&signature).ok()?;
        let id = payload(encoded, TRANSFER_PURPOSE, now)?.as_i64()?;
        (id > 0).then_some(id)
    }
}

pub fn password_salt_tail(digest: &str) -> Option<&str> {
    // BCrypt::Password#salt is the first 29 ASCII bytes; Rails binds resets to
    // its last ten characters so changing a password invalidates old links.
    if digest.len() != 60 || !digest.is_ascii() || !digest.starts_with("$2") {
        return None;
    }
    digest.get(19..29)
}

fn envelope(value: Value, purpose: &str, expires: DateTime<Utc>) -> String {
    URL_SAFE_NO_PAD.encode(
        json!({"_rails":{
            "data":value,"exp":expires.to_rfc3339_opts(SecondsFormat::Millis,true),"pur":purpose
        }})
        .to_string(),
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn split(token: &str, hex_length: usize) -> Option<(&str, Vec<u8>)> {
    if token.len() > 8192 {
        return None;
    }
    let (encoded, signature) = token.rsplit_once("--")?;
    if signature.len() != hex_length || !signature.is_ascii() {
        return None;
    }
    let bytes = signature
        .as_bytes()
        .chunks_exact(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    Some((encoded, bytes))
}

fn payload(encoded: &str, purpose: &str, now: DateTime<Utc>) -> Option<Value> {
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .or_else(|_| STANDARD.decode(encoded))
        .ok()?;
    let envelope: Value = serde_json::from_slice(&bytes).ok()?;
    let metadata = envelope.get("_rails")?;
    if metadata.get("pur")?.as_str()? != purpose {
        return None;
    }
    // Both link families always expire. Reject missing expiry, even with a MAC.
    let expiry = DateTime::parse_from_rfc3339(metadata.get("exp")?.as_str()?).ok()?;
    if expiry <= now {
        return None;
    }
    if let Some(message) = metadata.get("message").and_then(Value::as_str) {
        serde_json::from_slice(&STANDARD.decode(message).ok()?).ok()
    } else {
        metadata.get("data").cloned()
    }
}
