mod code_graph;
mod code_index;
mod config;
mod context;
mod memory;
mod rocky;
mod routes;
mod storage;

use anyhow::Context;
use axum::Router;
use config::Config;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tracing::info;

#[derive(Clone)]
struct AppState {
    config: Config,
    watchers: Arc<Mutex<HashSet<String>>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "xenonite_rs=info,tower_http=info".into()),
        )
        .init();

    let config = Config::load().context("load configuration")?;
    let state = AppState {
        config: config.clone(),
        watchers: Arc::new(Mutex::new(HashSet::new())),
    };
    let app: Router = routes::router(state);
    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    let listener = TcpListener::bind(addr).await.context("bind API listener")?;

    info!("[xenonite] API listening on :{}", config.port);
    info!(
        "[xenonite] rocky LLM: {} | embeddings: {}",
        config.rocky.llm_base_url, config.rocky.embed_base_url
    );
    info!("[xenonite] data: {}", config.data_dir.display());

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serve API")
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
