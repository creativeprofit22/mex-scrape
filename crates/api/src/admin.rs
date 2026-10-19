//! Read-only admin endpoints, protected by a bearer token.
//!
//! The token comes from `ADMIN_API_TOKEN`. Without a configured token (or
//! without a database) every admin route answers 503, so the endpoint fails
//! closed; a missing or wrong token answers 401.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header::AUTHORIZATION};
use db::PgPool;
use db::brightdata_runs::{HostAccessCounts, access_counts_by_host};
use serde::Serialize;
use utoipa::ToSchema;

/// Shortest `ADMIN_API_TOKEN` accepted; shorter values disable the endpoint.
pub const MIN_TOKEN_LEN: usize = 16;

/// Boxed future so the source can be a trait object.
pub type CountsFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<HostAccessCounts>, ()>> + Send + 'a>>;

/// Where per-host access counts come from; faked in tests.
pub trait AccessSource: Send + Sync {
    /// Outcome counts per host. The error carries no detail on purpose.
    fn counts(&self) -> CountsFuture<'_>;
}

/// [`AccessSource`] over Postgres.
pub struct PgAccessSource(pub PgPool);

impl AccessSource for PgAccessSource {
    fn counts(&self) -> CountsFuture<'_> {
        Box::pin(async move {
            access_counts_by_host(&self.0).await.map_err(|_| {
                tracing::warn!("reading Bright Data access counts failed");
            })
        })
    }
}

/// State of the admin routes. No `Debug`: it holds the token.
#[derive(Clone)]
pub struct AdminState {
    token: Option<Arc<str>>,
    source: Option<Arc<dyn AccessSource>>,
}

impl AdminState {
    /// Admin routes answer 503.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            token: None,
            source: None,
        }
    }

    /// Enabled with `token` and `source`. A token shorter than
    /// [`MIN_TOKEN_LEN`] bytes leaves the routes disabled.
    #[must_use]
    pub fn new(token: &str, source: Arc<dyn AccessSource>) -> Self {
        if token.len() < MIN_TOKEN_LEN {
            return Self::disabled();
        }
        Self {
            token: Some(Arc::from(token)),
            source: Some(source),
        }
    }
}

/// Access outcomes for one retailer host.
#[derive(Debug, Serialize, ToSchema)]
pub struct RetailerAccessCounts {
    /// Retailer id from the registry, or the host when it is not registered.
    pub retailer: String,
    /// Target host.
    pub host: String,
    /// Pages fetched.
    pub ok: i64,
    /// Refusals that need KYC (`blocked_needs_kyc`).
    pub blocked_needs_kyc: i64,
    /// URLs skipped because robots.txt disallows them.
    pub skipped_robots: i64,
    /// Other failures.
    pub failed: i64,
}

fn to_response(counts: Vec<HostAccessCounts>) -> Vec<RetailerAccessCounts> {
    counts
        .into_iter()
        .map(|c| RetailerAccessCounts {
            retailer: retailers::retailer_for_host(&c.host)
                .map_or_else(|| c.host.clone(), |r| r.id.to_owned()),
            host: c.host,
            ok: c.ok,
            blocked_needs_kyc: c.blocked_needs_kyc,
            skipped_robots: c.skipped_robots,
            failed: c.failed,
        })
        .collect()
}

/// Compares in time independent of where the bytes first differ.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= usize::from(a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0));
    }
    diff == 0
}

fn authorized(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|given| constant_time_eq(given.as_bytes(), token.as_bytes()))
}

/// Bright Data access outcomes per retailer host (admin only).
#[utoipa::path(
    get,
    path = "/admin/brightdata/access",
    operation_id = "brightdata_access",
    security(("admin_bearer" = [])),
    responses(
        (status = OK, description = "Outcome counts per host", body = Vec<RetailerAccessCounts>),
        (status = UNAUTHORIZED, description = "Missing or wrong bearer token"),
        (status = INTERNAL_SERVER_ERROR, description = "Counts could not be read"),
        (status = SERVICE_UNAVAILABLE, description = "Admin API is not configured")
    )
)]
pub(crate) async fn brightdata_access(
    State(state): State<AdminState>,
    headers: HeaderMap,
) -> Result<Json<Vec<RetailerAccessCounts>>, StatusCode> {
    let (Some(token), Some(source)) = (&state.token, &state.source) else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };
    if !authorized(&headers, token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let counts = source
        .counts()
        .await
        .map_err(|()| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(to_response(counts)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_matches_equality() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"", b"a"));
    }
}
