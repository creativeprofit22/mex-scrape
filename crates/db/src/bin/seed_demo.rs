//! Seeds the database in `DATABASE_URL` with demo data, for local demos with
//! the worker's demo mode (`DEMO_DATA=true`, see
//! `docs/modules/db/demo-data.md`). It refuses any `DATABASE_URL` that is
//! not on `localhost`, `127.0.0.1` or `::1` (no opt-in: the data is fake and
//! the history append-only).
//!
//! `seed_demo` (no arguments, safe to rerun) only orchestrates `db::demo`:
//! 1. a year of daily prices for every catalog listing without history
//!    ([`db::demo::history`]);
//! 2. the demo account, created or reset ([`db::demo::account`]);
//! 3. one queued demo-account search per catalog keyword group without a
//!    live search ([`db::demo::searches`]); the demo worker runs them.
//!
//! It leaves `safety_settings` alone: the demo worker needs no connector
//! flag (ADR 0016); it only warns when the kill switch would refuse the
//! searches.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use db::DatabaseUrl;
use db::brightdata_runs;
use db::demo::account::{self, DEMO_EMAIL};
use db::demo::catalog::DemoCatalog;
use db::demo::{history, searches};
use db::local_guard::require_local;
use domain::SAMS_RETAILER_ID;
use sqlx::PgPool;

fn now_ms() -> anyhow::Result<i64> {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before 1970")?;
    i64::try_from(since.as_millis()).context("system clock out of range")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    if std::env::args().len() > 1 {
        anyhow::bail!("usage: seed_demo (no arguments)");
    }
    let url = DatabaseUrl::from_env()
        .context("loading DATABASE_URL")?
        .context("DATABASE_URL is not set")?;
    require_local(&url)?;
    let catalog = DemoCatalog::embedded().context("loading the demo catalog")?;
    let result = async {
        let pool = db::connect(&url)
            .await
            .context("failed to connect to Postgres")?;
        seed(&pool, &catalog, now_ms()?).await
    }
    .await;
    if let Err(error) = result {
        let message = url.redactor().sanitize(&format!("{error:#}"));
        tracing::error!(error = %message, "seed_demo failed");
        return Err(anyhow::anyhow!(message));
    }
    Ok(())
}

async fn seed(pool: &PgPool, catalog: &DemoCatalog, now: i64) -> anyhow::Result<()> {
    let history = history::seed_history(pool, catalog, now).await?;
    let user = account::upsert_demo_account(pool, now).await?;

    // Read only: the kill switch still applies in demo mode.
    let settings = brightdata_runs::safety_settings(pool)
        .await
        .context("reading safety settings")?;
    if settings.is_none_or(|s| s.global_kill_switch) {
        tracing::warn!(
            "the global kill switch is on (or safety_settings is missing): demo searches will be refused until it is off; rerun seed_demo after turning it off"
        );
    }

    let queued = searches::queue_group_searches(pool, &user.tenant_id, catalog, now).await?;
    tracing::info!(
        login = DEMO_EMAIL,
        user_id = user.id,
        products = catalog.products.len(),
        listings_seeded = history.written,
        listings_kept = history.kept,
        days = history::HISTORY_DAYS,
        searches_queued = queued.queued.join(", "),
        searches_kept = queued.kept.join(", "),
        hidden_store = SAMS_RETAILER_ID,
        "demo data seeded; start the worker with DEMO_DATA=true to run the queued searches"
    );
    Ok(())
}
