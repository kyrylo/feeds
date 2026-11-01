// Retained Rails workflow reference; production Activity routes are tested in activity.rs.
mod common;
use axum::http::StatusCode;
use common::Harness;
use serde_json::json;

#[tokio::test]
async fn folders_preserve_sources_and_enforce_account_and_role_boundaries() {
    let h = Harness::legacy_reference().await;
    let outside = Harness::legacy_reference().await;
    let mut owner = h.signed_in().await;
    assert_eq!(owner.get("/new").await.location(), "/connect");
    assert_eq!(owner.get("/folders/new").await.status, StatusCode::OK);
    assert_eq!(
        owner
            .form("POST", "/folders", &[("folder[name]", " ")])
            .await
            .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        owner
            .form("POST", "/folders", &[("folder[name]", "Production")])
            .await
            .location(),
        "/sinks"
    );
    let folder: i64 = sqlx::query_scalar("SELECT id FROM folders WHERE account_id=$1")
        .bind(h.account)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    let path = format!("/folders/{folder}");
    assert_eq!(owner.get(&path).await.location(), "/sinks");
    // The new-source form must render even though its folder is still unset.
    assert_eq!(owner.get("/sinks/new").await.status, StatusCode::OK);
    let created = owner
        .form(
            "POST",
            "/sinks",
            &[
                ("sink[name]", "Grouped"),
                ("sink[folder_id]", &folder.to_string()),
            ],
        )
        .await;
    let sink_path = created.location().to_owned();
    let sink: i64 = sink_path.rsplit('/').next().unwrap().parse().unwrap();
    let event = h.event(sink, json!({"text":"preserved"})).await;
    let token: String = sqlx::query_scalar("SELECT token FROM sinks WHERE id=$1")
        .bind(sink)
        .fetch_one(&h.pool)
        .await
        .unwrap();
    owner
        .form("PATCH", &sink_path, &[("sink[name]", "Renamed")])
        .await;
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>("SELECT folder_id FROM sinks WHERE id=$1")
            .bind(sink)
            .fetch_one(&h.pool)
            .await
            .unwrap(),
        Some(folder)
    );
    assert_eq!(owner.get(&path).await.location(), sink_path);
    let member = h.member("member", 0).await;
    let mut m = h.signed_in_as(member).await;
    assert_eq!(
        m.get("/settings/nickname/edit").await.status,
        StatusCode::OK
    );
    assert_eq!(m.get(&path).await.location(), "/sinks");
    h.grant(member, sink).await;
    assert_eq!(m.get(&path).await.location(), sink_path);
    assert_eq!(m.get("/folders/new").await.status, StatusCode::FORBIDDEN);
    assert_eq!(
        m.get(&format!("{path}/edit")).await.status,
        StatusCode::FORBIDDEN
    );
    for method in ["PATCH", "DELETE"] {
        assert_eq!(
            m.form(method, &path, &[("folder[name]", "forbidden")])
                .await
                .status,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        m.form("POST", "/folders", &[("folder[name]", "forbidden")])
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let mut foreign = outside.signed_in().await;
    foreign.get("/settings/nickname/edit").await;
    for route in [path.clone(), format!("{path}/edit")] {
        assert_eq!(foreign.get(&route).await.status, StatusCode::NOT_FOUND);
    }
    for method in ["PATCH", "DELETE"] {
        assert_eq!(
            foreign
                .form(method, &path, &[("folder[name]", "forbidden")])
                .await
                .status,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        owner.get(&format!("{path}/edit")).await.status,
        StatusCode::OK
    );
    assert_eq!(
        owner
            .form("PATCH", &path, &[("folder[name]", "Renamed folder")])
            .await
            .location(),
        "/sinks"
    );
    assert!(owner.get(&sink_path).await.html.contains("Renamed folder"));
    assert_eq!(owner.form("DELETE", &path, &[]).await.location(), "/sinks");
    assert_eq!(
        sqlx::query_scalar::<_, Option<i64>>("SELECT folder_id FROM sinks WHERE id=$1")
            .bind(sink)
            .fetch_one(&h.pool)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT token FROM sinks WHERE id=$1")
            .bind(sink)
            .fetch_one(&h.pool)
            .await
            .unwrap(),
        token
    );
    assert_eq!(
        owner.get(&format!("/events/{event}")).await.status,
        StatusCode::OK
    );
    assert_eq!(owner.get(&path).await.status, StatusCode::NOT_FOUND);
    outside.finish().await;
    h.finish().await;
}

#[tokio::test]
async fn event_type_deletion_is_exact_scoped_and_available_through_turbo() {
    let h = Harness::legacy_reference().await;
    let sink = h.sink("Types").await;
    let hidden = h.sink("Hidden types").await;
    let other = h.member("member", 0).await;
    h.grant(h.user, sink).await;
    h.grant(other, sink).await;
    let kind = "payment/failed?tag=#<unsafe>";
    let encoded =
        percent_encoding::utf8_percent_encode(kind, percent_encoding::NON_ALPHANUMERIC).to_string();
    let same = h
        .event(sink, json!({"text":"remove me","event":kind}))
        .await;
    let kept = h
        .event(sink, json!({"text":"keep me","event":"other"}))
        .await;
    let hidden_event = h.event(hidden, json!({"text":"hidden","event":kind})).await;
    let path = format!("/sinks/{sink}/event_types/{encoded}");
    let mut owner = h.signed_in().await;
    let edit = owner.get(&format!("/sinks/{sink}/edit")).await;
    assert_eq!(edit.status, StatusCode::OK);
    assert!(edit.html.contains("id=\"event_types\""));
    assert!(!edit.html.contains("<unsafe>"));
    assert!(edit.html.replace("&#x2f;", "/").contains(&path));
    assert!(
        owner
            .get(&format!("/sinks/{sink}/event_types"))
            .await
            .html
            .starts_with("<turbo-frame id=\"event_types\"")
    );
    assert!(
        owner
            .get(&path)
            .await
            .html
            .contains("delete this event type")
    );
    let mut member = h.signed_in_as(other).await;
    member.get("/settings/nickname/edit").await;
    for route in [path.clone(), format!("/sinks/{sink}/event_types")] {
        assert_eq!(member.get(&route).await.status, StatusCode::FORBIDDEN);
    }
    assert_eq!(
        member.form("DELETE", &path, &[]).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        owner
            .form(
                "DELETE",
                &format!("/sinks/{hidden}/event_types/{encoded}"),
                &[]
            )
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let csrf = owner.cookies["telesink_csrf"].clone();
    let body = serde_urlencoded::to_string([("authenticity_token", csrf.as_str())]).unwrap();
    let deleted = owner
        .send(
            "DELETE",
            &path,
            body,
            &[("accept", "text/vnd.turbo-stream.html")],
        )
        .await;
    assert_eq!(deleted.status, StatusCode::OK);
    assert!(
        deleted
            .html
            .contains("action=\"replace\" target=\"event_types\"")
    );
    assert!(!deleted.html.contains(&encoded));
    assert_eq!(
        owner.get(&format!("/events/{same}")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        owner.get(&format!("/events/{kept}")).await.status,
        StatusCode::OK
    );
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM events WHERE id=$1)")
            .bind(hidden_event)
            .fetch_one(&h.pool)
            .await
            .unwrap()
    );
    assert_eq!(
        owner
            .form("DELETE", &format!("/sinks/{sink}/event_types/other"), &[])
            .await
            .location(),
        format!("/sinks/{sink}/edit")
    );
    h.finish().await;
}
