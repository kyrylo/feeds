mod common;
use chrono::{NaiveDate, Utc};
use common::Harness;
use feeds::{
    events::{EventInput, persist},
    feed::{self, Filters},
    feed_view,
};
use serde_json::{Value, json};

#[tokio::test]
async fn feed_queries_counts_and_calendars_match_independent_rails_oracle() {
    let h = Harness::new().await;
    let sink = h.sink("oracle").await;
    sqlx::query("INSERT INTO sink_memberships (user_id,sink_id,created_at,updated_at) VALUES ($1,$2,now(),now())").bind(h.user).bind(sink).execute(&h.pool).await.unwrap();
    let reference: Value =
        serde_json::from_str(include_str!("fixtures/feed-contracts.json")).unwrap();
    let mut ids = Vec::new();
    for raw in reference["inputs"].as_array().unwrap() {
        let input=EventInput::from_params(&json!({"event":raw["event_type"],"text":raw["text"],"properties":raw["properties"],"occurred_at":raw["occurred_at"]}),Utc::now().naive_utc()).unwrap();
        ids.push(persist(&h.pool, sink, &input).await.unwrap().unwrap());
    }
    for (index, case) in reference["queries"].as_array().unwrap().iter().enumerate() {
        let filters = Filters::from_params(&case["params"]);
        let zone = feed::time_zone(case["zone"].as_str());
        assert_eq!(
            serde_json::to_value(&filters).unwrap(),
            case["normalized"],
            "normalization {index}: {}",
            case["params"]
        );
        let anchor = case["before_index"].as_u64().map(|i| ids[i as usize]);
        let events = feed::batch(&h.pool, h.user, Some(sink), &filters, zone, anchor, 60)
            .await
            .unwrap();
        let indexes: Vec<_> = events
            .iter()
            .map(|e| ids.iter().position(|id| *id == e.id).unwrap())
            .collect();
        assert_eq!(
            json!(indexes),
            case["indexes"],
            "query {index}: {}",
            case["params"]
        );
        if case.get("counts").is_some() {
            assert_eq!(
                json!(
                    feed::type_counts(&h.pool, h.user, sink, &filters, zone)
                        .await
                        .unwrap()
                ),
                case["counts"],
                "counts {index}"
            );
            assert_eq!(
                json!(
                    feed::calendar(
                        &h.pool,
                        h.user,
                        sink,
                        &filters,
                        zone,
                        NaiveDate::from_ymd_opt(2026, 3, 1).unwrap()
                    )
                    .await
                    .unwrap()
                ),
                case["dates"],
                "calendar {index}"
            );
            let newest = feed::newest(&h.pool, h.user, sink, &filters, zone)
                .await
                .unwrap()
                .map(|d| {
                    d.and_utc()
                        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
                });
            assert_eq!(json!(newest), case["newest"], "newest {index}");
        }
    }
    for (index, case) in reference["column_queries"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let column = feeds::columns::Column {
            id: 0,
            sink_id: sink,
            name: "reference".into(),
            config: Some(case["config"].clone()),
        };
        let before = case["before_index"].as_u64().map(|i| ids[i as usize]);
        let events = feeds::columns::recent(&h.pool, h.user, &column, before)
            .await
            .unwrap();
        let indexes: Vec<_> = events
            .iter()
            .map(|e| ids.iter().position(|id| *id == e.id).unwrap())
            .collect();
        assert_eq!(
            json!(indexes),
            case["indexes"],
            "column query {index}: {}",
            case["config"]
        );
    }
    h.finish().await;
}

#[test]
fn event_labels_and_colors_match_original_helpers() {
    let reference: Value =
        serde_json::from_str(include_str!("fixtures/feed-contracts.json")).unwrap();
    for case in reference["labels"].as_array().unwrap() {
        let input = case["input"].as_str().unwrap();
        assert_eq!(feed_view::label(input), case["label"]);
        assert_eq!(json!(feed_view::color(input)), case["color"], "{input}");
    }
}

#[tokio::test]
async fn feed_and_anchor_queries_cannot_cross_membership_or_account_boundaries() {
    let h = Harness::new().await;
    let outside = Harness::new().await;
    let source = h.sink("allowed").await;
    let hidden = h.sink("hidden").await;
    let foreign = outside.sink("foreign").await;
    for sink in [source, foreign] {
        sqlx::query("INSERT INTO sink_memberships (user_id,sink_id,created_at,updated_at) VALUES ($1,$2,now(),now())").bind(h.user).bind(sink).execute(&h.pool).await.unwrap();
    }
    let input = EventInput::from_params(&json!({"text":"test"}), Utc::now().naive_utc()).unwrap();
    let allowed = persist(&h.pool, source, &input).await.unwrap().unwrap();
    let hidden_id = persist(&h.pool, hidden, &input).await.unwrap().unwrap();
    let foreign_id = persist(&h.pool, foreign, &input).await.unwrap().unwrap();
    let all = feed::batch(
        &h.pool,
        h.user,
        None,
        &Filters::default(),
        chrono_tz::UTC,
        None,
        60,
    )
    .await
    .unwrap();
    assert_eq!(all.iter().map(|e| e.id).collect::<Vec<_>>(), vec![allowed]);
    for id in [hidden_id, foreign_id] {
        assert!(feed::find(&h.pool, h.user, id).await.unwrap().is_none());
        assert!(
            feed::batch(
                &h.pool,
                h.user,
                Some(source),
                &Filters::default(),
                chrono_tz::UTC,
                Some(id),
                60
            )
            .await
            .unwrap()
            .is_empty()
        );
    }
    h.finish().await;
    outside.finish().await;
}
