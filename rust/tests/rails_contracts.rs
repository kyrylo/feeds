use chrono::{DateTime, Utc};
use feeds::{events::EventInput, session::SessionSigner};
use serde_json::Value;

fn reference() -> Value {
    serde_json::from_str(include_str!("fixtures/rails-contracts.json")).unwrap()
}

#[test]
fn event_normalization_matches_original_rails_outputs() {
    let reference = reference();
    let now = DateTime::parse_from_rfc3339(reference["now"].as_str().unwrap())
        .unwrap()
        .naive_utc();
    for case in reference["events"].as_array().unwrap() {
        let result = EventInput::from_params(&case["input"], now);
        if case["valid"] == true {
            let event = result.unwrap();
            let mut actual = serde_json::to_value(&event).unwrap();
            actual["occurred_at"] = Value::String(
                event
                    .occurred_at
                    .and_utc()
                    .to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
            );
            assert_eq!(actual, case["output"], "input: {}", case["input"]);
            assert_eq!(
                event.search_text(),
                case["search_text"].as_str().unwrap(),
                "input: {}",
                case["input"]
            );
        } else {
            assert_eq!(
                serde_json::to_value(result.unwrap_err()).unwrap(),
                case["errors"]
            );
        }
    }
}

#[test]
fn existing_rails_http_session_cookie_and_bcrypt_hash_are_accepted() {
    let reference = reference();
    let secret = b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let now = DateTime::parse_from_rfc3339(reference["now"].as_str().unwrap())
        .unwrap()
        .with_timezone(&Utc);
    assert_eq!(
        SessionSigner::new(secret).verify(reference["session"]["cookie"].as_str().unwrap(), now),
        reference["session"]["id"].as_i64()
    );
    assert!(
        bcrypt::verify(
            reference["session"]["password"].as_str().unwrap(),
            reference["session"]["password_digest"].as_str().unwrap()
        )
        .unwrap()
    );
}

#[test]
fn real_rails_reset_and_transfer_links_preserve_purpose_expiry_and_password_binding() {
    use feeds::tokens::{RailsTokens, password_salt_tail};
    let reference = reference();
    let tokens =
        RailsTokens::new(b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");
    let now = DateTime::parse_from_rfc3339(reference["now"].as_str().unwrap())
        .unwrap()
        .to_utc();
    let data = &reference["tokens"];
    let id = data["user_id"].as_i64().unwrap();
    let reset = data["reset"].as_str().unwrap();
    let transfer = data["transfer"].as_str().unwrap();
    assert_eq!(tokens.verify_transfer(transfer, now), Some(id));
    assert_eq!(
        tokens.reset_claims(reset, now),
        Some((
            id,
            password_salt_tail(data["password_digest"].as_str().unwrap())
                .unwrap()
                .into()
        ))
    );
    assert!(
        tokens
            .reset_claims(reset, now + chrono::Duration::minutes(15))
            .is_none()
    );
    assert!(
        tokens
            .verify_transfer(transfer, now + chrono::Duration::hours(4))
            .is_none()
    );
    assert!(tokens.reset_claims(transfer, now).is_none());
    assert!(tokens.verify_transfer(reset, now).is_none());
    assert!(tokens.reset_claims(&format!("{reset}0"), now).is_none());
    assert!(
        RailsTokens::new(b"wrong-secret")
            .verify_transfer(transfer, now)
            .is_none()
    );
}
