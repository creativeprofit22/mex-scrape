//! Public, read-only product endpoints: Keepa-style price stats.

use std::sync::Arc;

use app::{PriceStats, ProductPriceStats};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use domain::{Asin, PriceMxn, PriceWindow};
use serde::Serialize;
use utoipa::ToSchema;

/// State of the product routes.
#[derive(Clone)]
pub struct ProductsState {
    stats: Option<Arc<PriceStats>>,
}

impl ProductsState {
    /// Product routes answer 503 (no database).
    #[must_use]
    pub const fn disabled() -> Self {
        Self { stats: None }
    }

    /// Enabled with the price `stats` use case.
    #[must_use]
    pub fn new(stats: Arc<PriceStats>) -> Self {
        Self { stats: Some(stats) }
    }
}

/// Low, high and average price over one window. Amounts are integer
/// centavos; they are absent when no in-stock price was observed.
#[derive(Debug, Serialize, ToSchema)]
pub struct PriceWindowResponse {
    /// Window length in days (30, 90 or 365).
    pub days: u16,
    /// Lowest price, in centavos.
    pub low_centavos: Option<i64>,
    /// Highest price, in centavos.
    pub high_centavos: Option<i64>,
    /// Mean price (rounded half up), in centavos.
    pub average_centavos: Option<i64>,
    /// Observations counted (out-of-stock prices are not counted).
    pub observations: u32,
}

/// Price windows for one retailer listing matched to the product.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListingPriceStatsResponse {
    /// Retailer id, e.g. `walmart_mx`.
    pub retailer: String,
    /// The retailer's product id.
    pub retailer_sku: String,
    /// 30, 90 and 365-day windows, shortest first.
    pub windows: Vec<PriceWindowResponse>,
}

/// Price stats for an Amazon product across matched retailer listings.
#[derive(Debug, Serialize, ToSchema)]
pub struct PriceStatsResponse {
    /// The Amazon product.
    pub asin: String,
    /// Instant every window ends at (epoch milliseconds).
    pub as_of_ms: i64,
    /// One entry per accepted listing, sorted by retailer then SKU.
    pub listings: Vec<ListingPriceStatsResponse>,
}

fn centavos(p: Option<PriceMxn>) -> Option<i64> {
    p.map(PriceMxn::centavos)
}

impl From<&PriceWindow> for PriceWindowResponse {
    fn from(w: &PriceWindow) -> Self {
        Self {
            days: w.days,
            low_centavos: centavos(w.low),
            high_centavos: centavos(w.high),
            average_centavos: centavos(w.average),
            observations: w.observations,
        }
    }
}

impl From<ProductPriceStats> for PriceStatsResponse {
    fn from(s: ProductPriceStats) -> Self {
        Self {
            asin: s.asin.as_str().to_owned(),
            as_of_ms: s.as_of.0,
            listings: s
                .listings
                .into_iter()
                .map(|l| ListingPriceStatsResponse {
                    retailer: l.retailer.as_str().to_owned(),
                    retailer_sku: l.retailer_sku.as_str().to_owned(),
                    windows: l.windows.iter().map(Into::into).collect(),
                })
                .collect(),
        }
    }
}

/// Lowest, highest and average retailer price over 30, 90 and 365 days for
/// every retailer listing matched (accepted) to an Amazon product.
#[utoipa::path(
    get,
    path = "/products/{asin}/price-stats",
    operation_id = "product_price_stats",
    params(("asin" = String, Path, description = "Amazon ASIN (10 letters or digits)")),
    responses(
        (status = OK, description = "Price stats per listing", body = PriceStatsResponse),
        (status = BAD_REQUEST, description = "Not a valid ASIN"),
        (status = INTERNAL_SERVER_ERROR, description = "Price history could not be read"),
        (status = SERVICE_UNAVAILABLE, description = "Price history is not configured")
    )
)]
pub(crate) async fn product_price_stats(
    State(state): State<ProductsState>,
    Path(asin): Path<String>,
) -> Result<Json<PriceStatsResponse>, StatusCode> {
    let asin = Asin::new(&asin).map_err(|_| StatusCode::BAD_REQUEST)?;
    let Some(stats) = &state.stats else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };
    let out = stats.for_product(&asin).await.map_err(|_| {
        tracing::warn!("reading price stats failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(out.into()))
}
