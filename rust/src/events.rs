use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{SqliteConnection, SqlitePool};

use crate::{AppState, params};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventInput {
    pub event_type: String,
    pub emoji: String,
    pub text: String,
    pub properties: Value,
    pub occurred_at: NaiveDateTime,
    pub idempotency_key: Option<String>,
    pub sdk_name: Option<String>,
    pub sdk_version: Option<String>,
}

impl EventInput {
    pub fn from_params(raw: &Value, now: NaiveDateTime) -> Result<Self, Vec<String>> {
        let text = params::present_scalar(raw.get("text"))
            .ok_or_else(|| vec!["Text can't be blank".to_owned()])?;
        let event_type = params::present_scalar(raw.get("event")).unwrap_or_else(|| "event".into());
        let emoji = params::present_scalar(raw.get("emoji")).unwrap_or_else(|| "📌".into());
        let occurred_at = params::scalar(raw.get("occurred_at"))
            .and_then(|time| DateTime::parse_from_rfc3339(&time).ok())
            .map(|time| time.naive_utc())
            .unwrap_or(now);
        let idempotency_key = params::present_scalar(raw.get("idempotency_key"));
        let mut properties = raw
            .get("properties")
            .filter(|v| v.is_object())
            .cloned()
            .unwrap_or_else(|| json!({}));
        normalize_properties(&mut properties);
        let sdk = raw.get("sdk");
        Ok(Self {
            text,
            event_type,
            emoji,
            occurred_at,
            properties,
            idempotency_key,
            sdk_name: params::scalar(sdk.and_then(|v| v.get("name"))),
            sdk_version: params::scalar(sdk.and_then(|v| v.get("version"))),
        })
    }

    pub fn search_text(&self) -> String {
        let mut parts = vec![self.event_type.clone(), self.text.clone()];
        if let Some(properties) = self.properties.as_object() {
            for (key, value) in properties {
                parts.push(key.clone());
                parts.push(match value {
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                });
            }
        }
        parts
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn normalize_properties(value: &mut Value) {
    // Event#normalize_property_encodings calls to_h on nil in the existing
    // Rails implementation. Keep that persisted representation during the port.
    match value {
        Value::Null => *value = json!({}),
        Value::Object(map) => map.values_mut().for_each(normalize_properties),
        Value::Array(items) => items.iter_mut().for_each(normalize_properties),
        _ => {}
    }
}

pub async fn ingest(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Match Rails: token authentication precedes event validation.
    let sink_id: Option<i64> =
        match sqlx::query_scalar("SELECT id FROM sinks WHERE token = $1 LIMIT 1")
            .bind(&token)
            .fetch_optional(&state.pool)
            .await
        {
            Ok(id) => id,
            Err(_) => return database_failure(),
        };
    let Some(sink_id) = sink_id else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let raw = if content_type
        .split(';')
        .next()
        .is_some_and(|value| value == "application/json" || value.ends_with("+json"))
    {
        match serde_json::from_slice::<Value>(&body) {
            Ok(value) => value,
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        }
    } else if content_type.starts_with("application/x-www-form-urlencoded") {
        match params::parse_form(&body) {
            Ok(value) => value,
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        }
    } else {
        json!({})
    };
    let mut event = match EventInput::from_params(&raw, Utc::now().naive_utc()) {
        Ok(event) => event,
        Err(errors) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({"errors": errors})),
            )
                .into_response();
        }
    };
    if event.idempotency_key.is_none() {
        event.idempotency_key = headers
            .get("idempotency-key")
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned);
    }
    if event.emoji.chars().count() > 8
        || event
            .idempotency_key
            .as_ref()
            .is_some_and(|s| s.chars().count() > 255)
    {
        return (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"errors":["Emoji is limited to 8 characters and idempotency keys to 255 characters."]}))).into_response();
    }
    match persist_authenticated(&state.pool, sink_id, &token, &event).await {
        Ok(true) => StatusCode::CREATED.into_response(),
        Ok(false) => StatusCode::UNAUTHORIZED.into_response(),
        Err(_) => database_failure(),
    }
}

async fn persist_authenticated(
    pool: &SqlitePool,
    sink_id: i64,
    token: &str,
    event: &EventInput,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    // Rotation and deletion take the same writer lock: an obsolete URL cannot
    // acknowledge a write after either operation commits.
    let valid: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sinks WHERE id=$1 AND token=$2)")
            .bind(sink_id)
            .bind(token)
            .fetch_one(&mut *tx)
            .await?;
    if !valid {
        return Ok(false);
    }
    persist_in(&mut tx, sink_id, event).await?;
    tx.commit().await?;
    Ok(true)
}

fn database_failure() -> Response {
    // No tokens, submitted event data, SQL parameters or credentials in logs.
    eprintln!("event persistence failed; request was not acknowledged");
    StatusCode::SERVICE_UNAVAILABLE.into_response()
}

/// All durable effects are committed before HTTP success. The existing unique
/// index arbitrates concurrent retries; a duplicate never changes stored data or
/// increments unread counters. PostgreSQL notifications contain only the ID.
pub async fn persist(
    pool: &SqlitePool,
    sink_id: i64,
    event: &EventInput,
) -> Result<Option<i64>, sqlx::Error> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let id = persist_in(&mut tx, sink_id, event).await?;
    tx.commit().await?;
    Ok(id)
}

/// Allows the one-time queue importer to commit its receipt with the event.
pub async fn persist_in(
    connection: &mut SqliteConnection,
    sink_id: i64,
    event: &EventInput,
) -> Result<Option<i64>, sqlx::Error> {
    let now = Utc::now().naive_utc();
    if event
        .idempotency_key
        .as_ref()
        .is_some_and(|key| key.chars().count() > 255)
    {
        return Err(sqlx::Error::Protocol(
            "idempotency key exceeds the existing model limit".into(),
        ));
    }
    let id: Option<i64> = sqlx::query_scalar(
        "INSERT INTO events (sink_id, event_type, emoji, text, properties, occurred_at, \
         idempotency_key, sdk_name, sdk_version, search_text, created_at, updated_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$11) \
         ON CONFLICT (sink_id, idempotency_key) DO NOTHING RETURNING id",
    )
    .bind(sink_id)
    .bind(&event.event_type)
    .bind(&event.emoji)
    .bind(&event.text)
    .bind(&event.properties)
    .bind(event.occurred_at)
    .bind(&event.idempotency_key)
    .bind(&event.sdk_name)
    .bind(&event.sdk_version)
    .bind(event.search_text())
    .bind(now)
    .fetch_optional(&mut *connection)
    .await?;
    if let Some(id) = id {
        sqlx::query(
            "UPDATE sink_memberships AS m SET has_unread_events=true, \
             unread_count=m.unread_count+1, updated_at=$2 FROM users AS u \
             WHERE m.sink_id=$1 AND u.id=m.user_id AND NOT \
             (u.current_sink_id IS NOT DISTINCT FROM $1 AND m.last_viewed_at IS NOT NULL \
              AND m.last_viewed_at > $3)",
        )
        .bind(sink_id)
        .bind(now)
        .bind(now - chrono::Duration::seconds(45))
        .execute(&mut *connection)
        .await?;
        sqlx::query("INSERT INTO telesink_changes(payload) VALUES ($1)")
            .bind(id.to_string())
            .execute(&mut *connection)
            .await?;
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_sdk_defaults_and_required_text() {
        let now = Utc::now().naive_utc();
        for value in [
            json!({}),
            json!({"text":""}),
            json!({"text":" \u{00a0}\n"}),
            json!({"text":false}),
        ] {
            assert_eq!(
                EventInput::from_params(&value, now).unwrap_err(),
                ["Text can't be blank"]
            );
        }
        let event = EventInput::from_params(&json!({"text":"Hello"}), now).unwrap();
        assert_eq!(
            (&*event.event_type, &*event.emoji, event.occurred_at),
            ("event", "📌", now)
        );
        assert_eq!(event.properties, json!({}));
        assert!(event.idempotency_key.is_none());
    }

    #[test]
    fn timestamp_offsets_and_invalid_fallback() {
        let now = Utc::now().naive_utc();
        let event = EventInput::from_params(
            &json!({"text":"x","occurred_at":"2026-03-20T17:55:00.480+08:00"}),
            now,
        )
        .unwrap();
        assert_eq!(event.occurred_at.to_string(), "2026-03-20 09:55:00.480");
        let invalid =
            EventInput::from_params(&json!({"text":"x","occurred_at":"not-a-time"}), now).unwrap();
        assert_eq!(invalid.occurred_at, now);
    }

    #[test]
    fn search_includes_properties_and_nested_json_without_null_literal() {
        let event = EventInput::from_params(&json!({"event":"signup","text":"Hello\nworld", "properties":{"name":"Jane Doe","flag":false,"absent":null,"nested":{"count":2}}}), Utc::now().naive_utc()).unwrap();
        assert_eq!(
            event.search_text(),
            "signup Hello world name Jane Doe flag false absent {} nested {\"count\":2}"
        );
    }
}
