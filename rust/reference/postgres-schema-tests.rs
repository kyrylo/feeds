use chrono::Utc;
use serde_json::{Value, json};
use sqlx::{Connection, Executor, PgConnection, PgPool};
use std::sync::atomic::{AtomicU64, Ordering};
use feeds::{
    events::{EventInput, persist},
    schema,
};

struct Database {
    name: String,
    url: String,
    pool: PgPool,
}
impl Database {
    async fn new() -> Self {
        let url = std::env::var("TELESINK_TEST_DATABASE_URL").unwrap();
        assert_eq!(
            url,
            "postgresql://telesink@127.0.0.1:55438/telesink_rust_test"
        );
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = format!(
            "telesink_rust_schema_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let url = format!("postgresql://telesink@127.0.0.1:55438/{name}");
        let pool = schema::connect(&url, 3, true).await.unwrap();
        Self { name, url, pool }
    }
    async fn load(&self, sql: &str) {
        let sql = sql
            .lines()
            .filter(|l| !l.starts_with("SET ") && !l.starts_with("SELECT pg_catalog.set_config("))
            .collect::<Vec<_>>()
            .join("\n");
        self.pool.execute(sql.as_str()).await.unwrap();
    }
    async fn finish(self) {
        self.pool.close().await;
        let mut admin = PgConnection::connect("postgresql://telesink@127.0.0.1:55438/postgres")
            .await
            .unwrap();
        assert!(
            self.name
                .starts_with(&format!("telesink_rust_schema_{}_", std::process::id()))
        );
        admin
            .execute(format!("DROP DATABASE \"{}\"", self.name).as_str())
            .await
            .unwrap();
        admin.close().await.unwrap();
    }
}

async fn snapshot(pool: &PgPool) -> Value {
    let mut result = serde_json::Map::new();
    for table in [
        "accounts",
        "columns",
        "events",
        "folders",
        "saved_views",
        "sessions",
        "sink_memberships",
        "sinks",
        "users",
    ] {
        let value: Value = sqlx::query_scalar(&format!(
            "SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY id),'[]'::jsonb) FROM {table} t"
        ))
        .fetch_one(pool)
        .await
        .unwrap();
        result.insert(table.into(), value);
    }
    for table in ["ar_internal_metadata", "schema_migrations"] {
        let value: Value = sqlx::query_scalar(&format!("SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]'::jsonb) FROM {table} t"))
            .fetch_one(pool).await.unwrap();
        result.insert(table.into(), value);
    }
    let sequences: Value = sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(s) ORDER BY sequencename) FROM pg_sequences s WHERE schemaname='public'")
        .fetch_one(pool).await.unwrap();
    result.insert("sequences".into(), sequences);
    result.into()
}

#[tokio::test]
async fn bootstrap_is_atomic_concurrent_and_matches_the_original_rails_catalog() {
    let db = Database::new().await;
    let (a, b) = tokio::join!(
        schema::prepare(&db.pool, "test"),
        schema::prepare(&db.pool, "test")
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_ne!(a.bootstrapped, b.bootstrapped);
    assert_eq!(a.applied.len() + b.applied.len(), 33);
    schema::check(&db.pool).await.unwrap();
    let before = snapshot(&db.pool).await;
    assert!(
        schema::prepare(&db.pool, "production")
            .await
            .unwrap()
            .applied
            .is_empty()
    );
    assert_eq!(snapshot(&db.pool).await, before);
    // The command also works without a session secret or Ruby executable.
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_feeds"))
        .args(["db", "check"])
        .env("DATABASE_URL", &db.url)
        .env_remove("SECRET_KEY_BASE")
        .env("PATH", "/nonexistent")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        String::from_utf8(result.stdout)
            .unwrap()
            .contains(schema::CURRENT_VERSION)
    );
    db.finish().await;
}

#[tokio::test]
async fn early_rails_database_upgrades_keep_ids_tokens_credentials_sessions_and_payloads() {
    let db = Database::new().await;
    db.load(include_str!("fixtures/schema-20260224101644.sql"))
        .await;
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/rails-contracts.json")).unwrap();
    let digest = fixture["session"]["password_digest"].as_str().unwrap();
    let user:i64=sqlx::query_scalar("INSERT INTO users (email_address,password_digest,created_at,updated_at) VALUES ('legacy@example.test',$1,now(),now()) RETURNING id")
        .bind(digest).fetch_one(&db.pool).await.unwrap();
    let sink:i64=sqlx::query_scalar("INSERT INTO sinks (name,token,created_at,updated_at) VALUES ('Existing','preserved-ingest-token',now(),now()) RETURNING id")
        .fetch_one(&db.pool).await.unwrap();
    db.pool.execute(format!("INSERT INTO sink_memberships (user_id,sink_id,created_at,updated_at) VALUES ({user},{sink},now(),now()); INSERT INTO sessions (user_id,ip_address,created_at,updated_at) VALUES ({user},'127.0.0.1',now(),now()); INSERT INTO columns (sink_id,name,position,config,created_at,updated_at) VALUES ({sink},'Legacy column',7,'{{\"custom\":true}}',now(),now())").as_str()).await.unwrap();
    let properties = json!({"nested":{"array":[1,null,true]},"amount":42});
    let event:i64=sqlx::query_scalar("INSERT INTO events (sink_id,event_type,text,payload,occurred_at,created_at,updated_at) VALUES ($1,'payment','legacy event',$2,'2026-02-25 01:02:03',now(),now()) RETURNING id")
        .bind(sink).bind(&properties).fetch_one(&db.pool).await.unwrap();
    let report = schema::prepare(&db.pool, "test").await.unwrap();
    assert_eq!(report.applied.len(), 26);
    schema::check(&db.pool).await.unwrap();
    let row: (String, String, String, i32, i64) = sqlx::query_as(
        "SELECT email_address,password_digest,nickname,role,account_id FROM users WHERE id=$1",
    )
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        (&*row.0, &*row.1, &*row.2, row.3),
        ("legacy@example.test", digest, "legacy", 1)
    );
    let code: String = sqlx::query_scalar("SELECT join_code FROM accounts WHERE id=$1")
        .bind(row.4)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(code.split('-').map(str::len).collect::<Vec<_>>(), [4, 4, 4]);
    let source: (String, i64) = sqlx::query_as("SELECT token,account_id FROM sinks WHERE id=$1")
        .bind(sink)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(source, ("preserved-ingest-token".into(), row.4));
    let stored: (Value, String, String) =
        sqlx::query_as("SELECT properties,emoji,occurred_at::text FROM events WHERE id=$1")
            .bind(event)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        stored,
        (properties, "📌".into(), "2026-02-25 01:02:03".into())
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions WHERE user_id=$1")
            .bind(user)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );
    let column: (i32, Value) =
        sqlx::query_as("SELECT position,config FROM columns WHERE sink_id=$1")
            .bind(sink)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(column, (7, json!({"custom":true})));
    let input = EventInput::from_params(
        &json!({"text":"new native event","idempotency_key":"upgrade-check"}),
        Utc::now().naive_utc(),
    )
    .unwrap();
    assert!(persist(&db.pool, sink, &input).await.unwrap().unwrap() > event);
    assert_eq!(persist(&db.pool, sink, &input).await.unwrap(), None);
    let before = snapshot(&db.pool).await;
    schema::prepare(&db.pool, "test").await.unwrap();
    assert_eq!(snapshot(&db.pool).await, before);
    db.finish().await;
}

#[tokio::test]
async fn latest_rails_backfill_matches_original_and_failure_rolls_back_all_changes() {
    let db = Database::new().await;
    db.load(include_str!("fixtures/schema-20260628010000.sql"))
        .await;
    db.pool.execute("INSERT INTO accounts (id,created_at,updated_at) VALUES (1,now(),now()); INSERT INTO sinks (id,account_id,name,token,created_at,updated_at) VALUES (1,1,'Search','search-token',now(),now())").await.unwrap();
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/schema-search.json")).unwrap();
    for case in &cases {
        sqlx::query("INSERT INTO events (sink_id,event_type,text,properties,occurred_at,created_at,updated_at) VALUES (1,$1,$2,$3,now(),now(),now())")
            .bind(case["event_type"].as_str().unwrap()).bind(case["text"].as_str().unwrap()).bind(&case["properties"]).execute(&db.pool).await.unwrap();
    }
    let invalid:i64=sqlx::query_scalar("INSERT INTO events (sink_id,event_type,text,properties,occurred_at,created_at,updated_at) VALUES (1,'bad','invalid legacy shape','[]',now(),now(),now()) RETURNING id").fetch_one(&db.pool).await.unwrap();
    assert!(
        schema::prepare(&db.pool, "test")
            .await
            .unwrap_err()
            .to_string()
            .contains("object-valued")
    );
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM information_schema.columns WHERE table_schema='public' AND table_name='events' AND column_name='search_text'").fetch_one(&db.pool).await.unwrap(),0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM schema_migrations")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        32
    );
    sqlx::query("DELETE FROM events WHERE id=$1")
        .bind(invalid)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        schema::prepare(&db.pool, "test").await.unwrap().applied,
        [schema::CURRENT_VERSION]
    );
    let actual: Vec<String> = sqlx::query_scalar("SELECT search_text FROM events ORDER BY id")
        .fetch_all(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        actual,
        cases
            .iter()
            .map(|r| r["search_text"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    );
    db.finish().await;
}

#[tokio::test]
async fn incompatible_and_unknown_databases_are_rejected_without_rewriting_them() {
    let db = Database::new().await;
    db.pool
        .execute("CREATE TABLE unrelated (id integer); INSERT INTO unrelated VALUES(42)")
        .await
        .unwrap();
    assert!(
        schema::prepare(&db.pool, "test")
            .await
            .unwrap_err()
            .to_string()
            .contains("no Rails migration history")
    );
    assert_eq!(
        sqlx::query_scalar::<_, i32>("SELECT id FROM unrelated")
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        42
    );
    db.pool.execute("DROP TABLE unrelated").await.unwrap();
    schema::prepare(&db.pool, "test").await.unwrap();
    db.pool
        .execute("INSERT INTO schema_migrations VALUES ('20990101000000')")
        .await
        .unwrap();
    let before = snapshot(&db.pool).await;
    assert!(
        schema::prepare(&db.pool, "test")
            .await
            .unwrap_err()
            .to_string()
            .contains("unknown")
    );
    assert_eq!(snapshot(&db.pool).await, before);
    db.pool.execute("DELETE FROM schema_migrations WHERE version='20990101000000'; ALTER TABLE events ALTER COLUMN emoji DROP NOT NULL").await.unwrap();
    assert!(
        schema::check(&db.pool)
            .await
            .unwrap_err()
            .to_string()
            .contains("events.emoji")
    );
    assert!(schema::prepare(&db.pool, "test").await.is_err());
    db.pool.execute("ALTER TABLE events ALTER COLUMN emoji SET NOT NULL; DROP INDEX index_accounts_on_join_code; INSERT INTO accounts (join_code,created_at,updated_at) VALUES ('duplicate',now(),now()),('duplicate',now(),now())").await.unwrap();
    // PostgreSQL retains an INVALID index when a concurrent unique build fails.
    // Its pg_get_indexdef still matches; startup must check its usable state too.
    assert!(db.pool.execute("CREATE UNIQUE INDEX CONCURRENTLY index_accounts_on_join_code ON accounts USING btree (join_code)").await.is_err());
    assert!(
        schema::check(&db.pool)
            .await
            .unwrap_err()
            .to_string()
            .contains("accounts.index_accounts_on_join_code")
    );
    db.finish().await;
}
