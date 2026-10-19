//! The live client: Web Unlocker `POST /request` and Datasets v3 trigger,
//! progress, snapshot download and cancel (ported from linkgo
//! `brightdata/direct.rs`, async, over `http-guard`).

use http_guard::{
    DEFAULT_MAX_RESPONSE_BYTES, GuardError, GuardedClient, GuardedRequest, GuardedResponse,
    HostAllowlist, Redactor, SMALL_RESPONSE_BYTES, Secret, StatusCode, TransportPolicy, Url,
    header,
};
use serde::Serialize;
use serde_json::Value;

use crate::config::BrightDataConfig;
use crate::error::{BrightDataError, Failure};
use crate::ids::{CountryCode, DatasetId, DiscoverBy, SnapshotId, TargetUrl};
use crate::rate::{RateKey, ZoneLimiter};
use crate::retry::{Idempotency, RetryPolicy, parse_retry_after, with_retry};
use crate::transport::Transport;

/// The only host this client talks to.
pub const BRIGHTDATA_API_HOST: &str = "api.brightdata.com";
/// Fixed API base URL (not configurable; tests use a loopback base).
pub const BRIGHTDATA_API_BASE: &str = "https://api.brightdata.com";
/// Most inputs one dataset trigger may carry.
pub const MAX_TRIGGER_INPUTS: usize = 1000;
/// Cap for an unlocked page body.
const PAGE_RESPONSE_BYTES: u64 = DEFAULT_MAX_RESPONSE_BYTES;
/// Cap for a downloaded snapshot.
const SNAPSHOT_RESPONSE_BYTES: u64 = DEFAULT_MAX_RESPONSE_BYTES;
/// Bright Data's error-reason response header.
const BRD_ERROR_HEADER: &str = "x-brd-error";

/// One Web Unlocker fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnlockRequest {
    /// The page to fetch.
    pub url: TargetUrl,
    /// Exit country for geo-targeted content (e.g. [`CountryCode::MX`]).
    pub country: Option<CountryCode>,
}

/// A page returned by Web Unlocker (`format: raw`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnlockedPage {
    /// HTTP status of the Unlocker response (always 2xx).
    pub status: u16,
    /// The page body (invalid UTF-8 replaced), at most 8 MiB.
    pub body: String,
}

/// A Datasets v3 trigger.
#[derive(Debug, Clone, PartialEq)]
pub struct DatasetTrigger {
    /// The dataset (scraper) to run.
    pub dataset_id: DatasetId,
    /// Inputs, one JSON object each (e.g. `{"url": "..."}`); 1 to
    /// [`MAX_TRIGGER_INPUTS`].
    pub inputs: Vec<Value>,
    /// Discovery mode: sends `type=discover_new&discover_by=<value>`.
    pub discover_by: Option<DiscoverBy>,
    /// Max records per input (`limit_per_input`).
    pub limit_per_input: Option<u32>,
}

impl DatasetTrigger {
    /// Checks the inputs: 1 to [`MAX_TRIGGER_INPUTS`] JSON objects.
    ///
    /// # Errors
    ///
    /// [`BrightDataError::Terminal`] with fixed text.
    pub fn validate(&self) -> Result<(), BrightDataError> {
        if self.inputs.is_empty() || self.inputs.len() > MAX_TRIGGER_INPUTS {
            return Err(BrightDataError::Terminal(format!(
                "a dataset trigger needs 1 to {MAX_TRIGGER_INPUTS} inputs"
            )));
        }
        if !self.inputs.iter().all(Value::is_object) {
            return Err(BrightDataError::Terminal(
                "every dataset input must be a JSON object".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Collection state reported by `GET /datasets/v3/progress/{id}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotStatus {
    /// starting, running, building, collecting or digesting.
    Running,
    /// Results can be downloaded.
    Ready,
    /// Bright Data reported failure.
    Failed,
    /// Cancelled (either spelling).
    Cancelled,
}

/// Result of a snapshot download.
#[derive(Debug, Clone, PartialEq)]
pub enum Download {
    /// 202 or 409: the snapshot is still being built.
    NotReady,
    /// The records (`format=json`).
    Records(Vec<Value>),
}

#[derive(Serialize)]
struct UnlockBody<'a> {
    zone: &'a str,
    url: &'a str,
    format: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    country: Option<&'a str>,
}

/// Async Bright Data client. Holds the key and zone as [`Secret`]s (so it has
/// no `Debug`); both are redacted from every error.
pub struct BrightDataClient {
    unlocker: GuardedClient,
    datasets: GuardedClient,
    base: Url,
    api_key: Secret,
    zone: Secret,
    retry: RetryPolicy,
    limiter: ZoneLimiter,
}

impl BrightDataClient {
    /// Builds the production client for `https://api.brightdata.com`.
    ///
    /// # Errors
    ///
    /// [`BrightDataError::Terminal`] if the HTTP client cannot be created.
    pub fn new(config: BrightDataConfig) -> Result<Self, BrightDataError> {
        let base = Url::parse(BRIGHTDATA_API_BASE)
            .map_err(|_| BrightDataError::Terminal("invalid Bright Data base URL".to_owned()))?;
        Self::build(config, base, TransportPolicy::PUBLIC_HTTPS)
    }

    /// A client for a loopback fake server.
    #[cfg(test)]
    pub(crate) fn for_test(base: &str, config: BrightDataConfig) -> Self {
        let base = Url::parse(base).expect("test base URL");
        Self::build(config, base, TransportPolicy::LOOPBACK_TEST).expect("test client")
    }

    fn build(
        config: BrightDataConfig,
        base: Url,
        policy: TransportPolicy,
    ) -> Result<Self, BrightDataError> {
        let redactor = Redactor::new([&config.api_key, &config.unlocker_zone]);
        let client = |timeout| {
            let allowlist = HostAllowlist::new([BRIGHTDATA_API_HOST])?;
            GuardedClient::builder(allowlist)
                .policy(policy)
                .total_timeout(timeout)
                .redactor(redactor.clone())
                .build()
        };
        let build_error = |error: GuardError| {
            BrightDataError::Terminal(format!("Bright Data client could not be created: {error}"))
        };
        Ok(Self {
            unlocker: client(config.unlocker_timeout).map_err(build_error)?,
            datasets: client(config.datasets_timeout).map_err(build_error)?,
            base,
            api_key: config.api_key,
            zone: config.unlocker_zone,
            retry: config.retry,
            limiter: ZoneLimiter::new(config.rate),
        })
    }

    fn endpoint(&self, path: &str) -> Url {
        let mut url = self.base.clone();
        url.set_path(path);
        url
    }

    /// Waits for a rate-limit slot, adds the Bearer key and sends.
    async fn send(
        &self,
        key: RateKey,
        request: Result<GuardedRequest, GuardError>,
    ) -> Result<GuardedResponse, Failure> {
        let request = request
            .and_then(|request| request.bearer(&self.api_key))
            .map_err(|error| Failure::from_guard(&error))?;
        self.limiter.acquire(key).await;
        request
            .send()
            .await
            .map_err(|error| Failure::from_guard(&error))
    }

    /// Classifies a non-success response; the detail is redacted and
    /// URL-free (status, Bright Data's `x-brd-error` header and the body's
    /// `error`/`message` field or text).
    async fn failure_for(&self, response: GuardedResponse) -> Failure {
        let status = response.status();
        let header_text = |name| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        let retry_after = header_text(header::RETRY_AFTER.as_str())
            .as_deref()
            .and_then(parse_retry_after);
        let brd_error = header_text(BRD_ERROR_HEADER)
            .map(|value| response.sanitize(value.trim()))
            .filter(|value| !value.is_empty());
        let body = response
            .error_text(SMALL_RESPONSE_BYTES)
            .await
            .unwrap_or_default();
        let detail = error_detail(&body);
        let detail =
            match brd_error {
                Some(reason) if detail.is_empty() || detail.contains(&reason) => {
                    if detail.is_empty() { reason } else { detail }
                }
                Some(reason) => format!("{reason}; {detail}"),
                None => detail,
            };
        Failure::from_status(status, &self.datasets.sanitize(&detail), retry_after)
    }

    async fn ensure_success(&self, response: GuardedResponse) -> Result<GuardedResponse, Failure> {
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(self.failure_for(response).await)
        }
    }

    async fn unlock_once(&self, request: &UnlockRequest) -> Result<UnlockedPage, Failure> {
        let body = UnlockBody {
            zone: self.zone.expose_secret(),
            url: request.url.as_str(),
            format: "raw",
            country: request.country.as_ref().map(CountryCode::as_str),
        };
        let prepared = self
            .unlocker
            .post(&self.endpoint("/request"))
            .and_then(|prepared| prepared.json(&body));
        let response = self.send(RateKey::Unlocker, prepared).await?;
        let response = self.ensure_success(response).await?;
        let status = response.status().as_u16();
        let body = response
            .text(PAGE_RESPONSE_BYTES)
            .await
            .map_err(|error| Failure::from_guard(&error))?;
        Ok(UnlockedPage { status, body })
    }

    async fn trigger_once(&self, trigger: &DatasetTrigger) -> Result<SnapshotId, Failure> {
        let mut url = self.endpoint("/datasets/v3/trigger");
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("dataset_id", trigger.dataset_id.as_str());
            query.append_pair("include_errors", "true");
            if let Some(discover_by) = &trigger.discover_by {
                query.append_pair("type", "discover_new");
                query.append_pair("discover_by", discover_by.as_str());
            }
            if let Some(limit) = trigger.limit_per_input {
                query.append_pair("limit_per_input", &limit.to_string());
            }
        }
        let prepared = self
            .datasets
            .post(&url)
            .and_then(|prepared| prepared.json(&trigger.inputs));
        let response = self.send(RateKey::Datasets, prepared).await?;
        let response = self.ensure_success(response).await?;
        let body = read_json(response, SMALL_RESPONSE_BYTES).await?;
        let id = body
            .get("snapshot_id")
            .and_then(Value::as_str)
            .ok_or_else(|| Failure::terminal("Bright Data did not return a snapshot id"))?;
        SnapshotId::parse(id).map_err(|error| Failure::terminal(error.message()))
    }

    async fn progress_once(&self, id: &SnapshotId) -> Result<SnapshotStatus, Failure> {
        let url = self.endpoint(&format!("/datasets/v3/progress/{id}"));
        let response = self
            .send(RateKey::Datasets, self.datasets.get(&url))
            .await?;
        let response = self.ensure_success(response).await?;
        let body = read_json(response, SMALL_RESPONSE_BYTES).await?;
        match body.get("status").and_then(Value::as_str) {
            Some("starting" | "running" | "building" | "collecting" | "digesting") => {
                Ok(SnapshotStatus::Running)
            }
            Some("ready") => Ok(SnapshotStatus::Ready),
            Some("failed") => Ok(SnapshotStatus::Failed),
            Some("canceled" | "cancelled") => Ok(SnapshotStatus::Cancelled),
            _ => Err(Failure::terminal(
                "Bright Data returned an unknown snapshot status",
            )),
        }
    }

    async fn download_once(&self, id: &SnapshotId) -> Result<Download, Failure> {
        let mut url = self.endpoint(&format!("/datasets/v3/snapshot/{id}"));
        url.query_pairs_mut().append_pair("format", "json");
        let response = self
            .send(RateKey::Datasets, self.datasets.get(&url))
            .await?;
        let status = response.status();
        if status == StatusCode::ACCEPTED || status == StatusCode::CONFLICT {
            return Ok(Download::NotReady);
        }
        let response = self.ensure_success(response).await?;
        match read_json(response, SNAPSHOT_RESPONSE_BYTES).await? {
            Value::Array(records) => Ok(Download::Records(records)),
            _ => Err(Failure::terminal(
                "Bright Data returned an unexpected snapshot shape",
            )),
        }
    }

    async fn cancel_once(&self, id: &SnapshotId) -> Result<(), Failure> {
        let url = self.endpoint(&format!("/datasets/v3/snapshot/{id}/cancel"));
        let response = self
            .send(RateKey::Datasets, self.datasets.post(&url))
            .await?;
        self.ensure_success(response).await.map(|_| ())
    }
}

impl Transport for BrightDataClient {
    async fn unlock(&self, request: &UnlockRequest) -> Result<UnlockedPage, BrightDataError> {
        with_retry(&self.retry, Idempotency::Safe, move || {
            self.unlock_once(request)
        })
        .await
    }

    async fn trigger(&self, trigger: &DatasetTrigger) -> Result<SnapshotId, BrightDataError> {
        trigger.validate()?;
        // Starts a paid collection: never repeated after a possible accept.
        with_retry(&self.retry, Idempotency::Unsafe, move || {
            self.trigger_once(trigger)
        })
        .await
    }

    async fn progress(&self, id: &SnapshotId) -> Result<SnapshotStatus, BrightDataError> {
        with_retry(&self.retry, Idempotency::Safe, move || {
            self.progress_once(id)
        })
        .await
    }

    async fn download(&self, id: &SnapshotId) -> Result<Download, BrightDataError> {
        with_retry(&self.retry, Idempotency::Safe, move || {
            self.download_once(id)
        })
        .await
    }

    async fn cancel(&self, id: &SnapshotId) -> Result<(), BrightDataError> {
        with_retry(&self.retry, Idempotency::Safe, move || self.cancel_once(id)).await
    }
}

async fn read_json(response: GuardedResponse, max_bytes: u64) -> Result<Value, Failure> {
    response
        .json::<Value>(max_bytes)
        .await
        .map_err(|error| match error {
            GuardError::InvalidJson(_) => {
                Failure::terminal("Bright Data returned a response that was not JSON")
            }
            other => Failure::from_guard(&other),
        })
}

/// The `error`/`message` string of a JSON error body, the JSON itself, or
/// the trimmed text (Bright Data sometimes answers in plain text).
fn error_detail(body: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(value) => ["error", "message"]
            .iter()
            .find_map(|key| value.get(*key).and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| value.to_string()),
        Err(_) => body.trim().to_owned(),
    }
}

#[cfg(test)]
mod tests;
