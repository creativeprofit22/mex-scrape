//! Matches a fetched retailer offer to an Amazon product and stores the
//! result: records the price observation, looks the offer up on Amazon
//! (GTIN first, keywords only when the GTIN finds nothing), scores the
//! candidates with `matching` and upserts the match.

use app::{
    AmazonSource, AmazonSourceError, CatalogItem, MatchRepo, PriceHistoryRepo, UpsertOutcome,
};
use domain::{Offer, ProductMatch, UnixMillis};
use matching::{Candidate, MatchConfig, match_offer};

/// What [`match_and_store`] stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchOutcome {
    /// This match was stored.
    Stored(ProductMatch),
    /// A match was found, but a reviewer had rejected the listing's stored
    /// match, so the rejection was kept and nothing new was stored.
    KeptRejected,
    /// No plausible Amazon candidate; nothing was stored.
    NoCandidate,
}

/// Why [`match_and_store`] failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OfferMatchError {
    /// The observation or the match could not be stored.
    #[error("price history or match storage unavailable")]
    Storage,
    /// The Amazon lookup failed.
    #[error("Amazon lookup failed: {0}")]
    Amazon(AmazonSourceError),
}

/// Builds the keyword query (brand first, then title) with whitespace
/// collapsed, cut on a word boundary to at most
/// [`amazon::MAX_KEYWORD_CHARS`] characters, which both Amazon adapters
/// enforce. A single word longer than the limit is cut mid-word on a char
/// boundary.
fn keyword_query(brand: Option<&str>, title: &str) -> String {
    let mut query = String::new();
    let mut len = 0;
    for word in brand
        .into_iter()
        .flat_map(str::split_whitespace)
        .chain(title.split_whitespace())
    {
        let sep = usize::from(!query.is_empty());
        let word_len = word.chars().count();
        if len + sep + word_len > amazon::MAX_KEYWORD_CHARS {
            if query.is_empty() {
                query.extend(word.chars().take(amazon::MAX_KEYWORD_CHARS));
            }
            break;
        }
        if sep == 1 {
            query.push(' ');
        }
        query.push_str(word);
        len += sep + word_len;
    }
    query
}

fn candidates(items: Vec<CatalogItem>) -> Vec<Candidate> {
    items
        .into_iter()
        .map(|i| Candidate {
            asin: i.asin,
            title: i.title,
            brand: i.brand,
        })
        .collect()
}

/// Records `offer`'s price, then matches and stores it. The observation is
/// recorded even when the Amazon lookup fails.
///
/// # Errors
/// [`OfferMatchError::Storage`] if a write fails;
/// [`OfferMatchError::Amazon`] if a lookup fails (no match is stored).
pub async fn match_and_store(
    amazon: &dyn AmazonSource,
    matches: &dyn MatchRepo,
    prices: &dyn PriceHistoryRepo,
    offer: &Offer,
    config: &MatchConfig,
    now: UnixMillis,
) -> Result<MatchOutcome, OfferMatchError> {
    prices
        .record(offer)
        .await
        .map_err(|_| OfferMatchError::Storage)?;

    let gtin_hits = match &offer.gtin {
        Some(gtin) => candidates(
            amazon
                .catalog_by_gtin(gtin)
                .await
                .map_err(OfferMatchError::Amazon)?
                .items,
        ),
        None => Vec::new(),
    };
    let keyword_hits = if gtin_hits.is_empty() {
        let keywords = keyword_query(offer.brand.as_deref(), &offer.title);
        candidates(
            amazon
                .catalog_by_keyword(&keywords)
                .await
                .map_err(OfferMatchError::Amazon)?
                .items,
        )
    } else {
        Vec::new()
    };

    let Some(found) = match_offer(offer, &gtin_hits, &keyword_hits, config, now) else {
        return Ok(MatchOutcome::NoCandidate);
    };
    let written = matches
        .upsert(&found)
        .await
        .map_err(|_| OfferMatchError::Storage)?;
    Ok(match written {
        UpsertOutcome::Written => MatchOutcome::Stored(found),
        UpsertOutcome::KeptRejected => MatchOutcome::KeptRejected,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use app::{
        AmazonDataSource, CatalogResult, FeeEstimate, Fulfillment, OfferSummary, PortFuture,
        PricePoint, RepoError,
    };
    use domain::{
        Asin, Availability, Gtin, MatchMethod, MatchStatus, PriceMxn, RetailerId, RetailerSku,
    };

    use super::*;

    const NOW: UnixMillis = UnixMillis(1_760_000_000_000);

    /// Answers lookups from fixed results and logs which were called.
    struct FakeAmazon {
        by_gtin: Result<Vec<CatalogItem>, AmazonSourceError>,
        by_keyword: Result<Vec<CatalogItem>, AmazonSourceError>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeAmazon {
        fn new(
            by_gtin: Result<Vec<CatalogItem>, AmazonSourceError>,
            by_keyword: Result<Vec<CatalogItem>, AmazonSourceError>,
        ) -> Self {
            Self {
                by_gtin,
                by_keyword,
                calls: Mutex::new(Vec::new()),
            }
        }

        fn result(
            items: &Result<Vec<CatalogItem>, AmazonSourceError>,
        ) -> Result<CatalogResult, AmazonSourceError> {
            items.clone().map(|items| CatalogResult {
                items,
                source: AmazonDataSource::SpApi,
                fetched_at: NOW,
            })
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl AmazonSource for FakeAmazon {
        fn catalog_by_gtin<'a>(
            &'a self,
            gtin: &'a Gtin,
        ) -> PortFuture<'a, Result<CatalogResult, AmazonSourceError>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("gtin:{}", gtin.as_str()));
            Box::pin(async move { Self::result(&self.by_gtin) })
        }

        fn catalog_by_keyword<'a>(
            &'a self,
            keywords: &'a str,
        ) -> PortFuture<'a, Result<CatalogResult, AmazonSourceError>> {
            self.calls.lock().unwrap().push(format!("kw:{keywords}"));
            // Mirrors the adapters' `valid_keywords` length check.
            if keywords.chars().count() > amazon::MAX_KEYWORD_CHARS {
                return Box::pin(async { Err(AmazonSourceError::InvalidInput) });
            }
            Box::pin(async move { Self::result(&self.by_keyword) })
        }

        fn offers<'a>(
            &'a self,
            _: &'a Asin,
        ) -> PortFuture<'a, Result<OfferSummary, AmazonSourceError>> {
            Box::pin(async { Err(AmazonSourceError::Unsupported) })
        }

        fn fees<'a>(
            &'a self,
            _: &'a Asin,
            _: PriceMxn,
            _: Fulfillment,
        ) -> PortFuture<'a, Result<FeeEstimate, AmazonSourceError>> {
            Box::pin(async { Err(AmazonSourceError::Unsupported) })
        }
    }

    /// Logs upserted matches and answers every upsert with `answer`.
    struct FakeMatches {
        upserted: Mutex<Vec<ProductMatch>>,
        answer: UpsertOutcome,
    }

    impl FakeMatches {
        fn new(answer: UpsertOutcome) -> Self {
            Self {
                upserted: Mutex::new(Vec::new()),
                answer,
            }
        }
    }

    impl MatchRepo for FakeMatches {
        fn upsert<'a>(
            &'a self,
            m: &'a ProductMatch,
        ) -> PortFuture<'a, Result<UpsertOutcome, RepoError>> {
            self.upserted.lock().unwrap().push(m.clone());
            let answer = self.answer;
            Box::pin(async move { Ok(answer) })
        }

        fn accepted_listings<'a>(
            &'a self,
            _: &'a Asin,
        ) -> PortFuture<'a, Result<Vec<(RetailerId, RetailerSku)>, RepoError>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    #[derive(Default)]
    struct FakePrices(Mutex<Vec<PriceMxn>>);

    impl PriceHistoryRepo for FakePrices {
        fn record<'a>(&'a self, offer: &'a Offer) -> PortFuture<'a, Result<(), RepoError>> {
            self.0.lock().unwrap().push(offer.price_mxn);
            Box::pin(async { Ok(()) })
        }

        fn history<'a>(
            &'a self,
            _: &'a RetailerId,
            _: &'a RetailerSku,
            _: UnixMillis,
        ) -> PortFuture<'a, Result<Vec<PricePoint>, RepoError>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    fn offer(gtin: Option<&str>) -> Offer {
        Offer {
            retailer: RetailerId::new("walmart_mx").unwrap(),
            title: "Licuadora BLSTPYG1210 10 velocidades".into(),
            brand: Some("Oster".into()),
            price_mxn: PriceMxn::from_centavos(129_900).unwrap(),
            list_price_mxn: None,
            availability: Availability::InStock,
            gtin: gtin.map(|g| Gtin::new(g).unwrap()),
            retailer_sku: RetailerSku::new("123").unwrap(),
            url: "https://www.walmart.com.mx/ip/x/123".into(),
            membership_required: false,
            fetched_at: NOW,
        }
    }

    fn item(asin: &str, title: &str) -> CatalogItem {
        CatalogItem {
            asin: Asin::new(asin).unwrap(),
            title: title.into(),
            brand: Some("Oster".into()),
            images: Vec::new(),
            sales_ranks: Vec::new(),
        }
    }

    async fn run(
        amazon: &FakeAmazon,
        o: &Offer,
    ) -> (
        Result<MatchOutcome, OfferMatchError>,
        FakeMatches,
        FakePrices,
    ) {
        run_answering(amazon, o, UpsertOutcome::Written).await
    }

    async fn run_answering(
        amazon: &FakeAmazon,
        o: &Offer,
        answer: UpsertOutcome,
    ) -> (
        Result<MatchOutcome, OfferMatchError>,
        FakeMatches,
        FakePrices,
    ) {
        let matches = FakeMatches::new(answer);
        let prices = FakePrices::default();
        let out = match_and_store(amazon, &matches, &prices, o, &MatchConfig::default(), NOW).await;
        (out, matches, prices)
    }

    fn status(out: &Result<MatchOutcome, OfferMatchError>) -> Option<MatchStatus> {
        match out {
            Ok(MatchOutcome::Stored(m)) => Some(m.status),
            _ => None,
        }
    }

    #[tokio::test]
    async fn gtin_hit_skips_keyword_lookup() {
        let amazon = FakeAmazon::new(Ok(vec![item("B000000001", "Oster blender")]), Ok(vec![]));
        let (out, matches, prices) = run(&amazon, &offer(Some("7501055300075"))).await;
        assert_eq!(status(&out), Some(MatchStatus::Accepted));
        assert_eq!(amazon.calls(), ["gtin:7501055300075"]);
        let stored = matches.upserted.lock().unwrap();
        assert_eq!(stored[0].method, MatchMethod::Gtin);
        assert_eq!(stored[0].asin.as_str(), "B000000001");
        assert_eq!(prices.0.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn empty_gtin_result_falls_back_to_keywords() {
        let amazon = FakeAmazon::new(
            Ok(vec![]),
            Ok(vec![item(
                "B000000002",
                "Oster Licuadora BLSTPYG1210 10 velocidades",
            )]),
        );
        let (out, matches, _) = run(&amazon, &offer(Some("7501055300075"))).await;
        assert_eq!(status(&out), Some(MatchStatus::Accepted));
        assert_eq!(
            amazon.calls(),
            [
                "gtin:7501055300075",
                "kw:Oster Licuadora BLSTPYG1210 10 velocidades"
            ]
        );
        assert_eq!(
            matches.upserted.lock().unwrap()[0].method,
            MatchMethod::Title
        );
    }

    #[tokio::test]
    async fn low_score_is_stored_for_review() {
        let amazon = FakeAmazon::new(
            Ok(vec![]),
            Ok(vec![item(
                "B000000003",
                "Licuadora 10 velocidades roja vaso vidrio",
            )]),
        );
        let (out, matches, _) = run(&amazon, &offer(None)).await;
        assert_eq!(status(&out), Some(MatchStatus::NeedsReview));
        assert_eq!(amazon.calls().len(), 1, "no GTIN, keyword lookup only");
        assert_eq!(
            matches.upserted.lock().unwrap()[0].status,
            MatchStatus::NeedsReview
        );
    }

    #[tokio::test]
    async fn rejected_listing_is_reported_as_kept() {
        let amazon = FakeAmazon::new(Ok(vec![item("B000000001", "Oster blender")]), Ok(vec![]));
        let (out, matches, _) = run_answering(
            &amazon,
            &offer(Some("7501055300075")),
            UpsertOutcome::KeptRejected,
        )
        .await;
        assert_eq!(out, Ok(MatchOutcome::KeptRejected));
        assert_eq!(matches.upserted.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn no_candidate_stores_nothing() {
        let amazon = FakeAmazon::new(Ok(vec![]), Ok(vec![]));
        let (out, matches, prices) = run(&amazon, &offer(None)).await;
        assert_eq!(out, Ok(MatchOutcome::NoCandidate));
        assert!(matches.upserted.lock().unwrap().is_empty());
        assert_eq!(prices.0.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn long_title_keywords_are_bounded() {
        let amazon = FakeAmazon::new(Ok(vec![]), Ok(vec![]));
        let mut o = offer(None);
        // 300 chars of multibyte words: "licuadoraé " is 11 chars.
        o.title = "licuadoraé ".repeat(27) + "xyz";
        assert_eq!(o.title.chars().count(), 300);
        let (out, _, _) = run(&amazon, &o).await;
        assert_eq!(out, Ok(MatchOutcome::NoCandidate));
        let calls = amazon.calls();
        let kw = calls[0].strip_prefix("kw:").unwrap();
        assert!(kw.chars().count() <= amazon::MAX_KEYWORD_CHARS);
        assert!(kw.starts_with("Oster licuadoraé "));
        assert!(kw.ends_with("licuadoraé"), "cut on a word boundary: {kw}");
    }

    #[test]
    fn keyword_query_collapses_whitespace_and_cuts_long_words() {
        assert_eq!(keyword_query(Some(" Oster "), "a \t b\n c"), "Oster a b c");
        assert_eq!(keyword_query(None, "x"), "x");
        let word = "é".repeat(250);
        assert_eq!(
            keyword_query(None, &word).chars().count(),
            amazon::MAX_KEYWORD_CHARS
        );
    }

    #[tokio::test]
    async fn amazon_error_propagates_but_price_is_recorded() {
        let amazon = FakeAmazon::new(Err(AmazonSourceError::RateLimited), Ok(vec![]));
        let (out, matches, prices) = run(&amazon, &offer(Some("7501055300075"))).await;
        assert_eq!(
            out,
            Err(OfferMatchError::Amazon(AmazonSourceError::RateLimited))
        );
        assert_eq!(amazon.calls(), ["gtin:7501055300075"]);
        assert!(matches.upserted.lock().unwrap().is_empty());
        assert_eq!(prices.0.lock().unwrap().len(), 1);
    }
}
