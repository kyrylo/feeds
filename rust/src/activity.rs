//! One activity feed, with legacy sources retained as stable ingestion identities.
use crate::{
    AppState,
    browser::{BrowserRequest, User, page, redirect},
    feed::{self, Event, Filters},
    feed_view, params, sinks, view,
};
use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{QueryBuilder, Sqlite};

const PAGE_SIZE: usize = 50;

#[derive(Clone, Serialize, sqlx::FromRow)]
pub(crate) struct Source {
    pub id: i64,
    pub name: String,
}

fn unavailable(_: sqlx::Error) -> StatusCode {
    StatusCode::SERVICE_UNAVAILABLE
}

fn positive(value: Option<&Value>) -> Result<Option<i64>, StatusCode> {
    let Some(value) = params::scalar(value).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    value
        .parse::<i64>()
        .ok()
        .filter(|v| *v >= 0)
        .map(Some)
        .ok_or(StatusCode::BAD_REQUEST)
}

pub(crate) async fn sources(state: &AppState, user: i64) -> Result<Vec<Source>, StatusCode> {
    sqlx::query_as("SELECT s.id,s.name FROM sinks s JOIN sink_memberships m ON m.sink_id=s.id JOIN users u ON u.id=m.user_id WHERE u.id=$1 AND s.account_id=u.account_id ORDER BY s.name COLLATE NOCASE,s.id")
        .bind(user).fetch_all(&state.pool).await.map_err(unavailable)
}

pub(crate) async fn selected_source(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
) -> Result<Option<i64>, StatusCode> {
    let source = positive(request.params.get("source"))?;
    if let Some(id) = source {
        sinks::authorized(state, user.id, id).await?;
    }
    Ok(source)
}

fn parameters(request: &BrowserRequest, source: Option<i64>) -> Vec<(String, String)> {
    let mut values = Filters::search_only(&request.params).parameters(None);
    if let Some(id) = source {
        values.push(("source".into(), id.to_string()));
    }
    values
}

fn view_parameters(request: &BrowserRequest, source: Option<i64>) -> Vec<(String, String)> {
    let mut values = parameters(request, source);
    if crate::activity_tabs::selected_id(&request.path).is_some() {
        values.push(("_filters".into(), "1".into()));
    }
    values
}

fn view_path(request: &BrowserRequest, action: &str) -> String {
    match (crate::activity_tabs::selected_id(&request.path), action) {
        (Some(id), "") => format!("/feeds/{id}"),
        (Some(id), action) => format!("/feeds/{id}/{action}"),
        (None, "") => "/".into(),
        (None, action) => format!("/activity/{action}"),
    }
}

pub(crate) fn background_path(uri: &str) -> bool {
    let path = uri.split('?').next().unwrap_or(uri);
    path == "/activity/updates"
        || path.starts_with("/activity/events/")
        || (crate::activity_tabs::selected_id(path).is_some() && path.ends_with("/updates"))
}

fn url(path: &str, values: &[(String, String)]) -> String {
    if values.is_empty() {
        path.into()
    } else {
        format!("{path}?{}", serde_urlencoded::to_string(values).unwrap())
    }
}

pub async fn route(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
) -> Option<Result<Response, StatusCode>> {
    let path = request.path.as_str();
    if path == "/feeds" || path.starts_with("/feeds/") {
        return Some(crate::activity_tabs::route(state, request, user).await);
    }
    let result = match (request.method.as_str(), path) {
        ("GET", "/") => show(state, request, user).await,
        ("GET", "/activity/updates") => updates(state, request, user).await,
        ("GET", "/activity/older") => older(state, request, user).await,
        ("POST", "/activity/seen") => mark_seen(state, request, user).await,
        ("GET", "/connect") => connect(state, request, user).await,
        ("GET", "/new" | "/sinks/new") => Ok(redirect("/connect", StatusCode::FOUND)),
        ("GET", "/sinks" | "/folders") => Ok(redirect("/", StatusCode::FOUND)),
        ("GET", _) if path.starts_with("/events/") => detail(state, request, user, false).await,
        ("GET", _) if path.starts_with("/activity/events/") => {
            detail(state, request, user, true).await
        }
        ("GET", _) if path.starts_with("/sinks/") => legacy_link(state, request, user).await,
        (_, _)
            if path.starts_with("/sinks/")
                || path == "/sinks"
                || path.starts_with("/folders/")
                || path == "/folders" =>
        {
            Err(StatusCode::NOT_FOUND)
        }
        _ => return None,
    };
    Some(result)
}

async fn legacy_link(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
) -> Result<Response, StatusCode> {
    let mut parts = request.path.trim_matches('/').split('/').skip(1);
    let id = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or(StatusCode::NOT_FOUND)?;
    sinks::authorized(state, user.id, id).await?;
    let rest: Vec<_> = parts.collect();
    let path = if rest == ["edit"] { "/connect" } else { "/" };
    Ok(redirect(
        &url(path, &parameters(request, Some(id))),
        StatusCode::FOUND,
    ))
}

async fn watermark(state: &AppState, user: i64) -> Result<i64, StatusCode> {
    // Find the last ID with one index seek per authorized source instead of
    // joining every event on every page load and background poll.
    sqlx::query_scalar("SELECT COALESCE(MAX((SELECT MAX(e.id) FROM events e WHERE e.sink_id=s.id)),0) FROM sinks s JOIN sink_memberships m ON m.sink_id=s.id JOIN users u ON u.id=m.user_id WHERE m.user_id=$1 AND s.account_id=u.account_id")
        .bind(user)
        .fetch_one(&state.pool)
        .await
        .map_err(unavailable)
}

fn seen_token(
    state: &AppState,
    user: i64,
    through: i64,
    source: Option<i64>,
    filters: &Filters,
    tab: Option<&crate::activity_tabs::Tab>,
) -> Option<String> {
    let eligible = tab.map_or_else(
        || source.is_none() && filters.can_mark_viewed(),
        |tab| tab.source_id == source && tab.filters() == *filters,
    );
    eligible.then(|| {
        state.signer.sign_message(
            "activity_seen",
            &json!({"user":user,"through":through,"tab":tab.map(|t|t.id),"scope":tab.map(|t|t.scope())}),
            Utc::now() + chrono::Duration::hours(1),
        )
    })
}

async fn global_seen(state: &AppState, user: i64, through: i64) -> Result<i64, StatusCode> {
    Ok(
        sqlx::query_scalar("SELECT last_seen_id FROM telesink_activity_visits WHERE user_id=$1")
            .bind(user)
            .fetch_optional(&state.pool)
            .await
            .map_err(unavailable)?
            .unwrap_or(through),
    )
}

pub(crate) async fn unread_count(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    source: Option<i64>,
    filters: &Filters,
    seen: i64,
    through: i64,
) -> Result<i64, StatusCode> {
    let mut query = scoped(user.id, source, filters, request, "COUNT(*)");
    query
        .push(" AND e.id>")
        .push_bind(seen)
        .push(" AND e.id<=")
        .push_bind(through);
    query
        .build_query_scalar()
        .fetch_one(&state.pool)
        .await
        .map_err(unavailable)
}

async fn counters(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    through: i64,
) -> Result<Value, StatusCode> {
    let tabs = crate::activity_tabs::list(state, user).await?;
    let seen = global_seen(state, user.id, through).await?;
    Ok(json!({
        "all":unread_count(state,request,user,None,&Filters::default(),seen,through).await?,
        "tabs":crate::activity_tabs::navigation(state,request,user,&tabs,None,through).await?
    }))
}

fn scoped(
    user: i64,
    source: Option<i64>,
    filters: &Filters,
    request: &BrowserRequest,
    selection: &str,
) -> QueryBuilder<Sqlite> {
    let mut query = feed::scope(user, source, selection);
    filters.apply(&mut query, sinks::zone(request));
    query
}

fn row(event: &Event, sources: &[Source], seen: i64) -> Value {
    let source = sources
        .iter()
        .find(|s| s.id == event.sink_id)
        .map(|s| s.name.as_str())
        .unwrap_or("Feed");
    json!({"id":event.id.to_string(),"source_id":event.sink_id.to_string(),"source":source,
        "text":event.text,"emoji":event.emoji,"event_type":feed_view::label(&event.event_type),
        "event_color":feed_view::terminal_color(&event.event_type),
        "occurred_at":event.occurred_at.and_utc().to_rfc3339(),
        "log_time":event.occurred_at.format("%b %d %H:%M:%S").to_string(),"new":event.id>seen})
}

fn chronological_rows(events: &[Event], sources: &[Source], seen: i64) -> Vec<Value> {
    let mut events: Vec<_> = events.iter().take(PAGE_SIZE).collect();
    events.sort_by_key(|event| (event.occurred_at, event.id));
    events
        .iter()
        .map(|event| row(event, sources, seen))
        .collect()
}

async fn batch(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    source: Option<i64>,
    through: i64,
    before: Option<i64>,
) -> Result<Vec<Event>, StatusCode> {
    let filters = Filters::search_only(&request.params);
    let mut query = feed::chronological_scope(user.id, source, "e.*");
    filters.apply(&mut query, sinks::zone(request));
    query.push(" AND e.id<=").push_bind(through);
    if let Some(before) = before {
        let anchor = feed::find(&state.pool, user.id, before)
            .await
            .map_err(unavailable)?
            .ok_or(StatusCode::NOT_FOUND)?;
        if source.is_some_and(|id| id != anchor.sink_id) {
            return Err(StatusCode::NOT_FOUND);
        }
        query
            .push(" AND (e.occurred_at,e.id)<(")
            .push_bind(anchor.occurred_at)
            .push(",")
            .push_bind(anchor.id)
            .push(")");
    }
    query
        .push(" ORDER BY e.occurred_at DESC,e.id DESC LIMIT ")
        .push_bind((PAGE_SIZE + 1) as i64);
    query
        .build_query_as()
        .fetch_all(&state.pool)
        .await
        .map_err(unavailable)
}

fn next_page(
    request: &BrowserRequest,
    source: Option<i64>,
    events: &[Event],
    through: i64,
    seen: i64,
) -> Option<String> {
    if events.len() <= PAGE_SIZE {
        return None;
    }
    let mut values = view_parameters(request, source);
    values.extend([
        ("before".into(), events[PAGE_SIZE - 1].id.to_string()),
        ("through".into(), through.to_string()),
        ("seen".into(), seen.to_string()),
    ]);
    Some(url(&view_path(request, "older"), &values))
}

pub(crate) async fn show(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
) -> Result<Response, StatusCode> {
    let mut initial = request.clone();
    if matches!(initial.path.as_str(), "/" | "/feeds")
        && initial
            .params
            .as_object()
            .is_some_and(|params| params.is_empty())
    {
        if let Some(first) = crate::activity_tabs::list(state, user).await?.first() {
            initial.path = format!("/feeds/{}", first.id);
        } else if let Some(first) = sources(state, user.id).await?.first() {
            initial.params["source"] = json!(first.id.to_string());
        }
    }
    let (effective, tab) = crate::activity_tabs::resolve(state, &initial, user).await?;
    let request = &effective;
    let source = selected_source(state, request, user).await?;
    let filters = Filters::search_only(&request.params);
    let tabs = crate::activity_tabs::list(state, user).await?;
    let through = watermark(state, user.id).await?;
    let global_seen = global_seen(state, user.id, through).await?;
    let seen = tab.as_ref().map(|t| t.last_seen_id).unwrap_or(global_seen);
    let products = sources(state, user.id).await?;
    let events = batch(state, request, user, source, through, None).await?;
    let sidebar_sources: Vec<_> = products
        .iter()
        .filter(|source| !tabs.iter().any(|tab| tab.represents_source(source.id)))
        .collect();
    let new_count = unread_count(state, request, user, source, &filters, seen, through).await?;
    let rows = chronological_rows(&events, &products, seen);
    let mut clear_parameters = view_parameters(request, source);
    clear_parameters.retain(|(key, _)| key != "q" && key != "_filters");
    clear_parameters.push(("_filters".into(), "1".into()));
    let feed_name = tab.as_ref().map(|t| t.name.as_str()).or_else(|| {
        products
            .iter()
            .find(|s| Some(s.id) == source)
            .map(|s| s.name.as_str())
    });
    let context = json!({"title":feed_name.map(|name|format!("{name} · Feeds")).unwrap_or_else(||"Feeds".into()),"current_user":user,"activity":true,
        "tabs":crate::activity_tabs::navigation(state,request,user,&tabs,tab.as_ref().map(|t|t.id),through).await?,
        "tab_id":tab.as_ref().map(|t|t.id.to_string()),
        "feed_name":feed_name.unwrap_or("feeds"),"search_url":view_path(request,""),
        "sources":sidebar_sources,"source":source.map(|id|id.to_string()),"filters":filters,
        "searching":filters.search_query.is_some(),
        "clear_url":url(&view_path(request,""), &clear_parameters),"events":rows,
        "new_count":new_count,"seen":seen.to_string(),"cursor":through.to_string(),"through":through.to_string(),
        "seen_token":seen_token(state,user.id,through,source,&filters,tab.as_ref()),
        "updates_url":url(&view_path(request,"updates"), &view_parameters(request,source)),
        "older_url":next_page(request,source,&events,through,seen)});
    page(state, request, "activity.html", context, StatusCode::OK)
}

fn json_response(value: Value) -> Response {
    let mut response = Json(value).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

pub(crate) async fn updates(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
) -> Result<Response, StatusCode> {
    let (effective, tab) = crate::activity_tabs::resolve(state, request, user).await?;
    let request = &effective;
    let source = selected_source(state, request, user).await?;
    let filters = Filters::search_only(&request.params);
    let after = positive(request.params.get("after"))?.ok_or(StatusCode::BAD_REQUEST)?;
    let seen = positive(request.params.get("seen"))?.unwrap_or(after);
    let through = watermark(state, user.id).await?;
    let mut query = scoped(user.id, source, &filters, request, "e.*");
    query
        .push(" AND e.id>")
        .push_bind(after)
        .push(" AND e.id<=")
        .push_bind(through)
        .push(" ORDER BY e.id LIMIT ")
        .push_bind((PAGE_SIZE + 1) as i64);
    let events: Vec<Event> = query
        .build_query_as()
        .fetch_all(&state.pool)
        .await
        .map_err(unavailable)?;
    let more = events.len() > PAGE_SIZE;
    let cursor = if more {
        events[PAGE_SIZE - 1].id
    } else {
        through
    };
    let products = sources(state, user.id).await?;
    let rows = chronological_rows(&events, &products, seen);
    let html = view::fragment("activity-rows.html", json!({"events":rows}))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(json_response(
        json!({"html":html,"cursor":cursor.to_string(),"more":more,
        "seen_token":seen_token(state,user.id,cursor,source,&filters,tab.as_ref()),
        "counts":counters(state,request,user,through).await?,
        "sources":products.iter().map(|s|json!({"id":s.id.to_string(),"name":s.name})).collect::<Vec<_>>()}),
    ))
}

pub(crate) async fn older(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
) -> Result<Response, StatusCode> {
    let (effective, tab) = crate::activity_tabs::resolve(state, request, user).await?;
    let request = &effective;
    let source = selected_source(state, request, user).await?;
    let through = positive(request.params.get("through"))?.ok_or(StatusCode::BAD_REQUEST)?;
    let before = positive(request.params.get("before"))?.ok_or(StatusCode::BAD_REQUEST)?;
    let seen = positive(request.params.get("seen"))?.unwrap_or(through);
    let events = batch(state, request, user, source, through, Some(before)).await?;
    let products = sources(state, user.id).await?;
    let rows = chronological_rows(&events, &products, seen);
    let next = next_page(request, source, &events, through, seen);
    if request
        .headers
        .get("accept")
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| h.contains("application/json"))
    {
        let html = view::fragment("activity-rows.html", json!({"events":rows}))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok(json_response(json!({"html":html,"older_url":next})))
    } else {
        page(
            state,
            request,
            "activity-history.html",
            json!({"title":format!("Earlier {} · Feeds",tab.as_ref().map(|t|t.name.as_str()).or_else(||products.iter().find(|s|Some(s.id)==source).map(|s|s.name.as_str())).unwrap_or("events")),"current_user":user,"events":rows,"older_url":next,"back_url":url(&view_path(request,""),&view_parameters(request,source))}),
            StatusCode::OK,
        )
    }
}

async fn mark_seen(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
) -> Result<Response, StatusCode> {
    let token = params::scalar(request.params.get("seen_token")).ok_or(StatusCode::BAD_REQUEST)?;
    let value = state
        .signer
        .verify_message("activity_seen", &token, Utc::now())
        .ok_or(StatusCode::UNPROCESSABLE_ENTITY)?;
    if value["user"].as_i64() != Some(user.id) {
        return Err(StatusCode::FORBIDDEN);
    }
    let through = value["through"]
        .as_i64()
        .filter(|v| *v >= 0)
        .ok_or(StatusCode::BAD_REQUEST)?;
    if let Some(id) = value["tab"].as_i64() {
        // Hold the write lock while checking the saved query: a token from an
        // earlier filter configuration must not clear a newly edited tab.
        let mut tx = state
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(unavailable)?;
        let tab = crate::activity_tabs::find_on(&mut tx, user, id).await?;
        if value["scope"] != tab.scope() {
            return Err(StatusCode::CONFLICT);
        }
        sqlx::query("UPDATE telesink_activity_tab_visits SET last_seen_id=MAX(last_seen_id,$1),updated_at=now() WHERE tab_id=$2")
            .bind(through).bind(id).execute(&mut *tx).await.map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
    } else {
        sqlx::query("INSERT INTO telesink_activity_visits(user_id,last_seen_id,updated_at) VALUES ($1,$2,now()) ON CONFLICT(user_id) DO UPDATE SET last_seen_id=MAX(last_seen_id,excluded.last_seen_id),updated_at=now()")
            .bind(user.id).bind(through).execute(&state.pool).await.map_err(unavailable)?;
    }
    Ok(json_response(
        json!({"counts":counters(state,request,user,watermark(state,user.id).await?).await?}),
    ))
}

async fn detail(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    fragment: bool,
) -> Result<Response, StatusCode> {
    let id: i64 = request
        .path
        .rsplit('/')
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or(StatusCode::NOT_FOUND)?;
    let event = feed::find(&state.pool, user.id, id)
        .await
        .map_err(unavailable)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let product = sinks::authorized(state, user.id, event.sink_id).await?;
    let properties: Vec<_> = event.properties.as_object().into_iter().flat_map(|p|p.iter()).map(|(key,value)| {
        json!({"key":key,"value":if let Value::String(s)=value {s.clone()}else{value.to_string()}})
    }).collect();
    let context = json!({"title":"Event · Feeds","current_user":user,"event":row(&event,&[Source{id:product.id,name:product.name}],i64::MAX),
        "properties":properties});
    if fragment {
        sinks::fragment("activity-detail.html", context, false)
    } else {
        page(
            state,
            request,
            "activity-event.html",
            context,
            StatusCode::OK,
        )
    }
}

async fn connect(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
) -> Result<Response, StatusCode> {
    if user.role != 2 {
        return Err(StatusCode::FORBIDDEN);
    }
    let tabs = crate::activity_tabs::list(state, user).await?;
    let mut feeds = Vec::with_capacity(tabs.len());
    for tab in tabs {
        let token: String = sqlx::query_scalar("SELECT token FROM sinks WHERE id=$1")
            .bind(tab.source_id)
            .fetch_one(&state.pool)
            .await
            .map_err(unavailable)?;
        feeds.push(json!({"id":tab.id,"name":tab.name,"endpoint":crate::activity_tabs::endpoint(state,request,&token)}));
    }
    page(
        state,
        request,
        "connect.html",
        json!({"title":"Connect · Feeds","current_user":user,"feeds":feeds}),
        StatusCode::OK,
    )
}
