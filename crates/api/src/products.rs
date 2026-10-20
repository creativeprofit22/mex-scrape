//! Read-only product endpoints for signed-in users: Keepa-style price
//! stats and the daily price series behind the product chart.

use std::sync::Arc;

use app::{
    MAX_SERIES_DAYS, PriceSeries, PriceSeriesError, PriceStats, ProductPriceSeries,
    ProductPriceStats,
};
use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use domain::{Asin, PriceMxn, PriceWindow};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::auth::{ApiError, Auth, ErrorResponse, signed_in};

/// State of the product routes.
#[derive(Clone)]
pub struct ProductsState {
    inner: Option<Arc<Enabled>>,
}

struct Enabled {
    stats: Arc<PriceStats>,
    series: Arc<PriceSeries>,
}

impl ProductsState {
    /// Product routes answer 503 (no database).
    #[must_use]
    pub const fn disabled() -> Self {
        Self { inner: None }
    }

    /// Enabled with the price `stats` and `series` use cases.
    #[must_use]
    pub fn new(stats: Arc<PriceStats>, series: Arc<PriceSeries>) -> Self {
        Self {
            inner: Some(Arc::new(Enabled { stats, series })),
        }
    }

    fn enabled(&self) -> Result<&Enabled, ApiError> {
        self.inner
            .as_deref()
            .ok_or(ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))
    }
}

fn parse_asin(raw: &str) -> Result<Asin, ApiError> {
    Asin::new(raw).map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_asin"))
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
    /// One entry per accepted listing the user may see, sorted by retailer
    /// then SKU.
    pub listings: Vec<ListingPriceStatsResponse>,
    /// Listings left out because they need a store membership (Costco,
    /// Sam's Club) the user lacks.
    pub hidden_listings: u32,
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
            hidden_listings: s.hidden_listings,
        }
    }
}

/// One UTC day of a listing's prices.
#[derive(Debug, Serialize, ToSchema)]
pub struct DailyPriceResponse {
    /// Start of the UTC day (epoch milliseconds).
    pub day_ms: i64,
    /// Lowest price seen that day, in centavos.
    pub low_centavos: i64,
    /// Highest price seen that day, in centavos.
    pub high_centavos: i64,
    /// Whether any observation that day was in stock.
    pub in_stock: bool,
}

/// Daily prices for one retailer listing matched to the product.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListingPriceSeriesResponse {
    /// Retailer id, e.g. `walmart_mx`.
    pub retailer: String,
    /// The retailer's product id.
    pub retailer_sku: String,
    /// Days with observations, oldest first.
    pub points: Vec<DailyPriceResponse>,
}

/// Daily price series for an Amazon product across matched listings.
#[derive(Debug, Serialize, ToSchema)]
pub struct PriceHistoryResponse {
    /// The Amazon product.
    pub asin: String,
    /// Start of the series (epoch milliseconds).
    pub since_ms: i64,
    /// End of the series (epoch milliseconds).
    pub as_of_ms: i64,
    /// One entry per accepted listing the user may see, sorted by retailer
    /// then SKU.
    pub listings: Vec<ListingPriceSeriesResponse>,
    /// Listings left out because they need a store membership (Costco,
    /// Sam's Club) the user lacks.
    pub hidden_listings: u32,
}

impl From<ProductPriceSeries> for PriceHistoryResponse {
    fn from(s: ProductPriceSeries) -> Self {
        Self {
            asin: s.asin.as_str().to_owned(),
            since_ms: s.since.0,
            as_of_ms: s.as_of.0,
            listings: s
                .listings
                .into_iter()
                .map(|l| ListingPriceSeriesResponse {
                    retailer: l.retailer.as_str().to_owned(),
                    retailer_sku: l.retailer_sku.as_str().to_owned(),
                    points: l
                        .points
                        .into_iter()
                        .map(|p| DailyPriceResponse {
                            day_ms: p.day.0,
                            low_centavos: p.low_mxn.centavos(),
                            high_centavos: p.high_mxn.centavos(),
                            in_stock: p.in_stock,
                        })
                        .collect(),
                })
                .collect(),
            hidden_listings: s.hidden_listings,
        }
    }
}

/// Query of `GET /products/{asin}/price-history`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PriceHistoryQuery {
    /// Days back from now, 1–365 (default 365).
    #[param(minimum = 1, maximum = 365)]
    pub days: Option<u16>,
}

/// Lowest, highest and average retailer price over 30, 90 and 365 days for
/// every retailer listing matched (accepted) to an Amazon product. Listings
/// needing a membership the user lacks are left out and counted.
#[utoipa::path(
    get,
    path = "/products/{asin}/price-stats",
    operation_id = "product_price_stats",
    tag = "products",
    security(("session_cookie" = [])),
    params(("asin" = String, Path, description = "Amazon ASIN (10 letters or digits)")),
    responses(
        (status = OK, description = "Price stats per listing", body = PriceStatsResponse),
        (status = BAD_REQUEST, description = "Not a valid ASIN", body = ErrorResponse),
        (status = UNAUTHORIZED, description = "Not signed in", body = ErrorResponse),
        (status = TOO_MANY_REQUESTS, description = "Plan rate limit reached", body = ErrorResponse),
        (status = INTERNAL_SERVER_ERROR, description = "Price history could not be read", body = ErrorResponse),
        (status = SERVICE_UNAVAILABLE, description = "Price history is not configured", body = ErrorResponse)
    )
)]
pub(crate) async fn product_price_stats(
    State(state): State<ProductsState>,
    auth: Auth,
    Path(asin): Path<String>,
) -> Result<Json<PriceStatsResponse>, ApiError> {
    let inner = state.enabled()?;
    let user = signed_in(&auth)?;
    let asin = parse_asin(&asin)?;
    let out = inner
        .stats
        .for_product(&user.tenant, &asin, user.memberships)
        .await
        .map_err(|_| {
            tracing::warn!("reading price stats failed");
            ApiError::INTERNAL
        })?;
    Ok(Json(out.into()))
}

/// Daily low/high price per matched retailer listing, for the product
/// chart. Listings needing a membership the user lacks are left out and
/// counted.
#[utoipa::path(
    get,
    path = "/products/{asin}/price-history",
    operation_id = "product_price_history",
    tag = "products",
    security(("session_cookie" = [])),
    params(
        ("asin" = String, Path, description = "Amazon ASIN (10 letters or digits)"),
        PriceHistoryQuery
    ),
    responses(
        (status = OK, description = "Daily series per listing", body = PriceHistoryResponse),
        (status = BAD_REQUEST, description = "Not a valid ASIN or query", body = ErrorResponse),
        (status = UNAUTHORIZED, description = "Not signed in", body = ErrorResponse),
        (status = UNPROCESSABLE_ENTITY, description = "`days` outside 1–365", body = ErrorResponse),
        (status = TOO_MANY_REQUESTS, description = "Plan rate limit reached", body = ErrorResponse),
        (status = INTERNAL_SERVER_ERROR, description = "Price history could not be read", body = ErrorResponse),
        (status = SERVICE_UNAVAILABLE, description = "Price history is not configured", body = ErrorResponse)
    )
)]
pub(crate) async fn product_price_history(
    State(state): State<ProductsState>,
    auth: Auth,
    Path(asin): Path<String>,
    query: Result<Query<PriceHistoryQuery>, QueryRejection>,
) -> Result<Json<PriceHistoryResponse>, ApiError> {
    let inner = state.enabled()?;
    let user = signed_in(&auth)?;
    let asin = parse_asin(&asin)?;
    let Query(query) = query.map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_query"))?;
    let days = query.days.unwrap_or(MAX_SERIES_DAYS);
    match inner
        .series
        .for_product(&user.tenant, &asin, days, user.memberships)
        .await
    {
        Ok(out) => Ok(Json(out.into())),
        Err(PriceSeriesError::InvalidDays) => {
            Err(ApiError(StatusCode::UNPROCESSABLE_ENTITY, "invalid_days"))
        }
        Err(PriceSeriesError::Storage) => {
            tracing::warn!("reading the price series failed");
            Err(ApiError::INTERNAL)
        }
    }
}
