//! Compatibility routes for the column records retained by the Rails schema.
use crate::{
    AppState,
    browser::{BrowserRequest, User, page, redirect},
    feed::{self, Event, Filters},
    feed_view, params, sinks,
};
use axum::{body::Body, http::StatusCode, response::Response};
use chrono::{Days, Utc};
use regex::RegexBuilder;
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

#[derive(Clone, sqlx::FromRow, Serialize)]
pub struct Column {
    pub id: i64,
    pub sink_id: i64,
    pub name: String,
    pub config: Option<Value>,
}

impl Column {
    fn filters(&self) -> Option<&Value> {
        self.config.as_ref()?.get("filters")
    }
    pub fn event_types(&self) -> Vec<String> {
        array(self.filters().and_then(|v| v.get("event_types")))
    }
    pub fn search(&self) -> Option<String> {
        params::present_scalar(self.filters().and_then(|v| v.get("search")))
            .map(|s| s.trim().to_owned())
    }
}

fn array(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(|v| params::present_scalar(Some(v)))
            .collect(),
        value => params::present_scalar(value).into_iter().collect(),
    }
}

pub async fn recent(
    pool: &SqlitePool,
    user: i64,
    column: &Column,
    before: Option<i64>,
) -> Result<Vec<Event>, sqlx::Error> {
    let mut query = feed::scope(user, Some(column.sink_id), "e.*");
    let types = column.event_types();
    if !types.is_empty() {
        query
            .push(" AND e.event_type IN (SELECT value FROM json_each(")
            .push_bind(serde_json::json!(types))
            .push("))");
    }
    if let Some(search) = column.search() {
        // The retained column query uses SQL wildcards and searches text only.
        // The main feed has different (literal, property-aware) search semantics.
        query
            .push(" AND telesink_ilike(e.text,")
            .push_bind(format!("%{search}%"))
            .push(")");
    }
    if let Some(before) = before {
        query.push(" AND e.id < ").push_bind(before);
    }
    query.push(" ORDER BY e.occurred_at DESC,e.id DESC LIMIT 30");
    query.build_query_as().fetch_all(pool).await
}

pub async fn route(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    sink: &sinks::Sink,
    rest: &[&str],
) -> Result<Response, StatusCode> {
    if rest == ["column_order"] && matches!(request.method.as_str(), "PATCH" | "PUT") {
        sinks::admin(user)?;
        let fields = request
            .params
            .get("column_order")
            .filter(|v| v.is_object())
            .ok_or(StatusCode::BAD_REQUEST)?;
        let ids = array(fields.get("column_ids"));
        let mut tx = state
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        for (position, id) in ids.into_iter().enumerate() {
            let id = sinks::parse_id(&id)?;
            let result = sqlx::query("UPDATE columns SET position=$1 WHERE id=$2 AND sink_id=$3")
                .bind(position as i32)
                .bind(id)
                .bind(sink.id)
                .execute(&mut *tx)
                .await
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            if result.rows_affected() == 0 {
                return Err(StatusCode::NOT_FOUND);
            }
        }
        tx.commit()
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        return Ok(Response::new(Body::empty()));
    }
    if rest == ["columns"] && request.method == "POST" {
        sinks::admin(user)?;
        let column: Column = sqlx::query_as("INSERT INTO columns (sink_id,name,has_events,created_at,updated_at) VALUES ($1,'all events',EXISTS(SELECT 1 FROM events WHERE sink_id=$1),now(),now()) RETURNING id,sink_id,name,config")
            .bind(sink.id).fetch_one(&state.pool).await.map_err(|_|StatusCode::SERVICE_UNAVAILABLE)?;
        return show(state, request, user, sink, &column, Some("append")).await;
    }
    let ["columns", id, tail @ ..] = rest else {
        return Err(StatusCode::NOT_FOUND);
    };
    if matches!(request.method.as_str(), "PATCH" | "PUT" | "DELETE") || tail == ["edit"] {
        sinks::admin(user)?;
    }
    let mut column: Column =
        sqlx::query_as("SELECT id,sink_id,name,config FROM columns WHERE id=$1 AND sink_id=$2")
            .bind(sinks::parse_id(id)?)
            .bind(sink.id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
            .ok_or(StatusCode::NOT_FOUND)?;
    match (request.method.as_str(), tail) {
        ("GET", []) => show(state, request, user, sink, &column, None).await,
        ("GET", ["edit"]) => edit(state, request, user, sink, &column, false).await,
        ("PATCH" | "PUT", []) => {
            let fields = request
                .params
                .get("column")
                .filter(|v| v.is_object())
                .ok_or(StatusCode::BAD_REQUEST)?;
            if let Some(name) = params::scalar(fields.get("name")) {
                column.name = name;
            }
            let mut config = column
                .config
                .take()
                .filter(Value::is_object)
                .unwrap_or_else(|| json!({}));
            let mut filters = json!({});
            let types = array(fields.get("event_types"));
            if !types.is_empty() {
                filters["event_types"] = json!(types);
            }
            if let Some(search) = params::present_scalar(fields.get("search")) {
                filters["search"] = json!(search.trim());
            }
            config["filters"] = filters;
            column.config = Some(config);
            if column.name.trim().is_empty() {
                return edit(state, request, user, sink, &column, true).await;
            }
            sqlx::query(
                "UPDATE columns SET name=$1,config=$2,updated_at=now() WHERE id=$3 AND sink_id=$4",
            )
            .bind(&column.name)
            .bind(&column.config)
            .bind(column.id)
            .bind(sink.id)
            .execute(&state.pool)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            show(state, request, user, sink, &column, Some("replace")).await
        }
        ("DELETE", []) => {
            sqlx::query("DELETE FROM columns WHERE id=$1 AND sink_id=$2")
                .bind(column.id)
                .bind(sink.id)
                .execute(&state.pool)
                .await
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            Ok(redirect(
                &format!("/sinks/{}", sink.id),
                StatusCode::SEE_OTHER,
            ))
        }
        ("GET", ["events"]) => {
            let before = params::scalar(request.params.get("before_id"))
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let events = recent(&state.pool, user.id, &column, Some(before))
                .await
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            let mut context = context(state, request, user, sink, &column, &events).await?;
            context["stream_action"] = json!("older");
            sinks::fragment("column-stream.html", context, true)
        }
        ("POST", ["views"]) => {
            // A single atomic JSON update avoids losing another column's concurrent view.
            sqlx::query("UPDATE sink_memberships SET column_last_viewed_at=telesink_json_set(column_last_viewed_at,$1,$2),updated_at=now() WHERE user_id=$3 AND sink_id=$4")
                .bind(column.id.to_string()).bind(Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs,true)).bind(user.id).bind(sink.id)
                .execute(&state.pool).await.map_err(|_|StatusCode::SERVICE_UNAVAILABLE)?;
            Ok(Response::new(Body::empty()))
        }
        _ => Err(StatusCode::NOT_FOUND),
    }
}

async fn show(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    sink: &sinks::Sink,
    column: &Column,
    action: Option<&str>,
) -> Result<Response, StatusCode> {
    if !sinks::wants_stream(request)
        && (action.is_some() || !request.headers.contains_key("turbo-frame"))
    {
        return Ok(redirect(
            &format!("/sinks/{}", sink.id),
            if action.is_some() {
                StatusCode::FOUND
            } else {
                StatusCode::SEE_OTHER
            },
        ));
    }
    let events = recent(&state.pool, user.id, column, None)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let mut context = context(state, request, user, sink, column, &events).await?;
    if sinks::wants_stream(request) {
        context["stream_action"] = json!(action.unwrap_or("replace"));
        sinks::fragment("column-stream.html", context, true)
    } else {
        sinks::fragment("column.html", context, false)
    }
}

fn text_parts(text: &str, search: Option<&str>) -> Vec<Value> {
    let Some(pattern) = search.and_then(|s| {
        RegexBuilder::new(&regex::escape(s))
            .case_insensitive(true)
            .build()
            .ok()
    }) else {
        return vec![json!({"text":text})];
    };
    let mut parts = Vec::new();
    let mut previous = 0;
    for found in pattern.find_iter(text) {
        parts.push(json!({"text":&text[previous..found.start()]}));
        parts.push(json!({"text":found.as_str(),"highlight":true}));
        previous = found.end();
    }
    parts.push(json!({"text":&text[previous..]}));
    parts
}

async fn context(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    sink: &sinks::Sink,
    column: &Column,
    events: &[Event],
) -> Result<Value, StatusCode> {
    let mut context = sinks::base_context(state, request, user, &column.name).await?;
    let single = column.event_types().len() == 1;
    let search = column.search();
    let zone = sinks::zone(request);
    let today = Utc::now().with_timezone(&zone).date_naive();
    let mut previous = None;
    let mut rows = Vec::new();
    for event in events {
        let mut row = feed_view::row(event, &Filters::default(), None);
        let local = event.occurred_at.and_utc().with_timezone(&zone);
        let date = local.date_naive();
        if previous != Some(date) {
            row["day_label"] = json!(if date == today {
                "Today".into()
            } else if Some(date) == today.checked_sub_days(Days::new(1)) {
                "Yesterday".into()
            } else {
                local.format("%A, %B %d, %Y").to_string()
            });
        }
        previous = Some(date);
        row["text_parts"] = json!(text_parts(
            &feed_view::truncate(&event.text, if single { 101 } else { 87 }, true),
            search.as_deref()
        ));
        rows.push(row);
    }
    context["column"] = json!(column);
    context["column_search"] = json!(search);
    context["single_type"] = json!(single);
    context["events"] = json!(rows);
    context["more_url"] = json!(if events.len() == 30 {
        events.last().map(|e| {
            format!(
                "/sinks/{}/columns/{}/events?before_id={}",
                sink.id, column.id, e.id
            )
        })
    } else {
        None
    });
    Ok(context)
}

async fn edit(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    sink: &sinks::Sink,
    column: &Column,
    invalid: bool,
) -> Result<Response, StatusCode> {
    let mut context = context(state, request, user, sink, column, &[]).await?;
    let types: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT event_type FROM events WHERE sink_id=$1 ORDER BY event_type",
    )
    .bind(sink.id)
    .fetch_all(&state.pool)
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let selected = column.event_types();
    context["available_types"]=json!(types.into_iter().map(|name|json!({"label":feed_view::label(&name),"selected":selected.contains(&name),"value":name})).collect::<Vec<_>>());
    page(
        state,
        request,
        "column-edit.html",
        context,
        if invalid {
            StatusCode::UNPROCESSABLE_ENTITY
        } else {
            StatusCode::OK
        },
    )
}
