//! Per-zone request spacing. Zone names are secrets, so limits are keyed by
//! product: the configured Web Unlocker zone and the account's Datasets API.

use std::collections::BTreeMap;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

/// Which zone a request counts against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RateKey {
    /// The configured Web Unlocker zone.
    Unlocker,
    /// Datasets v3 (trigger, progress, download, cancel).
    Datasets,
}

/// Minimum spacing between consecutive requests per [`RateKey`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimits {
    /// Spacing for Web Unlocker requests (default 500 ms, ≤ 2 req/s).
    pub unlocker_interval: Duration,
    /// Spacing for Datasets v3 requests (default 1 s).
    pub datasets_interval: Duration,
}

impl Default for RateLimits {
    fn default() -> Self {
        Self {
            unlocker_interval: Duration::from_millis(500),
            datasets_interval: Duration::from_secs(1),
        }
    }
}

impl RateLimits {
    fn interval(&self, key: RateKey) -> Duration {
        match key {
            RateKey::Unlocker => self.unlocker_interval,
            RateKey::Datasets => self.datasets_interval,
        }
    }
}

/// Reserves the next slot: returns how long to wait from `now` and the
/// earliest start of the following request.
pub(crate) fn reserve(
    next: Option<Instant>,
    now: Instant,
    interval: Duration,
) -> (Duration, Instant) {
    let start = next.map_or(now, |next| next.max(now));
    (start - now, start + interval)
}

/// Spaces requests per zone. One per client; no global state.
#[derive(Debug)]
pub struct ZoneLimiter {
    limits: RateLimits,
    next: Mutex<BTreeMap<RateKey, Instant>>,
}

impl ZoneLimiter {
    /// A limiter with no requests made yet.
    #[must_use]
    pub fn new(limits: RateLimits) -> Self {
        Self {
            limits,
            next: Mutex::new(BTreeMap::new()),
        }
    }

    /// Waits until a request for `key` may start.
    pub async fn acquire(&self, key: RateKey) {
        let wait = {
            let mut next = self.next.lock().await;
            let now = Instant::now();
            let (wait, following) =
                reserve(next.get(&key).copied(), now, self.limits.interval(key));
            next.insert(key, following);
            wait
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RateKey, RateLimits, ZoneLimiter, reserve};
    use std::time::Duration;
    use tokio::time::Instant;

    #[tokio::test(start_paused = true)]
    async fn reserve_spaces_requests() {
        let now = Instant::now();
        let interval = Duration::from_millis(500);
        let (wait, next) = reserve(None, now, interval);
        assert_eq!(wait, Duration::ZERO);
        let (wait, next) = reserve(Some(next), now, interval);
        assert_eq!(wait, interval);
        // Long idle: no wait, and the slot restarts from now.
        let later = next + Duration::from_secs(10);
        let (wait, following) = reserve(Some(next), later, interval);
        assert_eq!(wait, Duration::ZERO);
        assert_eq!(following, later + interval);
    }

    #[tokio::test(start_paused = true)]
    async fn limiter_waits_per_key() {
        let limiter = ZoneLimiter::new(RateLimits::default());
        let start = Instant::now();
        limiter.acquire(RateKey::Unlocker).await;
        limiter.acquire(RateKey::Datasets).await;
        assert_eq!(start.elapsed(), Duration::ZERO);
        limiter.acquire(RateKey::Unlocker).await;
        assert_eq!(start.elapsed(), Duration::from_millis(500));
        limiter.acquire(RateKey::Datasets).await;
        assert_eq!(start.elapsed(), Duration::from_secs(1));
    }
}
