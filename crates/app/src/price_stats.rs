//! Use case: Keepa-style low/high/average price stats for an Amazon
//! product, per retailer listing matched to it.

use std::sync::Arc;

use domain::{
    Asin, DAY_MS, PriceSample, PriceWindow, RetailerId, RetailerSku, STATS_WINDOWS, UnixMillis,
    window_stats,
};

use crate::ports::{Clock, MatchRepo, PriceHistoryRepo};

/// Why price stats could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PriceStatsError {
    /// Matches or price history could not be read.
    #[error("price history could not be read")]
    Storage,
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
    /// The instant every window ends at.
    pub as_of: UnixMillis,
    /// One entry per accepted listing, sorted by retailer then SKU.
    pub listings: Vec<ListingPriceStats>,
}

/// Builds price stats from injected ports; never constructs adapters.
pub struct PriceStats {
    prices: Arc<dyn PriceHistoryRepo>,
    matches: Arc<dyn MatchRepo>,
    clock: Arc<dyn Clock>,
}

impl PriceStats {
    /// Stats reading `prices` for the listings `matches` links to an ASIN,
    /// with windows ending at `clock.now()`.
    #[must_use]
    pub fn new(
        prices: Arc<dyn PriceHistoryRepo>,
        matches: Arc<dyn MatchRepo>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            prices,
            matches,
            clock,
        }
    }

    /// 30/90/365-day stats for every listing with an accepted match to
    /// `asin` (no listings when there is none).
    ///
    /// # Errors
    /// [`PriceStatsError::Storage`] if matches or history cannot be read.
    pub async fn for_product(&self, asin: &Asin) -> Result<ProductPriceStats, PriceStatsError> {
        let now = self.clock.now();
        let longest = STATS_WINDOWS.iter().copied().max().unwrap_or(0);
        let since = UnixMillis(now.0.saturating_sub(i64::from(longest) * DAY_MS));
        let mut listings = self
            .matches
            .accepted_listings(asin)
            .await
            .map_err(|_| PriceStatsError::Storage)?;
        listings.sort();
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
            as_of: now,
            listings: out,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use domain::{Availability, Offer, PriceMxn, ProductMatch};

    use super::*;
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
        let out = s.for_product(&asin).await.unwrap();

        assert_eq!(out.asin, asin);
        assert_eq!(out.as_of, UnixMillis(NOW));
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

    #[tokio::test]
    async fn no_accepted_listing_gives_no_rows() {
        let s = stats(FakePrices(Ok(BTreeMap::new())), vec![]);
        let out = s
            .for_product(&Asin::new("B0CHHSFMRL").unwrap())
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
            s.for_product(&Asin::new("B0CHHSFMRL").unwrap()).await,
            Err(PriceStatsError::Storage)
        );
    }
}
