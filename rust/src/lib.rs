pub mod activity;
mod activity_tabs;
pub mod assets;
pub mod auth;
pub mod backup;
pub mod browser;
pub mod columns;
pub mod config;
pub mod demo;
pub mod event_types;
pub mod events;
pub mod feed;
pub mod feed_view;
pub mod folders;
#[cfg(feature = "postgres-import")]
pub mod import_postgres;
pub mod live;
pub mod mail;
pub mod owner;
pub mod params;
#[cfg(feature = "postgres-import")]
pub mod postgres;
#[cfg(feature = "postgres-import")]
pub mod queue;
pub mod saved_views;
pub mod schema;
pub mod session;
pub mod settings;
pub mod sinks;
pub mod sqlite_functions;
pub mod tokens;
pub mod view;

use std::sync::Arc;

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use sqlx::SqlitePool;
use tower_http::cors::{Any, CorsLayer};

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: Arc<config::Config>,
    pub signer: Arc<session::SessionSigner>,
    pub tokens: Arc<tokens::RailsTokens>,
    pub live: Arc<live::Hub>,
}

impl AppState {
    pub fn new(pool: SqlitePool, config: Arc<config::Config>) -> Self {
        let signer = Arc::new(session::SessionSigner::new(
            config.secret_key_base.as_bytes(),
        ));
        let tokens = Arc::new(tokens::RailsTokens::new(config.secret_key_base.as_bytes()));
        Self {
            pool,
            config,
            signer,
            tokens,
            live: Arc::new(live::Hub::default()),
        }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/up", get(|| async { StatusCode::OK }))
        .route("/ready", get(ready))
        .route("/cable", get(live::upgrade))
        .route(
            "/api/v1/feeds/{token}/events",
            post(events::ingest).layer(
                CorsLayer::new()
                    .allow_origin(Any)
                    .allow_methods([axum::http::Method::POST, axum::http::Method::OPTIONS])
                    .allow_headers([
                        axum::http::header::CONTENT_TYPE,
                        axum::http::HeaderName::from_static("idempotency-key"),
                        axum::http::header::USER_AGENT,
                        axum::http::header::ACCEPT,
                    ])
                    .max_age(std::time::Duration::from_secs(7200)),
            ),
        )
        .route(
            "/api/v1/sinks/{token}/events",
            post(events::ingest).layer(
                CorsLayer::new()
                    .allow_origin(Any)
                    .allow_methods([axum::http::Method::POST, axum::http::Method::OPTIONS])
                    .allow_headers([
                        axum::http::header::CONTENT_TYPE,
                        axum::http::HeaderName::from_static("idempotency-key"),
                        axum::http::header::USER_AGENT,
                        axum::http::header::ACCEPT,
                    ])
                    .max_age(std::time::Duration::from_secs(7200)),
            ),
        )
        .fallback(browser::handle)
        .with_state(state)
}

async fn ready(State(state): State<AppState>) -> StatusCode {
    if !state.live.is_ready() {
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    match sqlx::query("SELECT 1").execute(&state.pool).await {
        Ok(_) => StatusCode::OK,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}
