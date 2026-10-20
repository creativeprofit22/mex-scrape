//! Job messages, errors and the shared context of the three jobs:
//! [`search`] (queue `search_jobs`), [`refresh`] and [`category`] (queue
//! `refresh_jobs`). [`consumer`] reads the queues and applies retries and
//! dead-lettering (ADR 0011).

pub mod category;
pub mod consumer;
pub mod pages;
pub mod refresh;
pub mod search;

use std::sync::{Arc, Mutex};

use app::{GatedAmazonSource, MatchRepo, PriceHistoryRepo, RequestBudget};
use brightdata::Transport;
use brightdata::runs::{GateError, RunError, RunStore, Runner};
use db::PgPool;
use db::queue::Queue;
use domain::{RetailerSku, TenantId};
use http_guard::Redactor;
use matching::MatchConfig;
use retailers::ParseHealth;
use retailers::category::CategoryListing;
use serde_json::{Value, json};

use crate::amazon_source::TenantAmazon;
use crate::clock::JobClock;
use crate::config::WorkerConfig;

/// Most SKUs one `product_refresh` message may carry.
pub const MAX_REFRESH_SKUS: usize = 50;

/// A decoded queue message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobMessage {
    /// Run the stored search `search_id` (queue `search_jobs`).
    Search {
        /// `searches.id`.
        search_id: i64,
        /// Owner of the search.
        tenant: TenantId,
    },
    /// Refresh the prices of `skus` at `retailer` (queue `refresh_jobs`).
    ProductRefresh {
        /// Registered retailer id.
        retailer: &'static str,
        /// 1–[`MAX_REFRESH_SKUS`] SKUs.
        skus: Vec<RetailerSku>,
    },
    /// Browse `retailer`'s configured category listings (queue
    /// `refresh_jobs`).
    CategoryBrowse {
        /// A retailer with Browser category listings.
        retailer: &'static str,
    },
}

/// Why a message could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("malformed job message")]
pub struct MessageError;

fn retailer_id(value: Option<&Value>) -> Result<&'static str, MessageError> {
    let id = value.and_then(Value::as_str).ok_or(MessageError)?;
    retailers::adapter_for(id)
        .map(|a| a.info().id)
        .ok_or(MessageError)
}

impl JobMessage {
    /// Decodes a message body. Unknown kinds, extra or missing fields and
    /// invalid values are rejected.
    ///
    /// # Errors
    ///
    /// [`MessageError`].
    pub fn parse(json: &str) -> Result<Self, MessageError> {
        let value: Value = serde_json::from_str(json).map_err(|_| MessageError)?;
        let obj = value.as_object().ok_or(MessageError)?;
        let keys = |expected: &[&str]| {
            obj.len() == expected.len() && expected.iter().all(|k| obj.contains_key(*k))
        };
        match obj.get("kind").and_then(Value::as_str) {
            Some("search") if keys(&["kind", "search_id", "tenant_id"]) => {
                let search_id = obj
                    .get("search_id")
                    .and_then(Value::as_i64)
                    .filter(|id| *id > 0)
                    .ok_or(MessageError)?;
                let tenant = obj
                    .get("tenant_id")
                    .and_then(Value::as_str)
                    .and_then(|t| TenantId::new(t).ok())
                    .ok_or(MessageError)?;
                Ok(Self::Search { search_id, tenant })
            }
            Some("product_refresh") if keys(&["kind", "retailer", "skus"]) => {
                let retailer = retailer_id(obj.get("retailer"))?;
                let skus = obj
                    .get("skus")
                    .and_then(Value::as_array)
                    .filter(|s| (1..=MAX_REFRESH_SKUS).contains(&s.len()))
                    .ok_or(MessageError)?
                    .iter()
                    .map(|s| {
                        s.as_str()
                            .and_then(|s| RetailerSku::new(s).ok())
                            .ok_or(MessageError)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Self::ProductRefresh { retailer, skus })
            }
            Some("category_browse") if keys(&["kind", "retailer"]) => {
                let retailer = retailer_id(obj.get("retailer"))?;
                Ok(Self::CategoryBrowse { retailer })
            }
            _ => Err(MessageError),
        }
    }

    /// The JSON body, as stored in the queue.
    #[must_use]
    pub fn to_json(&self) -> String {
        match self {
            Self::Search { search_id, tenant } => {
                json!({"kind": "search", "search_id": search_id, "tenant_id": tenant.as_str()})
            }
            Self::ProductRefresh { retailer, skus } => json!({
                "kind": "product_refresh",
                "retailer": retailer,
                "skus": skus.iter().map(RetailerSku::as_str).collect::<Vec<_>>(),
            }),
            Self::CategoryBrowse { retailer } => {
                json!({"kind": "category_browse", "retailer": retailer})
            }
        }
        .to_string()
    }

    /// Stable kind name (`search`, `product_refresh`, `category_browse`).
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Search { .. } => "search",
            Self::ProductRefresh { .. } => "product_refresh",
            Self::CategoryBrowse { .. } => "category_browse",
        }
    }

    /// The queue this message belongs on.
    #[must_use]
    pub const fn queue(&self) -> Queue {
        match self {
            Self::Search { .. } => Queue::SearchJobs,
            Self::ProductRefresh { .. } | Self::CategoryBrowse { .. } => Queue::RefreshJobs,
        }
    }

    /// The tenant that pays for the job: the search owner, or `system`
    /// for scheduled jobs.
    #[must_use]
    pub fn tenant(&self) -> TenantId {
        match self {
            Self::Search { tenant, .. } => tenant.clone(),
            Self::ProductRefresh { .. } | Self::CategoryBrowse { .. } => TenantId::system(),
        }
    }
}

/// Why a job did not finish. Codes are fixed, secret-free strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JobError {
    /// May succeed later (storage, an active run): retried with backoff,
    /// dead-lettered after the attempt budget.
    #[error("transient job failure: {0}")]
    Transient(&'static str),
    /// Refused before any paid call (kill switch, connector disabled,
    /// missing key, a daily cap): finished as refused, never retried.
    #[error("job refused: {0}")]
    Refused(&'static str),
    /// Can never succeed (unknown search, invalid input): dead-lettered at
    /// once.
    #[error("permanent job failure: {0}")]
    Permanent(&'static str),
}

impl JobError {
    /// The fixed code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Transient(c) | Self::Refused(c) | Self::Permanent(c) => c,
        }
    }
}

/// Classifies a Bright Data run error: the one-active-run gate and
/// storage are transient; the other gates are refusals; anything else is
/// permanent.
#[must_use]
pub fn run_error(error: &RunError) -> JobError {
    match error {
        RunError::Gate(gate) => match gate {
            GateError::ActiveRun => JobError::Transient("active_run"),
            GateError::KillSwitch => JobError::Refused("kill_switch"),
            GateError::ConnectorDisabled => JobError::Refused("connector_disabled"),
            GateError::MissingKey => JobError::Refused("missing_key"),
            GateError::RunsPerDayCap => JobError::Refused("daily_run_cap"),
            GateError::ItemsPerRunCap => JobError::Refused("items_per_run_cap"),
            GateError::ItemsPerDayCap => JobError::Refused("daily_item_cap"),
        },
        RunError::Store(_) => JobError::Transient("storage"),
        RunError::Invalid(_) => JobError::Permanent("invalid_run"),
        RunError::NotFound | RunError::NotResumable(_) => JobError::Permanent("run_unavailable"),
    }
}

/// Everything a job needs. `amazon` gives the ungated source per paying
/// tenant (normally SP-API with a Bright Data fallback whose dataset runs
/// are charged to that tenant); jobs wrap it in a [`GatedAmazonSource`] for
/// the same tenant, so every Amazon call is checked against the kill switch
/// and the daily request cap first.
pub struct JobContext<'a, T, S, C> {
    /// Job tables and queues.
    pub pool: &'a PgPool,
    /// Gated Bright Data runs.
    pub runner: &'a Runner<'a, T, S, C>,
    /// The clock shared with `runner`.
    pub clock: &'a C,
    /// The ungated Amazon source, per paying tenant.
    pub amazon: Arc<dyn TenantAmazon>,
    /// The Amazon request budget.
    pub budget: Arc<dyn RequestBudget>,
    /// Price history writes.
    pub prices: &'a dyn PriceHistoryRepo,
    /// Match writes.
    pub matches: &'a dyn MatchRepo,
    /// Matching thresholds.
    pub match_config: &'a MatchConfig,
    /// Shared retailer parse health.
    pub health: &'a Mutex<ParseHealth>,
    /// Validated category listings.
    pub listings: &'a [CategoryListing],
    /// Worker settings.
    pub config: &'a WorkerConfig,
    /// Redacts secrets from logged errors.
    pub redactor: &'a Redactor,
}

impl<T: Transport, S: RunStore, C: JobClock> JobContext<'_, T, S, C> {
    /// The Amazon source as paid for by `tenant`.
    #[must_use]
    pub fn amazon_for(&self, tenant: &TenantId) -> GatedAmazonSource {
        GatedAmazonSource::new(
            self.amazon.for_tenant(tenant),
            Arc::clone(&self.budget),
            tenant.clone(),
        )
    }

    /// Now, in epoch ms.
    pub fn now_ms(&self) -> i64 {
        brightdata::runs::Clock::now_ms(self.clock)
    }

    /// Logs a storage error (redacted) and returns the transient error.
    pub fn storage(&self, error: &db::DbError) -> JobError {
        let message = self.redactor.sanitize(&error.to_string());
        tracing::warn!(error = %message, "job storage failed");
        JobError::Transient("storage")
    }
}

/// Dispatches decoded messages to the jobs.
pub struct Jobs<'a, T, S, C> {
    /// The shared context.
    pub ctx: JobContext<'a, T, S, C>,
}

impl<T: Transport, S: RunStore, C: JobClock> Jobs<'_, T, S, C> {
    /// Runs `message` (read as `msg_id` from `queue`).
    ///
    /// # Errors
    ///
    /// [`JobError`]; a message on the wrong queue is permanent.
    pub async fn handle(
        &self,
        queue: Queue,
        msg_id: i64,
        message: &JobMessage,
    ) -> Result<(), JobError> {
        if message.queue() != queue {
            return Err(JobError::Permanent("wrong_queue"));
        }
        match message {
            JobMessage::Search { search_id, tenant } => {
                search::run(&self.ctx, msg_id, *search_id, tenant).await
            }
            JobMessage::ProductRefresh { retailer, skus } => {
                refresh::run(&self.ctx, msg_id, retailer, skus).await
            }
            JobMessage::CategoryBrowse { retailer } => {
                category::run(&self.ctx, msg_id, retailer).await
            }
        }
    }

    /// Called once a message is dead-lettered or refused: a search is
    /// finished `failed`/`refused` with the code, so the API never shows a
    /// search stuck in `running`. Scheduled jobs need nothing.
    pub async fn give_up(&self, message: &JobMessage, error: JobError) {
        if let JobMessage::Search { search_id, .. } = message {
            search::give_up(&self.ctx, *search_id, error).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_and_name_their_queue() {
        let msgs = [
            JobMessage::Search {
                search_id: 7,
                tenant: TenantId::default_tenant(),
            },
            JobMessage::ProductRefresh {
                retailer: "walmart_mx",
                skus: vec![RetailerSku::new("123").unwrap()],
            },
            JobMessage::CategoryBrowse {
                retailer: "sears_mx",
            },
        ];
        for m in &msgs {
            assert_eq!(JobMessage::parse(&m.to_json()).unwrap(), *m);
        }
        assert_eq!(msgs[0].queue(), Queue::SearchJobs);
        assert_eq!(msgs[1].queue(), Queue::RefreshJobs);
        assert_eq!(msgs[2].tenant(), TenantId::system());
        assert_eq!(msgs[0].tenant(), TenantId::default_tenant());
    }

    #[test]
    fn malformed_messages_are_rejected() {
        for bad in [
            "",
            "[]",
            r#"{"kind":"nope"}"#,
            r#"{"kind":"search","search_id":0,"tenant_id":"default"}"#,
            r#"{"kind":"search","search_id":1,"tenant_id":"bad tenant"}"#,
            r#"{"kind":"search","search_id":1,"tenant_id":"default","x":1}"#,
            r#"{"kind":"product_refresh","retailer":"nope","skus":["1"]}"#,
            r#"{"kind":"product_refresh","retailer":"walmart_mx","skus":[]}"#,
            r#"{"kind":"product_refresh","retailer":"walmart_mx","skus":[""]}"#,
            r#"{"kind":"category_browse"}"#,
        ] {
            assert_eq!(JobMessage::parse(bad), Err(MessageError), "{bad}");
        }
        let too_many: Vec<String> = (0..=MAX_REFRESH_SKUS).map(|i| i.to_string()).collect();
        let json = json!({"kind":"product_refresh","retailer":"walmart_mx","skus":too_many});
        assert_eq!(JobMessage::parse(&json.to_string()), Err(MessageError));
    }

    #[test]
    fn gates_map_to_retry_or_refusal() {
        let gate = |g| run_error(&RunError::Gate(g));
        assert_eq!(
            gate(GateError::ActiveRun),
            JobError::Transient("active_run")
        );
        assert_eq!(
            gate(GateError::KillSwitch),
            JobError::Refused("kill_switch")
        );
        assert_eq!(
            gate(GateError::ConnectorDisabled),
            JobError::Refused("connector_disabled")
        );
        assert_eq!(
            gate(GateError::MissingKey),
            JobError::Refused("missing_key")
        );
        assert_eq!(
            gate(GateError::RunsPerDayCap),
            JobError::Refused("daily_run_cap")
        );
        assert_eq!(
            gate(GateError::ItemsPerRunCap),
            JobError::Refused("items_per_run_cap")
        );
        assert_eq!(
            gate(GateError::ItemsPerDayCap),
            JobError::Refused("daily_item_cap")
        );
        assert_eq!(
            run_error(&RunError::Store("x".into())),
            JobError::Transient("storage")
        );
        assert_eq!(
            run_error(&RunError::Invalid("x".into())),
            JobError::Permanent("invalid_run")
        );
    }
}
