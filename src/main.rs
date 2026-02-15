#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod config;
mod handlers;
mod rate_limiter;
mod security;
mod state;
mod template;
mod treasury;
mod usd_idr;
mod utils;
mod ws_manager;

use axum::{middleware as axum_middleware, Router};
use std::sync::Arc;
use tokio::signal;
use tower_http::compression::CompressionLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .compact()
        .init();

    let state = Arc::new(AppState::new());

    let s = state.clone();
    tokio::spawn(async move { treasury::treasury_ws_loop(s).await });

    let s = state.clone();
    tokio::spawn(async move { usd_idr::usd_idr_loop(s).await });

    let s = state.clone();
    tokio::spawn(async move { ws_manager::heartbeat_loop(s).await });

    let app = Router::new()
        .merge(handlers::routes())
        .layer(CompressionLayer::new().gzip(true).br(true).deflate(true))
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            security::security_middleware,
        ))
        .with_state(state);

    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "10000".into())
        .parse()
        .unwrap_or(10000);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .unwrap();

    info!("Server starting on 0.0.0.0:{}", port);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();
}

async fn shutdown_signal() {
    let ctrl_c = async { signal::ctrl_c().await.unwrap() };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .unwrap()
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
