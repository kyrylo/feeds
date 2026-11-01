//! Feed management. Each feed owns one ingestion identity and URL.
use crate::{
    AppState, activity,
    browser::{BrowserRequest, User, page, redirect},
    feed::Filters,
    params,
};
use axum::{http::StatusCode, response::Response};
use rand::Rng;
use serde_json::{Value, json};

#[derive(sqlx::FromRow)]
pub(crate) struct Tab {
    pub id: i64,
    pub source_id: Option<i64>,
    pub name: String,
    pub last_seen_id: i64,
}
impl Tab {
    pub fn scope(&self) -> Value {
        json!({"source": self.source_id, "filters": self.filters()})
    }
    pub fn filters(&self) -> Filters {
        Filters::default()
    }
    pub fn represents_source(&self, source: i64) -> bool {
        self.source_id == Some(source) && self.filters().can_mark_viewed()
    }
}

// Retain existing resource IDs and visit watermarks. Old saved searches remain
// stored, but only one canonical feed per source appears in the application.
const SELECT: &str = "SELECT t.*,v.last_seen_id FROM telesink_activity_tabs t JOIN telesink_activity_tab_visits v ON v.tab_id=t.id JOIN sinks s ON s.id=t.source_id JOIN sink_memberships m ON m.sink_id=s.id AND m.user_id=t.user_id JOIN users u ON u.id=t.user_id WHERE t.user_id=$1 AND s.account_id=u.account_id AND t.id=(SELECT x.id FROM telesink_activity_tabs x WHERE x.user_id=t.user_id AND x.source_id=t.source_id ORDER BY (COALESCE(json_extract(x.filters,'$.search_query'),'')='') DESC,x.id LIMIT 1)";
fn unavailable(_: sqlx::Error) -> StatusCode {
    StatusCode::SERVICE_UNAVAILABLE
}
pub(crate) async fn list(state: &AppState, user: &User) -> Result<Vec<Tab>, StatusCode> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} ORDER BY t.position,t.id"
    )))
    .bind(user.id)
    .fetch_all(&state.pool)
    .await
    .map_err(unavailable)
}
pub(crate) async fn find(state: &AppState, user: &User, id: i64) -> Result<Tab, StatusCode> {
    let mut connection = state.pool.acquire().await.map_err(unavailable)?;
    find_on(&mut connection, user, id).await
}
pub(crate) async fn find_on(
    connection: &mut sqlx::SqliteConnection,
    user: &User,
    id: i64,
) -> Result<Tab, StatusCode> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} AND t.id=$2")))
        .bind(user.id)
        .bind(id)
        .fetch_optional(connection)
        .await
        .map_err(unavailable)?
        .ok_or(StatusCode::NOT_FOUND)
}
pub(crate) async fn resolve(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
) -> Result<(BrowserRequest, Option<Tab>), StatusCode> {
    let mut effective = request.clone();
    let selected = if let Some(id) = selected_id(&request.path) {
        let tab = find(state, user, id).await?;
        // The URL identifies the feed; query parameters can only search it.
        effective.params["source"] = json!(tab.source_id.unwrap().to_string());
        Some(tab)
    } else {
        None
    };
    Ok((effective, selected))
}

pub(crate) fn selected_id(path: &str) -> Option<i64> {
    path.strip_prefix("/feeds/")?
        .split('/')
        .next()?
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
}
pub(crate) async fn navigation(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    tabs: &[Tab],
    selected: Option<i64>,
    through: i64,
) -> Result<Vec<Value>, StatusCode> {
    let mut result = Vec::with_capacity(tabs.len());
    for tab in tabs {
        let source = tab.source_id.filter(|&id| tab.represents_source(id));
        let current = selected == Some(tab.id)
            || (selected.is_none()
                && source.is_some_and(|id| {
                    params::scalar(request.params.get("source")) == Some(id.to_string())
                }));
        let unread = activity::unread_count(
            state,
            request,
            user,
            tab.source_id,
            &tab.filters(),
            tab.last_seen_id,
            through,
        )
        .await?;
        result.push(json!({"id":tab.id.to_string(),"source_id":source.map(|id|id.to_string()),"name":tab.name,"url":format!("/feeds/{}",tab.id),"current":current,"unread":unread,"unread_label":if unread>999 {"999+".into()} else {unread.to_string()}}));
    }
    Ok(result)
}
pub(crate) async fn route(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
) -> Result<Response, StatusCode> {
    let tail = request
        .path
        .strip_prefix("/feeds")
        .ok_or(StatusCode::NOT_FOUND)?;
    match (request.method.as_str(), tail) {
        ("GET", "") => activity::show(state, request, user).await,
        ("GET", "/new") if user.role == 2 => form(state, request, user, None, None).await,
        ("POST", "") if user.role == 2 => save(state, request, user, None).await,
        ("GET", "/new") | ("POST", "") => Err(StatusCode::FORBIDDEN),
        _ => {
            let mut parts = tail.strip_prefix('/').unwrap_or(tail).split('/');
            let id = parts
                .next()
                .and_then(|id| id.parse::<i64>().ok().filter(|id| *id > 0))
                .ok_or(StatusCode::NOT_FOUND)?;
            let action = parts.next();
            if parts.next().is_some() {
                return Err(StatusCode::NOT_FOUND);
            }
            match (request.method.as_str(), action) {
                ("GET", None) => return activity::show(state, request, user).await,
                ("GET", Some("updates")) => return activity::updates(state, request, user).await,
                ("GET", Some("older")) => return activity::older(state, request, user).await,
                _ => {}
            }
            if user.role != 2 {
                return Err(StatusCode::FORBIDDEN);
            }
            let tab = find(state, user, id).await?;
            match (request.method.as_str(), action) {
                ("GET", Some("edit")) => form(state, request, user, Some(&tab), None).await,
                ("PATCH" | "PUT", None) => save(state, request, user, Some(&tab)).await,
                ("DELETE", None) => destroy(state, user, &tab).await,
                ("PATCH", Some("url")) => {
                    let mut tx = state
                        .pool
                        .begin_with("BEGIN IMMEDIATE")
                        .await
                        .map_err(unavailable)?;
                    let tab = find_on(&mut tx, user, id).await?;
                    sqlx::query("UPDATE sinks SET token=$1,updated_at=now() WHERE id=$2")
                        .bind(new_token())
                        .bind(tab.source_id)
                        .execute(&mut *tx)
                        .await
                        .map_err(unavailable)?;
                    tx.commit().await.map_err(unavailable)?;
                    Ok(redirect(
                        &format!("/feeds/{id}/edit"),
                        StatusCode::SEE_OTHER,
                    ))
                }
                ("PATCH", Some("move")) => {
                    let direction = params::scalar(request.params.get("direction"))
                        .ok_or(StatusCode::BAD_REQUEST)?;
                    if direction != "left" && direction != "right" {
                        return Err(StatusCode::BAD_REQUEST);
                    }
                    let mut tx = state
                        .pool
                        .begin_with("BEGIN IMMEDIATE")
                        .await
                        .map_err(unavailable)?;
                    let tabs: Vec<Tab> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                        "{SELECT} ORDER BY t.position,t.id"
                    )))
                    .bind(user.id)
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(unavailable)?;
                    let index = tabs
                        .iter()
                        .position(|tab| tab.id == id)
                        .ok_or(StatusCode::NOT_FOUND)?;
                    let next = if direction == "left" {
                        index.checked_sub(1)
                    } else {
                        index.checked_add(1).filter(|n| *n < tabs.len())
                    };
                    if let Some(next) = next {
                        // Normalize positions as well as swapping: imported views may have gaps.
                        let mut order: Vec<_> = tabs.iter().map(|tab| tab.id).collect();
                        order.swap(index, next);
                        for (position, id) in order.into_iter().enumerate() {
                            sqlx::query("UPDATE telesink_activity_tabs SET position=$1,updated_at=now() WHERE id=$2 AND user_id=$3").bind(position as i64).bind(id).bind(user.id).execute(&mut *tx).await.map_err(unavailable)?;
                        }
                    }
                    tx.commit().await.map_err(unavailable)?;
                    Ok(redirect(
                        &format!("/feeds/{id}/edit"),
                        StatusCode::SEE_OTHER,
                    ))
                }
                _ => Err(StatusCode::NOT_FOUND),
            }
        }
    }
}
pub(crate) fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    format!(
        "feed_{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
}

pub(crate) fn endpoint(state: &AppState, request: &BrowserRequest, token: &str) -> String {
    let base = state
        .config
        .public_url
        .clone()
        .unwrap_or_else(|| request.base_url());
    format!("{base}/api/v1/feeds/{token}/events")
}

async fn form(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    tab: Option<&Tab>,
    error: Option<&str>,
) -> Result<Response, StatusCode> {
    let name = params::scalar(request.params.get("name"))
        .or_else(|| tab.map(|t| t.name.clone()))
        .unwrap_or_default();
    let tabs = list(state, user).await?;
    let index = tab.and_then(|t| tabs.iter().position(|v| v.id == t.id));
    let connection = if let Some(tab) = tab {
        let token: String = sqlx::query_scalar("SELECT token FROM sinks WHERE id=$1")
            .bind(tab.source_id)
            .fetch_one(&state.pool)
            .await
            .map_err(unavailable)?;
        Some(endpoint(state, request, &token))
    } else {
        None
    };
    let example = json!({"event":"user_signed_up","emoji":"👋","text":"Maya signed up","properties":{"plan":"Pro"}});
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let snippet = connection.as_ref().map(|url| {
        format!(
            "curl {} \\\n  -H 'Content-Type: application/json' \\\n  -d {}",
            quote(url),
            quote(&serde_json::to_string_pretty(&example).unwrap())
        )
    });
    page(
        state,
        request,
        "activity-tab-form.html",
        json!({"title":tab.map(|t|format!("Edit {} · Feeds",t.name)).unwrap_or_else(||"New feed · Feeds".into()),
            "current_user":user,"tab_id":tab.map(|t|t.id.to_string()),"name":name,"error":error,
            "endpoint":connection,"snippet":snippet,
            "save_path":tab.map(|t|format!("/feeds/{}",t.id)).unwrap_or_else(||"/feeds".into()),
            "can_left":index.is_some_and(|i|i>0),"can_right":index.is_some_and(|i|i+1<tabs.len())}),
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
    tab: Option<&Tab>,
) -> Result<Response, StatusCode> {
    let name = params::scalar(request.params.get("name"))
        .unwrap_or_default()
        .trim()
        .to_owned();
    if name.is_empty() || name.chars().count() > 80 {
        return form(
            state,
            request,
            user,
            tab,
            Some("Give this feed a name between 1 and 80 characters."),
        )
        .await;
    }
    let mut tx = state
        .pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(unavailable)?;
    if let Some(tab) = tab {
        find_on(&mut tx, user, tab.id).await?;
    }
    let duplicate: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sinks WHERE account_id=$1 AND name=$2 COLLATE NOCASE AND id<>$3) OR EXISTS(SELECT 1 FROM telesink_activity_tabs WHERE user_id=$4 AND source_id<>$3 AND name=$2 COLLATE NOCASE)")
        .bind(user.account_id).bind(&name).bind(tab.and_then(|t|t.source_id).unwrap_or(-1)).bind(user.id)
        .fetch_one(&mut *tx).await.map_err(unavailable)?;
    if duplicate {
        tx.rollback().await.map_err(unavailable)?;
        return form(
            state,
            request,
            user,
            tab,
            Some("You already have a feed with that name."),
        )
        .await;
    }
    let id = if let Some(tab) = tab {
        sqlx::query("UPDATE sinks SET name=$1,updated_at=now() WHERE id=$2")
            .bind(&name)
            .bind(tab.source_id)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("UPDATE telesink_activity_tabs SET name=$1,updated_at=now() WHERE id=$2")
            .bind(&name)
            .bind(tab.id)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        tab.id
    } else {
        let source: i64 = sqlx::query_scalar("INSERT INTO sinks(account_id,name,token,created_at,updated_at) VALUES ($1,$2,$3,now(),now()) RETURNING id")
            .bind(user.account_id).bind(&name).bind(new_token()).fetch_one(&mut *tx).await.map_err(unavailable)?;
        sqlx::query("INSERT INTO sink_memberships(user_id,sink_id,created_at,updated_at) VALUES ($1,$2,now(),now())")
            .bind(user.id).bind(source).execute(&mut *tx).await.map_err(unavailable)?;
        sqlx::query_scalar("INSERT INTO telesink_activity_tabs(user_id,source_id,name,filters,position,created_at,updated_at) SELECT $1,$2,$3,'{}',COALESCE(MAX(position),-1)+1,now(),now() FROM telesink_activity_tabs WHERE user_id=$1 RETURNING id")
            .bind(user.id).bind(source).bind(&name).fetch_one(&mut *tx).await.map_err(unavailable)?
    };
    tx.commit().await.map_err(unavailable)?;
    let path = if tab.is_some() {
        format!("/feeds/{id}")
    } else {
        format!("/feeds/{id}/edit")
    };
    Ok(redirect(&path, StatusCode::SEE_OTHER))
}

async fn destroy(state: &AppState, user: &User, tab: &Tab) -> Result<Response, StatusCode> {
    let mut tx = state
        .pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(unavailable)?;
    let tab = find_on(&mut tx, user, tab.id).await?;
    for table in ["events", "saved_views", "columns", "sink_memberships"] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE sink_id=$1"
        )))
        .bind(tab.source_id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
    }
    sqlx::query("DELETE FROM sinks WHERE id=$1")
        .bind(tab.source_id)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
    tx.commit().await.map_err(unavailable)?;
    Ok(redirect("/connect", StatusCode::SEE_OTHER))
}
