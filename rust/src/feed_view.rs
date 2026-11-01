//! Presentation values shared by full feeds, pagination, details and live rows.
use crate::{
    feed::{self, Event, Filters},
    params,
};
use chrono::{Datelike, NaiveDate};
use chrono_tz::Tz;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::OnceLock};
use unicode_normalization::UnicodeNormalization;

pub fn truncate(value: &str, limit: usize, ellipsis: bool) -> String {
    if value.chars().count() <= limit {
        return value.into();
    }
    let mut output: String = value
        .chars()
        .take(limit.saturating_sub(usize::from(ellipsis)))
        .collect();
    if ellipsis {
        output.push('…');
    }
    output
}

pub fn label(value: &str) -> String {
    static LABELS: OnceLock<HashMap<String, String>> = OnceLock::new();
    let value = value.trim();
    if !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return value.into();
    }
    if let Some(label) = LABELS
        .get_or_init(|| serde_json::from_str(include_str!("../data/event-labels.json")).unwrap())
        .get(value)
    {
        return label.clone();
    }
    let normalized = value
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let mut chars = normalized.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

fn color_hash(value: &str) -> u32 {
    value
        .nfc()
        .collect::<String>()
        .to_lowercase()
        .chars()
        .fold(2_166_136_261u32, |h, c| {
            (h ^ (c as u32)).wrapping_mul(16_777_619)
        })
}

pub fn terminal_color(value: &str) -> u32 {
    color_hash(value) % 6
}

pub fn color(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        return None;
    }
    const COLORS: [&str; 31] = [
        "red", "orange", "amber", "yellow", "lime", "green", "emerald", "teal", "sky", "blue",
        "indigo", "violet", "purple", "fuchsia", "pink", "rose", "red", "orange", "amber", "lime",
        "green", "emerald", "teal", "sky", "blue", "indigo", "violet", "purple", "fuchsia", "pink",
        "rose",
    ];
    let hash = color_hash(value);
    let index = hash as usize % COLORS.len();
    let (light, dark) = if index < 16 { (700, 300) } else { (800, 400) };
    Some(format!(
        "--event-type-color-light: var(--color-{}-{light}); --event-type-color-dark: var(--color-{}-{dark})",
        COLORS[index], COLORS[index]
    ))
}

pub fn dom_key(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn property_label(filters: &Filters) -> Option<String> {
    let key = filters.property_key.as_ref()?;
    match filters.property_op.as_deref() {
        Some("exists") => Some(format!("{key} exists")),
        Some(op @ ("lt" | "gt")) => Some(format!(
            "{key} {} {}",
            if op == "lt" { "<" } else { ">" },
            filters.property_value.as_ref()?
        )),
        _ => Some(format!(
            "{key}={}",
            value_label(filters.property_value.as_ref()?)
        )),
    }
}

fn value_label(value: &str) -> String {
    if value.trim().is_empty() {
        serde_json::to_string(value).unwrap()
    } else {
        value.into()
    }
}

pub fn summary(filters: &Filters) -> String {
    let mut parts = Vec::new();
    if let Some(value) = &filters.event_type {
        parts.push(label(value));
    }
    if let Some(value) = filters.event_date {
        parts.push(value.to_string());
    }
    if let Some(value) = &filters.search_query {
        parts.push(format!("\"{}\"", truncate(value, 48, true)));
    }
    if let Some(value) = property_label(filters) {
        parts.push(value);
    }
    parts.join(" ")
}

pub fn status_links(
    filters: &Filters,
    path: &str,
    month: Option<NaiveDate>,
    today: NaiveDate,
) -> Vec<Value> {
    let mut links = Vec::new();
    if let Some(value) = &filters.event_type {
        let mut next = filters.clone();
        next.event_type = None;
        links.push(json!({"label":label(value),"url":next.url(path,month),"title":"remove event filter","style":color(value),"colored":true}));
    }
    if let Some(value) = filters.event_date {
        let mut next = filters.clone();
        next.event_date = None;
        links.push(json!({"label":if value==today {"today".into()} else {value.to_string()},"url":next.url(path,month),"title":"remove date filter"}));
    }
    if let Some(value) = &filters.search_query {
        let mut next = filters.clone();
        next.search_query = None;
        links.push(json!({"label":format!("\"{}\"",truncate(value,48,true)),"url":next.url(path,month),"title":"remove search"}));
    }
    if let Some(value) = property_label(filters) {
        let mut next = filters.clone();
        next.property_key = None;
        next.property_op = None;
        next.property_value = None;
        links.push(
            json!({"label":value,"url":next.url(path,month),"title":"remove property filter"}),
        );
    }
    links
}

pub fn row(event: &Event, filters: &Filters, month: Option<NaiveDate>) -> Value {
    let mut context = serde_json::to_value(event).unwrap();
    context["occurred_at"] = json!(
        event
            .occurred_at
            .and_utc()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    );
    context["label"] = json!(label(&event.event_type));
    context["color"] = json!(color(&event.event_type));
    context["detail_url"] = json!(filters.url(&format!("/events/{}", event.id), month));
    let mut numeric = serde_json::Map::new();
    let mut preview = Vec::new();
    if let Some(properties) = event.properties.as_object() {
        for (key, value) in properties {
            if let Some(value) = params::scalar(Some(value)).and_then(|v| feed::numeric(&v)) {
                numeric.insert(key.clone(), json!(value));
            }
            if preview.len() < 3 {
                let value = match value {
                    Value::String(value) => {
                        if value.trim().is_empty() {
                            value_label(value)
                        } else {
                            value.split_whitespace().collect::<Vec<_>>().join(" ")
                        }
                    }
                    _ => value.to_string(),
                };
                preview.push(json!({"key":key,"value":value}));
            }
        }
    }
    context["preview"] = json!(preview);
    context["numeric_properties"] = json!(Value::Object(numeric).to_string());
    context
}

pub fn details(event: &Event, filters: &Filters, month: Option<NaiveDate>, zone: Tz) -> Value {
    let mut ctx = row(event, filters, month);
    ctx["local_time"] = json!(
        event
            .occurred_at
            .and_utc()
            .with_timezone(&zone)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    );
    ctx["utc_time"] = json!(event.occurred_at.format("%Y-%m-%d %H:%M:%S").to_string());
    let path = format!("/sinks/{}", event.sink_id);
    let mut properties = Vec::new();
    if let Some(map) = event.properties.as_object() {
        for (key, value) in map {
            let full = params::scalar(Some(value)).unwrap_or_else(|| value.to_string());
            let scalar = params::scalar(Some(value));
            let numeric = scalar.as_deref().and_then(feed::numeric);
            let mut next = filters.clone();
            next.property_key = Some(key.clone());
            next.property_op = Some("exists".into());
            next.property_value = None;
            let exists_url = next.url(&path, month);
            next.property_op = Some("eq".into());
            next.property_value = scalar.clone();
            let eq_url = next.url(&path, month);
            next.property_value = numeric.clone();
            next.property_op = Some("lt".into());
            let lt_url = next.url(&path, month);
            next.property_op = Some("gt".into());
            let gt_url = next.url(&path, month);
            let current = filters.property_key.as_ref() == Some(key)
                && match filters.property_op.as_deref() {
                    Some("exists") => true,
                    Some(op @ ("lt" | "gt")) => numeric
                        .as_ref()
                        .and_then(|v| v.parse::<f64>().ok())
                        .zip(
                            filters
                                .property_value
                                .as_ref()
                                .and_then(|v| v.parse::<f64>().ok()),
                        )
                        .is_some_and(|(v, n)| if op == "lt" { v < n } else { v > n }),
                    _ => scalar.is_some() && scalar == filters.property_value,
                };
            let teaser = truncate(&full, 120, true);
            let action_label = if teaser.trim().is_empty() {
                scalar
                    .as_deref()
                    .map(value_label)
                    .unwrap_or_else(|| "empty".into())
            } else {
                teaser.clone()
            };
            properties.push(json!({"key":key,"full":full,"teaser":teaser,"action_label":action_label,"long":full.trim().chars().count()>120,
            "filterable":scalar.is_some(),"scalar":scalar,"numeric":numeric,"current":current,"exists_url":exists_url,"eq_url":eq_url,"lt_url":lt_url,"gt_url":gt_url}));
        }
    }
    ctx["property_rows"] = json!(properties);
    ctx
}

pub fn calendar(
    month: NaiveDate,
    counts: &[(NaiveDate, i64)],
    filters: &Filters,
    path: &str,
    requested: Option<NaiveDate>,
) -> Value {
    let previous = month
        .checked_sub_months(chrono::Months::new(1))
        .unwrap_or(month);
    let next = month
        .checked_add_months(chrono::Months::new(1))
        .unwrap_or(month);
    let mut no_date = filters.clone();
    no_date.event_date = None;
    let mut cells = vec![Value::Null; month.weekday().num_days_from_sunday() as usize];
    let mut date = month;
    while date < next {
        let count = counts
            .iter()
            .find(|(d, _)| *d == date)
            .map(|(_, c)| *c)
            .unwrap_or(0);
        let mut day_filter = filters.clone();
        day_filter.event_date = Some(date);
        cells.push(json!({"day":date.day(),"date":date.to_string(),"count":count,"selected":filters.event_date==Some(date),"url":day_filter.url(path,requested)}));
        let Some(tomorrow) = date.succ_opt() else {
            break;
        };
        date = tomorrow;
    }
    json!({"month":month.to_string(),"label":month.format("%B %Y").to_string(),"previous":no_date.url(path,Some(previous)),"next":no_date.url(path,Some(next)),"cells":cells})
}
