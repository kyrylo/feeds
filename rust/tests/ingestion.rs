//! Exercise real PostgreSQL constraints and the public HTTP contract. This suite
//! refuses all but the dedicated synthetic database; no database tests are skipped.
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::NaiveDateTime;
use serde_json::{Value, json};
mod common;
use feeds::{AppState, config::Config};
use sqlx::{Row, SqlitePool};
use std::sync::Arc;
use tower::ServiceExt;

struct Harness {
    pool: SqlitePool,
    app: Router,
    account: i64,
    user: i64,
    sink: i64,
    token: String,
}

#[tokio::test]
async fn browser_sdk_cors_matches_rails_and_does_not_open_account_routes() {
    let h = Harness::new().await;
    let cases: Value = serde_json::from_str(include_str!("fixtures/cors.json")).unwrap();
    let permits =
        |headers: &std::collections::HashMap<String, String>, method: &str, requested: &str| {
            headers
                .get("access-control-allow-origin")
                .is_some_and(|v| v == "*" || v == "https://sender.example.test")
                && headers
                    .get("access-control-allow-methods")
                    .is_some_and(|v| v.split(',').any(|v| v.trim() == method))
                && requested.split(',').all(|name| {
                    headers
                        .get("access-control-allow-headers")
                        .is_some_and(|v| v.split(',').any(|v| v.trim().eq_ignore_ascii_case(name)))
                })
        };
    for case in cases.as_array().unwrap() {
        let mut request = Request::builder()
            .method(case["method"].as_str().unwrap())
            .uri(case["path"].as_str().unwrap())
            .header("origin", "https://sender.example.test");
        if let Some(method) = case["requested_method"].as_str() {
            request = request
                .header("access-control-request-method", method)
                .header(
                    "access-control-request-headers",
                    case["requested_headers"].as_str().unwrap(),
                );
        }
        let response = h
            .app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let headers: std::collections::HashMap<_, _> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_owned()))
            .collect();
        let expected: std::collections::HashMap<String, String> =
            serde_json::from_value(case["headers"].clone()).unwrap();
        if let Some(method) = case["requested_method"].as_str() {
            let allowed = permits(
                &expected,
                method,
                case["requested_headers"].as_str().unwrap(),
            );
            assert_eq!(
                permits(
                    &headers,
                    method,
                    case["requested_headers"].as_str().unwrap()
                ),
                allowed,
                "{case}"
            );
            if allowed {
                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(
                    headers["access-control-max-age"],
                    expected["access-control-max-age"]
                );
            }
        } else {
            assert_eq!(
                response.status().as_u16(),
                case["status"].as_u64().unwrap() as u16
            );
            assert_eq!(
                headers.get("access-control-allow-origin"),
                expected.get("access-control-allow-origin")
            );
        }
        assert!(!headers.contains_key("access-control-allow-credentials"));
    }
    let response = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/sinks/{}/events", h.token))
                .header("origin", "https://sender.example.test")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"text":"Browser SDK acceptance"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response.headers()["access-control-allow-origin"], "*");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM events WHERE sink_id=$1")
            .bind(h.sink)
            .fetch_one(&h.pool)
            .await
            .unwrap(),
        1
    );
    h.finish().await;
}

impl Harness {
    async fn new() -> Self {
        let (pool, url) = common::database().await;
        let account: i64 = sqlx::query_scalar(
            "INSERT INTO accounts (created_at,updated_at) VALUES (now(),now()) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let user: i64 = sqlx::query_scalar("INSERT INTO users (account_id,email_address,nickname,password_digest,role,created_at,updated_at) VALUES ($1,$2,'Tester','test-only-unusable-hash',2,now(),now()) RETURNING id")
            .bind(account).bind(format!("test-{account}@example.test")).fetch_one(&pool).await.unwrap();
        let token = format!("test-source-{account}");
        let sink: i64 = sqlx::query_scalar("INSERT INTO sinks (account_id,name,token,created_at,updated_at) VALUES ($1,'Existing source',$2,now(),now()) RETURNING id")
            .bind(account).bind(&token).fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO sink_memberships (user_id,sink_id,created_at,updated_at) VALUES ($1,$2,now(),now())")
            .bind(user).bind(sink).execute(&pool).await.unwrap();
        let config = Config {
            database_url: url,
            secret_key_base: "0".repeat(64),
            bind: "127.0.0.1:0".parse().unwrap(),
            public_url: None,
            database_connections: 3,
            mailer: None,
            demo: false,
        };
        let app = feeds::router(AppState::new(pool.clone(), Arc::new(config)));
        Self {
            pool,
            app,
            account,
            user,
            sink,
            token,
        }
    }

    async fn send(&self, token: &str, raw: &str, content_type: &str) -> (StatusCode, Vec<u8>) {
        let response = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/sinks/{token}/events"))
                    .header("content-type", content_type)
                    .body(Body::from(raw.to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        (
            status,
            to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
    }

    async fn finish(self) {
        sqlx::query(
            "DELETE FROM events WHERE sink_id IN (SELECT id FROM sinks WHERE account_id=$1)",
        )
        .bind(self.account)
        .execute(&self.pool)
        .await
        .unwrap();
        sqlx::query("DELETE FROM sink_memberships WHERE user_id=$1")
            .bind(self.user)
            .execute(&self.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM sinks WHERE account_id=$1")
            .bind(self.account)
            .execute(&self.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(self.user)
            .execute(&self.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM accounts WHERE id=$1")
            .bind(self.account)
            .execute(&self.pool)
            .await
            .unwrap();
        self.pool.close().await;
    }
}

#[tokio::test]
async fn original_url_stores_payload_in_existing_schema_before_success() {
    let h = Harness::new().await;
    let payload = json!({"event":"payment.succeeded","text":"Maya paid €29 🎉","emoji":"💳","properties":{"amount":29,"paid":true,"nested":{"x":[1,null]}},"occurred_at":"2026-10-18T08:30:00.123456+08:00","sdk":{"name":"telesink-ruby","version":"1.3.0"}});
    let (status, body) = h
        .send(&h.token, &payload.to_string(), "application/json")
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(body.is_empty());
    let row = sqlx::query("SELECT * FROM events WHERE sink_id=$1")
        .bind(h.sink)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(
        row.get::<String, _>("text"),
        payload["text"].as_str().unwrap()
    );
    assert_eq!(
        row.get::<Value, _>("properties"),
        json!({"amount":29,"paid":true,"nested":{"x":[1,{}]}})
    );
    assert_eq!(row.get::<String, _>("sdk_name"), "telesink-ruby");
    assert_eq!(
        row.get::<NaiveDateTime, _>("occurred_at").to_string(),
        "2026-10-18 00:30:00.123456"
    );
    let unread: i32 =
        sqlx::query_scalar("SELECT unread_count FROM sink_memberships WHERE sink_id=$1")
            .bind(h.sink)
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert_eq!(unread, 1);
    h.finish().await;
}

#[tokio::test]
async fn authentication_validation_and_form_clients_keep_their_contract() {
    let h = Harness::new().await;
    assert_eq!(
        h.send("invalid", "{}", "application/json").await.0,
        StatusCode::UNAUTHORIZED
    );
    let (status, body) = h.send(&h.token, "{}", "application/json").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"errors":["Text can't be blank"]})
    );
    assert_eq!(
        h.send(&h.token, "{", "application/json").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        h.send(
            &h.token,
            "text=Form+event&properties%5Bplan%5D=pro",
            "application/x-www-form-urlencoded"
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let row = sqlx::query("SELECT event_type,emoji,text,properties FROM events WHERE sink_id=$1")
        .bind(h.sink)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("event_type"), "event");
    assert_eq!(row.get::<String, _>("emoji"), "📌");
    assert_eq!(row.get::<Value, _>("properties"), json!({"plan":"pro"}));
    h.finish().await;
}

#[tokio::test]
async fn concurrent_retries_are_idempotent_per_source_and_do_not_overcount() {
    let h = Harness::new().await;
    let payload = json!({"text":"Original","idempotency_key":"same-key"}).to_string();
    let results = futures_util::future::join_all(
        (0..12).map(|_| h.send(&h.token, &payload, "application/json")),
    )
    .await;
    assert!(
        results
            .iter()
            .all(|(status, _)| *status == StatusCode::CREATED)
    );
    assert_eq!(
        h.send(
            &h.token,
            "{\"text\":\"Retry\",\"idempotency_key\":\"same-key\"}",
            "application/json"
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM events WHERE sink_id=$1")
        .bind(h.sink)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let unread: i32 =
        sqlx::query_scalar("SELECT unread_count FROM sink_memberships WHERE sink_id=$1")
            .bind(h.sink)
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert_eq!(unread, 1);
    let text: String = sqlx::query_scalar("SELECT text FROM events WHERE sink_id=$1")
        .bind(h.sink)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(text, "Original");
    let second = format!("{}-other", h.token);
    sqlx::query("INSERT INTO sinks (account_id,name,token,created_at,updated_at) VALUES ($1,'Other source',$2,now(),now())")
        .bind(h.account).bind(&second).execute(&h.pool).await.unwrap();
    assert_eq!(
        h.send(&second, &payload, "application/json").await.0,
        StatusCode::CREATED
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events e JOIN sinks s ON s.id=e.sink_id WHERE s.account_id=$1",
    )
    .bind(h.account)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(count, 2);
    h.finish().await;
}

#[tokio::test]
async fn actively_viewed_sources_do_not_gain_unread_counts() {
    let h = Harness::new().await;
    sqlx::query("UPDATE users SET current_sink_id=$1 WHERE id=$2")
        .bind(h.sink)
        .bind(h.user)
        .execute(&h.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE sink_memberships SET last_viewed_at=now() WHERE user_id=$1")
        .bind(h.user)
        .execute(&h.pool)
        .await
        .unwrap();
    assert_eq!(
        h.send(
            &h.token,
            "{\"text\":\"Already watching\"}",
            "application/json"
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let unread: i32 =
        sqlx::query_scalar("SELECT unread_count FROM sink_memberships WHERE sink_id=$1")
            .bind(h.sink)
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert_eq!(unread, 0);
    sqlx::query(
        "UPDATE sink_memberships SET last_viewed_at=strftime('%Y-%m-%d %H:%M:%f','now','-1 minute') WHERE user_id=$1",
    )
    .bind(h.user)
    .execute(&h.pool)
    .await
    .unwrap();
    h.send(&h.token, "{\"text\":\"Away now\"}", "application/json")
        .await;
    let unread: i32 =
        sqlx::query_scalar("SELECT unread_count FROM sink_memberships WHERE sink_id=$1")
            .bind(h.sink)
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert_eq!(unread, 1);
    h.finish().await;
}
