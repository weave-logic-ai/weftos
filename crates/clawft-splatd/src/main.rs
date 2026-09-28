//! splatd — the weft-splat job server.
//!
//! Env:
//!   SPLATD_CONFIG  path to splatd.toml   (default ./config/splatd.toml, falls back to defaults)
//!   SPLATD_ADDR    bind address          (default 127.0.0.1:7860)
//!   RUST_LOG       tracing filter       (default info)

mod api;
mod state;
mod worker;

use clawft_splat_pipeline::PipelineConfig;
use state::AppState;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Instant;
use tokio::sync::{Mutex, mpsc};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let config_path = std::env::var("SPLATD_CONFIG").unwrap_or_else(|_| "./config/splatd.toml".into());
    let cfg = match PipelineConfig::load(std::path::Path::new(&config_path)) {
        Ok(cfg) => {
            tracing::info!(%config_path, "loaded config");
            cfg
        }
        Err(e) => {
            tracing::warn!(%config_path, error = %e, "config not loaded, using defaults");
            PipelineConfig::default()
        }
    };
    let mut cfg = cfg;
    cfg.absolutize(); // data_dir → absolute; stages run with cwd=job root (see config.rs)
    tracing::info!(data_dir = %cfg.data_dir.display(), "resolved data_dir");
    std::fs::create_dir_all(cfg.data_dir.join("jobs"))?;

    let (tx, rx) = mpsc::unbounded_channel();
    let state = Arc::new(AppState {
        cfg,
        jobs: Mutex::new(Default::default()),
        tx,
        started: Instant::now(),
        jobs_done: AtomicU64::new(0),
        jobs_failed: AtomicU64::new(0),
        queue_depth: AtomicU64::new(0),
    });
    state.rehydrate().await;

    tokio::spawn(worker::worker_loop(state.clone(), rx));

    let addr = std::env::var("SPLATD_ADDR").unwrap_or_else(|_| "127.0.0.1:7860".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "splatd listening");
    axum::serve(listener, api::router(state)).await?;
    Ok(())
}
