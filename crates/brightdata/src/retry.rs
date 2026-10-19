//! Capped exponential backoff for transient failures (behaviour follows
//! brightdata/cli `utils/client.ts`, without its random jitter so delays are
//! deterministic).

use std::future::Future;
use std::time::Duration;

use crate::error::{BrightDataError, Failure};

/// How transient failures are retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Retries after the first attempt (default 3, so at most 4 requests).
    pub max_retries: u32,
    /// Delay before the first retry (default 1 s); doubles each retry.
    pub base_delay: Duration,
    /// Upper bound for any delay, including `Retry-After` (default 30 s).
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl RetryPolicy {
    /// Delay before retry number `attempt + 1`: `min(base · 2^attempt, max)`.
    #[must_use]
    pub fn backoff(&self, attempt: u32) -> Duration {
        let factor = 2_u32.saturating_pow(attempt);
        self.base_delay.saturating_mul(factor).min(self.max_delay)
    }

    /// The delay to use: the server's `Retry-After` when given (clamped to
    /// `max_delay`), else [`RetryPolicy::backoff`].
    #[must_use]
    pub fn delay(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        retry_after.map_or_else(|| self.backoff(attempt), |wait| wait.min(self.max_delay))
    }
}

/// Parses a numeric `Retry-After` header (seconds). HTTP-date values are
/// ignored and fall back to backoff.
#[must_use]
pub fn parse_retry_after(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// Whether repeating a request is harmless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Idempotency {
    /// Reads, cancels and Unlocker fetches: retry every transient failure.
    Safe,
    /// Dataset triggers start a paid collection: retry only when the request
    /// provably was not accepted (429, or no connection was made).
    Unsafe,
}

/// Runs `op` until it succeeds, fails terminally or retries run out.
pub(crate) async fn with_retry<T, F, Fut>(
    policy: &RetryPolicy,
    idempotency: Idempotency,
    mut op: F,
) -> Result<T, BrightDataError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Failure>>,
{
    let mut attempt = 0;
    loop {
        let failure = match op().await {
            Ok(value) => return Ok(value),
            Err(failure) => failure,
        };
        let retryable = failure.error.is_transient()
            && (idempotency == Idempotency::Safe || failure.not_accepted);
        if !retryable || attempt >= policy.max_retries {
            return Err(failure.error);
        }
        tokio::time::sleep(policy.delay(attempt, failure.retry_after)).await;
        attempt += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::{Idempotency, RetryPolicy, parse_retry_after, with_retry};
    use crate::error::{BrightDataError, Failure};
    use http_guard::StatusCode;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    #[test]
    fn backoff_doubles_and_caps() {
        let policy = RetryPolicy::default();
        let secs: Vec<u64> = (0..7).map(|n| policy.backoff(n).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 30, 30]);
        assert_eq!(policy.backoff(u32::MAX), Duration::from_secs(30));
    }

    #[test]
    fn retry_after_is_clamped() {
        let policy = RetryPolicy::default();
        assert_eq!(
            policy.delay(0, Some(Duration::from_secs(5))),
            Duration::from_secs(5)
        );
        assert_eq!(
            policy.delay(0, Some(Duration::from_secs(600))),
            Duration::from_secs(30)
        );
        assert_eq!(policy.delay(2, None), Duration::from_secs(4));
        assert_eq!(parse_retry_after(" 7 "), Some(Duration::from_secs(7)));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
    }

    fn server_error() -> Failure {
        Failure::from_status(StatusCode::INTERNAL_SERVER_ERROR, "", None)
    }

    #[tokio::test(start_paused = true)]
    async fn retries_transient_up_to_the_limit() {
        let counter = AtomicU32::new(0);
        let calls = &counter;
        let result: Result<(), _> =
            with_retry(&RetryPolicy::default(), Idempotency::Safe, || async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(server_error())
            })
            .await;
        assert!(matches!(result, Err(BrightDataError::Transient(_))));
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn terminal_and_unsafe_server_errors_are_not_retried() {
        let counter = AtomicU32::new(0);
        let calls = &counter;
        let _ = with_retry(&RetryPolicy::default(), Idempotency::Safe, || async move {
            calls.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>(Failure::from_status(StatusCode::BAD_REQUEST, "", None))
        })
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let counter = AtomicU32::new(0);
        let calls = &counter;
        let _ = with_retry(
            &RetryPolicy::default(),
            Idempotency::Unsafe,
            || async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(server_error())
            },
        )
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn unsafe_calls_retry_429_then_succeed() {
        let counter = AtomicU32::new(0);
        let calls = &counter;
        let result = with_retry(
            &RetryPolicy::default(),
            Idempotency::Unsafe,
            || async move {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(Failure::from_status(
                        StatusCode::TOO_MANY_REQUESTS,
                        "",
                        Some(Duration::from_secs(2)),
                    ))
                } else {
                    Ok(7)
                }
            },
        )
        .await;
        assert_eq!(result, Ok(7));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
