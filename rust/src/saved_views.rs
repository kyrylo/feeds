use crate::{
    browser::redirect,
    feed::{Filters, date},
    feed_view, params,
};
use axum::{http::StatusCode, response::Response};
use chrono::NaiveDate;
use serde_json::{Value, json};
use sqlx::SqlitePool;

#[derive(sqlx::FromRow)]
pub struct SavedView {
    pub id: i64,
    pub name: String,
    #[sqlx(flatten)]
    pub filters: Filters,
}

pub async fn list(
    pool: &SqlitePool,
    user: i64,
    sink: i64,
    current: &Filters,
    month: Option<NaiveDate>,
) -> Result<Vec<Value>, sqlx::Error> {
    let views: Vec<SavedView> = sqlx::query_as(
        "SELECT * FROM saved_views WHERE user_id=$1 AND sink_id=$2 ORDER BY name,id",
    )
    .bind(user)
    .bind(sink)
    .fetch_all(pool)
    .await?;
    let path = format!("/sinks/{sink}");
    Ok(views.into_iter().map(|view|json!({"id":view.id,"name":view.name,"summary":feed_view::summary(&view.filters),
        "current":view.filters==*current,"url":view.filters.url(&path,view.filters.event_date.or(month)),
        "delete_url":current.url(&format!("{path}/saved_views/{}",view.id),month)})).collect())
}

pub async fn create(
    pool: &SqlitePool,
    user: i64,
    sink: i64,
    parameters: &Value,
) -> Result<Response, StatusCode> {
    let fields = parameters
        .get("saved_view")
        .filter(|v| v.is_object())
        .ok_or(StatusCode::BAD_REQUEST)?;
    let filters = Filters::from_params(fields);
    let destination = filters.url(&format!("/sinks/{sink}"), date(parameters.get("month")));
    if filters.can_mark_viewed()
        || filters
            .event_type
            .as_ref()
            .is_some_and(|s| s.chars().count() > 255)
        || filters
            .search_query
            .as_ref()
            .is_some_and(|s| s.chars().count() > 255)
        || filters
            .property_key
            .as_ref()
            .is_some_and(|s| s.chars().count() > 255)
        || filters
            .property_value
            .as_ref()
            .is_some_and(|s| s.chars().count() > 1024)
    {
        return Ok(redirect(&destination, StatusCode::SEE_OTHER));
    }
    let name = params::scalar(fields.get("name"))
        .unwrap_or_default()
        .trim()
        .to_owned();
    let base = feed_view::truncate(
        &if name.is_empty() {
            feed_view::summary(&filters)
        } else {
            name
        },
        80,
        false,
    );
    let mut tx = pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    // Serialize a user's name allocation to retain Rails' suffixes under retries.
    sqlx::query("SELECT id FROM users WHERE id=$1")
        .bind(user)
        .execute(&mut *tx)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let mut name = base.clone();
    let mut suffix = 2;
    loop {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM saved_views WHERE user_id=$1 AND sink_id=$2 AND name=$3)",
        )
        .bind(user)
        .bind(sink)
        .bind(&name)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        if !exists {
            break;
        }
        let tail = format!(" {suffix}");
        name = feed_view::truncate(&base, 80 - tail.len(), false) + &tail;
        suffix += 1;
    }
    sqlx::query("INSERT INTO saved_views (user_id,sink_id,name,event_type,event_date,search_query,property_key,property_op,property_value,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,now(),now())")
        .bind(user).bind(sink).bind(name).bind(&filters.event_type).bind(filters.event_date).bind(&filters.search_query)
        .bind(&filters.property_key).bind(&filters.property_op).bind(&filters.property_value).execute(&mut *tx).await.map_err(|_|StatusCode::SERVICE_UNAVAILABLE)?;
    tx.commit()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(redirect(&destination, StatusCode::SEE_OTHER))
}

pub async fn destroy(
    pool: &SqlitePool,
    user: i64,
    sink: i64,
    id: i64,
    parameters: &Value,
) -> Result<Response, StatusCode> {
    let result = sqlx::query("DELETE FROM saved_views WHERE id=$1 AND user_id=$2 AND sink_id=$3")
        .bind(id)
        .bind(user)
        .bind(sink)
        .execute(pool)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if result.rows_affected() == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    let filters = Filters::from_params(parameters.get("saved_view").unwrap_or(parameters));
    Ok(redirect(
        &filters.url(&format!("/sinks/{sink}"), date(parameters.get("month"))),
        StatusCode::SEE_OTHER,
    ))
}
