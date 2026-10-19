//! [`AmazonSource`] over the SP-API client (the primary source).

use std::sync::Arc;

use app::{
    AmazonDataSource, AmazonSource, AmazonSourceError, CatalogItem, CatalogResult, FeeEstimate,
    Fulfillment, OfferSummary, PortFuture,
};
use brightdata::runs::Clock;
use domain::{Asin, Gtin, PriceMxn, UnixMillis};

use crate::client::SpApiClient;
use crate::error::SpApiError;

/// The SP-API as an [`AmazonSource`]. Errors cross the port without detail.
pub struct SpApiAmazonSource<C> {
    client: Arc<SpApiClient>,
    clock: C,
}

impl<C: Clock> SpApiAmazonSource<C> {
    /// Wraps `client`; `clock` stamps `fetched_at`.
    #[must_use]
    pub fn new(client: Arc<SpApiClient>, clock: C) -> Self {
        Self { client, clock }
    }

    fn now(&self) -> UnixMillis {
        UnixMillis(self.clock.now_ms())
    }

    fn catalog(
        &self,
        result: Result<Vec<CatalogItem>, SpApiError>,
    ) -> Result<CatalogResult, AmazonSourceError> {
        let items = result.map_err(|e| e.to_source_error())?;
        Ok(CatalogResult {
            items,
            source: AmazonDataSource::SpApi,
            fetched_at: self.now(),
        })
    }
}

impl<C: Clock> AmazonSource for SpApiAmazonSource<C> {
    fn catalog_by_gtin<'a>(
        &'a self,
        gtin: &'a Gtin,
    ) -> PortFuture<'a, Result<CatalogResult, AmazonSourceError>> {
        Box::pin(async move {
            let result = self
                .client
                .search_catalog_by_identifiers(std::slice::from_ref(gtin))
                .await;
            self.catalog(result)
        })
    }

    fn catalog_by_keyword<'a>(
        &'a self,
        keywords: &'a str,
    ) -> PortFuture<'a, Result<CatalogResult, AmazonSourceError>> {
        Box::pin(async move {
            let result = self.client.search_catalog_by_keyword(keywords).await;
            self.catalog(result)
        })
    }

    fn offers<'a>(
        &'a self,
        asin: &'a Asin,
    ) -> PortFuture<'a, Result<OfferSummary, AmazonSourceError>> {
        Box::pin(async move {
            let prices = self
                .client
                .item_offers(asin)
                .await
                .map_err(|e| e.to_source_error())?;
            Ok(prices.into_summary(AmazonDataSource::SpApi, self.now()))
        })
    }

    fn fees<'a>(
        &'a self,
        asin: &'a Asin,
        price: PriceMxn,
        fulfillment: Fulfillment,
    ) -> PortFuture<'a, Result<FeeEstimate, AmazonSourceError>> {
        Box::pin(async move {
            let fees = self
                .client
                .fees_estimate(asin, price, fulfillment == Fulfillment::Fba)
                .await
                .map_err(|e| e.to_source_error())?;
            Ok(FeeEstimate {
                total_mxn: fees.total_mxn,
                details: fees.details,
                source: AmazonDataSource::SpApi,
                fetched_at: self.now(),
            })
        })
    }
}
