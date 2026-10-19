//! Per-operation token buckets (GCRA form: a theoretical arrival time per
//! operation), seeded from the usage-plan table and slowed down when the
//! `x-amzn-RateLimit-Limit` header reports a lower rate.

use std::collections::BTreeMap;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::config::{Operation, RateLimit, RateTable};

/// Longest spacing adopted from a header (guards against `0.0001`).
const MAX_PER_REQUEST: Duration = Duration::from_secs(60);

/// Reserves the next slot for a bucket whose theoretical arrival time is
/// `tat`: returns how long to wait from `now` and the new arrival time.
///
/// Up to `burst` requests may start at once; after that one starts every
/// `per_request`.
pub(crate) fn reserve(tat: Option<Instant>, now: Instant, limit: RateLimit) -> (Duration, Instant) {
    let tolerance = limit.per_request.saturating_mul(limit.burst.max(1) - 1);
    let tat = tat.map_or(now, |tat| tat.max(now));
    let start = if tat > now + tolerance {
        tat - tolerance
    } else {
        now
    };
    (start - now, tat + limit.per_request)
}

/// Parses `x-amzn-RateLimit-Limit` (requests per second, e.g. `0.5`) into a
/// spacing. Non-numeric, non-positive or absurdly small rates are ignored.
#[must_use]
pub fn per_request_from_header(value: &str) -> Option<Duration> {
    let rate: f64 = value.trim().parse().ok()?;
    if !rate.is_finite() || rate <= 0.0 {
        return None;
    }
    let spacing = Duration::try_from_secs_f64(1.0 / rate).ok()?;
    Some(spacing.min(MAX_PER_REQUEST))
}

#[derive(Debug)]
struct State {
    limits: BTreeMap<Operation, RateLimit>,
    tat: BTreeMap<Operation, Instant>,
}

/// Spaces SP-API requests per operation. One per client; no global state.
#[derive(Debug)]
pub struct RateLimiter {
    state: Mutex<State>,
}

impl RateLimiter {
    /// A limiter with no requests made yet.
    #[must_use]
    pub fn new(table: RateTable) -> Self {
        let limits = Operation::ALL
            .into_iter()
            .map(|op| (op, table.get(op)))
            .collect();
        Self {
            state: Mutex::new(State {
                limits,
                tat: BTreeMap::new(),
            }),
        }
    }

    /// Waits until a request for `op` may start.
    pub async fn acquire(&self, op: Operation) {
        let wait = {
            let mut state = self.state.lock().await;
            let limit = state.limits.get(&op).copied().unwrap_or(RateLimit {
                per_request: MAX_PER_REQUEST,
                burst: 1,
            });
            let (wait, next) = reserve(state.tat.get(&op).copied(), Instant::now(), limit);
            state.tat.insert(op, next);
            wait
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    /// Adopts a rate reported by Amazon when it is slower than the current
    /// one (never speeds up beyond the table).
    pub async fn observe_limit_header(&self, op: Operation, value: &str) {
        let Some(spacing) = per_request_from_header(value) else {
            return;
        };
        let mut state = self.state.lock().await;
        if let Some(limit) = state.limits.get_mut(&op)
            && spacing > limit.per_request
        {
            limit.per_request = spacing;
        }
    }

    /// The current plan for `op`.
    pub async fn limit(&self, op: Operation) -> Option<RateLimit> {
        self.state.lock().await.limits.get(&op).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[tokio::test(start_paused = true)]
    async fn reserve_allows_a_burst_then_spaces() {
        let limit = RateLimit {
            per_request: MS(500),
            burst: 2,
        };
        let now = Instant::now();
        let (w1, t1) = reserve(None, now, limit);
        let (w2, t2) = reserve(Some(t1), now, limit);
        let (w3, t3) = reserve(Some(t2), now, limit);
        let (w4, _) = reserve(Some(t3), now, limit);
        assert_eq!([w1, w2, w3, w4], [MS(0), MS(0), MS(500), MS(1000)]);
        // Idle long enough: the full burst is available again.
        let later = now + Duration::from_secs(10);
        let (w, t) = reserve(Some(t3), later, limit);
        let (w_next, _) = reserve(Some(t), later, limit);
        assert_eq!([w, w_next], [MS(0), MS(0)]);
    }

    #[tokio::test(start_paused = true)]
    async fn limiter_spaces_each_operation_independently() {
        let limiter = RateLimiter::new(RateTable::default());
        let start = Instant::now();
        limiter.acquire(Operation::ItemOffers).await;
        limiter.acquire(Operation::SearchCatalog).await;
        limiter.acquire(Operation::SearchCatalog).await;
        limiter.acquire(Operation::FeesEstimate).await;
        assert_eq!(start.elapsed(), Duration::ZERO);
        // getItemOffers: 0.5 req/s, burst 1.
        limiter.acquire(Operation::ItemOffers).await;
        assert_eq!(start.elapsed(), Duration::from_secs(2));
        // searchCatalogItems' burst is spent; its next slot (0.5 s) is past.
        limiter.acquire(Operation::SearchCatalog).await;
        assert_eq!(start.elapsed(), Duration::from_secs(2));
    }

    #[tokio::test(start_paused = true)]
    async fn slower_header_rates_are_adopted_faster_ones_ignored() {
        let limiter = RateLimiter::new(RateTable::default());
        limiter
            .observe_limit_header(Operation::FeesEstimate, "0.25")
            .await;
        limiter
            .observe_limit_header(Operation::SearchCatalog, "20")
            .await;
        for junk in ["", "abc", "0", "-1", "NaN", "inf"] {
            limiter
                .observe_limit_header(Operation::ItemOffers, junk)
                .await;
        }
        let fees = limiter.limit(Operation::FeesEstimate).await.unwrap();
        assert_eq!(fees.per_request, Duration::from_secs(4));
        let search = limiter.limit(Operation::SearchCatalog).await.unwrap();
        assert_eq!(search.per_request, MS(500));
        let offers = limiter.limit(Operation::ItemOffers).await.unwrap();
        assert_eq!(offers.per_request, Duration::from_secs(2));
        assert_eq!(
            per_request_from_header("0.00001"),
            Some(Duration::from_secs(60))
        );
    }
}
