//! Per-plan rate limits on the user routes (`tower_governor`).
//!
//! `tower_governor` applies one quota per layer, so there is one
//! [`GovernorLayer`] per plan plus one for requests without a signed-in user,
//! and [`rate_limit`] sends each request through the layer for its caller:
//!
//! - signed in: keyed by user id, quota of the user's [`Plan`];
//! - not signed in (sign-up, sign-in, CSRF token, 401s): keyed by client IP.
//!
//! The client IP is the TCP peer (`ConnectInfo`), unless
//! [`RateLimits::client_ip_header`] names one header that a trusted front
//! proxy overwrites (for example `X-Real-IP`). That header must hold exactly
//! one IP; otherwise (missing, repeated, a list, malformed) the peer is used.
//! No other forwarded header is read: the Next.js `/backend` rewrite passes
//! client-sent headers through verbatim, so any of them can be forged. A
//! request with no known IP shares one bucket (fails closed).
//!
//! Over the limit: 429 `rate_limited` with `Retry-After` in whole seconds
//! (at least 1).

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse as _, Response};
use domain::Plan;
use governor::middleware::NoOpMiddleware;
use tower::{Layer as _, ServiceExt as _};
use tower_governor::GovernorError;
use tower_governor::GovernorLayer;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::{KeyExtractor, PeerIpKeyExtractor};

use crate::auth::{ApiError, Auth};

/// A quota: up to `burst` requests at once, refilled evenly to `per_minute`
/// requests per minute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quota {
    /// Requests allowed per minute (sustained). Zero is treated as one.
    pub per_minute: u32,
    /// Requests allowed back to back. Zero is treated as one.
    pub burst: u32,
}

impl Quota {
    /// `per_minute` sustained, `burst` at once.
    #[must_use]
    pub const fn new(per_minute: u32, burst: u32) -> Self {
        Self { per_minute, burst }
    }

    /// Time to refill one request.
    fn period(self) -> Duration {
        let per_minute = if self.per_minute == 0 {
            1
        } else {
            self.per_minute
        };
        // At least 1 ms, so a huge quota never yields a zero period.
        Duration::from_millis((60_000 / u64::from(per_minute)).max(1))
    }

    fn burst(self) -> u32 {
        self.burst.max(1)
    }
}

/// Quotas for the user routes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimits {
    /// Signed-in users on [`Plan::Free`].
    pub free: Quota,
    /// Signed-in users on [`Plan::Pro`].
    pub pro: Quota,
    /// Requests without a signed-in user, per client IP (sign-up, sign-in).
    pub anonymous: Quota,
    /// Header holding the client IP, set (overwritten) by a trusted front
    /// proxy; `None` keys anonymous requests by the TCP peer.
    pub client_ip_header: Option<HeaderName>,
}

impl RateLimits {
    /// Default free quota: 60 requests per minute, burst 30.
    pub const DEFAULT_FREE: Quota = Quota::new(60, 30);
    /// Default pro quota: 300 requests per minute, burst 100.
    pub const DEFAULT_PRO: Quota = Quota::new(300, 100);
    /// Default anonymous quota: 20 requests per minute per IP, burst 10.
    pub const DEFAULT_ANONYMOUS: Quota = Quota::new(20, 10);
}

impl RateLimits {
    /// The default quotas, keyed by the TCP peer (no client-IP header).
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            free: Self::DEFAULT_FREE,
            pro: Self::DEFAULT_PRO,
            anonymous: Self::DEFAULT_ANONYMOUS,
            client_ip_header: None,
        }
    }
}

impl Default for RateLimits {
    fn default() -> Self {
        Self::defaults()
    }
}

/// Rate-limit key of the signed-in user, set by [`rate_limit`] before the
/// per-plan governor runs.
#[derive(Debug, Clone, Copy)]
struct UserKey(i64);

/// Keys by the [`UserKey`] request extension.
#[derive(Debug, Clone, Copy)]
struct UserKeyExtractor;

impl KeyExtractor for UserKeyExtractor {
    type Key = i64;

    fn extract<T>(&self, req: &axum::http::Request<T>) -> Result<i64, GovernorError> {
        req.extensions()
            .get::<UserKey>()
            .map(|k| k.0)
            .ok_or(GovernorError::UnableToExtractKey)
    }
}

/// Keys by client IP; `None` (one shared bucket) when it is unknown.
#[derive(Debug, Clone)]
struct ClientIpExtractor {
    header: Option<HeaderName>,
}

impl ClientIpExtractor {
    /// The IP in the configured header, only if it appears once and holds
    /// exactly one IP.
    fn header_ip<T>(&self, req: &axum::http::Request<T>) -> Option<IpAddr> {
        let mut values = req.headers().get_all(self.header.as_ref()?).iter();
        let value = values.next()?;
        if values.next().is_some() {
            return None;
        }
        value.to_str().ok()?.trim().parse().ok()
    }
}

impl KeyExtractor for ClientIpExtractor {
    type Key = Option<IpAddr>;

    fn extract<T>(&self, req: &axum::http::Request<T>) -> Result<Option<IpAddr>, GovernorError> {
        Ok(self
            .header_ip(req)
            .or_else(|| PeerIpKeyExtractor.extract(req).ok()))
    }
}

/// 429 `rate_limited` with `Retry-After`; anything else is a 500.
fn limited(error: GovernorError) -> Response<Body> {
    match error {
        GovernorError::TooManyRequests { wait_time, .. } => {
            let mut response =
                ApiError(StatusCode::TOO_MANY_REQUESTS, "rate_limited").into_response();
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(wait_time.max(1)));
            response
        }
        GovernorError::UnableToExtractKey | GovernorError::Other { .. } => {
            ApiError::INTERNAL.into_response()
        }
    }
}

type Governed<K> = GovernorLayer<K, NoOpMiddleware, Body>;

fn layer<K: KeyExtractor>(quota: Quota, key: K) -> Governed<K> {
    // `finish` is `None` only for a zero period or burst; `Quota::period`
    // is at least 1 ms and `Quota::burst` at least 1.
    #[expect(clippy::expect_used, reason = "period and burst are never zero")]
    let config = GovernorConfigBuilder::default()
        .period(quota.period())
        .burst_size(quota.burst())
        .key_extractor(key)
        .finish()
        .expect("non-zero period and burst");
    GovernorLayer::new(config).error_handler(limited)
}

/// One governor per plan plus the anonymous one; the limiters are shared by
/// every clone.
#[derive(Clone)]
pub(crate) struct Limiters {
    free: Governed<UserKeyExtractor>,
    pro: Governed<UserKeyExtractor>,
    anonymous: Governed<ClientIpExtractor>,
}

impl Limiters {
    pub(crate) fn new(limits: &RateLimits) -> Arc<Self> {
        Arc::new(Self {
            free: layer(limits.free, UserKeyExtractor),
            pro: layer(limits.pro, UserKeyExtractor),
            anonymous: layer(
                limits.anonymous,
                ClientIpExtractor {
                    header: limits.client_ip_header.clone(),
                },
            ),
        })
    }
}

/// Sends the request through the governor for the caller's plan (or the
/// per-IP one when nobody is signed in). Runs after the auth layer.
pub(crate) async fn rate_limit(
    State(limiters): State<Arc<Limiters>>,
    auth: Auth,
    mut request: Request,
    next: Next,
) -> Response {
    let outcome = match auth.user.as_ref().map(|u| &u.record) {
        Some(user) => {
            request.extensions_mut().insert(UserKey(user.id.get()));
            let governor = match user.plan {
                Plan::Free => &limiters.free,
                Plan::Pro => &limiters.pro,
            };
            governor.layer(next).oneshot(request).await
        }
        None => limiters.anonymous.layer(next).oneshot(request).await,
    };
    match outcome {
        Ok(response) => response,
        Err(never) => match never {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_spreads_a_minute_over_the_quota() {
        assert_eq!(Quota::new(60, 1).period(), Duration::from_secs(1));
        assert_eq!(Quota::new(300, 1).period(), Duration::from_millis(200));
        assert_eq!(Quota::new(0, 0).period(), Duration::from_secs(60));
        assert_eq!(Quota::new(0, 0).burst(), 1);
        assert_eq!(Quota::new(u32::MAX, 5).period(), Duration::from_millis(1));
    }

    #[test]
    fn rate_limited_answers_429_with_retry_after_of_at_least_one_second() {
        let response = limited(GovernorError::TooManyRequests {
            wait_time: 0,
            headers: None,
        });
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response.headers().get(header::RETRY_AFTER),
            Some(&HeaderValue::from(1_u64))
        );
    }

    fn ip(raw: &str) -> IpAddr {
        raw.parse().unwrap_or_else(|_| unreachable!())
    }

    /// The key for a request from peer `192.0.2.1` with `headers`.
    fn key(extractor: &ClientIpExtractor, headers: &[(&str, &str)]) -> Option<IpAddr> {
        let mut builder = axum::http::Request::builder();
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let mut request = builder.body(()).unwrap_or_else(|_| unreachable!());
        request
            .extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [192, 0, 2, 1],
                40_000,
            ))));
        extractor
            .extract(&request)
            .unwrap_or_else(|_| unreachable!())
    }

    #[test]
    fn only_the_configured_client_ip_header_counts() {
        let peer = Some(ip("192.0.2.1"));
        let none = ClientIpExtractor { header: None };
        let real_ip = ClientIpExtractor {
            header: Some(HeaderName::from_static("x-real-ip")),
        };
        // Without a configured header every forwarded header is ignored.
        assert_eq!(key(&none, &[("x-real-ip", "203.0.113.7")]), peer);
        assert_eq!(key(&none, &[("x-forwarded-for", "203.0.113.7")]), peer);
        // The configured header is used.
        assert_eq!(
            key(&real_ip, &[("x-real-ip", " 203.0.113.7 ")]),
            Some(ip("203.0.113.7"))
        );
        assert_eq!(
            key(&real_ip, &[("x-real-ip", "2001:db8::1")]),
            Some(ip("2001:db8::1"))
        );
        // Other forwarded headers stay ignored (forgeable through Next).
        assert_eq!(key(&real_ip, &[("x-forwarded-for", "203.0.113.7")]), peer);
        assert_eq!(key(&real_ip, &[("forwarded", "for=203.0.113.7")]), peer);
        // Missing, list, repeated or malformed values fall back to the peer.
        assert_eq!(key(&real_ip, &[]), peer);
        assert_eq!(
            key(&real_ip, &[("x-real-ip", "203.0.113.7, 198.51.100.2")]),
            peer
        );
        assert_eq!(
            key(
                &real_ip,
                &[("x-real-ip", "203.0.113.7"), ("x-real-ip", "198.51.100.2")]
            ),
            peer
        );
        assert_eq!(key(&real_ip, &[("x-real-ip", "not-an-ip")]), peer);
    }
}
