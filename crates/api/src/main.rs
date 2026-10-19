//! Runs the mex-scrape HTTP API on `API_ADDR` (default `127.0.0.1:8080`).

use anyhow::Context as _;

/// Admin routes need `ADMIN_API_TOKEN` and `DATABASE_URL`; without both they
/// answer 503. Errors name the variable only.
async fn admin_state() -> api::AdminState {
    let token = std::env::var("ADMIN_API_TOKEN").unwrap_or_default();
    if token.len() < api::MIN_TOKEN_LEN {
        tracing::info!("ADMIN_API_TOKEN is unset or too short; admin API disabled");
        return api::AdminState::disabled();
    }
    let url = match db::DatabaseUrl::from_env() {
        Ok(Some(url)) => url,
        Ok(None) => {
            tracing::info!("DATABASE_URL is not set; admin API disabled");
            return api::AdminState::disabled();
        }
        Err(error) => {
            tracing::warn!(%error, "DATABASE_URL is invalid; admin API disabled");
            return api::AdminState::disabled();
        }
    };
    match db::connect(&url).await {
        Ok(pool) => api::AdminState::new(&token, std::sync::Arc::new(api::PgAccessSource(pool))),
        Err(error) => {
            let detail = url.redactor().sanitize(&error.to_string());
            tracing::warn!(error = %detail, "database unavailable; admin API disabled");
            api::AdminState::disabled()
        }
    }
}

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

    let (router, _) = api::router_with(admin_state().await);
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
