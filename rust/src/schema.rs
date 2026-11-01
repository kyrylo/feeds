//! SQLite storage is local, durable and independent of a database server.
use serde::Serialize;
use sqlx::{
    SqliteConnection, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use std::{str::FromStr, time::Duration};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const BASE_VERSION: &str = "2026101801";
const ACTIVITY_VERSION: &str = "2026101802";
const TABS_VERSION: &str = "2026101803";
const UNREAD_VERSION: &str = "2026101804";
pub const CURRENT_VERSION: &str = "2026103001";
pub const DDL: &str = include_str!("../schema/sqlite.sql");
const ACTIVITY_DDL: &str = include_str!("../schema/activity.sql");
const TABS_DDL: &str = include_str!("../schema/activity-tabs.sql");
const UNREAD_DDL: &str = include_str!("../schema/activity-unread.sql");

#[derive(Debug, Serialize)]
pub struct Report {
    pub bootstrapped: bool,
    pub applied: Vec<String>,
    pub version: &'static str,
}

pub fn database_url() -> String {
    std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        // A rename must not strand an existing native installation's database.
        if std::path::Path::new("storage/telesink.sqlite3").exists() {
            "sqlite://storage/telesink.sqlite3".into()
        } else {
            "sqlite://storage/feeds.sqlite3".into()
        }
    })
}

pub async fn connect(url: &str, connections: u32, create: bool) -> Result<SqlitePool> {
    connect_options(url, connections, create, false).await
}

pub async fn connect_read_only(url: &str) -> Result<SqlitePool> {
    connect_options(url, 1, false, true).await
}

async fn connect_options(
    url: &str,
    connections: u32,
    create: bool,
    read_only: bool,
) -> Result<SqlitePool> {
    if !url.starts_with("sqlite:") {
        return Err("DATABASE_URL must be a SQLite URL; migrate PostgreSQL into a new SQLite file before starting this runtime".into());
    }
    let mut options = SqliteConnectOptions::from_str(url)?
        .create_if_missing(create)
        .read_only(read_only)
        .foreign_keys(true)
        .synchronous(SqliteSynchronous::Full)
        .busy_timeout(Duration::from_secs(5))
        .pragma("cache_size", "-2048")
        .pragma("wal_autocheckpoint", "1000");
    if !read_only {
        options = options.journal_mode(SqliteJournalMode::Wal);
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let pool = loop {
        let result = SqlitePoolOptions::new()
            .max_connections(connections)
            .min_connections(1)
            .acquire_timeout(Duration::from_secs(10))
            .after_connect(|connection, _| {
                Box::pin(async move {
                    crate::sqlite_functions::register(connection).await?;
                    Ok(())
                })
            })
            .connect_with(options.clone())
            .await;
        match result {
            Ok(pool) => break pool,
            Err(sqlx::Error::Database(error))
                if error
                    .code()
                    .and_then(|code| code.parse::<i32>().ok())
                    .is_some_and(|code| matches!(code & 255, 5 | 6))
                    && tokio::time::Instant::now() < deadline =>
            {
                // Initial WAL activation can race another process preparing a
                // brand-new file. SQLite may bypass its busy handler here.
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => return Err(error.into()),
        }
    };
    let version: String = sqlx::query_scalar("SELECT sqlite_version()")
        .fetch_one(&pool)
        .await?;
    let parts: Vec<u32> = version.split('.').filter_map(|v| v.parse().ok()).collect();
    if parts.as_slice() < [3, 51, 3].as_slice() {
        return Err("SQLite 3.51.3 or later is required for the WAL-reset fix".into());
    }
    Ok(pool)
}

pub async fn prepare(pool: &SqlitePool, environment: &str) -> Result<Report> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let history: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='telesink_schema_migrations' AND type='table')")
        .fetch_one(&mut *tx).await?;
    let mut report = Report {
        bootstrapped: !history,
        applied: vec![],
        version: CURRENT_VERSION,
    };
    if !history {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'")
                .fetch_one(&mut *tx)
                .await?;
        if count != 0 {
            return Err(
                "existing SQLite file is not a native Feeds database; refusing to overwrite it"
                    .into(),
            );
        }
        sqlx::raw_sql(DDL).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO telesink_schema_migrations(version) VALUES ($1)")
            .bind(BASE_VERSION)
            .execute(&mut *tx)
            .await?;
        for part in include_str!("../schema/upgrades.sql")
            .split("\n-- version: ")
            .skip(1)
        {
            let version = part
                .split_once('\n')
                .ok_or("invalid reference migration")?
                .0;
            sqlx::query("INSERT INTO schema_migrations(version) VALUES ($1)")
                .bind(version)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("INSERT INTO ar_internal_metadata(key,value,created_at,updated_at) VALUES ('environment',$1,now(),now())").bind(environment).execute(&mut *tx).await?;
        report.applied.push(BASE_VERSION.into());
    }
    let mut versions: Vec<String> =
        sqlx::query_scalar("SELECT version FROM telesink_schema_migrations ORDER BY version")
            .fetch_all(&mut *tx)
            .await?;
    if versions == [BASE_VERSION] {
        // Additive upgrade: original Rails tables, identities and tokens are untouched.
        check_definitions(&mut tx, DDL).await?;
        sqlx::raw_sql(ACTIVITY_DDL).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO telesink_schema_migrations(version) VALUES ($1)")
            .bind(ACTIVITY_VERSION)
            .execute(&mut *tx)
            .await?;
        report.applied.push(ACTIVITY_VERSION.into());
        versions.push(ACTIVITY_VERSION.into());
    }
    if versions == [BASE_VERSION, ACTIVITY_VERSION] {
        check_definitions(&mut tx, DDL).await?;
        check_definitions(&mut tx, ACTIVITY_DDL).await?;
        sqlx::raw_sql(TABS_DDL).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO telesink_schema_migrations(version) VALUES ($1)")
            .bind(TABS_VERSION)
            .execute(&mut *tx)
            .await?;
        report.applied.push(TABS_VERSION.into());
        versions.push(TABS_VERSION.into());
    }
    if versions == [BASE_VERSION, ACTIVITY_VERSION, TABS_VERSION] {
        check_definitions(&mut tx, DDL).await?;
        check_definitions(&mut tx, ACTIVITY_DDL).await?;
        check_definitions(&mut tx, TABS_DDL).await?;
        sqlx::raw_sql(UNREAD_DDL).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO telesink_schema_migrations(version) VALUES ($1)")
            .bind(UNREAD_VERSION)
            .execute(&mut *tx)
            .await?;
        report.applied.push(UNREAD_VERSION.into());
        versions.push(UNREAD_VERSION.into());
    }
    if versions == [BASE_VERSION, ACTIVITY_VERSION, TABS_VERSION, UNREAD_VERSION] {
        sqlx::raw_sql(include_str!("../schema/feeds.sql"))
            .execute(&mut *tx)
            .await?;
        // Only sources without a usable URL need a new credential.
        let missing: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM sinks WHERE token IS NULL OR token=''")
                .fetch_all(&mut *tx)
                .await?;
        for id in missing {
            sqlx::query("UPDATE sinks SET token=$1 WHERE id=$2")
                .bind(crate::activity_tabs::new_token())
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("INSERT INTO telesink_schema_migrations(version) VALUES ($1)")
            .bind(CURRENT_VERSION)
            .execute(&mut *tx)
            .await?;
        report.applied.push(CURRENT_VERSION.into());
    }
    check_connection(&mut tx).await?;
    tx.commit().await?;
    Ok(report)
}

pub async fn check(pool: &SqlitePool) -> Result<()> {
    let mut connection = pool.acquire().await?;
    check_connection(&mut connection).await
}

async fn check_connection(connection: &mut SqliteConnection) -> Result<()> {
    let versions: Vec<String> =
        sqlx::query_scalar("SELECT version FROM telesink_schema_migrations ORDER BY version")
            .fetch_all(&mut *connection)
            .await?;
    if versions
        != [
            BASE_VERSION,
            ACTIVITY_VERSION,
            TABS_VERSION,
            UNREAD_VERSION,
            CURRENT_VERSION,
        ]
    {
        return Err("unknown SQLite schema version; refusing to serve this database".into());
    }
    check_definitions(connection, DDL).await?;
    check_definitions(connection, ACTIVITY_DDL).await?;
    check_definitions(connection, TABS_DDL).await?;
    check_definitions(connection, UNREAD_DDL).await?;
    let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&mut *connection)
        .await?;
    if foreign_keys != 1 {
        return Err("SQLite foreign keys are disabled".into());
    }
    let violations = sqlx::query("PRAGMA foreign_key_check")
        .fetch_optional(&mut *connection)
        .await?;
    if violations.is_some() {
        return Err("SQLite contains foreign-key violations".into());
    }
    let integrity: Vec<String> = sqlx::query_scalar("PRAGMA quick_check")
        .fetch_all(&mut *connection)
        .await?;
    if integrity != ["ok"] {
        return Err("SQLite integrity check failed".into());
    }
    Ok(())
}

async fn check_definitions(connection: &mut SqliteConnection, ddl: &str) -> Result<()> {
    let actual: Vec<String> =
        sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE sql IS NOT NULL")
            .fetch_all(&mut *connection)
            .await?;
    let normalize = |sql: &str| {
        sql.trim()
            .trim_end_matches(';')
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let actual: Vec<String> = actual.iter().map(|sql| normalize(sql)).collect();
    for definition in ddl
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("CREATE "))
    {
        if !actual.contains(&normalize(definition)) {
            let object = definition
                .split_whitespace()
                .take(4)
                .collect::<Vec<_>>()
                .join(" ");
            return Err(format!("SQLite schema mismatch: {object}").into());
        }
    }
    Ok(())
}
