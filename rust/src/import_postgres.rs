//! Offline, verified PostgreSQL snapshot import. The ordinary server has no PG driver.
use crate::{
    backup::StagedDatabase,
    postgres,
    schema::{self, Result},
};
use chrono::NaiveDateTime;
use futures_util::TryStreamExt;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{QueryBuilder, Row, Sqlite, sqlite::SqliteRow};
use std::{collections::BTreeMap, path::Path};

const TABLES: &[&str] = &[
    "accounts",
    "folders",
    "sinks",
    "users",
    "sessions",
    "sink_memberships",
    "columns",
    "saved_views",
    "events",
    "schema_migrations",
    "ar_internal_metadata",
    "telesink_imported_jobs",
];

#[derive(Debug, Serialize)]
pub struct TableReport {
    pub rows: u64,
    pub sha256: String,
    pub sequence_floor: Option<i64>,
}
#[derive(Debug, Serialize)]
pub struct Report {
    pub source_schema: String,
    pub sqlite_schema: String,
    pub tables: BTreeMap<String, TableReport>,
    pub integrity: String,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
enum Cell {
    Null,
    Integer(i64),
    Text(String),
}

fn cell(value: &Value, kind: &str) -> Result<Cell> {
    if value.is_null() {
        return Ok(Cell::Null);
    }
    Ok(match kind {
        "bigint" | "integer" | "smallint" => Cell::Integer(
            value
                .as_i64()
                .ok_or("source integer is outside SQLite's exact range")?,
        ),
        "boolean" => Cell::Integer(i64::from(value.as_bool().ok_or("invalid source boolean")?)),
        "jsonb" | "json" => Cell::Text(serde_json::to_string(value)?),
        "timestamp without time zone" => {
            let raw = value.as_str().ok_or("invalid source timestamp")?;
            let time = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f")
                .or_else(|_| NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S%.f"))?;
            Cell::Text(time.format("%F %T%.f").to_string())
        }
        "text" | "character varying" | "date" => {
            Cell::Text(value.as_str().ok_or("invalid source text")?.into())
        }
        _ => return Err(format!("unsupported PostgreSQL data type: {kind}").into()),
    })
}

fn digest_row(hash: &mut Sha256, row: &[Cell]) -> Result<()> {
    let bytes = serde_json::to_vec(row)?;
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
    Ok(())
}

fn hex(hash: Sha256) -> String {
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn read_row(row: &SqliteRow, columns: &[(String, String)]) -> Result<Vec<Cell>> {
    use sqlx::ValueRef;
    columns
        .iter()
        .enumerate()
        .map(|(i, (_, kind))| {
            if row.try_get_raw(i)?.is_null() {
                return Ok(Cell::Null);
            }
            Ok(match kind.as_str() {
                "bigint" | "integer" | "smallint" | "boolean" => Cell::Integer(row.try_get(i)?),
                _ => Cell::Text(row.try_get(i)?),
            })
        })
        .collect()
}

/// The caller must stop every source writer before invoking this operation.
/// Read-only repeatable-read prevents mixed snapshots; it cannot prevent a live
/// application from accepting writes after the snapshot has been taken.
pub async fn migrate(source: &str, output: &Path) -> Result<Report> {
    let staging = StagedDatabase::new(output)?;

    let pg = postgres::connect(source, 1, false).await?;
    postgres::check(&pg).await?;
    let mut snapshot = pg.begin().await?;
    sqlx::raw_sql("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *snapshot)
        .await?;
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT tablename FROM pg_tables WHERE schemaname='public' ORDER BY tablename",
    )
    .fetch_all(&mut *snapshot)
    .await?;
    if tables.iter().any(|table| !TABLES.contains(&table.as_str())) {
        return Err(
            "source contains unrecognized application tables; refusing an incomplete migration"
                .into(),
        );
    }
    let other_data: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relkind IN ('m','f'))")
        .fetch_one(&mut *snapshot).await?;
    if other_data {
        return Err("source contains unrecognized materialized views or foreign tables; refusing an incomplete migration".into());
    }
    let sqlite = schema::connect(&format!("sqlite://{}", staging.path.display()), 1, false).await?;
    schema::prepare(&sqlite, "production").await?;
    let mut tx = sqlite.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::raw_sql("PRAGMA defer_foreign_keys=ON; DELETE FROM schema_migrations; DELETE FROM ar_internal_metadata;").execute(&mut *tx).await?;
    let mut report = Report {
        source_schema: postgres::CURRENT_VERSION.into(),
        sqlite_schema: schema::CURRENT_VERSION.into(),
        tables: BTreeMap::new(),
        integrity: String::new(),
    };
    for &table in TABLES {
        if !tables.iter().any(|name| name == table) {
            if table == "telesink_imported_jobs" {
                continue;
            }
            return Err(format!("source table missing: {table}").into());
        }
        let columns: Vec<(String,String)> = sqlx::query_as("SELECT column_name,data_type FROM information_schema.columns WHERE table_schema='public' AND table_name=$1 ORDER BY ordinal_position")
            .bind(table).fetch_all(&mut *snapshot).await?;
        // Names come exclusively from the fixed table list and a checked column
        // set. Quote identifiers anyway; payload values are always bound.
        let sqlite_columns: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT name FROM pragma_table_info('{table}') ORDER BY cid"
        )))
        .fetch_all(&mut *tx)
        .await?;
        let mut expected = sqlite_columns.clone();
        expected.sort();
        let mut actual: Vec<String> = columns.iter().map(|(name, _)| name.clone()).collect();
        actual.sort();
        if expected != actual {
            return Err(format!("column mismatch for {table}; refusing to omit data").into());
        }
        let names = columns
            .iter()
            .map(|(name, _)| format!("\"{}\"", name.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(",");
        let key = if table == "schema_migrations" {
            "version"
        } else if table == "ar_internal_metadata" {
            "key"
        } else if table == "telesink_imported_jobs" {
            "active_job_id"
        } else {
            "id"
        };
        let mut expected_hash = Sha256::new();
        let mut count = 0;
        let ordering = if key == "id" {
            format!("\"{key}\"")
        } else {
            format!("\"{key}\" COLLATE \"C\"")
        };
        {
            let mut rows = sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(format!(
                "SELECT row_to_json(t)::text FROM \"{table}\" t ORDER BY {ordering}"
            )))
            .fetch(&mut *snapshot);
            while let Some(raw) = rows.try_next().await? {
                let object: Value = serde_json::from_str(&raw)?;
                let values: Vec<Cell> = columns
                    .iter()
                    .map(|(name, kind)| {
                        cell(object.get(name).ok_or("source column missing")?, kind)
                    })
                    .collect::<Result<_>>()?;
                digest_row(&mut expected_hash, &values)?;
                let mut insert = QueryBuilder::<Sqlite>::new(format!(
                    "INSERT INTO \"{table}\" ({names}) VALUES ("
                ));
                for (index, value) in values.into_iter().enumerate() {
                    if index > 0 {
                        insert.push(",");
                    }
                    match value {
                        Cell::Null => {
                            insert.push_bind(Option::<String>::None);
                        }
                        Cell::Integer(value) => {
                            insert.push_bind(value);
                        }
                        Cell::Text(value) => {
                            insert.push_bind(value);
                        }
                    }
                }
                insert.push(")").build().execute(&mut *tx).await?;
                count += 1;
            }
        }
        let expected_hash = hex(expected_hash);
        let mut actual_hash = Sha256::new();
        let mut actual_count = 0;
        {
            let mut rows = sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT {names} FROM \"{table}\" ORDER BY \"{key}\""
            )))
            .fetch(&mut *tx);
            while let Some(row) = rows.try_next().await? {
                digest_row(&mut actual_hash, &read_row(&row, &columns)?)?;
                actual_count += 1;
            }
        }
        if actual_count != count || hex(actual_hash) != expected_hash {
            return Err(format!("row verification failed for {table}").into());
        }
        let sequence_floor = if key == "id" {
            let (last, called): (i64, bool) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT last_value,is_called FROM \"{table}_id_seq\""
            )))
            .fetch_one(&mut *snapshot)
            .await?;
            let maximum: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT COALESCE(MAX(id),0) FROM \"{table}\""
            )))
            .fetch_one(&mut *tx)
            .await?;
            let floor = maximum.max(if called { last } else { last - 1 });
            sqlx::query("DELETE FROM sqlite_sequence WHERE name=$1")
                .bind(table)
                .execute(&mut *tx)
                .await?;
            sqlx::query("INSERT INTO sqlite_sequence(name,seq) VALUES ($1,$2)")
                .bind(table)
                .bind(floor)
                .execute(&mut *tx)
                .await?;
            Some(floor)
        } else {
            None
        };
        report.tables.insert(
            table.into(),
            TableReport {
                rows: count,
                sha256: expected_hash,
                sequence_floor,
            },
        );
    }
    // Bootstrap ran before the Rails rows were copied. Seed their saved tabs
    // now, using the same static migration query as existing SQLite upgrades.
    let tab_seed = include_str!("../schema/activity-tabs.sql")
        .split_once("\n\n")
        .ok_or("saved-tab migration seed is missing")?
        .1;
    sqlx::raw_sql(tab_seed).execute(&mut *tx).await?;
    if sqlx::query("PRAGMA foreign_key_check")
        .fetch_optional(&mut *tx)
        .await?
        .is_some()
    {
        return Err("import would violate foreign keys".into());
    }
    tx.commit().await?;
    snapshot.commit().await?;
    pg.close().await;
    schema::check(&sqlite).await?;
    let integrity: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_all(&sqlite)
        .await?;
    if integrity != ["ok"] {
        return Err("imported SQLite integrity check failed".into());
    }
    let (busy, _, _): (i64, i64, i64) = sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
        .fetch_one(&sqlite)
        .await?;
    if busy != 0 {
        return Err("could not checkpoint the verified output".into());
    }
    sqlite.close().await;
    staging.publish()?;
    report.integrity = "ok".into();
    Ok(report)
}
