//! Rendition — open source enterprise media CDN
//!
//! Entry point: initialises logging, loads configuration from `RENDITION_*`
//! environment variables, and starts the Axum HTTP server.  Application
//! logic lives in the `rendition` library crate.

use rendition::config::AppConfig;
use std::process::ExitCode;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rendition=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Fail-fast configuration load. Any error must prevent the process from
    // binding its listeners — there is no graceful degradation for
    // misconfiguration (FR-02).
    let config = match AppConfig::load() {
        Ok(c) => c,
        Err(err) => {
            tracing::error!("configuration error: {err}");
            return ExitCode::from(1);
        }
    };

    // Custom Debug impl on AppConfig redacts sensitive fields.
    tracing::info!("Loaded config: {config:?}");

    let app = match rendition::build_app(&config).await {
        Ok(app) => app,
        Err(err) => {
            tracing::error!("failed to build application: {err}");
            return ExitCode::from(1);
        }
    };

    let bind_addr = config.bind_addr;
    tracing::info!("Rendition CDN listening on {bind_addr}");

    let listener = match tokio::net::TcpListener::bind(bind_addr).await {
        Ok(l) => l,
        Err(err) => {
            tracing::error!("failed to bind {bind_addr}: {err}");
            return ExitCode::from(1);
        }
    };
    if let Err(err) = axum::serve(listener, app).await {
        tracing::error!("server terminated with error: {err}");
        return ExitCode::from(1);
    }

    ExitCode::SUCCESS
}
