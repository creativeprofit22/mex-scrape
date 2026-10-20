//! The AI assistant (ADR 0013): `POST /assistant/chat` proxies a chat to
//! the gg-agent sidecar, `POST /matches/decisions` records the user's
//! verdict on a `needs_review` match, and `/internal/agent/*` serves the
//! sidecar's tools (see [`internal`]).
//!
//! The sidecar is untrusted orchestration: it never writes anything but
//! suggestions, and the tenant always comes from the session (public
//! routes) or a signed grant (internal routes).

pub mod grant;
pub(crate) mod internal;
pub mod sidecar;

use std::sync::Arc;

use app::{
    AgentUsageRepo, AssistantChat, AssistantEngine, AssistantError, ChatId, Clock, DecideMatch,
    MatchDecisionRepo, MatchReviewError, PriceSeries, PriceStats, RecordAgentUsage, ReviewTarget,
    SearchRepo, SuggestMatch, SuggestionId, SuggestionView, TokenCaps, TrackedOffers, UserRepo,
};
use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use domain::{
    Asin, ChatError, ChatHistory, ChatRole, MAX_CHAT_TURNS, MatchDecision, RetailerId, RetailerSku,
};
use http_guard::Secret;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::{ApiError, Auth, ErrorResponse, json_body, signed_in};
use grant::GrantKey;
use sidecar::DisabledEngine;

/// Shortest accepted `AGENT_SERVICE_TOKEN`.
pub const MIN_SERVICE_TOKEN_LEN: usize = 32;

/// Shared secrets between the api and the sidecar. No `Debug`.
pub struct AgentAuth {
    /// Bearer token the sidecar sends on tool calls (and the api sends on
    /// chat calls).
    pub service_token: Secret,
    /// Key signing the per-chat tool grants.
    pub grants: Arc<GrantKey>,
}

/// Everything the assistant routes need.
pub struct AssistantDeps {
    /// The sidecar client; `None` makes chats answer 503.
    pub engine: Option<Arc<dyn AssistantEngine>>,
    /// Secrets for the internal tool routes; `None` makes them answer 503.
    pub agent: Option<AgentAuth>,
    /// Stored searches.
    pub searches: Arc<dyn SearchRepo>,
    /// Per-tenant suggestions and decisions.
    pub decisions: Arc<dyn MatchDecisionRepo>,
    /// Daily token usage.
    pub usage: Arc<dyn AgentUsageRepo>,
    /// Accounts (grants are re-checked against the stored user).
    pub users: Arc<dyn UserRepo>,
    /// Price stats with the tenant overlay.
    pub stats: Arc<PriceStats>,
    /// Price series with the tenant overlay.
    pub series: Arc<PriceSeries>,
    /// Wall clock.
    pub clock: Arc<dyn Clock>,
    /// Daily token caps per plan.
    pub caps: TokenCaps,
}

pub(crate) struct Internal {
    pub(crate) auth: AgentAuth,
    pub(crate) users: Arc<dyn UserRepo>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) usage: RecordAgentUsage,
    pub(crate) tracked: TrackedOffers,
    pub(crate) suggest: SuggestMatch,
    pub(crate) stats: Arc<PriceStats>,
    pub(crate) series: Arc<PriceSeries>,
}

struct Enabled {
    chat: AssistantChat,
    decide: DecideMatch,
    internal: Option<Internal>,
}

/// State of the assistant routes. Without it every route answers 503.
#[derive(Clone)]
pub struct AssistantState {
    inner: Option<Arc<Enabled>>,
}

impl AssistantState {
    /// Every assistant route answers 503 (no database).
    #[must_use]
    pub const fn disabled() -> Self {
        Self { inner: None }
    }

    /// Enabled over `deps`.
    #[must_use]
    pub fn new(deps: AssistantDeps) -> Self {
        let engine = deps
            .engine
            .unwrap_or_else(|| Arc::new(DisabledEngine) as Arc<dyn AssistantEngine>);
        let internal = deps.agent.map(|auth| Internal {
            auth,
            users: deps.users.clone(),
            clock: deps.clock.clone(),
            usage: RecordAgentUsage::new(deps.usage.clone(), deps.clock.clone(), deps.caps),
            tracked: TrackedOffers::new(
                deps.searches.clone(),
                deps.decisions.clone(),
                deps.clock.clone(),
            ),
            suggest: SuggestMatch::new(
                deps.searches.clone(),
                deps.decisions.clone(),
                deps.clock.clone(),
            ),
            stats: deps.stats.clone(),
            series: deps.series.clone(),
        });
        Self {
            inner: Some(Arc::new(Enabled {
                chat: AssistantChat::new(
                    engine,
                    deps.usage,
                    deps.decisions.clone(),
                    deps.clock.clone(),
                    deps.caps,
                ),
                decide: DecideMatch::new(deps.searches, deps.decisions, deps.clock),
                internal,
            })),
        }
    }

    fn enabled(&self) -> Result<&Enabled, ApiError> {
        self.inner
            .as_deref()
            .ok_or(ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))
    }

    pub(crate) fn internal(&self) -> Option<&Internal> {
        self.inner.as_deref().and_then(|e| e.internal.as_ref())
    }
}

/// One chat message.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ChatMessageRequest {
    /// `user` or `assistant`.
    pub role: String,
    /// Text, 1–2000 characters after trimming.
    pub content: String,
}

/// Body of `POST /assistant/chat`: the whole conversation, oldest first.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ChatRequest {
    /// 1–20 messages, at most 16 000 characters in total, the last from
    /// the user.
    pub messages: Vec<ChatMessageRequest>,
}

/// A verdict the assistant proposed; nothing changes until the user
/// confirms it with `POST /matches/decisions`.
#[derive(Debug, Serialize, ToSchema)]
pub struct MatchSuggestionResponse {
    /// Suggestion id (pass it as `suggestion_id` when confirming).
    pub id: i64,
    /// Retailer id.
    pub retailer: String,
    /// The retailer's product id.
    pub retailer_sku: String,
    /// Amazon product of the match under review.
    pub asin: String,
    /// Listing title (from the user's own search results).
    pub title: String,
    /// `accept` or `reject`.
    pub decision: String,
    /// Why, in the assistant's words (untrusted text).
    pub reason: String,
}

impl From<SuggestionView> for MatchSuggestionResponse {
    fn from(s: SuggestionView) -> Self {
        Self {
            id: s.id.0,
            retailer: s.retailer.as_str().to_owned(),
            retailer_sku: s.retailer_sku.as_str().to_owned(),
            asin: s.asin.as_str().to_owned(),
            title: s.title,
            decision: s.decision.as_str().to_owned(),
            reason: s.reason,
        }
    }
}

/// Tokens a run used.
#[derive(Debug, Serialize, ToSchema)]
pub struct TokenUsageResponse {
    /// Prompt tokens (including tool results).
    pub input_tokens: u64,
    /// Generated tokens.
    pub output_tokens: u64,
}

/// The assistant's answer.
#[derive(Debug, Serialize, ToSchema)]
pub struct ChatResponse {
    /// Id of this chat run.
    pub chat_id: String,
    /// Answer text (plain text; render it escaped).
    pub reply: String,
    /// `done`, `max_turns` or `token_budget`.
    pub stop: String,
    /// Match suggestions made during this run.
    pub suggestions: Vec<MatchSuggestionResponse>,
    /// Tokens used.
    pub usage: TokenUsageResponse,
}

/// A fresh chat id: 128 random bits as 32 hex digits.
fn new_chat_id() -> Option<ChatId> {
    let id = tower_sessions::session::Id::default().0;
    ChatId::parse(&format!("{id:032x}"))
}

fn chat_history(body: &ChatRequest) -> Result<ChatHistory, ApiError> {
    let invalid = ApiError(StatusCode::UNPROCESSABLE_ENTITY, "invalid_messages");
    // Count first, so a huge array is refused before any other work.
    if body.messages.len() > MAX_CHAT_TURNS {
        return Err(invalid);
    }
    let pairs = body
        .messages
        .iter()
        .map(|m| ChatRole::parse(&m.role).map(|r| (r, m.content.as_str())))
        .collect::<Option<Vec<_>>>()
        .ok_or(invalid)?;
    ChatHistory::parse(pairs).map_err(|_: ChatError| invalid)
}

/// Ask the assistant about the signed-in user's tracked products (their
/// searches of the last 90 days): prices, seasons (Hot Sale, Buen Fin) and
/// uncertain matches. Runs within the plan's daily token budget.
#[utoipa::path(
    post,
    path = "/assistant/chat",
    operation_id = "assistant_chat",
    tag = "assistant",
    security(("session_cookie" = [], "csrf_token" = [])),
    request_body = ChatRequest,
    responses(
        (status = OK, description = "The answer", body = ChatResponse),
        (status = BAD_REQUEST, description = "Body is not JSON", body = ErrorResponse),
        (status = UNAUTHORIZED, description = "Not signed in", body = ErrorResponse),
        (status = FORBIDDEN, description = "CSRF check failed", body = ErrorResponse),
        (status = CONFLICT, description = "`agent_busy`: a chat of this user is still running", body = ErrorResponse),
        (status = UNPROCESSABLE_ENTITY, description = "`invalid_messages`", body = ErrorResponse),
        (status = TOO_MANY_REQUESTS, description = "`agent_budget` (daily tokens used up) or plan rate limit", body = ErrorResponse),
        (status = INTERNAL_SERVER_ERROR, description = "Storage failed", body = ErrorResponse),
        (status = BAD_GATEWAY, description = "`agent_failed`", body = ErrorResponse),
        (status = SERVICE_UNAVAILABLE, description = "`agent_unavailable` or `unavailable`", body = ErrorResponse),
        (status = GATEWAY_TIMEOUT, description = "`agent_timeout`", body = ErrorResponse)
    )
)]
pub(crate) async fn assistant_chat(
    State(state): State<AssistantState>,
    auth: Auth,
    body: Result<Json<ChatRequest>, JsonRejection>,
) -> Result<Json<ChatResponse>, ApiError> {
    let inner = state.enabled()?;
    let user = signed_in(&auth)?;
    let body = json_body(body)?;
    let history = chat_history(&body)?;
    let chat_id = new_chat_id().ok_or(ApiError::INTERNAL)?;
    let out = inner
        .chat
        .chat(user, chat_id.clone(), history)
        .await
        .map_err(|e| match e {
            AssistantError::Budget => ApiError(StatusCode::TOO_MANY_REQUESTS, "agent_budget"),
            AssistantError::Busy => ApiError(StatusCode::CONFLICT, "agent_busy"),
            AssistantError::Timeout => ApiError(StatusCode::GATEWAY_TIMEOUT, "agent_timeout"),
            AssistantError::Failed => ApiError(StatusCode::BAD_GATEWAY, "agent_failed"),
            AssistantError::Unavailable => {
                ApiError(StatusCode::SERVICE_UNAVAILABLE, "agent_unavailable")
            }
            AssistantError::Storage => {
                tracing::warn!("assistant storage failed");
                ApiError::INTERNAL
            }
        })?;
    Ok(Json(ChatResponse {
        chat_id: chat_id.as_str().to_owned(),
        reply: out.reply,
        stop: out.stop.as_str().to_owned(),
        suggestions: out.suggestions.into_iter().map(Into::into).collect(),
        usage: TokenUsageResponse {
            input_tokens: out.usage.input.get(),
            output_tokens: out.usage.output.get(),
        },
    }))
}

/// Body of `POST /matches/decisions`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct MatchDecisionRequest {
    /// The assistant suggestion being confirmed, if any.
    pub suggestion_id: Option<i64>,
    /// Retailer id.
    pub retailer: String,
    /// The retailer's product id.
    pub retailer_sku: String,
    /// Amazon product of the match under review.
    pub asin: String,
    /// `accept` or `reject`.
    pub decision: String,
}

/// The stored verdict.
#[derive(Debug, Serialize, ToSchema)]
pub struct MatchDecisionResponse {
    /// Retailer id.
    pub retailer: String,
    /// The retailer's product id.
    pub retailer_sku: String,
    /// Amazon product.
    pub asin: String,
    /// `accept` or `reject`.
    pub decision: String,
}

/// Parses a listing and ASIN from untrusted input; `None` if any is invalid.
pub(crate) fn review_target(retailer: &str, sku: &str, asin: &str) -> Option<ReviewTarget> {
    Some(ReviewTarget {
        retailer: RetailerId::new(retailer).ok()?,
        retailer_sku: RetailerSku::new(sku).ok()?,
        asin: Asin::new(asin).ok()?,
    })
}

pub(crate) fn review_error(e: MatchReviewError) -> ApiError {
    match e {
        MatchReviewError::NotReviewable => ApiError(StatusCode::CONFLICT, "not_reviewable"),
        MatchReviewError::SuggestionMismatch => {
            ApiError(StatusCode::UNPROCESSABLE_ENTITY, "suggestion_mismatch")
        }
        MatchReviewError::Storage => {
            tracing::warn!("storing a match review failed");
            ApiError::INTERNAL
        }
    }
}

/// Accept or reject a `needs_review` match in one of the signed-in user's
/// recent searches. Only this user's tenant sees the change (search
/// results and price stats); the decision records who made it.
#[utoipa::path(
    post,
    path = "/matches/decisions",
    operation_id = "decide_match",
    tag = "assistant",
    security(("session_cookie" = [], "csrf_token" = [])),
    request_body = MatchDecisionRequest,
    responses(
        (status = OK, description = "Decision stored", body = MatchDecisionResponse),
        (status = BAD_REQUEST, description = "Body is not JSON", body = ErrorResponse),
        (status = UNAUTHORIZED, description = "Not signed in", body = ErrorResponse),
        (status = FORBIDDEN, description = "CSRF check failed", body = ErrorResponse),
        (status = CONFLICT, description = "`not_reviewable`: not a needs-review match in your recent searches", body = ErrorResponse),
        (status = UNPROCESSABLE_ENTITY, description = "`invalid_decision` or `suggestion_mismatch`", body = ErrorResponse),
        (status = TOO_MANY_REQUESTS, description = "Plan rate limit reached", body = ErrorResponse),
        (status = INTERNAL_SERVER_ERROR, description = "Storage failed", body = ErrorResponse),
        (status = SERVICE_UNAVAILABLE, description = "Not configured", body = ErrorResponse)
    )
)]
pub(crate) async fn decide_match(
    State(state): State<AssistantState>,
    auth: Auth,
    body: Result<Json<MatchDecisionRequest>, JsonRejection>,
) -> Result<Json<MatchDecisionResponse>, ApiError> {
    let inner = state.enabled()?;
    let user = signed_in(&auth)?;
    let body = json_body(body)?;
    let invalid = ApiError(StatusCode::UNPROCESSABLE_ENTITY, "invalid_decision");
    let target = review_target(&body.retailer, &body.retailer_sku, &body.asin).ok_or(invalid)?;
    let decision = MatchDecision::parse(&body.decision).ok_or(invalid)?;
    let suggestion = match body.suggestion_id {
        Some(id) if id > 0 => Some(SuggestionId(id)),
        Some(_) => return Err(invalid),
        None => None,
    };
    let response = MatchDecisionResponse {
        retailer: target.retailer.as_str().to_owned(),
        retailer_sku: target.retailer_sku.as_str().to_owned(),
        asin: target.asin.as_str().to_owned(),
        decision: decision.as_str().to_owned(),
    };
    inner
        .decide
        .decide(user, suggestion, target, decision)
        .await
        .map_err(review_error)?;
    Ok(Json(response))
}
