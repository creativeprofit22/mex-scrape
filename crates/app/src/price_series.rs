//! Use case: a daily price series per retailer listing matched to an Amazon
//! product, for the product page chart.

use std::collections::BTreeMap;
use std::sync::Arc;

use domain::{
    Asin, Availability, DAY_MS, Memberships, PriceMxn, RetailerId, RetailerSku, TenantId,
    UnixMillis,
};

use crate::assistant::tenant_listings;
use crate::ports::{Clock, MatchDecisionRepo, MatchRepo, PriceHistoryRepo, PricePoint};
use crate::price_stats::visible_listings;

/// Longest series served, in days.
pub const MAX_SERIES_DAYS: u16 = 365;

/// Why a series could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PriceSeriesError {
    /// `days` is 0 or above [`MAX_SERIES_DAYS`].
    #[error("invalid number of days")]
    InvalidDays,
    /// Matches or price history could not be read.
    #[error("price history could not be read")]
    Storage,
}

/// Prices of one listing on one UTC day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DailyPrice {
    /// Start of the UTC day.
    pub day: UnixMillis,
    /// Lowest price seen that day.
    pub low_mxn: PriceMxn,
    /// Highest price seen that day.
    pub high_mxn: PriceMxn,
    /// Whether any observation that day was in stock.
    pub in_stock: bool,
}

/// The series of one retailer listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingPriceSeries {
    /// Retailer of the listing.
    pub retailer: RetailerId,
    /// The retailer's product id.
    pub retailer_sku: RetailerSku,
    /// One point per day with observations, oldest first.
    pub points: Vec<DailyPrice>,
}

/// Daily series for one Amazon product.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductPriceSeries {
    /// The Amazon product.
    pub asin: Asin,
    /// First instant covered.
    pub since: UnixMillis,
    /// When the series was built.
    pub as_of: UnixMillis,
    /// One entry per accepted listing the reader may see, sorted by
    /// retailer then SKU.
    pub listings: Vec<ListingPriceSeries>,
    /// Accepted listings removed because they need a membership the reader
    /// lacks.
    pub hidden_listings: u32,
}

/// Builds price series from injected ports.
pub struct PriceSeries {
    prices: Arc<dyn PriceHistoryRepo>,
    matches: Arc<dyn MatchRepo>,
    decisions: Arc<dyn MatchDecisionRepo>,
    clock: Arc<dyn Clock>,
}

impl PriceSeries {
    /// Series reading `prices` for the listings `matches` links to an ASIN,
    /// ending at `clock.now()`.
    #[must_use]
    pub fn new(
        prices: Arc<dyn PriceHistoryRepo>,
        matches: Arc<dyn MatchRepo>,
        decisions: Arc<dyn MatchDecisionRepo>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            prices,
            matches,
            decisions,
            clock,
        }
    }

    /// Daily low/high of the last `days` days for every listing with an
    /// accepted match to `asin`, as `tenant` sees it (its own match
    /// decisions add or remove listings, ADR 0013). Listings from a
    /// membership store not in
    /// `memberships` are removed and counted in
    /// [`ProductPriceSeries::hidden_listings`].
    ///
    /// # Errors
    /// [`PriceSeriesError::InvalidDays`] unless `1 <= days <= 365`;
    /// [`PriceSeriesError::Storage`] if storage cannot be read.
    pub async fn for_product(
        &self,
        tenant: &TenantId,
        asin: &Asin,
        days: u16,
        memberships: Memberships,
    ) -> Result<ProductPriceSeries, PriceSeriesError> {
        if days == 0 || days > MAX_SERIES_DAYS {
            return Err(PriceSeriesError::InvalidDays);
        }
        let now = self.clock.now();
        let since = UnixMillis(now.0.saturating_sub(i64::from(days) * DAY_MS));
        let listings =
            tenant_listings(self.matches.as_ref(), self.decisions.as_ref(), tenant, asin)
                .await
                .map_err(|_| PriceSeriesError::Storage)?;
        let (listings, hidden_listings) = visible_listings(listings, memberships);
        let mut out = Vec::with_capacity(listings.len());
        for (retailer, retailer_sku) in listings {
            let history = self
                .prices
                .history(&retailer, &retailer_sku, since)
                .await
                .map_err(|_| PriceSeriesError::Storage)?;
            out.push(ListingPriceSeries {
                retailer,
                retailer_sku,
                points: daily(&history, now),
            });
        }
        Ok(ProductPriceSeries {
            asin: asin.clone(),
            since,
            as_of: now,
            listings: out,
            hidden_listings,
        })
    }
}

/// Buckets observations by UTC day (ignoring any after `now`).
fn daily(history: &[PricePoint], now: UnixMillis) -> Vec<DailyPrice> {
    let mut days: BTreeMap<i64, DailyPrice> = BTreeMap::new();
    for p in history.iter().filter(|p| p.observed_at <= now) {
        let day = p.observed_at.0.div_euclid(DAY_MS) * DAY_MS;
        let in_stock = p.availability == Availability::InStock;
        days.entry(day)
            .and_modify(|d| {
                d.low_mxn = d.low_mxn.min(p.price_mxn);
                d.high_mxn = d.high_mxn.max(p.price_mxn);
                d.in_stock |= in_stock;
            })
            .or_insert(DailyPrice {
                day: UnixMillis(day),
                low_mxn: p.price_mxn,
                high_mxn: p.price_mxn,
                in_stock,
            });
    }
    days.into_values().collect()
}

#[cfg(test)]
mod tests {
    use domain::{Offer, ProductMatch};

    use super::*;
    use crate::assistant::fakes::FakeDecisions;

    fn me() -> domain::TenantId {
        domain::TenantId::new("user-7").unwrap()
    }
    use crate::ports::{PortFuture, RepoError, UpsertOutcome};

    // A UTC midnight.
    const DAY0: i64 = 20_000 * DAY_MS;
    const NOW: i64 = DAY0 + 3 * DAY_MS + 1_000;

    struct FixedClock;

    impl Clock for FixedClock {
        fn now(&self) -> UnixMillis {
            UnixMillis(NOW)
        }
    }

    struct FakePrices(Result<Vec<PricePoint>, RepoError>);

    impl PriceHistoryRepo for FakePrices {
        fn record<'a>(&'a self, _: &'a Offer) -> PortFuture<'a, Result<(), RepoError>> {
            Box::pin(async { Ok(()) })
        }

        fn history<'a>(
            &'a self,
            _: &'a RetailerId,
            _: &'a RetailerSku,
            since: UnixMillis,
        ) -> PortFuture<'a, Result<Vec<PricePoint>, RepoError>> {
            Box::pin(async move {
                let v = self.0.as_ref().map_err(|e| *e)?;
                Ok(v.iter()
                    .filter(|p| p.observed_at >= since)
                    .cloned()
                    .collect())
            })
        }
    }

    struct FakeMatches(Vec<(RetailerId, RetailerSku)>);

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
        ) -> PortFuture<'a, Result<Vec<(RetailerId, RetailerSku)>, RepoError>> {
            Box::pin(async move { Ok(self.0.clone()) })
        }
    }

    fn price(c: i64) -> PriceMxn {
        PriceMxn::from_centavos(c).unwrap()
    }

    fn point(c: i64, at: i64, a: Availability) -> PricePoint {
        PricePoint {
            price_mxn: price(c),
            list_price_mxn: None,
            availability: a,
            observed_at: UnixMillis(at),
        }
    }

    const NONE: Memberships = Memberships {
        costco: false,
        sams: false,
    };

    fn series(prices: FakePrices, listings: usize) -> PriceSeries {
        let keys = (0..listings)
            .rev()
            .map(|i| {
                (
                    RetailerId::new(&format!("r{i}")).unwrap(),
                    RetailerSku::new("sku").unwrap(),
                )
            })
            .collect();
        series_of(prices, keys)
    }

    fn series_of(prices: FakePrices, keys: Vec<(RetailerId, RetailerSku)>) -> PriceSeries {
        PriceSeries::new(
            Arc::new(prices),
            Arc::new(FakeMatches(keys)),
            Arc::new(FakeDecisions::default()),
            Arc::new(FixedClock),
        )
    }

    #[tokio::test]
    async fn buckets_by_utc_day_with_low_high_and_stock() {
        use Availability::{InStock, OutOfStock};
        let history = vec![
            point(1_000, DAY0 + 10, OutOfStock),
            point(900, DAY0 + DAY_MS - 1, InStock),
            point(1_200, DAY0 + DAY_MS, OutOfStock),
            point(5, NOW + 1, InStock), // future: ignored
        ];
        let asin = Asin::new("B000000001").unwrap();
        let s = series(FakePrices(Ok(history)), 2)
            .for_product(&me(), &asin, 30, NONE)
            .await
            .unwrap();
        assert_eq!(s.since, UnixMillis(NOW - 30 * DAY_MS));
        assert_eq!(s.listings[0].retailer.as_str(), "r0");
        let pts = &s.listings[0].points;
        assert_eq!(
            pts,
            &[
                DailyPrice {
                    day: UnixMillis(DAY0),
                    low_mxn: price(900),
                    high_mxn: price(1_000),
                    in_stock: true,
                },
                DailyPrice {
                    day: UnixMillis(DAY0 + DAY_MS),
                    low_mxn: price(1_200),
                    high_mxn: price(1_200),
                    in_stock: false,
                },
            ]
        );
    }

    #[tokio::test]
    async fn membership_listings_are_hidden_unless_the_reader_has_it() {
        let keys = ["sams_mx", "costco_mx", "liverpool"]
            .iter()
            .map(|r| {
                (
                    RetailerId::new(r).unwrap(),
                    RetailerSku::new("sku").unwrap(),
                )
            })
            .collect();
        let s = series_of(FakePrices(Ok(Vec::new())), keys);
        let asin = Asin::new("B000000001").unwrap();
        let shown = |out: ProductPriceSeries| {
            let names: Vec<String> = out
                .listings
                .iter()
                .map(|l| l.retailer.as_str().to_owned())
                .collect();
            (names, out.hidden_listings)
        };
        let none = s.for_product(&me(), &asin, 30, NONE).await.unwrap();
        assert_eq!(shown(none), (vec!["liverpool".to_owned()], 2));
        let costco = Memberships {
            costco: true,
            sams: false,
        };
        let out = s.for_product(&me(), &asin, 30, costco).await.unwrap();
        assert_eq!(
            shown(out),
            (vec!["costco_mx".to_owned(), "liverpool".to_owned()], 1)
        );
    }

    #[tokio::test]
    async fn validates_days_and_reports_storage_errors() {
        let asin = Asin::new("B000000001").unwrap();
        let ok = series(FakePrices(Ok(Vec::new())), 1);
        assert_eq!(
            ok.for_product(&me(), &asin, 0, NONE).await,
            Err(PriceSeriesError::InvalidDays)
        );
        assert_eq!(
            ok.for_product(&me(), &asin, 366, NONE).await,
            Err(PriceSeriesError::InvalidDays)
        );
        let bad = series(FakePrices(Err(RepoError::Unavailable)), 1);
        assert_eq!(
            bad.for_product(&me(), &asin, 365, NONE).await,
            Err(PriceSeriesError::Storage)
        );
    }
}
