//! Runs the mex-scrape HTTP API on `API_ADDR` (default `127.0.0.1:8080`).

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use app::{AccessReport, Clock, PriceStats};
use db::access::PgAccessCounts;
use db::price_history::{PgMatches, PgPriceHistory};
use domain::UnixMillis;
use retailers::RetailerOfferSource;

/// The wall clock. A clock before 1970 reads as 0.
struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> UnixMillis {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        UnixMillis(ms)
    }
}

/// Composition root: price stats over Postgres and the wall clock.
fn price_stats(pool: db::PgPool) -> PriceStats {
    PriceStats::new(
        Arc::new(PgPriceHistory(pool.clone())),
        Arc::new(PgMatches(pool)),
        Arc::new(SystemClock),
    )
}

/// Composition root: the access report over Postgres and the retailer registry.
fn access_report(pool: db::PgPool) -> AccessReport {
    AccessReport::new(
        Arc::new(PgAccessCounts(pool)),
        Arc::new(RetailerOfferSource::lookup_only()),
    )
}

/// Connects to `DATABASE_URL`; `None` (logged) when it is unset, invalid
/// or unreachable. Errors name the variable only or are redacted.
async fn database() -> Option<db::PgPool> {
    let url = match db::DatabaseUrl::from_env() {
        Ok(Some(url)) => url,
        Ok(None) => {
            tracing::info!("DATABASE_URL is not set; database routes disabled");
            return None;
        }
        Err(error) => {
            tracing::warn!(%error, "DATABASE_URL is invalid; database routes disabled");
            return None;
        }
    };
    match db::connect(&url).await {
        Ok(pool) => Some(pool),
        Err(error) => {
            let detail = url.redactor().sanitize(&error.to_string());
            tracing::warn!(error = %detail, "database unavailable; database routes disabled");
            None
        }
    }
}

/// Admin routes need `ADMIN_API_TOKEN` and a database; without both they
/// answer 503.
fn admin_state(pool: Option<&db::PgPool>) -> api::AdminState {
    let token = std::env::var("ADMIN_API_TOKEN").unwrap_or_default();
    if token.len() < api::MIN_TOKEN_LEN {
        tracing::info!("ADMIN_API_TOKEN is unset or too short; admin API disabled");
        return api::AdminState::disabled();
    }
    match pool {
        Some(pool) => api::AdminState::new(&token, Arc::new(access_report(pool.clone()))),
        None => api::AdminState::disabled(),
    }
}

/// Product routes need a database; without one they answer 503.
fn products_state(pool: Option<&db::PgPool>) -> api::ProductsState {
    pool.map_or_else(api::ProductsState::disabled, |pool| {
        api::ProductsState::new(Arc::new(price_stats(pool.clone())))
    })
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

    let pool = database().await;
    let (router, _) =
        api::router_with_state(admin_state(pool.as_ref()), products_state(pool.as_ref()));
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
