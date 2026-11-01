//! Native bootstrap and upgrades retain Rails' schema_migrations and schema.
use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use sqlx::{
    Connection, PgConnection, PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{str::FromStr, time::Duration};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub const CURRENT_VERSION: &str = "20260629010000";

#[derive(Debug, Serialize)]
pub struct Report {
    pub bootstrapped: bool,
    pub applied: Vec<String>,
    pub version: &'static str,
}

pub fn migrations() -> Vec<(&'static str, &'static str)> {
    include_str!("../schema/upgrades.sql")
        .split("\n-- version: ")
        .skip(1)
        .map(|part| part.split_once('\n').expect("embedded migration version"))
        .collect()
}

pub async fn connect(url: &str, connections: u32, create: bool) -> Result<PgPool> {
    let options = PgConnectOptions::from_str(url)?;
    let pool = || {
        PgPoolOptions::new()
            .max_connections(connections)
            .min_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .idle_timeout(Duration::from_secs(60))
            .after_connect(|connection, _| {
                Box::pin(async move {
                    sqlx::query("SELECT set_config('TimeZone','UTC',false), set_config('search_path','public',false)")
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
    };
    match pool().connect_with(options.clone()).await {
        Ok(pool) => Ok(pool),
        Err(sqlx::Error::Database(error)) if create && error.code().as_deref() == Some("3D000") => {
            let database = options
                .get_database()
                .ok_or("DATABASE_URL must name a database")?;
            let mut admin =
                PgConnection::connect_with(&options.clone().database("postgres")).await?;
            sqlx::query("SELECT pg_advisory_lock(hashtextextended($1,0))")
                .bind(database)
                .execute(&mut admin)
                .await?;
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname=$1)")
                    .bind(database)
                    .fetch_one(&mut admin)
                    .await?;
            if !exists {
                sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                    "CREATE DATABASE \"{}\"",
                    database.replace('"', "\"\"")
                )))
                .execute(&mut admin)
                .await?;
            }
            admin.close().await?;
            Ok(pool().connect_with(options).await?)
        }
        Err(error) => Err(error.into()),
    }
}

pub async fn prepare(pool: &PgPool, environment: &str) -> Result<Report> {
    let mut tx = pool.begin().await?;
    sqlx::raw_sql(
        "SET LOCAL TIME ZONE 'UTC'; SET LOCAL search_path TO public; SET LOCAL lock_timeout='30s'",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query("SELECT pg_advisory_xact_lock(1413829460, 2)")
        .execute(&mut *tx)
        .await?;
    let history_exists: bool =
        sqlx::query_scalar("SELECT to_regclass('public.schema_migrations') IS NOT NULL")
            .fetch_one(&mut *tx)
            .await?;
    let migrations = migrations();
    let mut report = Report {
        bootstrapped: !history_exists,
        applied: vec![],
        version: CURRENT_VERSION,
    };
    if !history_exists {
        let tables: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relkind IN ('r','p','v','m','S')")
            .fetch_one(&mut *tx).await?;
        if tables != 0 {
            return Err(
                "database has objects but no Rails migration history; refusing to overwrite it"
                    .into(),
            );
        }
        // pg_dump's session SET commands are inappropriate on a pooled runtime
        // connection; the DDL itself is the unchanged Rails reference dump.
        let ddl = include_str!("../schema/rails-current.sql")
            .lines()
            .filter(|line| {
                !line.starts_with("SET ") && !line.starts_with("SELECT pg_catalog.set_config(")
            })
            .collect::<Vec<_>>()
            .join("\n");
        sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
            .execute(&mut *tx)
            .await?;
        for (version, _) in &migrations {
            record(&mut tx, version).await?;
            report.applied.push((*version).into());
        }
    } else {
        let applied = versions(&mut tx).await?;
        if !migrations
            .iter()
            .map(|(version, _)| *version)
            .collect::<Vec<_>>()
            .starts_with(&applied.iter().map(String::as_str).collect::<Vec<_>>())
        {
            return Err(
                "migration history is unknown or has gaps; refusing to change this database".into(),
            );
        }
        for (version, sql) in migrations.iter().skip(applied.len()) {
            sqlx::raw_sql(sqlx::AssertSqlSafe(*sql))
                .execute(&mut *tx)
                .await
                .map_err(|error| format!("migration {version} failed: {error}"))?;
            match *version {
                "20260406090736" => backfill_accounts(&mut tx).await?,
                "20260629010000" => backfill_search(&mut tx).await?,
                _ => {}
            }
            record(&mut tx, version).await?;
            report.applied.push((*version).into());
        }
    }
    if !report.applied.is_empty() {
        sqlx::raw_sql("CREATE TABLE IF NOT EXISTS ar_internal_metadata (key varchar PRIMARY KEY, value varchar, created_at timestamp(6) NOT NULL, updated_at timestamp(6) NOT NULL)").execute(&mut *tx).await?;
        sqlx::query("INSERT INTO ar_internal_metadata (key,value,created_at,updated_at) VALUES ('environment',$1,now(),now()) ON CONFLICT (key) DO NOTHING")
            .bind(environment).execute(&mut *tx).await?;
    }
    check_connection(&mut tx).await?;
    tx.commit().await?;
    Ok(report)
}

async fn versions(connection: &mut PgConnection) -> Result<Vec<String>> {
    Ok(
        sqlx::query_scalar("SELECT version FROM schema_migrations ORDER BY version")
            .fetch_all(connection)
            .await?,
    )
}
async fn record(connection: &mut PgConnection, version: &str) -> Result<()> {
    sqlx::query("INSERT INTO schema_migrations (version) VALUES ($1)")
        .bind(version)
        .execute(connection)
        .await?;
    Ok(())
}

pub async fn check(pool: &PgPool) -> Result<()> {
    let mut connection = pool.acquire().await?;
    check_connection(&mut connection).await
}
async fn check_connection(connection: &mut PgConnection) -> Result<()> {
    let expected_versions: Vec<_> = migrations().iter().map(|(v, _)| *v).collect();
    if versions(connection).await? != expected_versions {
        return Err("database migrations are not current; run telesink db prepare".into());
    }
    let actual: Value = sqlx::query_scalar(include_str!("../schema/catalog.sql"))
        .fetch_one(&mut *connection)
        .await?;
    let expected: Value = serde_json::from_str(include_str!("../schema/rails-catalog.json"))?;
    for kind in ["columns", "constraints", "indexes", "extensions"] {
        for definition in expected[kind]
            .as_array()
            .ok_or("invalid embedded schema catalog")?
        {
            if !actual[kind]
                .as_array()
                .is_some_and(|list| list.contains(definition))
            {
                let name = if let Some(fields) = definition.as_array() {
                    fields
                        .iter()
                        .take(2)
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(".")
                } else {
                    definition.as_str().unwrap_or(kind).into()
                };
                return Err(format!(
                    "schema mismatch in {kind}: {name}; refusing to serve an incompatible database"
                )
                .into());
            }
        }
    }
    Ok(())
}

async fn backfill_accounts(connection: &mut PgConnection) -> Result<()> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM accounts)")
        .fetch_one(&mut *connection)
        .await?;
    if exists {
        return Ok(());
    }
    let mut cursor = 0i64;
    loop {
        let users: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM users WHERE id>$1 ORDER BY id LIMIT 500")
                .bind(cursor)
                .fetch_all(&mut *connection)
                .await?;
        if users.is_empty() {
            break;
        }
        for user in users {
            let account: i64 = sqlx::query_scalar("INSERT INTO accounts (join_code,created_at,updated_at) VALUES ($1,now(),now()) RETURNING id")
                .bind(crate::browser::join_code()).fetch_one(&mut *connection).await?;
            sqlx::query("UPDATE users SET account_id=$1 WHERE id=$2")
                .bind(account)
                .bind(user)
                .execute(&mut *connection)
                .await?;
            cursor = user;
        }
    }
    Ok(())
}

async fn backfill_search(connection: &mut PgConnection) -> Result<()> {
    let mut cursor = 0i64;
    loop {
        let rows: Vec<(i64, String, String, Value)> = sqlx::query_as(
            "SELECT id,event_type,text,properties FROM events WHERE id>$1 ORDER BY id LIMIT 500",
        )
        .bind(cursor)
        .fetch_all(&mut *connection)
        .await?;
        if rows.is_empty() {
            break;
        }
        for (id, event_type, text, properties) in rows {
            if !properties.is_object() {
                return Err("search backfill requires object-valued event properties; no upgrade was committed".into());
            }
            let event = crate::events::EventInput {
                event_type,
                text,
                properties,
                occurred_at: Utc::now().naive_utc(),
                emoji: String::new(),
                idempotency_key: None,
                sdk_name: None,
                sdk_version: None,
            };
            sqlx::query("UPDATE events SET search_text=$1 WHERE id=$2")
                .bind(event.search_text())
                .bind(id)
                .execute(&mut *connection)
                .await?;
            cursor = id;
        }
    }
    sqlx::raw_sql("ALTER TABLE events ALTER COLUMN search_text SET NOT NULL; CREATE INDEX index_events_on_search_text_trgm ON events USING gin(search_text gin_trgm_ops)")
        .execute(connection).await?;
    Ok(())
}
