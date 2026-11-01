mod common;
use axum::http::StatusCode;
use common::{Harness, smtp::Smtp};
use feeds::{AppState, config::Config, mail::Mailer};
use std::{sync::Arc, time::Duration};

fn configure(h: &mut Harness, mailer: Arc<Mailer>) {
    h.state = AppState::new(
        h.pool.clone(),
        Arc::new(Config {
            database_url: h.state.config.database_url.clone(),
            secret_key_base: h.state.config.secret_key_base.clone(),
            bind: h.state.config.bind,
            public_url: None,
            database_connections: h.state.config.database_connections,
            mailer: Some(mailer),
            demo: false,
        }),
    );
    h.app = feeds::router(h.state.clone());
}

// Decode the standard quoted-printable MIME parts captured over actual SMTP.
fn decoded(message: &str) -> String {
    let bytes = message.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"=\r\n") {
            i += 3;
            continue;
        }
        if bytes[i] == b'='
            && i + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(value) = u8::from_str_radix(hex, 16)
        {
            out.push(value);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap()
}

#[tokio::test]
async fn reset_request_delivers_real_smtp_and_its_link_resets_the_existing_user() {
    let smtp = Smtp::new(true, false).await;
    let mut h = Harness::new().await;
    configure(&mut h, smtp.mailer("none"));
    let mut existing_session = h.signed_in().await;
    let mut c = h.client();
    c.get("/passwords/new").await;
    let csrf = c.cookies["telesink_csrf"].clone();
    let reply = c
        .send(
            "POST",
            "/passwords",
            serde_urlencoded::to_string([
                ("email_address", format!("  {}  ", h.email.to_uppercase())),
                ("authenticity_token", csrf),
            ])
            .unwrap(),
            &[
                ("host", "attacker.example.test"),
                ("x-forwarded-host", "attacker.example.test"),
            ],
        )
        .await;
    assert_eq!(reply.location(), "/session/new");
    let notice = "password reset instructions sent (if user with that email address exists).";
    assert!(c.get("/session/new").await.html.contains(notice));
    let messages = smtp.messages.lock().unwrap().clone();
    assert_eq!(messages.len(), 1);
    let mail = decoded(&messages[0]);
    assert!(mail.contains("Subject: Reset your password\r\n"));
    assert!(mail.contains("From: Feeds <reset@example.test>\r\n"));
    assert!(mail.contains(&format!("To: {}\r\n", h.email)));
    assert!(mail.contains("multipart/alternative"));
    assert!(mail.contains("text/plain") && mail.contains("text/html"));
    assert!(mail.contains("This link will expire in 15 minutes."));
    assert!(!mail.contains("attacker.example.test"));
    assert!(
        smtp.commands
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.starts_with("AUTH PLAIN "))
    );
    let link = mail
        .lines()
        .find(|l| l.starts_with("https://events.example.test/passwords/"))
        .unwrap();
    let path = link.strip_prefix("https://events.example.test").unwrap();
    assert!(path.ends_with("/edit"));
    assert_eq!(c.get(path).await.status, StatusCode::OK);
    let result = c
        .form(
            "PUT",
            path.strip_suffix("/edit").unwrap(),
            &[
                ("password", "new-synthetic-password"),
                ("password_confirmation", "new-synthetic-password"),
            ],
        )
        .await;
    assert_eq!(result.location(), "/session/new");
    assert_eq!(c.get(path).await.location(), "/passwords/new");
    assert_eq!(
        existing_session.get("/settings").await.location(),
        "/session/new"
    );
    let digest: String = sqlx::query_scalar("SELECT password_digest FROM users WHERE id=$1")
        .bind(h.user)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert!(bcrypt::verify("new-synthetic-password", &digest).unwrap());
    assert_eq!(
        c.form(
            "POST",
            "/passwords",
            &[("email_address", "absent@example.test")]
        )
        .await
        .location(),
        "/session/new"
    );
    assert!(c.get("/session/new").await.html.contains(notice));
    assert_eq!(smtp.messages.lock().unwrap().len(), 1);
    h.finish().await;
}

#[tokio::test]
async fn reset_failures_csrf_and_rate_limits_do_not_claim_delivery_or_change_credentials() {
    let smtp = Smtp::new(false, false).await;
    let mut h = Harness::new().await;
    let mut disabled = h.client();
    disabled.get("/passwords/new").await;
    assert_eq!(
        disabled
            .form("POST", "/passwords", &[("email_address", &h.email)])
            .await
            .location(),
        "/passwords/new"
    );
    assert!(
        disabled
            .get("/passwords/new")
            .await
            .html
            .contains("password reset email is unavailable")
    );
    configure(&mut h, smtp.mailer("none"));
    let mut c = h.client();
    c.peer.set_ip("127.2.0.1".parse().unwrap());
    assert_eq!(
        c.form("POST", "/passwords", &[("email_address", &h.email)])
            .await
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert!(smtp.messages.lock().unwrap().is_empty());
    c.get("/passwords/new").await;
    assert_eq!(
        c.form("POST", "/passwords", &[("email_address", &h.email)])
            .await
            .location(),
        "/passwords/new"
    );
    assert!(
        c.get("/passwords/new")
            .await
            .html
            .contains("password reset email is unavailable")
    );
    for _ in 0..9 {
        assert_eq!(
            c.form(
                "POST",
                "/passwords",
                &[("email_address", "absent@example.test")]
            )
            .await
            .location(),
            "/session/new"
        );
    }
    assert_eq!(
        c.form("POST", "/passwords", &[("email_address", &h.email)])
            .await
            .location(),
        "/passwords/new"
    );
    assert!(
        c.get("/passwords/new")
            .await
            .html
            .contains("Try again later.")
    );
    assert_eq!(smtp.messages.lock().unwrap().len(), 1);
    let digest: String = sqlx::query_scalar("SELECT password_digest FROM users WHERE id=$1")
        .bind(h.user)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(digest, h.digest);
    h.finish().await;
}

#[tokio::test]
async fn smtp_requires_starttls_without_downgrading_or_sending_credentials() {
    let smtp = Smtp::new(true, false).await;
    assert!(
        smtp.mailer("starttls")
            .password_reset("recipient@example.test", "synthetic-token")
            .await
            .is_err()
    );
    assert!(smtp.messages.lock().unwrap().is_empty());
    assert!(
        !smtp
            .commands
            .lock()
            .unwrap()
            .iter()
            .any(|c| c.starts_with("AUTH ") || c.starts_with("MAIL FROM:"))
    );
}

#[tokio::test]
async fn smtp_connections_are_bounded_and_capacity_returns_after_delivery() {
    let smtp = Smtp::new(true, true).await;
    let mailer = smtp.mailer("none");
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..2 {
        let mailer = mailer.clone();
        tasks.spawn(async move {
            mailer
                .password_reset("recipient@example.test", "synthetic-token")
                .await
        });
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if smtp.messages.lock().unwrap().len() == 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        mailer
            .password_reset("recipient@example.test", "synthetic-token")
            .await,
        Err("email capacity reached")
    );
    smtp.gate.add_permits(2);
    while let Some(result) = tasks.join_next().await {
        assert_eq!(result.unwrap(), Ok(()));
    }
    assert_eq!(
        mailer
            .password_reset("recipient@example.test", "synthetic-token")
            .await,
        Ok(())
    );
    assert_eq!(smtp.messages.lock().unwrap().len(), 3);
}
