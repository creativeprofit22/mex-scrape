//! Queues one demo-account search per catalog keyword group, so a new demo
//! user signs in to searches, matches and product histories already filled
//! by the demo worker. It goes through the API's start-search use case
//! (`app::StartSearch` over [`PgSearches`], i.e. [`crate::searches::create_search`]:
//! a `searches` row and a `search_jobs` message in one transaction).

use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::Context as _;
use app::{Clock, StartSearch};
use domain::{TenantId, UnixMillis};
use sqlx::{PgPool, Row as _};

use super::catalog::DemoCatalog;
use crate::searches::PgSearches;

/// A clock stuck at the seed's start, so every queued search shares it.
struct SeedClock(i64);

impl Clock for SeedClock {
    fn now(&self) -> UnixMillis {
        UnixMillis(self.0)
    }
}

/// What [`queue_group_searches`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueueReport {
    /// Groups queued now.
    pub queued: Vec<String>,
    /// Groups that already have a queued, running or finished search.
    pub kept: Vec<String>,
}

/// The groups without a search in `existing`, in catalog order.
#[must_use]
pub fn groups_to_queue<'a>(groups: &'a [String], existing: &BTreeSet<String>) -> Vec<&'a str> {
    groups
        .iter()
        .map(String::as_str)
        .filter(|g| !existing.contains(*g))
        .collect()
}

/// The tenant's search queries that are not `failed` or `refused` (those
/// are queued again, e.g. after the kill switch refused them).
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn existing_queries(
    pool: &PgPool,
    tenant: &str,
) -> Result<BTreeSet<String>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT DISTINCT query FROM searches \
         WHERE tenant_id = $1 AND status NOT IN ('failed', 'refused')",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await?;
    rows.iter().map(|r| r.try_get("query")).collect()
}

/// Queues a search for every catalog group the tenant has no live search
/// for; a rerun queues nothing while the earlier searches exist.
///
/// # Errors
///
/// Database errors or an invalid tenant or group; redact before logging.
pub async fn queue_group_searches(
    pool: &PgPool,
    tenant: &str,
    catalog: &DemoCatalog,
    now_ms: i64,
) -> anyhow::Result<QueueReport> {
    let tenant_id = TenantId::new(tenant).map_err(|e| anyhow::anyhow!("demo tenant: {e}"))?;
    let existing = existing_queries(pool, tenant)
        .await
        .context("reading the demo searches")?;
    let start = StartSearch::new(Arc::new(PgSearches::new(
        pool.clone(),
        Arc::new(SeedClock(now_ms)),
    )));
    let mut report = QueueReport::default();
    for group in groups_to_queue(&catalog.groups, &existing) {
        start
            .start(&tenant_id, group)
            .await
            .with_context(|| format!("queueing the demo search {group}"))?;
        report.queued.push(group.to_owned());
    }
    report.kept = catalog
        .groups
        .iter()
        .filter(|g| existing.contains(*g))
        .cloned()
        .collect();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_groups_without_a_live_search_are_queued() {
        let groups: Vec<String> = ["cocina", "hogar", "despensa"].map(String::from).to_vec();
        assert_eq!(
            groups_to_queue(&groups, &BTreeSet::new()),
            ["cocina", "hogar", "despensa"]
        );
        let existing = BTreeSet::from(["hogar".to_owned(), "otra".to_owned()]);
        assert_eq!(groups_to_queue(&groups, &existing), ["cocina", "despensa"]);
        let all: BTreeSet<String> = groups.iter().cloned().collect();
        assert!(groups_to_queue(&groups, &all).is_empty());
    }
}
