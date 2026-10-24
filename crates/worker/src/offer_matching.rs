//! Matches a fetched retailer offer to an Amazon product and stores the
//! result: records the price observation, looks the offer up on Amazon
//! (GTIN first, keywords only when the GTIN finds nothing), scores the
//! candidates with `matching`, stores the matched product's Amazon name
//! (ADR 0017) and upserts the match.

use app::{
    AmazonProduct, AmazonProductRepo, AmazonSource, AmazonSourceError, CatalogItem, CatalogResult,
    MatchRepo, PriceHistoryRepo, UpsertOutcome,
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
    /// The observation, the product name or the match could not be
    /// stored.
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
pub(crate) fn keyword_query(brand: Option<&str>, title: &str) -> String {
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

fn candidates(result: Option<&CatalogResult>) -> Vec<Candidate> {
    result
        .into_iter()
        .flat_map(|r| &r.items)
        .map(|i: &CatalogItem| Candidate {
            asin: i.asin.clone(),
            title: i.title.clone(),
            brand: i.brand.clone(),
        })
        .collect()
}

/// The Amazon name of the matched product `found`, from the lookup that
/// returned it (GTIN hits first); `None` when no item for that ASIN carries
/// a usable title.
pub(crate) fn matched_product(
    found: &ProductMatch,
    gtin: Option<&CatalogResult>,
    keyword: Option<&CatalogResult>,
) -> Option<AmazonProduct> {
    gtin.into_iter().chain(keyword).find_map(|result| {
        result
            .items
            .iter()
            .filter(|item| item.asin == found.asin)
            .find_map(|item| AmazonProduct::from_catalog(item, result.source, result.fetched_at))
    })
}

/// Records `offer`'s price, then matches and stores it. The observation is
/// recorded even when the Amazon lookup fails. When a match is found, the
/// matched product's Amazon name is stored first, so every match written
/// has a name (the newest lookup wins).
///
/// # Errors
/// [`OfferMatchError::Storage`] if a write fails;
/// [`OfferMatchError::Amazon`] if a lookup fails (no match is stored).
pub async fn match_and_store(
    amazon: &dyn AmazonSource,
    matches: &dyn MatchRepo,
    products: &dyn AmazonProductRepo,
    prices: &dyn PriceHistoryRepo,
    offer: &Offer,
    config: &MatchConfig,
    now: UnixMillis,
) -> Result<MatchOutcome, OfferMatchError> {
    prices
        .record(offer)
        .await
        .map_err(|_| OfferMatchError::Storage)?;

    let gtin_result = match &offer.gtin {
        Some(gtin) => Some(
            amazon
                .catalog_by_gtin(gtin)
                .await
                .map_err(OfferMatchError::Amazon)?,
        ),
        None => None,
    };
    let gtin_hits = candidates(gtin_result.as_ref());
    let keyword_result = if gtin_hits.is_empty() {
        let keywords = keyword_query(offer.brand.as_deref(), &offer.title);
        Some(
            amazon
                .catalog_by_keyword(&keywords)
                .await
                .map_err(OfferMatchError::Amazon)?,
        )
    } else {
        None
    };
    let keyword_hits = candidates(keyword_result.as_ref());

    let Some(found) = match_offer(offer, &gtin_hits, &keyword_hits, config, now) else {
        return Ok(MatchOutcome::NoCandidate);
    };
    if let Some(product) = matched_product(&found, gtin_result.as_ref(), keyword_result.as_ref()) {
        products
            .upsert(&product)
            .await
            .map_err(|_| OfferMatchError::Storage)?;
    }
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
        AmazonDataSource, FeeEstimate, Fulfillment, OfferSummary, PortFuture, PricePoint, RepoError,
    };
    use domain::{
        Asin, Availability, Confidence, Gtin, MatchMethod, MatchStatus, PriceMxn, RetailerId,
        RetailerSku,
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

        fn has_any_match<'a>(&'a self, _: &'a Asin) -> PortFuture<'a, Result<bool, RepoError>> {
            Box::pin(async { Ok(false) })
        }
    }

    /// Logs stored product names; fails every write when `fail`.
    #[derive(Default)]
    struct FakeProducts {
        stored: Mutex<Vec<AmazonProduct>>,
        fail: bool,
    }

    impl AmazonProductRepo for FakeProducts {
        fn upsert<'a>(&'a self, p: &'a AmazonProduct) -> PortFuture<'a, Result<(), RepoError>> {
            if self.fail {
                return Box::pin(async { Err(RepoError::Unavailable) });
            }
            self.stored.lock().unwrap().push(p.clone());
            Box::pin(async { Ok(()) })
        }

        fn find<'a>(
            &'a self,
            _: &'a Asin,
        ) -> PortFuture<'a, Result<Option<AmazonProduct>, RepoError>> {
            Box::pin(async { Ok(None) })
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
        let (out, matches, _, prices) = run_with(amazon, o, answer, FakeProducts::default()).await;
        (out, matches, prices)
    }

    async fn run_with(
        amazon: &FakeAmazon,
        o: &Offer,
        answer: UpsertOutcome,
        products: FakeProducts,
    ) -> (
        Result<MatchOutcome, OfferMatchError>,
        FakeMatches,
        FakeProducts,
        FakePrices,
    ) {
        let matches = FakeMatches::new(answer);
        let prices = FakePrices::default();
        let out = match_and_store(
            amazon,
            &matches,
            &products,
            &prices,
            o,
            &MatchConfig::default(),
            NOW,
        )
        .await;
        (out, matches, products, prices)
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

    #[tokio::test]
    async fn matched_product_name_is_stored_with_brand() {
        let amazon = FakeAmazon::new(
            Ok(vec![item("B000000001", "  Oster   blender ")]),
            Ok(vec![]),
        );
        let (out, matches, products, _) = run_with(
            &amazon,
            &offer(Some("7501055300075")),
            UpsertOutcome::Written,
            FakeProducts::default(),
        )
        .await;
        assert_eq!(status(&out), Some(MatchStatus::Accepted));
        let stored = products.stored.lock().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].asin.as_str(), "B000000001");
        assert_eq!(stored[0].title, "Oster blender");
        assert_eq!(stored[0].brand.as_deref(), Some("Oster"));
        assert_eq!(stored[0].source, AmazonDataSource::SpApi);
        assert_eq!(stored[0].seen_at, NOW);
        assert_eq!(matches.upserted.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn no_candidate_stores_no_name() {
        let amazon = FakeAmazon::new(Ok(vec![]), Ok(vec![]));
        let (out, _, products, _) = run_with(
            &amazon,
            &offer(None),
            UpsertOutcome::Written,
            FakeProducts::default(),
        )
        .await;
        assert_eq!(out, Ok(MatchOutcome::NoCandidate));
        assert!(products.stored.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn name_storage_error_stops_before_the_match() {
        let amazon = FakeAmazon::new(Ok(vec![item("B000000001", "Oster blender")]), Ok(vec![]));
        let failing = FakeProducts {
            fail: true,
            ..FakeProducts::default()
        };
        let (out, matches, _, _) = run_with(
            &amazon,
            &offer(Some("7501055300075")),
            UpsertOutcome::Written,
            failing,
        )
        .await;
        assert_eq!(out, Err(OfferMatchError::Storage));
        assert!(matches.upserted.lock().unwrap().is_empty());
    }

    #[test]
    fn matched_product_prefers_gtin_hits_and_skips_blank_titles() {
        let found = |asin: &str| ProductMatch {
            retailer: RetailerId::new("walmart_mx").unwrap(),
            retailer_sku: RetailerSku::new("123").unwrap(),
            asin: Asin::new(asin).unwrap(),
            method: MatchMethod::Gtin,
            confidence: Confidence::MAX,
            status: MatchStatus::Accepted,
            matched_at: NOW,
        };
        let result = |items, source| CatalogResult {
            items,
            source,
            fetched_at: NOW,
        };
        let gtin = result(
            vec![item("B000000001", "Por GTIN")],
            AmazonDataSource::SpApi,
        );
        let kw = result(
            vec![item("B000000001", "Por palabra"), item("B000000002", " ")],
            AmazonDataSource::BrightData,
        );
        let p = matched_product(&found("B000000001"), Some(&gtin), Some(&kw)).unwrap();
        assert_eq!(p.title, "Por GTIN");
        let p = matched_product(&found("B000000001"), None, Some(&kw)).unwrap();
        assert_eq!(
            (p.title.as_str(), p.source),
            ("Por palabra", AmazonDataSource::BrightData)
        );
        assert!(matched_product(&found("B000000002"), None, Some(&kw)).is_none());
        assert!(matched_product(&found("B000000003"), Some(&gtin), Some(&kw)).is_none());
    }
}
