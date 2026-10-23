//! Background job runner for mex-scrape (ADR 0011).
//!
//! Loads server secrets and settings at start-up (failing closed on a
//! malformed value), then, when `DATABASE_URL` is set, waits for the Bright
//! Data driver lock, recovers interrupted runs and resumes the kept ones,
//! and runs the job consumer and the cron schedules until Ctrl-C/SIGTERM.
//! Only the driver-lock holder runs jobs, so schedules never double-fire.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context as _;
use app::AccessReport;
use app::AmazonSource;
use brightdata::Transport;
use brightdata::recovery::recover_interrupted;
use brightdata::robots::{HttpRobotsSource, RobotsChecker};
use brightdata::runs::{Runner, SystemClock, Timing};
use db::DatabaseUrl;
use db::access::PgAccessCounts;
use db::brightdata_runs::DriverLock;
use db::demo::account::DEMO_EMAIL;
use db::demo::catalog::DemoCatalog;
use db::price_history::{PgMatches, PgPriceHistory};
use db::usage::PgRequestBudget;
use http_guard::Redactor;
use matching::MatchConfig;
use retailers::category::CategoryListing;
use retailers::{ParseHealth, RetailerOfferSource};
use tokio::sync::watch;

use worker::access_metrics;
use worker::amazon_source::{TenantAmazon, amazon_source, driver_fallback};
use worker::brightdata_resume::{RefusingSink, client, resume_kept};
use worker::brightdata_store::PgRunStore;
use worker::clock::WallClock;
use worker::config::WorkerConfig;
use worker::demo::{DemoFees, DemoTransport};
use worker::jobs::consumer::{self, ConsumerConfig};
use worker::jobs::{JobContext, Jobs};
use worker::schedule::{self, Schedules};
use worker::secrets::ServerSecrets;

/// How often a worker without the driver lock tries again.
const DRIVER_LOCK_RETRY: Duration = Duration::from_secs(60);

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

/// Retailer parse-health alert settings from `RETAILER_PARSE_ALERT_THRESHOLD`
/// and `RETAILER_PARSE_ALERT_MIN_SAMPLES` (unset keeps the default; a
/// malformed value fails closed, naming the variable only).
fn parse_health_config() -> Result<retailers::HealthConfig, retailers::HealthConfigError> {
    retailers::HealthConfig::from_lookup(|name| std::env::var(name).ok())
}

/// Loads and validates `CATEGORY_LISTINGS_PATH`; logs, per retailer, how
/// many listings each browse covers and how many the page cap skips.
fn load_listings(config: &WorkerConfig) -> anyhow::Result<Vec<CategoryListing>> {
    let json = std::fs::read_to_string(&config.category_listings_path)
        .context("reading CATEGORY_LISTINGS_PATH")?;
    let listings =
        retailers::category::parse_listings(&json).context("validating CATEGORY_LISTINGS_PATH")?;
    for info in retailers::category::category_retailers() {
        let total = listings.iter().filter(|l| l.retailer == info.id).count();
        let browsed = total.min(config.category_max_pages);
        tracing::info!(
            retailer = info.id,
            configured = total,
            browsed,
            skipped = total - browsed,
            "category listings"
        );
    }
    Ok(listings)
}

/// A receiver that turns true on Ctrl-C or SIGTERM.
fn shutdown_signal() -> watch::Receiver<bool> {
    let (tx, rx) = watch::channel(false);
    tokio::spawn(async move {
        wait_for_signal().await;
        tracing::info!("shutdown requested; finishing the current job");
        let _ = tx.send(true);
    });
    rx
}

#[cfg(unix)]
async fn wait_for_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    match signal(SignalKind::terminate()) {
        Ok(mut term) => {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
        }
        Err(_) => {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

#[cfg(not(unix))]
async fn wait_for_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

async fn run(secrets: &ServerSecrets) -> anyhow::Result<()> {
    let parse_health = parse_health_config().context("loading retailer parse health settings")?;
    tracing::info!(
        threshold = parse_health.threshold,
        min_samples = parse_health.min_samples,
        window = parse_health.window,
        "retailer parse health alerts"
    );
    let config = WorkerConfig::from_env().context("loading worker settings")?;
    let listings = load_listings(&config)?;
    // Errors name the variable only, never the value.
    let Some(url) = DatabaseUrl::from_env().context("loading DATABASE_URL")? else {
        tracing::info!("DATABASE_URL is not set; no jobs run");
        return Ok(());
    };
    if config.demo_data {
        // Demo data must never reach a shared database (append-only prices).
        db::local_guard::require_local(&url).context("DEMO_DATA=true")?;
    }
    let redactor = secrets.redactor_extending(url.redactor());
    let db_error = |error: anyhow::Error| anyhow::anyhow!(redactor.sanitize(&format!("{error:#}")));
    let pool = db::connect(&url)
        .await
        .context("failed to connect to Postgres")
        .map_err(db_error)?;
    let mut shutdown = shutdown_signal();
    let Some(lock) = wait_for_driver_lock(&pool, &mut shutdown)
        .await
        .map_err(db_error)?
    else {
        return Ok(());
    };
    // Demo mode runs only the demo account's searches (ADR 0016).
    let only_tenant = if config.demo_data {
        Some(demo_tenant(&pool).await.map_err(db_error)?)
    } else {
        None
    };
    let parts = Parts {
        url: &url,
        pool,
        config: &config,
        listings: &listings,
        parse_health,
        redactor: &redactor,
        only_tenant,
    };
    let result = if config.demo_data {
        let catalog = DemoCatalog::embedded().context("loading the demo catalog")?;
        tracing::warn!(
            products = catalog.products.len(),
            "DEMO_DATA=true: Bright Data and Amazon are served from config/demo_catalog.json; no network calls, credentials ignored"
        );
        let transport = Arc::new(DemoTransport::new(catalog.clone()));
        let fees: Arc<dyn AmazonSource> = Arc::new(DemoFees::new(catalog, Arc::new(WallClock)));
        // No robots.txt fetches: the demo pages are not on the network.
        drive(
            parts,
            Some(transport),
            None,
            (None, None, None),
            Some(fees),
            shutdown,
        )
        .await
    } else {
        let (api_key, unlocker_zone, browser_zone) = secrets.brightdata();
        // `None` without credentials: every run then fails the key gate.
        let client = client(api_key, unlocker_zone, browser_zone, &redactor).map(Arc::new);
        drive(
            parts,
            client,
            robots_checker(),
            secrets.spapi(),
            None,
            shutdown,
        )
        .await
    };
    result.map_err(db_error)?;
    // Held until here: this process drove Bright Data runs.
    drop(lock);
    tracing::info!("worker stopped");
    Ok(())
}

/// What [`drive`] needs besides the transport.
struct Parts<'a> {
    url: &'a DatabaseUrl,
    pool: db::PgPool,
    config: &'a WorkerConfig,
    listings: &'a [CategoryListing],
    parse_health: retailers::HealthConfig,
    redactor: &'a Redactor,
    /// Demo mode: the only tenant whose searches run.
    only_tenant: Option<domain::TenantId>,
}

/// The tenant of the demo account that `seed_demo` creates.
async fn demo_tenant(pool: &db::PgPool) -> anyhow::Result<domain::TenantId> {
    let (user, _) = db::users::find_credentials(pool, DEMO_EMAIL)
        .await
        .context("reading the demo account")?
        .context(
            "DEMO_DATA=true needs the demo account: run `cargo run -p db --bin seed_demo` first",
        )?;
    domain::TenantId::new(&user.tenant_id)
        .map_err(|_| anyhow::anyhow!("the demo account has an invalid tenant id"))
}

type SpApiSecrets = (
    Option<http_guard::Secret>,
    Option<http_guard::Secret>,
    Option<http_guard::Secret>,
);

/// Recovers Bright Data runs, then runs the consumer and the schedules over
/// `client` (the real Bright Data client, or demo mode's fake) until
/// shutdown. The caller holds the driver lock. `primary` replaces SP-API
/// when set (demo fee estimates).
async fn drive<T: Transport + 'static>(
    parts: Parts<'_>,
    client: Option<Arc<T>>,
    robots: Option<RobotsChecker>,
    spapi: SpApiSecrets,
    primary: Option<Arc<dyn AmazonSource>>,
    shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let Parts {
        url,
        pool,
        config,
        listings,
        parse_health,
        redactor,
        only_tenant,
    } = parts;
    recover_brightdata_runs(
        url,
        pool.clone(),
        client.as_deref(),
        robots.as_ref(),
        config,
        redactor,
    )
    .await?;

    let store = PgRunStore::new(pool.clone(), redactor.clone());
    // The demo transport makes no paid call, so it needs no connector flag.
    let store = Arc::new(if config.demo_data {
        store.with_demo_connector()
    } else {
        store
    });
    let fallback = driver_fallback(true, client.clone(), Arc::clone(&store));
    let (amazon, _) = amazon_source(spapi, fallback, config.caps, redactor);
    let amazon = match primary {
        Some(primary) => amazon.with_primary(primary),
        None => amazon,
    };
    let amazon: Arc<dyn TenantAmazon> = Arc::new(amazon);
    let budget = Arc::new(PgRequestBudget::new(
        pool.clone(),
        Arc::new(WallClock),
        config.amazon_requests_per_day,
    ));
    let runner = Runner {
        transport: client.as_deref(),
        store: store.as_ref(),
        clock: &WallClock,
        timing: Timing::default(),
        caps: config.caps,
        robots: robots.as_ref(),
    };
    let prices = PgPriceHistory(pool.clone());
    let matches = PgMatches(pool.clone());
    let health = Mutex::new(ParseHealth::new(parse_health));
    let match_config = MatchConfig::default();
    let jobs = Jobs {
        ctx: JobContext {
            pool: &pool,
            runner: &runner,
            clock: &WallClock,
            amazon,
            budget,
            prices: &prices,
            matches: &matches,
            match_config: &match_config,
            health: &health,
            listings,
            config,
            redactor,
        },
        only_tenant,
    };
    let consumer_config = ConsumerConfig {
        visibility_secs: config.visibility_secs,
        max_attempts: config.max_attempts,
        poll_idle: Duration::from_millis(config.poll_idle_ms),
    };
    let schedules = Schedules {
        refresh: &config.refresh_cron,
        category: &config.category_cron,
        refresh_batch: config.refresh_batch_size,
        listings,
    };
    tracing::info!(
        product = domain::PRODUCT_NAME,
        refresh_cron = config.refresh_cron.as_str(),
        category_cron = config.category_cron.as_str(),
        demo_data = config.demo_data,
        "worker started; consuming search_jobs and refresh_jobs"
    );
    tokio::join!(
        consumer::run(&jobs, consumer_config, shutdown.clone()),
        schedule::run(&pool, &WallClock, schedules, redactor, shutdown),
    );
    Ok(())
}

/// Takes the Bright Data driver lock, retrying every
/// [`DRIVER_LOCK_RETRY`] while another worker holds it. `None` when a
/// shutdown arrives first.
async fn wait_for_driver_lock(
    pool: &db::PgPool,
    shutdown: &mut watch::Receiver<bool>,
) -> anyhow::Result<Option<DriverLock>> {
    loop {
        if let Some(lock) = db::brightdata_runs::try_driver_lock(pool)
            .await
            .context("taking the Bright Data driver lock")?
        {
            return Ok(Some(lock));
        }
        tracing::info!("another worker drives Bright Data runs; waiting for the driver lock");
        tokio::select! {
            () = tokio::time::sleep(DRIVER_LOCK_RETRY) => {}
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(None);
                }
            }
        }
    }
}

/// Builds the robots.txt checker for a [`Runner`], fetching robots.txt
/// directly from the registered retailer hosts; `None` (with a warning) when
/// it cannot be built, in which case checks are skipped.
fn robots_checker() -> Option<RobotsChecker> {
    HttpRobotsSource::new(retailers::hosts())
        .map(|source| RobotsChecker::new(Box::new(source), Box::new(SystemClock)))
        .map_err(|_| tracing::warn!("robots.txt checker unavailable; checks skipped"))
        .ok()
}

/// Fails or keeps interrupted Bright Data runs and resumes the kept ones.
/// The caller must hold the driver lock.
///
/// Kept runs are dataset runs: no dataset importer exists yet, so they are
/// resumed into [`RefusingSink`] and stay resumable.
async fn recover_brightdata_runs<T: Transport>(
    url: &DatabaseUrl,
    pool: db::PgPool,
    transport: Option<&T>,
    robots: Option<&RobotsChecker>,
    config: &WorkerConfig,
    redactor: &Redactor,
) -> anyhow::Result<()> {
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
        let runner = Runner {
            transport,
            store: &store,
            clock: &SystemClock,
            timing: Timing::default(),
            caps: config.caps,
            robots,
        };
        let resumed = resume_kept(&runner, &report.kept, &RefusingSink, redactor).await;
        tracing::info!(
            finished = resumed.finished,
            active = resumed.active,
            refused = resumed.refused,
            abandoned = resumed.abandoned,
            "Bright Data run resume finished"
        );
    }
    let access_report = AccessReport::new(
        Arc::new(PgAccessCounts(metrics_pool)),
        Arc::new(RetailerOfferSource::lookup_only()),
    );
    access_metrics::log_access_metrics(&access_report).await;
    Ok(())
}
