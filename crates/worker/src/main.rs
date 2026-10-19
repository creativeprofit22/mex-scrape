//! Background job runner for mex-scrape (price fetching, matching, history).
//!
//! Placeholder: logs start-up and exits. Jobs will be added in later phases.

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    tracing::info!(
        product = domain::PRODUCT_NAME,
        "worker started; no jobs configured yet"
    );
}
