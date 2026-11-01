use crate::{
    AppState,
    auth::hash_password,
    browser::{BrowserRequest, User, flash_redirect, page, redirect, valid_email},
    params,
};
use axum::{http::StatusCode, response::Response};
use serde_json::{Value, json};

fn context(user: &User, title: &str, mode: &str) -> Value {
    json!({"title":title,"mode":mode,"current_user":user,"sidebar":"settings-sidebar.html"})
}

pub async fn route(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
) -> Option<Result<Response, StatusCode>> {
    if !(request.path == "/settings"
        || request.path.starts_with("/settings/")
        || request.path == "/account/join_code")
    {
        return None;
    }
    Some(dispatch(state, request, user).await)
}

async fn dispatch(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
) -> Result<Response, StatusCode> {
    let parts: Vec<_> = request.path.trim_matches('/').split('/').collect();
    match (request.method.as_str(), parts.as_slice()) {
        ("GET", ["settings"]) => return Ok(redirect("/settings/nickname/edit", StatusCode::FOUND)),
        (
            "GET",
            [
                "settings",
                field @ ("nickname" | "email" | "password"),
                "edit",
            ],
        ) => {
            let mut ctx = context(user, &format!("change {field}"), field);
            ctx["action"] = json!(format!("/settings/{field}"));
            return page(state, request, "settings.html", ctx, StatusCode::OK);
        }
        ("PUT" | "PATCH", ["settings", field @ ("nickname" | "email" | "password")]) => {
            return update_personal(state, request, user, field).await;
        }
        _ => {}
    }
    Err(StatusCode::NOT_FOUND)
}

async fn update_personal(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    field: &str,
) -> Result<Response, StatusCode> {
    let fields = request
        .params
        .get("user")
        .filter(|v| v.is_object())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let path = format!("/settings/{field}/edit");
    if field == "password" {
        return update_password(state, fields, user, &path).await;
    }
    let (column, value, error) = if field == "nickname" {
        let value = params::scalar(fields.get("nickname")).unwrap_or_default();
        let invalid = value.trim().is_empty();
        (
            "nickname",
            value,
            invalid.then_some("Nickname can't be blank"),
        )
    } else {
        let value = params::scalar(fields.get("email_address"))
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        let invalid = !valid_email(&value);
        (
            "email_address",
            value,
            invalid.then_some("Email address is invalid"),
        )
    };
    if let Some(error) = error {
        return Ok(flash_redirect(state, &path, error, "alert"));
    }
    // Column name comes from the fixed branches above, never user input.
    let result = sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE users SET {column}=$1,updated_at=now() WHERE id=$2"
    )))
    .bind(value)
    .bind(user.id)
    .execute(&state.pool)
    .await;
    match result {
        Ok(_) => Ok(flash_redirect(state, &path, "done.", "notice")),
        Err(sqlx::Error::Database(error)) if error.is_unique_violation() => Ok(flash_redirect(
            state,
            &path,
            "Email address has already been taken",
            "alert",
        )),
        Err(_) => Err(StatusCode::SERVICE_UNAVAILABLE),
    }
}

async fn update_password(
    state: &AppState,
    fields: &Value,
    user: &User,
    path: &str,
) -> Result<Response, StatusCode> {
    let old: String = sqlx::query_scalar("SELECT password_digest FROM users WHERE id=$1")
        .bind(user.id)
        .fetch_one(&state.pool)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if let Some(challenge) = params::scalar(fields.get("password_challenge")) {
        let digest = old.clone();
        if !tokio::task::spawn_blocking(move || bcrypt::verify(challenge, &digest).unwrap_or(false))
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        {
            return Ok(flash_redirect(
                state,
                path,
                "Password challenge is invalid",
                "alert",
            ));
        }
    }
    let password = params::scalar(fields.get("password"));
    let confirmation = params::scalar(fields.get("password_confirmation"));
    let error = if fields.get("password") == Some(&Value::Null) {
        Some("Password can't be blank")
    } else if password.as_ref().is_some_and(|p| p.len() > 72) {
        Some("Password is too long")
    } else if password
        .as_ref()
        .is_some_and(|p| !p.is_empty() && confirmation.as_ref().is_some_and(|c| c != p))
    {
        Some("Password confirmation doesn't match Password")
    } else {
        None
    };
    if let Some(error) = error {
        return Ok(flash_redirect(state, path, error, "alert"));
    }
    let digest = if let Some(password) = password.filter(|p| !p.is_empty()) {
        hash_password(password).await?
    } else {
        old.clone()
    };
    let mut tx = state
        .pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let current: String = sqlx::query_scalar("SELECT password_digest FROM users WHERE id=$1")
        .bind(user.id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if current != old {
        return Ok(flash_redirect(
            state,
            path,
            "Password challenge is invalid",
            "alert",
        ));
    }
    sqlx::query("UPDATE users SET password_digest=$1,updated_at=now() WHERE id=$2")
        .bind(digest)
        .bind(user.id)
        .execute(&mut *tx)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    sqlx::query("DELETE FROM sessions WHERE user_id=$1 AND id<>$2")
        .bind(user.id)
        .bind(user.session_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    tx.commit()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(flash_redirect(state, path, "done.", "notice"))
}
