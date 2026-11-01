use feeds::{import_postgres, schema};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("migration failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> schema::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--help"] {
        println!(
            "Usage: feeds-import-postgres --source-stopped --output NEW.sqlite3 [--no-legacy-queue] [--rehearsal]\n\nReads POSTGRES_DATABASE_URL and verifies QUEUE_DATABASE_URL has no unfinished jobs.\nUse --no-legacy-queue only for an installation whose Rails queue was already retired.\nUse --rehearsal on an isolated restored backup to test conversion before resolving\nthe original queue. Its output is a rehearsal snapshot, not ready for cutover.\nThe source is read-only. Existing output files are never overwritten."
        );
        return Ok(());
    }
    let output = match args.as_slice() {
        [stopped, flag, output, options @ ..]
            if stopped == "--source-stopped"
                && flag == "--output"
                && options.iter().all(|option| {
                    matches!(option.as_str(), "--no-legacy-queue" | "--rehearsal")
                }) =>
        {
            output
        }
        _ => {
            return Err(
                "stop all source writers and use --source-stopped --output NEW.sqlite3; see --help"
                    .into(),
            );
        }
    };
    let source =
        std::env::var("POSTGRES_DATABASE_URL").map_err(|_| "POSTGRES_DATABASE_URL is required")?;
    let queue = std::env::var("QUEUE_DATABASE_URL").ok();
    if queue.is_none() && !args.iter().any(|a| a == "--no-legacy-queue") {
        return Err(
            "QUEUE_DATABASE_URL is required unless the legacy queue has already been retired"
                .into(),
        );
    }
    let rehearsal = args.iter().any(|a| a == "--rehearsal");
    let inspection = if let Some(queue) = queue {
        let pool = feeds::postgres::connect(&queue, 1, false).await?;
        let inspection = feeds::queue::inspect(&pool).await?;
        pool.close().await;
        if !rehearsal && (inspection.pending != 0 || inspection.live_processes != 0) {
            return Err("legacy queue still has unfinished jobs or live workers; resolve it before migration".into());
        }
        Some(inspection)
    } else {
        None
    };
    if rehearsal {
        eprintln!(
            "REHEARSAL ONLY: this snapshot is not ready for cutover; the legacy queue has not been resolved."
        );
    }
    let report = import_postgres::migrate(&source, std::path::Path::new(output)).await?;
    if rehearsal {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "rehearsal_only": true,
                "source_queue": inspection,
                "migration": report
            }))?
        );
    } else {
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}
