// Retained Rails workflow reference; production Activity routes are tested in activity.rs.
mod common;
use axum::http::StatusCode;
use common::{Client, Harness};
use serde_json::{Value, json};

async fn stream(
    client: &mut Client,
    method: &str,
    path: &str,
    fields: &[(&str, &str)],
) -> common::Reply {
    let csrf = client.cookies["telesink_csrf"].clone();
    let mut fields = fields.to_vec();
    fields.push(("authenticity_token", &csrf));
    client
        .send(
            method,
            path,
            serde_urlencoded::to_string(fields).unwrap(),
            &[("accept", "text/vnd.turbo-stream.html")],
        )
        .await
}

#[tokio::test]
async fn existing_column_config_and_events_survive_native_management() {
    let h = Harness::legacy_reference().await;
    let sink = h.sink("Columns").await;
    h.grant(h.user, sink).await;
    let event = h
        .event(
            sink,
            json!({"event":"payment_failed","text":"Invoice <script>unsafe</script> failed"}),
        )
        .await;
    let mut owner = h.signed_in().await;
    owner.get(&format!("/sinks/{sink}")).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM columns WHERE sink_id=$1")
            .bind(sink)
            .fetch_one(&h.pool)
            .await
            .unwrap(),
        0,
        "the current Rails app no longer creates a default column"
    );
    let created = stream(&mut owner, "POST", &format!("/sinks/{sink}/columns"), &[]).await;
    assert_eq!(created.status, StatusCode::OK);
    assert!(
        created
            .html
            .contains("action=\"append\" target=\"columns\"")
    );
    let id: i64 = sqlx::query_scalar("SELECT id FROM columns WHERE sink_id=$1")
        .bind(sink)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT has_events FROM columns WHERE id=$1")
            .bind(id)
            .fetch_one(&h.pool)
            .await
            .unwrap()
    );
    let path = format!("/sinks/{sink}/columns/{id}");
    assert_eq!(owner.get(&path).await.status, StatusCode::SEE_OTHER);
    assert_eq!(owner.get(&path).await.location(), format!("/sinks/{sink}"));
    let shown = owner
        .send(
            "GET",
            &path,
            String::new(),
            &[("turbo-frame", &format!("column_{id}"))],
        )
        .await;
    assert_eq!(shown.status, StatusCode::OK);
    assert!(shown.html.contains(&format!("id=\"column_{id}\"")));
    assert!(!shown.html.contains("<script>"));
    assert!(shown.html.contains("&lt;script&gt;"));
    sqlx::query("UPDATE columns SET config=$1 WHERE id=$2")
        .bind(json!({"legacy_option":"keep","filters":{"event_types":["payment_failed"]}}))
        .bind(id)
        .execute(&h.pool)
        .await
        .unwrap();
    let edit = owner.get(&format!("{path}/edit")).await;
    assert_eq!(edit.status, StatusCode::OK);
    assert!(edit.html.contains("value=\"payment_failed\" selected"));
    let update = stream(
        &mut owner,
        "PATCH",
        &path,
        &[
            ("column[name]", "Failures"),
            ("column[event_types][]", ""),
            ("column[event_types][]", "payment_failed"),
            ("column[search]", "  invoice  "),
        ],
    )
    .await;
    assert_eq!(update.status, StatusCode::OK);
    assert!(update.html.contains("action=\"replace\""));
    assert!(
        update
            .html
            .contains("<mark class=\"event-preview__highlight\">Invoice</mark>")
    );
    assert!(!update.html.contains("<script>"));
    let config: Value = sqlx::query_scalar("SELECT config FROM columns WHERE id=$1")
        .bind(id)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    assert_eq!(
        config,
        json!({"legacy_option":"keep","filters":{"event_types":["payment_failed"],"search":"invoice"}})
    );
    assert_eq!(
        owner
            .form("PATCH", &path, &[("column[name]", "")])
            .await
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT name FROM columns WHERE id=$1")
            .bind(id)
            .fetch_one(&h.pool)
            .await
            .unwrap(),
        "Failures"
    );
    assert_eq!(
        owner
            .form("PATCH", &path, &[("column[name]", "All")])
            .await
            .location(),
        format!("/sinks/{sink}")
    );
    assert_eq!(
        sqlx::query_scalar::<_, Value>("SELECT config FROM columns WHERE id=$1")
            .bind(id)
            .fetch_one(&h.pool)
            .await
            .unwrap(),
        json!({"legacy_option":"keep","filters":{}})
    );
    assert_eq!(
        owner.form("DELETE", &path, &[]).await.status,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        owner.get(&format!("/events/{event}")).await.status,
        StatusCode::OK
    );
    assert_eq!(owner.get(&path).await.status, StatusCode::NOT_FOUND);
    h.finish().await;
}

#[tokio::test]
async fn column_permissions_reorder_rollback_and_view_timestamps_are_scoped() {
    let h = Harness::legacy_reference().await;
    let outside = Harness::legacy_reference().await;
    let sink = h.sink("Shared").await;
    let hidden = h.sink("Hidden").await;
    let foreign = outside.sink("Foreign").await;
    let member = h.member("member", 0).await;
    let admin = h.member("admin", 1).await;
    for user in [h.user, member, admin] {
        h.grant(user, sink).await;
    }
    let mut ids = Vec::new();
    for source in [sink, sink, hidden, foreign] {
        ids.push(sqlx::query_scalar::<_,i64>("INSERT INTO columns (sink_id,name,created_at,updated_at) VALUES ($1,'Legacy',now(),now()) RETURNING id").bind(source).fetch_one(&h.pool).await.unwrap());
    }
    let path = format!("/sinks/{sink}/columns/{}", ids[0]);
    let order = format!("/sinks/{sink}/column_order");
    let mut m = h.signed_in_as(member).await;
    m.get(&format!("/sinks/{sink}")).await;
    assert_eq!(
        stream(&mut m, "GET", &path, &[]).await.status,
        StatusCode::OK
    );
    assert_eq!(
        m.get(&format!("{path}/edit")).await.status,
        StatusCode::FORBIDDEN
    );
    for method in ["PATCH", "DELETE"] {
        assert_eq!(
            m.form(method, &path, &[("column[name]", "bad")])
                .await
                .status,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        m.form("POST", &format!("/sinks/{sink}/columns"), &[])
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        m.form(
            "PATCH",
            &order,
            &[("column_order[column_ids][]", &ids[1].to_string())]
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    let mut owner = h.signed_in().await;
    owner.get(&format!("/sinks/{sink}")).await;
    for (source, id) in [
        (sink, ids[2]),
        (sink, ids[3]),
        (hidden, ids[2]),
        (foreign, ids[3]),
    ] {
        let path = format!("/sinks/{source}/columns/{id}");
        for suffix in ["", "/edit", "/events?before_id=999"] {
            assert_eq!(
                owner.get(&format!("{path}{suffix}")).await.status,
                StatusCode::NOT_FOUND
            );
        }
        assert_eq!(
            owner
                .form("POST", &format!("{path}/views"), &[])
                .await
                .status,
            StatusCode::NOT_FOUND
        );
    }
    let changes = [
        ("column_order[column_ids][]", ids[1].to_string()),
        ("column_order[column_ids][]", ids[0].to_string()),
    ];
    let fields: Vec<_> = changes.iter().map(|(k, v)| (*k, v.as_str())).collect();
    assert_eq!(
        owner.form("PATCH", &order, &fields).await.status,
        StatusCode::OK
    );
    let positions: Vec<(i64, i32)> =
        sqlx::query_as("SELECT id,position FROM columns WHERE sink_id=$1 ORDER BY position")
            .bind(sink)
            .fetch_all(&h.pool)
            .await
            .unwrap();
    assert_eq!(positions, vec![(ids[1], 0), (ids[0], 1)]);
    assert_eq!(
        owner
            .form(
                "PATCH",
                &order,
                &[
                    ("column_order[column_ids][]", &ids[0].to_string()),
                    ("column_order[column_ids][]", &ids[2].to_string())
                ]
            )
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        sqlx::query_scalar::<_, i32>("SELECT position FROM columns WHERE id=$1")
            .bind(ids[0])
            .fetch_one(&h.pool)
            .await
            .unwrap(),
        1,
        "failed reorder rolls back its earlier writes"
    );
    let mut other = h.signed_in_as(member).await;
    other.get(&format!("/sinks/{sink}")).await;
    let first = format!("{path}/views");
    let second = format!("/sinks/{sink}/columns/{}/views", ids[1]);
    let (one, two) = tokio::join!(
        m.form("POST", &first, &[]),
        other.form("POST", &second, &[])
    );
    assert_eq!(one.status, StatusCode::OK);
    assert_eq!(two.status, StatusCode::OK);
    let timestamps: Value = sqlx::query_scalar(
        "SELECT column_last_viewed_at FROM sink_memberships WHERE user_id=$1 AND sink_id=$2",
    )
    .bind(member)
    .bind(sink)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(timestamps.as_object().unwrap().len(), 2);
    for id in &ids[..2] {
        chrono::DateTime::parse_from_rfc3339(timestamps[&id.to_string()].as_str().unwrap())
            .unwrap();
    }
    let other_timestamps: Value = sqlx::query_scalar(
        "SELECT column_last_viewed_at FROM sink_memberships WHERE user_id=$1 AND sink_id=$2",
    )
    .bind(h.user)
    .bind(sink)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(other_timestamps, json!({}));
    let mut a = h.signed_in_as(admin).await;
    a.get(&format!("/sinks/{sink}")).await;
    assert_eq!(
        a.form("POST", &format!("/sinks/{sink}/columns"), &[])
            .await
            .status,
        StatusCode::FOUND
    );
    outside.finish().await;
    h.finish().await;
}

#[tokio::test]
async fn column_lazy_pages_keep_thirty_row_batches_and_original_id_cursor() {
    let h = Harness::legacy_reference().await;
    let sink = h.sink("Paging").await;
    h.grant(h.user, sink).await;
    let id:i64=sqlx::query_scalar("INSERT INTO columns (sink_id,name,created_at,updated_at) VALUES ($1,'All events',now(),now()) RETURNING id").bind(sink).fetch_one(&h.pool).await.unwrap();
    let mut ids = Vec::new();
    for index in 0..35 {
        ids.push(h.event(sink,json!({"text":format!("row {index}"),"occurred_at":format!("2026-10-18T00:00:{index:02}Z")})).await);
    }
    let mut owner = h.signed_in().await;
    owner.get(&format!("/sinks/{sink}")).await;
    let path = format!("/sinks/{sink}/columns/{id}");
    let shown = stream(&mut owner, "GET", &path, &[]).await;
    assert_eq!(
        shown.html.matches("class=\"event-preview-card\"").count(),
        30
    );
    assert!(shown.html.contains(&format!("before_id={}", ids[5])));
    assert!(shown.html.contains("loading=\"lazy\""));
    let older = owner
        .get(&format!("{path}/events?before_id={}", ids[5]))
        .await;
    assert_eq!(older.status, StatusCode::OK);
    assert!(
        older
            .html
            .contains(&format!("action=\"append\" target=\"events_list_{id}\""))
    );
    assert_eq!(
        older.html.matches("class=\"event-preview-card\"").count(),
        5
    );
    assert!(older.html.contains("events__end-of-feed"));
    let no_anchor = owner.get(&format!("{path}/events")).await;
    assert_eq!(
        no_anchor
            .html
            .matches("class=\"event-preview-card\"")
            .count(),
        0
    );
    h.finish().await;
}
