//! Product matching between Mexican retailer offers and Amazon.com.mx
//! catalog items.
//!
//! Pure logic only; depends on `domain`. [`match_offer`] links an offer to
//! an ASIN by exact GTIN first, then by normalized brand + title similarity
//! with a confidence in basis points. Low-confidence or ambiguous matches
//! come back as [`domain::MatchStatus::NeedsReview`], never accepted. See
//! `docs/modules/matching.md` for the rules and thresholds.

mod normalize;
mod score;

pub use normalize::{dice_bp, digit_tokens, tokens};
pub use score::{AMBIGUITY_MARGIN_BP, Candidate, MatchConfig, MatchConfigError, match_offer};
