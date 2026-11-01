mod common;
use axum::http::StatusCode;
use chrono::Utc;
use common::Harness;

#[tokio::test]
async fn personal_settings_escape_values_validate_input_and_keep_email_unique() {
    let h = Harness::new().await;
    let mut c = h.signed_in().await;
    assert_eq!(
        c.get("/settings").await.location(),
        "/settings/nickname/edit"
    );
    for field in ["nickname", "email", "password"] {
        let reply = c.get(&format!("/settings/{field}/edit")).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert!(reply.html.contains(&format!("change {field}")));
    }
    assert_eq!(
        c.form("PATCH", "/settings/nickname", &[]).await.status,
        StatusCode::BAD_REQUEST
    );
    c.form("PATCH", "/settings/nickname", &[("user[nickname]", "  ")])
        .await;
    assert!(
        c.get("/settings/nickname/edit")
            .await
            .html
            .contains("Nickname can&#x27;t be blank")
    );
    c.form(
        "PATCH",
        "/settings/nickname",
        &[("user[nickname]", "<script>alert('x')</script>")],
    )
    .await;
    let html = c.get("/settings/nickname/edit").await.html;
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("<script>alert("));
    let email = format!("new-{}@example.test", h.account);
    c.form(
        "PUT",
        "/settings/email",
        &[(
            "user[email_address]",
            &format!("  {} ", email.to_uppercase()),
        )],
    )
    .await;
    let stored: String = sqlx::query_scalar("SELECT email_address FROM users WHERE id=$1")
        .bind(h.user)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(stored, email);
    c.form(
        "PATCH",
        "/settings/email",
        &[("user[email_address]", "invalid")],
    )
    .await;
    assert!(
        c.get("/settings/email/edit")
            .await
            .html
            .contains("Email address is invalid")
    );
    let other = h.member("duplicate", 0).await;
    c.form(
        "PATCH",
        "/settings/email",
        &[(
            "user[email_address]",
            &format!("duplicate-{}@example.test", h.account),
        )],
    )
    .await;
    assert!(
        c.get("/settings/email/edit")
            .await
            .html
            .contains("already been taken")
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT email_address FROM users WHERE id=$1")
            .bind(h.user)
            .fetch_one(&h.pool)
            .await
            .unwrap(),
        email
    );
    assert_ne!(other, h.user);
    h.finish().await;
}

#[tokio::test]
async fn changing_password_requires_valid_supplied_challenge_and_revokes_other_sessions_only() {
    let h = Harness::new().await;
    let mut c = h.signed_in().await;
    let mut other = h.signed_in().await;
    let reset = h
        .state
        .tokens
        .issue_reset(h.user, &h.digest, Utc::now())
        .unwrap();
    c.get("/settings/password/edit").await;
    c.form(
        "PATCH",
        "/settings/password",
        &[
            ("user[password_challenge]", "wrong"),
            ("user[password]", "new"),
            ("user[password_confirmation]", "new"),
        ],
    )
    .await;
    assert!(
        c.get("/settings/password/edit")
            .await
            .html
            .contains("Password challenge is invalid")
    );
    c.form(
        "PATCH",
        "/settings/password",
        &[
            ("user[password_challenge]", "synthetic-password"),
            ("user[password]", "new"),
            ("user[password_confirmation]", "different"),
        ],
    )
    .await;
    assert!(
        c.get("/settings/password/edit")
            .await
            .html
            .contains("doesn&#x27;t match")
    );
    c.form(
        "PATCH",
        "/settings/password",
        &[
            ("user[password_challenge]", "synthetic-password"),
            ("user[password]", "new"),
            ("user[password_confirmation]", "new"),
        ],
    )
    .await;
    assert!(
        c.get("/settings/password/edit")
            .await
            .html
            .contains("done.")
    );
    assert_eq!(
        other.get("/settings/password/edit").await.location(),
        "/session/new"
    );
    assert_eq!(
        c.get(&format!("/passwords/{reset}/edit")).await.location(),
        "/passwords/new"
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
        1
    );
    h.finish().await;
}

#[tokio::test]
async fn team_management_is_retired_without_changing_existing_data() {
    let h = Harness::new().await;
    let mut client = h.signed_in().await;
    let page = client.get("/settings/nickname/edit").await;
    assert!(!page.html.contains("/settings/invitations"));
    assert!(!page.html.contains("/settings/members"));
    for path in [
        "/settings/invitations".into(),
        "/settings/members".into(),
        format!("/settings/members/{}/sinks", h.user),
        format!("/settings/members/{}/transfer", h.user),
    ] {
        assert_eq!(client.get(&path).await.status, StatusCode::NOT_FOUND);
    }
    for (method, path) in [
        ("POST", "/account/join_code".into()),
        ("PATCH", format!("/settings/members/{}/role", h.user)),
        ("PATCH", format!("/settings/members/{}/sinks", h.user)),
    ] {
        assert_eq!(
            client
                .form(method, &path, &[("user[role]", "member")])
                .await
                .status,
            StatusCode::NOT_FOUND
        );
    }
    let code: String = sqlx::query_scalar("SELECT join_code FROM accounts WHERE id=$1")
        .bind(h.account)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(code, h.join_code);
    let role: i32 = sqlx::query_scalar("SELECT role FROM users WHERE id=$1")
        .bind(h.user)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(role, 2);
    h.finish().await;
}
