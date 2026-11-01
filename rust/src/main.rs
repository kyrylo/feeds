use feeds::{AppState, config::Config, schema};
use std::sync::Arc;

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(2)
        .enable_all()
        .build()
        .expect("Tokio runtime");
    if let Err(error) = runtime.block_on(run()) {
        eprintln!("feeds: {error}");
        std::process::exit(1);
    }
}

async fn run() -> schema::Result<()> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let command: Vec<_> = arguments.iter().map(String::as_str).collect();
    let environment = feeds::config::setting("FEEDS_ENV")
        .or_else(|_| std::env::var("RAILS_ENV"))
        .unwrap_or_else(|_| "production".into());
    match command.as_slice() {
        ["--help" | "-h" | "help"] => {
            println!(
                "Feeds\n\n  feeds [serve]       Prepare SQLite and run HTTP + live updates\n  feeds hash-password Print an owner password hash (reads stdin)\n  feeds reset-password\n                        Reset the existing owner password (reads stdin)\n  feeds db prepare    Create the SQLite file if missing and apply native upgrades\n  feeds db check      Read-only schema and integrity check\n  feeds db backup PATH\n                        Create a verified snapshot in a new file\n  feeds healthcheck   Check the running service's database and live relay\n\nDATABASE_URL defaults to sqlite://storage/feeds.sqlite3. Serving also requires\nSECRET_KEY_BASE. See rust/README.md for Kamal, backups and runtime settings."
            );
            #[cfg(feature = "postgres-import")]
            println!(
                "\nMigration build only:\n  feeds queue inspect\n  feeds queue drain --rails-stopped\nQueue commands also require QUEUE_DATABASE_URL."
            );
            return Ok(());
        }
        ["--version"] => {
            println!("feeds {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        ["healthcheck"] => return healthcheck().await,
        ["hash-password"] => {
            let password = feeds::owner::read_password(std::io::stdin().lock())?;
            println!("{}", feeds::owner::hash_password(&password)?);
            return Ok(());
        }
        ["reset-password"] => {
            let password = feeds::owner::read_password(std::io::stdin().lock())?;
            let digest = feeds::owner::hash_password(&password)?;
            let pool = schema::connect(&schema::database_url(), 1, false).await?;
            schema::check(&pool).await?;
            feeds::owner::reset_password(&pool, &digest).await?;
            pool.close().await;
            println!("Owner password updated. All sessions signed out.");
            return Ok(());
        }
        ["db", "backup", destination] => {
            feeds::backup::create(&schema::database_url(), std::path::Path::new(destination))
                .await?;
            println!("Verified backup created at {destination}.");
            return Ok(());
        }
        #[cfg(feature = "postgres-import")]
        ["queue", "inspect"] => {
            let url = std::env::var("QUEUE_DATABASE_URL")
                .map_err(|_| "QUEUE_DATABASE_URL is required")?;
            let queue = feeds::postgres::connect(&url, 1, false).await?;
            println!(
                "{}",
                serde_json::to_string(&feeds::queue::inspect(&queue).await?)?
            );
            queue.close().await;
            return Ok(());
        }
        #[cfg(feature = "postgres-import")]
        ["queue", "drain", "--rails-stopped"] => {
            let config = Arc::new(Config::from_env()?);
            let url = std::env::var("QUEUE_DATABASE_URL")
                .map_err(|_| "QUEUE_DATABASE_URL is required")?;
            let primary = schema::connect(&config.database_url, 2, false).await?;
            schema::check(&primary).await?;
            let queue = feeds::postgres::connect(&url, 1, false).await?;
            let report =
                feeds::queue::drain(&AppState::new(primary.clone(), config), &queue).await?;
            println!("{}", serde_json::to_string(&report)?);
            queue.close().await;
            primary.close().await;
            if report.remaining != 0 {
                return Err("queue still contains unfinished jobs; see the report".into());
            }
            return Ok(());
        }
        #[cfg(feature = "postgres-import")]
        ["queue", "drain"] => return Err(
            "stop Rails web, worker and scheduler processes, then use queue drain --rails-stopped"
                .into(),
        ),
        ["db", action @ ("prepare" | "check")] => {
            let url = schema::database_url();
            let pool = if *action == "prepare" {
                schema::connect(&url, 1, true).await?
            } else {
                schema::connect_read_only(&url).await?
            };
            if *action == "prepare" {
                println!(
                    "{}",
                    serde_json::to_string(&schema::prepare(&pool, &environment).await?)?
                );
            } else {
                schema::check(&pool).await?;
                println!("Schema {} is compatible.", schema::CURRENT_VERSION);
            }
            pool.close().await;
            return Ok(());
        }
        [] | ["serve"] => {}
        _ => return Err("unknown command; run feeds --help".into()),
    }
    let config = Arc::new(Config::from_env()?);
    let pool = schema::connect(&config.database_url, config.database_connections, true).await?;
    schema::prepare(&pool, if config.demo { "demo" } else { &environment }).await?;
    if !config.demo {
        feeds::owner::bootstrap(
            &pool,
            feeds::config::setting("FEEDS_PASSWORD_HASH")
                .ok()
                .as_deref(),
            feeds::config::setting("FEEDS_OWNER_EMAIL").ok().as_deref(),
        )
        .await?;
    }
    let state = AppState::new(pool.clone(), config.clone());
    let _relay = state.live.start(&config.database_url, pool.clone()).await?;
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    eprintln!("Feeds listening on {}", config.bind);
    let app = feeds::router(state);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await?;
    pool.close().await;
    Ok(())
}

async fn shutdown() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("SIGTERM handler");
        tokio::select! { _=tokio::signal::ctrl_c()=>{}, _=terminate.recv()=>{} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

async fn healthcheck() -> schema::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mut address: std::net::SocketAddr = feeds::config::setting("FEEDS_BIND")
        .unwrap_or_else(|_| "127.0.0.1:3000".into())
        .parse()?;
    if address.ip().is_unspecified() {
        address.set_ip(if address.is_ipv4() {
            std::net::Ipv4Addr::LOCALHOST.into()
        } else {
            std::net::Ipv6Addr::LOCALHOST.into()
        });
    }
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        let mut stream = tokio::net::TcpStream::connect(address).await?;
        stream
            .write_all(
                format!("GET /ready HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await?;
        let mut status = String::new();
        BufReader::new(stream).read_line(&mut status).await?;
        if !status.starts_with("HTTP/1.1 200 ") {
            return Err("service is not ready".into());
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    })
    .await?
}
