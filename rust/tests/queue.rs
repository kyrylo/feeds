#![cfg(feature = "postgres-import")]
mod common;
use chrono::{Duration, Utc};
use common::{Harness, smtp::Smtp};
use feeds::{AppState, config::Config, mail::Mailer, postgres, queue};
use serde_json::{Value, json};
use sqlx::{Connection, Executor, PgConnection, PgPool};
use std::sync::Arc;

struct Queue {
    pool: PgPool,
    name: String,
    url: String,
    account: i64,
}
impl Queue {
    async fn new(h: &Harness) -> Self {
        assert_eq!(
            std::env::var("TELESINK_TEST_DATABASE_URL").unwrap(),
            "postgresql://telesink@127.0.0.1:55438/telesink_rust_test"
        );
        let name = format!("telesink_rust_queue_{}_{}", std::process::id(), h.account);
        let url = format!("postgresql://telesink@127.0.0.1:55438/{name}");
        let pool = postgres::connect(&url, 2, true).await.unwrap();
        let ddl = include_str!("fixtures/queue-schema.sql")
            .lines()
            .filter(|l| !l.starts_with("SET ") && !l.starts_with("SELECT pg_catalog.set_config("))
            .collect::<Vec<_>>()
            .join("\n");
        sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
            .execute(&pool)
            .await
            .unwrap();
        Self {
            pool,
            name,
            url,
            account: h.account,
        }
    }
    async fn add(&self, h: &Harness, sink: i64, index: usize, number: usize, future: bool) -> i64 {
        let fixtures: Value =
            serde_json::from_str(include_str!("fixtures/queue-jobs.json")).unwrap();
        let job = &fixtures["jobs"][index];
        let mut arguments = job["arguments"].clone();
        let identity = format!("queue-{}-{number}", h.account);
        arguments["job_id"] = identity.clone().into();
        if index < 3 {
            arguments["arguments"][0]["_aj_globalid"] =
                format!("gid://telesink/Sink/{sink}").into();
        }
        if index == 3 {
            arguments["arguments"][3]["args"][0]["_aj_globalid"] =
                format!("gid://telesink/User/{}", h.user).into();
        }
        let scheduled = (Utc::now()
            + if future {
                Duration::hours(1)
            } else {
                Duration::seconds(-1)
            })
        .naive_utc();
        let id:i64=sqlx::query_scalar("INSERT INTO solid_queue_jobs (queue_name,class_name,arguments,priority,active_job_id,scheduled_at,created_at,updated_at) VALUES ($1,$2,$3,0,$4,$5,now(),now()) RETURNING id")
            .bind(job["queue_name"].as_str().unwrap()).bind(job["class_name"].as_str().unwrap()).bind(arguments.to_string()).bind(identity).bind(scheduled).fetch_one(&self.pool).await.unwrap();
        let table = if future {
            "solid_queue_scheduled_executions"
        } else {
            "solid_queue_ready_executions"
        };
        let (column, value) = if future {
            (",scheduled_at", ",$3")
        } else {
            ("", "")
        };
        let sql = format!(
            "INSERT INTO {table} (job_id,queue_name,priority,created_at{column}) VALUES ($1,$2,0,now(){value})"
        );
        let query = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .bind(job["queue_name"].as_str().unwrap());
        if future {
            query.bind(scheduled).execute(&self.pool).await.unwrap();
        } else {
            query.execute(&self.pool).await.unwrap();
        }
        id
    }
    async fn arguments(&self, id: i64) -> Value {
        let text: String = sqlx::query_scalar("SELECT arguments FROM solid_queue_jobs WHERE id=$1")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .unwrap();
        serde_json::from_str(&text).unwrap()
    }
    async fn replace(&self, id: i64, arguments: Value) {
        sqlx::query("UPDATE solid_queue_jobs SET arguments=$1 WHERE id=$2")
            .bind(arguments.to_string())
            .bind(id)
            .execute(&self.pool)
            .await
            .unwrap();
    }
    async fn finish(self, h: &Harness) {
        if sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='telesink_imported_jobs')",
        )
        .fetch_one(&h.pool)
        .await
        .unwrap()
        {
            sqlx::query("DELETE FROM telesink_imported_jobs WHERE active_job_id LIKE $1")
                .bind(format!("queue-{}-%", self.account))
                .execute(&h.pool)
                .await
                .unwrap();
        }
        self.pool.close().await;
        assert_eq!(
            self.name,
            format!("telesink_rust_queue_{}_{}", std::process::id(), h.account)
        );
        let mut admin = PgConnection::connect("postgresql://telesink@127.0.0.1:55438/postgres")
            .await
            .unwrap();
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
fn mail(h: &Harness, mailer: Arc<Mailer>) -> AppState {
    AppState::new(
        h.pool.clone(),
        Arc::new(Config {
            database_url: h.state.config.database_url.clone(),
            secret_key_base: h.state.config.secret_key_base.clone(),
            bind: h.state.config.bind,
            public_url: None,
            database_connections: 3,
            mailer: Some(mailer),
            demo: false,
        }),
    )
}
async fn events(h: &Harness, sink: i64) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM events WHERE sink_id=$1")
        .bind(sink)
        .fetch_one(&h.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn original_queue_jobs_deliver_events_mail_and_keep_future_work_until_due() {
    let h = Harness::new().await;
    let sink = h.sink("Queue").await;
    h.grant(h.user, sink).await;
    let q = Queue::new(&h).await;
    let mut ids = Vec::new();
    for index in 0..6 {
        ids.push(q.add(&h, sink, index, index, index == 2).await);
    }
    let inspection = queue::inspect(&q.pool).await.unwrap();
    assert_eq!((inspection.pending, inspection.due), (6, 5));
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_feeds"))
        .args(["queue", "inspect"])
        .env("QUEUE_DATABASE_URL", &q.url)
        .env_remove("SECRET_KEY_BASE")
        .env("PATH", "/nonexistent")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["pending"], 6);
    let smtp = Smtp::new(true, false).await;
    let state = mail(&h, smtp.mailer("none"));
    let report = queue::drain(&state, &q.pool).await.unwrap();
    assert_eq!(
        (
            report.completed,
            report.events,
            report.password_emails,
            report.remaining
        ),
        (5, 2, 1, 1)
    );
    assert_eq!(
        (
            report.obsolete_broadcasts,
            report.retired_cleanup_jobs,
            report.failure_count
        ),
        (1, 1, 0)
    );
    assert_eq!(events(&h, sink).await, 2);
    let props: Value = sqlx::query_scalar(
        "SELECT properties FROM events WHERE sink_id=$1 AND event_type='payment'",
    )
    .bind(sink)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(
        props,
        json!({"amount":125,"absent":{},"nested":{"list":[1,"two"]}})
    );
    let metadata: (String, String, String) = sqlx::query_as(
        "SELECT sdk_name,sdk_version,text FROM events WHERE sink_id=$1 AND event_type='payment'",
    )
    .bind(sink)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(
        metadata,
        (
            "reference".into(),
            "1.0".into(),
            "Accepted before cutover".into()
        )
    );
    assert_eq!(smtp.messages.lock().unwrap().len(), 1);
    sqlx::query("UPDATE solid_queue_jobs SET scheduled_at=(now() AT TIME ZONE 'UTC')-INTERVAL '1 second' WHERE id=$1").bind(ids[2]).execute(&q.pool).await.unwrap();
    q.add(&h, sink, 1, 6, false).await;
    let report = queue::drain(&state, &q.pool).await.unwrap();
    assert_eq!(
        (report.completed, report.remaining, report.failure_count),
        (2, 0, 0)
    );
    assert_eq!(events(&h, sink).await, 3);
    let unread: i32 = sqlx::query_scalar(
        "SELECT unread_count FROM sink_memberships WHERE user_id=$1 AND sink_id=$2",
    )
    .bind(h.user)
    .bind(sink)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(unread, 3);
    assert_eq!(queue::drain(&state, &q.pool).await.unwrap().completed, 0);
    assert_eq!(smtp.messages.lock().unwrap().len(), 1);
    q.finish(&h).await;
    h.finish().await;
}

#[tokio::test]
async fn primary_receipt_survives_queue_ack_failure_without_replaying_unkeyed_events() {
    let h = Harness::new().await;
    let sink = h.sink("Recovery").await;
    h.grant(h.user, sink).await;
    let q = Queue::new(&h).await;
    let id = q.add(&h, sink, 0, 0, false).await;
    q.pool.execute("DELETE FROM solid_queue_ready_executions; INSERT INTO solid_queue_claimed_executions (job_id,created_at) SELECT id,now() FROM solid_queue_jobs; CREATE FUNCTION reject_ack() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic acknowledgement failure'; END $$; CREATE TRIGGER reject_ack BEFORE UPDATE ON solid_queue_jobs FOR EACH ROW EXECUTE FUNCTION reject_ack()").await.unwrap();
    assert!(queue::drain(&h.state, &q.pool).await.is_err());
    assert_eq!(events(&h, sink).await, 1);
    assert_eq!(queue::inspect(&q.pool).await.unwrap().claimed, 1);
    q.pool
        .execute("DROP TRIGGER reject_ack ON solid_queue_jobs; DROP FUNCTION reject_ack()")
        .await
        .unwrap();
    let restarted = AppState::new(h.pool.clone(), h.state.config.clone());
    assert_eq!(
        queue::drain(&restarted, &q.pool).await.unwrap().completed,
        1
    );
    assert_eq!(events(&h, sink).await, 1);
    assert_eq!(queue::inspect(&q.pool).await.unwrap().pending, 0);
    let mut changed = q.arguments(id).await;
    changed["arguments"][1]["text"] = "different payload reusing job identity".into();
    q.replace(id, changed).await;
    sqlx::query("UPDATE solid_queue_jobs SET finished_at=NULL WHERE id=$1")
        .bind(id)
        .execute(&q.pool)
        .await
        .unwrap();
    let report = queue::drain(&h.state, &q.pool).await.unwrap();
    assert_eq!(
        report.failures[0].reason,
        "job_identity_reused_with_changed_payload"
    );
    assert_eq!(events(&h, sink).await, 1);
    q.finish(&h).await;
    h.finish().await;
}

#[tokio::test]
async fn unsupported_invalid_paused_and_running_work_remains_in_the_original_queue() {
    let h = Harness::new().await;
    let sink = h.sink("Invalid").await;
    let q = Queue::new(&h).await;
    let unsupported = q.add(&h, sink, 0, 0, false).await;
    let mut args = q.arguments(unsupported).await;
    args["job_class"] = "CustomJob".into();
    q.replace(unsupported, args).await;
    sqlx::query("UPDATE solid_queue_jobs SET class_name='CustomJob' WHERE id=$1")
        .bind(unsupported)
        .execute(&q.pool)
        .await
        .unwrap();
    let invalid = q.add(&h, sink, 0, 1, false).await;
    let mut args = q.arguments(invalid).await;
    args["arguments"][1]["text"] = "".into();
    q.replace(invalid, args).await;
    let oversized = q.add(&h, sink, 1, 2, false).await;
    let mut args = q.arguments(oversized).await;
    args["arguments"][1]["idempotency_key"] = "x".repeat(256).into();
    q.replace(oversized, args).await;
    q.pool.execute("DELETE FROM solid_queue_ready_executions; INSERT INTO solid_queue_failed_executions (job_id,error,created_at) SELECT id,'original failure retained',now() FROM solid_queue_jobs; INSERT INTO solid_queue_processes (kind,pid,hostname,name,last_heartbeat_at,created_at) VALUES ('Worker',1,'fixture','active-worker',now(),now())").await.unwrap();
    assert!(
        queue::drain(&h.state, &q.pool)
            .await
            .unwrap_err()
            .to_string()
            .contains("heartbeats")
    );
    q.pool.execute("DELETE FROM solid_queue_processes; INSERT INTO solid_queue_pauses (queue_name,created_at) VALUES ('events',now())").await.unwrap();
    assert!(
        queue::drain(&h.state, &q.pool)
            .await
            .unwrap_err()
            .to_string()
            .contains("paused")
    );
    q.pool
        .execute("DELETE FROM solid_queue_pauses")
        .await
        .unwrap();
    let report = queue::drain(&h.state, &q.pool).await.unwrap();
    assert_eq!(
        (report.completed, report.failure_count, report.remaining),
        (0, 3, 3)
    );
    assert_eq!(
        report
            .failures
            .iter()
            .map(|f| f.reason.as_str())
            .collect::<Vec<_>>(),
        [
            "unsupported_job",
            "invalid_event",
            "event_persistence_failed"
        ]
    );
    let errors: Vec<String> =
        sqlx::query_scalar("SELECT error FROM solid_queue_failed_executions ORDER BY job_id")
            .fetch_all(&q.pool)
            .await
            .unwrap();
    assert_eq!(errors, vec!["original failure retained"; 3]);
    assert_eq!(events(&h, sink).await, 0);
    q.finish(&h).await;
    h.finish().await;
}

#[tokio::test]
async fn queued_password_mail_is_retryable_when_smtp_is_missing_or_rejects_delivery() {
    let h = Harness::new().await;
    let sink = h.sink("Mail").await;
    let q = Queue::new(&h).await;
    q.add(&h, sink, 3, 0, false).await;
    assert_eq!(
        queue::drain(&h.state, &q.pool).await.unwrap().failures[0].reason,
        "smtp_not_configured"
    );
    let refused = Smtp::new(false, false).await;
    assert_eq!(
        queue::drain(&mail(&h, refused.mailer("none")), &q.pool)
            .await
            .unwrap()
            .failures[0]
            .reason,
        "smtp_delivery_failed"
    );
    let accepted = Smtp::new(true, false).await;
    assert_eq!(
        queue::drain(&mail(&h, accepted.mailer("none")), &q.pool)
            .await
            .unwrap()
            .password_emails,
        1
    );
    assert_eq!(accepted.messages.lock().unwrap().len(), 1);
    assert_eq!(queue::inspect(&q.pool).await.unwrap().pending, 0);
    q.finish(&h).await;
    h.finish().await;
}
