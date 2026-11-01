//! The demo retains Rails' explicit route exceptions and source permissions.
use crate::{
    AppState,
    browser::{self, BrowserRequest},
};
use axum::{
    http::{Method, StatusCode, header},
    response::Response,
};
use rand::{Rng, distr::Alphanumeric};

pub fn allowed(request: &BrowserRequest) -> bool {
    let parts: Vec<_> = request.path.trim_matches('/').split('/').collect();
    let id = |s: &str| s.parse::<i64>().is_ok_and(|id| id > 0);
    match (request.method.as_str(), parts.as_slice()) {
        ("GET", [""] | ["sinks"]) => true,
        ("GET", ["sinks" | "folders" | "events", resource]) => id(resource),
        ("GET", ["sinks", sink, "events"]) => id(sink),
        ("POST", ["sinks", sink, "view" | "saved_views" | "columns"]) => id(sink),
        ("PATCH" | "PUT", ["sinks", sink, "column_order"]) => id(sink),
        ("GET" | "PUT" | "PATCH" | "DELETE", ["sinks", sink, "columns", column]) => {
            id(sink) && id(column)
        }
        ("GET", ["sinks", sink, "columns", column, "edit" | "events"]) => id(sink) && id(column),
        ("POST", ["sinks", sink, "columns", column, "views"]) => id(sink) && id(column),
        _ => false,
    }
}

pub fn return_path(request: &BrowserRequest) -> String {
    request
        .headers
        .get(header::REFERER)
        .and_then(|h| h.to_str().ok())
        .and_then(|url| url.strip_prefix(&format!("{}/", request.base_url())))
        .map(|path| format!("/{path}"))
        .filter(|path| !path.starts_with("//") && !path.contains('\\'))
        .unwrap_or_else(|| "/".into())
}

pub async fn start_session(
    state: &AppState,
    request: &BrowserRequest,
) -> Result<Response, StatusCode> {
    let mut tx = state
        .pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let existing: Option<i64> =
        sqlx::query_scalar("SELECT id FROM users WHERE email_address='demo@telesink.com'")
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let user = if let Some(id) = existing {
        id
    } else {
        let password: String = rand::rng()
            .sample_iter(Alphanumeric)
            .take(64)
            .map(char::from)
            .collect();
        let digest = crate::auth::hash_password(password).await?;
        let account: i64 = sqlx::query_scalar("INSERT INTO accounts (join_code,created_at,updated_at) VALUES ($1,now(),now()) RETURNING id")
            .bind(browser::join_code()).fetch_one(&mut *tx).await.map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        sqlx::query_scalar("INSERT INTO users (account_id,email_address,nickname,password_digest,role,created_at,updated_at) VALUES ($1,'demo@telesink.com','demo',$2,0,now(),now()) RETURNING id")
            .bind(account).bind(digest).fetch_one(&mut *tx).await.map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
    };
    tx.commit()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let destination = if request.method == Method::GET {
        request.uri.as_str()
    } else {
        "/"
    };
    browser::start_session(state, request, user, destination).await
}
