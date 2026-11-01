use feeds::schema;
use std::sync::atomic::{AtomicU64, Ordering};

async fn database() -> (sqlx::SqlitePool, String) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tmp/sqlite-migration/tests");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join(format!(
        "schema-{}-{}-{}.sqlite3",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    let url = format!("sqlite://{}", path.display());
    (schema::connect(&url, 3, false).await.unwrap(), url)
}

#[tokio::test]
async fn concurrent_bootstrap_retains_rows_history_and_high_water_ids() {
    let (pool, url) = database().await;
    let (a, b) = tokio::join!(
        schema::prepare(&pool, "test"),
        schema::prepare(&pool, "test")
    );
    assert_ne!(a.unwrap().bootstrapped, b.unwrap().bootstrapped);
    let version: String = sqlx::query_scalar("SELECT sqlite_version()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(version, "3.51.3");
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "wal"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA synchronous")
            .fetch_one(&pool)
            .await
            .unwrap(),
        2
    );
    sqlx::query("INSERT INTO accounts(id,join_code,created_at,updated_at) VALUES (1234567,'retained',now(),now())").execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM accounts WHERE id=1234567")
        .execute(&pool)
        .await
        .unwrap();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO accounts(created_at,updated_at) VALUES (now(),now()) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(id, 1234568);
    let history: Vec<String> =
        sqlx::query_scalar("SELECT version FROM schema_migrations ORDER BY version")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(history.len(), 33);
    pool.close().await;
    let reopened = schema::connect(&url, 2, false).await.unwrap();
    let report = schema::prepare(&reopened, "production").await.unwrap();
    assert!(!report.bootstrapped);
    assert!(report.applied.is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT id FROM accounts")
            .fetch_one(&reopened)
            .await
            .unwrap(),
        id
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT value FROM ar_internal_metadata WHERE key='environment'"
        )
        .fetch_one(&reopened)
        .await
        .unwrap(),
        "test"
    );
    schema::check(&reopened).await.unwrap();
    reopened.close().await;
}

#[tokio::test]
async fn unrelated_and_unknown_databases_are_never_overwritten() {
    let (pool, _) = database().await;
    sqlx::query("CREATE TABLE unrelated(id INTEGER PRIMARY KEY,value TEXT)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO unrelated VALUES (1,'retain me')")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        schema::prepare(&pool, "test")
            .await
            .unwrap_err()
            .to_string()
            .contains("refusing to overwrite")
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT value FROM unrelated")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "retain me"
    );
    pool.close().await;
    let (pool, _) = database().await;
    schema::prepare(&pool, "test").await.unwrap();
    sqlx::query("INSERT INTO telesink_schema_migrations(version) VALUES ('future')")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        schema::prepare(&pool, "test")
            .await
            .unwrap_err()
            .to_string()
            .contains("unknown")
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT version FROM telesink_schema_migrations WHERE version='future'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        "future"
    );
    pool.close().await;
}

#[tokio::test]
async fn missing_uniqueness_and_dangling_relations_are_detected() {
    let (pool, _) = database().await;
    schema::prepare(&pool, "test").await.unwrap();
    let error =
        sqlx::query("INSERT INTO sessions(user_id,created_at,updated_at) VALUES (999,now(),now())")
            .execute(&pool)
            .await
            .unwrap_err();
    assert!(
        error
            .as_database_error()
            .unwrap()
            .is_foreign_key_violation()
    );
    sqlx::query("DROP INDEX index_sinks_on_token")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        schema::check(&pool)
            .await
            .unwrap_err()
            .to_string()
            .contains("schema mismatch")
    );
    pool.close().await;
}

#[tokio::test]
async fn postgres_urls_are_refused_instead_of_starting_an_empty_sqlite_database() {
    assert!(
        schema::connect("postgresql://localhost/production", 1, false)
            .await
            .unwrap_err()
            .to_string()
            .contains("migrate PostgreSQL")
    );
}

#[tokio::test]
async fn live_backup_restores_committed_wal_and_preserves_deleted_id_floors() {
    let (pool, url) = database().await;
    schema::prepare(&pool, "test").await.unwrap();
    sqlx::raw_sql("INSERT INTO accounts(id,join_code,created_at,updated_at) VALUES (600,'committed',now(),now()); INSERT INTO accounts(id,created_at,updated_at) VALUES (900,now(),now()); DELETE FROM accounts WHERE id=900;")
        .execute(&pool).await.unwrap();
    let mut pending = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
    sqlx::query(
        "INSERT INTO accounts(join_code,created_at,updated_at) VALUES ('uncommitted',now(),now())",
    )
    .execute(&mut *pending)
    .await
    .unwrap();
    let source_path = std::path::Path::new(url.strip_prefix("sqlite://").unwrap());
    let output = source_path.with_extension("backup.sqlite3");
    feeds::backup::create(&url, &output).await.unwrap();
    pending.rollback().await.unwrap();
    sqlx::query("UPDATE accounts SET join_code='after snapshot' WHERE id=600")
        .execute(&pool)
        .await
        .unwrap();
    let backup_url = format!("sqlite://{}", output.display());
    let readonly = schema::connect_read_only(&backup_url).await.unwrap();
    assert!(
        sqlx::query("DELETE FROM accounts")
            .execute(&readonly)
            .await
            .is_err()
    );
    readonly.close().await;
    let restored = schema::connect(&backup_url, 1, false).await.unwrap();
    schema::check(&restored).await.unwrap();
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id,join_code FROM accounts")
        .fetch_all(&restored)
        .await
        .unwrap();
    assert_eq!(rows, [(600, "committed".into())]);
    let next: i64 = sqlx::query_scalar(
        "INSERT INTO accounts(created_at,updated_at) VALUES (now(),now()) RETURNING id",
    )
    .fetch_one(&restored)
    .await
    .unwrap();
    assert_eq!(next, 901);
    let before = std::fs::read(&output).unwrap();
    assert!(
        feeds::backup::create(&url, &output)
            .await
            .unwrap_err()
            .to_string()
            .contains("overwrite")
    );
    assert_eq!(std::fs::read(&output).unwrap(), before);
    restored.close().await;
    pool.close().await;
}

// Run in a separate OS process below, then kill without graceful shutdown.
#[tokio::test]
async fn crash_writer() {
    use std::io::Write;
    let Ok(url) = std::env::var("TELESINK_DURABILITY_CHILD") else {
        return;
    };
    let pool = schema::connect(&url, 1, false).await.unwrap();
    sqlx::query("INSERT INTO accounts(id,join_code,created_at,updated_at) VALUES (1200,'durable',now(),now())").execute(&pool).await.unwrap();
    let mut pending = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
    sqlx::query("INSERT INTO accounts(id,join_code,created_at,updated_at) VALUES (1201,'not committed',now(),now())").execute(&mut *pending).await.unwrap();
    println!("DURABILITY_READY");
    std::io::stdout().flush().unwrap();
    std::thread::park();
    unreachable!("parent must kill this process");
}

#[tokio::test]
async fn abrupt_process_death_retains_commits_and_discards_uncommitted_writes() {
    use std::io::BufRead;
    let (pool, url) = database().await;
    schema::prepare(&pool, "test").await.unwrap();
    pool.close().await;
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_writer", "--nocapture"])
        .env("TELESINK_DURABILITY_CHILD", &url)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (sent, received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let ready = std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
            .any(|line| line.contains("DURABILITY_READY"));
        let _ = sent.send(ready);
    });
    let ready = received.recv_timeout(std::time::Duration::from_secs(15));
    // Always reap the process, including a failed or timed-out startup.
    let _ = child.kill();
    child.wait().unwrap();
    assert!(ready.unwrap());
    let reopened = schema::connect(&url, 1, false).await.unwrap();
    schema::check(&reopened).await.unwrap();
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id,join_code FROM accounts")
        .fetch_all(&reopened)
        .await
        .unwrap();
    assert_eq!(rows, [(1200, "durable".into())]);
    reopened.close().await;
}

#[tokio::test]
async fn activity_upgrade_is_additive_atomic_and_preserves_rails_rows_and_sequences() {
    let (pool, _) = database().await;
    sqlx::raw_sql(schema::DDL).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO telesink_schema_migrations(version) VALUES ('2026101801')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("INSERT INTO accounts(id,join_code,created_at,updated_at) VALUES (1,'keep',now(),now()); INSERT INTO users(id,account_id,email_address,password_digest,role,created_at,updated_at) VALUES (1,1,'owner@example.test','digest',2,now(),now()); INSERT INTO sinks(id,account_id,name,token,created_at,updated_at) VALUES (17,1,'Product','legacy-token',now(),now()); INSERT INTO sink_memberships(user_id,sink_id,created_at,updated_at) VALUES (1,17,now(),now()); INSERT INTO events(id,sink_id,event_type,text,search_text,occurred_at,created_at,updated_at) VALUES (9000,17,'signup','keep this','keep this',now(),now(),now()); INSERT INTO events(id,sink_id,event_type,text,search_text,occurred_at,created_at,updated_at) VALUES (9999,17,'deleted','deleted','deleted',now(),now(),now()); DELETE FROM events WHERE id=9999;").execute(&pool).await.unwrap();
    let before: Vec<(i64, String, String)> = sqlx::query_as("SELECT id,token,name FROM sinks")
        .fetch_all(&pool)
        .await
        .unwrap();
    let (first, second) = tokio::join!(
        schema::prepare(&pool, "test"),
        schema::prepare(&pool, "test")
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert!(!first.bootstrapped && !second.bootstrapped);
    assert_eq!(first.applied.len() + second.applied.len(), 4);
    assert_eq!(
        sqlx::query_as::<_, (i64, String, String)>("SELECT id,token,name FROM sinks")
            .fetch_all(&pool)
            .await
            .unwrap(),
        before
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT text FROM events WHERE id=9000")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "keep this"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT seq FROM sqlite_sequence WHERE name='events'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        9999
    );
    schema::check(&pool).await.unwrap();
    pool.close().await;
}

#[tokio::test]
async fn saved_tab_upgrade_preserves_legacy_saved_views_and_imports_them_once() {
    let (pool, _) = database().await;
    schema::prepare(&pool, "test").await.unwrap();
    sqlx::raw_sql("DROP TRIGGER initialize_activity_tab_visit; DROP TABLE telesink_activity_tab_visits; DROP TABLE telesink_activity_tabs; DELETE FROM telesink_schema_migrations WHERE version IN ('2026101803','2026101804','2026103001'); INSERT INTO accounts(id,created_at,updated_at) VALUES (1,now(),now()); INSERT INTO users(id,account_id,email_address,password_digest,role,created_at,updated_at) VALUES (1,1,'owner@example.test','unchanged',2,now(),now()); INSERT INTO sinks(id,account_id,name,token,created_at,updated_at) VALUES (17,1,'Project','unchanged',now(),now()); INSERT INTO sink_memberships(user_id,sink_id,created_at,updated_at) VALUES (1,17,now(),now()); INSERT INTO saved_views(id,user_id,sink_id,name,search_query,event_type,event_date,property_key,property_op,property_value,created_at,updated_at) VALUES (29,1,17,'Payments','Maya','paid','2026-10-18','amount','gt','10',now(),now());").execute(&pool).await.unwrap();
    let before: (String, String) =
        sqlx::query_as("SELECT name,search_query FROM saved_views WHERE id=29")
            .fetch_one(&pool)
            .await
            .unwrap();
    let (a, b) = tokio::join!(
        schema::prepare(&pool, "test"),
        schema::prepare(&pool, "test")
    );
    assert_eq!(a.unwrap().applied.len() + b.unwrap().applied.len(), 3);
    let tab: (i64, i64, String, serde_json::Value) =
        sqlx::query_as("SELECT user_id,source_id,name,filters FROM telesink_activity_tabs")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((tab.0, tab.1, tab.2), (1, 17, "Payments".into()));
    let filters = feeds::feed::Filters::from_params(&tab.3);
    assert_eq!(filters.search_query.as_deref(), Some("Maya"));
    assert_eq!(filters.event_type.as_deref(), Some("paid"));
    assert_eq!(filters.property_value.as_deref(), Some("10"));
    assert_eq!(filters.event_date.unwrap().to_string(), "2026-10-18");
    assert_eq!(
        sqlx::query_as::<_, (String, String)>(
            "SELECT name,search_query FROM saved_views WHERE id=29"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        before
    );
    assert!(
        schema::prepare(&pool, "test")
            .await
            .unwrap()
            .applied
            .is_empty()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM telesink_activity_tabs")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    schema::check(&pool).await.unwrap();
    pool.close().await;
}

#[tokio::test]
async fn unread_upgrade_seeds_existing_progress_once_and_new_tabs_get_independent_visits() {
    let (pool, _) = database().await;
    schema::prepare(&pool, "test").await.unwrap();
    sqlx::raw_sql("DROP TRIGGER initialize_activity_tab_visit; DROP TABLE telesink_activity_tab_visits; DELETE FROM telesink_schema_migrations WHERE version IN ('2026101804','2026103001');
        INSERT INTO accounts(id,created_at,updated_at) VALUES (1,now(),now());
        INSERT INTO users(id,account_id,email_address,password_digest,created_at,updated_at) VALUES (1,1,'one@example.test','unchanged',now(),now()),(2,1,'two@example.test','unchanged',now(),now());
        INSERT INTO sinks(id,account_id,name,token,created_at,updated_at) VALUES (17,1,'Project','keep-token',now(),now()),(18,1,'Unshared','hidden-token',now(),now());
        INSERT INTO sink_memberships(user_id,sink_id,created_at,updated_at) VALUES (1,17,now(),now()),(2,17,now(),now());
        INSERT INTO events(id,sink_id,event_type,text,search_text,occurred_at,created_at,updated_at) VALUES (9,17,'signup','keep event','keep event',now(),now(),now()),(20,18,'signup','unshared event','unshared event',now(),now(),now());
        INSERT INTO telesink_activity_visits(user_id,last_seen_id,updated_at) VALUES (1,5,now());
        INSERT INTO telesink_activity_tabs(id,user_id,source_id,name,filters,position,created_at,updated_at) VALUES (1,1,17,'Existing','{}',0,now(),now()),(2,2,17,'No previous visit','{}',0,now(),now());").execute(&pool).await.unwrap();
    let (a, b) = tokio::join!(
        schema::prepare(&pool, "test"),
        schema::prepare(&pool, "test")
    );
    assert_eq!(a.unwrap().applied.len() + b.unwrap().applied.len(), 2);
    assert_eq!(
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT tab_id,last_seen_id FROM telesink_activity_tab_visits ORDER BY tab_id"
        )
        .fetch_all(&pool)
        .await
        .unwrap(),
        [(1, 5), (2, 9)]
    );
    sqlx::raw_sql("UPDATE telesink_activity_visits SET last_seen_id=9 WHERE user_id=1; INSERT INTO telesink_activity_tabs(id,user_id,name,filters,position,created_at,updated_at) VALUES (3,1,'New','{}',1,now(),now());").execute(&pool).await.unwrap();
    assert_eq!(
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT tab_id,last_seen_id FROM telesink_activity_tab_visits ORDER BY tab_id"
        )
        .fetch_all(&pool)
        .await
        .unwrap(),
        [(1, 5), (2, 9), (3, 9)]
    );
    sqlx::query("DELETE FROM telesink_activity_tabs WHERE id=3")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM telesink_activity_tab_visits")
            .fetch_one(&pool)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT token FROM sinks WHERE id=17")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "keep-token"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT text FROM events WHERE id=9")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "keep event"
    );
    schema::check(&pool).await.unwrap();
    pool.close().await;
}

#[tokio::test]
async fn feed_upgrade_preserves_existing_identities_and_backfills_missing_resources_once() {
    let (pool, _) = database().await;
    schema::prepare(&pool, "test").await.unwrap();
    sqlx::raw_sql("DELETE FROM telesink_schema_migrations WHERE version='2026103001';
        INSERT INTO accounts(id,created_at,updated_at) VALUES (1,now(),now());
        INSERT INTO users(id,account_id,email_address,password_digest,role,created_at,updated_at) VALUES (1,1,'owner@example.test','unchanged',2,now(),now());
        INSERT INTO sinks(id,account_id,name,token,created_at,updated_at) VALUES (3,1,'Existing','keep-url',now(),now()),(7,1,'Unlisted',NULL,now(),now());
        INSERT INTO sink_memberships(user_id,sink_id,created_at,updated_at) VALUES (1,3,now(),now()),(1,7,now(),now());
        INSERT INTO events(id,sink_id,event_type,text,search_text,occurred_at,created_at,updated_at) VALUES (44,3,'signup','keep event','keep event',now(),now(),now());
        INSERT INTO telesink_activity_tabs(id,user_id,source_id,name,filters,position,created_at,updated_at) VALUES (2,1,3,'Existing','{}',0,now(),now()),(5,1,3,'Saved search','{\"search_query\":\"failed\"}',1,now(),now());
        UPDATE telesink_activity_tab_visits SET last_seen_id=40 WHERE tab_id=2;").execute(&pool).await.unwrap();
    let before: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT id,name,filters FROM telesink_activity_tabs ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    let (a, b) = tokio::join!(
        schema::prepare(&pool, "test"),
        schema::prepare(&pool, "test")
    );
    assert_eq!(a.unwrap().applied.len() + b.unwrap().applied.len(), 1);
    assert_eq!(
        sqlx::query_as::<_, (i64, String, String)>(
            "SELECT id,name,filters FROM telesink_activity_tabs WHERE id IN (2,5) ORDER BY id"
        )
        .fetch_all(&pool)
        .await
        .unwrap(),
        before
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT last_seen_id FROM telesink_activity_tab_visits WHERE tab_id=2"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        40
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM telesink_activity_tabs WHERE source_id=7"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT token FROM sinks WHERE id=3")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "keep-url"
    );
    assert!(
        sqlx::query_scalar::<_, String>("SELECT token FROM sinks WHERE id=7")
            .fetch_one(&pool)
            .await
            .unwrap()
            .starts_with("feed_")
    );
    assert_eq!(
        sqlx::query_as::<_, (i64, String)>("SELECT id,text FROM events")
            .fetch_all(&pool)
            .await
            .unwrap(),
        [(44, "keep event".into())]
    );
    assert!(
        schema::prepare(&pool, "test")
            .await
            .unwrap()
            .applied
            .is_empty()
    );
    schema::check(&pool).await.unwrap();
    pool.close().await;
}
