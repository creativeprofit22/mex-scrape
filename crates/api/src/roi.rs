//! `POST /roi`: ROI for up to 100 items in one call. The calculator page
//! and the search page both use it, so their numbers always agree with
//! `domain::calculate_roi` (the worker uses the same function).

use app::{MAX_ROI_BATCH, RoiBatch, RoiBatchError};
use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use domain::{PriceMxn, RoiError, RoiInput, RoiResult};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::{ApiError, Auth, ErrorResponse, json_body, signed_in};

/// One item to evaluate. Amounts are integer centavos (0 or more).
#[derive(Debug, Deserialize, ToSchema)]
pub struct RoiItemRequest {
    /// Amazon selling price.
    pub sell_price_centavos: i64,
    /// Retailer purchase price.
    pub buy_price_centavos: i64,
    /// Amazon fees for one unit at the selling price.
    pub amazon_fees_centavos: i64,
    /// Prep-center fee per unit.
    #[serde(default)]
    pub prep_fee_centavos: i64,
    /// Inbound shipping per unit.
    #[serde(default)]
    pub shipping_centavos: i64,
    /// Monthly fixed costs spread over `units_per_month`.
    #[serde(default)]
    pub monthly_fixed_centavos: i64,
    /// Units sold per month (needed when monthly fixed costs are set).
    #[serde(default)]
    pub units_per_month: u32,
}

/// Body of `POST /roi`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RoiRequest {
    /// 1–100 items.
    pub items: Vec<RoiItemRequest>,
}

/// Result for one item, in input order. Money is centavos, ratios basis
/// points. `error` is set (and the numbers absent) when the item cannot be
/// computed.
#[derive(Debug, Default, Serialize, ToSchema)]
pub struct RoiItemResponse {
    /// `zero_units` (fixed costs without units) or `overflow`.
    pub error: Option<String>,
    /// Share of the monthly fixed costs per unit.
    pub fixed_cost_per_unit_centavos: Option<i64>,
    /// Buy + prep + shipping + fixed share.
    pub total_cost_centavos: Option<i64>,
    /// Profit per unit (negative for a loss).
    pub profit_centavos: Option<i64>,
    /// Profit over total cost.
    pub roi_bp: Option<i64>,
    /// Profit over selling price.
    pub margin_bp: Option<i64>,
}

/// Body of a `POST /roi` response.
#[derive(Debug, Serialize, ToSchema)]
pub struct RoiResponse {
    /// One result per item, in input order.
    pub results: Vec<RoiItemResponse>,
}

impl From<Result<RoiResult, RoiError>> for RoiItemResponse {
    fn from(r: Result<RoiResult, RoiError>) -> Self {
        match r {
            Ok(r) => Self {
                error: None,
                fixed_cost_per_unit_centavos: Some(r.fixed_cost_per_unit.centavos()),
                total_cost_centavos: Some(r.total_cost.centavos()),
                profit_centavos: Some(r.profit_centavos),
                roi_bp: r.roi_bp,
                margin_bp: r.margin_bp,
            },
            Err(e) => Self {
                error: Some(
                    match e {
                        RoiError::ZeroUnits => "zero_units",
                        RoiError::Overflow => "overflow",
                    }
                    .to_owned(),
                ),
                ..Self::default()
            },
        }
    }
}

fn price(centavos: i64) -> Result<PriceMxn, ApiError> {
    PriceMxn::from_centavos(centavos)
        .map_err(|_| ApiError(StatusCode::UNPROCESSABLE_ENTITY, "invalid_amount"))
}

fn roi_input(i: &RoiItemRequest) -> Result<RoiInput, ApiError> {
    Ok(RoiInput {
        sell_price: price(i.sell_price_centavos)?,
        buy_price: price(i.buy_price_centavos)?,
        amazon_fees: price(i.amazon_fees_centavos)?,
        prep_fee: price(i.prep_fee_centavos)?,
        shipping: price(i.shipping_centavos)?,
        monthly_fixed: price(i.monthly_fixed_centavos)?,
        units_per_month: i.units_per_month,
    })
}

/// Profit, ROI and margin per unit for up to 100 items, using the same
/// calculation as search results.
#[utoipa::path(
    post,
    path = "/roi",
    operation_id = "calculate_roi",
    tag = "roi",
    security(("session_cookie" = [], "csrf_token" = [])),
    request_body = RoiRequest,
    responses(
        (status = OK, description = "One result per item", body = RoiResponse),
        (status = BAD_REQUEST, description = "Body is not JSON", body = ErrorResponse),
        (status = UNAUTHORIZED, description = "Not signed in", body = ErrorResponse),
        (status = FORBIDDEN, description = "CSRF check failed", body = ErrorResponse),
        (status = UNPROCESSABLE_ENTITY, description = "`invalid_amount`, `empty` or `too_many`", body = ErrorResponse),
        (status = TOO_MANY_REQUESTS, description = "Plan rate limit reached", body = ErrorResponse),
        (status = SERVICE_UNAVAILABLE, description = "Accounts are not configured", body = ErrorResponse)
    )
)]
pub(crate) async fn calculate_roi(
    auth: Auth,
    body: Result<Json<RoiRequest>, JsonRejection>,
) -> Result<Json<RoiResponse>, ApiError> {
    signed_in(&auth)?;
    let body = json_body(body)?;
    // Size first, so an oversized body is refused before any parsing work.
    if body.items.len() > MAX_ROI_BATCH {
        return Err(ApiError(StatusCode::UNPROCESSABLE_ENTITY, "too_many"));
    }
    let inputs = body
        .items
        .iter()
        .map(roi_input)
        .collect::<Result<Vec<_>, _>>()?;
    let results = RoiBatch.calculate(&inputs).map_err(|e| match e {
        RoiBatchError::Empty => ApiError(StatusCode::UNPROCESSABLE_ENTITY, "empty"),
        RoiBatchError::TooMany => ApiError(StatusCode::UNPROCESSABLE_ENTITY, "too_many"),
    })?;
    Ok(Json(RoiResponse {
        results: results.into_iter().map(Into::into).collect(),
    }))
}
