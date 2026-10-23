//! Demo fee estimates: the Bright Data fallback has none.

use std::sync::Arc;

use app::{
    AmazonDataSource, AmazonSource, AmazonSourceError, CatalogResult, FeeDetail, FeeEstimate,
    Fulfillment, OfferSummary, PortFuture,
};
use db::demo::catalog::DemoCatalog;
use domain::{Asin, PriceMxn};

/// Amazon's referral fee in this demo, in per cent of the sale price.
pub const REFERRAL_FEE_PERCENT: i64 = 15;

/// Demo fee estimates (the Bright Data fallback has none): the referral fee
/// on the sale price plus the catalog's FBA fee. Catalog and offer lookups
/// are `NotConfigured`, so they fall back to Bright Data.
pub struct DemoFees {
    catalog: DemoCatalog,
    clock: Arc<dyn app::Clock>,
}

impl DemoFees {
    /// Fees for the products of `catalog`.
    #[must_use]
    pub fn new(catalog: DemoCatalog, clock: Arc<dyn app::Clock>) -> Self {
        Self { catalog, clock }
    }

    /// The estimate for `asin` sold at `price`, or `None` for an ASIN the
    /// catalog does not have.
    #[must_use]
    pub fn estimate(&self, asin: &Asin, price: PriceMxn) -> Option<FeeEstimate> {
        let amazon = self
            .catalog
            .products
            .iter()
            .filter_map(|p| p.amazon.as_ref())
            .find(|a| a.asin == asin.as_str())?;
        let referral =
            PriceMxn::from_centavos(price.centavos() * REFERRAL_FEE_PERCENT / 100).ok()?;
        let fba = PriceMxn::from_centavos(amazon.fba_fee_centavos).ok()?;
        let total = PriceMxn::from_centavos(referral.centavos() + fba.centavos()).ok()?;
        Some(FeeEstimate {
            total_mxn: total,
            details: vec![
                FeeDetail {
                    fee_type: "ReferralFee".to_owned(),
                    amount_mxn: referral,
                },
                FeeDetail {
                    fee_type: "FBAFees".to_owned(),
                    amount_mxn: fba,
                },
            ],
            // Stands in for SP-API's fee estimate, the only real fee source.
            source: AmazonDataSource::SpApi,
            fetched_at: self.clock.now(),
        })
    }
}

impl AmazonSource for DemoFees {
    fn catalog_by_gtin<'a>(
        &'a self,
        _gtin: &'a domain::Gtin,
    ) -> PortFuture<'a, Result<CatalogResult, AmazonSourceError>> {
        Box::pin(async { Err(AmazonSourceError::NotConfigured) })
    }

    fn catalog_by_keyword<'a>(
        &'a self,
        _keywords: &'a str,
    ) -> PortFuture<'a, Result<CatalogResult, AmazonSourceError>> {
        Box::pin(async { Err(AmazonSourceError::NotConfigured) })
    }

    fn offers<'a>(
        &'a self,
        _asin: &'a Asin,
    ) -> PortFuture<'a, Result<OfferSummary, AmazonSourceError>> {
        Box::pin(async { Err(AmazonSourceError::NotConfigured) })
    }

    fn fees<'a>(
        &'a self,
        asin: &'a Asin,
        price: PriceMxn,
        _fulfillment: Fulfillment,
    ) -> PortFuture<'a, Result<FeeEstimate, AmazonSourceError>> {
        Box::pin(async move {
            self.estimate(asin, price)
                .ok_or(AmazonSourceError::NotConfigured)
        })
    }
}
