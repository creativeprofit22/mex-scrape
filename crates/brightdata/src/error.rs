//! Terminal vs transient error classes.

use std::time::Duration;

use http_guard::{GuardError, StatusCode, truncate_error_message};

/// A failed Bright Data call. Messages are redacted (API key, zone name),
/// URL-free and bounded, so they are safe to log, store on a run or return.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BrightDataError {
    /// Retrying will not help: bad request, auth, unknown snapshot, invalid
    /// input or an unexpected response shape.
    #[error("{0}")]
    Terminal(String),
    /// Rate limit, server or network trouble; the call may succeed later.
    #[error("{0}")]
    Transient(String),
}

impl BrightDataError {
    /// True for [`BrightDataError::Transient`].
    #[must_use]
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Transient(_))
    }

    /// The (already redacted) message.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::Terminal(message) | Self::Transient(message) => message,
        }
    }
}

/// True when an HTTP status is worth retrying: 408, 429 and 5xx. Every other
/// non-success status (400, 401, 403, 404, 422 and the rest of 4xx) is
/// terminal, so a paid call is never repeated blindly.
#[must_use]
pub fn is_transient_status(status: u16) -> bool {
    status == 408 || status == 429 || (500..=599).contains(&status)
}

/// One failed attempt, with what the retry loop needs to know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    pub(crate) error: BrightDataError,
    /// Server-requested delay (`Retry-After` on 429).
    pub(crate) retry_after: Option<Duration>,
    /// The request provably was not accepted (429 or no connection), so even
    /// a non-idempotent call may be retried.
    pub(crate) not_accepted: bool,
}

impl Failure {
    /// A terminal failure that is never retried.
    pub(crate) fn terminal(message: impl Into<String>) -> Self {
        Self {
            error: BrightDataError::Terminal(message.into()),
            retry_after: None,
            not_accepted: false,
        }
    }

    /// Classifies a non-success HTTP status. `detail` must already be
    /// redacted; the final message (prefix included) is truncated.
    pub(crate) fn from_status(
        status: StatusCode,
        detail: &str,
        retry_after: Option<Duration>,
    ) -> Self {
        let code = status.as_u16();
        let message = if detail.is_empty() {
            format!("Bright Data request failed with HTTP {code}")
        } else {
            format!("Bright Data request failed with HTTP {code}: {detail}")
        };
        let message = truncate_error_message(&message);
        let error = if is_transient_status(code) {
            BrightDataError::Transient(message)
        } else {
            BrightDataError::Terminal(message)
        };
        Self {
            error,
            retry_after: if code == 429 { retry_after } else { None },
            not_accepted: code == 429,
        }
    }

    /// Classifies a transport error (already redacted and URL-free); the
    /// final message (prefix included) is truncated.
    pub(crate) fn from_guard(error: &GuardError) -> Self {
        let not_accepted = matches!(error, GuardError::Connect(_));
        let error = if error.is_transient() {
            BrightDataError::Transient(truncate_error_message(&format!(
                "Bright Data could not be reached: {error}"
            )))
        } else {
            BrightDataError::Terminal(truncate_error_message(&format!(
                "Bright Data request was refused: {error}"
            )))
        };
        Self {
            error,
            retry_after: None,
            not_accepted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BrightDataError, Failure, is_transient_status};
    use http_guard::{GuardError, MAX_ERROR_MESSAGE_CHARS, StatusCode};
    use std::time::Duration;

    #[test]
    fn status_classes() {
        for terminal in [400, 401, 403, 404, 410, 422] {
            assert!(!is_transient_status(terminal), "{terminal}");
        }
        for transient in [408, 429, 500, 502, 503, 504] {
            assert!(is_transient_status(transient), "{transient}");
        }
    }

    #[test]
    fn only_429_keeps_retry_after_and_counts_as_not_accepted() {
        let wait = Some(Duration::from_secs(3));
        let limited = Failure::from_status(StatusCode::TOO_MANY_REQUESTS, "slow down", wait);
        assert_eq!(limited.retry_after, wait);
        assert!(limited.not_accepted);
        assert!(limited.error.is_transient());
        assert_eq!(
            limited.error.message(),
            "Bright Data request failed with HTTP 429: slow down"
        );
        let server = Failure::from_status(StatusCode::BAD_GATEWAY, "", wait);
        assert_eq!(server.retry_after, None);
        assert!(!server.not_accepted);
        assert!(server.error.is_transient());
        let bad = Failure::from_status(StatusCode::UNPROCESSABLE_ENTITY, "", None);
        assert!(matches!(bad.error, BrightDataError::Terminal(_)));
    }

    #[test]
    fn guard_errors_map_by_transience() {
        let connect = Failure::from_guard(&GuardError::Connect("refused".to_owned()));
        assert!(connect.error.is_transient());
        assert!(connect.not_accepted);
        let timeout = Failure::from_guard(&GuardError::Timeout);
        assert!(timeout.error.is_transient());
        assert!(!timeout.not_accepted);
        let too_large = Failure::from_guard(&GuardError::BodyTooLarge { limit: 10 });
        assert!(!too_large.error.is_transient());
    }

    #[test]
    fn messages_stay_bounded_after_the_prefix() {
        let status = Failure::from_status(StatusCode::BAD_REQUEST, &"x".repeat(600), None);
        let message = status.error.message();
        assert!(message.chars().count() <= MAX_ERROR_MESSAGE_CHARS + 1);
        assert!(message.starts_with("Bright Data request failed with HTTP 400: "));
        assert!(message.ends_with('…'));

        let guard = Failure::from_guard(&GuardError::Connect("x".repeat(600)));
        assert!(guard.error.message().chars().count() <= MAX_ERROR_MESSAGE_CHARS + 1);
    }
}
