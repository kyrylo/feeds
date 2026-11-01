use crate::{
    AppState,
    browser::{BrowserRequest, User, page, redirect},
    feed_view, sinks,
};
use axum::{http::StatusCode, response::Response};
use serde_json::{Value, json};

pub async fn list(state: &AppState, sink: i64) -> Result<Value, StatusCode> {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT event_type FROM events WHERE sink_id=$1 ORDER BY event_type",
    )
    .bind(sink)
    .fetch_all(&state.pool)
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(json!(
        names
            .into_iter()
            .map(|name| json!({"label":feed_view::label(&name),"url":url(sink,&name)}))
            .collect::<Vec<_>>()
    ))
}

fn url(sink: i64, name: &str) -> String {
    format!(
        "/sinks/{sink}/event_types/{}",
        percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC)
    )
}

pub async fn route(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    sink: &sinks::Sink,
    name: Option<&str>,
) -> Result<Response, StatusCode> {
    sinks::admin(user)?;
    let name = name
        .map(|name| {
            percent_encoding::percent_decode_str(name)
                .decode_utf8()
                .map(|s| s.into_owned())
        })
        .transpose()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if request.method == "DELETE" {
        let name = name.ok_or(StatusCode::NOT_FOUND)?;
        sqlx::query("DELETE FROM events WHERE sink_id=$1 AND event_type=$2")
            .bind(sink.id)
            .bind(name)
            .execute(&state.pool)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if !sinks::wants_stream(request) {
            return Ok(redirect(
                &format!("/sinks/{}/edit", sink.id),
                StatusCode::FOUND,
            ));
        }
        return sinks::fragment(
            "event-types-stream.html",
            json!({"event_types":list(state,sink.id).await?}),
            true,
        );
    }
    if request.method != "GET" {
        return Err(StatusCode::NOT_FOUND);
    }
    let mut context = sinks::base_context(state, request, user, "event types in this sink").await?;
    context["sink"] = json!(sink);
    if let Some(name) = name {
        context["event_type_label"] = json!(feed_view::label(&name));
        context["event_type_url"] = json!(url(sink.id, &name));
        // A full render also issues the CSRF cookie on a direct visit. Turbo
        // extracts event_types from the page when this is a frame navigation.
        page(state, request, "event-type.html", context, StatusCode::OK)
    } else {
        context["event_types"] = list(state, sink.id).await?;
        sinks::fragment("event-types.html", context, false)
    }
}
