//! One-time Solid Queue handoff, not a resident job runner. Original queue rows
//! remain available for Rails rollback; receipts prevent replaying imported events.
use crate::{
    AppState,
    events::{self, EventInput},
    schema::Result,
};
use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool};

#[derive(Serialize, sqlx::FromRow)]
pub struct ClassCount {
    pub class_name: String,
    pub pending: i64,
}
#[derive(Serialize)]
pub struct Inspection {
    pub pending: i64,
    pub due: i64,
    pub failed: i64,
    pub claimed: i64,
    pub live_processes: i64,
    pub paused_queues: i64,
    pub classes: Vec<ClassCount>,
}
pub async fn inspect(pool: &PgPool) -> Result<Inspection> {
    let classes: Vec<ClassCount> = sqlx::query_as("SELECT class_name,count(*) AS pending FROM solid_queue_jobs WHERE finished_at IS NULL GROUP BY class_name ORDER BY class_name")
        .fetch_all(pool).await?;
    let due = sqlx::query_scalar("SELECT count(*) FROM solid_queue_jobs WHERE finished_at IS NULL AND (scheduled_at IS NULL OR scheduled_at<=now() AT TIME ZONE 'UTC')").fetch_one(pool).await?;
    let failed = sqlx::query_scalar("SELECT count(*) FROM solid_queue_failed_executions f JOIN solid_queue_jobs j ON j.id=f.job_id WHERE j.finished_at IS NULL").fetch_one(pool).await?;
    let claimed = sqlx::query_scalar("SELECT count(*) FROM solid_queue_claimed_executions c JOIN solid_queue_jobs j ON j.id=c.job_id WHERE j.finished_at IS NULL").fetch_one(pool).await?;
    let live_processes = sqlx::query_scalar("SELECT count(*) FROM solid_queue_processes WHERE last_heartbeat_at > (now() AT TIME ZONE 'UTC') - INTERVAL '60 seconds'").fetch_one(pool).await?;
    let paused_queues = sqlx::query_scalar("SELECT count(*) FROM solid_queue_pauses")
        .fetch_one(pool)
        .await?;
    Ok(Inspection {
        pending: classes.iter().map(|c| c.pending).sum(),
        due,
        failed,
        claimed,
        live_processes,
        paused_queues,
        classes,
    })
}

#[derive(sqlx::FromRow)]
struct Job {
    id: i64,
    class_name: String,
    active_job_id: Option<String>,
    arguments: Option<String>,
}
enum Work {
    Event(i64, Value),
    Password(i64),
    Broadcast,
    Maintenance,
}
impl Work {
    fn kind(&self) -> &'static str {
        match self {
            Self::Event(..) => "event",
            Self::Password(..) => "password",
            Self::Broadcast => "broadcast",
            Self::Maintenance => "maintenance",
        }
    }
}

fn gid(value: &Value, model: &str) -> Option<i64> {
    value
        .get("_aj_globalid")?
        .as_str()?
        .strip_prefix(&format!("gid://telesink/{model}/"))?
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
}
fn clean(value: &Value) -> std::result::Result<Value, &'static str> {
    match value {
        Value::Object(map) => {
            if let Some(serializer) = map.get("_aj_serialized") {
                return match serializer.as_str() {
                    Some(
                        "ActiveJob::Serializers::TimeSerializer"
                        | "ActiveJob::Serializers::DateTimeSerializer"
                        | "ActiveJob::Serializers::DateSerializer"
                        | "ActiveJob::Serializers::SymbolSerializer",
                    ) => map
                        .get("value")
                        .filter(|v| v.is_string())
                        .cloned()
                        .ok_or("invalid_serialized_value"),
                    _ => Err("unsupported_argument_serializer"),
                };
            }
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            let mut output = serde_json::Map::new();
            for key in keys {
                if !matches!(
                    key.as_str(),
                    "_aj_symbol_keys" | "_aj_ruby2_keywords" | "_aj_hash_with_indifferent_access"
                ) {
                    output.insert(key.clone(), clean(&map[key])?);
                }
            }
            Ok(output.into())
        }
        Value::Array(values) => values
            .iter()
            .map(clean)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map(Value::Array),
        _ => Ok(value.clone()),
    }
}
fn decode(job: &Job) -> std::result::Result<(String, String, Work), &'static str> {
    let envelope: Value =
        serde_json::from_str(job.arguments.as_deref().ok_or("missing_arguments")?)
            .map_err(|_| "invalid_arguments")?;
    if envelope["job_class"] != job.class_name {
        return Err("job_class_mismatch");
    }
    let id = envelope["job_id"]
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 255)
        .ok_or("missing_job_identity")?;
    if job.active_job_id.as_deref() != Some(id) {
        return Err("job_identity_mismatch");
    }
    let arguments = clean(envelope.get("arguments").ok_or("missing_arguments")?)?;
    let values = arguments.as_array().ok_or("invalid_arguments")?;
    let work = match (job.class_name.as_str(), values.as_slice()) {
        ("ProcessEventJob", [sink, payload]) if payload.is_object() => Work::Event(
            gid(sink, "Sink").ok_or("invalid_sink_reference")?,
            payload.clone(),
        ),
        ("ActionMailer::MailDeliveryJob", [mailer, method, delivery, options])
            if mailer == "PasswordsMailer" && method == "reset" && delivery == "deliver_now" =>
        {
            let args = options
                .get("args")
                .and_then(Value::as_array)
                .filter(|a| a.len() == 1)
                .ok_or("unsupported_mail_arguments")?;
            Work::Password(gid(&args[0], "User").ok_or("invalid_user_reference")?)
        }
        (
            "Turbo::Streams::BroadcastStreamJob"
            | "Turbo::Streams::BroadcastJob"
            | "Turbo::Streams::ActionBroadcastJob",
            _,
        ) => Work::Broadcast,
        ("SolidQueue::RecurringJob", [command])
            if command
                == "SolidQueue::Job.clear_finished_in_batches(sleep_between_batches: 0.3)" =>
        {
            Work::Maintenance
        }
        _ => return Err("unsupported_job"),
    };
    let hash = Sha256::digest(format!("{}\n{arguments}", job.class_name).as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok((id.into(), hash, work))
}

#[derive(Debug, Serialize)]
pub struct JobFailure {
    pub id: i64,
    pub class_name: String,
    pub reason: String,
}
#[derive(Debug, Serialize, Default)]
pub struct DrainReport {
    pub completed: u64,
    pub events: u64,
    pub password_emails: u64,
    pub obsolete_broadcasts: u64,
    pub retired_cleanup_jobs: u64,
    pub failures: Vec<JobFailure>,
    pub failure_count: u64,
    pub remaining: i64,
}

/// Caller must explicitly confirm stopped Rails web/worker/scheduler processes.
/// Fresh queue heartbeats and paused queues additionally prevent the handoff.
pub async fn drain(state: &AppState, queue: &PgPool) -> Result<DrainReport> {
    let inspection = inspect(queue).await?;
    if inspection.live_processes > 0 {
        return Err(
            "Rails queue processes still have fresh heartbeats; stop them before draining".into(),
        );
    }
    if inspection.paused_queues > 0 {
        return Err("resume paused queues in Rails before draining".into());
    }
    crate::schema::check(&state.pool).await?;
    let mut report = DrainReport::default();
    let mut cursor = 0i64;
    loop {
        let mut queued = queue.begin().await?;
        // Only one native importer drains this queue at a time, and each job's
        // queue lock is held until its primary commit has been acknowledged.
        sqlx::query("SELECT pg_advisory_xact_lock(1413829460, 3)")
            .execute(&mut *queued)
            .await?;
        let job:Option<Job>=sqlx::query_as("SELECT id,class_name,active_job_id,arguments FROM solid_queue_jobs WHERE finished_at IS NULL AND id>$1 AND (scheduled_at IS NULL OR scheduled_at<=now() AT TIME ZONE 'UTC') ORDER BY id LIMIT 1 FOR UPDATE")
            .bind(cursor).fetch_optional(&mut *queued).await?;
        let Some(job) = job else {
            break;
        };
        cursor = job.id;
        let result = match decode(&job) {
            Ok((id, hash, work)) => import(state, &id, &hash, &work).await.map(|_| work.kind()),
            Err(error) => Err(error),
        };
        match result {
            Ok(kind) => {
                acknowledge(&mut queued, job.id).await?;
                queued.commit().await?;
                report.completed += 1;
                match kind {
                    "event" => report.events += 1,
                    "password" => report.password_emails += 1,
                    "broadcast" => report.obsolete_broadcasts += 1,
                    _ => report.retired_cleanup_jobs += 1,
                }
            }
            Err(error) => {
                report.failure_count += 1;
                if report.failures.len() < 100 {
                    report.failures.push(JobFailure {
                        id: job.id,
                        class_name: job.class_name,
                        reason: error.into(),
                    });
                }
                // Dropping the transaction preserves the original job and its
                // ready/claimed/failed state for inspection or a later retry.
            }
        }
    }
    report.remaining =
        sqlx::query_scalar("SELECT count(*) FROM solid_queue_jobs WHERE finished_at IS NULL")
            .fetch_one(queue)
            .await?;
    Ok(report)
}

async fn import(
    state: &AppState,
    id: &str,
    hash: &str,
    work: &Work,
) -> std::result::Result<(), &'static str> {
    let mut tx = state
        .pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|_| "primary_unavailable")?;
    let inserted=sqlx::query("INSERT INTO telesink_imported_jobs (active_job_id,payload_hash,kind,imported_at) VALUES ($1,$2,$3,now()) ON CONFLICT DO NOTHING")
        .bind(id).bind(hash).bind(work.kind()).execute(&mut *tx).await.map_err(|_|"receipt_failed")?.rows_affected();
    if inserted == 0 {
        let previous: String = sqlx::query_scalar(
            "SELECT payload_hash FROM telesink_imported_jobs WHERE active_job_id=$1",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| "receipt_failed")?;
        return if previous == hash {
            Ok(())
        } else {
            Err("job_identity_reused_with_changed_payload")
        };
    }
    match work {
        Work::Event(sink, payload) => {
            let event = EventInput::from_params(payload, Utc::now().naive_utc())
                .map_err(|_| "invalid_event")?;
            events::persist_in(&mut tx, *sink, &event)
                .await
                .map_err(|_| "event_persistence_failed")?;
        }
        Work::Password(user_id) => {
            let mailer = state.config.mailer.as_ref().ok_or("smtp_not_configured")?;
            let user: Option<(String, String)> =
                sqlx::query_as("SELECT email_address,password_digest FROM users WHERE id=$1")
                    .bind(user_id)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(|_| "primary_unavailable")?;
            let (email, digest) = user.ok_or("user_no_longer_exists")?;
            let token = state
                .tokens
                .issue_reset(*user_id, &digest, Utc::now())
                .ok_or("invalid_password_digest")?;
            mailer
                .password_reset(&email, &token)
                .await
                .map_err(|_| "smtp_delivery_failed")?;
        }
        Work::Broadcast | Work::Maintenance => {}
    }
    tx.commit().await.map_err(|_| "primary_commit_failed")?;
    Ok(())
}

async fn acknowledge(connection: &mut PgConnection, id: i64) -> Result<()> {
    for table in [
        "solid_queue_ready_executions",
        "solid_queue_claimed_executions",
        "solid_queue_failed_executions",
        "solid_queue_scheduled_executions",
        "solid_queue_blocked_executions",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE job_id=$1"
        )))
        .bind(id)
        .execute(&mut *connection)
        .await?;
    }
    sqlx::query("UPDATE solid_queue_jobs SET finished_at=now(),updated_at=now() WHERE id=$1")
        .bind(id)
        .execute(connection)
        .await?;
    Ok(())
}
