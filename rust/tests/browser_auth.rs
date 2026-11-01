mod common;
use axum::http::StatusCode;
use chrono::Utc;
use common::{Harness, delete_account};
use feeds::browser::current_user;

#[tokio::test]
async fn reverse_proxy_clients_keep_independent_rate_limits_and_session_addresses() {
    use axum::http::HeaderMap;
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/proxy.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let mut headers = HeaderMap::new();
        for (field, name) in [("forwarded", "x-forwarded-for"), ("client", "client-ip")] {
            if let Some(value) = case[field].as_str() {
                headers.insert(name, value.parse().unwrap());
            }
        }
        let actual =
            feeds::browser::client_ip(&headers, case["peer"].as_str().unwrap().parse().unwrap());
        if case["rejected"] == true {
            assert_eq!(actual.unwrap_err(), StatusCode::BAD_REQUEST);
        } else {
            assert_eq!(
                actual.unwrap().to_string(),
                case["expected"].as_str().unwrap(),
                "{case}"
            );
        }
    }
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", "203.0.113.200".parse().unwrap());
    headers.insert("client-ip", "198.51.100.90".parse().unwrap());
    let direct = "192.0.2.10".parse().unwrap();
    assert_eq!(feeds::browser::client_ip(&headers, direct).unwrap(), direct);

    let h = Harness::new().await;
    let mut client = h.client();
    client.peer = "127.0.0.1:3010".parse().unwrap();
    client.get("/session/new").await;
    let csrf = client.cookies["telesink_csrf"].clone();
    let first = format!("2001:db8:7::{:x}", h.account);
    let second = format!("2001:db8:8::{:x}", h.account);
    let bad = serde_urlencoded::to_string([
        ("email_address", h.email.as_str()),
        ("password", "wrong"),
        ("authenticity_token", &csrf),
    ])
    .unwrap();
    for _ in 0..10 {
        client
            .send(
                "POST",
                "/session",
                bad.clone(),
                &[("x-forwarded-for", &first)],
            )
            .await;
    }
    let good = serde_urlencoded::to_string([
        ("email_address", h.email.as_str()),
        ("password", "synthetic-password"),
        ("authenticity_token", &csrf),
    ])
    .unwrap();
    assert_eq!(
        client
            .send(
                "POST",
                "/session",
                good.clone(),
                &[("x-forwarded-for", &first)]
            )
            .await
            .location(),
        "/session/new"
    );
    assert!(
        client
            .get("/session/new")
            .await
            .html
            .contains("try again later.")
    );
    assert!(!client.cookies.contains_key("session_id"));
    assert_eq!(
        client
            .send("POST", "/session", good, &[("x-forwarded-for", &second)])
            .await
            .location(),
        "/"
    );
    let id = h
        .state
        .signer
        .verify(&client.cookies["session_id"], Utc::now())
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT ip_address FROM sessions WHERE id=$1")
            .bind(id)
            .fetch_one(&h.pool)
            .await
            .unwrap(),
        second
    );
    h.finish().await;
}

#[tokio::test]
async fn existing_password_login_return_path_cookie_restart_and_logout() {
    let h = Harness::new().await;
    let mut c = h.client();
    assert_eq!(
        c.get("/sinks/42?q=retained").await.location(),
        "/session/new"
    );
    let page = c.get("/session/new").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(
        page.html
            .replace("&#x2f;", "/")
            .contains("/assets/activity-")
    );
    assert!(!page.html.contains("None"));
    let wrong = c
        .form(
            "POST",
            "/session",
            &[("email_address", &h.email), ("password", "wrong")],
        )
        .await;
    assert_eq!(wrong.location(), "/session/new");
    assert!(!c.cookies.contains_key("session_id"));
    assert!(
        c.get("/session/new")
            .await
            .html
            .contains("username or password incorrect.")
    );
    let reply = c
        .form(
            "POST",
            "/session",
            &[
                ("email_address", &format!("  {}  ", h.email.to_uppercase())),
                ("password", "synthetic-password"),
            ],
        )
        .await;
    assert_eq!(reply.location(), "/sinks/42?q=retained");
    let session = h
        .state
        .signer
        .verify(&c.cookies["session_id"], Utc::now())
        .unwrap();
    let row: (i64, String, String) =
        sqlx::query_as("SELECT user_id,ip_address,user_agent FROM sessions WHERE id=$1")
            .bind(session)
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert_eq!(
        row,
        (
            h.user,
            c.peer.ip().to_string(),
            "Rust compatibility test".into()
        )
    );
    // New state/router represents a server restart: persisted sessions survive.
    c.app = feeds::router(feeds::AppState::new(h.pool.clone(), h.state.config.clone()));
    assert_eq!(c.get("/registration/new").await.location(), "/");
    let logout = c.form("POST", "/session", &[("_method", "delete")]).await;
    assert_eq!(logout.status, StatusCode::SEE_OTHER);
    assert_eq!(logout.location(), "/session/new");
    assert!(!c.cookies.contains_key("session_id"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions WHERE id=$1")
            .bind(session)
            .fetch_one(&h.pool)
            .await
            .unwrap(),
        0
    );
    h.finish().await;
}

#[tokio::test]
async fn csrf_rejects_missing_cookie_wrong_token_and_cross_origin_before_mutation() {
    let h = Harness::new().await;
    let mut c = h.signed_in().await;
    assert_eq!(
        c.form("DELETE", "/session", &[]).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    c.get("/session/new").await;
    let token = c.cookies["telesink_csrf"].clone();
    assert_eq!(
        c.send("DELETE", "/session", "authenticity_token=wrong".into(), &[])
            .await
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        c.send(
            "DELETE",
            "/session",
            String::new(),
            &[
                ("x-csrf-token", &token),
                ("origin", "https://attacker.test")
            ]
        )
        .await
        .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(c.get("/registration/new").await.location(), "/");
    assert_eq!(
        c.send(
            "DELETE",
            "/session",
            String::new(),
            &[
                ("x-csrf-token", &token),
                ("origin", "http://localhost:3001")
            ]
        )
        .await
        .status,
        StatusCode::SEE_OTHER
    );
    h.finish().await;
}

#[tokio::test]
async fn registration_creates_first_owner_then_closes_public_signup() {
    let h = Harness::isolated().await;
    delete_account(&h.pool, h.account).await;
    let mut c = h.client();
    c.get("/settings/email/edit").await;
    c.get("/registration/new").await;
    let email = format!("created-{}@example.test", h.account);
    let mismatch = c
        .form(
            "POST",
            "/registration",
            &[
                ("user[email_address]", &email),
                ("user[password]", "secret"),
                ("user[password_confirmation]", "different"),
            ],
        )
        .await;
    assert_eq!(mismatch.location(), "/registration/new");
    assert!(!c.cookies.contains_key("session_id"));
    let reply = c
        .form(
            "POST",
            "/registration",
            &[
                ("user[email_address]", &email),
                ("user[password]", "secret"),
                ("user[password_confirmation]", "secret"),
                ("user[role]", "0"),
                ("user[account_id]", &h.account.to_string()),
            ],
        )
        .await;
    assert_eq!(reply.location(), "/");
    let row: (i64, i32, String, String) = sqlx::query_as(
        "SELECT account_id,role,nickname,password_digest FROM users WHERE email_address=$1",
    )
    .bind(&email)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_ne!(row.0, h.account);
    assert_eq!(row.1, 2);
    assert_eq!(row.2, format!("created-{}", h.account));
    assert!(bcrypt::verify("secret", &row.3).unwrap());
    assert_eq!(&row.3[4..6], "12");
    assert_eq!(c.get("/registration/new").await.location(), "/");
    let mut duplicate = h.client();
    assert_eq!(
        duplicate.get("/registration/new").await.location(),
        "/session/new"
    );
    duplicate.get("/session/new").await;
    assert_eq!(
        duplicate
            .form(
                "POST",
                "/registration",
                &[
                    ("user[email_address]", &email),
                    ("user[password]", "secret")
                ]
            )
            .await
            .location(),
        "/session/new"
    );
    assert!(
        duplicate
            .get("/session/new")
            .await
            .html
            .contains("setup is complete. sign in with your account.")
    );
    assert!(!duplicate.cookies.contains_key("session_id"));
    assert!(
        !duplicate
            .get("/session/new")
            .await
            .html
            .contains("/registration/new")
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM accounts")
            .fetch_one(&h.pool)
            .await
            .unwrap(),
        1
    );
    delete_account(&h.pool, row.0).await;
    h.finish().await;
}

#[tokio::test]
async fn simultaneous_setup_requests_create_only_one_owner() {
    let h = Harness::isolated().await;
    delete_account(&h.pool, h.account).await;
    let mut first = h.client();
    let mut second = h.client();
    first.get("/registration/new").await;
    second.get("/registration/new").await;
    let (a, b) = tokio::join!(
        first.form(
            "POST",
            "/registration",
            &[
                ("user[email_address]", "first@example.test"),
                ("user[password]", "secret")
            ]
        ),
        second.form(
            "POST",
            "/registration",
            &[
                ("user[email_address]", "second@example.test"),
                ("user[password]", "secret")
            ]
        )
    );
    let mut locations = [a.location(), b.location()];
    locations.sort();
    assert_eq!(locations, ["/", "/session/new"]);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users WHERE role=2")
            .fetch_one(&h.pool)
            .await
            .unwrap(),
        1
    );
    let account: i64 = sqlx::query_scalar("SELECT id FROM accounts")
        .fetch_one(&h.pool)
        .await
        .unwrap();
    delete_account(&h.pool, account).await;
    h.finish().await;
}

#[tokio::test]
async fn old_invitations_cannot_create_another_user() {
    let h = Harness::new().await;
    let mut client = h.client();
    client.get("/session/new").await;
    let path = format!("/join/{}", h.join_code);
    assert_eq!(client.get(&path).await.status, StatusCode::NOT_FOUND);
    assert_eq!(
        client
            .form(
                "POST",
                &path,
                &[
                    ("user[email_address]", "unwanted@example.test"),
                    ("user[password]", "secret")
                ]
            )
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users WHERE account_id=$1")
            .bind(h.account)
            .fetch_one(&h.pool)
            .await
            .unwrap(),
        1
    );
    h.finish().await;
}

#[tokio::test]
async fn password_reset_revokes_all_sessions_invalidates_old_token_and_preserves_other_users() {
    let h = Harness::new().await;
    let other = Harness::new().await;
    let mut old_browser = h.signed_in().await;
    let _second_session = h.signed_in().await;
    let other_browser = other.signed_in().await;
    let token = h
        .state
        .tokens
        .issue_reset(h.user, &h.digest, Utc::now())
        .unwrap();
    let path = format!("/passwords/{token}");
    let mut c = h.client();
    assert_eq!(c.get(&format!("{path}/edit")).await.status, StatusCode::OK);
    assert_eq!(
        c.form(
            "PUT",
            &path,
            &[("password", "new"), ("password_confirmation", "different")]
        )
        .await
        .location(),
        format!("{path}/edit")
    );
    let reply = c
        .form(
            "POST",
            &path,
            &[
                ("_method", "put"),
                ("password", "new"),
                ("password_confirmation", "new"),
            ],
        )
        .await;
    assert_eq!(reply.location(), "/session/new");
    assert!(
        c.get("/session/new")
            .await
            .html
            .contains("password has been reset.")
    );
    let digest: String = sqlx::query_scalar("SELECT password_digest FROM users WHERE id=$1")
        .bind(h.user)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert!(bcrypt::verify("new", &digest).unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions WHERE user_id=$1")
            .bind(h.user)
            .fetch_one(&h.pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        c.get(&format!("{path}/edit")).await.location(),
        "/passwords/new"
    );
    assert_eq!(
        old_browser.get("/registration/new").await.location(),
        "/session/new"
    );
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "cookie",
        format!("session_id={}", other_browser.cookies["session_id"])
            .parse()
            .unwrap(),
    );
    assert!(
        current_user(&other.pool, &other.state.signer, &headers)
            .await
            .unwrap()
            .is_some()
    );
    other.finish().await;
    h.finish().await;
}

#[tokio::test]
async fn transfer_links_sign_in_and_expired_or_tampered_links_do_not() {
    let h = Harness::new().await;
    let token = h.state.tokens.issue_transfer(h.user, Utc::now());
    let mut c = h.client();
    c.get("/settings/email/edit").await;
    assert_eq!(
        c.get(&format!("/session/transfer/{token}x"))
            .await
            .location(),
        "/session/new"
    );
    assert!(!c.cookies.contains_key("session_id"));
    let expired = h
        .state
        .tokens
        .issue_transfer(h.user, Utc::now() - chrono::Duration::hours(5));
    assert_eq!(
        c.get(&format!("/session/transfer/{expired}"))
            .await
            .location(),
        "/session/new"
    );
    assert_eq!(
        c.get(&format!("/session/transfer/{token}"))
            .await
            .location(),
        "/sinks"
    );
    assert!(c.cookies.contains_key("session_id"));
    h.finish().await;
}

#[tokio::test]
async fn expired_background_polls_do_not_replace_the_login_destination() {
    let h = Harness::new().await;
    let mut c = h.client();
    c.get("/feeds/42").await;
    let destination = c.cookies["telesink_return_to"].clone();
    for path in [
        "/activity/updates?after=1",
        "/feeds/42/updates?after=1",
        "/activity/events/1",
    ] {
        assert_eq!(c.get(path).await.status, StatusCode::UNAUTHORIZED);
        assert_eq!(c.cookies["telesink_return_to"], destination);
    }
    assert_eq!(
        c.send(
            "GET",
            "/activity/older?before=2",
            String::new(),
            &[("accept", "application/json")]
        )
        .await
        .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(c.cookies["telesink_return_to"], destination);
    h.finish().await;
}
