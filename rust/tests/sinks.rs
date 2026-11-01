// Retained Rails workflow reference; production Activity routes are tested in activity.rs.
mod common;
use axum::http::StatusCode;
use common::Harness;
use serde_json::json;

fn row_ids(html: &str) -> Vec<i64> {
    regex::Regex::new(r#"id="feed_item_event_(\d+)""#)
        .unwrap()
        .captures_iter(html)
        .map(|c| c[1].parse().unwrap())
        .collect()
}

#[tokio::test]
async fn sink_creation_editing_original_api_url_and_dependent_deletion_work() {
    let h = Harness::legacy_reference().await;
    let mut c = h.signed_in().await;
    assert!(c.get("/").await.html.contains("create your first sink"));
    assert_eq!(c.get("/sinks/new").await.status, StatusCode::OK);
    assert_eq!(
        c.form("POST", "/sinks", &[("sink[name]", "")]).await.status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let created = c
        .form("POST", "/sinks", &[("sink[name]", "Payments")])
        .await;
    assert_eq!(created.status, StatusCode::FOUND);
    let path = created.location().to_owned();
    let id: i64 = path.rsplit('/').next().unwrap().parse().unwrap();
    let token: String = sqlx::query_scalar("SELECT token FROM sinks WHERE id=$1")
        .bind(id)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(token.len(), 24);
    assert_eq!(c.get("/").await.location(), path);
    let empty_feed = c.get(&path).await;
    assert_eq!(empty_feed.status, StatusCode::OK);
    assert!(empty_feed.html.contains("no events yet"));
    assert_eq!(
        c.form("PATCH", &path, &[("sink[name]", "Revenue")])
            .await
            .location(),
        path
    );
    assert!(
        c.get(&format!("{path}/edit"))
            .await
            .html
            .replace("&#x2f;", "/")
            .contains(&format!("/api/v1/sinks/{token}/events"))
    );
    assert_eq!(
        c.send(
            "POST",
            &format!("/api/v1/sinks/{token}/events"),
            "text=old+URL+still+works&event=payment_succeeded".into(),
            &[]
        )
        .await
        .status,
        StatusCode::CREATED
    );
    assert!(c.get(&path).await.html.contains("old URL still works"));
    c.form(
        "POST",
        &format!("{path}/saved_views"),
        &[("saved_view[event_type]", "payment_succeeded")],
    )
    .await;
    sqlx::query("INSERT INTO columns (sink_id,name,position,created_at,updated_at) VALUES ($1,'legacy column',0,now(),now())").bind(id).execute(&h.pool).await.unwrap();
    let another = h.sink("Another").await;
    h.grant(h.user, another).await;
    let deleted = c.form("DELETE", &path, &[]).await;
    assert_eq!(deleted.status, StatusCode::SEE_OTHER);
    assert_eq!(deleted.location(), format!("/sinks/{another}"));
    for table in ["events", "columns", "saved_views", "sink_memberships"] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
                "SELECT count(*) FROM {table} WHERE sink_id=$1"
            )))
            .bind(id)
            .fetch_one(&h.pool)
            .await
            .unwrap(),
            0
        );
    }
    assert_eq!(c.get(&path).await.status, StatusCode::NOT_FOUND);
    assert_eq!(
        c.send(
            "POST",
            &format!("/api/v1/sinks/{token}/events"),
            "text=deleted".into(),
            &[]
        )
        .await
        .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        c.form("DELETE", &format!("/sinks/{another}"), &[])
            .await
            .location(),
        "/sinks"
    );
    h.finish().await;
}

#[tokio::test]
async fn roles_memberships_and_folders_cannot_broaden_source_access() {
    let h = Harness::legacy_reference().await;
    let outside = Harness::legacy_reference().await;
    let member = h.member("member", 0).await;
    let admin = h.member("admin", 1).await;
    let source = h.sink("Shared").await;
    let hidden = h.sink("Hidden").await;
    let foreign = outside.sink("Foreign").await;
    for user in [h.user, member, admin] {
        h.grant(user, source).await;
    }
    let hidden_event = h.event(hidden, json!({"text":"hidden"})).await;
    let mut m = h.signed_in_as(member).await;
    m.get(&format!("/sinks/{source}")).await;
    assert_eq!(m.get("/sinks/new").await.status, StatusCode::FORBIDDEN);
    assert_eq!(
        m.get(&format!("/sinks/{source}/edit")).await.status,
        StatusCode::FORBIDDEN
    );
    for method in ["PATCH", "DELETE"] {
        assert_eq!(
            m.form(
                method,
                &format!("/sinks/{source}"),
                &[("sink[name]", "changed")]
            )
            .await
            .status,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        m.form("POST", "/sinks", &[("sink[name]", "forbidden")])
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let mut owner = h.signed_in().await;
    for path in [
        format!("/sinks/{hidden}"),
        format!("/sinks/{foreign}"),
        format!("/events/{hidden_event}"),
        format!("/sinks/{hidden}/events"),
    ] {
        assert_eq!(owner.get(&path).await.status, StatusCode::NOT_FOUND);
    }
    let folder:i64=sqlx::query_scalar("INSERT INTO folders (account_id,name,created_at,updated_at) VALUES ($1,'Private',now(),now()) RETURNING id").bind(outside.account).fetch_one(&h.pool).await.unwrap();
    owner.get(&format!("/sinks/{source}/edit")).await;
    owner
        .form(
            "PATCH",
            &format!("/sinks/{source}"),
            &[
                ("sink[name]", "Safe"),
                ("sink[folder_id]", &folder.to_string()),
            ],
        )
        .await;
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>("SELECT folder_id FROM sinks WHERE id=$1")
            .bind(source)
            .fetch_one(&h.pool)
            .await
            .unwrap(),
        None
    );
    let mut a = h.signed_in_as(admin).await;
    a.get("/sinks/new").await;
    assert_eq!(
        a.form("POST", "/sinks", &[("sink[name]", "Admin source")])
            .await
            .status,
        StatusCode::FOUND
    );
    outside.finish().await;
    h.finish().await;
}

#[tokio::test]
async fn feed_pagination_frames_and_unread_state_preserve_original_workflow() {
    let h = Harness::legacy_reference().await;
    let source = h.sink("Feed").await;
    let other = h.member("other", 0).await;
    h.grant(h.user, source).await;
    h.grant(other, source).await;
    let mut ids = Vec::new();
    for i in 0..70 {
        ids.push(h.event(source,json!({"event":if i%2==0 {"payment_succeeded"}else{"deploy_finished"},"text":format!("event {i}"),"occurred_at":format!("2026-10-18T00:{:02}:{:02}Z",i/60,i%60),"properties":{"amount":i}})).await);
    }
    let mut c = h.signed_in().await;
    let path = format!("/sinks/{source}");
    c.cookies
        .insert("telesink_time_zone".into(), "Asia%2FManila".into());
    let filtered = c.get(&format!("{path}?event_type=payment_succeeded")).await;
    assert_eq!(filtered.status, StatusCode::OK);
    assert_eq!(
        row_ids(&filtered.html),
        ids.iter().step_by(2).copied().collect::<Vec<_>>()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i32>(
            "SELECT unread_count FROM sink_memberships WHERE user_id=$1 AND sink_id=$2"
        )
        .bind(h.user)
        .bind(source)
        .fetch_one(&h.pool)
        .await
        .unwrap(),
        70
    );
    c.form("POST", &format!("{path}/view"), &[("q", "event")])
        .await;
    assert_eq!(
        sqlx::query_scalar::<_, i32>(
            "SELECT unread_count FROM sink_memberships WHERE user_id=$1 AND sink_id=$2"
        )
        .bind(h.user)
        .bind(source)
        .fetch_one(&h.pool)
        .await
        .unwrap(),
        70
    );
    let page = c.get(&path).await;
    assert_eq!(page.status, StatusCode::OK);
    assert_eq!(row_ids(&page.html), ids[10..]);
    assert!(page.html.contains("data-event-feed-target=\"olderLoader\""));
    assert_eq!(
        sqlx::query_scalar::<_, i32>(
            "SELECT unread_count FROM sink_memberships WHERE user_id=$1 AND sink_id=$2"
        )
        .bind(h.user)
        .bind(source)
        .fetch_one(&h.pool)
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i32>(
            "SELECT unread_count FROM sink_memberships WHERE user_id=$1 AND sink_id=$2"
        )
        .bind(other)
        .bind(source)
        .fetch_one(&h.pool)
        .await
        .unwrap(),
        70
    );
    let older = c.get(&format!("{path}/events?before_id={}", ids[10])).await;
    assert_eq!(
        older.headers["content-type"],
        "text/vnd.turbo-stream.html; charset=utf-8"
    );
    assert_eq!(row_ids(&older.html), ids[..10]);
    assert!(
        older
            .html
            .contains("action=\"prepend\" target=\"events_feed\"")
    );
    assert!(
        row_ids(
            &c.get(&format!("{path}/events?before_id=invalid"))
                .await
                .html
        )
        .is_empty()
    );
    let sidebar = c
        .send("GET", &path, String::new(), &[("turbo-frame", "sinks")])
        .await;
    assert!(sidebar.html.contains("action=\"update\" target=\"sinks\""));
    assert!(row_ids(&sidebar.html).is_empty());
    let event = c
        .send(
            "GET",
            &format!("/events/{}", ids[0]),
            String::new(),
            &[("turbo-frame", &format!("detail_event_{}", ids[0]))],
        )
        .await;
    assert!(
        event
            .html
            .starts_with(&format!("<turbo-frame id=\"detail_event_{}\">", ids[0]))
    );
    assert!(event.html.contains("2026-10-18 08:00:00"));
    assert!(event.html.contains("above"));
    assert!(
        c.get(&format!("/events/{}", ids[0]))
            .await
            .html
            .contains("event-permalink")
    );
    h.finish().await;
}

#[tokio::test]
async fn saved_views_keep_names_filters_month_and_user_boundaries() {
    let h = Harness::legacy_reference().await;
    let source = h.sink("Views").await;
    h.grant(h.user, source).await;
    let member = h.member("member", 0).await;
    h.grant(member, source).await;
    let mut c = h.signed_in().await;
    let path = format!("/sinks/{source}");
    c.get(&path).await;
    for _ in 0..2 {
        let reply = c
            .form(
                "POST",
                &format!("{path}/saved_views"),
                &[
                    ("saved_view[name]", "urgent"),
                    ("saved_view[property_key]", "amount"),
                    ("saved_view[property_op]", "gt"),
                    ("saved_view[property_value]", "10"),
                    ("month", "2026-10-18"),
                ],
            )
            .await;
        assert_eq!(reply.status, StatusCode::SEE_OTHER);
        assert!(reply.location().contains("month=2026-10-01"));
        assert!(reply.location().contains("property_value=10"));
    }
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id,name FROM saved_views WHERE sink_id=$1 AND user_id=$2 ORDER BY name",
    )
    .bind(source)
    .bind(h.user)
    .fetch_all(&h.pool)
    .await
    .unwrap();
    assert_eq!(
        rows.iter().map(|r| r.1.as_str()).collect::<Vec<_>>(),
        vec!["urgent", "urgent 2"]
    );
    let page = c
        .get(&format!(
            "{path}?property_key=amount&property_op=gt&property_value=10"
        ))
        .await;
    assert!(page.html.contains("saved-views__item--current"));
    assert!(!page.html.contains("aria-label=\"view name\""));
    c.form(
        "POST",
        &format!("{path}/saved_views"),
        &[("saved_view[name]", "no filters")],
    )
    .await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM saved_views WHERE sink_id=$1")
            .bind(source)
            .fetch_one(&h.pool)
            .await
            .unwrap(),
        2
    );
    let mut m = h.signed_in_as(member).await;
    let html = m.get(&path).await.html;
    assert!(!html.contains("urgent"));
    assert_eq!(
        m.form("DELETE", &format!("{path}/saved_views/{}", rows[0].0), &[])
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        c.form(
            "DELETE",
            &format!("{path}/saved_views/{}?q=kept", rows[0].0),
            &[]
        )
        .await
        .location(),
        format!("{path}?q=kept")
    );
    h.finish().await;
}

#[tokio::test]
async fn event_content_is_escaped_and_property_links_keep_filter_context() {
    let h = Harness::legacy_reference().await;
    let source = h.sink("Escaping").await;
    h.grant(h.user, source).await;
    let id=h.event(source,json!({"text":"<script>event</script>","event":"custom_name","properties":{"empty":"","html":"<img src=x onerror=alert(1)>","amount":"2.5","object":{"foo":"bar"}}})).await;
    let mut c = h.signed_in().await;
    let page = c.get(&format!("/sinks/{source}")).await;
    assert!(page.html.contains("&lt;script&gt;event"));
    assert!(!page.html.contains("<script>event"));
    let detail = c
        .get(&format!(
            "/events/{id}?event_type=custom_name&q=script&month=2026-10-01"
        ))
        .await;
    assert!(!detail.html.contains("<img src=x"));
    assert!(detail.html.contains("&lt;img src=x"));
    let html = detail.html.replace("&#x2f;", "/").replace("&amp;", "&");
    assert!(
        html.contains("property_key=amount&property_op=gt&property_value=2.5&month=2026-10-01")
    );
    assert!(html.contains("event_type=custom_name&q=script"));
    assert!(html.contains("property_key=empty&property_op=eq&property_value=&month="));
    h.finish().await;
}
