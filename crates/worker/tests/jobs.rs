//! Worker integration tests: the job consumer against a real Postgres with
//! a fake Bright Data transport (serving the retailer fixtures) and a fake
//! Amazon source. Ignored by default (they need a database); run with
//! `cargo test -p worker --test jobs -- --ignored` and `DATABASE_URL` set.
//!
//! The tests share the queues and the `safety_settings` row, so each one
//! holds a Postgres advisory lock for its whole run (no process-level
//! state) and leaves the connector disabled and the kill switch off, as
//! freshly migrated.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use app::{
    AmazonDataSource, AmazonSource, AmazonSourceError, BudgetError, CatalogItem, CatalogResult,
    FeeEstimate, Fulfillment, OfferSummary, PortFuture, RequestBudget, SearchRepo,
};
use brightdata::runs::{Clock as RunClock, Runner, Timing};
use brightdata::{
    BrightDataError, DatasetTrigger, Download, SnapshotId, SnapshotStatus, Transport,
    UnlockRequest, UnlockedPage,
};
use db::DatabaseUrl;
use db::amazon_products::{self, PgAmazonProducts};
use db::brightdata_runs::{self, SafetySettings};
use db::price_history::{PgMatches, PgPriceHistory};
use db::queue::{self, Queue};
use db::searches::PgSearches;
use db::usage::PgRequestBudget;
use domain::{
    Asin, Gtin, PriceMxn, RetailerSearchOutcome, RetailerSku, SearchQuery, SearchStatus, TenantId,
    UnixMillis,
};
use http_guard::Redactor;
use matching::MatchConfig;
use retailers::category::CategoryListing;
use retailers::{HealthConfig, Page, ParseHealth};
use sqlx::Row as _;
use sqlx::postgres::{PgConnection, PgPool};
use worker::amazon_source::{SharedAmazon, TenantAmazon, amazon_source};
use worker::brightdata_store::PgRunStore;
use worker::config::WorkerConfig;
use worker::jobs::consumer::{self, ConsumerConfig, Decision, Processed};
use worker::jobs::{JobContext, JobMessage, Jobs};

/// Advisory lock key serializing these tests (distinct from the driver
/// lock).
const TEST_LOCK: i64 = 0x6d65_785f_6a6f_6273;

fn now_ms() -> i64 {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    i64::try_from(ms).unwrap()
}

fn unique_tenant() -> TenantId {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    TenantId::new(&format!("test-{nanos}")).unwrap()
}

/// A fixed clock for both clock ports.
#[derive(Clone, Copy)]
struct TestClock(i64);

impl RunClock for TestClock {
    fn now_ms(&self) -> i64 {
        self.0
    }
}

impl app::Clock for TestClock {
    fn now(&self) -> UnixMillis {
        UnixMillis(self.0)
    }
}

/// Serves `crates/retailers/tests/fixtures/<retailer>/<page>` for every
/// fetched URL and records `(retailer, call)`.
struct FakeTransport {
    page: &'static str,
    calls: Mutex<Vec<(String, &'static str)>>,
}

impl FakeTransport {
    fn new(page: &'static str) -> Self {
        Self {
            page,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<(String, &'static str)> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn serve(
        &self,
        request: &UnlockRequest,
        call: &'static str,
    ) -> Result<UnlockedPage, BrightDataError> {
        let Some(info) = retailers::retailer_for_host(&request.url.host()) else {
            return Err(BrightDataError::Terminal("unknown host".to_owned()));
        };
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((info.id.to_owned(), call));
        let body = std::fs::read_to_string(fixture(info.id, self.page))
            .map_err(|_| BrightDataError::Terminal("no fixture".to_owned()))?;
        Ok(UnlockedPage { status: 200, body })
    }
}

fn fixture(retailer: &str, page: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../retailers/tests/fixtures")
        .join(retailer)
        .join(page)
}

impl Transport for FakeTransport {
    async fn unlock(&self, request: &UnlockRequest) -> Result<UnlockedPage, BrightDataError> {
        self.serve(request, "unlock")
    }

    async fn browse(&self, request: &UnlockRequest) -> Result<UnlockedPage, BrightDataError> {
        self.serve(request, "browse")
    }

    async fn trigger(&self, _: &DatasetTrigger) -> Result<SnapshotId, BrightDataError> {
        Err(BrightDataError::Terminal("datasets not faked".to_owned()))
    }

    async fn progress(&self, _: &SnapshotId) -> Result<SnapshotStatus, BrightDataError> {
        Err(BrightDataError::Terminal("datasets not faked".to_owned()))
    }

    async fn download(&self, _: &SnapshotId) -> Result<Download, BrightDataError> {
        Err(BrightDataError::Terminal("datasets not faked".to_owned()))
    }

    async fn cancel(&self, _: &SnapshotId) -> Result<(), BrightDataError> {
        Ok(())
    }
}

/// Answers every lookup with one candidate titled like the query, priced
/// 900.00 MXN with 100.00 MXN of fees; counts calls.
struct FakeAmazon {
    calls: Mutex<u32>,
    now: UnixMillis,
}

impl FakeAmazon {
    fn count(&self) {
        *self.calls.lock().unwrap_or_else(PoisonError::into_inner) += 1;
    }

    fn calls(&self) -> u32 {
        *self.calls.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn catalog(&self, title: &str) -> CatalogResult {
        CatalogResult {
            items: vec![CatalogItem {
                asin: Asin::new("B0TESTJOB1").unwrap(),
                title: title.to_owned(),
                brand: None,
                images: Vec::new(),
                sales_ranks: Vec::new(),
            }],
            source: AmazonDataSource::SpApi,
            fetched_at: self.now,
        }
    }
}

impl AmazonSource for FakeAmazon {
    fn catalog_by_gtin<'a>(
        &'a self,
        _: &'a Gtin,
    ) -> PortFuture<'a, Result<CatalogResult, AmazonSourceError>> {
        self.count();
        // No GTIN hits: matching falls back to the keyword lookup.
        Box::pin(async move {
            Ok(CatalogResult {
                items: Vec::new(),
                source: AmazonDataSource::SpApi,
                fetched_at: self.now,
            })
        })
    }

    fn catalog_by_keyword<'a>(
        &'a self,
        keywords: &'a str,
    ) -> PortFuture<'a, Result<CatalogResult, AmazonSourceError>> {
        self.count();
        Box::pin(async move { Ok(self.catalog(keywords)) })
    }

    fn offers<'a>(
        &'a self,
        _: &'a Asin,
    ) -> PortFuture<'a, Result<OfferSummary, AmazonSourceError>> {
        self.count();
        Box::pin(async move {
            Ok(OfferSummary {
                buy_box_mxn: Some(PriceMxn::from_centavos(90_000).unwrap()),
                lowest_new_mxn: None,
                offer_count: 1,
                source: AmazonDataSource::SpApi,
                fetched_at: self.now,
            })
        })
    }

    fn fees<'a>(
        &'a self,
        _: &'a Asin,
        _: PriceMxn,
        _: Fulfillment,
    ) -> PortFuture<'a, Result<FeeEstimate, AmazonSourceError>> {
        self.count();
        Box::pin(async move {
            Ok(FeeEstimate {
                total_mxn: PriceMxn::from_centavos(10_000).unwrap(),
                details: Vec::new(),
                source: AmazonDataSource::SpApi,
                fetched_at: self.now,
            })
        })
    }
}

/// Everything a job run needs, owned by the test.
struct Harness {
    pool: PgPool,
    // Holds the advisory lock until the test ends.
    _lock: PgConnection,
    clock: TestClock,
    transport: FakeTransport,
    store: PgRunStore,
    amazon: Arc<FakeAmazon>,
    budget: Arc<PgRequestBudget>,
    prices: PgPriceHistory,
    matches: PgMatches,
    products: PgAmazonProducts,
    match_config: MatchConfig,
    health: Mutex<ParseHealth>,
    listings: Vec<CategoryListing>,
    config: WorkerConfig,
    redactor: Redactor,
}

impl Harness {
    async fn new(page: &'static str, settings: SafetySettings) -> Self {
        let url = DatabaseUrl::from_env()
            .expect("DATABASE_URL is valid")
            .expect("DATABASE_URL is set");
        let pool = db::connect(&url).await.expect("connect");
        db::migrate(&pool).await.expect("migrate");
        // Detached: dropping it closes the session and releases the lock.
        let mut lock = pool.acquire().await.expect("lock connection").detach();
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(TEST_LOCK)
            .execute(&mut lock)
            .await
            .unwrap();
        for q in Queue::ALL {
            sqlx::query("SELECT pgmq.purge_queue($1::text)")
                .bind(q.as_str())
                .execute(&pool)
                .await
                .unwrap();
        }
        brightdata_runs::set_safety_settings(&pool, settings)
            .await
            .unwrap();
        let clock = TestClock(now_ms());
        let config = WorkerConfig::from_lookup(|name| {
            let value = match name {
                "JOB_MAX_ATTEMPTS" => "2",
                "BRIGHTDATA_MAX_RUNS_PER_TENANT_PER_DAY" => "10000",
                "BRIGHTDATA_MAX_ITEMS_PER_TENANT_PER_DAY" => "1000000",
                "BRIGHTDATA_MAX_ITEMS_PER_RUN" => "1000",
                "AMAZON_MAX_REQUESTS_PER_TENANT_PER_DAY" => "1000000",
                "CATEGORY_MAX_PAGES_PER_RUN" => "1",
                _ => return None,
            };
            Some(value.to_owned())
        })
        .unwrap();
        let listings = retailers::category::parse_listings(
            &std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/category_listings.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let redactor = Redactor::default();
        Self {
            store: PgRunStore::new(pool.clone(), redactor.clone()),
            amazon: Arc::new(FakeAmazon {
                calls: Mutex::new(0),
                now: UnixMillis(clock.0),
            }),
            budget: Arc::new(PgRequestBudget::new(
                pool.clone(),
                Arc::new(clock),
                config.amazon_requests_per_day,
            )),
            prices: PgPriceHistory(pool.clone()),
            matches: PgMatches(pool.clone()),
            products: PgAmazonProducts(pool.clone()),
            match_config: MatchConfig::default(),
            health: Mutex::new(ParseHealth::new(HealthConfig::default())),
            transport: FakeTransport::new(page),
            pool,
            _lock: lock,
            clock,
            listings,
            config,
            redactor,
        }
    }

    fn consumer_config(&self) -> ConsumerConfig {
        ConsumerConfig {
            visibility_secs: self.config.visibility_secs,
            max_attempts: self.config.max_attempts,
            poll_idle: std::time::Duration::from_millis(self.config.poll_idle_ms),
        }
    }

    fn runner(&self) -> Runner<'_, FakeTransport, PgRunStore, TestClock> {
        Runner {
            transport: Some(&self.transport),
            store: &self.store,
            clock: &self.clock,
            timing: Timing::default(),
            caps: self.config.caps,
            robots: None,
        }
    }

    fn context<'a>(
        &'a self,
        runner: &'a Runner<'a, FakeTransport, PgRunStore, TestClock>,
        amazon: Arc<dyn TenantAmazon>,
    ) -> JobContext<'a, FakeTransport, PgRunStore, TestClock> {
        JobContext {
            pool: &self.pool,
            runner,
            clock: &self.clock,
            amazon,
            budget: Arc::clone(&self.budget) as Arc<dyn app::RequestBudget>,
            prices: &self.prices,
            matches: &self.matches,
            products: &self.products,
            match_config: &self.match_config,
            health: &self.health,
            listings: &self.listings,
            config: &self.config,
            redactor: &self.redactor,
        }
    }

    async fn process(&self, queue: Queue) -> Option<Processed> {
        let runner = self.runner();
        let amazon = SharedAmazon(Arc::clone(&self.amazon) as Arc<dyn AmazonSource>);
        let jobs = Jobs {
            ctx: self.context(&runner, Arc::new(amazon)),
            only_tenant: None,
        };
        consumer::process_one(&jobs, queue, self.consumer_config())
            .await
            .expect("queue access")
    }

    fn searches(&self) -> PgSearches {
        PgSearches::new(self.pool.clone(), Arc::new(self.clock))
    }

    async fn count(&self, sql: &'static str, retailer: &str) -> i64 {
        sqlx::query(sql)
            .bind(retailer)
            .fetch_one(&self.pool)
            .await
            .unwrap()
            .get::<i64, _>(0)
    }

    /// Back to the freshly migrated settings.
    async fn restore(&self) {
        brightdata_runs::set_safety_settings(&self.pool, CLOSED)
            .await
            .unwrap();
    }
}

const OPEN: SafetySettings = SafetySettings {
    global_kill_switch: false,
    brightdata_enabled: true,
};

const CLOSED: SafetySettings = SafetySettings {
    global_kill_switch: false,
    brightdata_enabled: false,
};

const OBSERVATIONS: &str = "SELECT count(*) FROM price_observations WHERE retailer = $1";

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn search_job_writes_results_with_roi_and_marks_category_only_stores() {
    let h = Harness::new("search.html", OPEN).await;
    let tenant = unique_tenant();
    let repo = h.searches();
    let id = repo
        .create_and_enqueue(
            &tenant,
            &SearchQuery::parse("audifonos inalambricos").unwrap(),
        )
        .await
        .unwrap();

    let processed = h.process(Queue::SearchJobs).await.expect("one message");
    assert_eq!(processed.decision, Decision::Delete);
    h.restore().await;

    let view = repo
        .load(&tenant, id)
        .await
        .unwrap()
        .expect("search exists");
    assert!(
        matches!(view.status, SearchStatus::Done | SearchStatus::Partial),
        "{view:?}"
    );
    assert!(view.finished_at.is_some());
    for store in ["sears_mx", "sanborns"] {
        let row = view
            .retailers
            .iter()
            .find(|r| r.retailer.as_str() == store)
            .expect("category-only store listed");
        assert_eq!(row.outcome, RetailerSearchOutcome::CategoryOnly);
    }
    assert!(
        view.retailers
            .iter()
            .any(|r| r.outcome == RetailerSearchOutcome::Ok),
        "{:?}",
        view.retailers
    );
    let priced: Vec<_> = view
        .results
        .iter()
        .filter_map(|r| r.amazon.as_ref())
        .filter(|a| a.roi_bp.is_some())
        .collect();
    assert!(!priced.is_empty(), "{:?}", view.results);
    assert!(
        priced
            .iter()
            .all(|a| a.amazon_price_mxn == Some(PriceMxn::from_centavos(90_000).unwrap()))
    );
    // The membership flag is stored per result and follows the registry
    // (Costco and Sam's Club need one); availability round-trips (an invalid
    // stored value would fail the load closed).
    for result in &view.results {
        let info = retailers::all()
            .iter()
            .find(|i| i.id == result.retailer.as_str())
            .expect("registered retailer");
        assert_eq!(
            result.membership_required, info.membership_required,
            "{result:?}"
        );
    }
    // Category-only stores were never fetched by the search.
    let calls = h.transport.calls();
    assert!(!calls.is_empty());
    assert!(
        calls
            .iter()
            .all(|(r, _)| r != "sears_mx" && r != "sanborns"),
        "{calls:?}"
    );
    assert!(h.amazon.calls() > 0);
    // The matched product's Amazon name was stored (ADR 0017).
    let product = amazon_products::find_product(&h.pool, "B0TESTJOB1")
        .await
        .unwrap()
        .expect("amazon_products row for the matched ASIN");
    assert!(!product.title.is_empty());
    assert_eq!(product.source, "sp_api");
    assert!(product.seen_at_ms >= h.amazon.now.0);
    // Another tenant cannot read it.
    assert!(repo.load(&unique_tenant(), id).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn transient_failures_retry_then_dead_letter() {
    let h = Harness::new("search.html", OPEN).await;
    let tenant = unique_tenant();
    // An active run on the search scope: every attempt is refused by the
    // one-active-run gate, which is transient.
    let run_id: i64 = sqlx::query(
        "INSERT INTO brightdata_runs (tenant_id, job, mode, input_json, status) \
         VALUES ($1, 'search', 'dataset', '{}', 'ready') RETURNING id",
    )
    .bind(tenant.as_str())
    .fetch_one(&h.pool)
    .await
    .unwrap()
    .get(0);
    let repo = h.searches();
    let id = repo
        .create_and_enqueue(&tenant, &SearchQuery::parse("licuadora").unwrap())
        .await
        .unwrap();

    let first = h.process(Queue::SearchJobs).await.expect("first attempt");
    assert_eq!(first.decision, Decision::Retry { delay_secs: 30 });
    // Hidden by the backoff until made visible again.
    assert!(h.process(Queue::SearchJobs).await.is_none());
    queue::set_vt(&h.pool, Queue::SearchJobs, first.msg_id, 0)
        .await
        .unwrap();
    let second = h.process(Queue::SearchJobs).await.expect("second attempt");
    assert_eq!(second.msg_id, first.msg_id);
    assert_eq!(second.decision, Decision::DeadLetter { code: "active_run" });
    sqlx::query("UPDATE brightdata_runs SET status = 'failed' WHERE id = $1")
        .bind(run_id)
        .execute(&h.pool)
        .await
        .unwrap();
    h.restore().await;

    assert_eq!(queue::length(&h.pool, Queue::SearchJobs).await.unwrap(), 0);
    let dead = queue::read(&h.pool, Queue::SearchJobsDlq, 30, 10)
        .await
        .unwrap();
    assert_eq!(dead.len(), 1);
    let body: serde_json::Value = serde_json::from_str(&dead[0].message).unwrap();
    assert_eq!(body["error_code"], "active_run");
    assert_eq!(body["attempts"], 2);
    assert_eq!(body["message"]["search_id"], id.0);
    let view = repo.load(&tenant, id).await.unwrap().unwrap();
    assert_eq!(view.status, SearchStatus::Failed);
    assert_eq!(view.reason, "active_run");
    assert!(h.transport.calls().is_empty());
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn kill_switch_refuses_before_any_paid_call() {
    let h = Harness::new(
        "search.html",
        SafetySettings {
            global_kill_switch: true,
            brightdata_enabled: true,
        },
    )
    .await;
    let tenant = unique_tenant();
    let repo = h.searches();
    let id = repo
        .create_and_enqueue(&tenant, &SearchQuery::parse("cafetera").unwrap())
        .await
        .unwrap();
    queue::send(
        &h.pool,
        Queue::RefreshJobs,
        &JobMessage::CategoryBrowse {
            retailer: "sears_mx",
        }
        .to_json(),
    )
    .await
    .unwrap();

    let search = h.process(Queue::SearchJobs).await.unwrap();
    let browse = h.process(Queue::RefreshJobs).await.unwrap();
    // The Amazon budget refuses under the kill switch too.
    let budget = h.budget.consume(&tenant, 1).await;
    h.restore().await;
    assert_eq!(budget, Err(BudgetError::KillSwitch));

    // Refusals are finished, not retried.
    assert_eq!(search.decision, Decision::Delete);
    assert_eq!(browse.decision, Decision::Delete);
    assert!(h.transport.calls().is_empty());
    assert_eq!(h.amazon.calls(), 0);
    let view = repo.load(&tenant, id).await.unwrap().unwrap();
    assert_eq!(view.status, SearchStatus::Refused);
    assert_eq!(view.reason, "kill_switch");
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn product_refresh_writes_price_observations() {
    let h = Harness::new("product.html", OPEN).await;
    // The SKU the walmart_mx product fixture parses to.
    let adapter = retailers::adapter_for("walmart_mx").unwrap();
    let seed = adapter
        .product_url(&RetailerSku::new("1").unwrap())
        .unwrap();
    let parsed = adapter
        .parse_product(&Page {
            url: seed,
            body: std::fs::read_to_string(fixture("walmart_mx", "product.html")).unwrap(),
            fetched_at: UnixMillis(h.clock.0),
        })
        .unwrap();
    let sku = parsed.retailer_sku.clone();
    db::tracking::track(&h.pool, "walmart_mx", sku.as_str(), h.clock.0 - 1)
        .await
        .unwrap();
    let before = h.count(OBSERVATIONS, "walmart_mx").await;
    queue::send(
        &h.pool,
        Queue::RefreshJobs,
        &JobMessage::ProductRefresh {
            retailer: "walmart_mx",
            skus: vec![sku.clone()],
        }
        .to_json(),
    )
    .await
    .unwrap();

    let processed = h.process(Queue::RefreshJobs).await.unwrap();
    h.restore().await;

    assert_eq!(processed.decision, Decision::Delete);
    assert_eq!(h.count(OBSERVATIONS, "walmart_mx").await, before + 1);
    assert_eq!(
        h.transport.calls(),
        vec![("walmart_mx".to_owned(), "unlock")]
    );
    let refreshed: Option<i64> = sqlx::query(
        "SELECT (extract(epoch FROM last_refreshed_at) * 1000)::bigint FROM tracked_products \
         WHERE retailer = 'walmart_mx' AND retailer_sku = $1",
    )
    .bind(sku.as_str())
    .fetch_one(&h.pool)
    .await
    .unwrap()
    .get(0);
    assert_eq!(refreshed, Some(h.clock.0));
    // Refreshes never call Amazon.
    assert_eq!(h.amazon.calls(), 0);
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn category_browse_respects_the_page_cap_and_feeds_matching() {
    let h = Harness::new("search.html", OPEN).await;
    let configured = h
        .listings
        .iter()
        .filter(|l| l.retailer == "sears_mx")
        .count();
    assert!(configured > 1, "the cap must cut something");
    let before = h.count(OBSERVATIONS, "sears_mx").await;
    queue::send(
        &h.pool,
        Queue::RefreshJobs,
        &JobMessage::CategoryBrowse {
            retailer: "sears_mx",
        }
        .to_json(),
    )
    .await
    .unwrap();

    let processed = h.process(Queue::RefreshJobs).await.unwrap();
    h.restore().await;

    assert_eq!(processed.decision, Decision::Delete);
    // CATEGORY_MAX_PAGES_PER_RUN = 1, through the Browser API.
    assert_eq!(h.transport.calls(), vec![("sears_mx".to_owned(), "browse")]);
    assert!(h.count(OBSERVATIONS, "sears_mx").await > before);
    assert!(
        h.count(
            "SELECT count(*) FROM offer_matches WHERE retailer = $1",
            "sears_mx"
        )
        .await
            > 0
    );
    assert!(h.amazon.calls() > 0);
}

#[tokio::test]
#[ignore = "needs Postgres (DATABASE_URL)"]
async fn amazon_fallback_runs_are_charged_to_the_paying_tenant() {
    const FALLBACK_RUNS: &str = "SELECT count(*) FROM brightdata_runs          WHERE tenant_id = $1 AND job = 'amazon_mx_fallback' AND id > $2";
    let h = Harness::new("search.html", OPEN).await;
    let before: i64 = sqlx::query("SELECT coalesce(max(id), 0) FROM brightdata_runs")
        .fetch_one(&h.pool)
        .await
        .unwrap()
        .get(0);
    // No SP-API: every lookup goes to the Bright Data fallback.
    let fallback = (
        Arc::new(FakeTransport::new("search.html")),
        Arc::new(PgRunStore::new(h.pool.clone(), h.redactor.clone())),
    );
    let (sources, _) = amazon_source(
        (None, None, None),
        Some(fallback),
        h.config.caps,
        &h.redactor,
    );
    let runner = h.runner();
    let ctx = h.context(&runner, Arc::new(sources));
    let gtin = Gtin::new("7501055363063").unwrap();
    // The fake transport fails the dataset trigger, after the run row exists.
    let result = ctx
        .amazon_for(&TenantId::default_tenant())
        .catalog_by_gtin(&gtin)
        .await;
    h.restore().await;

    assert_eq!(result.unwrap_err(), AmazonSourceError::Unavailable);
    let runs = |tenant: &'static str| {
        sqlx::query(FALLBACK_RUNS)
            .bind(tenant)
            .bind(before)
            .fetch_one(&h.pool)
    };
    assert_eq!(runs("default").await.unwrap().get::<i64, _>(0), 1);
    assert_eq!(runs("system").await.unwrap().get::<i64, _>(0), 0);
}
