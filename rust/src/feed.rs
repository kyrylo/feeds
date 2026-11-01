use chrono::{Datelike, NaiveDate, NaiveDateTime, TimeZone};
use chrono_tz::Tz;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use std::sync::OnceLock;

use crate::params;

const NUMERIC_PATTERN: &str = r"[+-]?([0-9]+([.][0-9]*)?|[.][0-9]+)([eE][+-]?[0-9]+)?";

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, sqlx::FromRow)]
pub struct Filters {
    pub event_type: Option<String>,
    pub event_date: Option<NaiveDate>,
    pub search_query: Option<String>,
    pub property_key: Option<String>,
    pub property_op: Option<String>,
    pub property_value: Option<String>,
}

fn trimmed(value: Option<&Value>) -> Option<String> {
    params::scalar(value)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

pub fn numeric(value: &str) -> Option<String> {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    let value = value.trim();
    PATTERN
        .get_or_init(|| Regex::new(&format!(r"\A{NUMERIC_PATTERN}\z")).unwrap())
        .is_match(value)
        .then(|| value.to_owned())
}

impl Filters {
    // The current app supports text search only. Keep the richer parser below
    // for the legacy reference UI, without applying hidden conditions to Feeds.
    pub fn search_only(raw: &Value) -> Self {
        Self {
            search_query: trimmed(raw.get("search_query")).or_else(|| trimmed(raw.get("q"))),
            ..Self::default()
        }
    }

    pub fn from_params(raw: &Value) -> Self {
        let mut filters = Self {
            event_type: trimmed(raw.get("event_type")),
            event_date: trimmed(raw.get("event_date"))
                .or_else(|| trimmed(raw.get("date")))
                .and_then(|date| NaiveDate::parse_from_str(&date, "%Y-%m-%d").ok()),
            search_query: trimmed(raw.get("search_query")).or_else(|| trimmed(raw.get("q"))),
            property_key: trimmed(raw.get("property_key")),
            property_op: trimmed(raw.get("property_op")).or_else(|| Some("eq".into())),
            property_value: raw
                .get("property_value")
                .map(|v| params::scalar(Some(v)).unwrap_or_default()),
        };
        let valid = filters.property_key.is_some()
            && match filters.property_op.as_deref() {
                Some("exists") => {
                    filters.property_value = None;
                    true
                }
                Some("eq") => filters.property_value.is_some(),
                Some("lt" | "gt") => {
                    filters.property_value = filters.property_value.as_deref().and_then(numeric);
                    filters.property_value.is_some()
                }
                _ => false,
            };
        if !valid {
            filters.property_key = None;
            filters.property_op = None;
            filters.property_value = None;
        }
        filters
    }

    pub fn can_mark_viewed(&self) -> bool {
        self.event_type.is_none()
            && self.event_date.is_none()
            && self.search_query.is_none()
            && self.property_key.is_none()
    }

    pub fn parameters(&self, month: Option<NaiveDate>) -> Vec<(String, String)> {
        let mut params = Vec::new();
        for (key, value) in [
            ("event_type", self.event_type.clone()),
            ("date", self.event_date.map(|d| d.to_string())),
            ("q", self.search_query.clone()),
            ("property_key", self.property_key.clone()),
            ("property_op", self.property_op.clone()),
            ("property_value", self.property_value.clone()),
            (
                "month",
                month.and_then(|d| d.with_day(1)).map(|d| d.to_string()),
            ),
        ] {
            if let Some(value) = value {
                params.push((key.into(), value));
            }
        }
        params
    }

    pub fn url(&self, path: &str, month: Option<NaiveDate>) -> String {
        let params = self.parameters(month);
        if params.is_empty() {
            path.into()
        } else {
            format!("{path}?{}", serde_urlencoded::to_string(params).unwrap())
        }
    }

    pub(crate) fn apply(&self, query: &mut QueryBuilder<Sqlite>, zone: Tz) {
        if let Some(event_type) = &self.event_type {
            query
                .push(" AND e.event_type=")
                .push_bind(event_type.clone());
        }
        if let Some(date) = self.event_date {
            query
                .push(" AND e.occurred_at >= ")
                .push_bind(local_midnight(date, zone))
                .push(" AND e.occurred_at < ")
                .push_bind(local_midnight(date.succ_opt().unwrap_or(date), zone));
        }
        if let Some(search) = &self.search_query {
            query.push(" AND telesink_contains(COALESCE(e.search_text,e.event_type || ' ' || e.text || ' ' || e.properties),")
                .push_bind(search.clone()).push(")");
        }
        if let Some(key) = &self.property_key {
            query
                .push(" AND telesink_property(e.properties,")
                .push_bind(key.clone())
                .push(",")
                .push_bind(self.property_op.clone().unwrap_or_default())
                .push(",")
                .push_bind(self.property_value.clone().unwrap_or_default())
                .push(")");
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Event {
    pub id: i64,
    pub sink_id: i64,
    pub event_type: String,
    pub emoji: String,
    pub text: String,
    pub properties: Value,
    pub search_text: String,
    pub occurred_at: NaiveDateTime,
    pub created_at: NaiveDateTime,
}

pub(crate) fn scope(user_id: i64, sink_id: Option<i64>, selection: &str) -> QueryBuilder<Sqlite> {
    scope_from(user_id, sink_id, selection, "events e")
}

pub(crate) fn chronological_scope(
    user_id: i64,
    sink_id: Option<i64>,
    selection: &str,
) -> QueryBuilder<Sqlite> {
    // The snapshot's id upper bound otherwise favors the (sink_id, rowid)
    // index and sorts an entire feed for LIMIT 51. Walk the existing time
    // index instead, including the ID tie-breaker used by pagination.
    let table = if sink_id.is_some() {
        "events e INDEXED BY index_events_on_sink_id_and_occurred_at"
    } else {
        "events e INDEXED BY index_events_on_occurred_at_and_id"
    };
    scope_from(user_id, sink_id, selection, table)
}

fn scope_from(
    user_id: i64,
    sink_id: Option<i64>,
    selection: &str,
    table: &str,
) -> QueryBuilder<Sqlite> {
    // Source identity remains intact. This authorization boundary also supports
    // a future combined feed without migrating events or weakening permissions.
    let mut query = QueryBuilder::new(format!(
        "SELECT {selection} FROM {table} JOIN sinks s ON s.id=e.sink_id JOIN sink_memberships m ON m.sink_id=s.id JOIN users u ON u.id=m.user_id WHERE m.user_id="
    ));
    query
        .push_bind(user_id)
        .push(" AND s.account_id=u.account_id");
    if let Some(sink_id) = sink_id {
        query.push(" AND e.sink_id=").push_bind(sink_id);
    }
    query
}

pub async fn batch(
    pool: &SqlitePool,
    user_id: i64,
    sink_id: Option<i64>,
    filters: &Filters,
    zone: Tz,
    before_id: Option<i64>,
    limit: i64,
) -> Result<Vec<Event>, sqlx::Error> {
    let anchor = if let Some(before_id) = before_id {
        let mut query = scope(user_id, sink_id, "e.*");
        filters.apply(&mut query, zone);
        query.push(" AND e.id=").push_bind(before_id);
        let Some(event) = query.build_query_as::<Event>().fetch_optional(pool).await? else {
            return Ok(vec![]);
        };
        Some((event.occurred_at, event.id))
    } else {
        None
    };
    let mut query = scope(user_id, sink_id, "e.*");
    filters.apply(&mut query, zone);
    if let Some((occurred_at, id)) = anchor {
        query
            .push(" AND (e.occurred_at,e.id) < (")
            .push_bind(occurred_at)
            .push(",")
            .push_bind(id)
            .push(")");
    }
    query
        .push(" ORDER BY e.occurred_at DESC,e.id DESC LIMIT ")
        .push_bind(limit.clamp(1, 60));
    let mut events = query.build_query_as::<Event>().fetch_all(pool).await?;
    events.reverse();
    Ok(events)
}

pub fn time_zone(name: Option<&str>) -> Tz {
    static ALIASES: OnceLock<std::collections::HashMap<String, String>> = OnceLock::new();
    name.filter(|s| s.len() <= 128)
        .and_then(|name| {
            name.parse().ok().or_else(|| {
                ALIASES
                    .get_or_init(|| {
                        serde_json::from_str(include_str!("../data/time-zones.json")).unwrap()
                    })
                    .get(name)
                    .and_then(|name| name.parse().ok())
            })
        })
        .unwrap_or(chrono_tz::UTC)
}

pub fn date(value: Option<&Value>) -> Option<NaiveDate> {
    trimmed(value).and_then(|date| NaiveDate::parse_from_str(&date, "%Y-%m-%d").ok())
}

pub async fn find(pool: &SqlitePool, user: i64, id: i64) -> Result<Option<Event>, sqlx::Error> {
    let mut query = scope(user, None, "e.*");
    query.push(" AND e.id=").push_bind(id);
    query.build_query_as().fetch_optional(pool).await
}

pub async fn matching_event(
    pool: &SqlitePool,
    user: i64,
    sink: i64,
    id: i64,
    filters: &Filters,
    zone: Tz,
) -> Result<Option<Event>, sqlx::Error> {
    let mut query = scope(user, Some(sink), "e.*");
    filters.apply(&mut query, zone);
    query.push(" AND e.id=").push_bind(id);
    query.build_query_as().fetch_optional(pool).await
}

pub async fn after(
    pool: &SqlitePool,
    user: i64,
    sink: i64,
    cursor: i64,
    filters: &Filters,
    zone: Tz,
) -> Result<Vec<Event>, sqlx::Error> {
    let mut query = scope(user, Some(sink), "e.*");
    filters.apply(&mut query, zone);
    query
        .push(" AND e.id > ")
        .push_bind(cursor)
        .push(" ORDER BY e.occurred_at,e.id LIMIT 61");
    query.build_query_as().fetch_all(pool).await
}

pub async fn type_counts(
    pool: &SqlitePool,
    user: i64,
    sink: i64,
    filters: &Filters,
    zone: Tz,
) -> Result<Vec<(String, i64)>, sqlx::Error> {
    let mut filters = filters.clone();
    filters.event_type = None;
    let mut query = scope(user, Some(sink), "e.event_type,COUNT(*)");
    filters.apply(&mut query, zone);
    query.push(" GROUP BY e.event_type ORDER BY e.event_type");
    query.build_query_as().fetch_all(pool).await
}

pub async fn newest(
    pool: &SqlitePool,
    user: i64,
    sink: i64,
    filters: &Filters,
    zone: Tz,
) -> Result<Option<NaiveDateTime>, sqlx::Error> {
    let mut filters = filters.clone();
    filters.event_date = None;
    let mut query = scope(user, Some(sink), "MAX(e.occurred_at)");
    filters.apply(&mut query, zone);
    query.build_query_scalar().fetch_one(pool).await
}

pub async fn calendar(
    pool: &SqlitePool,
    user: i64,
    sink: i64,
    filters: &Filters,
    zone: Tz,
    month: NaiveDate,
) -> Result<Vec<(NaiveDate, i64)>, sqlx::Error> {
    let mut filters = filters.clone();
    filters.event_date = None;
    let date_sql = format!("telesink_local_date(e.occurred_at,'{}')", zone.name());
    let mut query = scope(user, Some(sink), &format!("{date_sql},COUNT(*)"));
    filters.apply(&mut query, zone);
    let next = month
        .checked_add_months(chrono::Months::new(1))
        .unwrap_or(month);
    query
        .push(" AND e.occurred_at >= ")
        .push_bind(local_midnight(month, zone))
        .push(" AND e.occurred_at < ")
        .push_bind(local_midnight(next, zone));
    query.push(format!(" GROUP BY {date_sql} ORDER BY {date_sql}"));
    query.build_query_as().fetch_all(pool).await
}

/// PostgreSQL chooses the later instant for an ambiguous local time and the
/// pre-transition offset for a skipped one. Preserve those calendar boundaries.
fn local_midnight(date: NaiveDate, zone: Tz) -> NaiveDateTime {
    use chrono::{LocalResult, Offset};
    let local = date.and_hms_opt(0, 0, 0).unwrap();
    match zone.from_local_datetime(&local) {
        LocalResult::Single(time) => time.naive_utc(),
        LocalResult::Ambiguous(a, b) => a.max(b).naive_utc(),
        LocalResult::None => {
            for hour in 1..=48 {
                if let Some(before) = local.checked_sub_signed(chrono::Duration::hours(hour))
                    && let Some(time) = zone.from_local_datetime(&before).latest()
                {
                    return local
                        - chrono::Duration::seconds(time.offset().fix().local_minus_utc() as i64);
                }
            }
            local
        }
    }
}

pub fn local_date(event: &Event, zone: Tz) -> NaiveDate {
    chrono::Utc
        .from_utc_datetime(&event.occurred_at)
        .with_timezone(&zone)
        .date_naive()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn snapshot_pages_use_time_indexes_without_sorting_the_feed() {
        use sqlx::{Connection, Row};
        let mut db = sqlx::SqliteConnection::connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(crate::schema::DDL)
            .execute(&mut db)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!("../schema/activity.sql"))
            .execute(&mut db)
            .await
            .unwrap();
        for source in [Some(1), None] {
            let mut query = chronological_scope(1, source, "e.*");
            query
                .push(" AND e.id<=")
                .push_bind(1_000_000_i64)
                .push(" ORDER BY e.occurred_at DESC,e.id DESC LIMIT 51");
            let mut explain = sqlx::query(sqlx::AssertSqlSafe(format!(
                "EXPLAIN QUERY PLAN {}",
                query.sql().as_str()
            )))
            .bind(1_i64);
            if let Some(id) = source {
                explain = explain.bind(id);
            }
            let plan: Vec<String> = explain
                .bind(1_000_000_i64)
                .fetch_all(&mut db)
                .await
                .unwrap()
                .into_iter()
                .map(|row| row.get("detail"))
                .collect();
            assert!(
                !plan.iter().any(|step| step.contains("TEMP B-TREE")),
                "Snapshot pages must stop after the requested rows: {plan:?}"
            );
            let index = if source.is_some() {
                "index_events_on_sink_id_and_occurred_at"
            } else {
                "index_events_on_occurred_at_and_id"
            };
            assert!(plan.iter().any(|step| step.contains(index)), "{plan:?}");
        }
    }

    #[test]
    fn empty_values_remain_distinct_from_absent_filters() {
        let empty = Filters::from_params(&json!({"property_key":"email","property_value":""}));
        assert_eq!(empty.property_value, Some(String::new()));
        assert!(!empty.can_mark_viewed());
        assert!(Filters::from_params(&json!({"property_key":"email"})).can_mark_viewed());
        let exists = Filters::from_params(
            &json!({"property_key":"email","property_op":"exists","property_value":"irrelevant"}),
        );
        assert_eq!(exists.property_value, None);
        assert_eq!(exists.property_op.as_deref(), Some("exists"));
    }

    #[test]
    fn malformed_filters_cannot_become_sql() {
        for value in ["1; DROP TABLE events", "NaN", "Infinity", "12x", "", "1\n2"] {
            assert!(
                Filters::from_params(
                    &json!({"property_key":"amount","property_op":"gt","property_value":value})
                )
                .can_mark_viewed()
            );
        }
        for value in ["1", "-0.5", ".2", "1.", "+1.2e-3"] {
            assert!(numeric(value).is_some());
        }
    }
}
