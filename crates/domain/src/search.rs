//! Product searches run as background jobs (ADR 0011).

use std::fmt;

use crate::UserId;

/// Longest accepted search query, in characters.
pub const MAX_SEARCH_QUERY_CHARS: usize = 100;

/// Why a [`SearchQuery`] was rejected. The query text is never echoed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchQueryError {
    /// Empty or whitespace only.
    Empty,
    /// Longer than [`MAX_SEARCH_QUERY_CHARS`] after whitespace cleanup.
    TooLong,
}

impl fmt::Display for SearchQueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("search query is empty"),
            Self::TooLong => write!(
                f,
                "search query is longer than {MAX_SEARCH_QUERY_CHARS} characters"
            ),
        }
    }
}

impl std::error::Error for SearchQueryError {}

/// A keyword search: trimmed, inner whitespace collapsed to single spaces,
/// 1..=100 characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SearchQuery(String);

impl SearchQuery {
    /// Cleans and validates a raw query.
    ///
    /// # Errors
    /// [`SearchQueryError`] if empty or too long.
    pub fn parse(raw: &str) -> Result<Self, SearchQueryError> {
        let q = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        if q.is_empty() {
            Err(SearchQueryError::Empty)
        } else if q.chars().count() > MAX_SEARCH_QUERY_CHARS {
            Err(SearchQueryError::TooLong)
        } else {
            Ok(Self(q))
        }
    }

    /// The cleaned query.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A tenant id that is not 1..=100 ASCII letters, digits, `_`, `.`, `:` or
/// `-`. The value is never echoed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantIdError;

impl fmt::Display for TenantIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("tenant id must be 1-100 ASCII letters, digits, '_', '.', ':' or '-'")
    }
}

impl std::error::Error for TenantIdError {}

/// Who a job, search or paid request belongs to. Same character rule as the
/// `tenant_id` columns and the Bright Data run scope.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TenantId(String);

impl TenantId {
    /// Tenant used by API searches until accounts exist (ADR 0011).
    pub const DEFAULT: &'static str = "default";
    /// Tenant used by scheduled jobs (refresh, category browse).
    pub const SYSTEM: &'static str = "system";

    /// Validates a tenant id.
    ///
    /// # Errors
    /// [`TenantIdError`] if empty, longer than 100 bytes or using other
    /// characters.
    pub fn new(value: &str) -> Result<Self, TenantIdError> {
        let ok = !value.is_empty()
            && value.len() <= 100
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'));
        if ok {
            Ok(Self(value.to_owned()))
        } else {
            Err(TenantIdError)
        }
    }

    /// The tenant owning a user's searches: `user-<id>`.
    #[must_use]
    pub fn for_user(user: UserId) -> Self {
        Self(format!("user-{user}"))
    }

    /// The `default` tenant.
    #[must_use]
    pub fn default_tenant() -> Self {
        Self(Self::DEFAULT.to_owned())
    }

    /// The `system` tenant.
    #[must_use]
    pub fn system() -> Self {
        Self(Self::SYSTEM.to_owned())
    }

    /// The id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Lifecycle of a search job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SearchStatus {
    /// Stored and enqueued, not yet picked up.
    Queued,
    /// A worker is running it.
    Running,
    /// Every retailer and the Amazon source answered.
    Done,
    /// Finished, but some retailers failed or Amazon calls were refused.
    Partial,
    /// Failed after the retry budget.
    Failed,
    /// Refused before any paid call (kill switch, disabled, cap).
    Refused,
}

impl SearchStatus {
    /// Stable lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::Refused => "refused",
        }
    }

    /// Reads a name produced by [`Self::as_str`].
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "done" => Some(Self::Done),
            "partial" => Some(Self::Partial),
            "failed" => Some(Self::Failed),
            "refused" => Some(Self::Refused),
            _ => None,
        }
    }

    /// Whether the search will not change any more.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Queued | Self::Running)
    }
}

/// What a search got from one retailer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RetailerSearchOutcome {
    /// Results page fetched and parsed.
    Ok,
    /// The store has no keyword search; it is covered by scheduled category
    /// browsing instead (ADR 0008).
    CategoryOnly,
    /// Bright Data refused the URL until KYC is approved.
    BlockedNeedsKyc,
    /// robots.txt disallows the URL.
    SkippedRobots,
    /// Fetch or parse failed.
    Failed,
    /// Fetched and parsed, but no offers.
    NoOffers,
}

impl RetailerSearchOutcome {
    /// Stable lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::CategoryOnly => "category_only",
            Self::BlockedNeedsKyc => "blocked_needs_kyc",
            Self::SkippedRobots => "skipped_robots",
            Self::Failed => "failed",
            Self::NoOffers => "no_offers",
        }
    }

    /// Reads a name produced by [`Self::as_str`].
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ok" => Some(Self::Ok),
            "category_only" => Some(Self::CategoryOnly),
            "blocked_needs_kyc" => Some(Self::BlockedNeedsKyc),
            "skipped_robots" => Some(Self::SkippedRobots),
            "failed" => Some(Self::Failed),
            "no_offers" => Some(Self::NoOffers),
            _ => None,
        }
    }

    /// Whether this outcome makes the search partial (a real failure, not a
    /// known limitation such as category-only or an empty result).
    #[must_use]
    pub const fn is_failure(self) -> bool {
        matches!(
            self,
            Self::BlockedNeedsKyc | Self::SkippedRobots | Self::Failed
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_is_trimmed_and_collapsed() {
        let q = SearchQuery::parse("  audífonos \t bluetooth\n").unwrap();
        assert_eq!(q.as_str(), "audífonos bluetooth");
    }

    #[test]
    fn query_rejects_empty_and_too_long() {
        assert_eq!(SearchQuery::parse(" \n\t"), Err(SearchQueryError::Empty));
        let ok = "é".repeat(MAX_SEARCH_QUERY_CHARS);
        assert!(SearchQuery::parse(&ok).is_ok());
        let long = "é".repeat(MAX_SEARCH_QUERY_CHARS + 1);
        assert_eq!(SearchQuery::parse(&long), Err(SearchQueryError::TooLong));
    }

    #[test]
    fn query_error_does_not_echo_input() {
        let long = "secretish".repeat(20);
        let msg = SearchQuery::parse(&long).unwrap_err().to_string();
        assert!(!msg.contains("secretish"));
    }

    #[test]
    fn tenant_id_validates() {
        assert_eq!(
            TenantId::new("acme:mx-1.a_b").unwrap().as_str(),
            "acme:mx-1.a_b"
        );
        assert_eq!(TenantId::default_tenant().as_str(), "default");
        assert_eq!(TenantId::system().as_str(), "system");
        assert_eq!(TenantId::new(""), Err(TenantIdError));
        assert_eq!(TenantId::new("a b"), Err(TenantIdError));
        assert_eq!(TenantId::new("ñ"), Err(TenantIdError));
        assert_eq!(TenantId::new(&"a".repeat(101)), Err(TenantIdError));
        assert!(TenantId::new(&"a".repeat(100)).is_ok());
    }

    #[test]
    fn user_tenant_is_valid_and_distinct() {
        let t = TenantId::for_user(UserId::new(42).unwrap());
        assert_eq!(t.as_str(), "user-42");
        assert_eq!(TenantId::new(t.as_str()), Ok(t.clone()));
        assert_ne!(t, TenantId::for_user(UserId::new(4).unwrap()));
        let max = TenantId::for_user(UserId::new(i64::MAX).unwrap());
        assert!(TenantId::new(max.as_str()).is_ok());
    }

    #[test]
    fn status_round_trips() {
        for s in [
            SearchStatus::Queued,
            SearchStatus::Running,
            SearchStatus::Done,
            SearchStatus::Partial,
            SearchStatus::Failed,
            SearchStatus::Refused,
        ] {
            assert_eq!(SearchStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(SearchStatus::parse("DONE"), None);
        assert!(!SearchStatus::Running.is_terminal());
        assert!(SearchStatus::Refused.is_terminal());
    }

    #[test]
    fn outcome_round_trips() {
        for o in [
            RetailerSearchOutcome::Ok,
            RetailerSearchOutcome::CategoryOnly,
            RetailerSearchOutcome::BlockedNeedsKyc,
            RetailerSearchOutcome::SkippedRobots,
            RetailerSearchOutcome::Failed,
            RetailerSearchOutcome::NoOffers,
        ] {
            assert_eq!(RetailerSearchOutcome::parse(o.as_str()), Some(o));
        }
        assert!(!RetailerSearchOutcome::CategoryOnly.is_failure());
        assert!(RetailerSearchOutcome::Failed.is_failure());
    }
}
