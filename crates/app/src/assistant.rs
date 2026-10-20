//! Use cases for the AI assistant (ADR 0013): chat with a daily token
//! budget, read-only tools over the tenant's tracked products, assistant
//! match suggestions and the user's per-tenant match decisions.
//!
//! The engine's output is untrusted (ADR 0004): replies are cleaned, the
//! tenant always comes from the caller (never from tool arguments), and
//! suggestions only become decisions when the user confirms them.

use std::collections::BTreeMap;
use std::sync::Arc;

use domain::{
    Asin, ChatHistory, DAY_MS, MatchDecision, MatchStatus, Memberships, Plan, RetailerId,
    RetailerSku, SearchQuery, SuggestionReason, TenantId, TokenCount, TokenUsage, UnixMillis,
    clean_agent_reply,
};

use crate::ports::{
    AgentUsageRepo, AssistantEngine, ChatId, Clock, EngineError, EngineRequest, MatchDecisionRepo,
    MatchRepo, NewDecision, NewSuggestion, RecentResultsFilter, RepoError, SearchRepo,
    SearchResultView, StoredDecision, SuggestionId, SuggestionView, TrackedResultView, UserRecord,
};

/// How far back "tracked products" reach: listings in the tenant's own
/// searches from the last 90 days.
pub const TRACKED_DAYS: i64 = 90;
/// Most rows one tracked-offers call returns.
pub const MAX_TRACKED_LIMIT: u32 = 20;
/// Rows scanned before overlay and filtering.
const TRACKED_SCAN: u32 = 200;

/// Days since 1970-01-01 UTC.
#[must_use]
pub fn epoch_day(at: UnixMillis) -> i64 {
    at.0.div_euclid(DAY_MS)
}

fn tracked_since(now: UnixMillis) -> UnixMillis {
    UnixMillis(now.0.saturating_sub(TRACKED_DAYS * DAY_MS))
}

type Listing = (RetailerId, RetailerSku);

/// The tenant's decisions keyed by listing.
fn decision_map(decisions: Vec<StoredDecision>) -> BTreeMap<Listing, (Asin, MatchDecision)> {
    decisions
        .into_iter()
        .map(|d| ((d.retailer, d.retailer_sku), (d.asin, d.decision)))
        .collect()
}

/// Applies the tenant's decisions to a result: accept makes the match
/// `accepted`, reject removes it. A decision about another ASIN is ignored.
pub(crate) fn overlay_result(
    result: &mut SearchResultView,
    decisions: &BTreeMap<Listing, (Asin, MatchDecision)>,
) {
    let key = (result.retailer.clone(), result.retailer_sku.clone());
    let Some((asin, decision)) = decisions.get(&key) else {
        return;
    };
    if result.amazon.as_ref().is_none_or(|m| &m.asin != asin) {
        return;
    }
    match decision {
        MatchDecision::Accept => {
            if let Some(m) = result.amazon.as_mut() {
                m.status = MatchStatus::Accepted;
            }
        }
        MatchDecision::Reject => result.amazon = None,
    }
}

/// Applies the tenant's decisions to many results.
pub(crate) async fn overlay_results(
    decisions: &dyn MatchDecisionRepo,
    tenant: &TenantId,
    results: &mut [SearchResultView],
) -> Result<(), RepoError> {
    if results.is_empty() {
        return Ok(());
    }
    let listings: Vec<Listing> = results
        .iter()
        .map(|r| (r.retailer.clone(), r.retailer_sku.clone()))
        .collect();
    let map = decision_map(decisions.decisions_for_listings(tenant, &listings).await?);
    for r in results {
        overlay_result(r, &map);
    }
    Ok(())
}

/// Listings matched to `asin` as the tenant sees them: globally accepted
/// ones, minus the tenant's rejects, plus the tenant's accepts. Unsorted.
pub(crate) async fn tenant_listings(
    matches: &dyn MatchRepo,
    decisions: &dyn MatchDecisionRepo,
    tenant: &TenantId,
    asin: &Asin,
) -> Result<Vec<Listing>, RepoError> {
    let mut listings = matches.accepted_listings(asin).await?;
    let mine = decisions.decisions_for_asin(tenant, asin).await?;
    for d in mine.into_iter().filter(|d| &d.asin == asin) {
        let key = (d.retailer, d.retailer_sku);
        match d.decision {
            MatchDecision::Accept => {
                if !listings.contains(&key) {
                    listings.push(key);
                }
            }
            MatchDecision::Reject => listings.retain(|l| l != &key),
        }
    }
    Ok(listings)
}

/// Daily assistant token caps per plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenCaps {
    /// Cap for the free plan.
    pub free: TokenCount,
    /// Cap for the pro plan.
    pub pro: TokenCount,
}

impl TokenCaps {
    /// The cap for `plan`.
    #[must_use]
    pub const fn for_plan(self, plan: Plan) -> TokenCount {
        match plan {
            Plan::Free => self.free,
            Plan::Pro => self.pro,
        }
    }
}

/// Why a chat failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AssistantError {
    /// Today's token budget is used up.
    #[error("assistant token budget used up")]
    Budget,
    /// The tenant already has a chat in flight.
    #[error("assistant busy")]
    Busy,
    /// The engine took too long.
    #[error("assistant timed out")]
    Timeout,
    /// The engine failed.
    #[error("assistant failed")]
    Failed,
    /// No engine is configured or reachable.
    #[error("assistant unavailable")]
    Unavailable,
    /// Usage or suggestions could not be read.
    #[error("assistant storage unavailable")]
    Storage,
}

/// A finished chat turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatOutcome {
    /// Cleaned assistant reply.
    pub reply: String,
    /// Why the run ended.
    pub stop: domain::AgentStop,
    /// Tokens the run used.
    pub usage: TokenUsage,
    /// Suggestions stored during this chat (read from storage, never from
    /// model output).
    pub suggestions: Vec<SuggestionView>,
}

/// Runs a chat through the engine within the tenant's daily token budget.
pub struct AssistantChat {
    engine: Arc<dyn AssistantEngine>,
    usage: Arc<dyn AgentUsageRepo>,
    decisions: Arc<dyn MatchDecisionRepo>,
    clock: Arc<dyn Clock>,
    caps: TokenCaps,
}

impl AssistantChat {
    /// Chats through `engine`, budgeting with `usage` and `caps`.
    #[must_use]
    pub fn new(
        engine: Arc<dyn AssistantEngine>,
        usage: Arc<dyn AgentUsageRepo>,
        decisions: Arc<dyn MatchDecisionRepo>,
        clock: Arc<dyn Clock>,
        caps: TokenCaps,
    ) -> Self {
        Self {
            engine,
            usage,
            decisions,
            clock,
            caps,
        }
    }

    /// Answers the last message of `history` for `user`.
    ///
    /// # Errors
    /// [`AssistantError::Budget`] before calling the engine when the
    /// budget is used up; engine and storage failures otherwise.
    pub async fn chat(
        &self,
        user: &UserRecord,
        chat_id: ChatId,
        history: ChatHistory,
    ) -> Result<ChatOutcome, AssistantError> {
        let day = epoch_day(self.clock.now());
        let used = self
            .usage
            .used_on(&user.tenant, day)
            .await
            .map_err(|_| AssistantError::Storage)?;
        let remaining = self.caps.for_plan(user.plan).saturating_sub(used.total());
        if remaining == TokenCount::ZERO {
            return Err(AssistantError::Budget);
        }
        let request = EngineRequest {
            chat_id,
            user: user.id,
            tenant: user.tenant.clone(),
            locale: user.locale,
            history,
            token_budget: remaining,
        };
        let reply = self.engine.chat(&request).await.map_err(|e| match e {
            EngineError::Unavailable => AssistantError::Unavailable,
            EngineError::Busy => AssistantError::Busy,
            EngineError::Timeout => AssistantError::Timeout,
            EngineError::Failed => AssistantError::Failed,
        })?;
        let suggestions = self
            .decisions
            .suggestions_for_chat(&user.tenant, &request.chat_id)
            .await
            .map_err(|_| AssistantError::Storage)?;
        Ok(ChatOutcome {
            reply: clean_agent_reply(&reply.reply),
            stop: reply.stop,
            usage: reply.usage,
            suggestions,
        })
    }
}

/// Storage failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("assistant storage unavailable")]
pub struct AssistantStorageError;

/// Records tokens used by a running chat and reports what is left.
pub struct RecordAgentUsage {
    usage: Arc<dyn AgentUsageRepo>,
    clock: Arc<dyn Clock>,
    caps: TokenCaps,
}

impl RecordAgentUsage {
    /// Records through `usage` against `caps`.
    #[must_use]
    pub fn new(usage: Arc<dyn AgentUsageRepo>, clock: Arc<dyn Clock>, caps: TokenCaps) -> Self {
        Self { usage, clock, caps }
    }

    /// Adds `used` to today's total and returns the tokens left (zero when
    /// the budget is spent).
    ///
    /// # Errors
    /// [`AssistantStorageError`] if usage cannot be written.
    pub async fn record(
        &self,
        tenant: &TenantId,
        plan: Plan,
        used: TokenUsage,
    ) -> Result<TokenCount, AssistantStorageError> {
        let day = epoch_day(self.clock.now());
        let total = self
            .usage
            .add(tenant, day, used)
            .await
            .map_err(|_| AssistantStorageError)?;
        Ok(self.caps.for_plan(plan).saturating_sub(total.total()))
    }
}

/// Filters for [`TrackedOffers::list`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackedQuery {
    /// Title contains this text.
    pub query: Option<SearchQuery>,
    /// Only results with this (overlaid) match status.
    pub match_status: Option<MatchStatus>,
}

/// Tracked offers as the tenant sees them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedOffersView {
    /// Matching rows, newest first.
    pub offers: Vec<TrackedResultView>,
    /// Rows hidden because they need a membership the reader lacks.
    pub hidden: u32,
}

/// Reads the tenant's tracked products (results of their own searches in
/// the last [`TRACKED_DAYS`] days). Never starts a search.
pub struct TrackedOffers {
    searches: Arc<dyn SearchRepo>,
    decisions: Arc<dyn MatchDecisionRepo>,
    clock: Arc<dyn Clock>,
}

impl TrackedOffers {
    /// Reads through `searches`, overlaying `decisions`.
    #[must_use]
    pub fn new(
        searches: Arc<dyn SearchRepo>,
        decisions: Arc<dyn MatchDecisionRepo>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            searches,
            decisions,
            clock,
        }
    }

    /// Up to `limit` (clamped to 1..=[`MAX_TRACKED_LIMIT`]) tracked offers
    /// of `tenant`, with the tenant's decisions applied and membership-only
    /// listings the reader lacks removed.
    ///
    /// # Errors
    /// [`AssistantStorageError`] if storage cannot be read.
    pub async fn list(
        &self,
        tenant: &TenantId,
        memberships: Memberships,
        query: &TrackedQuery,
        limit: u32,
    ) -> Result<TrackedOffersView, AssistantStorageError> {
        let limit = usize::try_from(limit.clamp(1, MAX_TRACKED_LIMIT)).unwrap_or(1);
        let filter = RecentResultsFilter {
            title_contains: query.query.clone(),
            listing: None,
        };
        let since = tracked_since(self.clock.now());
        let mut rows = self
            .searches
            .recent_results(tenant, since, &filter, TRACKED_SCAN)
            .await
            .map_err(|_| AssistantStorageError)?;
        let before = rows.len();
        rows.retain(|t| memberships.allows(t.result.membership_required, &t.result.retailer));
        let hidden = u32::try_from(before - rows.len()).unwrap_or(u32::MAX);
        let mut results: Vec<SearchResultView> = rows.iter().map(|t| t.result.clone()).collect();
        overlay_results(self.decisions.as_ref(), tenant, &mut results)
            .await
            .map_err(|_| AssistantStorageError)?;
        let offers = rows
            .into_iter()
            .zip(results)
            .map(|(t, result)| TrackedResultView { result, ..t })
            .filter(|t| match query.match_status {
                None => true,
                Some(s) => t.result.amazon.as_ref().is_some_and(|m| m.status == s),
            })
            .take(limit)
            .collect();
        Ok(TrackedOffersView { offers, hidden })
    }
}

/// Why a suggestion or decision was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MatchReviewError {
    /// The listing is not a `needs_review` match to that ASIN in the
    /// tenant's recent, visible results.
    #[error("listing is not under review for this tenant")]
    NotReviewable,
    /// The suggestion is unknown, another tenant's, or about another
    /// listing.
    #[error("suggestion does not match")]
    SuggestionMismatch,
    /// Storage could not be read or written.
    #[error("storage unavailable")]
    Storage,
}

/// The listing and ASIN a review is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewTarget {
    /// Retailer of the listing.
    pub retailer: RetailerId,
    /// The retailer's product id.
    pub retailer_sku: RetailerSku,
    /// Amazon product of the match under review.
    pub asin: Asin,
}

/// The tenant's stored result for `target` if it is a visible
/// `needs_review` match to `target.asin` (as stored, before decisions).
async fn reviewable(
    searches: &dyn SearchRepo,
    now: UnixMillis,
    tenant: &TenantId,
    memberships: Memberships,
    target: &ReviewTarget,
) -> Result<SearchResultView, MatchReviewError> {
    let filter = RecentResultsFilter {
        title_contains: None,
        listing: Some((target.retailer.clone(), target.retailer_sku.clone())),
    };
    let rows = searches
        .recent_results(tenant, tracked_since(now), &filter, 1)
        .await
        .map_err(|_| MatchReviewError::Storage)?;
    rows.into_iter()
        .map(|t| t.result)
        .find(|r| {
            r.retailer == target.retailer
                && r.retailer_sku == target.retailer_sku
                && memberships.allows(r.membership_required, &r.retailer)
                && r.amazon
                    .as_ref()
                    .is_some_and(|m| m.asin == target.asin && m.status == MatchStatus::NeedsReview)
        })
        .ok_or(MatchReviewError::NotReviewable)
}

/// Stores an assistant suggestion for a `needs_review` match. Never
/// changes any match.
pub struct SuggestMatch {
    searches: Arc<dyn SearchRepo>,
    decisions: Arc<dyn MatchDecisionRepo>,
    clock: Arc<dyn Clock>,
}

impl SuggestMatch {
    /// Validates against `searches`, stores in `decisions`.
    #[must_use]
    pub fn new(
        searches: Arc<dyn SearchRepo>,
        decisions: Arc<dyn MatchDecisionRepo>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            searches,
            decisions,
            clock,
        }
    }

    /// Stores the suggestion if `target` is under review for `tenant`.
    ///
    /// # Errors
    /// [`MatchReviewError::NotReviewable`] or
    /// [`MatchReviewError::Storage`].
    pub async fn suggest(
        &self,
        tenant: &TenantId,
        memberships: Memberships,
        chat_id: ChatId,
        target: ReviewTarget,
        decision: MatchDecision,
        reason: SuggestionReason,
    ) -> Result<SuggestionView, MatchReviewError> {
        let now = self.clock.now();
        let result = reviewable(self.searches.as_ref(), now, tenant, memberships, &target).await?;
        let new = NewSuggestion {
            tenant: tenant.clone(),
            chat_id,
            retailer: target.retailer,
            retailer_sku: target.retailer_sku,
            asin: target.asin,
            title: result.title,
            decision,
            reason,
            created_at: now,
        };
        let id = self
            .decisions
            .create_suggestion(&new)
            .await
            .map_err(|_| MatchReviewError::Storage)?;
        Ok(SuggestionView {
            id,
            chat_id: new.chat_id,
            retailer: new.retailer,
            retailer_sku: new.retailer_sku,
            asin: new.asin,
            title: new.title,
            decision: new.decision,
            reason: new.reason.as_str().to_owned(),
            created_at: now,
        })
    }
}

/// Records a user's verdict on a `needs_review` match for their tenant.
pub struct DecideMatch {
    searches: Arc<dyn SearchRepo>,
    decisions: Arc<dyn MatchDecisionRepo>,
    clock: Arc<dyn Clock>,
}

impl DecideMatch {
    /// Validates against `searches`, stores in `decisions`.
    #[must_use]
    pub fn new(
        searches: Arc<dyn SearchRepo>,
        decisions: Arc<dyn MatchDecisionRepo>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            searches,
            decisions,
            clock,
        }
    }

    /// Stores `user`'s verdict. When `suggestion` is given it must be the
    /// tenant's suggestion about the same listing and ASIN.
    ///
    /// # Errors
    /// [`MatchReviewError`] if the listing is not under review, the
    /// suggestion does not match, or storage fails.
    pub async fn decide(
        &self,
        user: &UserRecord,
        suggestion: Option<SuggestionId>,
        target: ReviewTarget,
        decision: MatchDecision,
    ) -> Result<(), MatchReviewError> {
        if let Some(id) = suggestion {
            let s = self
                .decisions
                .suggestion(&user.tenant, id)
                .await
                .map_err(|_| MatchReviewError::Storage)?
                .ok_or(MatchReviewError::SuggestionMismatch)?;
            if s.retailer != target.retailer
                || s.retailer_sku != target.retailer_sku
                || s.asin != target.asin
            {
                return Err(MatchReviewError::SuggestionMismatch);
            }
        }
        let now = self.clock.now();
        reviewable(
            self.searches.as_ref(),
            now,
            &user.tenant,
            user.memberships,
            &target,
        )
        .await?;
        self.decisions
            .decide(&NewDecision {
                tenant: user.tenant.clone(),
                retailer: target.retailer,
                retailer_sku: target.retailer_sku,
                asin: target.asin,
                decision,
                decided_by: user.id,
                suggestion_id: suggestion,
                decided_at: now,
            })
            .await
            .map_err(|_| MatchReviewError::Storage)
    }
}

/// In-memory fakes shared by the app tests.
#[cfg(test)]
pub(crate) mod fakes {
    use std::sync::Mutex;

    use super::*;
    use crate::ports::PortFuture;

    /// Decisions and suggestions per tenant.
    #[derive(Default)]
    pub(crate) struct FakeDecisions {
        pub(crate) decisions: Mutex<Vec<(TenantId, NewDecision)>>,
        pub(crate) suggestions: Mutex<Vec<NewSuggestion>>,
        pub(crate) fail: bool,
    }

    impl FakeDecisions {
        pub(crate) fn with(tenant: &TenantId, decisions: Vec<StoredDecision>) -> Self {
            let me = Self::default();
            for d in decisions {
                me.decisions.lock().unwrap().push((
                    tenant.clone(),
                    NewDecision {
                        tenant: tenant.clone(),
                        retailer: d.retailer,
                        retailer_sku: d.retailer_sku,
                        asin: d.asin,
                        decision: d.decision,
                        decided_by: domain::UserId::new(1).unwrap(),
                        suggestion_id: None,
                        decided_at: UnixMillis(0),
                    },
                ));
            }
            me
        }

        fn stored(&self, tenant: &TenantId) -> Vec<StoredDecision> {
            let mut v: Vec<StoredDecision> = self
                .decisions
                .lock()
                .unwrap()
                .iter()
                .filter(|(t, _)| t == tenant)
                .map(|(_, d)| StoredDecision {
                    retailer: d.retailer.clone(),
                    retailer_sku: d.retailer_sku.clone(),
                    asin: d.asin.clone(),
                    decision: d.decision,
                })
                .collect();
            v.sort();
            v
        }

        fn view(i: usize, s: &NewSuggestion) -> SuggestionView {
            SuggestionView {
                id: SuggestionId(i64::try_from(i + 1).unwrap()),
                chat_id: s.chat_id.clone(),
                retailer: s.retailer.clone(),
                retailer_sku: s.retailer_sku.clone(),
                asin: s.asin.clone(),
                title: s.title.clone(),
                decision: s.decision,
                reason: s.reason.as_str().to_owned(),
                created_at: s.created_at,
            }
        }
    }

    impl MatchDecisionRepo for FakeDecisions {
        fn create_suggestion<'a>(
            &'a self,
            s: &'a NewSuggestion,
        ) -> PortFuture<'a, Result<SuggestionId, RepoError>> {
            Box::pin(async move {
                if self.fail {
                    return Err(RepoError::Unavailable);
                }
                let mut all = self.suggestions.lock().unwrap();
                all.push(s.clone());
                Ok(SuggestionId(i64::try_from(all.len()).unwrap()))
            })
        }

        fn suggestions_for_chat<'a>(
            &'a self,
            tenant: &'a TenantId,
            chat_id: &'a ChatId,
        ) -> PortFuture<'a, Result<Vec<SuggestionView>, RepoError>> {
            Box::pin(async move {
                if self.fail {
                    return Err(RepoError::Unavailable);
                }
                Ok(self
                    .suggestions
                    .lock()
                    .unwrap()
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| &s.tenant == tenant && &s.chat_id == chat_id)
                    .map(|(i, s)| Self::view(i, s))
                    .collect())
            })
        }

        fn suggestion<'a>(
            &'a self,
            tenant: &'a TenantId,
            id: SuggestionId,
        ) -> PortFuture<'a, Result<Option<SuggestionView>, RepoError>> {
            Box::pin(async move {
                let all = self.suggestions.lock().unwrap();
                let i = usize::try_from(id.0 - 1).ok();
                Ok(i.and_then(|i| all.get(i).map(|s| (i, s)))
                    .filter(|(_, s)| &s.tenant == tenant)
                    .map(|(i, s)| Self::view(i, s)))
            })
        }

        fn decide<'a>(&'a self, d: &'a NewDecision) -> PortFuture<'a, Result<(), RepoError>> {
            Box::pin(async move {
                if self.fail {
                    return Err(RepoError::Unavailable);
                }
                let mut all = self.decisions.lock().unwrap();
                all.retain(|(t, x)| {
                    !(t == &d.tenant
                        && x.retailer == d.retailer
                        && x.retailer_sku == d.retailer_sku)
                });
                all.push((d.tenant.clone(), d.clone()));
                Ok(())
            })
        }

        fn decisions_for_asin<'a>(
            &'a self,
            tenant: &'a TenantId,
            asin: &'a Asin,
        ) -> PortFuture<'a, Result<Vec<StoredDecision>, RepoError>> {
            Box::pin(async move {
                if self.fail {
                    return Err(RepoError::Unavailable);
                }
                Ok(self
                    .stored(tenant)
                    .into_iter()
                    .filter(|d| &d.asin == asin)
                    .collect())
            })
        }

        fn decisions_for_listings<'a>(
            &'a self,
            tenant: &'a TenantId,
            listings: &'a [(RetailerId, RetailerSku)],
        ) -> PortFuture<'a, Result<Vec<StoredDecision>, RepoError>> {
            Box::pin(async move {
                if self.fail {
                    return Err(RepoError::Unavailable);
                }
                Ok(self
                    .stored(tenant)
                    .into_iter()
                    .filter(|d| listings.contains(&(d.retailer.clone(), d.retailer_sku.clone())))
                    .collect())
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use domain::{AgentStop, Availability, ChatRole, Confidence, Email, Locale, PriceMxn, UserId};

    use super::fakes::FakeDecisions;
    use super::*;
    use crate::ports::{EngineReply, PortFuture, ResultMatchView, SearchId, SearchView};

    const NOW: i64 = 1_760_000_000_000;

    struct FixedClock;

    impl Clock for FixedClock {
        fn now(&self) -> UnixMillis {
            UnixMillis(NOW)
        }
    }

    fn tenant(n: &str) -> TenantId {
        TenantId::new(n).unwrap()
    }

    fn asin() -> Asin {
        Asin::new("B0E2ETEST1").unwrap()
    }

    fn user(plan: Plan) -> UserRecord {
        UserRecord {
            id: UserId::new(7).unwrap(),
            email: Email::parse("a@example.mx").unwrap(),
            tenant: tenant("user-7"),
            plan,
            memberships: Memberships::default(),
            onboarded: true,
            locale: Locale::Es,
        }
    }

    fn result(
        retailer: &str,
        sku: &str,
        status: Option<MatchStatus>,
        membership: bool,
    ) -> SearchResultView {
        SearchResultView {
            retailer: RetailerId::new(retailer).unwrap(),
            retailer_sku: RetailerSku::new(sku).unwrap(),
            title: format!("Taladro {sku}"),
            url: "https://example.mx/p".to_owned(),
            price_mxn: PriceMxn::from_centavos(100_000).unwrap(),
            availability: Availability::InStock,
            membership_required: membership,
            amazon: status.map(|status| ResultMatchView {
                asin: asin(),
                status,
                confidence: Confidence::from_bp(6_000).unwrap(),
                amazon_price_mxn: None,
                fees_mxn: None,
                profit_centavos: None,
                roi_bp: None,
                margin_bp: None,
            }),
        }
    }

    /// Recent results per tenant; records the arguments it saw.
    #[derive(Default)]
    struct FakeSearches {
        rows: Vec<(TenantId, TrackedResultView)>,
        seen: Mutex<Vec<(UnixMillis, RecentResultsFilter, u32)>>,
    }

    impl FakeSearches {
        fn with(t: &TenantId, results: Vec<SearchResultView>) -> Self {
            Self {
                rows: results
                    .into_iter()
                    .map(|result| {
                        (
                            t.clone(),
                            TrackedResultView {
                                search_id: SearchId(1),
                                seen_at: UnixMillis(NOW - DAY_MS),
                                result,
                            },
                        )
                    })
                    .collect(),
                seen: Mutex::default(),
            }
        }
    }

    impl SearchRepo for FakeSearches {
        fn create_and_enqueue<'a>(
            &'a self,
            _: &'a TenantId,
            _: &'a SearchQuery,
        ) -> PortFuture<'a, Result<SearchId, RepoError>> {
            Box::pin(async { Err(RepoError::Unavailable) })
        }

        fn load<'a>(
            &'a self,
            _: &'a TenantId,
            _: SearchId,
        ) -> PortFuture<'a, Result<Option<SearchView>, RepoError>> {
            Box::pin(async { Ok(None) })
        }

        fn recent_results<'a>(
            &'a self,
            t: &'a TenantId,
            since: UnixMillis,
            filter: &'a RecentResultsFilter,
            limit: u32,
        ) -> PortFuture<'a, Result<Vec<TrackedResultView>, RepoError>> {
            Box::pin(async move {
                self.seen
                    .lock()
                    .unwrap()
                    .push((since, filter.clone(), limit));
                Ok(self
                    .rows
                    .iter()
                    .filter(|(o, _)| o == t)
                    .map(|(_, r)| r.clone())
                    .filter(|r| {
                        filter.listing.as_ref().is_none_or(|(ret, sku)| {
                            &r.result.retailer == ret && &r.result.retailer_sku == sku
                        })
                    })
                    .take(usize::try_from(limit).unwrap())
                    .collect())
            })
        }
    }

    #[derive(Default)]
    struct FakeUsage(Mutex<BTreeMap<(TenantId, i64), TokenUsage>>);

    impl AgentUsageRepo for FakeUsage {
        fn used_on<'a>(
            &'a self,
            t: &'a TenantId,
            day: i64,
        ) -> PortFuture<'a, Result<TokenUsage, RepoError>> {
            Box::pin(async move {
                Ok(self
                    .0
                    .lock()
                    .unwrap()
                    .get(&(t.clone(), day))
                    .copied()
                    .unwrap_or_default())
            })
        }

        fn add<'a>(
            &'a self,
            t: &'a TenantId,
            day: i64,
            usage: TokenUsage,
        ) -> PortFuture<'a, Result<TokenUsage, RepoError>> {
            Box::pin(async move {
                let mut map = self.0.lock().unwrap();
                let e = map.entry((t.clone(), day)).or_default();
                e.input = e.input.saturating_add(usage.input);
                e.output = e.output.saturating_add(usage.output);
                Ok(*e)
            })
        }
    }

    /// Engine that returns a fixed answer and records requests.
    struct FakeEngine {
        answer: Result<EngineReply, EngineError>,
        seen: Mutex<Vec<EngineRequest>>,
    }

    impl AssistantEngine for FakeEngine {
        fn chat<'a>(
            &'a self,
            request: &'a EngineRequest,
        ) -> PortFuture<'a, Result<EngineReply, EngineError>> {
            Box::pin(async move {
                self.seen.lock().unwrap().push(request.clone());
                self.answer.clone()
            })
        }
    }

    const CAPS: TokenCaps = TokenCaps {
        free: TokenCount::new(1_000),
        pro: TokenCount::new(10_000),
    };

    fn usage(i: u64, o: u64) -> TokenUsage {
        TokenUsage {
            input: TokenCount::new(i),
            output: TokenCount::new(o),
        }
    }

    fn chat_id() -> ChatId {
        ChatId::parse("chat-0001").unwrap()
    }

    fn history() -> ChatHistory {
        ChatHistory::parse([(ChatRole::User, "¿Qué compro para Buen Fin?")]).unwrap()
    }

    #[tokio::test]
    async fn chat_passes_remaining_budget_and_attaches_stored_suggestions() {
        let u = user(Plan::Free);
        let used = Arc::new(FakeUsage::default());
        used.add(&u.tenant, epoch_day(UnixMillis(NOW)), usage(300, 100))
            .await
            .unwrap();
        let decisions = Arc::new(FakeDecisions::default());
        decisions.suggestions.lock().unwrap().push(NewSuggestion {
            tenant: u.tenant.clone(),
            chat_id: chat_id(),
            retailer: RetailerId::new("walmart_mx").unwrap(),
            retailer_sku: RetailerSku::new("1").unwrap(),
            asin: asin(),
            title: "Taladro".to_owned(),
            decision: MatchDecision::Accept,
            reason: SuggestionReason::parse("mismo modelo").unwrap(),
            created_at: UnixMillis(NOW),
        });
        // Same chat id, other tenant: never attached.
        let mut foreign = decisions.suggestions.lock().unwrap()[0].clone();
        foreign.tenant = tenant("user-8");
        decisions.suggestions.lock().unwrap().push(foreign);
        let engine = Arc::new(FakeEngine {
            answer: Ok(EngineReply {
                reply: " Hola\u{0} ".to_owned(),
                stop: AgentStop::Done,
                usage: usage(50, 20),
            }),
            seen: Mutex::default(),
        });
        let uc = AssistantChat::new(engine.clone(), used, decisions, Arc::new(FixedClock), CAPS);
        let out = uc.chat(&u, chat_id(), history()).await.unwrap();
        assert_eq!(out.reply, "Hola");
        assert_eq!(out.stop, AgentStop::Done);
        assert_eq!(out.suggestions.len(), 1);
        let seen = engine.seen.lock().unwrap();
        assert_eq!(seen[0].token_budget, TokenCount::new(600));
        assert_eq!(seen[0].tenant, u.tenant);
    }

    #[tokio::test]
    async fn spent_budget_refuses_before_the_engine_and_engine_errors_map() {
        let u = user(Plan::Free);
        let used = Arc::new(FakeUsage::default());
        used.add(&u.tenant, epoch_day(UnixMillis(NOW)), usage(900, 100))
            .await
            .unwrap();
        let engine = Arc::new(FakeEngine {
            answer: Err(EngineError::Busy),
            seen: Mutex::default(),
        });
        let make = |e: Arc<FakeEngine>, u: Arc<FakeUsage>| {
            AssistantChat::new(
                e,
                u,
                Arc::new(FakeDecisions::default()),
                Arc::new(FixedClock),
                CAPS,
            )
        };
        let uc = make(engine.clone(), used.clone());
        assert_eq!(
            uc.chat(&u, chat_id(), history()).await,
            Err(AssistantError::Budget)
        );
        assert!(engine.seen.lock().unwrap().is_empty());
        // Pro has a higher cap, so the engine runs and its error maps.
        let pro = UserRecord {
            plan: Plan::Pro,
            ..u.clone()
        };
        assert_eq!(
            uc.chat(&pro, chat_id(), history()).await,
            Err(AssistantError::Busy)
        );
        for (e, want) in [
            (EngineError::Unavailable, AssistantError::Unavailable),
            (EngineError::Timeout, AssistantError::Timeout),
            (EngineError::Failed, AssistantError::Failed),
        ] {
            let engine = Arc::new(FakeEngine {
                answer: Err(e),
                seen: Mutex::default(),
            });
            let uc = make(engine, Arc::new(FakeUsage::default()));
            assert_eq!(uc.chat(&u, chat_id(), history()).await, Err(want));
        }
    }

    #[tokio::test]
    async fn record_usage_returns_what_is_left() {
        let rec = RecordAgentUsage::new(Arc::new(FakeUsage::default()), Arc::new(FixedClock), CAPS);
        let t = tenant("user-7");
        assert_eq!(
            rec.record(&t, Plan::Free, usage(400, 100)).await,
            Ok(TokenCount::new(500))
        );
        assert_eq!(
            rec.record(&t, Plan::Free, usage(600, 0)).await,
            Ok(TokenCount::ZERO)
        );
        assert_eq!(
            rec.record(&tenant("user-8"), Plan::Pro, usage(1, 0)).await,
            Ok(TokenCount::new(9_999))
        );
    }

    #[tokio::test]
    async fn tracked_offers_overlay_hide_and_filter() {
        let me = tenant("user-7");
        let searches = Arc::new(FakeSearches::with(
            &me,
            vec![
                result("walmart_mx", "1", Some(MatchStatus::NeedsReview), false),
                result("walmart_mx", "2", Some(MatchStatus::NeedsReview), false),
                result("costco_mx", "3", Some(MatchStatus::Accepted), true),
                result("liverpool", "4", None, false),
            ],
        ));
        let decisions = Arc::new(FakeDecisions::with(
            &me,
            vec![StoredDecision {
                retailer: RetailerId::new("walmart_mx").unwrap(),
                retailer_sku: RetailerSku::new("1").unwrap(),
                asin: asin(),
                decision: MatchDecision::Accept,
            }],
        ));
        let uc = TrackedOffers::new(searches.clone(), decisions, Arc::new(FixedClock));
        let all = uc
            .list(&me, Memberships::default(), &TrackedQuery::default(), 50)
            .await
            .unwrap();
        assert_eq!(all.hidden, 1);
        assert_eq!(all.offers.len(), 3);
        assert_eq!(
            all.offers[0].result.amazon.as_ref().unwrap().status,
            MatchStatus::Accepted
        );
        let review = uc
            .list(
                &me,
                Memberships::default(),
                &TrackedQuery {
                    query: None,
                    match_status: Some(MatchStatus::NeedsReview),
                },
                1,
            )
            .await
            .unwrap();
        let skus: Vec<&str> = review
            .offers
            .iter()
            .map(|t| t.result.retailer_sku.as_str())
            .collect();
        assert_eq!(skus, ["2"]);
        let (since, _, _) = searches.seen.lock().unwrap()[0].clone();
        assert_eq!(since, UnixMillis(NOW - 90 * DAY_MS));
        // Another tenant sees nothing of mine.
        let other = uc
            .list(
                &tenant("user-8"),
                Memberships::default(),
                &TrackedQuery::default(),
                20,
            )
            .await
            .unwrap();
        assert!(other.offers.is_empty());
    }

    fn target(sku: &str) -> ReviewTarget {
        ReviewTarget {
            retailer: RetailerId::new("walmart_mx").unwrap(),
            retailer_sku: RetailerSku::new(sku).unwrap(),
            asin: asin(),
        }
    }

    #[tokio::test]
    async fn suggestions_need_a_visible_needs_review_result_of_the_tenant() {
        let me = tenant("user-7");
        let searches = Arc::new(FakeSearches::with(
            &me,
            vec![
                result("walmart_mx", "1", Some(MatchStatus::NeedsReview), false),
                result("walmart_mx", "2", Some(MatchStatus::Accepted), false),
            ],
        ));
        let decisions = Arc::new(FakeDecisions::default());
        let uc = SuggestMatch::new(searches, decisions.clone(), Arc::new(FixedClock));
        let reason = || SuggestionReason::parse("mismo modelo y marca").unwrap();
        let s = uc
            .suggest(
                &me,
                Memberships::default(),
                chat_id(),
                target("1"),
                MatchDecision::Accept,
                reason(),
            )
            .await
            .unwrap();
        assert_eq!(s.title, "Taladro 1");
        assert_eq!(s.id, SuggestionId(1));
        for (t, sku) in [
            (me.clone(), "2"),
            (me.clone(), "9"),
            (tenant("user-8"), "1"),
        ] {
            assert_eq!(
                uc.suggest(
                    &t,
                    Memberships::default(),
                    chat_id(),
                    target(sku),
                    MatchDecision::Reject,
                    reason()
                )
                .await,
                Err(MatchReviewError::NotReviewable),
                "{t:?} {sku}"
            );
        }
        let mut wrong_asin = target("1");
        wrong_asin.asin = Asin::new("B000000001").unwrap();
        assert_eq!(
            uc.suggest(
                &me,
                Memberships::default(),
                chat_id(),
                wrong_asin,
                MatchDecision::Accept,
                reason()
            )
            .await,
            Err(MatchReviewError::NotReviewable)
        );
        assert_eq!(decisions.suggestions.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn decisions_record_who_decided_and_check_the_suggestion() {
        let u = user(Plan::Free);
        let searches = Arc::new(FakeSearches::with(
            &u.tenant,
            vec![
                result("walmart_mx", "1", Some(MatchStatus::NeedsReview), false),
                result("walmart_mx", "2", Some(MatchStatus::NeedsReview), false),
            ],
        ));
        let decisions = Arc::new(FakeDecisions::default());
        let suggest = SuggestMatch::new(searches.clone(), decisions.clone(), Arc::new(FixedClock));
        let s = suggest
            .suggest(
                &u.tenant,
                u.memberships,
                chat_id(),
                target("1"),
                MatchDecision::Accept,
                SuggestionReason::parse("mismo modelo").unwrap(),
            )
            .await
            .unwrap();
        let uc = DecideMatch::new(searches, decisions.clone(), Arc::new(FixedClock));
        uc.decide(&u, Some(s.id), target("1"), MatchDecision::Accept)
            .await
            .unwrap();
        {
            let stored = decisions.decisions.lock().unwrap();
            let (_, d) = &stored[0];
            assert_eq!(d.decided_by, u.id);
            assert_eq!(d.suggestion_id, Some(s.id));
            assert_eq!(d.decided_at, UnixMillis(NOW));
        }
        // Wrong listing for that suggestion, unknown suggestion, another
        // tenant's suggestion.
        assert_eq!(
            uc.decide(&u, Some(s.id), target("2"), MatchDecision::Accept)
                .await,
            Err(MatchReviewError::SuggestionMismatch)
        );
        assert_eq!(
            uc.decide(
                &u,
                Some(SuggestionId(99)),
                target("1"),
                MatchDecision::Accept
            )
            .await,
            Err(MatchReviewError::SuggestionMismatch)
        );
        let other = UserRecord {
            id: UserId::new(8).unwrap(),
            tenant: tenant("user-8"),
            ..u.clone()
        };
        assert_eq!(
            uc.decide(&other, Some(s.id), target("1"), MatchDecision::Accept)
                .await,
            Err(MatchReviewError::SuggestionMismatch)
        );
        assert_eq!(
            uc.decide(&other, None, target("1"), MatchDecision::Accept)
                .await,
            Err(MatchReviewError::NotReviewable)
        );
        // Changing your mind replaces the decision.
        uc.decide(&u, None, target("1"), MatchDecision::Reject)
            .await
            .unwrap();
        let stored = decisions.decisions.lock().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].1.decision, MatchDecision::Reject);
        assert_eq!(stored[0].1.suggestion_id, None);
    }

    #[test]
    fn overlay_ignores_decisions_about_another_asin() {
        let mut r = result("walmart_mx", "1", Some(MatchStatus::NeedsReview), false);
        let mut map = BTreeMap::new();
        map.insert(
            (r.retailer.clone(), r.retailer_sku.clone()),
            (Asin::new("B000000001").unwrap(), MatchDecision::Reject),
        );
        overlay_result(&mut r, &map);
        assert!(r.amazon.is_some());
        map.insert(
            (r.retailer.clone(), r.retailer_sku.clone()),
            (asin(), MatchDecision::Reject),
        );
        overlay_result(&mut r, &map);
        assert!(r.amazon.is_none());
    }

    #[test]
    fn chat_ids_are_validated() {
        assert!(ChatId::parse("0123abcd-ef").is_some());
        assert!(ChatId::parse("short").is_none());
        assert!(ChatId::parse("bad id!!").is_none());
        assert!(ChatId::parse(&"a".repeat(65)).is_none());
        assert_eq!(epoch_day(UnixMillis(DAY_MS - 1)), 0);
        assert_eq!(epoch_day(UnixMillis(-1)), -1);
    }
}
