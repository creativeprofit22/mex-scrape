//! Seeds the database in `DATABASE_URL` with demo price history, for local
//! demos with the worker's demo mode (`DEMO_DATA=true`, see
//! `docs/modules/db.md` "Demo data"). It refuses any `DATABASE_URL` that is
//! not on `localhost`, `127.0.0.1` or `::1` (no opt-in: the history is
//! append-only and fake).
//!
//! `seed_demo`, idempotent:
//! - writes [`HISTORY_DAYS`] days of daily price observations (ending
//!   yesterday) for every listing of `config/demo_catalog.json` that has no
//!   history yet (observations are append-only, so a rerun skips them);
//! - creates (or resets) the demo account [`DEMO_EMAIL`] with password
//!   [`DEMO_PASSWORD`]: Spanish, `free` plan, onboarded, with Costco and
//!   Sam's Club memberships so every demo store shows. The demo worker runs
//!   only this account's searches, so the e2e account is never touched.
//!
//! It leaves `safety_settings` alone: the demo worker needs no connector
//! flag (ADR 0016). Matches come from searches run by the worker.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use db::DatabaseUrl;
use db::brightdata_runs;
use db::demo_catalog::{DEMO_EMAIL, DemoCatalog};
use db::local_guard::require_local;
use sqlx::{PgPool, Row as _};

/// Days of price history per listing.
const HISTORY_DAYS: i64 = 365;

/// The demo account's password (a local fixture, like `seed_e2e`'s).
const DEMO_PASSWORD: &str = "demo-password-123";

const DAY_MS: i64 = 86_400_000;

fn now_ms() -> anyhow::Result<i64> {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before 1970")?;
    i64::try_from(since.as_millis()).context("system clock out of range")
}

/// A stable per-listing offset, so listings do not move in lockstep.
fn salt(retailer: &str, sku: &str) -> i64 {
    retailer
        .bytes()
        .chain(sku.bytes())
        .fold(17_i64, |acc, b| (acc * 31 + i64::from(b)) % 997)
}

/// Price `days_ago` days back for a listing sold at `base` today: a
/// 70-day triangle wave of ±6 %, a 12 % sale for one week every 120 days,
/// rounded to whole pesos. Deterministic and always positive.
fn price_on(base: i64, days_ago: i64, salt: i64) -> i64 {
    let phase = (days_ago + salt) % 70;
    // 0..=35..=0 → -60..=+60 per mille.
    let tri = 35 - (phase - 35).abs();
    let mut per_mille = 1000 + (tri * 120) / 35 - 60;
    if (days_ago + salt) % 120 < 7 {
        per_mille -= 120;
    }
    let centavos = base * per_mille / 1000;
    ((centavos + 50) / 100).max(1) * 100
}

/// Out of stock on a few scattered days.
fn availability_on(days_ago: i64, salt: i64) -> &'static str {
    if (days_ago + salt) % 47 == 0 {
        "out_of_stock"
    } else {
        "in_stock"
    }
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
        seed(&pool, &catalog).await
    }
    .await;
    if let Err(error) = result {
        let message = url.redactor().sanitize(&format!("{error:#}"));
        tracing::error!(error = %message, "seed_demo failed");
        return Err(anyhow::anyhow!(message));
    }
    Ok(())
}

async fn seed(pool: &PgPool, catalog: &DemoCatalog) -> anyhow::Result<()> {
    let now = now_ms()?;
    let (mut written, mut kept) = (0_u32, 0_u32);
    for (_, listing) in catalog.listings() {
        let existing: i64 = sqlx::query(
            "SELECT count(*) AS n FROM price_observations WHERE retailer = $1 AND retailer_sku = $2",
        )
        .bind(&listing.retailer)
        .bind(&listing.sku)
        .fetch_one(pool)
        .await
        .context("counting observations")?
        .try_get("n")
        .context("counting observations")?;
        if existing > 0 {
            kept += 1;
            continue;
        }
        let salt = salt(&listing.retailer, &listing.sku);
        let days: Vec<i64> = (1..=HISTORY_DAYS).rev().collect();
        let prices: Vec<i64> = days
            .iter()
            .map(|d| price_on(listing.price_centavos, *d, salt))
            .collect();
        let stock: Vec<&str> = days.iter().map(|d| availability_on(*d, salt)).collect();
        let times: Vec<i64> = days.iter().map(|d| now - d * DAY_MS).collect();
        sqlx::query(
            "INSERT INTO price_observations \
             (retailer, retailer_sku, price_mxn, availability, observed_at) \
             SELECT $1, $2, p, a, to_timestamp(t / 1000.0) \
             FROM unnest($3::BIGINT[], $4::TEXT[], $5::BIGINT[]) AS x(p, a, t)",
        )
        .bind(&listing.retailer)
        .bind(&listing.sku)
        .bind(&prices)
        .bind(&stock)
        .bind(&times)
        .execute(pool)
        .await
        .context("storing observations")?;
        written += 1;
    }

    let user_id = upsert_demo_account(pool, now).await?;

    // Read only: the kill switch still applies in demo mode.
    let settings = brightdata_runs::safety_settings(pool)
        .await
        .context("reading safety settings")?;
    if settings.is_none_or(|s| s.global_kill_switch) {
        tracing::warn!(
            "the global kill switch is on (or safety_settings is missing): demo searches will be refused until it is off"
        );
    }

    let mut keywords: Vec<&str> = catalog
        .products
        .iter()
        .filter_map(|p| p.keywords.first().map(String::as_str))
        .collect();
    keywords.sort_unstable();
    tracing::info!(
        login = DEMO_EMAIL,
        user_id,
        products = catalog.products.len(),
        listings_seeded = written,
        listings_kept = kept,
        days = HISTORY_DAYS,
        try_searching = keywords.join(", "),
        "demo data seeded"
    );
    Ok(())
}

/// Creates the demo account or resets its password, locale, plan and
/// memberships; returns its id.
async fn upsert_demo_account(pool: &PgPool, now: i64) -> anyhow::Result<i64> {
    let hash = password_auth::generate_hash(DEMO_PASSWORD);
    let id = match db::users::find_credentials(pool, DEMO_EMAIL)
        .await
        .context("reading the demo account")?
    {
        Some((user, _)) => {
            sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
                .bind(user.id)
                .bind(&hash)
                .execute(pool)
                .await
                .context("resetting the demo password")?;
            user.id
        }
        None => {
            db::users::create_user(pool, DEMO_EMAIL, &hash, "es", now)
                .await
                .context("creating the demo account")?
                .id
        }
    };
    db::users::set_locale(pool, id, "es", now)
        .await
        .context("setting the demo locale")?;
    db::users::set_plan(pool, id, "free", now)
        .await
        .context("setting the demo plan")?;
    // Also marks onboarding done.
    db::users::set_memberships(pool, id, true, true, now)
        .await
        .context("setting the demo memberships")?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_stay_positive_near_today_and_vary() {
        let catalog = DemoCatalog::embedded().unwrap();
        for (_, l) in catalog.listings() {
            let s = salt(&l.retailer, &l.sku);
            let prices: Vec<i64> = (1..=HISTORY_DAYS)
                .map(|d| price_on(l.price_centavos, d, s))
                .collect();
            assert!(prices.iter().all(|p| *p > 0 && p % 100 == 0));
            assert!(prices.iter().min() < prices.iter().max());
            let low = l.price_centavos * 80 / 100;
            let high = l.price_centavos * 107 / 100;
            assert!(prices.iter().all(|p| (low..=high).contains(p)), "{}", l.sku);
        }
    }

    #[test]
    fn the_wave_and_stock_are_deterministic() {
        assert_eq!(price_on(100_000, 10, 3), price_on(100_000, 10, 3));
        assert_eq!(salt("liverpool", "1"), salt("liverpool", "1"));
        assert_ne!(salt("liverpool", "1"), salt("liverpool", "2"));
        let out = (1..=HISTORY_DAYS)
            .filter(|d| availability_on(*d, 5) == "out_of_stock")
            .count();
        assert!((5..=10).contains(&out));
    }
}
