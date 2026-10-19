//! Background job runner for mex-scrape (price fetching, matching, history).
//!
//! Loads server secrets at start-up and, when `DATABASE_URL` is set,
//! recovers interrupted Bright Data runs and resumes the kept ones; then
//! exits: jobs will be added in later phases.

mod brightdata_resume;
mod brightdata_store;
mod secrets;

use anyhow::Context as _;
use brightdata::BrightDataClient;
use brightdata::recovery::recover_interrupted;
use brightdata::runs::{Caps, Runner, SystemClock, Timing};
use db::DatabaseUrl;
use db::brightdata_runs::DriverLock;
use http_guard::Redactor;

use crate::brightdata_resume::{RefusingSink, client, resume_kept};
use crate::brightdata_store::PgRunStore;
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

async fn run(secrets: &ServerSecrets) -> anyhow::Result<()> {
    // Errors name the variable only, never the value.
    let database = DatabaseUrl::from_env().context("loading DATABASE_URL")?;
    // Held until the worker exits: this process drives Bright Data runs.
    let _driver = match &database {
        Some(url) => {
            let redactor = secrets.redactor();
            let (api_key, unlocker_zone) = secrets.brightdata();
            // `None` without credentials: resumes then fail the key gate.
            let client = client(api_key, unlocker_zone, &redactor);
            recover_brightdata_runs(url, client.as_ref(), &redactor)
                .await
                .map_err(|error| anyhow::anyhow!(url.redactor().sanitize(&format!("{error:#}"))))?
        }
        None => {
            tracing::info!("DATABASE_URL is not set; Bright Data run recovery skipped");
            None
        }
    };
    tracing::info!(
        product = domain::PRODUCT_NAME,
        "worker started; no jobs configured yet"
    );
    Ok(())
}

/// Takes the driver lock, fails or keeps interrupted Bright Data runs and
/// resumes the kept ones. Skips both when another worker holds the lock:
/// its runs are live.
async fn recover_brightdata_runs(
    url: &DatabaseUrl,
    transport: Option<&BrightDataClient>,
    redactor: &Redactor,
) -> anyhow::Result<Option<DriverLock>> {
    let pool = db::connect(url)
        .await
        .context("failed to connect to Postgres")?;
    let Some(lock) = db::brightdata_runs::try_driver_lock(&pool)
        .await
        .context("taking the Bright Data driver lock")?
    else {
        tracing::info!("another worker drives Bright Data runs; recovery skipped");
        return Ok(None);
    };
    let store = PgRunStore::new(pool, url.redactor().clone());
    let report = recover_interrupted(&store, &SystemClock)
        .await
        .context("recovering interrupted Bright Data runs")?;
    tracing::info!(
        failed = report.failed,
        kept = report.kept.len(),
        "Bright Data run recovery finished"
    );
    if !report.kept.is_empty() {
        let runner = Runner {
            transport,
            store: &store,
            clock: &SystemClock,
            timing: Timing::default(),
            caps: Caps::default(),
        };
        // Still under the driver lock.
        let resumed = resume_kept(&runner, &report.kept, &RefusingSink, redactor).await;
        tracing::info!(
            finished = resumed.finished,
            active = resumed.active,
            refused = resumed.refused,
            "Bright Data run resume finished"
        );
    }
    Ok(Some(lock))
}
