//! `/internal/agent/*`: the routes behind the sidecar's tools (ADR 0013).
//!
//! Outside the session layers. Every request needs `Authorization: Bearer
//! AGENT_SERVICE_TOKEN` and an `X-Agent-Grant` minted by the api for one
//! chat. The user is reloaded from storage and must still own the grant's
//! tenant; tool arguments never pick the tenant. Only
//! `match-suggestions` and `usage` write, and suggestions never change a
//! match.

use app::{ChatId, MAX_TRACKED_LIMIT, TrackedQuery, UserRecord};
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Extension, Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse as _, Response};
use axum::{Json, http::header};
use domain::{
    Asin, MatchDecision, MatchStatus, SearchQuery, SuggestionReason, TokenCount, TokenUsage,
};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq as _;
use utoipa::{IntoParams, ToSchema};

use super::{AssistantState, MatchSuggestionResponse, review_error, review_target};
use crate::auth::{ApiError, ErrorResponse, json_body};
use crate::products::{PriceHistoryQuery, PriceHistoryResponse, PriceStatsResponse};
use crate::roi::{RoiRequest, RoiResponse};
use crate::searches::SearchResultResponse;

/// Header carrying the per-chat grant.
pub const GRANT_HEADER: &str = "x-agent-grant";

/// Most tokens one usage report may add.
const MAX_USAGE_REPORT: u64 = 1_000_000;

/// The chat a tool call belongs to, from a verified grant.
#[derive(Debug, Clone)]
pub(crate) struct AgentCaller {
    user: UserRecord,
    chat_id: ChatId,
}

fn bearer(request: &Request) -> Option<&str> {
    request
        .headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

/// Checks the service token and grant, then attaches the [`AgentCaller`].
pub(crate) async fn agent_guard(
    State(state): State<AssistantState>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(internal) = state.internal() else {
        return ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable").into_response();
    };
    let expected = internal.auth.service_token.expose_secret().as_bytes();
    let token_ok = bearer(&request).is_some_and(|t| bool::from(t.as_bytes().ct_eq(expected)));
    if !token_ok {
        return ApiError::UNAUTHENTICATED.into_response();
    }
    let invalid_grant = ApiError(StatusCode::UNAUTHORIZED, "invalid_grant");
    let grant = request
        .headers()
        .get(GRANT_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|g| internal.auth.grants.verify(g, internal.clock.now()));
    let Some(grant) = grant else {
        return invalid_grant.into_response();
    };
    let user = match internal.users.find_by_id(grant.user).await {
        Ok(Some(user)) if user.tenant == grant.tenant => user,
        Ok(_) => return invalid_grant.into_response(),
        Err(_) => {
            tracing::warn!("loading the grant's user failed");
            return ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable").into_response();
        }
    };
    request.extensions_mut().insert(AgentCaller {
        user,
        chat_id: grant.chat_id,
    });
    next.run(request).await
}

fn internal(state: &AssistantState) -> Result<&super::Internal, ApiError> {
    state
        .internal()
        .ok_or(ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))
}

/// Query of `GET /internal/agent/offers`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AgentOffersQuery {
    /// Title contains this text (1–100 characters).
    pub query: Option<String>,
    /// `accepted` or `needs_review`.
    pub match_status: Option<String>,
    /// 1–20 (default 10).
    #[param(minimum = 1, maximum = 20)]
    pub limit: Option<u32>,
}

/// One tracked offer.
#[derive(Debug, Serialize, ToSchema)]
pub struct AgentOfferResponse {
    /// Search it was found by.
    pub search_id: i64,
    /// When that search ran (epoch milliseconds).
    pub seen_at_ms: i64,
    /// The offer, with this tenant's match decisions applied.
    pub offer: SearchResultResponse,
}

/// The tenant's tracked offers.
#[derive(Debug, Serialize, ToSchema)]
pub struct AgentOffersResponse {
    /// Newest first.
    pub offers: Vec<AgentOfferResponse>,
    /// Offers hidden because they need a membership the user lacks.
    pub hidden: u32,
}

/// Tracked offers (the user's searches of the last 90 days). Read-only;
/// never starts a search.
#[utoipa::path(
    get,
    path = "/internal/agent/offers",
    operation_id = "agent_offers",
    tag = "agent-internal",
    security(("agent_service" = [], "agent_grant" = [])),
    params(AgentOffersQuery),
    responses(
        (status = OK, description = "Tracked offers", body = AgentOffersResponse),
        (status = BAD_REQUEST, description = "Invalid query string", body = ErrorResponse),
        (status = UNAUTHORIZED, description = "`unauthenticated` or `invalid_grant`", body = ErrorResponse),
        (status = UNPROCESSABLE_ENTITY, description = "`invalid_query`, `invalid_match_status` or `invalid_limit`", body = ErrorResponse),
        (status = INTERNAL_SERVER_ERROR, description = "Storage failed", body = ErrorResponse),
        (status = SERVICE_UNAVAILABLE, description = "Not configured", body = ErrorResponse)
    )
)]
pub(crate) async fn agent_offers(
    State(state): State<AssistantState>,
    Extension(caller): Extension<AgentCaller>,
    query: Result<Query<AgentOffersQuery>, QueryRejection>,
) -> Result<Json<AgentOffersResponse>, ApiError> {
    let inner = internal(&state)?;
    let Query(q) = query.map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_query"))?;
    let text = match q.query.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(raw) => Some(
            SearchQuery::parse(raw)
                .map_err(|_| ApiError(StatusCode::UNPROCESSABLE_ENTITY, "invalid_query"))?,
        ),
    };
    let match_status = match q.match_status.as_deref() {
        None => None,
        Some(raw) => Some(MatchStatus::parse(raw).ok_or(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_match_status",
        ))?),
    };
    let limit = q.limit.unwrap_or(10);
    if !(1..=MAX_TRACKED_LIMIT).contains(&limit) {
        return Err(ApiError(StatusCode::UNPROCESSABLE_ENTITY, "invalid_limit"));
    }
    let view = inner
        .tracked
        .list(
            &caller.user.tenant,
            caller.user.memberships,
            &TrackedQuery {
                query: text,
                match_status,
            },
            limit,
        )
        .await
        .map_err(|_| {
            tracing::warn!("reading tracked offers failed");
            ApiError::INTERNAL
        })?;
    Ok(Json(AgentOffersResponse {
        offers: view
            .offers
            .into_iter()
            .map(|t| AgentOfferResponse {
                search_id: t.search_id.0,
                seen_at_ms: t.seen_at.0,
                offer: t.result.into(),
            })
            .collect(),
        hidden: view.hidden,
    }))
}

/// Price stats and daily history of a product.
#[derive(Debug, Serialize, ToSchema)]
pub struct AgentPriceHistoryResponse {
    /// 30/90/365-day low, high and average per listing.
    pub stats: PriceStatsResponse,
    /// Daily low/high per listing over the requested days.
    pub history: PriceHistoryResponse,
}

/// Price stats (30/90/365 days) and the daily series of a product as this
/// tenant sees it (its own match decisions applied).
#[utoipa::path(
    get,
    path = "/internal/agent/products/{asin}/price-history",
    operation_id = "agent_price_history",
    tag = "agent-internal",
    security(("agent_service" = [], "agent_grant" = [])),
    params(
        ("asin" = String, Path, description = "Amazon ASIN (10 letters or digits)"),
        PriceHistoryQuery
    ),
    responses(
        (status = OK, description = "Stats and series", body = AgentPriceHistoryResponse),
        (status = BAD_REQUEST, description = "Not a valid ASIN or query", body = ErrorResponse),
        (status = UNAUTHORIZED, description = "`unauthenticated` or `invalid_grant`", body = ErrorResponse),
        (status = NOT_FOUND, description = "`product_not_found`: the ASIN has no match and no stored name", body = ErrorResponse),
        (status = UNPROCESSABLE_ENTITY, description = "`days` outside 1–365", body = ErrorResponse),
        (status = INTERNAL_SERVER_ERROR, description = "Storage failed", body = ErrorResponse),
        (status = SERVICE_UNAVAILABLE, description = "Not configured", body = ErrorResponse)
    )
)]
pub(crate) async fn agent_price_history(
    State(state): State<AssistantState>,
    Extension(caller): Extension<AgentCaller>,
    Path(asin): Path<String>,
    query: Result<Query<PriceHistoryQuery>, QueryRejection>,
) -> Result<Json<AgentPriceHistoryResponse>, ApiError> {
    let inner = internal(&state)?;
    let asin = Asin::new(&asin).map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_asin"))?;
    let Query(query) = query.map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_query"))?;
    let days = query.days.unwrap_or(app::MAX_SERIES_DAYS);
    let user = &caller.user;
    let stats = inner
        .stats
        .for_product(&user.tenant, &asin, user.memberships)
        .await
        .map_err(crate::products::stats_error)?;
    let history = inner
        .series
        .for_product(&user.tenant, &asin, days, user.memberships)
        .await
        .map_err(crate::products::series_error)?;
    Ok(Json(AgentPriceHistoryResponse {
        stats: stats.into(),
        history: history.into(),
    }))
}

/// ROI for up to 100 items (same calculation as `POST /roi`).
#[utoipa::path(
    post,
    path = "/internal/agent/roi",
    operation_id = "agent_roi",
    tag = "agent-internal",
    security(("agent_service" = [], "agent_grant" = [])),
    request_body = RoiRequest,
    responses(
        (status = OK, description = "One result per item", body = RoiResponse),
        (status = BAD_REQUEST, description = "Body is not JSON", body = ErrorResponse),
        (status = UNAUTHORIZED, description = "`unauthenticated` or `invalid_grant`", body = ErrorResponse),
        (status = UNPROCESSABLE_ENTITY, description = "`invalid_amount`, `empty` or `too_many`", body = ErrorResponse),
        (status = SERVICE_UNAVAILABLE, description = "Not configured", body = ErrorResponse)
    )
)]
pub(crate) async fn agent_roi(
    State(state): State<AssistantState>,
    Extension(_caller): Extension<AgentCaller>,
    body: Result<Json<RoiRequest>, JsonRejection>,
) -> Result<Json<RoiResponse>, ApiError> {
    internal(&state)?;
    crate::roi::compute(&json_body(body)?).map(Json)
}

/// Body of `POST /internal/agent/match-suggestions`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AgentSuggestionRequest {
    /// Retailer id.
    pub retailer: String,
    /// The retailer's product id.
    pub retailer_sku: String,
    /// Amazon product of the match under review.
    pub asin: String,
    /// `accept` or `reject`.
    pub decision: String,
    /// Why, 1–300 characters.
    pub reason: String,
}

/// Propose a verdict on a `needs_review` match in the user's recent
/// searches. Stores a suggestion only; the user confirms it in the app.
#[utoipa::path(
    post,
    path = "/internal/agent/match-suggestions",
    operation_id = "agent_suggest_match",
    tag = "agent-internal",
    security(("agent_service" = [], "agent_grant" = [])),
    request_body = AgentSuggestionRequest,
    responses(
        (status = CREATED, description = "Suggestion stored", body = MatchSuggestionResponse),
        (status = BAD_REQUEST, description = "Body is not JSON", body = ErrorResponse),
        (status = UNAUTHORIZED, description = "`unauthenticated` or `invalid_grant`", body = ErrorResponse),
        (status = CONFLICT, description = "`not_reviewable`", body = ErrorResponse),
        (status = UNPROCESSABLE_ENTITY, description = "`invalid_suggestion`", body = ErrorResponse),
        (status = INTERNAL_SERVER_ERROR, description = "Storage failed", body = ErrorResponse),
        (status = SERVICE_UNAVAILABLE, description = "Not configured", body = ErrorResponse)
    )
)]
pub(crate) async fn agent_suggest_match(
    State(state): State<AssistantState>,
    Extension(caller): Extension<AgentCaller>,
    body: Result<Json<AgentSuggestionRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<MatchSuggestionResponse>), ApiError> {
    let inner = internal(&state)?;
    let body = json_body(body)?;
    let invalid = ApiError(StatusCode::UNPROCESSABLE_ENTITY, "invalid_suggestion");
    let target = review_target(&body.retailer, &body.retailer_sku, &body.asin).ok_or(invalid)?;
    let decision = MatchDecision::parse(&body.decision).ok_or(invalid)?;
    let reason = SuggestionReason::parse(&body.reason).map_err(|_| invalid)?;
    let view = inner
        .suggest
        .suggest(
            &caller.user.tenant,
            caller.user.memberships,
            caller.chat_id,
            target,
            decision,
            reason,
        )
        .await
        .map_err(review_error)?;
    Ok((StatusCode::CREATED, Json(view.into())))
}

/// Body of `POST /internal/agent/usage`: tokens of one finished turn.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AgentUsageRequest {
    /// Prompt tokens.
    pub input_tokens: u64,
    /// Generated tokens.
    pub output_tokens: u64,
}

/// Tokens left today.
#[derive(Debug, Serialize, ToSchema)]
pub struct AgentUsageResponse {
    /// Tokens the tenant may still use today (0 when spent).
    pub remaining_tokens: u64,
}

/// Add a turn's tokens to the tenant's daily total; answers what is left
/// (authoritative; the sidecar stops at 0).
#[utoipa::path(
    post,
    path = "/internal/agent/usage",
    operation_id = "agent_usage",
    tag = "agent-internal",
    security(("agent_service" = [], "agent_grant" = [])),
    request_body = AgentUsageRequest,
    responses(
        (status = OK, description = "Recorded", body = AgentUsageResponse),
        (status = BAD_REQUEST, description = "Body is not JSON", body = ErrorResponse),
        (status = UNAUTHORIZED, description = "`unauthenticated` or `invalid_grant`", body = ErrorResponse),
        (status = UNPROCESSABLE_ENTITY, description = "`invalid_usage`", body = ErrorResponse),
        (status = INTERNAL_SERVER_ERROR, description = "Storage failed", body = ErrorResponse),
        (status = SERVICE_UNAVAILABLE, description = "Not configured", body = ErrorResponse)
    )
)]
pub(crate) async fn agent_usage(
    State(state): State<AssistantState>,
    Extension(caller): Extension<AgentCaller>,
    body: Result<Json<AgentUsageRequest>, JsonRejection>,
) -> Result<Json<AgentUsageResponse>, ApiError> {
    let inner = internal(&state)?;
    let body = json_body(body)?;
    if body.input_tokens > MAX_USAGE_REPORT || body.output_tokens > MAX_USAGE_REPORT {
        return Err(ApiError(StatusCode::UNPROCESSABLE_ENTITY, "invalid_usage"));
    }
    let remaining = inner
        .usage
        .record(
            &caller.user.tenant,
            caller.user.plan,
            TokenUsage {
                input: TokenCount::new(body.input_tokens),
                output: TokenCount::new(body.output_tokens),
            },
        )
        .await
        .map_err(|_| {
            tracing::warn!("recording assistant usage failed");
            ApiError::INTERNAL
        })?;
    Ok(Json(AgentUsageResponse {
        remaining_tokens: remaining.get(),
    }))
}
