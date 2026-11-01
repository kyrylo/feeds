use crate::{
    AppState,
    browser::{BrowserRequest, User, page, redirect},
    params, sinks,
};
use axum::{http::StatusCode, response::Response};
use serde::Serialize;
use serde_json::json;

#[derive(sqlx::FromRow, Serialize)]
struct Folder {
    id: i64,
    name: String,
}

pub async fn route(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
) -> Option<Result<Response, StatusCode>> {
    let parts: Vec<_> = request.path.trim_matches('/').split('/').collect();
    if parts.first() != Some(&"folders") {
        return None;
    }
    Some(dispatch(state, request, user, &parts).await)
}

async fn dispatch(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    parts: &[&str],
) -> Result<Response, StatusCode> {
    if request.method != "GET" || parts.last() == Some(&"new") || parts.last() == Some(&"edit") {
        sinks::admin(user)?;
    }
    match (request.method.as_str(), parts) {
        ("GET", ["folders", "new"]) => form(state, request, user, None, None).await,
        ("POST", ["folders"]) => save(state, request, user, None).await,
        (_, ["folders", id, rest @ ..]) => {
            let folder: Folder =
                sqlx::query_as("SELECT id,name FROM folders WHERE id=$1 AND account_id=$2")
                    .bind(sinks::parse_id(id)?)
                    .bind(user.account_id)
                    .fetch_optional(&state.pool)
                    .await
                    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
                    .ok_or(StatusCode::NOT_FOUND)?;
            match (request.method.as_str(), rest) {
                ("GET", []) => {
                    let sink: Option<i64> = sqlx::query_scalar("SELECT s.id FROM sinks s JOIN sink_memberships m ON m.sink_id=s.id WHERE s.folder_id=$1 AND s.account_id=$2 AND m.user_id=$3 ORDER BY s.name LIMIT 1")
                        .bind(folder.id).bind(user.account_id).bind(user.id).fetch_optional(&state.pool).await
                        .map_err(|_|StatusCode::SERVICE_UNAVAILABLE)?;
                    Ok(redirect(
                        &sink
                            .map(|id| format!("/sinks/{id}"))
                            .unwrap_or_else(|| "/sinks".into()),
                        StatusCode::SEE_OTHER,
                    ))
                }
                ("GET", ["edit"]) => form(state, request, user, Some(&folder), None).await,
                ("PATCH" | "PUT", []) => save(state, request, user, Some(&folder)).await,
                ("DELETE", []) => {
                    let mut tx = state
                        .pool
                        .begin_with("BEGIN IMMEDIATE")
                        .await
                        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
                    sqlx::query("SELECT id FROM folders WHERE id=$1")
                        .bind(folder.id)
                        .execute(&mut *tx)
                        .await
                        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
                    // Rails' dependent: :nullify preserves each source and its token/events.
                    sqlx::query("UPDATE sinks SET folder_id=NULL WHERE folder_id=$1")
                        .bind(folder.id)
                        .execute(&mut *tx)
                        .await
                        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
                    sqlx::query("DELETE FROM folders WHERE id=$1")
                        .bind(folder.id)
                        .execute(&mut *tx)
                        .await
                        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
                    tx.commit()
                        .await
                        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
                    Ok(redirect("/sinks", StatusCode::FOUND))
                }
                _ => Err(StatusCode::NOT_FOUND),
            }
        }
        _ => Err(StatusCode::NOT_FOUND),
    }
}

async fn form(
    state: &AppState,
    request: &BrowserRequest,
    user: &User,
    folder: Option<&Folder>,
    error: Option<&str>,
) -> Result<Response, StatusCode> {
    let title = folder
        .map(|f| format!("edit \"{}\"", f.name))
        .unwrap_or_else(|| "new folder".into());
    let mut context = sinks::base_context(state, request, user, &title).await?;
    context["folder"] = json!(folder);
    context["form_name"] = request
        .params
        .get("folder")
        .and_then(|v| v.get("name"))
        .cloned()
        .unwrap_or_else(|| json!(folder.map(|f| &f.name)));
    context["action"] = json!(
        folder
            .map(|f| format!("/folders/{}", f.id))
            .unwrap_or_else(|| "/folders".into())
    );
    context["error"] = json!(error);
    context["sidebar_mode"] = json!(if folder.is_some() { "edit" } else { "create" });
    context["creating_folder"] = json!(folder.is_none());
    page(
        state,
        request,
        "folder-form.html",
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
    folder: Option<&Folder>,
) -> Result<Response, StatusCode> {
    let fields = request
        .params
        .get("folder")
        .filter(|v| v.is_object())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let name = params::scalar(fields.get("name"))
        .or_else(|| folder.map(|f| f.name.clone()))
        .unwrap_or_default();
    if name.trim().is_empty() {
        return form(state, request, user, folder, Some("Name can't be blank")).await;
    }
    if let Some(folder) = folder {
        sqlx::query("UPDATE folders SET name=$1,updated_at=now() WHERE id=$2")
            .bind(name)
            .bind(folder.id)
            .execute(&state.pool)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    } else {
        sqlx::query("INSERT INTO folders (account_id,name,created_at,updated_at) VALUES ($1,$2,now(),now())")
            .bind(user.account_id.ok_or(StatusCode::NOT_FOUND)?).bind(name).execute(&state.pool).await.map_err(|_|StatusCode::SERVICE_UNAVAILABLE)?;
        if sinks::wants_stream(request) {
            return sinks::fragment(
                "sidebar-stream.html",
                sinks::base_context(state, request, user, "sinks").await?,
                true,
            );
        }
    }
    Ok(redirect("/sinks", StatusCode::FOUND))
}
