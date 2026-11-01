//! Synthetic native outputs for validation by the unchanged Rails application.
use chrono::Utc;
use feeds::{session::SessionSigner, tokens::RailsTokens};
use serde_json::json;

fn main() {
    let secret = b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let now = Utc::now();
    let id = 1_234_567_890;
    let digest = bcrypt::hash("synthetic-native-password", 12).unwrap();
    let tokens = RailsTokens::new(secret);
    println!(
        "{}",
        json!({
            "user_id":id,"session_id":id,"password":"synthetic-native-password","password_digest":digest,
            "session_cookie":SessionSigner::new(secret).issue(id,now).unwrap(),
            "reset_token":tokens.issue_reset(id,&digest,now).unwrap(),
            "transfer_token":tokens.issue_transfer(id,now)
        })
    );
}
