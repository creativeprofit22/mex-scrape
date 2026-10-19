//! Runs the mex-scrape HTTP API on `API_ADDR` (default `127.0.0.1:8080`).

use anyhow::Context as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let addr = std::env::var("API_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;
    tracing::info!(product = domain::PRODUCT_NAME, %addr, "api listening");

    let (router, _) = api::router();
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            // If the signal handler cannot be installed, serve until killed.
            if tokio::signal::ctrl_c().await.is_err() {
                std::future::pending::<()>().await;
            }
        })
        .await
        .context("server error")
}
