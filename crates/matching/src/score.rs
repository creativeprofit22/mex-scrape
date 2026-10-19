//! Offer-to-ASIN matching: exact GTIN first, then brand + title scoring.

use std::collections::BTreeSet;
use std::fmt;

use domain::{Asin, Confidence, MatchMethod, MatchStatus, Offer, ProductMatch, UnixMillis};

use crate::normalize::{dice_bp, digit_tokens, tokens};

/// Two candidates whose confidences differ by at most this many basis
/// points are ambiguous: the best one is only stored for review.
pub const AMBIGUITY_MARGIN_BP: u16 = 500;

/// Why a [`MatchConfig`] was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchConfigError {
    /// A threshold is above 10 000 bp.
    AboveMax,
    /// The review floor is above the accept threshold.
    FloorAboveAccept,
}

impl fmt::Display for MatchConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AboveMax => f.write_str("match threshold is above 10000 bp"),
            Self::FloorAboveAccept => f.write_str("review floor is above the accept threshold"),
        }
    }
}

impl std::error::Error for MatchConfigError {}

/// Match thresholds in basis points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchConfig {
    accept_bp: u16,
    floor_bp: u16,
}

impl MatchConfig {
    /// Default accept threshold (85 %).
    pub const DEFAULT_ACCEPT_BP: u16 = 8_500;
    /// Default review floor (30 %).
    pub const DEFAULT_FLOOR_BP: u16 = 3_000;

    /// Matches scoring `>= accept_bp` are accepted; `floor_bp..accept_bp` are
    /// stored for review; below `floor_bp` nothing is stored.
    ///
    /// # Errors
    /// [`MatchConfigError`] unless `floor_bp <= accept_bp <= 10000`.
    pub const fn new(accept_bp: u16, floor_bp: u16) -> Result<Self, MatchConfigError> {
        if accept_bp > 10_000 {
            Err(MatchConfigError::AboveMax)
        } else if floor_bp > accept_bp {
            Err(MatchConfigError::FloorAboveAccept)
        } else {
            Ok(Self {
                accept_bp,
                floor_bp,
            })
        }
    }

    /// Accept threshold.
    #[must_use]
    pub const fn accept_bp(&self) -> u16 {
        self.accept_bp
    }

    /// Review floor.
    #[must_use]
    pub const fn floor_bp(&self) -> u16 {
        self.floor_bp
    }
}

impl Default for MatchConfig {
    fn default() -> Self {
        Self {
            accept_bp: Self::DEFAULT_ACCEPT_BP,
            floor_bp: Self::DEFAULT_FLOOR_BP,
        }
    }
}

/// One Amazon catalog item considered for a match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Amazon product id.
    pub asin: Asin,
    /// Catalog title.
    pub title: String,
    /// Catalog brand, when known.
    pub brand: Option<String>,
}

/// Links `offer` to the best Amazon candidate, or `None` when no candidate
/// is plausible.
///
/// `gtin_hits` are the catalog items Amazon returned for the offer's GTIN
/// (ignored when the offer has none); `keyword_hits` are used only when
/// `gtin_hits` is empty. Never returns [`MatchStatus::Rejected`].
#[must_use]
pub fn match_offer(
    offer: &Offer,
    gtin_hits: &[Candidate],
    keyword_hits: &[Candidate],
    config: &MatchConfig,
    matched_at: UnixMillis,
) -> Option<ProductMatch> {
    let build = |asin: &Asin, method, confidence, status| ProductMatch {
        retailer: offer.retailer.clone(),
        retailer_sku: offer.retailer_sku.clone(),
        asin: asin.clone(),
        method,
        confidence,
        status,
        matched_at,
    };

    if offer.gtin.is_some() && !gtin_hits.is_empty() {
        let ranked = rank(offer, gtin_hits);
        let (best, _) = ranked.first()?;
        let distinct: BTreeSet<&Asin> = gtin_hits.iter().map(|c| &c.asin).collect();
        let single = distinct.len() == 1;
        let brand_conflict = gtin_hits
            .iter()
            .any(|c| brands_conflict(offer.brand.as_deref(), c.brand.as_deref()));
        let status = if single && !brand_conflict {
            MatchStatus::Accepted
        } else {
            MatchStatus::NeedsReview
        };
        return Some(build(best, MatchMethod::Gtin, Confidence::MAX, status));
    }

    let ranked = rank(offer, keyword_hits);
    let (best, top) = ranked.first()?;
    if *top < u32::from(config.floor_bp) {
        return None;
    }
    let ambiguous = ranked
        .get(1)
        .is_some_and(|(_, second)| top - second <= u32::from(AMBIGUITY_MARGIN_BP));
    let status = if *top >= u32::from(config.accept_bp) && !ambiguous {
        MatchStatus::Accepted
    } else {
        MatchStatus::NeedsReview
    };
    Some(build(
        best,
        MatchMethod::Title,
        Confidence::saturating(*top),
        status,
    ))
}

/// Best score per distinct ASIN, sorted by (score desc, ASIN asc).
fn rank<'c>(offer: &Offer, candidates: &'c [Candidate]) -> Vec<(&'c Asin, u32)> {
    let offer_tokens = tokens(&offer.title);
    let mut best: Vec<(&Asin, u32)> = Vec::new();
    for c in candidates {
        let s = score(&offer_tokens, offer.brand.as_deref(), c);
        match best.iter_mut().find(|(a, _)| *a == &c.asin) {
            Some(entry) => entry.1 = entry.1.max(s),
            None => best.push((&c.asin, s)),
        }
    }
    best.sort_by(|(a1, s1), (a2, s2)| s2.cmp(s1).then_with(|| a1.cmp(a2)));
    best
}

/// Confidence of one candidate in basis points.
fn score(offer_tokens: &BTreeSet<String>, offer_brand: Option<&str>, c: &Candidate) -> u32 {
    let cand_tokens = tokens(&c.title);
    let title_bp = dice_bp(offer_tokens, &cand_tokens);
    let brand_tenths = brand_factor_tenths(offer_tokens, offer_brand, &cand_tokens, c);
    let mut conf = title_bp * brand_tenths / 10;
    let a = digit_tokens(offer_tokens);
    let b = digit_tokens(&cand_tokens);
    if !a.is_empty() && !b.is_empty() && a.is_disjoint(&b) {
        conf /= 2;
    }
    conf
}

/// Brand factor in tenths: 10 when the brands agree, 9 when there is no
/// brand evidence, 0 when both brands are known and differ.
fn brand_factor_tenths(
    offer_tokens: &BTreeSet<String>,
    offer_brand: Option<&str>,
    cand_tokens: &BTreeSet<String>,
    c: &Candidate,
) -> u32 {
    let in_title = |brand: &str, title: &BTreeSet<String>| {
        let b = tokens(brand);
        !b.is_empty() && b.is_subset(title)
    };
    match (offer_brand, c.brand.as_deref()) {
        (Some(o), Some(k)) => {
            if tokens(o) == tokens(k) {
                10
            } else {
                0
            }
        }
        (None, Some(k)) if in_title(k, offer_tokens) => 10,
        (Some(o), None) if in_title(o, cand_tokens) => 10,
        _ => 9,
    }
}

/// True when both brands are known and normalize differently.
fn brands_conflict(offer: Option<&str>, cand: Option<&str>) -> bool {
    match (offer, cand) {
        (Some(o), Some(c)) => tokens(o) != tokens(c),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use domain::{Availability, Gtin, PriceMxn, RetailerId, RetailerSku};

    use super::*;

    const AT: UnixMillis = UnixMillis(1_760_000_000_000);

    fn offer(title: &str, brand: Option<&str>, gtin: Option<&str>) -> Offer {
        Offer {
            retailer: RetailerId::new("walmart_mx").unwrap(),
            title: title.into(),
            brand: brand.map(Into::into),
            price_mxn: PriceMxn::from_centavos(129_900).unwrap(),
            list_price_mxn: None,
            availability: Availability::InStock,
            gtin: gtin.map(|g| Gtin::new(g).unwrap()),
            retailer_sku: RetailerSku::new("00750105530007").unwrap(),
            url: "https://www.walmart.com.mx/ip/x/00750105530007".into(),
            membership_required: false,
            fetched_at: AT,
        }
    }

    fn cand(asin: &str, title: &str, brand: Option<&str>) -> Candidate {
        Candidate {
            asin: Asin::new(asin).unwrap(),
            title: title.into(),
            brand: brand.map(Into::into),
        }
    }

    fn run(o: &Offer, gtin: &[Candidate], kw: &[Candidate]) -> Option<ProductMatch> {
        match_offer(o, gtin, kw, &MatchConfig::default(), AT)
    }

    const GTIN: &str = "7501055300075";

    #[test]
    fn single_gtin_hit_is_accepted() {
        let o = offer("Licuadora Oster", Some("Oster"), Some(GTIN));
        let m = run(
            &o,
            &[cand("B000000001", "Oster Blender", Some("Oster"))],
            &[],
        )
        .unwrap();
        assert_eq!(m.asin.as_str(), "B000000001");
        assert_eq!(m.method, MatchMethod::Gtin);
        assert_eq!(m.confidence, Confidence::MAX);
        assert_eq!(m.status, MatchStatus::Accepted);
        assert_eq!(m.retailer.as_str(), "walmart_mx");
        assert_eq!(m.matched_at, AT);
    }

    #[test]
    fn gtin_hit_with_brand_conflict_needs_review() {
        let o = offer("Licuadora Oster", Some("Oster"), Some(GTIN));
        let m = run(&o, &[cand("B000000001", "Licuadora", Some("Taurus"))], &[]).unwrap();
        assert_eq!(m.method, MatchMethod::Gtin);
        assert_eq!(m.status, MatchStatus::NeedsReview);
    }

    #[test]
    fn gtin_with_several_asins_needs_review_and_picks_best_title() {
        let o = offer("Licuadora Oster 10 velocidades", Some("Oster"), Some(GTIN));
        let hits = [
            cand("B000000001", "Batidora Oster", Some("Oster")),
            cand(
                "B000000002",
                "Licuadora Oster 10 velocidades",
                Some("Oster"),
            ),
        ];
        let m = run(&o, &hits, &[]).unwrap();
        assert_eq!(m.asin.as_str(), "B000000002");
        assert_eq!(m.status, MatchStatus::NeedsReview);
    }

    #[test]
    fn gtin_hits_are_ignored_when_offer_has_no_gtin() {
        let o = offer("Licuadora Oster", Some("Oster"), None);
        let hits = [cand("B000000001", "Algo distinto", Some("Oster"))];
        assert_eq!(run(&o, &hits, &[]), None);
    }

    #[test]
    fn near_identical_title_and_brand_is_accepted() {
        let o = offer(
            "Licuadora Oster BLSTPYG1210 de 10 Velocidades",
            Some("Oster"),
            None,
        );
        let kw = [cand(
            "B000000003",
            "Oster Licuadora BLSTPYG1210 10 velocidades",
            Some("OSTER"),
        )];
        let m = run(&o, &[], &kw).unwrap();
        assert_eq!(m.method, MatchMethod::Title);
        assert_eq!(m.confidence.bp(), 10_000);
        assert_eq!(m.status, MatchStatus::Accepted);
    }

    #[test]
    fn accents_and_order_do_not_matter() {
        let o = offer("Cafetera Café Expréss Nescafé Dolce Gusto", None, None);
        let kw = [cand(
            "B000000004",
            "Nescafe Dolce Gusto cafetera cafe express",
            Some("Nescafé"),
        )];
        let m = run(&o, &[], &kw).unwrap();
        assert_eq!(m.confidence.bp(), 10_000);
        assert_eq!(m.status, MatchStatus::Accepted);
    }

    #[test]
    fn size_mismatch_halves_confidence() {
        let o = offer("Shampoo Pantene Restauración 500 ml", Some("Pantene"), None);
        let kw = [cand(
            "B000000005",
            "Shampoo Pantene Restauracion 1 l",
            Some("Pantene"),
        )];
        // Dice 6000 (3 of 5 tokens shared), halved by the size guard.
        let m = run(&o, &[], &kw).unwrap();
        assert_eq!(m.confidence.bp(), 3_000);
        assert_eq!(m.status, MatchStatus::NeedsReview);
    }

    #[test]
    fn brand_conflict_scores_zero() {
        let o = offer("Licuadora 10 velocidades", Some("Oster"), None);
        let kw = [cand(
            "B000000006",
            "Licuadora 10 velocidades",
            Some("Taurus"),
        )];
        assert_eq!(run(&o, &[], &kw), None);
    }

    #[test]
    fn close_runner_up_needs_review() {
        let o = offer("Audifonos Sony WH1000XM5 negro", Some("Sony"), None);
        let kw = [
            cand("B000000007", "Sony audifonos WH1000XM5 negro", Some("Sony")),
            cand("B000000008", "Sony audifonos WH1000XM5 negro", Some("Sony")),
        ];
        let m = run(&o, &[], &kw).unwrap();
        // Identical scores: the smaller ASIN wins, but only for review.
        assert_eq!(m.asin.as_str(), "B000000007");
        assert_eq!(m.status, MatchStatus::NeedsReview);
    }

    #[test]
    fn tie_break_is_independent_of_input_order() {
        let o = offer("Audifonos Sony WH1000XM5 negro", Some("Sony"), None);
        let a = cand("B000000009", "Sony audifonos WH1000XM5 negro", Some("Sony"));
        let b = cand("B000000007", "Sony audifonos WH1000XM5 negro", Some("Sony"));
        let m1 = run(&o, &[], &[a.clone(), b.clone()]).unwrap();
        let m2 = run(&o, &[], &[b, a]).unwrap();
        assert_eq!(m1, m2);
        assert_eq!(m1.asin.as_str(), "B000000007");
    }

    #[test]
    fn below_floor_stores_nothing() {
        let o = offer("Licuadora Oster 10 velocidades", Some("Oster"), None);
        let kw = [cand("B000000010", "Juego de sartenes antiadherentes", None)];
        assert_eq!(run(&o, &[], &kw), None);
        assert_eq!(run(&o, &[], &[]), None);
    }

    #[test]
    fn mid_score_needs_review_never_accepted() {
        let o = offer("Licuadora Oster negra vaso vidrio", Some("Oster"), None);
        let kw = [cand(
            "B000000011",
            "Licuadora Oster roja vaso plastico",
            Some("Oster"),
        )];
        // 3 of 5 tokens shared each side => 6000 bp.
        let m = run(&o, &[], &kw).unwrap();
        assert_eq!(m.confidence.bp(), 6_000);
        assert_eq!(m.status, MatchStatus::NeedsReview);
    }

    #[test]
    fn config_is_validated() {
        assert_eq!(MatchConfig::new(10_001, 0), Err(MatchConfigError::AboveMax));
        assert_eq!(
            MatchConfig::new(5_000, 6_000),
            Err(MatchConfigError::FloorAboveAccept)
        );
        let c = MatchConfig::new(9_000, 9_000).unwrap();
        assert_eq!((c.accept_bp(), c.floor_bp()), (9_000, 9_000));
        let d = MatchConfig::default();
        assert_eq!((d.accept_bp(), d.floor_bp()), (8_500, 3_000));
    }
}
