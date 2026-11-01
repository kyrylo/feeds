mod common;
use axum::http::StatusCode;
use common::{Harness, delete_account};
use feeds::{owner, schema};

#[tokio::test]
async fn bootstrap_is_atomic_preserves_owner_and_reset_revokes_sessions() {
    let h = Harness::isolated().await;
    delete_account(&h.pool, h.account).await;
    assert!(
        owner::bootstrap(&h.pool, Some("invalid"), None)
            .await
            .is_err()
    );
    let digest = owner::hash_password("initial-owner-password").unwrap();
    let (first, second) = tokio::join!(
        owner::bootstrap(&h.pool, Some(&digest), None),
        owner::bootstrap(&h.pool, Some(&digest), None)
    );
    assert_ne!(first.unwrap(), second.unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users")
            .fetch_one(&h.pool)
            .await
            .unwrap(),
        1
    );
    // Environment hashes never overwrite credentials saved in the database.
    assert!(
        !owner::bootstrap(&h.pool, Some("invalid and ignored"), None)
            .await
            .unwrap()
    );
    let mut client = h.client();
    let login = client.get("/session/new").await;
    assert_eq!(login.status, StatusCode::OK);
    assert!(!login.html.contains("type=\"email\""));
    assert_eq!(
        client
            .form(
                "POST",
                "/session",
                &[("password", "initial-owner-password")]
            )
            .await
            .location(),
        "/"
    );
    assert_eq!(client.get("/").await.status, StatusCode::OK);
    let replacement = owner::hash_password("replacement-owner-password").unwrap();
    owner::reset_password(&h.pool, &replacement).await.unwrap();
    assert_eq!(client.get("/").await.location(), "/session/new");
    client.get("/session/new").await;
    assert_eq!(
        client
            .form(
                "POST",
                "/session",
                &[("password", "initial-owner-password")]
            )
            .await
            .location(),
        "/session/new"
    );
    client.get("/session/new").await;
    assert_eq!(
        client
            .form(
                "POST",
                "/session",
                &[("password", "replacement-owner-password")]
            )
            .await
            .location(),
        "/"
    );
    assert!(
        !owner::bootstrap(&h.pool, Some(&digest), None)
            .await
            .unwrap()
    );
    schema::check(&h.pool).await.unwrap();
    h.finish().await;
}

#[tokio::test]
async fn imported_multiple_users_require_email_and_cannot_be_reset_ambiguously() {
    let h = Harness::isolated().await;
    sqlx::query("INSERT INTO users (account_id,email_address,nickname,password_digest,role,created_at,updated_at) VALUES ($1,'second@example.test','second',$2,0,now(),now())")
        .bind(h.account).bind(&h.digest).execute(&h.pool).await.unwrap();
    let mut client = h.client();
    assert!(
        client
            .get("/session/new")
            .await
            .html
            .contains("type=\"email\"")
    );
    assert_eq!(
        client
            .form("POST", "/session", &[("password", "synthetic-password")])
            .await
            .location(),
        "/session/new"
    );
    assert!(owner::reset_password(&h.pool, &h.digest).await.is_err());
    client.get("/session/new").await;
    assert_eq!(
        client
            .form(
                "POST",
                "/session",
                &[
                    ("email_address", &h.email),
                    ("password", "synthetic-password")
                ]
            )
            .await
            .location(),
        "/"
    );
    h.finish().await;
}
