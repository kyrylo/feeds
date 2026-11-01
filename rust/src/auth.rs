use axum::{http::StatusCode, response::Response};
use chrono::Utc;
use serde_json::{Value, json};

use crate::{
    AppState,
    browser::{BrowserRequest, flash_redirect, page, start_session},
    params,
    tokens::password_salt_tail,
};

/// Password recovery and previously issued sign-in links. Invitations are retired.
pub async fn route(
    state: &AppState,
    request: &BrowserRequest,
) -> Option<Result<Response, StatusCode>> {
    let decoded: Option<Vec<_>> = request
        .path
        .trim_matches('/')
        .split('/')
        .map(|s| {
            percent_encoding::percent_decode_str(s)
                .decode_utf8()
                .ok()
                .map(|s| s.into_owned())
        })
        .collect();
    let Some(decoded) = decoded else {
        return Some(Err(StatusCode::BAD_REQUEST));
    };
    let parts: Vec<_> = decoded.iter().map(String::as_str).collect();
    match (request.method.as_str(), parts.as_slice()) {
        ("GET" | "POST", ["join", _]) => Some(Err(StatusCode::NOT_FOUND)),
        ("GET", ["session", "transfer", token]) => Some(transfer(state, request, token).await),
        ("GET", ["passwords", "new"]) => Some(page(
            state,
            request,
            "password.html",
            json!({
                "title":"forgot your password?","mode":"request","action":"/passwords",
                "local_recovery":state.config.mailer.is_none(),
                "email_address":request.params.get("email_address")
            }),
            StatusCode::OK,
        )),
        ("POST", ["passwords"]) => Some(request_password(state, request).await),
        ("GET", ["passwords", token, "edit"]) => Some(edit_password(state, request, token).await),
        ("PUT" | "PATCH", ["passwords", token]) => {
            Some(update_password(state, request, token).await)
        }
        _ => None,
    }
}

async fn transfer(
    state: &AppState,
    request: &BrowserRequest,
    token: &str,
) -> Result<Response, StatusCode> {
    if let Some(id) = state.tokens.verify_transfer(token, Utc::now()) {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1)")
            .bind(id)
            .fetch_one(&state.pool)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if exists {
            return start_session(state, request, id, "/sinks").await;
        }
    }
    Ok(flash_redirect(
        state,
        "/session/new",
        "this sign-in link is invalid or has expired.",
        "alert",
    ))
}

fn invalid_reset(state: &AppState) -> Response {
    flash_redirect(
        state,
        "/passwords/new",
        "password reset link is invalid or has expired.",
        "alert",
    )
}

async fn request_password(
    state: &AppState,
    request: &BrowserRequest,
) -> Result<Response, StatusCode> {
    if crate::browser::limited(request.ip, "password_reset") {
        return Ok(flash_redirect(
            state,
            "/passwords/new",
            "Try again later.",
            "alert",
        ));
    }
    let failure = || {
        flash_redirect(
            state,
            "/passwords/new",
            "password reset email is unavailable. please try again later.",
            "alert",
        )
    };
    let Some(mailer) = &state.config.mailer else {
        return Ok(failure());
    };
    // Rails normalizes the email attribute when querying it, too.
    let email = params::scalar(request.params.get("email_address"))
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    let user: Option<(i64, String)> =
        sqlx::query_as("SELECT id,password_digest FROM users WHERE email_address=$1")
            .bind(&email)
            .fetch_optional(&state.pool)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if let Some((id, digest)) = user {
        let token = state
            .tokens
            .issue_reset(id, &digest, Utc::now())
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
        if mailer.password_reset(&email, &token).await.is_err() {
            // Do not log addresses, credentials, reset links, or raw SMTP replies.
            eprintln!("telesink: password reset SMTP delivery failed");
            return Ok(failure());
        }
    }
    Ok(flash_redirect(
        state,
        "/session/new",
        "password reset instructions sent (if user with that email address exists).",
        "notice",
    ))
}

async fn reset_user(state: &AppState, token: &str) -> Result<Option<(i64, String)>, StatusCode> {
    let Some((id, salt)) = state.tokens.reset_claims(token, Utc::now()) else {
        return Ok(None);
    };
    let digest: Option<String> =
        sqlx::query_scalar("SELECT password_digest FROM users WHERE id=$1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(digest
        .filter(|d| password_salt_tail(d) == Some(salt.as_str()))
        .map(|d| (id, d)))
}

async fn edit_password(
    state: &AppState,
    request: &BrowserRequest,
    token: &str,
) -> Result<Response, StatusCode> {
    if reset_user(state, token).await?.is_none() {
        return Ok(invalid_reset(state));
    }
    page(
        state,
        request,
        "password.html",
        json!({"title":"update your password","mode":"edit",
        "action":format!("/passwords/{token}")}),
        StatusCode::OK,
    )
}

async fn update_password(
    state: &AppState,
    request: &BrowserRequest,
    token: &str,
) -> Result<Response, StatusCode> {
    let Some((id, old_digest)) = reset_user(state, token).await? else {
        return Ok(invalid_reset(state));
    };
    let password = params::scalar(request.params.get("password"));
    let confirmation = params::scalar(request.params.get("password_confirmation"));
    if request.params.get("password") == Some(&Value::Null)
        || password.as_ref().is_some_and(|p| {
            p.len() > 72 || (!p.is_empty() && confirmation.as_ref().is_some_and(|c| c != p))
        })
    {
        return Ok(flash_redirect(
            state,
            &format!("/passwords/{token}/edit"),
            "passwords did not match.",
            "alert",
        ));
    }
    let digest = if let Some(password) = password.filter(|p| !p.is_empty()) {
        hash_password(password).await?
    } else {
        old_digest.clone()
    };
    let mut tx = state
        .pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    // Re-check after acquiring the row lock. A token redeemed concurrently or
    // a password changed in settings must not overwrite the newer credential.
    let current: Option<String> =
        sqlx::query_scalar("SELECT password_digest FROM users WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if current.as_deref() != Some(old_digest.as_str())
        || state.tokens.reset_claims(token, Utc::now()).is_none()
    {
        return Ok(invalid_reset(state));
    }
    sqlx::query("UPDATE users SET password_digest=$1,updated_at=now() WHERE id=$2")
        .bind(digest)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    sqlx::query("DELETE FROM sessions WHERE user_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    tx.commit()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(flash_redirect(
        state,
        "/session/new",
        "password has been reset.",
        "notice",
    ))
}

pub async fn hash_password(password: String) -> Result<String, StatusCode> {
    tokio::task::spawn_blocking(move || bcrypt::hash(password, 12))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)
}
