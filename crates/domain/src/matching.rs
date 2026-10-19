//! A stored link between a retailer listing and an Amazon product.

use std::fmt;

use crate::{Asin, RetailerId, RetailerSku, UnixMillis};

/// How a match was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchMethod {
    /// Exact barcode (GTIN/EAN/UPC) lookup.
    Gtin,
    /// Normalized brand + title similarity.
    Title,
}

impl MatchMethod {
    /// Stable lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gtin => "gtin",
            Self::Title => "title",
        }
    }

    /// Reads a name produced by [`Self::as_str`].
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "gtin" => Some(Self::Gtin),
            "title" => Some(Self::Title),
            _ => None,
        }
    }
}

/// Review state of a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchStatus {
    /// Confident enough to use without review.
    Accepted,
    /// Plausible but below the accept threshold or ambiguous.
    NeedsReview,
    /// Rejected by a reviewer; never set by the automatic matcher.
    Rejected,
}

impl MatchStatus {
    /// Stable lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::NeedsReview => "needs_review",
            Self::Rejected => "rejected",
        }
    }

    /// Reads a name produced by [`Self::as_str`].
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "accepted" => Some(Self::Accepted),
            "needs_review" => Some(Self::NeedsReview),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }
}

/// A [`Confidence`] above 10 000 basis points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfidenceError(pub u32);

impl fmt::Display for ConfidenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "confidence {} bp is above 10000", self.0)
    }
}

impl std::error::Error for ConfidenceError {}

/// Match confidence in basis points, `0..=10000` (10000 = certain).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Confidence(u16);

impl Confidence {
    /// Full confidence (10 000 bp).
    pub const MAX: Self = Self(10_000);

    /// Validates a basis-point value.
    ///
    /// # Errors
    /// [`ConfidenceError`] if `bp > 10000`.
    pub fn from_bp(bp: u32) -> Result<Self, ConfidenceError> {
        u16::try_from(bp)
            .ok()
            .filter(|v| *v <= 10_000)
            .map(Self)
            .ok_or(ConfidenceError(bp))
    }

    /// `bp` clamped to `0..=10000`.
    #[must_use]
    pub const fn saturating(bp: u32) -> Self {
        if bp >= 10_000 {
            Self::MAX
        } else {
            // bp < 10_000, so the cast cannot truncate.
            Self(bp as u16)
        }
    }

    /// The value in basis points.
    #[must_use]
    pub const fn bp(self) -> u16 {
        self.0
    }
}

/// A retailer listing linked to an Amazon product.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductMatch {
    /// Retailer of the listing.
    pub retailer: RetailerId,
    /// The retailer's product id.
    pub retailer_sku: RetailerSku,
    /// Matched Amazon product.
    pub asin: Asin,
    /// How the match was found.
    pub method: MatchMethod,
    /// How sure the matcher is.
    pub confidence: Confidence,
    /// Review state.
    pub status: MatchStatus,
    /// When the match was made.
    pub matched_at: UnixMillis,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for m in [MatchMethod::Gtin, MatchMethod::Title] {
            assert_eq!(MatchMethod::parse(m.as_str()), Some(m));
        }
        for s in [
            MatchStatus::Accepted,
            MatchStatus::NeedsReview,
            MatchStatus::Rejected,
        ] {
            assert_eq!(MatchStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(MatchMethod::parse("GTIN"), None);
        assert_eq!(MatchStatus::parse(""), None);
    }

    #[test]
    fn confidence_is_bounded() {
        assert_eq!(Confidence::from_bp(0).unwrap().bp(), 0);
        assert_eq!(Confidence::from_bp(10_000), Ok(Confidence::MAX));
        assert_eq!(Confidence::from_bp(10_001), Err(ConfidenceError(10_001)));
        assert_eq!(Confidence::from_bp(70_000), Err(ConfidenceError(70_000)));
        assert_eq!(Confidence::saturating(70_000), Confidence::MAX);
        assert_eq!(Confidence::saturating(8_500).bp(), 8_500);
    }
}
