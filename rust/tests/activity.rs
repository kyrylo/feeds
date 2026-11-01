mod common;
use axum::http::StatusCode;
use common::{Client, Harness};
use serde_json::{Value, json};

fn ids(html: &str) -> Vec<i64> {
    regex::Regex::new(r#"data-event-id="(\d+)""#)
        .unwrap()
        .captures_iter(html)
        .map(|c| c[1].parse().unwrap())
        .collect()
}
fn attr(html: &str, name: &str) -> String {
    regex::Regex::new(&format!(r#"{name}="([^"]*)""#))
        .unwrap()
        .captures(html)
        .unwrap()[1]
        .replace("&#x2f;", "/")
        .replace("&amp;", "&")
}
#[tokio::test]
async fn text_search_and_details_respect_sources_and_escape_payloads() {
    let h = Harness::new().await;
    let foreign = Harness::new().await;
    let a = h.sink("Acme").await;
    let b = h.sink("Papertrail").await;
    let hidden = h.sink("Hidden").await;
    let other = foreign.sink("Other").await;
    for s in [a, b] {
        h.grant(h.user, s).await;
    }
    let first=h.event(a,json!({"text":"Maya upgraded <script>alert(1)</script>","event":"upgrade","properties":{"plan":"Pro","amount":29,"big":9007199254740993u64},"occurred_at":"2026-10-18T01:00:00Z"})).await;
    let second=h.event(b,json!({"text":"Payment failed","event":"payment_failed","properties":{"amount":49},"occurred_at":"2026-10-18T02:00:00Z"})).await;
    let invisible = h.event(hidden, json!({"text":"hidden payload"})).await;
    h.event(other, json!({"text":"foreign payload"})).await;
    let mut c = h.signed_in().await;
    let page = c.get("/?_filters=1").await;
    assert_eq!(page.status, StatusCode::OK);
    assert_eq!(ids(&page.html), [first, second]);
    assert_eq!(attr(&page.html, "data-cursor"), second.to_string());
    assert!(page.html.contains("+ new feed"));
    assert!(!page.html.contains("/sinks/new"));
    assert!(!page.html.contains("/folders"));
    assert!(!page.html.contains("hidden payload"));
    assert!(!page.html.contains("foreign payload"));
    assert!(!page.html.contains("<script>alert"));
    assert!(page.html.contains("&lt;script&gt;"));
    for (path, expected) in [
        (format!("/?source={a}"), vec![first]),
        ("/?q=Payment".into(), vec![second]),
        ("/?q=upgrade".into(), vec![first]),
        ("/?q=49".into(), vec![second]),
        ("/?q=plan".into(), vec![first]),
        ("/?q=Pro".into(), vec![first]),
        ("/?q=9007199254740993".into(), vec![first]),
        ("/?q=nonmatching".into(), vec![]),
        (
            "/?q=Payment&event_type=upgrade&date=2026-10-17&property_key=amount&property_op=lt&property_value=30".into(),
            vec![second],
        ),
    ] {
        let page = c.get(&path).await;
        assert_eq!(page.status, StatusCode::OK, "{path}");
        assert_eq!(ids(&page.html), expected, "{path}");
    }
    assert_eq!(
        c.get(&format!("/?source={hidden}")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        c.get(&format!("/activity/events/{invisible}")).await.status,
        StatusCode::NOT_FOUND
    );
    let detail = c.get(&format!("/events/{first}")).await;
    assert_eq!(detail.status, StatusCode::OK);
    assert!(detail.html.contains("9007199254740993"));
    assert!(!detail.html.contains("<script>alert"));
    assert!(detail.html.contains("Permalink"));
    assert!(!detail.html.contains("Raw event"));
    assert!(!detail.html.contains("Copy JSON"));
    let fragment = c.get(&format!("/activity/events/{first}")).await;
    assert!(!fragment.html.contains("<!doctype"));
    assert!(fragment.html.contains("<dt>plan</dt><dd>Pro</dd>"));
    assert!(!fragment.html.contains("property_key="));
    assert!(!fragment.html.contains("Raw event"));
    assert_eq!(
        c.get(&format!("/sinks/{a}?q=Maya")).await.location(),
        format!("/?q=Maya&source={a}")
    );
    assert_eq!(c.get("/sinks/new").await.location(), "/connect");
    assert_eq!(
        c.form("POST", "/sinks", &[("sink[name]", "Removed UI")])
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        c.get(&format!("/sinks/{hidden}")).await.status,
        StatusCode::NOT_FOUND
    );
    foreign.finish().await;
    h.finish().await;
}

#[tokio::test]
async fn pagination_and_live_cursors_cover_races_late_events_filters_and_large_ids() {
    let h = Harness::new().await;
    let source = h.sink("Busy").await;
    h.grant(h.user, source).await;
    // Exact IDs must survive both server pagination and JavaScript's JSON boundary.
    sqlx::query("INSERT INTO sqlite_sequence(name,seq) SELECT 'events',9007199254740992 WHERE NOT EXISTS(SELECT 1 FROM sqlite_sequence WHERE name='events')").execute(&h.pool).await.unwrap();
    sqlx::query("UPDATE sqlite_sequence SET seq=MAX(seq,9007199254740992) WHERE name='events'")
        .execute(&h.pool)
        .await
        .unwrap();
    let mut expected = Vec::new();
    for i in 0..111 {
        expected.push(
            h.event(
                source,
                json!({"text":format!("row {i}"),"occurred_at":"2026-10-18T00:00:00Z"}),
            )
            .await,
        );
    }
    let mut c = h.signed_in().await;
    let page = c.get("/?_filters=1").await;
    let cursor = attr(&page.html, "data-cursor");
    let seen = attr(&page.html, "data-seen");
    let mut found = ids(&page.html);
    assert_eq!(found.len(), 50);
    assert_eq!(found, expected[61..]);
    assert!(
        page.html.find("id=\"load-older\"").unwrap()
            < page.html.find("id=\"activity-list\"").unwrap()
    );
    let mut older = regex::Regex::new(r#"id="load-older" href="([^"]+)""#)
        .unwrap()
        .captures(&page.html)
        .unwrap()[1]
        .replace("&amp;", "&")
        .replace("&#x2f;", "/");
    // New, backdated arrivals must be found by the live cursor, never skipped by time pagination.
    let late = h
        .event(
            source,
            json!({"text":"late arrival","occurred_at":"2020-01-01T00:00:00Z"}),
        )
        .await;
    loop {
        let page = c
            .send(
                "GET",
                &older,
                String::new(),
                &[("accept", "application/json")],
            )
            .await;
        assert_eq!(page.status, StatusCode::OK);
        let data: Value = serde_json::from_str(&page.html).unwrap();
        let earlier = ids(data["html"].as_str().unwrap());
        assert!(earlier.windows(2).all(|pair| pair[0] < pair[1]));
        found.splice(0..0, earlier);
        if let Some(next) = data["older_url"].as_str() {
            older = next.into();
        } else {
            break;
        }
    }
    assert_eq!(found, expected);
    assert!(!found.contains(&late));
    let reply = c
        .get(&format!("/activity/updates?after={cursor}&seen={seen}"))
        .await;
    let data: Value = serde_json::from_str(&reply.html).unwrap();
    assert_eq!(ids(data["html"].as_str().unwrap()), [late]);
    assert!(data["html"].as_str().unwrap().contains("data-new=\"true\""));
    assert_eq!(data["cursor"], late.to_string());
    let empty = c
        .get(&format!("/activity/updates?after={cursor}&q=nonmatching"))
        .await;
    let data: Value = serde_json::from_str(&empty.html).unwrap();
    assert!(ids(data["html"].as_str().unwrap()).is_empty());
    assert_eq!(data["cursor"], late.to_string());
    assert!(data["seen_token"].is_null());
    h.finish().await;
}

#[tokio::test]
async fn live_batches_display_chronologically_without_changing_the_arrival_cursor() {
    let h = Harness::new().await;
    let source = h.sink("Backdated").await;
    h.grant(h.user, source).await;
    let mut arrivals = Vec::new();
    for minute in (0..60).rev() {
        arrivals.push(
            h.event(
                source,
                json!({
                    "text":format!("minute {minute}"),
                    "occurred_at":format!("2026-10-18T00:{minute:02}:00Z")
                }),
            )
            .await,
        );
    }
    let mut c = h.signed_in().await;
    let page = c.get(&format!("/?source={source}")).await;
    let first_page: Vec<_> = arrivals[..50].iter().copied().rev().collect();
    assert_eq!(ids(&page.html), first_page);
    let live: Value = serde_json::from_str(
        &c.get(&format!("/activity/updates?source={source}&after=0"))
            .await
            .html,
    )
    .unwrap();
    assert_eq!(ids(live["html"].as_str().unwrap()), first_page);
    assert_eq!(live["cursor"], arrivals[49].to_string());
    assert_eq!(live["more"], true);
    let rest: Value = serde_json::from_str(
        &c.get(&format!(
            "/activity/updates?source={source}&after={}",
            arrivals[49]
        ))
        .await
        .html,
    )
    .unwrap();
    assert_eq!(
        ids(rest["html"].as_str().unwrap()),
        arrivals[50..].iter().copied().rev().collect::<Vec<_>>()
    );
    assert_eq!(rest["cursor"], arrivals[59].to_string());
    assert_eq!(rest["more"], false);
    h.finish().await;
}

#[tokio::test]
async fn catch_up_is_persistent_monotonic_scoped_and_requires_csrf_and_signed_snapshot() {
    let h = Harness::new().await;
    let source = h.sink("Visits").await;
    h.grant(h.user, source).await;
    let first = h.event(source, json!({"text":"before visit"})).await;
    let mut c = h.signed_in().await;
    let page = c.get("/?_filters=1").await;
    let token = attr(&page.html, "data-seen-token");
    assert_eq!(
        c.form("POST", "/activity/seen", &[("seen_token", &token)])
            .await
            .status,
        StatusCode::OK
    );
    let second = h.event(source, json!({"text":"since visit"})).await;
    let page = c.get("/?_filters=1").await;
    assert!(page.html.contains(r#"id="new-count">1</span>"#));
    let newer = attr(&page.html, "data-seen-token");
    assert_eq!(
        c.send("POST", "/activity/seen", format!("seen_token={newer}"), &[])
            .await
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        c.form("POST", "/activity/seen", &[("seen_token", "forged")])
            .await
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        c.form("POST", "/activity/seen", &[("seen_token", &newer)])
            .await
            .status,
        StatusCode::OK
    );
    c.form("POST", "/activity/seen", &[("seen_token", &token)])
        .await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT last_seen_id FROM telesink_activity_visits WHERE user_id=$1"
        )
        .bind(h.user)
        .fetch_one(&h.pool)
        .await
        .unwrap(),
        second
    );
    assert!(second > first);
    let filtered = c.get(&format!("/?source={source}")).await;
    assert!(attr(&filtered.html, "data-seen-token").is_empty());
    let member = h.member("another", 0).await;
    let mut other = h.signed_in_as(member).await;
    other.get("/?_filters=1").await;
    assert_eq!(
        other
            .form("POST", "/activity/seen", &[("seen_token", &newer)])
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    h.finish().await;
}

async fn create(c: &mut Client, name: &str) -> i64 {
    let reply = c.form("POST", "/feeds", &[("name", name)]).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER, "{}", reply.html);
    reply
        .location()
        .strip_prefix("/feeds/")
        .unwrap()
        .strip_suffix("/edit")
        .unwrap()
        .parse()
        .unwrap()
}
async fn token(h: &Harness, id: i64) -> (i64, String) {
    sqlx::query_as("SELECT s.id,s.token FROM sinks s JOIN telesink_activity_tabs t ON t.source_id=s.id WHERE t.id=$1")
        .bind(id).fetch_one(&h.pool).await.unwrap()
}
async fn send(c: &mut Client, token: &str, raw: Value) -> common::Reply {
    c.send(
        "POST",
        &format!("/api/v1/feeds/{token}/events"),
        raw.to_string(),
        &[("content-type", "application/json")],
    )
    .await
}

#[tokio::test]
async fn feed_lifecycle_creates_real_sources_preserves_urls_on_rename_and_deletes_only_its_events()
{
    let h = Harness::new().await;
    let mut c = h.signed_in().await;
    c.get("/connect").await;
    let a = create(&mut c, "Deploys").await;
    let b = create(&mut c, "Payments").await;
    let (source, a_token) = token(&h, a).await;
    let (other_source, b_token) = token(&h, b).await;
    assert_ne!(a_token, b_token);
    assert_eq!(
        send(
            &mut c,
            &a_token,
            json!({"text":"Deploy complete","feed":"Payments"})
        )
        .await
        .status,
        StatusCode::CREATED
    );
    assert_eq!(
        send(&mut c, &b_token, json!({"text":"Payment received"}))
            .await
            .status,
        StatusCode::CREATED
    );
    let page = c.get(&format!("/feeds/{a}")).await;
    assert!(page.html.contains("Deploy complete"));
    assert!(!page.html.contains("Payment received"));
    assert!(page.html.contains("<title>Deploys · Feeds</title>"));
    assert!(!page.html.contains("Save search"));
    assert_eq!(
        c.form(
            "POST",
            &format!("/feeds/{a}"),
            &[("_method", "patch"), ("name", "Production")]
        )
        .await
        .location(),
        format!("/feeds/{a}")
    );
    assert_eq!(token(&h, a).await.1, a_token);
    assert!(
        c.get(&format!("/feeds/{a}"))
            .await
            .html
            .contains("<title>Production · Feeds</title>")
    );
    let connections = c.get("/connect").await;
    assert!(connections.html.contains("#Production"));
    assert!(connections.html.contains("#Payments"));
    assert!(!connections.html.contains("api-key"));
    // Exercise cleanup of retained legacy dependencies as well as events.
    sqlx::query(
        "INSERT INTO columns(sink_id,name,created_at,updated_at) VALUES ($1,'legacy',now(),now())",
    )
    .bind(source)
    .execute(&h.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO saved_views(sink_id,user_id,name,created_at,updated_at) VALUES ($1,$2,'legacy',now(),now())").bind(source).bind(h.user).execute(&h.pool).await.unwrap();
    sqlx::query("UPDATE users SET current_sink_id=$1 WHERE id=$2")
        .bind(source)
        .bind(h.user)
        .execute(&h.pool)
        .await
        .unwrap();
    assert_eq!(
        c.form("POST", &format!("/feeds/{a}"), &[("_method", "delete")])
            .await
            .location(),
        "/connect"
    );
    assert_eq!(
        c.get(&format!("/feeds/{a}")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&mut c, &a_token, json!({"text":"after deletion"}))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    for table in ["events", "saved_views", "columns", "sink_memberships"] {
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM {table} WHERE sink_id=$1"
        )))
        .bind(source)
        .fetch_one(&h.pool)
        .await
        .unwrap();
        assert_eq!(count, 0);
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM events WHERE sink_id=$1")
            .bind(other_source)
            .fetch_one(&h.pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        send(&mut c, &b_token, json!({"text":"still works"}))
            .await
            .status,
        StatusCode::CREATED
    );
    feeds::schema::check(&h.pool).await.unwrap();
    h.finish().await;
}

#[tokio::test]
async fn per_feed_urls_retry_independently_and_rotation_revokes_both_url_spellings() {
    let h = Harness::new().await;
    let mut c = h.signed_in().await;
    c.get("/").await;
    let a = create(&mut c, "First").await;
    let b = create(&mut c, "Second").await;
    let (source, key) = token(&h, a).await;
    let (_, other) = token(&h, b).await;
    let body =
        json!({"event":"paid","emoji":"💸","text":"Paid","properties":{"amount":29}}).to_string();
    for (route, t) in [
        ("feeds", &key),
        ("feeds", &key),
        ("sinks", &key),
        ("feeds", &other),
    ] {
        assert_eq!(
            c.send(
                "POST",
                &format!("/api/v1/{route}/{t}/events"),
                body.clone(),
                &[
                    ("content-type", "application/json"),
                    ("idempotency-key", "same-retry")
                ]
            )
            .await
            .status,
            StatusCode::CREATED
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM events WHERE sink_id=$1")
            .bind(source)
            .fetch_one(&h.pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        c.form("POST", &format!("/feeds/{a}/url"), &[("_method", "patch")])
            .await
            .status,
        StatusCode::SEE_OTHER
    );
    let (_, replacement) = token(&h, a).await;
    assert_ne!(key, replacement);
    for route in ["feeds", "sinks"] {
        assert_eq!(
            c.send(
                "POST",
                &format!("/api/v1/{route}/{key}/events"),
                body.clone(),
                &[("content-type", "application/json")]
            )
            .await
            .status,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        send(&mut c, &replacement, json!({"text":"new URL"}))
            .await
            .status,
        StatusCode::CREATED
    );
    assert_eq!(
        send(&mut c, &other, json!({"text":"other URL"}))
            .await
            .status,
        StatusCode::CREATED
    );
    assert_eq!(
        send(
            &mut c,
            &replacement,
            json!({"text":"invalid","emoji":"123456789"})
        )
        .await
        .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        c.send(
            "POST",
            &format!("/api/v1/feeds/{replacement}/events"),
            body,
            &[
                ("content-type", "application/json"),
                ("idempotency-key", &"a".repeat(256))
            ]
        )
        .await
        .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        c.form("POST", "/connect/key", &[]).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        c.send(
            "POST",
            "/events",
            json!({"feed":"Unexpected","text":"must not create"}).to_string(),
            &[("content-type", "application/json")]
        )
        .await
        .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    h.finish().await;
}

#[tokio::test]
async fn feed_management_requires_owner_membership_and_csrf_and_validates_names() {
    let h = Harness::new().await;
    let foreign = Harness::new().await;
    let mut c = h.signed_in().await;
    c.get("/").await;
    let id = create(&mut c, "Valid").await;
    for name in ["", "   ", &"a".repeat(81), "vALID"] {
        assert_eq!(
            c.form("POST", "/feeds", &[("name", name)]).await.status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let mut other = foreign.signed_in().await;
    other.get("/").await;
    for path in [
        format!("/feeds/{id}"),
        format!("/feeds/{id}/edit"),
        format!("/feeds/{id}/updates?after=0"),
    ] {
        assert_eq!(other.get(&path).await.status, StatusCode::NOT_FOUND);
    }
    for (method, path) in [
        ("PATCH", format!("/feeds/{id}")),
        ("DELETE", format!("/feeds/{id}")),
        ("PATCH", format!("/feeds/{id}/url")),
        ("PATCH", format!("/feeds/{id}/move")),
    ] {
        assert_eq!(
            other
                .form(method, &path, &[("name", "stolen"), ("direction", "left")])
                .await
                .status,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            c.send(method, &path, String::new(), &[]).await.status,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    sqlx::query("UPDATE users SET role=0 WHERE id=$1")
        .bind(h.user)
        .execute(&h.pool)
        .await
        .unwrap();
    for path in [
        "/connect".into(),
        "/feeds/new".into(),
        format!("/feeds/{id}/edit"),
    ] {
        assert_eq!(c.get(&path).await.status, StatusCode::FORBIDDEN);
    }
    assert_eq!(
        c.form("POST", "/feeds", &[("name", "no")]).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        c.form("DELETE", &format!("/feeds/{id}"), &[]).await.status,
        StatusCode::FORBIDDEN
    );
    h.finish().await;
    foreign.finish().await;
}

#[tokio::test]
async fn feed_resource_scopes_search_polling_and_unread_progress_and_escapes_names() {
    let h = Harness::new().await;
    let mut c = h.signed_in().await;
    c.get("/").await;
    let a = create(&mut c, "<script>Deploys</script>").await;
    let b = create(&mut c, "Payments").await;
    let (source, _) = token(&h, a).await;
    let (other, _) = token(&h, b).await;
    let first = h.event(source, json!({"text":"failed deploy"})).await;
    h.event(other, json!({"text":"failed payment"})).await;
    let page = c
        .get(&format!("/feeds/{a}?q=failed&source={other}&_filters=1"))
        .await;
    assert_eq!(ids(&page.html), [first]);
    assert!(
        page.html
            .contains("&lt;script&gt;Deploys&lt;&#x2f;script&gt; · Feeds")
    );
    assert!(!page.html.contains("<script>Deploys"));
    let updates = c
        .get(&format!(
            "/feeds/{a}/updates?after=0&source={other}&_filters=1"
        ))
        .await;
    let data: Value = serde_json::from_str(&updates.html).unwrap();
    assert_eq!(ids(data["html"].as_str().unwrap()), [first]);
    let scoped = attr(&c.get(&format!("/feeds/{a}")).await.html, "data-seen-token");
    assert_eq!(
        c.form("POST", "/activity/seen", &[("seen_token", &scoped)])
            .await
            .status,
        StatusCode::OK
    );
    let counts: Value =
        serde_json::from_str(&c.get("/activity/updates?after=0").await.html).unwrap();
    let tabs = counts["counts"]["tabs"].as_array().unwrap();
    let a_id = a.to_string();
    let b_id = b.to_string();
    assert_eq!(
        tabs.iter()
            .find(|t| t["id"].as_str() == Some(a_id.as_str()))
            .unwrap()["unread"],
        0
    );
    assert_eq!(
        tabs.iter()
            .find(|t| t["id"].as_str() == Some(b_id.as_str()))
            .unwrap()["unread"],
        1
    );
    let search = c.get(&format!("/feeds/{a}?q=missing")).await;
    assert!(ids(&search.html).is_empty());
    assert!(search.html.contains("Clear search"));
    assert!(!search.html.contains("Save search"));
    let edit = c.get(&format!("/feeds/{a}/edit")).await;
    assert!(!edit.html.contains("name=\"source\""));
    assert!(!edit.html.contains("name=\"q\""));
    assert_eq!(
        c.form(
            "POST",
            &format!("/feeds/{b}/move"),
            &[("_method", "patch"), ("direction", "left")]
        )
        .await
        .status,
        StatusCode::SEE_OTHER
    );
    assert!(
        c.get("/")
            .await
            .html
            .contains("<title>Payments · Feeds</title>")
    );
    h.finish().await;
}

#[tokio::test]
async fn connection_examples_use_public_origin_without_shared_key_or_feed_selector() {
    use std::sync::Arc;
    let mut h = Harness::new().await;
    let config = feeds::config::Config {
        public_url: Some("https://events.example.test".into()),
        ..(*h.state.config).clone()
    };
    h.state = feeds::AppState::new(h.pool.clone(), Arc::new(config));
    h.app = feeds::router(h.state.clone());
    let mut c = h.signed_in().await;
    c.get("/").await;
    let id = create(&mut c, "Deploys").await;
    let (_, key) = token(&h, id).await;
    let page = c.get(&format!("/feeds/{id}/edit")).await;
    let decoded = page
        .html
        .replace("&#x2f;", "/")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&quot;", "\"");
    assert!(decoded.contains(&format!(
        "https://events.example.test/api/v1/feeds/{key}/events"
    )));
    assert!(!decoded.contains("Authorization:"));
    assert!(!decoded.contains("\"feed\":"));
    assert!(decoded.contains("curl '") && decoded.contains(" \\\n  -H"));
    h.finish().await;
}
