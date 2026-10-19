//! Background job runner for mex-scrape (price fetching, matching, history).
//!
//! Loads server secrets at start-up and, when `DATABASE_URL` is set,
//! recovers interrupted Bright Data runs and resumes the kept ones. It also
//! builds the Amazon.com.mx source (SP-API, Bright Data fallback) and logs
//! which parts are configured; then exits: jobs will be added in later
//! phases.

mod access_metrics;
mod amazon_source;
mod brightdata_resume;
mod brightdata_store;
mod offer_matching;
mod secrets;

use std::sync::Arc;

use anyhow::Context as _;
use app::AccessReport;
use brightdata::BrightDataClient;
use brightdata::recovery::recover_interrupted;
use brightdata::robots::{HttpRobotsSource, RobotsChecker};
use brightdata::runs::{Caps, Runner, SystemClock, Timing};
use db::DatabaseUrl;
use db::access::PgAccessCounts;
use db::brightdata_runs::DriverLock;
use http_guard::Redactor;
use retailers::RetailerOfferSource;

use crate::amazon_source::{amazon_source, driver_fallback};
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
    // Read now so a malformed value stops the worker before any job runs;
    // the future store-fetch job owns a `ParseHealth` built from it and
    // passes it to `RetailerOfferSource::new`.
    let parse_health = parse_health_config().context("loading retailer parse health settings")?;
    tracing::info!(
        threshold = parse_health.threshold,
        min_samples = parse_health.min_samples,
        window = parse_health.window,
        "retailer parse health alerts"
    );
    // Errors name the variable only, never the value.
    let database = DatabaseUrl::from_env().context("loading DATABASE_URL")?;
    let redactor = secrets.redactor();
    // Held until the worker exits: this process drives Bright Data runs.
    let (_driver, _amazon) = match &database {
        Some(url) => {
            let (api_key, unlocker_zone, browser_zone) = secrets.brightdata();
            // `None` without credentials: resumes then fail the key gate.
            let client = client(api_key, unlocker_zone, browser_zone, &redactor).map(Arc::new);
            let db_error = |error: anyhow::Error| {
                anyhow::anyhow!(url.redactor().sanitize(&format!("{error:#}")))
            };
            let pool = db::connect(url)
                .await
                .context("failed to connect to Postgres")
                .map_err(db_error)?;
            let driver = recover_brightdata_runs(url, pool.clone(), client.as_deref(), &redactor)
                .await
                .map_err(db_error)?;
            // The Amazon fallback runs through the same gated, durable runs,
            // so only the driver-lock holder may start them.
            let store = Arc::new(PgRunStore::new(pool, url.redactor().clone()));
            let fallback = driver_fallback(driver.is_some(), client, store);
            let (amazon, _) = amazon_source(secrets.spapi(), fallback, &redactor);
            (driver, amazon)
        }
        None => {
            tracing::info!("DATABASE_URL is not set; Bright Data run recovery skipped");
            let (amazon, _) =
                amazon_source::<BrightDataClient, PgRunStore>(secrets.spapi(), None, &redactor);
            (None, amazon)
        }
    };
    tracing::info!(
        product = domain::PRODUCT_NAME,
        "worker started; no jobs configured yet"
    );
    Ok(())
}

/// Retailer parse-health alert settings from `RETAILER_PARSE_ALERT_THRESHOLD`
/// and `RETAILER_PARSE_ALERT_MIN_SAMPLES` (unset keeps the default; a
/// malformed value fails closed, naming the variable only).
fn parse_health_config() -> Result<retailers::HealthConfig, retailers::HealthConfigError> {
    retailers::HealthConfig::from_lookup(|name| std::env::var(name).ok())
}

/// Builds the robots.txt checker for a [`Runner`], fetching robots.txt
/// directly from the registered retailer hosts; `None` (with a warning) when
/// it cannot be built, in which case checks are skipped.
///
/// Only `Runner::start_unlocker_run` consults it; resumes do not, and no
/// worker job starts Unlocker runs yet. The future store-fetch job must build
/// its `Runner` with `robots: robots_checker().as_ref()`.
fn robots_checker() -> Option<RobotsChecker> {
    HttpRobotsSource::new(retailers::hosts())
        .map(|source| RobotsChecker::new(Box::new(source), Box::new(SystemClock)))
        .map_err(|_| tracing::warn!("robots.txt checker unavailable; checks skipped"))
        .ok()
}

/// Takes the driver lock, fails or keeps interrupted Bright Data runs and
/// resumes the kept ones. Skips both when another worker holds the lock:
/// its runs are live.
async fn recover_brightdata_runs(
    url: &DatabaseUrl,
    pool: db::PgPool,
    transport: Option<&BrightDataClient>,
    redactor: &Redactor,
) -> anyhow::Result<Option<DriverLock>> {
    let Some(lock) = db::brightdata_runs::try_driver_lock(&pool)
        .await
        .context("taking the Bright Data driver lock")?
    else {
        tracing::info!("another worker drives Bright Data runs; recovery skipped");
        return Ok(None);
    };
    let metrics_pool = pool.clone();
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
        let robots = robots_checker();
        let runner = Runner {
            transport,
            store: &store,
            clock: &SystemClock,
            timing: Timing::default(),
            caps: Caps::default(),
            robots: robots.as_ref(),
        };
        // Still under the driver lock.
        let resumed = resume_kept(&runner, &report.kept, &RefusingSink, redactor).await;
        tracing::info!(
            finished = resumed.finished,
            active = resumed.active,
            refused = resumed.refused,
            abandoned = resumed.abandoned,
            "Bright Data run resume finished"
        );
    }
    // Runs on every start-up that holds the driver lock, not just after a resume.
    let access_report = AccessReport::new(
        Arc::new(PgAccessCounts(metrics_pool)),
        Arc::new(RetailerOfferSource::lookup_only()),
    );
    access_metrics::log_access_metrics(&access_report).await;
    Ok(Some(lock))
}
