#![cfg(feature = "postgres-import")]
use feeds::{
    events::{EventInput, persist},
    import_postgres, postgres, schema,
};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Fixture {
    pool: PgPool,
    url: String,
    name: String,
    output: PathBuf,
}
impl Fixture {
    async fn new() -> Self {
        assert_eq!(
            std::env::var("TELESINK_TEST_DATABASE_URL").unwrap(),
            "postgresql://telesink@127.0.0.1:55438/telesink_rust_test"
        );
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = format!(
            "telesink_sqlite_import_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let url = format!("postgresql://telesink@127.0.0.1:55438/{name}");
        let pool = postgres::connect(&url, 2, true).await.unwrap();
        postgres::prepare(&pool, "production").await.unwrap();
        let directory =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tmp/sqlite-migration/tests");
        std::fs::create_dir_all(&directory).unwrap();
        let output = directory.join(format!("{name}.sqlite3"));
        assert!(!output.exists());
        Self {
            pool,
            url,
            name,
            output,
        }
    }
    async fn seed(&self) {
        // The schema comes from the independent Rails pg_dump contract. Fixtures
        // explicitly exercise nullable columns, cyclic references and every table.
        sqlx::raw_sql(r#"
INSERT INTO accounts(id,join_code,created_at,updated_at) VALUES (100,'original-invite','2026-06-01 12:00:00.000123','2026-06-02 12:00:00');
INSERT INTO folders(id,account_id,name,created_at,updated_at) VALUES (101,100,'original folder',now(),now());
INSERT INTO sinks(id,account_id,folder_id,name,token,created_at,updated_at) VALUES (202,100,101,'original source','do-not-reroute',now(),now());
INSERT INTO users(id,account_id,current_folder_id,current_sink_id,email_address,nickname,password_digest,role,created_at,updated_at) VALUES (303,100,101,202,'owner@example.test',NULL,'unchanged-bcrypt-digest',2,now(),now());
INSERT INTO sessions(id,user_id,ip_address,user_agent,created_at,updated_at) VALUES (404,303,'192.0.2.1','original browser',now(),now());
INSERT INTO sink_memberships(id,user_id,sink_id,column_last_viewed_at,last_viewed_at,has_unread_events,unread_count,created_at,updated_at) VALUES (505,303,202,'{"606":"2026-06-01T00:00:00Z","unknown":{"keep":true}}',NULL,true,7,now(),now());
INSERT INTO columns(id,sink_id,name,config,position,has_events,created_at,updated_at) VALUES (606,202,'retained column','{"search":"old","legacy_option":[null,true,9007199254740993]}',3,true,now(),now());
INSERT INTO saved_views(id,user_id,sink_id,name,event_date,property_key,property_op,property_value,created_at,updated_at) VALUES (707,303,202,'retained view','2026-06-01','price','gt','1.0000000000000000001',now(),now());
INSERT INTO events(id,sink_id,event_type,emoji,text,properties,occurred_at,idempotency_key,sdk_name,sdk_version,search_text,created_at,updated_at) VALUES (9007199254740993,202,'payment','💰','original event','{"large":9007199254740993,"decimal":1.0000000000000000001,"null":null,"array":[null,true,"x"]}','2026-06-01 12:00:00.000123','original-idempotency','ruby','1.0','original searchable text',now(),now());
SELECT setval('events_id_seq',9007199254741099,true);
CREATE TABLE telesink_imported_jobs(active_job_id varchar PRIMARY KEY,payload_hash varchar(64) NOT NULL,kind varchar NOT NULL,imported_at timestamp(6) NOT NULL);
INSERT INTO telesink_imported_jobs VALUES ('retained-job','abc','event',now());
"#).execute(&self.pool).await.unwrap();
    }
    async fn finish(self) {
        self.pool.close().await;
        let mut admin = PgConnection::connect("postgresql://telesink@127.0.0.1:55438/postgres")
            .await
            .unwrap();
        assert!(
            self.name
                .starts_with(&format!("telesink_sqlite_import_{}_", std::process::id()))
        );
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE \"{}\"",
            self.name
        )))
        .execute(&mut admin)
        .await
        .unwrap();
        admin.close().await.unwrap();
    }
}

#[tokio::test]
async fn every_table_survives_with_exact_payloads_references_and_sequence_floors() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let report = import_postgres::migrate(&fixture.url, &fixture.output)
        .await
        .unwrap();
    assert_eq!(report.integrity, "ok");
    assert_eq!(report.tables.len(), 12);
    assert_eq!(report.tables["events"].rows, 1);
    assert_eq!(
        report.tables["events"].sequence_floor,
        Some(9007199254741099)
    );
    let sqlite = schema::connect(&format!("sqlite://{}", fixture.output.display()), 3, false)
        .await
        .unwrap();
    schema::check(&sqlite).await.unwrap();
    let tab: (i64, i64, String, Value) =
        sqlx::query_as("SELECT user_id,source_id,name,filters FROM telesink_activity_tabs")
            .fetch_one(&sqlite)
            .await
            .unwrap();
    assert_eq!((tab.0, tab.1, tab.2), (303, 202, "retained view".into()));
    assert_eq!(tab.3["property_value"], "1.0000000000000000001");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT last_seen_id FROM telesink_activity_tab_visits")
            .fetch_one(&sqlite)
            .await
            .unwrap(),
        9007199254740993,
        "Imported saved tabs start from the latest accessible event"
    );
    let expected: Value =
        sqlx::query_scalar("SELECT properties FROM events WHERE id=9007199254740993")
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
    let actual: Value =
        sqlx::query_scalar("SELECT properties FROM events WHERE id=9007199254740993")
            .fetch_one(&sqlite)
            .await
            .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual["large"].to_string(), "9007199254740993");
    assert_eq!(actual["decimal"].to_string(), "1.0000000000000000001");
    assert!(actual["null"].is_null());
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT token FROM sinks WHERE id=202")
            .fetch_one(&sqlite)
            .await
            .unwrap(),
        "do-not-reroute"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT user_id FROM sessions WHERE id=404")
            .fetch_one(&sqlite)
            .await
            .unwrap(),
        303
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT occurred_at FROM events")
            .fetch_one(&sqlite)
            .await
            .unwrap(),
        "2026-06-01 12:00:00.000123"
    );
    let duplicate = EventInput::from_params(
        &json!({"text":"ignored duplicate","idempotency_key":"original-idempotency"}),
        chrono::Utc::now().naive_utc(),
    )
    .unwrap();
    assert_eq!(persist(&sqlite, 202, &duplicate).await.unwrap(), None);
    let event = EventInput::from_params(
        &json!({"text":"first SQLite write"}),
        chrono::Utc::now().naive_utc(),
    )
    .unwrap();
    assert!(persist(&sqlite, 202, &event).await.unwrap().unwrap() > 9007199254741099);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM events")
            .fetch_one(&fixture.pool)
            .await
            .unwrap(),
        1,
        "source must remain unchanged"
    );
    sqlite.close().await;
    fixture.finish().await;
}

#[tokio::test]
async fn refuses_overwriting_outputs_and_unknown_source_data_without_modifying_source() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    std::fs::write(&fixture.output, "existing data").unwrap();
    assert!(
        import_postgres::migrate(&fixture.url, &fixture.output)
            .await
            .unwrap_err()
            .to_string()
            .contains("overwrite")
    );
    assert_eq!(
        std::fs::read_to_string(&fixture.output).unwrap(),
        "existing data"
    );
    let other = fixture.output.with_extension("refused.sqlite3");
    sqlx::query("CREATE TABLE unknown_business_data(id bigint PRIMARY KEY,value text)")
        .execute(&fixture.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO unknown_business_data VALUES (1,'must survive')")
        .execute(&fixture.pool)
        .await
        .unwrap();
    assert!(
        import_postgres::migrate(&fixture.url, &other)
            .await
            .unwrap_err()
            .to_string()
            .contains("unrecognized")
    );
    assert!(!other.exists());
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT value FROM unknown_business_data")
            .fetch_one(&fixture.pool)
            .await
            .unwrap(),
        "must survive"
    );
    sqlx::query("DROP TABLE unknown_business_data")
        .execute(&fixture.pool)
        .await
        .unwrap();
    let retried = import_postgres::migrate(&fixture.url, &other)
        .await
        .unwrap();
    assert_eq!(retried.tables["events"].rows, 1);
    assert_eq!(retried.integrity, "ok");
    fixture.finish().await;
}
