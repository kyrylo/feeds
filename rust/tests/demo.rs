mod common;
use axum::http::StatusCode;
use common::{Harness, delete_account};
use feeds::{AppState, config::Config};
use std::sync::Arc;

fn enable(h: &mut Harness) {
    h.state = AppState::new(
        h.pool.clone(),
        Arc::new(Config {
            database_url: h.state.config.database_url.clone(),
            secret_key_base: h.state.config.secret_key_base.clone(),
            bind: h.state.config.bind,
            public_url: None,
            database_connections: h.state.config.database_connections,
            mailer: None,
            demo: true,
        }),
    );
    h.app = feeds::router(h.state.clone());
}

#[tokio::test]
async fn anonymous_demo_visits_create_one_valid_account_and_reuse_existing_sources() {
    let mut h = Harness::new().await;
    enable(&mut h);
    let existing: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE email_address='demo@telesink.com'")
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert_eq!(
        existing, 0,
        "demo test requires its synthetic user to be absent"
    );
    let (mut first, mut second) = (h.client(), h.client());
    let (a, b) = tokio::join!(first.get("/"), second.get("/"));
    assert_eq!(a.location(), "/");
    assert_eq!(b.location(), "/");
    assert_ne!(first.cookies["session_id"], second.cookies["session_id"]);
    let rows: Vec<(i64, i64, String)> = sqlx::query_as(
        "SELECT id,account_id,password_digest FROM users WHERE email_address='demo@telesink.com'",
    )
    .fetch_all(&h.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    let (demo, account, digest) = &rows[0];
    assert!(digest.starts_with("$2b$12$"));
    assert_eq!(first.get("/").await.status, StatusCode::OK);
    let private = h.sink("Private").await;
    assert_eq!(
        first.get(&format!("/sinks/{private}")).await.status,
        StatusCode::NOT_FOUND
    );
    let sink: i64 = sqlx::query_scalar("INSERT INTO sinks (account_id,name,token,created_at,updated_at) VALUES ($1,'Existing demo source','synthetic-demo-token',now(),now()) RETURNING id")
        .bind(account).fetch_one(&h.pool).await.unwrap();
    h.grant(*demo, sink).await;
    let mut restarted = h.client();
    let path = format!("/sinks/{sink}");
    assert_eq!(restarted.get(&path).await.location(), path);
    assert_eq!(restarted.get(&path).await.status, StatusCode::OK);
    let preserved: (i64, String) =
        sqlx::query_as("SELECT account_id,password_digest FROM users WHERE id=$1")
            .bind(demo)
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert_eq!(preserved, (*account, digest.clone()));
    delete_account(&h.pool, *account).await;
    h.finish().await;
}

#[tokio::test]
async fn demo_keeps_reading_and_saved_views_but_blocks_profile_account_and_source_mutations() {
    let mut h = Harness::new().await;
    let sink = h.sink("Demo").await;
    h.grant(h.user, sink).await;
    let event = h
        .event(
            sink,
            serde_json::json!({"event":"exception","text":"visible demo event"}),
        )
        .await;
    enable(&mut h);
    let mut c = h.signed_in().await;
    let path = format!("/sinks/{sink}");
    let page = c.get(&path).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.html.contains("Tester"));
    assert!(!page.html.contains("~Tester"));
    assert!(!page.html.contains("log out"));
    assert!(!page.html.contains("href=\"/settings/nickname/edit\""));
    assert!(
        page.html
            .contains("name=\"turbo-prefetch\" content=\"false\"")
    );
    assert_eq!(
        c.get(&format!("/events/{event}")).await.status,
        StatusCode::OK
    );
    for url in [
        "/sinks/new",
        "/folders/new",
        "/settings",
        "/registration/new",
        "/passwords/new",
        "/session/new",
    ] {
        assert_eq!(c.get(url).await.location(), "/", "{url}");
    }
    assert_eq!(c.form("DELETE", &path, &[]).await.location(), "/");
    assert_eq!(
        c.form(
            "PATCH",
            "/settings/nickname",
            &[("user[nickname]", "changed")]
        )
        .await
        .location(),
        "/"
    );
    assert_eq!(
        c.form("POST", "/passwords", &[("email_address", &h.email)])
            .await
            .location(),
        "/"
    );
    assert_eq!(c.form("DELETE", "/session", &[]).await.location(), "/");
    let saved = c
        .form(
            "POST",
            &format!("{path}/saved_views"),
            &[
                ("saved_view[name]", "exceptions"),
                ("saved_view[event_type]", "exception"),
            ],
        )
        .await;
    assert_eq!(saved.location(), format!("{path}?event_type=exception"));
    let view: i64 =
        sqlx::query_scalar("SELECT id FROM saved_views WHERE user_id=$1 AND sink_id=$2")
            .bind(h.user)
            .bind(sink)
            .fetch_one(&h.pool)
            .await
            .unwrap();
    assert_eq!(
        c.form("DELETE", &format!("{path}/saved_views/{view}"), &[])
            .await
            .location(),
        "/"
    );
    assert_eq!(
        c.form("POST", &format!("{path}/view"), &[]).await.status,
        StatusCode::OK
    );
    let user: String = sqlx::query_scalar("SELECT nickname FROM users WHERE id=$1")
        .bind(h.user)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(user, "Tester");
    assert_eq!(c.get(&path).await.status, StatusCode::OK);
    assert_eq!(
        c.send(
            "POST",
            &format!("/api/v1/sinks/{}-Demo/events", h.account),
            "text=demo+API+still+works".into(),
            &[]
        )
        .await
        .status,
        StatusCode::CREATED
    );
    h.finish().await;
}

#[tokio::test]
async fn demo_column_exceptions_keep_role_checks_and_restriction_redirects_stay_local() {
    let mut h = Harness::new().await;
    let sink = h.sink("Columns").await;
    h.grant(h.user, sink).await;
    let member = h.member("Viewer", 0).await;
    h.grant(member, sink).await;
    enable(&mut h);
    let mut c = h.signed_in().await;
    let path = format!("/sinks/{sink}");
    c.get(&path).await;
    assert_eq!(
        c.form("POST", &format!("{path}/columns"), &[])
            .await
            .location(),
        path
    );
    let column: i64 = sqlx::query_scalar("SELECT id FROM columns WHERE sink_id=$1")
        .bind(sink)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    let column_path = format!("{path}/columns/{column}");
    assert_eq!(
        c.form("PATCH", &column_path, &[("column[name]", "Changed")])
            .await
            .location(),
        path
    );
    assert_eq!(
        c.form(
            "PATCH",
            &format!("{path}/column_order"),
            &[("column_order[column_ids][]", &column.to_string())]
        )
        .await
        .status,
        StatusCode::OK
    );
    let mut viewer = h.signed_in_as(member).await;
    viewer.get(&path).await;
    assert_eq!(
        viewer
            .form("POST", &format!("{path}/columns"), &[])
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        viewer
            .form("POST", &format!("{column_path}/views"), &[])
            .await
            .status,
        StatusCode::OK
    );
    let local = format!("http://localhost:3001{path}?q=kept");
    assert_eq!(
        c.send("GET", "/settings", String::new(), &[("referer", &local)])
            .await
            .location(),
        format!("{path}?q=kept")
    );
    for remote in [
        "https://external.example/",
        "http://localhost:3001//external.example/",
        "http://localhost:3001.evil.example/",
    ] {
        assert_eq!(
            c.send("GET", "/settings", String::new(), &[("referer", remote)])
                .await
                .location(),
            "/"
        );
    }
    assert_eq!(c.form("DELETE", &column_path, &[]).await.location(), path);
    h.finish().await;
}
