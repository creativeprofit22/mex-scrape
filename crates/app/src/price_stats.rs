//! Use case: Keepa-style low/high/average price stats for an Amazon
//! product, per retailer listing matched to it.

use std::sync::Arc;

use domain::{
    Asin, DAY_MS, Memberships, PriceSample, PriceWindow, RetailerId, RetailerSku, STATS_WINDOWS,
    TenantId, UnixMillis, is_membership_retailer, window_stats,
};

use crate::assistant::tenant_listings;
use crate::ports::{AmazonProductRepo, Clock, MatchDecisionRepo, MatchRepo, PriceHistoryRepo};
use crate::product;

/// Why price stats could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PriceStatsError {
    /// Matches or price history could not be read.
    #[error("price history could not be read")]
    Storage,
    /// The ASIN has no stored name, no match in any status and no listing
    /// the reader sees: the product is unknown.
    #[error("product not found")]
    NotFound,
}

/// Price windows for one retailer listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingPriceStats {
    /// Retailer of the listing.
    pub retailer: RetailerId,
    /// The retailer's product id.
    pub retailer_sku: RetailerSku,
    /// One window per entry of [`STATS_WINDOWS`], shortest first.
    pub windows: Vec<PriceWindow>,
}

/// Price stats for one Amazon product.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductPriceStats {
    /// The Amazon product.
    pub asin: Asin,
    /// Product name on Amazon; `None` until a catalog lookup stored one.
    pub title: Option<String>,
    /// Brand on Amazon, when known.
    pub brand: Option<String>,
    /// The instant every window ends at.
    pub as_of: UnixMillis,
    /// One entry per accepted listing the reader may see, sorted by
    /// retailer then SKU.
    pub listings: Vec<ListingPriceStats>,
    /// Accepted listings removed because they need a membership the reader
    /// lacks.
    pub hidden_listings: u32,
}

/// Builds price stats from injected ports; never constructs adapters.
pub struct PriceStats {
    prices: Arc<dyn PriceHistoryRepo>,
    matches: Arc<dyn MatchRepo>,
    decisions: Arc<dyn MatchDecisionRepo>,
    products: Arc<dyn AmazonProductRepo>,
    clock: Arc<dyn Clock>,
}

impl PriceStats {
    /// Stats reading `prices` for the listings `matches` links to an ASIN,
    /// with windows ending at `clock.now()`.
    #[must_use]
    pub fn new(
        prices: Arc<dyn PriceHistoryRepo>,
        matches: Arc<dyn MatchRepo>,
        decisions: Arc<dyn MatchDecisionRepo>,
        products: Arc<dyn AmazonProductRepo>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            prices,
            matches,
            decisions,
            products,
            clock,
        }
    }

    /// 30/90/365-day stats for every listing with an accepted match to
    /// `asin` as `tenant` sees it: the tenant's own match decisions add or
    /// remove listings (ADR 0013). No listings when there is none. Listings
    /// from a membership
    /// store not in `memberships` are removed and counted in
    /// [`ProductPriceStats::hidden_listings`].
    ///
    /// # Errors
    /// [`PriceStatsError::Storage`] if matches or history cannot be read;
    /// [`PriceStatsError::NotFound`] if the product is unknown.
    pub async fn for_product(
        &self,
        tenant: &TenantId,
        asin: &Asin,
        memberships: Memberships,
    ) -> Result<ProductPriceStats, PriceStatsError> {
        let now = self.clock.now();
        let longest = STATS_WINDOWS.iter().copied().max().unwrap_or(0);
        let since = UnixMillis(now.0.saturating_sub(i64::from(longest) * DAY_MS));
        let listings =
            tenant_listings(self.matches.as_ref(), self.decisions.as_ref(), tenant, asin)
                .await
                .map_err(|_| PriceStatsError::Storage)?;
        let name = product::lookup(
            self.matches.as_ref(),
            self.products.as_ref(),
            asin,
            !listings.is_empty(),
        )
        .await
        .map_err(|_| PriceStatsError::Storage)?
        .ok_or(PriceStatsError::NotFound)?;
        let (listings, hidden_listings) = visible_listings(listings, memberships);
        let mut out = Vec::with_capacity(listings.len());
        for (retailer, retailer_sku) in listings {
            let samples: Vec<PriceSample> = self
                .prices
                .history(&retailer, &retailer_sku, since)
                .await
                .map_err(|_| PriceStatsError::Storage)?
                .into_iter()
                .map(|p| PriceSample {
                    price_mxn: p.price_mxn,
                    availability: p.availability,
                    observed_at: p.observed_at,
                })
                .collect();
            let windows = STATS_WINDOWS
                .iter()
                .map(|days| window_stats(&samples, now, *days))
                .collect();
            out.push(ListingPriceStats {
                retailer,
                retailer_sku,
                windows,
            });
        }
        Ok(ProductPriceStats {
            asin: asin.clone(),
            title: name.title,
            brand: name.brand,
            as_of: now,
            listings: out,
            hidden_listings,
        })
    }
}

/// Sorts `listings` and drops those from a membership store the reader
/// lacks (ADR 0012, as in search results); returns them with the number
/// dropped.
pub(crate) fn visible_listings(
    mut listings: Vec<(RetailerId, RetailerSku)>,
    memberships: Memberships,
) -> (Vec<(RetailerId, RetailerSku)>, u32) {
    listings.sort();
    let before = listings.len();
    listings.retain(|(retailer, _)| memberships.allows(is_membership_retailer(retailer), retailer));
    let hidden = u32::try_from(before - listings.len()).unwrap_or(u32::MAX);
    (listings, hidden)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use domain::{Availability, Offer, PriceMxn, ProductMatch};

    use super::*;
    use crate::assistant::fakes::{FakeDecisions, FakeProducts};

    fn me() -> domain::TenantId {
        domain::TenantId::new("user-7").unwrap()
    }
    use crate::ports::{PortFuture, PricePoint, RepoError, UpsertOutcome};

    const NOW: i64 = 1_760_000_000_000;

    struct FixedClock;

    impl Clock for FixedClock {
        fn now(&self) -> UnixMillis {
            UnixMillis(NOW)
        }
    }

    type Key = (RetailerId, RetailerSku);

    /// History per listing; `history` filters by `since` like the real repo.
    struct FakePrices(Result<BTreeMap<Key, Vec<PricePoint>>, RepoError>);

    impl PriceHistoryRepo for FakePrices {
        fn record<'a>(&'a self, _: &'a Offer) -> PortFuture<'a, Result<(), RepoError>> {
            Box::pin(async { Ok(()) })
        }

        fn history<'a>(
            &'a self,
            retailer: &'a RetailerId,
            sku: &'a RetailerSku,
            since: UnixMillis,
        ) -> PortFuture<'a, Result<Vec<PricePoint>, RepoError>> {
            Box::pin(async move {
                let map = self.0.as_ref().map_err(|e| *e)?;
                Ok(map
                    .get(&(retailer.clone(), sku.clone()))
                    .into_iter()
                    .flatten()
                    .filter(|p| p.observed_at >= since)
                    .cloned()
                    .collect())
            })
        }
    }

    struct FakeMatches(Vec<Key>);

    impl MatchRepo for FakeMatches {
        fn upsert<'a>(
            &'a self,
            _: &'a ProductMatch,
        ) -> PortFuture<'a, Result<UpsertOutcome, RepoError>> {
            Box::pin(async { Ok(UpsertOutcome::Written) })
        }

        fn accepted_listings<'a>(
            &'a self,
            _: &'a Asin,
        ) -> PortFuture<'a, Result<Vec<Key>, RepoError>> {
            Box::pin(async move { Ok(self.0.clone()) })
        }

        fn has_any_match<'a>(&'a self, _: &'a Asin) -> PortFuture<'a, Result<bool, RepoError>> {
            Box::pin(async move { Ok(!self.0.is_empty()) })
        }
    }

    fn key(r: &str, s: &str) -> Key {
        (RetailerId::new(r).unwrap(), RetailerSku::new(s).unwrap())
    }

    fn point(centavos: i64, days_ago: i64, availability: Availability) -> PricePoint {
        PricePoint {
            price_mxn: PriceMxn::from_centavos(centavos).unwrap(),
            list_price_mxn: None,
            availability,
            observed_at: UnixMillis(NOW - days_ago * DAY_MS),
        }
    }

    fn stats(prices: FakePrices, listings: Vec<Key>) -> PriceStats {
        PriceStats::new(
            Arc::new(prices),
            Arc::new(FakeMatches(listings)),
            Arc::new(FakeDecisions::default()),
            Arc::new(FakeProducts::named("Licuadora", Some("Oster"))),
            Arc::new(FixedClock),
        )
    }

    fn summary(w: &PriceWindow) -> (u16, Option<i64>, Option<i64>, Option<i64>, u32) {
        let c = |p: Option<PriceMxn>| p.map(PriceMxn::centavos);
        (w.days, c(w.low), c(w.high), c(w.average), w.observations)
    }

    #[tokio::test]
    async fn computes_windows_per_listing_from_fixture() {
        let walmart = key("walmart_mx", "123");
        let liverpool = key("liverpool", "9");
        let mut history = BTreeMap::new();
        history.insert(
            walmart.clone(),
            vec![
                point(120_000, 400, Availability::InStock), // outside 365
                point(100_000, 200, Availability::InStock),
                point(90_000, 60, Availability::InStock),
                point(10_000, 20, Availability::OutOfStock), // ignored
                point(110_000, 10, Availability::InStock),
                point(105_000, 1, Availability::Unknown),
            ],
        );
        history.insert(liverpool.clone(), vec![]);
        // Unsorted on purpose: the use case sorts.
        let s = stats(FakePrices(Ok(history)), vec![walmart, liverpool]);
        let asin = Asin::new("B0CHHSFMRL").unwrap();
        let out = s.for_product(&me(), &asin, NONE).await.unwrap();

        assert_eq!(out.asin, asin);
        assert_eq!(out.as_of, UnixMillis(NOW));
        assert_eq!(out.hidden_listings, 0);
        let names: Vec<&str> = out.listings.iter().map(|l| l.retailer.as_str()).collect();
        assert_eq!(names, ["liverpool", "walmart_mx"]);

        let empty: Vec<_> = out.listings[0].windows.iter().map(summary).collect();
        assert_eq!(
            empty,
            [
                (30, None, None, None, 0),
                (90, None, None, None, 0),
                (365, None, None, None, 0)
            ]
        );
        let w: Vec<_> = out.listings[1].windows.iter().map(summary).collect();
        assert_eq!(
            w,
            [
                (30, Some(105_000), Some(110_000), Some(107_500), 2),
                (90, Some(90_000), Some(110_000), Some(101_667), 3),
                (365, Some(90_000), Some(110_000), Some(101_250), 4),
            ]
        );
    }

    const NONE: Memberships = Memberships {
        costco: false,
        sams: false,
    };

    #[tokio::test]
    async fn membership_listings_are_hidden_unless_the_reader_has_it() {
        let listings = vec![
            key("sams_mx", "2"),
            key("costco_mx", "1"),
            key("walmart_mx", "3"),
        ];
        let s = stats(FakePrices(Ok(BTreeMap::new())), listings);
        let asin = Asin::new("B0CHHSFMRL").unwrap();
        let shown = |out: ProductPriceStats| {
            let names: Vec<String> = out
                .listings
                .iter()
                .map(|l| l.retailer.as_str().to_owned())
                .collect();
            (names, out.hidden_listings)
        };
        let none = s.for_product(&me(), &asin, NONE).await.unwrap();
        assert_eq!(shown(none), (vec!["walmart_mx".to_owned()], 2));
        let costco = Memberships {
            costco: true,
            sams: false,
        };
        let out = s.for_product(&me(), &asin, costco).await.unwrap();
        assert_eq!(
            shown(out),
            (vec!["costco_mx".to_owned(), "walmart_mx".to_owned()], 1)
        );
    }

    #[tokio::test]
    async fn no_accepted_listing_gives_no_rows() {
        let s = stats(FakePrices(Ok(BTreeMap::new())), vec![]);
        let out = s
            .for_product(&me(), &Asin::new("B0CHHSFMRL").unwrap(), NONE)
            .await
            .unwrap();
        assert!(out.listings.is_empty());
    }

    #[tokio::test]
    async fn storage_failure_is_reported() {
        let s = stats(
            FakePrices(Err(RepoError::Unavailable)),
            vec![key("walmart_mx", "1")],
        );
        assert_eq!(
            s.for_product(&me(), &Asin::new("B0CHHSFMRL").unwrap(), NONE)
                .await,
            Err(PriceStatsError::Storage)
        );
    }

    #[tokio::test]
    async fn the_tenants_decisions_change_its_listings_only() {
        let asin = Asin::new("B0CHHSFMRL").unwrap();
        let decide = |sku: &str, decision| crate::ports::StoredDecision {
            retailer: RetailerId::new("walmart_mx").unwrap(),
            retailer_sku: RetailerSku::new(sku).unwrap(),
            asin: asin.clone(),
            decision,
        };
        let decisions = FakeDecisions::with(
            &me(),
            vec![
                decide("1", domain::MatchDecision::Reject),
                decide("2", domain::MatchDecision::Accept),
            ],
        );
        let s = PriceStats::new(
            Arc::new(FakePrices(Ok(BTreeMap::new()))),
            Arc::new(FakeMatches(vec![
                key("walmart_mx", "1"),
                key("liverpool", "9"),
            ])),
            Arc::new(decisions),
            Arc::new(FakeProducts::default()),
            Arc::new(FixedClock),
        );
        let skus = |out: ProductPriceStats| {
            out.listings
                .iter()
                .map(|l| format!("{}/{}", l.retailer.as_str(), l.retailer_sku.as_str()))
                .collect::<Vec<_>>()
        };
        let mine = s.for_product(&me(), &asin, NONE).await.unwrap();
        assert_eq!(skus(mine), ["liverpool/9", "walmart_mx/2"]);
        let other = domain::TenantId::new("user-8").unwrap();
        let theirs = s.for_product(&other, &asin, NONE).await.unwrap();
        assert_eq!(skus(theirs), ["liverpool/9", "walmart_mx/1"]);
    }

    #[tokio::test]
    async fn names_known_products_and_refuses_unknown_ones() {
        let asin = Asin::new("B000000001").unwrap();
        let named = stats(FakePrices(Ok(BTreeMap::new())), vec![key("liverpool", "9")])
            .for_product(&me(), &asin, NONE)
            .await
            .unwrap();
        assert_eq!(named.title.as_deref(), Some("Licuadora"));
        assert_eq!(named.brand.as_deref(), Some("Oster"));

        let build = |listings| {
            PriceStats::new(
                Arc::new(FakePrices(Ok(BTreeMap::new()))),
                Arc::new(FakeMatches(listings)),
                Arc::new(FakeDecisions::default()),
                Arc::new(FakeProducts::default()),
                Arc::new(FixedClock),
            )
        };
        let unnamed = build(vec![key("liverpool", "9")])
            .for_product(&me(), &asin, NONE)
            .await
            .unwrap();
        assert_eq!((unnamed.title, unnamed.brand), (None, None));
        assert_eq!(unnamed.listings.len(), 1);
        assert_eq!(
            build(vec![]).for_product(&me(), &asin, NONE).await,
            Err(PriceStatsError::NotFound)
        );
    }
}
