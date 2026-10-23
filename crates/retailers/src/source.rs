//! [`app::OfferSource`] over the store adapters: builds the URL, fetches it
//! through an injected [`PageFetcher`], and parses the page through
//! [`crate::health::parse_page`], so every parse feeds the caller's
//! [`ParseHealth`] and a drop below the threshold logs an alert.
//!
//! This crate still does no I/O: the caller injects the fetcher. Robots.txt
//! checks and spend caps are the fetcher's job; the worker will inject one
//! backed by `brightdata::Runner`, so no Bright Data call bypasses it.

use std::sync::{Arc, Mutex, PoisonError};

use app::{Clock, OfferSource, OfferSourceError, PortFuture};
use brightdata::TargetUrl;
use domain::{Offer, RetailerId, RetailerSku};

use crate::adapter::{AdapterError, Page, ParseError, ParseKind, Retailer};
use crate::health::{ParseHealth, parse_page};
use crate::registry::{AccessMode, PageKind};

/// Why a page fetch failed. No detail: fetch errors may echo URLs or
/// credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    /// The provider refused the fetch until KYC is completed.
    #[error("fetch blocked until KYC is completed")]
    BlockedNeedsKyc,
    /// Any other failure (network, robots.txt, caps, provider error).
    #[error("page fetch failed")]
    Failed,
}

/// Fetches one page's HTML body.
pub trait PageFetcher: Send + Sync {
    /// The body of `url`, fetched with `mode`.
    fn fetch<'a>(
        &'a self,
        url: &'a TargetUrl,
        mode: AccessMode,
    ) -> PortFuture<'a, Result<String, FetchError>>;
}

/// The store adapters as an [`OfferSource`].
pub struct RetailerOfferSource {
    fetch: Option<Fetch>,
}

struct Fetch {
    fetcher: Arc<dyn PageFetcher>,
    clock: Arc<dyn Clock>,
    health: Arc<Mutex<ParseHealth>>,
}

impl RetailerOfferSource {
    /// A source fetching pages with `fetcher`, stamping them with `clock` and
    /// recording every parse in `health` (owned by the caller).
    #[must_use]
    pub fn new(
        fetcher: Arc<dyn PageFetcher>,
        clock: Arc<dyn Clock>,
        health: Arc<Mutex<ParseHealth>>,
    ) -> Self {
        Self {
            fetch: Some(Fetch {
                fetcher,
                clock,
                health,
            }),
        }
    }

    /// Registry lookups only: `search` and `product` fail closed with
    /// [`OfferSourceError::Unavailable`] after validating their input.
    #[must_use]
    pub const fn lookup_only() -> Self {
        Self { fetch: None }
    }

    async fn page(
        &self,
        adapter: &dyn Retailer,
        url: TargetUrl,
        kind: PageKind,
    ) -> Result<Page, OfferSourceError> {
        let Some(Fetch { fetcher, clock, .. }) = &self.fetch else {
            return Err(OfferSourceError::Unavailable);
        };
        let body = fetcher
            .fetch(&url, adapter.info().access_for(kind))
            .await
            .map_err(|error| match error {
                FetchError::BlockedNeedsKyc => OfferSourceError::BlockedNeedsKyc,
                FetchError::Failed => OfferSourceError::FetchFailed,
            })?;
        Ok(Page {
            url,
            body,
            fetched_at: clock.now(),
        })
    }

    /// Parses a fetched `page` and records the result in the parse health.
    /// Synchronous, so the lock is never held across an `.await`.
    fn parse(
        &self,
        adapter: &dyn Retailer,
        kind: ParseKind,
        page: &Page,
    ) -> Result<Vec<Offer>, OfferSourceError> {
        let Some(Fetch { health, .. }) = &self.fetch else {
            return Err(OfferSourceError::Unavailable);
        };
        // A panic elsewhere mid-`record` leaves only a counter update
        // half-done; keep tracking rather than silencing the alerts.
        let mut health = health.lock().unwrap_or_else(PoisonError::into_inner);
        match parse_page(adapter, kind, page, &mut health) {
            Ok(offers) => Ok(offers),
            // The store says it has no results: an empty answer.
            Err(ParseError::NoResults) => Ok(Vec::new()),
            Err(_) => Err(OfferSourceError::ParseFailed),
        }
    }
}

fn adapter(retailer: &RetailerId) -> Result<&'static dyn Retailer, OfferSourceError> {
    crate::adapter_for(retailer.as_str()).ok_or(OfferSourceError::UnknownRetailer)
}

/// Maps a URL-building error; `invalid` is the input error for this call.
fn url_error(error: AdapterError, invalid: OfferSourceError) -> OfferSourceError {
    match error {
        AdapterError::EmptyQuery | AdapterError::QueryTooLong => OfferSourceError::InvalidQuery,
        AdapterError::InvalidSku => OfferSourceError::InvalidSku,
        AdapterError::SearchUnavailable => OfferSourceError::SearchUnavailable,
        AdapterError::InvalidUrl => invalid,
    }
}

impl OfferSource for RetailerOfferSource {
    fn retailer_for_host(&self, host: &str) -> Option<RetailerId> {
        crate::retailer_for_host(host).and_then(|info| RetailerId::new(info.id).ok())
    }

    fn search<'a>(
        &'a self,
        retailer: &'a RetailerId,
        query: &'a str,
    ) -> PortFuture<'a, Result<Vec<Offer>, OfferSourceError>> {
        Box::pin(async move {
            let adapter = adapter(retailer)?;
            let url = adapter
                .search_url(query)
                .map_err(|e| url_error(e, OfferSourceError::InvalidQuery))?;
            let page = self.page(adapter, url, PageKind::Search).await?;
            self.parse(adapter, ParseKind::Search, &page)
        })
    }

    fn product<'a>(
        &'a self,
        retailer: &'a RetailerId,
        sku: &'a RetailerSku,
    ) -> PortFuture<'a, Result<Offer, OfferSourceError>> {
        Box::pin(async move {
            let adapter = adapter(retailer)?;
            let url = adapter
                .product_url(sku)
                .map_err(|e| url_error(e, OfferSourceError::InvalidSku))?;
            let page = self.page(adapter, url, PageKind::Product).await?;
            self.parse(adapter, ParseKind::Product, &page)?
                .into_iter()
                .next()
                .ok_or(OfferSourceError::ParseFailed)
        })
    }
}

#[cfg(test)]
mod tests {
    use domain::UnixMillis;

    use super::*;
    use crate::health::{HealthConfig, HealthStatus};

    const SEARCH: &str = include_str!("../tests/fixtures/liverpool/search.html");
    const PRODUCT: &str = include_str!("../tests/fixtures/liverpool/product.html");
    const NOW: UnixMillis = UnixMillis(1_760_000_000_000);

    struct FixedClock;

    impl Clock for FixedClock {
        fn now(&self) -> UnixMillis {
            NOW
        }
    }

    /// Answers with `result` and records each requested URL and mode.
    struct FakeFetcher {
        result: Result<&'static str, FetchError>,
        calls: Mutex<Vec<(String, AccessMode)>>,
    }

    impl FakeFetcher {
        fn new(result: Result<&'static str, FetchError>) -> Arc<Self> {
            Arc::new(Self {
                result,
                calls: Mutex::new(Vec::new()),
            })
        }
    }

    impl PageFetcher for FakeFetcher {
        fn fetch<'a>(
            &'a self,
            url: &'a TargetUrl,
            mode: AccessMode,
        ) -> PortFuture<'a, Result<String, FetchError>> {
            self.calls
                .lock()
                .unwrap()
                .push((url.as_str().to_owned(), mode));
            Box::pin(async move { self.result.map(str::to_owned) })
        }
    }

    fn health(min_samples: usize) -> Arc<Mutex<ParseHealth>> {
        Arc::new(Mutex::new(ParseHealth::new(HealthConfig {
            threshold: 0.8,
            min_samples,
            window: 10,
        })))
    }

    fn source(fetcher: &Arc<FakeFetcher>) -> RetailerOfferSource {
        RetailerOfferSource::new(fetcher.clone(), Arc::new(FixedClock), health(5))
    }

    fn liverpool() -> RetailerId {
        RetailerId::new("liverpool").unwrap()
    }

    fn direct(url: &TargetUrl, body: &str) -> Page {
        Page {
            url: url.clone(),
            body: body.to_owned(),
            fetched_at: NOW,
        }
    }

    #[tokio::test]
    async fn search_fetches_the_search_url_and_matches_the_direct_parse() {
        let fetcher = FakeFetcher::new(Ok(SEARCH));
        let offers = source(&fetcher)
            .search(&liverpool(), " licuadora ")
            .await
            .unwrap();
        let adapter = crate::adapter_for("liverpool").unwrap();
        let url = adapter.search_url("licuadora").unwrap();
        assert_eq!(offers, adapter.parse_search(&direct(&url, SEARCH)).unwrap());
        assert!(offers.iter().all(|o| o.fetched_at == NOW));
        assert_eq!(
            *fetcher.calls.lock().unwrap(),
            [(url.as_str().to_owned(), AccessMode::Unlocker)]
        );
    }

    #[tokio::test]
    async fn product_matches_the_direct_parse() {
        let fetcher = FakeFetcher::new(Ok(PRODUCT));
        let sku = RetailerSku::new("1185461820").unwrap();
        let offer = source(&fetcher).product(&liverpool(), &sku).await.unwrap();
        let adapter = crate::adapter_for("liverpool").unwrap();
        let url = adapter.product_url(&sku).unwrap();
        assert_eq!(
            offer,
            adapter.parse_product(&direct(&url, PRODUCT)).unwrap()
        );
        assert_eq!(offer.fetched_at, NOW);
    }

    #[tokio::test]
    async fn bad_input_fails_before_fetching() {
        let fetcher = FakeFetcher::new(Ok(SEARCH));
        let source = source(&fetcher);
        let unknown = RetailerId::new("nope").unwrap();
        assert_eq!(
            source.search(&unknown, "licuadora").await,
            Err(OfferSourceError::UnknownRetailer)
        );
        assert_eq!(
            source.search(&liverpool(), "  ").await,
            Err(OfferSourceError::InvalidQuery)
        );
        let bad_sku = RetailerSku::new("x/1").unwrap();
        assert_eq!(
            source.product(&liverpool(), &bad_sku).await,
            Err(OfferSourceError::InvalidSku)
        );
        let sanborns = RetailerId::new("sanborns").unwrap();
        assert_eq!(
            source.search(&sanborns, "pantalla").await,
            Err(OfferSourceError::SearchUnavailable)
        );
        assert!(fetcher.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn fetch_failures_map_and_unparsable_pages_fail() {
        let kyc = FakeFetcher::new(Err(FetchError::BlockedNeedsKyc));
        assert_eq!(
            source(&kyc).search(&liverpool(), "licuadora").await,
            Err(OfferSourceError::BlockedNeedsKyc)
        );
        let failed = FakeFetcher::new(Err(FetchError::Failed));
        assert_eq!(
            source(&failed).search(&liverpool(), "licuadora").await,
            Err(OfferSourceError::FetchFailed)
        );
        let empty = FakeFetcher::new(Ok("<html></html>"));
        assert_eq!(
            source(&empty).search(&liverpool(), "licuadora").await,
            Err(OfferSourceError::ParseFailed)
        );
    }

    #[tokio::test]
    async fn parses_feed_the_parse_health() {
        let health = health(1);
        let empty = FakeFetcher::new(Ok("<html></html>"));
        let source = RetailerOfferSource::new(empty, Arc::new(FixedClock), health.clone());
        assert_eq!(
            source.search(&liverpool(), "licuadora").await,
            Err(OfferSourceError::ParseFailed)
        );
        let mut health = health.lock().unwrap();
        assert_eq!(health.rate("liverpool"), Some((0.0, 1)));
        // The failed search already raised the alert, so the next drop is `Low`.
        assert_eq!(
            health.record("liverpool", false),
            HealthStatus::Low { rate: 0.0 }
        );
    }

    #[tokio::test]
    async fn successful_product_parses_count_as_successes() {
        let health = health(1);
        let fetcher = FakeFetcher::new(Ok(PRODUCT));
        let source = RetailerOfferSource::new(fetcher, Arc::new(FixedClock), health.clone());
        let sku = RetailerSku::new("1185461820").unwrap();
        assert!(source.product(&liverpool(), &sku).await.is_ok());
        assert_eq!(health.lock().unwrap().rate("liverpool"), Some((1.0, 1)));
    }

    #[tokio::test]
    async fn lookup_only_resolves_hosts_and_refuses_to_fetch() {
        let source = RetailerOfferSource::lookup_only();
        assert_eq!(
            source.retailer_for_host("WWW.Liverpool.com.mx"),
            Some(liverpool())
        );
        assert_eq!(source.retailer_for_host("evil.example"), None);
        assert_eq!(
            source.search(&liverpool(), "licuadora").await,
            Err(OfferSourceError::Unavailable)
        );
        let sku = RetailerSku::new("1185461820").unwrap();
        assert_eq!(
            source.product(&liverpool(), &sku).await,
            Err(OfferSourceError::Unavailable)
        );
    }
}
