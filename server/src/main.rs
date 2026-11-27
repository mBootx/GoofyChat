mod account;
mod api;
mod auth;
mod error;
mod friends;
mod imaging;
mod internal;
mod links;
mod media;
mod mentions;
mod messages;
mod music;
mod photos;
mod presence;
mod profiles;
mod state;
mod stories;
mod ws;

#[cfg(test)]
mod tests;

use std::net::SocketAddr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let config = state::Config::from_env();
    let addr: SocketAddr = std::env::var("BIND")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()?;
    let state = state::AppState::new(config).await?;
    let janitor = state.clone();
    tokio::spawn(async move {
        let mut every = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            every.tick().await;
            if let Err(e) = media::cleanup(&janitor).await {
                tracing::error!("cleanup: {}", e.1);
            }
        }
    });
    tokio::spawn(music::activity_loop(state.clone()));
    tokio::spawn(stories::expire_loop(state.clone()));
    if state.has_worker() {
        // The stories worker's API, on its own port that compose never publishes.
        let worker_addr: SocketAddr = state.config.worker_bind.parse()?;
        let worker_listener = tokio::net::TcpListener::bind(worker_addr).await?;
        let internal = api::internal_router(state.clone());
        tracing::info!("worker API on {worker_addr}");
        tokio::spawn(async move {
            if let Err(e) = axum::serve(worker_listener, internal).await {
                tracing::error!("worker API: {e}");
            }
        });
    } else {
        tracing::info!("WORKER_TOKEN is empty: no video stories, archives stay uncompressed");
    }
    let app = api::router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("listening on {addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

#[cfg(unix)]
async fn shutdown() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("signal handler");
    term.recv().await;
}

/// Development builds on other systems.
#[cfg(not(unix))]
async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}
