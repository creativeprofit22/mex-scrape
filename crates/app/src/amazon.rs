//! Amazon source routing: SP-API first, Bright Data only as a fallback.

use std::sync::Arc;

use domain::{Asin, Gtin, PriceMxn};

use crate::ports::{
    AmazonSource, AmazonSourceError, CatalogResult, FeeEstimate, Fulfillment, OfferSummary,
    PortFuture,
};

/// An [`AmazonSource`] that asks `primary` first and `fallback` only when
/// the primary is absent, not configured, not authorized or unavailable.
///
/// Rate limits, parse failures, bad input and unsupported lookups are
/// returned as-is, so a busy SP-API never triggers paid fallback calls.
pub struct FallbackAmazonSource {
    primary: Option<Arc<dyn AmazonSource>>,
    fallback: Option<Arc<dyn AmazonSource>>,
}

impl FallbackAmazonSource {
    /// Routes between `primary` (SP-API) and `fallback` (Bright Data).
    /// Either may be `None` when its credentials are missing.
    #[must_use]
    pub fn new(
        primary: Option<Arc<dyn AmazonSource>>,
        fallback: Option<Arc<dyn AmazonSource>>,
    ) -> Self {
        Self { primary, fallback }
    }

    /// True when the error means "this source cannot serve anything right
    /// now", so the fallback may be tried.
    #[must_use]
    pub const fn falls_back(err: AmazonSourceError) -> bool {
        matches!(
            err,
            AmazonSourceError::NotConfigured
                | AmazonSourceError::NotAuthorized
                | AmazonSourceError::Unavailable
        )
    }

    fn route<'a, T: Send + 'a>(
        &'a self,
        call: impl Fn(&'a dyn AmazonSource) -> PortFuture<'a, Result<T, AmazonSourceError>> + Send + 'a,
    ) -> PortFuture<'a, Result<T, AmazonSourceError>> {
        Box::pin(async move {
            let mut last = AmazonSourceError::NotConfigured;
            if let Some(primary) = &self.primary {
                match call(primary.as_ref()).await {
                    Err(e) if Self::falls_back(e) => last = e,
                    other => return other,
                }
            }
            match &self.fallback {
                Some(fallback) => call(fallback.as_ref()).await,
                None => Err(last),
            }
        })
    }
}

impl AmazonSource for FallbackAmazonSource {
    fn catalog_by_gtin<'a>(
        &'a self,
        gtin: &'a Gtin,
    ) -> PortFuture<'a, Result<CatalogResult, AmazonSourceError>> {
        self.route(move |s| s.catalog_by_gtin(gtin))
    }

    fn catalog_by_keyword<'a>(
        &'a self,
        keywords: &'a str,
    ) -> PortFuture<'a, Result<CatalogResult, AmazonSourceError>> {
        self.route(move |s| s.catalog_by_keyword(keywords))
    }

    fn offers<'a>(
        &'a self,
        asin: &'a Asin,
    ) -> PortFuture<'a, Result<OfferSummary, AmazonSourceError>> {
        self.route(move |s| s.offers(asin))
    }

    fn fees<'a>(
        &'a self,
        asin: &'a Asin,
        price: PriceMxn,
        fulfillment: Fulfillment,
    ) -> PortFuture<'a, Result<FeeEstimate, AmazonSourceError>> {
        self.route(move |s| s.fees(asin, price, fulfillment))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use domain::UnixMillis;

    use super::*;
    use crate::ports::AmazonDataSource;

    /// Answers every call with `result` and counts calls.
    struct Fake {
        source: AmazonDataSource,
        result: Result<(), AmazonSourceError>,
        calls: AtomicU32,
    }

    impl Fake {
        fn new(source: AmazonDataSource, result: Result<(), AmazonSourceError>) -> Arc<Self> {
            Arc::new(Self {
                source,
                result,
                calls: AtomicU32::new(0),
            })
        }

        fn hit<T>(&self, ok: T) -> Result<T, AmazonSourceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result.map(|()| ok)
        }

        fn catalog(&self) -> CatalogResult {
            CatalogResult {
                items: Vec::new(),
                source: self.source,
                fetched_at: UnixMillis(1),
            }
        }
    }

    impl AmazonSource for Fake {
        fn catalog_by_gtin<'a>(
            &'a self,
            _: &'a Gtin,
        ) -> PortFuture<'a, Result<CatalogResult, AmazonSourceError>> {
            Box::pin(async move { self.hit(self.catalog()) })
        }

        fn catalog_by_keyword<'a>(
            &'a self,
            _: &'a str,
        ) -> PortFuture<'a, Result<CatalogResult, AmazonSourceError>> {
            Box::pin(async move { self.hit(self.catalog()) })
        }

        fn offers<'a>(
            &'a self,
            _: &'a Asin,
        ) -> PortFuture<'a, Result<OfferSummary, AmazonSourceError>> {
            Box::pin(async move {
                self.hit(OfferSummary {
                    buy_box_mxn: None,
                    lowest_new_mxn: None,
                    offer_count: 0,
                    source: self.source,
                    fetched_at: UnixMillis(1),
                })
            })
        }

        fn fees<'a>(
            &'a self,
            _: &'a Asin,
            price: PriceMxn,
            _: Fulfillment,
        ) -> PortFuture<'a, Result<FeeEstimate, AmazonSourceError>> {
            Box::pin(async move {
                self.hit(FeeEstimate {
                    total_mxn: price,
                    details: Vec::new(),
                    source: self.source,
                    fetched_at: UnixMillis(1),
                })
            })
        }
    }

    fn gtin() -> Gtin {
        Gtin::new("7501055363063").unwrap()
    }

    fn routed(primary: Option<&Arc<Fake>>, fallback: Option<&Arc<Fake>>) -> FallbackAmazonSource {
        let dyn_arc = |f: &Arc<Fake>| -> Arc<dyn AmazonSource> { f.clone() };
        FallbackAmazonSource::new(primary.map(dyn_arc), fallback.map(dyn_arc))
    }

    #[tokio::test]
    async fn healthy_primary_never_touches_fallback() {
        let sp = Fake::new(AmazonDataSource::SpApi, Ok(()));
        let bd = Fake::new(AmazonDataSource::BrightData, Ok(()));
        let r = routed(Some(&sp), Some(&bd));
        let got = r.catalog_by_gtin(&gtin()).await.unwrap();
        assert_eq!(got.source, AmazonDataSource::SpApi);
        assert_eq!(bd.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn unavailable_or_unauthorized_primary_uses_fallback() {
        for err in [
            AmazonSourceError::NotConfigured,
            AmazonSourceError::NotAuthorized,
            AmazonSourceError::Unavailable,
        ] {
            let sp = Fake::new(AmazonDataSource::SpApi, Err(err));
            let bd = Fake::new(AmazonDataSource::BrightData, Ok(()));
            let r = routed(Some(&sp), Some(&bd));
            let got = r.catalog_by_keyword("taladro").await.unwrap();
            assert_eq!(got.source, AmazonDataSource::BrightData, "{err:?}");
            assert_eq!(sp.calls.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn rate_limit_and_parse_errors_do_not_fall_back() {
        for err in [
            AmazonSourceError::RateLimited,
            AmazonSourceError::ParseFailed,
            AmazonSourceError::InvalidInput,
            AmazonSourceError::Unsupported,
        ] {
            let sp = Fake::new(AmazonDataSource::SpApi, Err(err));
            let bd = Fake::new(AmazonDataSource::BrightData, Ok(()));
            let r = routed(Some(&sp), Some(&bd));
            let asin = Asin::new("B0CHHSFMRL").unwrap();
            assert_eq!(r.offers(&asin).await.unwrap_err(), err);
            assert_eq!(bd.calls.load(Ordering::SeqCst), 0, "{err:?}");
        }
    }

    #[tokio::test]
    async fn missing_primary_goes_straight_to_fallback() {
        let bd = Fake::new(AmazonDataSource::BrightData, Ok(()));
        let r = routed(None, Some(&bd));
        let asin = Asin::new("B0CHHSFMRL").unwrap();
        let price = PriceMxn::from_centavos(10_000).unwrap();
        let got = r.fees(&asin, price, Fulfillment::Fba).await.unwrap();
        assert_eq!(got.source, AmazonDataSource::BrightData);
    }

    #[tokio::test]
    async fn no_sources_is_not_configured_and_primary_error_survives() {
        let none = routed(None, None);
        assert_eq!(
            none.catalog_by_gtin(&gtin()).await.unwrap_err(),
            AmazonSourceError::NotConfigured
        );
        let sp = Fake::new(
            AmazonDataSource::SpApi,
            Err(AmazonSourceError::NotAuthorized),
        );
        let only_sp = routed(Some(&sp), None);
        assert_eq!(
            only_sp.catalog_by_gtin(&gtin()).await.unwrap_err(),
            AmazonSourceError::NotAuthorized
        );
    }

    #[tokio::test]
    async fn fallback_error_is_returned_when_both_fail() {
        let sp = Fake::new(AmazonDataSource::SpApi, Err(AmazonSourceError::Unavailable));
        let bd = Fake::new(
            AmazonDataSource::BrightData,
            Err(AmazonSourceError::Unsupported),
        );
        let r = routed(Some(&sp), Some(&bd));
        let asin = Asin::new("B0CHHSFMRL").unwrap();
        let price = PriceMxn::from_centavos(10_000).unwrap();
        assert_eq!(
            r.fees(&asin, price, Fulfillment::Merchant)
                .await
                .unwrap_err(),
            AmazonSourceError::Unsupported
        );
    }
}
