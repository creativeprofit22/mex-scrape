//! Assistant tables (migration `0009_assistant.sql`): daily token usage
//! per tenant, match suggestions made by the assistant and the user's
//! match decisions, plus the [`app::AgentUsageRepo`] and
//! [`app::MatchDecisionRepo`] adapters.
//!
//! Every query is keyed by tenant. Nothing here writes `offer_matches`:
//! a decision only overlays the deciding tenant's own views (ADR 0013).

use app::{
    AgentUsageRepo, ChatId, MatchDecisionRepo, NewDecision, NewSuggestion, PortFuture, RepoError,
    StoredDecision, SuggestionId, SuggestionView,
};
use domain::{
    Asin, MatchDecision, RetailerId, RetailerSku, TenantId, TokenCount, TokenUsage, UnixMillis,
};
use sqlx::Row as _;
use sqlx::postgres::{PgPool, PgRow};

/// Tokens stored for one tenant and day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UsageRow {
    /// Prompt tokens.
    pub input_tokens: i64,
    /// Generated tokens.
    pub output_tokens: i64,
}

/// `epoch_day` (days since 1970-01-01 UTC) as an `INTEGER` for SQL date
/// arithmetic; `None` outside the `DATE` range Postgres accepts here.
fn day_offset(epoch_day: i64) -> Option<i32> {
    i32::try_from(epoch_day).ok()
}

/// Tokens `tenant_id` used on `epoch_day`; zero when nothing was recorded.
///
/// # Errors
///
/// Database errors (including an out-of-range day); redact before logging.
pub async fn used_on(
    pool: &PgPool,
    tenant_id: &str,
    epoch_day: i64,
) -> Result<UsageRow, sqlx::Error> {
    let day = day_offset(epoch_day).ok_or(sqlx::Error::RowNotFound)?;
    let row = sqlx::query(
        "SELECT input_tokens, output_tokens FROM agent_token_usage \
         WHERE tenant_id = $1 AND day = DATE '1970-01-01' + $2::INTEGER",
    )
    .bind(tenant_id)
    .bind(day)
    .fetch_optional(pool)
    .await?;
    row.map_or(Ok(UsageRow::default()), |r| {
        Ok(UsageRow {
            input_tokens: r.try_get("input_tokens")?,
            output_tokens: r.try_get("output_tokens")?,
        })
    })
}

/// Atomically adds tokens to `tenant_id`'s total for `epoch_day` and
/// returns the new total. Concurrent adds never lose an update.
///
/// # Errors
///
/// Database errors (including negative amounts, refused by CHECK);
/// redact before logging.
pub async fn add_usage(
    pool: &PgPool,
    tenant_id: &str,
    epoch_day: i64,
    input_tokens: i64,
    output_tokens: i64,
) -> Result<UsageRow, sqlx::Error> {
    let day = day_offset(epoch_day).ok_or(sqlx::Error::RowNotFound)?;
    let row = sqlx::query(
        "INSERT INTO agent_token_usage (tenant_id, day, input_tokens, output_tokens) \
         VALUES ($1, DATE '1970-01-01' + $2::INTEGER, $3, $4) \
         ON CONFLICT (tenant_id, day) DO UPDATE SET \
           input_tokens = agent_token_usage.input_tokens + EXCLUDED.input_tokens, \
           output_tokens = agent_token_usage.output_tokens + EXCLUDED.output_tokens \
         RETURNING input_tokens, output_tokens",
    )
    .bind(tenant_id)
    .bind(day)
    .bind(input_tokens)
    .bind(output_tokens)
    .fetch_one(pool)
    .await?;
    Ok(UsageRow {
        input_tokens: row.try_get("input_tokens")?,
        output_tokens: row.try_get("output_tokens")?,
    })
}

/// A stored suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestionRow {
    /// Suggestion id.
    pub id: i64,
    /// Chat that produced it.
    pub chat_id: String,
    /// Retailer id.
    pub retailer: String,
    /// Retailer SKU.
    pub retailer_sku: String,
    /// Amazon product.
    pub asin: String,
    /// Listing title.
    pub title: String,
    /// `accept` or `reject`.
    pub decision: String,
    /// Why.
    pub reason: String,
    /// Creation time (epoch ms).
    pub created_at_ms: i64,
}

macro_rules! suggestion_columns {
    () => {
        "id, chat_id, retailer, retailer_sku, asin, title, decision, reason, \
         (EXTRACT(EPOCH FROM created_at) * 1000)::BIGINT AS created_at_ms"
    };
}

fn suggestion_row(r: &PgRow) -> Result<SuggestionRow, sqlx::Error> {
    Ok(SuggestionRow {
        id: r.try_get("id")?,
        chat_id: r.try_get("chat_id")?,
        retailer: r.try_get("retailer")?,
        retailer_sku: r.try_get("retailer_sku")?,
        asin: r.try_get("asin")?,
        title: r.try_get("title")?,
        decision: r.try_get("decision")?,
        reason: r.try_get("reason")?,
        created_at_ms: r.try_get("created_at_ms")?,
    })
}

/// Maps a stored suggestion to the app view; `None` if it breaks the
/// schema's invariants (callers fail closed).
#[must_use]
pub fn suggestion_view(r: SuggestionRow) -> Option<SuggestionView> {
    Some(SuggestionView {
        id: SuggestionId(r.id),
        chat_id: ChatId::parse(&r.chat_id)?,
        retailer: RetailerId::new(&r.retailer).ok()?,
        retailer_sku: RetailerSku::new(&r.retailer_sku).ok()?,
        asin: Asin::new(&r.asin).ok()?,
        title: r.title,
        decision: MatchDecision::parse(&r.decision)?,
        reason: r.reason,
        created_at: UnixMillis(r.created_at_ms),
    })
}

/// A stored decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionRow {
    /// Retailer id.
    pub retailer: String,
    /// Retailer SKU.
    pub retailer_sku: String,
    /// Amazon product.
    pub asin: String,
    /// `accept` or `reject`.
    pub decision: String,
}

fn decision_row(r: &PgRow) -> Result<DecisionRow, sqlx::Error> {
    Ok(DecisionRow {
        retailer: r.try_get("retailer")?,
        retailer_sku: r.try_get("retailer_sku")?,
        asin: r.try_get("asin")?,
        decision: r.try_get("decision")?,
    })
}

/// Maps a stored decision to the app view; `None` if it breaks the
/// schema's invariants (callers fail closed).
#[must_use]
pub fn decision_view(r: DecisionRow) -> Option<StoredDecision> {
    Some(StoredDecision {
        retailer: RetailerId::new(&r.retailer).ok()?,
        retailer_sku: RetailerSku::new(&r.retailer_sku).ok()?,
        asin: Asin::new(&r.asin).ok()?,
        decision: MatchDecision::parse(&r.decision)?,
    })
}

/// Stores a suggestion and returns its id.
///
/// # Errors
///
/// Database errors (including CHECK violations); redact before logging.
pub async fn create_suggestion(pool: &PgPool, s: &NewSuggestion) -> Result<i64, sqlx::Error> {
    sqlx::query(
        "INSERT INTO match_suggestions \
           (tenant_id, chat_id, retailer, retailer_sku, asin, title, decision, reason, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, to_timestamp($9 / 1000.0)) \
         RETURNING id",
    )
    .bind(s.tenant.as_str())
    .bind(s.chat_id.as_str())
    .bind(s.retailer.as_str())
    .bind(s.retailer_sku.as_str())
    .bind(s.asin.as_str())
    .bind(&s.title)
    .bind(s.decision.as_str())
    .bind(s.reason.as_str())
    .bind(s.created_at.0)
    .fetch_one(pool)
    .await?
    .try_get("id")
}

/// `tenant_id`'s suggestions from `chat_id`, oldest first.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn suggestions_for_chat(
    pool: &PgPool,
    tenant_id: &str,
    chat_id: &str,
) -> Result<Vec<SuggestionRow>, sqlx::Error> {
    sqlx::query(concat!(
        "SELECT ",
        suggestion_columns!(),
        " FROM match_suggestions WHERE tenant_id = $1 AND chat_id = $2 ORDER BY id"
    ))
    .bind(tenant_id)
    .bind(chat_id)
    .fetch_all(pool)
    .await?
    .iter()
    .map(suggestion_row)
    .collect()
}

/// `tenant_id`'s suggestion `id`; `None` if unknown or another tenant's.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn suggestion(
    pool: &PgPool,
    tenant_id: &str,
    id: i64,
) -> Result<Option<SuggestionRow>, sqlx::Error> {
    sqlx::query(concat!(
        "SELECT ",
        suggestion_columns!(),
        " FROM match_suggestions WHERE tenant_id = $1 AND id = $2"
    ))
    .bind(tenant_id)
    .bind(id)
    .fetch_optional(pool)
    .await?
    .as_ref()
    .map(suggestion_row)
    .transpose()
}

/// Inserts or replaces the tenant's verdict on the listing, recording who
/// decided, when and which suggestion (if any) it confirms.
///
/// # Errors
///
/// Database errors (including a suggestion of another tenant, refused by
/// the foreign key); redact before logging.
pub async fn decide(pool: &PgPool, d: &NewDecision) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO match_decisions \
           (tenant_id, retailer, retailer_sku, asin, decision, decided_by_user_id, \
            suggestion_id, decided_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, to_timestamp($8 / 1000.0)) \
         ON CONFLICT (tenant_id, retailer, retailer_sku) DO UPDATE SET \
           asin = EXCLUDED.asin, decision = EXCLUDED.decision, \
           decided_by_user_id = EXCLUDED.decided_by_user_id, \
           suggestion_id = EXCLUDED.suggestion_id, decided_at = EXCLUDED.decided_at",
    )
    .bind(d.tenant.as_str())
    .bind(d.retailer.as_str())
    .bind(d.retailer_sku.as_str())
    .bind(d.asin.as_str())
    .bind(d.decision.as_str())
    .bind(d.decided_by.get())
    .bind(d.suggestion_id.map(|s| s.0))
    .bind(d.decided_at.0)
    .execute(pool)
    .await?;
    Ok(())
}

/// `tenant_id`'s verdicts about `asin`, sorted by retailer then SKU.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn decisions_for_asin(
    pool: &PgPool,
    tenant_id: &str,
    asin: &str,
) -> Result<Vec<DecisionRow>, sqlx::Error> {
    sqlx::query(
        "SELECT retailer, retailer_sku, asin, decision FROM match_decisions \
         WHERE tenant_id = $1 AND asin = $2 \
         ORDER BY retailer COLLATE \"C\", retailer_sku COLLATE \"C\"",
    )
    .bind(tenant_id)
    .bind(asin)
    .fetch_all(pool)
    .await?
    .iter()
    .map(decision_row)
    .collect()
}

/// `tenant_id`'s verdicts on any of `listings` (`(retailer, sku)` pairs),
/// sorted by retailer then SKU.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn decisions_for_listings(
    pool: &PgPool,
    tenant_id: &str,
    listings: &[(&str, &str)],
) -> Result<Vec<DecisionRow>, sqlx::Error> {
    if listings.is_empty() {
        return Ok(Vec::new());
    }
    let retailers: Vec<&str> = listings.iter().map(|(r, _)| *r).collect();
    let skus: Vec<&str> = listings.iter().map(|(_, s)| *s).collect();
    sqlx::query(
        "SELECT d.retailer, d.retailer_sku, d.asin, d.decision FROM match_decisions d \
         JOIN UNNEST($2::TEXT[], $3::TEXT[]) AS l(retailer, retailer_sku) \
           ON d.retailer = l.retailer AND d.retailer_sku = l.retailer_sku \
         WHERE d.tenant_id = $1 \
         ORDER BY d.retailer COLLATE \"C\", d.retailer_sku COLLATE \"C\"",
    )
    .bind(tenant_id)
    .bind(retailers)
    .bind(skus)
    .fetch_all(pool)
    .await?
    .iter()
    .map(decision_row)
    .collect()
}

fn token_usage(row: UsageRow) -> Option<TokenUsage> {
    Some(TokenUsage {
        input: TokenCount::new(u64::try_from(row.input_tokens).ok()?),
        output: TokenCount::new(u64::try_from(row.output_tokens).ok()?),
    })
}

/// [`AgentUsageRepo`] over `agent_token_usage`. Errors become
/// [`RepoError::Unavailable`] without detail.
#[derive(Clone)]
pub struct PgAgentUsage {
    pool: PgPool,
}

impl PgAgentUsage {
    /// Usage in `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl AgentUsageRepo for PgAgentUsage {
    fn used_on<'a>(
        &'a self,
        tenant: &'a TenantId,
        epoch_day: i64,
    ) -> PortFuture<'a, Result<TokenUsage, RepoError>> {
        Box::pin(async move {
            let row = used_on(&self.pool, tenant.as_str(), epoch_day)
                .await
                .map_err(|_| RepoError::Unavailable)?;
            token_usage(row).ok_or(RepoError::Unavailable)
        })
    }

    fn add<'a>(
        &'a self,
        tenant: &'a TenantId,
        epoch_day: i64,
        usage: TokenUsage,
    ) -> PortFuture<'a, Result<TokenUsage, RepoError>> {
        Box::pin(async move {
            // Counts beyond i64 never happen; clamp rather than wrap.
            let input = i64::try_from(usage.input.get()).unwrap_or(i64::MAX);
            let output = i64::try_from(usage.output.get()).unwrap_or(i64::MAX);
            let row = add_usage(&self.pool, tenant.as_str(), epoch_day, input, output)
                .await
                .map_err(|_| RepoError::Unavailable)?;
            token_usage(row).ok_or(RepoError::Unavailable)
        })
    }
}

/// [`MatchDecisionRepo`] over `match_suggestions` and `match_decisions`.
/// Errors become [`RepoError::Unavailable`] without detail.
#[derive(Clone)]
pub struct PgMatchDecisions {
    pool: PgPool,
}

impl PgMatchDecisions {
    /// Suggestions and decisions in `pool`.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn decision_views(rows: Vec<DecisionRow>) -> Result<Vec<StoredDecision>, RepoError> {
    rows.into_iter()
        .map(|r| decision_view(r).ok_or(RepoError::Unavailable))
        .collect()
}

impl MatchDecisionRepo for PgMatchDecisions {
    fn create_suggestion<'a>(
        &'a self,
        s: &'a NewSuggestion,
    ) -> PortFuture<'a, Result<SuggestionId, RepoError>> {
        Box::pin(async move {
            create_suggestion(&self.pool, s)
                .await
                .map(SuggestionId)
                .map_err(|_| RepoError::Unavailable)
        })
    }

    fn suggestions_for_chat<'a>(
        &'a self,
        tenant: &'a TenantId,
        chat_id: &'a ChatId,
    ) -> PortFuture<'a, Result<Vec<SuggestionView>, RepoError>> {
        Box::pin(async move {
            suggestions_for_chat(&self.pool, tenant.as_str(), chat_id.as_str())
                .await
                .map_err(|_| RepoError::Unavailable)?
                .into_iter()
                .map(|r| suggestion_view(r).ok_or(RepoError::Unavailable))
                .collect()
        })
    }

    fn suggestion<'a>(
        &'a self,
        tenant: &'a TenantId,
        id: SuggestionId,
    ) -> PortFuture<'a, Result<Option<SuggestionView>, RepoError>> {
        Box::pin(async move {
            suggestion(&self.pool, tenant.as_str(), id.0)
                .await
                .map_err(|_| RepoError::Unavailable)?
                .map(|r| suggestion_view(r).ok_or(RepoError::Unavailable))
                .transpose()
        })
    }

    fn decide<'a>(&'a self, d: &'a NewDecision) -> PortFuture<'a, Result<(), RepoError>> {
        Box::pin(async move {
            decide(&self.pool, d)
                .await
                .map_err(|_| RepoError::Unavailable)
        })
    }

    fn decisions_for_asin<'a>(
        &'a self,
        tenant: &'a TenantId,
        asin: &'a Asin,
    ) -> PortFuture<'a, Result<Vec<StoredDecision>, RepoError>> {
        Box::pin(async move {
            let rows = decisions_for_asin(&self.pool, tenant.as_str(), asin.as_str())
                .await
                .map_err(|_| RepoError::Unavailable)?;
            decision_views(rows)
        })
    }

    fn decisions_for_listings<'a>(
        &'a self,
        tenant: &'a TenantId,
        listings: &'a [(RetailerId, RetailerSku)],
    ) -> PortFuture<'a, Result<Vec<StoredDecision>, RepoError>> {
        Box::pin(async move {
            let pairs: Vec<(&str, &str)> = listings
                .iter()
                .map(|(r, s)| (r.as_str(), s.as_str()))
                .collect();
            let rows = decisions_for_listings(&self.pool, tenant.as_str(), &pairs)
                .await
                .map_err(|_| RepoError::Unavailable)?;
            decision_views(rows)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_map_to_views_and_bad_rows_fail_closed() {
        let row = SuggestionRow {
            id: 3,
            chat_id: "chat-0001".to_owned(),
            retailer: "liverpool".to_owned(),
            retailer_sku: "1001".to_owned(),
            asin: "B0E2ETEST1".to_owned(),
            title: "Taladro".to_owned(),
            decision: "accept".to_owned(),
            reason: "Mismo modelo".to_owned(),
            created_at_ms: 5,
        };
        let view = suggestion_view(row.clone()).unwrap();
        assert_eq!(view.id, SuggestionId(3));
        assert_eq!(view.decision, MatchDecision::Accept);
        assert!(
            suggestion_view(SuggestionRow {
                decision: "maybe".to_owned(),
                ..row.clone()
            })
            .is_none()
        );
        assert!(
            suggestion_view(SuggestionRow {
                chat_id: "x".to_owned(),
                ..row
            })
            .is_none()
        );

        let d = DecisionRow {
            retailer: "liverpool".to_owned(),
            retailer_sku: "1001".to_owned(),
            asin: "B0E2ETEST1".to_owned(),
            decision: "reject".to_owned(),
        };
        assert_eq!(
            decision_view(d.clone()).unwrap().decision,
            MatchDecision::Reject
        );
        assert!(
            decision_view(DecisionRow {
                asin: "bad".to_owned(),
                ..d
            })
            .is_none()
        );
    }

    #[test]
    fn negative_usage_fails_closed_and_days_must_fit() {
        assert!(
            token_usage(UsageRow {
                input_tokens: -1,
                output_tokens: 0
            })
            .is_none()
        );
        assert_eq!(
            token_usage(UsageRow {
                input_tokens: 2,
                output_tokens: 3
            })
            .unwrap()
            .total(),
            TokenCount::new(5)
        );
        assert_eq!(day_offset(20_000), Some(20_000));
        assert_eq!(day_offset(i64::MAX), None);
    }
}
