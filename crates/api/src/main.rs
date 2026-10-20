//! Runs the mex-scrape HTTP API on `API_ADDR` (default `127.0.0.1:8080`).

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use api::auth::hasher::Argon2Hasher;
use app::{AccessReport, Clock, PriceSeries, PriceStats, SearchResults, StartSearch};
use axum::http::HeaderName;
use db::access::PgAccessCounts;
use db::price_history::{PgMatches, PgPriceHistory};
use db::searches::PgSearches;
use db::users::PgUsers;
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

/// Composition root: daily price series over Postgres and the wall clock.
fn price_series(pool: db::PgPool) -> PriceSeries {
    PriceSeries::new(
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
        api::ProductsState::new(
            Arc::new(price_stats(pool.clone())),
            Arc::new(price_series(pool.clone())),
        )
    })
}

/// Search routes need a database; without one they answer 503.
fn searches_state(pool: Option<&db::PgPool>) -> api::SearchesState {
    pool.map_or_else(api::SearchesState::disabled, |pool| {
        let repo = Arc::new(PgSearches::new(pool.clone(), Arc::new(SystemClock)));
        api::SearchesState::new(StartSearch::new(repo.clone()), SearchResults::new(repo))
    })
}

/// Origins allowed on mutating requests when `APP_ORIGINS` is unset (the
/// local Next.js dev server).
const DEFAULT_APP_ORIGINS: &str = "http://localhost:3000,http://127.0.0.1:3000";

/// Cookie and origin settings from `SESSION_COOKIE_SECURE` (default `true`;
/// only `false` turns it off) and `APP_ORIGINS` (comma-separated).
fn session_config() -> api::SessionConfig {
    let secure_cookie =
        std::env::var("SESSION_COOKIE_SECURE").map_or(true, |v| v.trim() != "false");
    if !secure_cookie {
        tracing::warn!("SESSION_COOKIE_SECURE=false: session cookies are sent over plain HTTP");
    }
    let origins = std::env::var("APP_ORIGINS").unwrap_or_else(|_| DEFAULT_APP_ORIGINS.to_owned());
    api::SessionConfig {
        secure_cookie,
        allowed_origins: origins
            .split(',')
            .map(|o| o.trim().trim_end_matches('/').to_owned())
            .filter(|o| !o.is_empty())
            .collect(),
    }
}

/// A quota from `<PREFIX>_PER_MINUTE` and `<PREFIX>_BURST`; unset or
/// invalid values keep `default`.
fn quota(prefix: &str, default: api::Quota) -> api::Quota {
    let read = |suffix: &str, fallback: u32| {
        let name = format!("{prefix}_{suffix}");
        match std::env::var(&name) {
            Ok(raw) => raw
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|v| *v > 0)
                .unwrap_or_else(|| {
                    tracing::warn!("{name} must be a positive integer; using {fallback}");
                    fallback
                }),
            Err(_) => fallback,
        }
    };
    api::Quota::new(
        read("PER_MINUTE", default.per_minute),
        read("BURST", default.burst),
    )
}

/// The client-IP header from `CLIENT_IP_HEADER` (set and overwritten by a
/// trusted front proxy). The deprecated `TRUST_PROXY_HEADERS=true` maps to
/// `x-real-ip`. An invalid name is ignored with a warning.
fn client_ip_header() -> Option<HeaderName> {
    if let Ok(raw) = std::env::var("CLIENT_IP_HEADER") {
        let raw = raw.trim();
        if !raw.is_empty() {
            return match raw.parse::<HeaderName>() {
                Ok(name) => Some(name),
                Err(_) => {
                    tracing::warn!("CLIENT_IP_HEADER is not a valid header name; ignoring it");
                    None
                }
            };
        }
    }
    if std::env::var("TRUST_PROXY_HEADERS").is_ok_and(|v| v.trim() == "true") {
        tracing::warn!(
            "TRUST_PROXY_HEADERS is deprecated; use CLIENT_IP_HEADER=x-real-ip (only X-Real-IP is read)"
        );
        return Some(HeaderName::from_static("x-real-ip"));
    }
    None
}

/// Quotas from `RATE_LIMIT_{FREE,PRO,ANON}_{PER_MINUTE,BURST}`; the client
/// IP comes from [`client_ip_header`], else the TCP peer.
fn rate_limits() -> api::RateLimits {
    let client_ip_header = client_ip_header();
    match &client_ip_header {
        Some(name) => {
            tracing::info!(header = %name, "anonymous rate limits key on this client-IP header")
        }
        None => tracing::warn!(
            "CLIENT_IP_HEADER is unset: anonymous rate limits key on the TCP peer, so everyone \
             behind the web proxy (Next.js /backend rewrite) shares one bucket"
        ),
    }
    api::RateLimits {
        free: quota("RATE_LIMIT_FREE", api::RateLimits::DEFAULT_FREE),
        pro: quota("RATE_LIMIT_PRO", api::RateLimits::DEFAULT_PRO),
        anonymous: quota("RATE_LIMIT_ANON", api::RateLimits::DEFAULT_ANONYMOUS),
        client_ip_header,
    }
}

/// How often expired sessions are deleted.
const SESSION_CLEANUP_EVERY: std::time::Duration = std::time::Duration::from_secs(3600);

/// Accounts need a database; without one every user route answers 503.
/// Also starts the hourly expired-session cleanup.
fn accounts_state(pool: Option<&db::PgPool>) -> api::AccountsState {
    let Some(pool) = pool else {
        return api::AccountsState::disabled();
    };
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let store = api::PgSessionStore::new(pool.clone(), clock.clone());
    let cleanup = store.clone();
    tokio::spawn(async move {
        let mut every = tokio::time::interval(SESSION_CLEANUP_EVERY);
        loop {
            every.tick().await;
            if cleanup.delete_expired().await.is_err() {
                tracing::warn!("deleting expired sessions failed");
            }
        }
    });
    api::AccountsState::new(
        Arc::new(PgUsers::new(pool.clone(), clock)),
        Arc::new(Argon2Hasher::new()),
        api::SessionStoreHandle::new(store),
        session_config(),
    )
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
    let (router, _) = api::build(api::ApiConfig {
        admin: admin_state(pool.as_ref()),
        products: products_state(pool.as_ref()),
        searches: searches_state(pool.as_ref()),
        accounts: accounts_state(pool.as_ref()),
        rate_limits: rate_limits(),
    });
    // Peer addresses feed the per-IP rate limit.
    let app = router.into_make_service_with_connect_info::<std::net::SocketAddr>();
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            // If the signal handler cannot be installed, serve until killed.
            if tokio::signal::ctrl_c().await.is_err() {
                std::future::pending::<()>().await;
            }
        })
        .await
        .context("server error")
}
