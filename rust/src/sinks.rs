use crate::{
    AppState,
    browser::{BrowserRequest, User, page, redirect},
    event_types,
    feed::{self, Filters},
    feed_view, params, saved_views, view,
};
use axum::{
    body::Body,
    http::{Method, StatusCode},
    response::Response,
};
use chrono::{Datelike, NaiveDate, NaiveDateTime, Utc};
use rand::Rng;
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Clone, Serialize, sqlx::FromRow)]
pub struct Sink {
    pub id: i64,
    pub account_id: i64,
    pub name: String,
    pub token: String,
    pub folder_id: Option<i64>,
    pub folder_name: Option<String>,
    pub unread_count: i32,
    pub has_unread_events: bool,
    pub last_viewed_at: Option<NaiveDateTime>,
}
const SELECT_SINK: &str = "SELECT s.id,s.account_id,s.name,COALESCE(s.token,'') AS token,s.folder_id,f.name AS folder_name,m.unread_count,m.has_unread_events,m.last_viewed_at FROM sinks s JOIN sink_memberships m ON m.sink_id=s.id JOIN users u ON u.id=m.user_id LEFT JOIN folders f ON f.id=s.folder_id AND f.account_id=s.account_id WHERE m.user_id=$1 AND s.account_id=u.account_id";

pub async fn authorized(state: &AppState, user: i64, id: i64) -> Result<Sink, StatusCode> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT_SINK} AND s.id=$2")))
        .bind(user)
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::NOT_FOUND)
}

pub async fn navigation(state: &AppState, user: &User) -> Result<Value, StatusCode> {
    let mut sinks: Vec<Sink> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT_SINK} ORDER BY s.name ASC"
    )))
    .bind(user.id)
    .fetch_all(&state.pool)
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    sinks.sort_by_key(|s| {
        (
            s.folder_id.is_none(),
            s.folder_name.as_deref().unwrap_or_default().to_lowercase(),
            s.folder_id,
            s.name.to_lowercase(),
        )
    });
    let mut rows = Vec::new();
    let mut previous = None;
    for sink in sinks {
        if sink.folder_id != previous && sink.folder_id.is_some() {
            rows.push(json!({"folder":true,"id":sink.folder_id,"name":sink.folder_name}));
        }
        previous = sink.folder_id;
        rows.push(json!({"id":sink.id,"name":sink.name,"in_folder":sink.folder_id.is_some(),"current":user.current_sink_id==Some(sink.id),
            "unread":if sink.unread_count==0 && sink.has_unread_events {1}else{sink.unread_count}}));
    }
    Ok(json!(rows))
}

pub async fn base_context(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    title: &str,
) -> Result<Value, StatusCode> {
    Ok(
        json!({"title":title,"current_user":user,"sidebar":"sinks-sidebar.html","navigation":navigation(state,user).await?,
        "can_administer":matches!(user.role,1|2),"csrf":request.cookie("telesink_csrf"),"sidebar_stream":crate::live::Grant::Sidebar{user:user.id}.token(state)}),
    )
}

pub fn fragment(name: &str, context: Value, stream: bool) -> Result<Response, StatusCode> {
    let html = view::fragment(name, context).map_err(|error| {
        eprintln!("template {name}: {error}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Response::builder()
        .header(
            "content-type",
            if stream {
                "text/vnd.turbo-stream.html; charset=utf-8"
            } else {
                "text/html; charset=utf-8"
            },
        )
        .header("cache-control", "no-store")
        .body(Body::from(html))
        .unwrap())
}

pub fn zone(request: &BrowserRequest) -> chrono_tz::Tz {
    let name = request
        .cookie("telesink_time_zone")
        .and_then(|s| percent_encoding::percent_decode_str(s).decode_utf8().ok());
    feed::time_zone(name.as_deref())
}

pub async fn route(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
) -> Option<Result<Response, StatusCode>> {
    let parts: Vec<_> = request.path.trim_matches('/').split('/').collect();
    match parts.as_slice() {
        [""] | ["sinks"] | ["sinks", ..] | ["events", _] => {
            Some(dispatch(state, request, user, &parts).await)
        }
        _ => None,
    }
}

async fn dispatch(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    parts: &[&str],
) -> Result<Response, StatusCode> {
    match (request.method.as_str(), parts) {
        ("GET", [""] | ["sinks"]) => index(state, request, user).await,
        ("GET", ["sinks", "new"]) => {
            admin(user)?;
            form(state, request, user, None, None).await
        }
        ("POST", ["sinks"]) => {
            admin(user)?;
            save(state, request, user, None).await
        }
        ("GET", ["events", id]) => detail(state, request, user, parse_id(id)?).await,
        (_, ["sinks", id, rest @ ..]) => {
            if matches!(request.method, Method::DELETE | Method::PATCH | Method::PUT)
                && rest.is_empty()
                || rest == ["edit"]
            {
                admin(user)?;
            }
            let sink = authorized(state, user.id, parse_id(id)?).await?;
            if matches!(rest.first(), Some(&"columns" | &"column_order")) {
                return crate::columns::route(state, request, user, &sink, rest).await;
            }
            match (request.method.as_str(), rest) {
                ("GET", []) => show(state, request, user, &sink).await,
                ("GET", ["edit"]) => {
                    set_current(state, user.id, sink.id).await?;
                    let mut user = user.clone();
                    user.current_sink_id = Some(sink.id);
                    form(state, request, &user, Some(&sink), None).await
                }
                ("PUT" | "PATCH", []) => save(state, request, user, Some(&sink)).await,
                ("DELETE", []) => destroy(state, user, &sink).await,
                ("GET", ["events"]) => older(state, request, user, &sink).await,
                ("GET", ["event_types"]) => {
                    event_types::route(state, request, user, &sink, None).await
                }
                ("GET" | "DELETE", ["event_types", name]) => {
                    event_types::route(state, request, user, &sink, Some(name)).await
                }
                ("POST", ["view"]) => {
                    if Filters::from_params(&request.params).can_mark_viewed() {
                        mark_viewed(state, user.id, sink.id).await?;
                    }
                    Ok(Response::new(Body::empty()))
                }
                ("POST", ["saved_views"]) => {
                    saved_views::create(&state.pool, user.id, sink.id, &request.params).await
                }
                ("DELETE", ["saved_views", id]) => {
                    saved_views::destroy(
                        &state.pool,
                        user.id,
                        sink.id,
                        parse_id(id)?,
                        &request.params,
                    )
                    .await
                }
                _ => Err(StatusCode::NOT_FOUND),
            }
        }
        _ => Err(StatusCode::NOT_FOUND),
    }
}

pub fn parse_id(id: &str) -> Result<i64, StatusCode> {
    id.parse::<i64>().map_err(|_| StatusCode::NOT_FOUND)
}
pub fn admin(user: &User) -> Result<(), StatusCode> {
    if matches!(user.role, 1 | 2) {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}
pub fn wants_stream(request: &BrowserRequest) -> bool {
    request
        .headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/vnd.turbo-stream.html"))
}
fn sidebar_request(request: &BrowserRequest) -> bool {
    request
        .headers
        .get("turbo-frame")
        .is_some_and(|h| h == "sinks")
}

async fn index(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
) -> Result<Response, StatusCode> {
    if sidebar_request(request) {
        return fragment(
            "sidebar-stream.html",
            base_context(state, request, user, "sinks").await?,
            true,
        );
    }
    let rows:Vec<i64>=sqlx::query_scalar("SELECT s.id FROM sinks s JOIN sink_memberships m ON m.sink_id=s.id WHERE m.user_id=$1 AND s.account_id=$2 ORDER BY s.name ASC")
        .bind(user.id).bind(user.account_id).fetch_all(&state.pool).await.map_err(|_|StatusCode::SERVICE_UNAVAILABLE)?;
    if let Some(id) = user
        .current_sink_id
        .filter(|id| rows.contains(id))
        .or_else(|| rows.first().copied())
    {
        return Ok(redirect(&format!("/sinks/{id}"), StatusCode::SEE_OTHER));
    }
    page(
        state,
        request,
        "welcome.html",
        base_context(state, request, user, "welcome to telesink").await?,
        StatusCode::OK,
    )
}

async fn set_current(state: &AppState, user: i64, sink: i64) -> Result<(), StatusCode> {
    sqlx::query("UPDATE users SET current_sink_id=$1,current_folder_id=NULL WHERE id=$2")
        .bind(sink)
        .bind(user)
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(())
}

pub async fn mark_viewed(state: &AppState, user: i64, sink: i64) -> Result<(), StatusCode> {
    let mut tx = state
        .pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let changed:bool=sqlx::query_scalar("SELECT unread_count>0 OR has_unread_events FROM sink_memberships WHERE user_id=$1 AND sink_id=$2")
        .bind(user).bind(sink).fetch_one(&mut *tx).await.map_err(|_|StatusCode::SERVICE_UNAVAILABLE)?;
    sqlx::query("UPDATE sink_memberships SET unread_count=0,has_unread_events=false,last_viewed_at=now(),updated_at=now() WHERE user_id=$1 AND sink_id=$2")
        .bind(user).bind(sink).execute(&mut *tx).await.map_err(|_|StatusCode::SERVICE_UNAVAILABLE)?;
    if changed {
        sqlx::query("INSERT INTO telesink_changes(payload) VALUES ($1)")
            .bind(format!("u:{user}"))
            .execute(&mut *tx)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    }
    tx.commit()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(())
}

async fn show(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    sink: &Sink,
) -> Result<Response, StatusCode> {
    set_current(state, user.id, sink.id).await?;
    let mut user = user.clone();
    user.current_sink_id = Some(sink.id);
    if sidebar_request(request) {
        return fragment(
            "sidebar-stream.html",
            base_context(state, request, &user, &sink.name).await?,
            true,
        );
    }
    let filters = Filters::from_params(&request.params);
    if filters.can_mark_viewed() {
        mark_viewed(state, user.id, sink.id).await?;
    }
    let context = feed_context(state, request, &user, sink, &filters).await?;
    page(state, request, "feed.html", context, StatusCode::OK)
}

fn requested_month(request: &BrowserRequest) -> Option<NaiveDate> {
    feed::date(request.params.get("month")).and_then(|d| d.with_day(1))
}

async fn feed_context(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    sink: &Sink,
    filters: &Filters,
) -> Result<Value, StatusCode> {
    let zone = zone(request);
    let cursor: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id),0) FROM events WHERE sink_id=$1")
        .bind(sink.id)
        .fetch_one(&state.pool)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let requested = requested_month(request);
    let path = format!("/sinks/{}", sink.id);
    let events = feed::batch(&state.pool, user.id, Some(sink.id), filters, zone, None, 60)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let latest = feed::newest(&state.pool, user.id, sink.id, filters, zone)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let now = Utc::now();
    let today = now.with_timezone(&zone).date_naive();
    let month = requested
        .or(filters.event_date)
        .or_else(|| latest.map(|d| d.and_utc().with_timezone(&zone).date_naive()))
        .unwrap_or(today)
        .with_day(1)
        .unwrap();
    let dates = feed::calendar(&state.pool, user.id, sink.id, filters, zone, month)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let type_filters = type_filters(state, user.id, sink.id, filters, zone, requested).await?;
    let saved = saved_views::list(&state.pool, user.id, sink.id, filters, requested)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let current_saved = saved.iter().find(|v| v["current"] == true).cloned();
    let title = sink
        .folder_name
        .as_ref()
        .map(|folder| format!("{folder} / {}", sink.name))
        .unwrap_or_else(|| sink.name.clone());
    let mut context = base_context(state, request, user, &title).await?;
    context["sink"] = json!(sink);
    context["feed_stream"] = json!(
        crate::live::Grant::Feed {
            user: user.id,
            sink: sink.id,
            filters: filters.clone(),
            zone: zone.name().into(),
            month: requested,
            after: cursor,
            seen: events.iter().map(|e| e.id).collect()
        }
        .token(state)
    );
    context["filters"] = json!(filters);
    context["active_filter"] = json!(!filters.can_mark_viewed());
    context["can_mark_viewed"] = json!(filters.can_mark_viewed());
    context["calendar"] = feed_view::calendar(month, &dates, filters, &path, requested);
    context["seen_cutoff"] = json!(sink.last_viewed_at.map(|t| t.and_utc().to_rfc3339()));
    context["events"] = json!(
        events
            .iter()
            .map(|event| feed_view::row(event, filters, requested))
            .collect::<Vec<_>>()
    );
    context["older_url"] = json!(older_url(sink.id, &events, filters, requested));
    context["initial"] = json!(true);
    context["empty_label"] = json!(if filters.can_mark_viewed() {
        "no events yet".into()
    } else {
        format!("no events matching {}", feed_view::summary(filters))
    });
    context["type_filters"] = json!(type_filters);
    context["saved_views"] = json!(saved);
    context["current_saved_view"] = json!(current_saved);
    context["suggested_name"] = json!(feed_view::summary(filters));
    context["month_parameter"] = json!(requested.map(|d| d.to_string()));
    let mut search = filters.clone();
    search.search_query = None;
    context["search_fields"] = json!(search.parameters(requested));
    context["status_links"] = json!(feed_view::status_links(filters, &path, requested, today));
    context["permalink"] = json!(request.base_url() + &filters.url(&path, requested));
    context["now"] = json!(now.to_rfc3339());
    context["local_clock"] = json!(now.with_timezone(&zone).format("%H:%M:%S").to_string());
    context["utc_clock"] = json!(now.format("%H:%M:%S").to_string());
    Ok(context)
}

pub async fn type_filters(
    state: &AppState,
    user: i64,
    sink: i64,
    filters: &Filters,
    zone: chrono_tz::Tz,
    month: Option<NaiveDate>,
) -> Result<Value, StatusCode> {
    let path = format!("/sinks/{sink}");
    let counts = feed::type_counts(&state.pool, user, sink, filters, zone)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let types: Vec<String> = counts.iter().map(|(name, _)| name.clone()).collect();
    let icons:Vec<(String,String)>=sqlx::query_as("SELECT event_type,emoji FROM (SELECT event_type,emoji,ROW_NUMBER() OVER (PARTITION BY event_type ORDER BY id DESC) AS rank FROM events WHERE sink_id=$1 AND event_type IN (SELECT value FROM json_each($2)) AND emoji<>'') WHERE rank=1 ORDER BY event_type")
        .bind(sink).bind(serde_json::json!(types)).fetch_all(&state.pool).await.map_err(|_|StatusCode::SERVICE_UNAVAILABLE)?;
    let mut no_type = filters.clone();
    no_type.event_type = None;
    let mut type_filters = vec![
        json!({"all":true,"label":"all","count":counts.iter().map(|(_,c)|c).sum::<i64>(),"key":"all","current":filters.event_type.is_none(),"url":no_type.url(&path,month)}),
    ];
    for (name, count) in counts {
        let mut next = filters.clone();
        next.event_type = Some(name.clone());
        let icon = icons
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.as_str())
            .unwrap_or("•");
        type_filters.push(json!({"label":feed_view::label(&name),"count":count,"key":feed_view::dom_key(&name),"current":filters.event_type.as_ref()==Some(&name),"url":next.url(&path,month),"icon":icon,"color":feed_view::color(&name)}));
    }
    Ok(json!(type_filters))
}

fn older_url(
    sink: i64,
    events: &[feed::Event],
    filters: &Filters,
    month: Option<NaiveDate>,
) -> Option<String> {
    if events.len() != 60 {
        return None;
    }
    let mut params = filters.parameters(month);
    params.push(("before_id".into(), events.first()?.id.to_string()));
    Some(format!(
        "/sinks/{sink}/events?{}",
        serde_urlencoded::to_string(params).unwrap()
    ))
}

async fn older(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    sink: &Sink,
) -> Result<Response, StatusCode> {
    let filters = Filters::from_params(&request.params);
    let month = requested_month(request);
    let before = params::present_scalar(request.params.get("before_id"))
        .map(|s| s.parse::<i64>().unwrap_or(0));
    let events = feed::batch(
        &state.pool,
        user.id,
        Some(sink.id),
        &filters,
        zone(request),
        before,
        60,
    )
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    fragment(
        "older-stream.html",
        json!({"events":events.iter().map(|e|feed_view::row(e,&filters,month)).collect::<Vec<_>>(),"older_url":older_url(sink.id,&events,&filters,month),"initial":false}),
        true,
    )
}

async fn detail(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    id: i64,
) -> Result<Response, StatusCode> {
    let event = feed::find(&state.pool, user.id, id)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let sink = authorized(state, user.id, event.sink_id).await?;
    let filters = Filters::from_params(&request.params);
    let mut context = base_context(state, request, user, &format!("#{id}")).await?;
    context["sink"] = json!(sink);
    context["event"] =
        feed_view::details(&event, &filters, requested_month(request), zone(request));
    context["detail_frame"] = json!(request.headers.contains_key("turbo-frame"));
    if context["detail_frame"] == true {
        fragment("event.html", context, false)
    } else {
        page(state, request, "event.html", context, StatusCode::OK)
    }
}

async fn form(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    sink: Option<&Sink>,
    error: Option<&str>,
) -> Result<Response, StatusCode> {
    let title = sink
        .map(|s| format!("edit \"{}\"", s.name))
        .unwrap_or_else(|| "new sink".into());
    let mut context = base_context(state, request, user, &title).await?;
    let folders: Vec<(i64, String)> =
        sqlx::query_as("SELECT id,name FROM folders WHERE account_id=$1 ORDER BY name")
            .bind(user.account_id)
            .fetch_all(&state.pool)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    context["folders"] = json!(folders);
    context["form_folder_id"] = json!(
        request
            .params
            .get("sink")
            .and_then(|v| v.get("folder_id"))
            .and_then(|v| params::scalar(Some(v)))
            .and_then(|s| s.parse::<i64>().ok())
            .or_else(|| sink.and_then(|s| s.folder_id))
    );
    context["sink"] = json!(sink);
    if let Some(sink) = sink {
        context["event_types"] = event_types::list(state, sink.id).await?;
    }
    context["error"] = json!(error);
    context["sidebar_mode"] = json!(if sink.is_some() { "edit" } else { "create" });
    context["form_name"] = json!(
        request
            .params
            .get("sink")
            .and_then(|v| v.get("name"))
            .cloned()
            .or_else(|| sink.map(|s| json!(s.name)))
    );
    context["endpoint"] =
        json!(sink.map(|s| format!("{}/api/v1/sinks/{}/events", request.base_url(), s.token)));
    context["action"] = json!(
        sink.map(|s| format!("/sinks/{}", s.id))
            .unwrap_or_else(|| "/sinks".into())
    );
    page(
        state,
        request,
        "sink-form.html",
        context,
        if error.is_some() {
            StatusCode::UNPROCESSABLE_ENTITY
        } else {
            StatusCode::OK
        },
    )
}

async fn save(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    existing: Option<&Sink>,
) -> Result<Response, StatusCode> {
    let fields = request
        .params
        .get("sink")
        .filter(|v| v.is_object())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let name = params::scalar(fields.get("name"))
        .or_else(|| existing.map(|s| s.name.clone()))
        .unwrap_or_default();
    if name.trim().is_empty() {
        return form(state, request, user, existing, Some("Name can't be blank")).await;
    }
    let account = user.account_id.ok_or(StatusCode::NOT_FOUND)?;
    let raw_folder = params::present_scalar(fields.get("folder_id"));
    let mut folder = if fields.get("folder_id").is_none() {
        existing.and_then(|s| s.folder_id)
    } else {
        None
    };
    if let Some(value) = raw_folder {
        folder = sqlx::query_scalar("SELECT id FROM folders WHERE id=$1 AND account_id=$2")
            .bind(value.parse::<i64>().unwrap_or(0))
            .bind(account)
            .fetch_optional(&state.pool)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if folder.is_none() {
            folder = existing.and_then(|s| s.folder_id);
        }
    }
    let id = if let Some(sink) = existing {
        sqlx::query("UPDATE sinks SET name=$1,folder_id=$2,updated_at=now() WHERE id=$3")
            .bind(name)
            .bind(folder)
            .bind(sink.id)
            .execute(&state.pool)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        sink.id
    } else {
        const ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
        let token: String = (0..24)
            .map(|_| ALPHABET[rand::rng().random_range(0..ALPHABET.len())] as char)
            .collect();
        let mut tx = state
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        let id:i64=sqlx::query_scalar("INSERT INTO sinks (account_id,name,token,folder_id,created_at,updated_at) VALUES ($1,$2,$3,$4,now(),now()) RETURNING id")
            .bind(account).bind(name).bind(token).bind(folder).fetch_one(&mut *tx).await.map_err(|_|StatusCode::SERVICE_UNAVAILABLE)?;
        sqlx::query("INSERT INTO sink_memberships (user_id,sink_id,last_viewed_at,created_at,updated_at) VALUES ($1,$2,now(),now(),now())").bind(user.id).bind(id).execute(&mut *tx).await.map_err(|_|StatusCode::SERVICE_UNAVAILABLE)?;
        sqlx::query("UPDATE users SET current_sink_id=$1,current_folder_id=NULL WHERE id=$2")
            .bind(id)
            .bind(user.id)
            .execute(&mut *tx)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        tx.commit()
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        id
    };
    if existing.is_none() && wants_stream(request) {
        let sink = authorized(state, user.id, id).await?;
        let mut user = user.clone();
        user.current_sink_id = Some(id);
        let context = feed_context(state, request, &user, &sink, &Filters::default()).await?;
        return fragment("created-stream.html", context, true);
    }
    Ok(redirect(&format!("/sinks/{id}"), StatusCode::FOUND))
}

async fn destroy(state: &AppState, user: &User, sink: &Sink) -> Result<Response, StatusCode> {
    let mut tx = state
        .pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    sqlx::query("SELECT id FROM sinks WHERE id=$1")
        .bind(sink.id)
        .execute(&mut *tx)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    for table in ["events", "saved_views", "columns", "sink_memberships"] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE sink_id=$1"
        )))
        .bind(sink.id)
        .execute(&mut *tx)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    }
    sqlx::query("DELETE FROM sinks WHERE id=$1")
        .bind(sink.id)
        .execute(&mut *tx)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    tx.commit()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let next:Option<i64>=sqlx::query_scalar("SELECT s.id FROM sinks s JOIN sink_memberships m ON m.sink_id=s.id WHERE m.user_id=$1 AND s.account_id=$2 ORDER BY s.name LIMIT 1")
        .bind(user.id).bind(user.account_id).fetch_optional(&state.pool).await.map_err(|_|StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(if let Some(id) = next {
        redirect(&format!("/sinks/{id}"), StatusCode::SEE_OTHER)
    } else {
        redirect("/sinks", StatusCode::FOUND)
    })
}
