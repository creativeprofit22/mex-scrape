//! Seeds the database in `DATABASE_URL` for the web end-to-end tests, so
//! they need no worker, Bright Data or Amazon access. Never run it against
//! a real database: it creates a known account. It refuses to connect
//! unless the `DATABASE_URL` host is `localhost`, `127.0.0.1` or `::1`, or
//! `E2E_ALLOW_REMOTE_DB=true` is set (a Unix socket also needs the opt-in).
//!
//! - `seed_e2e seed`: creates (or resets) the account `E2E_EMAIL`
//!   (default `e2e@example.com`) with password `E2E_PASSWORD` (default
//!   below, at least 8 characters), onboarded with a Costco membership and
//!   Spanish locale; the Amazon name of [`ASIN`]; accepted matches of three
//!   listings to it; and
//!   400 days of daily price observations for them (only for listings with
//!   no history yet, as observations are append-only). It also creates (or
//!   resets) `E2E_PENDING_EMAIL` (default `e2e-pending@example.com`, same
//!   password) with onboarding not done and no memberships. Idempotent.
//! - `seed_e2e complete [--watch]`: fills every queued or running search of
//!   that account with fixed results and marks it `done`. With `--watch` it
//!   keeps doing so every 500 ms until stopped.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, bail};
use db::DatabaseUrl;
use db::amazon_products::ProductRow;
use db::local_guard::{ALLOW_REMOTE_ENV, check_target};
use db::price_history::MatchRow;
use db::searches::{RetailerOutcomeRow, SearchResultRow};
use sqlx::{PgPool, Row as _};

/// The Amazon product every seeded listing is matched to.
const ASIN: &str = "B0E2ETEST1";
/// Amazon name and brand stored for [`ASIN`] (the product page heading).
const PRODUCT_TITLE: &str = "Licuadora de prueba E2E 600 W";
const PRODUCT_BRAND: &str = "Marca E2E";

/// The listing whose match `complete` leaves `needs_review`. The scripted
/// assistant model used by the e2e suite (`services/agent`, fake provider)
/// proposes a decision for exactly this retailer, SKU and [`ASIN`], so keep
/// them in sync.
const REVIEW_RETAILER: &str = "costco_mx";
/// SKU of the `needs_review` listing (see [`REVIEW_RETAILER`]).
const REVIEW_SKU: &str = "E2E-COS-1";

/// Default e-mail of the seeded account.
const DEFAULT_EMAIL: &str = "e2e@example.com";

/// Default e-mail of the seeded account that has not finished onboarding.
const DEFAULT_PENDING_EMAIL: &str = "e2e-pending@example.com";

/// Default password of the seeded account (test fixture only).
const DEFAULT_PASSWORD: &str = "e2e-password-123";

/// Days of price history per listing.
const HISTORY_DAYS: i64 = 400;

const DAY_MS: i64 = 86_400_000;

/// `(retailer, sku, base price in centavos)` of the matched listings.
const LISTINGS: [(&str, &str, i64); 3] = [
    ("liverpool", "E2E-LIV-1", 129_900),
    ("walmart_mx", "E2E-WAL-1", 119_900),
    ("costco_mx", "E2E-COS-1", 109_900),
];

fn now_ms() -> anyhow::Result<i64> {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before 1970")?;
    i64::try_from(since.as_millis()).context("system clock out of range")
}

fn email() -> String {
    std::env::var("E2E_EMAIL").unwrap_or_else(|_| DEFAULT_EMAIL.to_owned())
}

fn pending_email() -> String {
    std::env::var("E2E_PENDING_EMAIL").unwrap_or_else(|_| DEFAULT_PENDING_EMAIL.to_owned())
}

fn password() -> anyhow::Result<String> {
    let password = std::env::var("E2E_PASSWORD").unwrap_or_else(|_| DEFAULT_PASSWORD.to_owned());
    if password.chars().count() < domain::MIN_PASSWORD_CHARS {
        bail!("E2E_PASSWORD is shorter than the password policy allows");
    }
    Ok(password)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let url = DatabaseUrl::from_env()
        .context("loading DATABASE_URL")?
        .context("DATABASE_URL is not set")?;
    check_target(&url, std::env::var_os(ALLOW_REMOTE_ENV).as_deref())?;
    let result = async {
        let pool = db::connect(&url)
            .await
            .context("failed to connect to Postgres")?;
        match args.as_slice() {
            ["seed"] => seed(&pool).await,
            ["complete"] => complete(&pool).await.map(|_| ()),
            ["complete", "--watch"] => loop {
                complete(&pool).await?;
                tokio::time::sleep(Duration::from_millis(500)).await;
            },
            ["reset-assistant"] => reset_assistant(&pool).await,
            _ => bail!(
                "usage: seed_e2e seed | seed_e2e complete [--watch] | seed_e2e reset-assistant"
            ),
        }
    }
    .await;
    if let Err(error) = result {
        let message = url.redactor().sanitize(&format!("{error:#}"));
        tracing::error!(error = %message, "seed_e2e failed");
        return Err(anyhow::anyhow!(message));
    }
    Ok(())
}

/// Price of a listing `days_ago` days back: a slow wave around `base`
/// (deterministic, so charts and stats are stable between runs).
fn price_on(base: i64, days_ago: i64) -> i64 {
    let step = (days_ago % 60 - 30).abs(); // 0..=30
    base - 15_000 + step * 1_000
}

/// Creates the account `email` (Spanish, `free` plan) or resets its
/// password, locale and plan; returns its id.
async fn upsert_account(pool: &PgPool, email: &str, hash: &str, now: i64) -> anyhow::Result<i64> {
    let id = match db::users::find_credentials(pool, email)
        .await
        .context("reading the account")?
    {
        Some((user, _)) => {
            sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
                .bind(user.id)
                .bind(hash)
                .execute(pool)
                .await
                .context("resetting the password")?;
            user.id
        }
        None => {
            db::users::create_user(pool, email, hash, "es", now)
                .await
                .context("creating the account")?
                .id
        }
    };
    db::users::set_locale(pool, id, "es", now)
        .await
        .context("setting the locale")?;
    db::users::set_plan(pool, id, "free", now)
        .await
        .context("setting the plan")?;
    Ok(id)
}

/// Deletes the e2e accounts' match decisions, suggestions and assistant
/// token usage, so the `needs_review` fixture is back for every run.
async fn reset_assistant(pool: &PgPool) -> anyhow::Result<()> {
    let mut emails = Vec::new();
    for raw in [email(), pending_email()] {
        let parsed =
            domain::Email::parse(&raw).map_err(|_| anyhow::anyhow!("invalid e2e email"))?;
        emails.push(parsed.as_str().to_owned());
    }
    let mut tx = pool.begin().await.context("starting the reset")?;
    // Decisions first: they reference suggestions.
    for sql in [
        "DELETE FROM match_decisions WHERE tenant_id IN \
         (SELECT tenant_id FROM users WHERE email = ANY($1))",
        "DELETE FROM match_suggestions WHERE tenant_id IN \
         (SELECT tenant_id FROM users WHERE email = ANY($1))",
        "DELETE FROM agent_token_usage WHERE tenant_id IN \
         (SELECT tenant_id FROM users WHERE email = ANY($1))",
    ] {
        sqlx::query(sql)
            .bind(&emails[..])
            .execute(&mut *tx)
            .await
            .context("clearing assistant rows")?;
    }
    tx.commit().await.context("committing the reset")?;
    Ok(())
}

async fn seed(pool: &PgPool) -> anyhow::Result<()> {
    let now = now_ms()?;
    let email = domain::Email::parse(&email()).map_err(|_| anyhow::anyhow!("invalid E2E_EMAIL"))?;
    let pending = domain::Email::parse(&pending_email())
        .map_err(|_| anyhow::anyhow!("invalid E2E_PENDING_EMAIL"))?;
    if pending.as_str() == email.as_str() {
        bail!("E2E_PENDING_EMAIL must differ from E2E_EMAIL");
    }
    let hash = password_auth::generate_hash(password()?);
    let id = upsert_account(pool, email.as_str(), &hash, now).await?;
    db::users::set_memberships(pool, id, true, false, now)
        .await
        .context("setting memberships")?;
    let pending_id = upsert_account(pool, pending.as_str(), &hash, now).await?;
    sqlx::query(
        "UPDATE users SET has_costco = false, has_sams = false, onboarded = false WHERE id = $1",
    )
    .bind(pending_id)
    .execute(pool)
    .await
    .context("resetting onboarding")?;

    reset_assistant(pool).await?;

    db::amazon_products::upsert_product(
        pool,
        &ProductRow {
            asin: ASIN.to_owned(),
            title: PRODUCT_TITLE.to_owned(),
            brand: Some(PRODUCT_BRAND.to_owned()),
            source: "sp_api".to_owned(),
            seen_at_ms: now,
        },
    )
    .await
    .context("storing the Amazon name")?;

    for (retailer, sku, base) in LISTINGS {
        db::price_history::upsert_match(
            pool,
            MatchRow {
                retailer,
                retailer_sku: sku,
                asin: ASIN,
                method: "gtin",
                confidence_bp: 10_000,
                status: "accepted",
                matched_at_ms: now,
            },
        )
        .await
        .context("storing a match")?;
        // Observations are append-only: keep an earlier seed's history.
        let existing: i64 = sqlx::query(
            "SELECT count(*) AS n FROM price_observations              WHERE retailer = $1 AND retailer_sku = $2",
        )
        .bind(retailer)
        .bind(sku)
        .fetch_one(pool)
        .await
        .context("counting observations")?
        .try_get("n")
        .context("counting observations")?;
        if existing > 0 {
            continue;
        }
        let days: Vec<i64> = (0..HISTORY_DAYS).rev().collect();
        let prices: Vec<i64> = days.iter().map(|d| price_on(base, *d)).collect();
        let stock: Vec<&str> = days
            .iter()
            .map(|d| {
                if d % 45 == 7 {
                    "out_of_stock"
                } else {
                    "in_stock"
                }
            })
            .collect();
        let times: Vec<i64> = days.iter().map(|d| now - d * DAY_MS).collect();
        sqlx::query(
            "INSERT INTO price_observations              (retailer, retailer_sku, price_mxn, availability, observed_at)              SELECT $1, $2, p, a, to_timestamp(t / 1000.0)              FROM unnest($3::BIGINT[], $4::TEXT[], $5::BIGINT[]) AS x(p, a, t)",
        )
        .bind(retailer)
        .bind(sku)
        .bind(&prices)
        .bind(&stock)
        .bind(&times)
        .execute(pool)
        .await
        .context("storing observations")?;
    }
    tracing::info!(
        user_id = id,
        asin = ASIN,
        days = HISTORY_DAYS,
        "e2e data seeded"
    );
    Ok(())
}

fn outcome(retailer: &str, outcome: &str, offers: i32) -> RetailerOutcomeRow {
    RetailerOutcomeRow {
        retailer: retailer.to_owned(),
        outcome: outcome.to_owned(),
        offers,
    }
}

/// Fixed results: an accepted match with ROI, a Costco offer needing
/// review, an unmatched offer and a Sam's Club offer the seeded account
/// cannot see (no membership).
fn results(query: &str) -> Vec<SearchResultRow> {
    let row = |retailer: &str, sku: &str, price: i64, membership: bool| SearchResultRow {
        retailer: retailer.to_owned(),
        retailer_sku: sku.to_owned(),
        title: format!("{query} ({sku})"),
        url: format!("https://example.com/{retailer}/{sku}"),
        price_mxn: price,
        availability: "in_stock".to_owned(),
        membership_required: membership,
        asin: None,
        match_status: None,
        confidence_bp: None,
        amazon_price_mxn: None,
        fees_mxn: None,
        profit_centavos: None,
        roi_bp: None,
        margin_bp: None,
    };
    let matched = |mut r: SearchResultRow, status: &str, confidence: i32| {
        let zero = domain::PriceMxn::from_centavos(0).ok()?;
        let input = domain::RoiInput {
            sell_price: domain::PriceMxn::from_centavos(199_900).ok()?,
            buy_price: domain::PriceMxn::from_centavos(r.price_mxn).ok()?,
            amazon_fees: domain::PriceMxn::from_centavos(35_000).ok()?,
            prep_fee: zero,
            shipping: zero,
            monthly_fixed: zero,
            units_per_month: 0,
        };
        let roi = domain::calculate_roi(&input).ok()?;
        r.asin = Some(ASIN.to_owned());
        r.match_status = Some(status.to_owned());
        r.confidence_bp = Some(confidence);
        r.amazon_price_mxn = Some(input.sell_price.centavos());
        r.fees_mxn = Some(input.amazon_fees.centavos());
        r.profit_centavos = Some(roi.profit_centavos);
        r.roi_bp = roi.roi_bp;
        r.margin_bp = roi.margin_bp;
        Some(r)
    };
    let mut out = Vec::new();
    out.extend(matched(
        row("liverpool", "E2E-LIV-1", 129_900, false),
        "accepted",
        9_800,
    ));
    out.extend(matched(
        row(REVIEW_RETAILER, REVIEW_SKU, 109_900, true),
        "needs_review",
        7_200,
    ));
    out.push(row("walmart_mx", "E2E-WAL-9", 89_900, false));
    out.push(row("sams_mx", "E2E-SAM-1", 99_900, true));
    out
}

/// Completes the account's pending searches; returns how many.
async fn complete(pool: &PgPool) -> anyhow::Result<usize> {
    let email = email();
    let rows = sqlx::query(
        "SELECT s.id, s.query FROM searches s JOIN users u ON u.tenant_id = s.tenant_id \
         WHERE u.email = $1 AND s.status IN ('queued', 'running') ORDER BY s.id",
    )
    .bind(email.trim().to_lowercase())
    .fetch_all(pool)
    .await
    .context("listing pending searches")?;
    for row in &rows {
        let id: i64 = row.try_get("id").context("reading a search id")?;
        let query: String = row.try_get("query").context("reading a search query")?;
        let now = now_ms()?;
        db::searches::mark_running(pool, id, now)
            .await
            .context("starting a search")?;
        for o in [
            outcome("costco_mx", "ok", 1),
            outcome("liverpool", "ok", 1),
            outcome("sams_mx", "ok", 1),
            outcome("sears_mx", "category_only", 0),
            outcome("walmart_mx", "ok", 1),
        ] {
            db::searches::upsert_retailer_outcome(pool, id, &o)
                .await
                .context("storing an outcome")?;
        }
        for r in results(&query) {
            db::searches::upsert_result(pool, id, &r)
                .await
                .context("storing a result")?;
        }
        db::searches::finish(pool, id, "done", "", now)
            .await
            .context("finishing a search")?;
        tracing::info!(search_id = id, "e2e search completed");
    }
    Ok(rows.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_stay_positive_and_vary() {
        for (_, _, base) in LISTINGS {
            let prices: Vec<i64> = (0..HISTORY_DAYS).map(|d| price_on(base, d)).collect();
            assert!(prices.iter().all(|p| *p > 0));
            assert!(prices.iter().min() < prices.iter().max());
        }
    }

    #[test]
    fn results_carry_roi_from_the_domain_calculation() {
        let rows = results("taladro");
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].match_status.as_deref(), Some("accepted"));
        // 1999 - 350 - 1299 = 350 profit.
        assert_eq!(rows[0].profit_centavos, Some(35_000));
        assert_eq!(rows[1].match_status.as_deref(), Some("needs_review"));
        assert!(rows[3].membership_required);
    }
}
