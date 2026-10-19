//! Background job runner for mex-scrape (price fetching, matching, history).
//!
//! Loads server secrets at start-up, then exits: jobs will be added in later
//! phases.

mod secrets;

use anyhow::Context as _;

use crate::secrets::ServerSecrets;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    // Fails closed on a malformed value; the error names the variable only.
    let secrets = ServerSecrets::from_env().context("loading server secrets")?;
    for (name, configured) in secrets.presence() {
        tracing::info!(secret = name, configured, "server secret");
    }
    let redactor = secrets.redactor();
    if let Err(error) = run(&secrets).await {
        let message = redactor.sanitize(&format!("{error:#}"));
        tracing::error!(error = %message, "worker failed");
        return Err(anyhow::anyhow!(message));
    }
    Ok(())
}

async fn run(_secrets: &ServerSecrets) -> anyhow::Result<()> {
    tracing::info!(
        product = domain::PRODUCT_NAME,
        "worker started; no jobs configured yet"
    );
    Ok(())
}
