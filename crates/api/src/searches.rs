//! Search endpoints: enqueue a background search and read its results
//! (ADR 0011). The worker runs the search; these routes only enqueue and
//! read. Each search belongs to the signed-in user's tenant (ADR 0012), and
//! results needing a store membership the user lacks are hidden.
//!
//! Without a database every route answers 503 (fails closed); without a
//! session they answer 401 before the body or id is looked at.

use std::sync::Arc;

use app::{
    ResultMatchView, RetailerOutcomeView, SearchError, SearchId, SearchResultView, SearchResults,
    SearchView, StartSearch,
};
use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::{ApiError, Auth, ErrorResponse, json_body, signed_in};

/// State of the search routes.
#[derive(Clone)]
pub struct SearchesState {
    inner: Option<Arc<Enabled>>,
}

struct Enabled {
    start: StartSearch,
    results: SearchResults,
}

impl SearchesState {
    /// Search routes answer 503.
    #[must_use]
    pub const fn disabled() -> Self {
        Self { inner: None }
    }

    /// Enabled with the search use cases.
    #[must_use]
    pub fn new(start: StartSearch, results: SearchResults) -> Self {
        Self {
            inner: Some(Arc::new(Enabled { start, results })),
        }
    }

    fn enabled(&self) -> Result<&Enabled, ApiError> {
        self.inner
            .as_deref()
            .ok_or(ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))
    }
}

/// Body of `POST /searches`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct StartSearchRequest {
    /// Keywords, 1–100 characters after trimming and collapsing spaces.
    pub query: String,
}

/// A search was enqueued.
#[derive(Debug, Serialize, ToSchema)]
pub struct SearchCreatedResponse {
    /// Id to poll with `GET /searches/{id}`.
    pub id: i64,
}

/// What one retailer contributed to a search.
#[derive(Debug, Serialize, ToSchema)]
pub struct SearchRetailerResponse {
    /// Retailer id, e.g. `walmart_mx`.
    pub retailer: String,
    /// `ok`, `category_only` (no keyword search; covered by scheduled
    /// category browsing), `blocked_needs_kyc`, `skipped_robots`, `failed`
    /// or `no_offers`.
    pub outcome: String,
    /// Offers it returned.
    pub offers: u32,
}

/// Amazon match and ROI for one result. Money is integer centavos and
/// ratios are basis points.
#[derive(Debug, Serialize, ToSchema)]
pub struct SearchMatchResponse {
    /// Matched Amazon product.
    pub asin: String,
    /// `accepted` or `needs_review`.
    pub match_status: String,
    /// Matcher confidence, basis points (0–10000).
    pub confidence_bp: u16,
    /// Amazon selling price used (buy box, else lowest new), in centavos.
    pub amazon_price_centavos: Option<i64>,
    /// Estimated Amazon fees at that price, in centavos.
    pub fees_centavos: Option<i64>,
    /// Profit per unit, in centavos (negative for a loss).
    pub profit_centavos: Option<i64>,
    /// Profit over total cost, basis points.
    pub roi_bp: Option<i64>,
    /// Profit over selling price, basis points.
    pub margin_bp: Option<i64>,
}

/// One retailer offer found by a search.
#[derive(Debug, Serialize, ToSchema)]
pub struct SearchResultResponse {
    /// Retailer id.
    pub retailer: String,
    /// The retailer's product id.
    pub retailer_sku: String,
    /// Listing title.
    pub title: String,
    /// Listing URL.
    pub url: String,
    /// Retailer price, in centavos.
    pub price_centavos: i64,
    /// `in_stock`, `out_of_stock` or `unknown` when the offer was seen.
    pub availability: String,
    /// Buying needs a store membership (Costco, Sam's Club) the user holds.
    pub membership_required: bool,
    /// Amazon match and ROI, when one was found.
    pub amazon: Option<SearchMatchResponse>,
}

/// A search with its status, per-retailer outcomes and results.
#[derive(Debug, Serialize, ToSchema)]
pub struct SearchResponse {
    /// Search id.
    pub id: i64,
    /// The cleaned query.
    pub query: String,
    /// `queued`, `running`, `done`, `partial`, `failed` or `refused`.
    pub status: String,
    /// Fixed reason code for `partial`, `failed` or `refused`.
    pub reason: Option<String>,
    /// When the search was created (epoch milliseconds).
    pub created_at_ms: i64,
    /// When it finished (epoch milliseconds).
    pub finished_at_ms: Option<i64>,
    /// Per-retailer outcomes, sorted by retailer.
    pub retailers: Vec<SearchRetailerResponse>,
    /// Results, sorted by retailer then SKU.
    pub results: Vec<SearchResultResponse>,
    /// Results hidden because they need a store membership the user lacks.
    pub hidden_results: u32,
}

impl From<RetailerOutcomeView> for SearchRetailerResponse {
    fn from(r: RetailerOutcomeView) -> Self {
        Self {
            retailer: r.retailer.as_str().to_owned(),
            outcome: r.outcome.as_str().to_owned(),
            offers: r.offers,
        }
    }
}

impl From<ResultMatchView> for SearchMatchResponse {
    fn from(m: ResultMatchView) -> Self {
        Self {
            asin: m.asin.as_str().to_owned(),
            match_status: m.status.as_str().to_owned(),
            confidence_bp: m.confidence.bp(),
            amazon_price_centavos: m.amazon_price_mxn.map(domain::PriceMxn::centavos),
            fees_centavos: m.fees_mxn.map(domain::PriceMxn::centavos),
            profit_centavos: m.profit_centavos,
            roi_bp: m.roi_bp,
            margin_bp: m.margin_bp,
        }
    }
}

impl From<SearchResultView> for SearchResultResponse {
    fn from(r: SearchResultView) -> Self {
        Self {
            retailer: r.retailer.as_str().to_owned(),
            retailer_sku: r.retailer_sku.as_str().to_owned(),
            title: r.title,
            url: r.url,
            price_centavos: r.price_mxn.centavos(),
            availability: r.availability.as_str().to_owned(),
            membership_required: r.membership_required,
            amazon: r.amazon.map(Into::into),
        }
    }
}

impl From<SearchView> for SearchResponse {
    fn from(s: SearchView) -> Self {
        Self {
            id: s.id.0,
            query: s.query,
            status: s.status.as_str().to_owned(),
            reason: (!s.reason.is_empty()).then_some(s.reason),
            created_at_ms: s.created_at.0,
            finished_at_ms: s.finished_at.map(|t| t.0),
            retailers: s.retailers.into_iter().map(Into::into).collect(),
            results: s.results.into_iter().map(Into::into).collect(),
            hidden_results: s.hidden_results,
        }
    }
}

/// Enqueue a background search across retailers and Amazon for the
/// signed-in user.
#[utoipa::path(
    post,
    path = "/searches",
    operation_id = "start_search",
    tag = "search",
    security(("session_cookie" = [], "csrf_token" = [])),
    request_body = StartSearchRequest,
    responses(
        (status = ACCEPTED, description = "Search enqueued", body = SearchCreatedResponse),
        (status = BAD_REQUEST, description = "Body is not JSON", body = ErrorResponse),
        (status = UNAUTHORIZED, description = "Not signed in", body = ErrorResponse),
        (status = FORBIDDEN, description = "CSRF check failed", body = ErrorResponse),
        (status = UNPROCESSABLE_ENTITY, description = "Missing, empty or too long query", body = ErrorResponse),
        (status = TOO_MANY_REQUESTS, description = "Plan rate limit reached", body = ErrorResponse),
        (status = INTERNAL_SERVER_ERROR, description = "Search could not be enqueued", body = ErrorResponse),
        (status = SERVICE_UNAVAILABLE, description = "Search API is not configured", body = ErrorResponse)
    )
)]
pub(crate) async fn start_search(
    State(state): State<SearchesState>,
    auth: Auth,
    body: Result<Json<StartSearchRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<SearchCreatedResponse>), ApiError> {
    let inner = state.enabled()?;
    let user = signed_in(&auth)?;
    let body = json_body(body)?;
    match inner.start.start(&user.tenant, &body.query).await {
        Ok(id) => Ok((
            StatusCode::ACCEPTED,
            Json(SearchCreatedResponse { id: id.0 }),
        )),
        Err(SearchError::InvalidQuery(_)) => {
            Err(ApiError(StatusCode::UNPROCESSABLE_ENTITY, "invalid_query"))
        }
        Err(SearchError::Storage) => {
            tracing::warn!("enqueuing a search failed");
            Err(ApiError::INTERNAL)
        }
    }
}

/// A search of the signed-in user: status, per-retailer outcomes and
/// results with ROI. Results needing a membership the user lacks are left
/// out and counted in `hidden_results`.
#[utoipa::path(
    get,
    path = "/searches/{id}",
    operation_id = "get_search",
    tag = "search",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Search id from `POST /searches`")),
    responses(
        (status = OK, description = "The search", body = SearchResponse),
        (status = UNAUTHORIZED, description = "Not signed in", body = ErrorResponse),
        (status = NOT_FOUND, description = "No such search for this user", body = ErrorResponse),
        (status = TOO_MANY_REQUESTS, description = "Plan rate limit reached", body = ErrorResponse),
        (status = INTERNAL_SERVER_ERROR, description = "Search could not be read", body = ErrorResponse),
        (status = SERVICE_UNAVAILABLE, description = "Search API is not configured", body = ErrorResponse)
    )
)]
pub(crate) async fn get_search(
    State(state): State<SearchesState>,
    auth: Auth,
    Path(id): Path<String>,
) -> Result<Json<SearchResponse>, ApiError> {
    let inner = state.enabled()?;
    let user = signed_in(&auth)?;
    let not_found = ApiError(StatusCode::NOT_FOUND, "not_found");
    let id = id
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or(not_found)?;
    match inner
        .results
        .get(&user.tenant, SearchId(id), user.memberships)
        .await
    {
        Ok(Some(view)) => Ok(Json(view.into())),
        Ok(None) => Err(not_found),
        Err(_) => {
            tracing::warn!("reading a search failed");
            Err(ApiError::INTERNAL)
        }
    }
}
