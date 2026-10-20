//! Tests for the assistant (ADR 0013): `POST /assistant/chat` against a
//! fake sidecar on loopback, `POST /matches/decisions`, and the
//! `/internal/agent/*` tool routes with their service bearer and grants.
//! Covers auth (401/403), history validation, the daily token budget,
//! sidecar failures, disconnect, tenant isolation, the per-tenant
//! decision overlay and that secrets never reach logs or responses.

mod common;

use std::collections::HashMap;
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use api::{AgentAuth, AssistantDeps, AssistantState, GrantKey, ProductsState, SearchesState};
use app::{
    AgentUsageRepo, ChatId, Clock, MatchRepo, PortFuture, PriceHistoryRepo, PricePoint,
    PriceSeries, PriceStats, RecentResultsFilter, RepoError, ResultMatchView, RetailerOutcomeView,
    SearchId, SearchRepo, SearchResultView, SearchResults, SearchView, StartSearch, TokenCaps,
    TrackedResultView, UpsertOutcome,
};
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::response::{IntoResponse as _, Response};
use common::{Client, FakeUsers, MemoryDecisions, TestApp, generous_limits};
use domain::{
    Asin, Availability, Confidence, DAY_MS, MatchStatus, Offer, PriceMxn, ProductMatch, RetailerId,
    RetailerSearchOutcome, RetailerSku, SearchQuery, SearchStatus, TenantId, TokenCount,
    TokenUsage, UnixMillis, UserId,
};
use http_body_util::BodyExt as _;
use http_guard::{GuardedClient, HostAllowlist, Redactor, Secret, TransportPolicy, Url};
use serde_json::{Value, json};
use tower::ServiceExt as _;

const NOW: i64 = 1_760_000_000_000;
const ASIN: &str = "B0E2ETEST1";
const SERVICE_TOKEN: &str = "svc-token-0123456789abcdef0123456789abcdef";
const GRANT_KEY: &str = "grant-key-0123456789abcdef0123456789abcdef";
const FREE_CAP: u64 = 1_000;

struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> UnixMillis {
        UnixMillis(NOW)
    }
}

fn secret(raw: &str) -> Secret {
    Secret::new(raw.to_owned()).unwrap()
}

fn grant_key() -> GrantKey {
    GrantKey::new(secret(GRANT_KEY)).unwrap()
}

// ---------------------------------------------------------------- fixtures

fn result(retailer: &str, sku: &str, status: Option<MatchStatus>) -> SearchResultView {
    SearchResultView {
        retailer: RetailerId::new(retailer).unwrap(),
        retailer_sku: RetailerSku::new(sku).unwrap(),
        title: format!("Taladro {sku}"),
        url: format!("https://example.com/{retailer}/{sku}"),
        price_mxn: PriceMxn::from_centavos(50_000).unwrap(),
        availability: Availability::InStock,
        membership_required: false,
        amazon: status.map(|status| ResultMatchView {
            asin: Asin::new(ASIN).unwrap(),
            status,
            confidence: Confidence::from_bp(7_000).unwrap(),
            amazon_price_mxn: Some(PriceMxn::from_centavos(90_000).unwrap()),
            fees_mxn: Some(PriceMxn::from_centavos(15_000).unwrap()),
            profit_centavos: Some(25_000),
            roi_bp: Some(5_000),
            margin_bp: Some(2_777),
        }),
    }
}

/// Results per tenant; search 1 of each tenant holds all its results.
struct FakeSearches(Vec<(TenantId, SearchResultView)>);

impl FakeSearches {
    fn fixture() -> Self {
        let t1 = TenantId::for_user(UserId::new(1).unwrap());
        let t2 = TenantId::for_user(UserId::new(2).unwrap());
        Self(vec![
            (
                t1.clone(),
                result("liverpool", "L1", Some(MatchStatus::NeedsReview)),
            ),
            (t1, result("walmart_mx", "W1", Some(MatchStatus::Accepted))),
            (
                t2.clone(),
                result("liverpool", "L1", Some(MatchStatus::NeedsReview)),
            ),
            (t2, result("sears_mx", "S2", None)),
        ])
    }

    fn of(&self, tenant: &TenantId) -> Vec<SearchResultView> {
        self.0
            .iter()
            .filter(|(t, _)| t == tenant)
            .map(|(_, r)| r.clone())
            .collect()
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
        tenant: &'a TenantId,
        id: SearchId,
    ) -> PortFuture<'a, Result<Option<SearchView>, RepoError>> {
        Box::pin(async move {
            Ok((id == SearchId(1)).then(|| SearchView {
                id,
                query: "taladro".to_owned(),
                status: SearchStatus::Done,
                reason: String::new(),
                created_at: UnixMillis(NOW - DAY_MS),
                finished_at: Some(UnixMillis(NOW - DAY_MS)),
                retailers: vec![RetailerOutcomeView {
                    retailer: RetailerId::new("liverpool").unwrap(),
                    outcome: RetailerSearchOutcome::Ok,
                    offers: 1,
                }],
                results: self.of(tenant),
                hidden_results: 0,
            }))
        })
    }

    fn recent_results<'a>(
        &'a self,
        tenant: &'a TenantId,
        since: UnixMillis,
        filter: &'a RecentResultsFilter,
        limit: u32,
    ) -> PortFuture<'a, Result<Vec<TrackedResultView>, RepoError>> {
        Box::pin(async move {
            assert!(since.0 < NOW - 89 * DAY_MS, "tracked window is 90 days");
            Ok(self
                .of(tenant)
                .into_iter()
                .filter(|r| {
                    filter
                        .listing
                        .as_ref()
                        .is_none_or(|(ret, sku)| &r.retailer == ret && &r.retailer_sku == sku)
                })
                .filter(|r| {
                    filter
                        .title_contains
                        .as_ref()
                        .is_none_or(|q| r.title.to_lowercase().contains(&q.as_str().to_lowercase()))
                })
                .take(usize::try_from(limit).unwrap())
                .map(|result| TrackedResultView {
                    search_id: SearchId(1),
                    seen_at: UnixMillis(NOW - DAY_MS),
                    result,
                })
                .collect())
        })
    }
}

/// Daily usage per tenant.
#[derive(Default)]
struct FakeUsage(Mutex<HashMap<String, TokenUsage>>);

impl FakeUsage {
    fn preset(&self, tenant: &str, total: u64) {
        self.0.lock().unwrap().insert(
            tenant.to_owned(),
            TokenUsage {
                input: TokenCount::new(total),
                output: TokenCount::ZERO,
            },
        );
    }
}

impl AgentUsageRepo for FakeUsage {
    fn used_on<'a>(
        &'a self,
        tenant: &'a TenantId,
        _: i64,
    ) -> PortFuture<'a, Result<TokenUsage, RepoError>> {
        Box::pin(async move {
            Ok(self
                .0
                .lock()
                .unwrap()
                .get(tenant.as_str())
                .copied()
                .unwrap_or_default())
        })
    }

    fn add<'a>(
        &'a self,
        tenant: &'a TenantId,
        _: i64,
        usage: TokenUsage,
    ) -> PortFuture<'a, Result<TokenUsage, RepoError>> {
        Box::pin(async move {
            let mut map = self.0.lock().unwrap();
            let entry = map.entry(tenant.as_str().to_owned()).or_default();
            entry.input = entry.input.saturating_add(usage.input);
            entry.output = entry.output.saturating_add(usage.output);
            Ok(*entry)
        })
    }
}

struct FakePrices;

impl PriceHistoryRepo for FakePrices {
    fn record<'a>(&'a self, _: &'a Offer) -> PortFuture<'a, Result<(), RepoError>> {
        Box::pin(async { Ok(()) })
    }

    fn history<'a>(
        &'a self,
        _: &'a RetailerId,
        _: &'a RetailerSku,
        _: UnixMillis,
    ) -> PortFuture<'a, Result<Vec<PricePoint>, RepoError>> {
        Box::pin(async {
            Ok(vec![PricePoint {
                price_mxn: PriceMxn::from_centavos(45_000).unwrap(),
                list_price_mxn: None,
                availability: Availability::InStock,
                observed_at: UnixMillis(NOW - 2 * DAY_MS),
            }])
        })
    }
}

/// Globally only `walmart_mx/W1` is accepted.
struct FakeMatches;

impl MatchRepo for FakeMatches {
    fn upsert<'a>(
        &'a self,
        _: &'a ProductMatch,
    ) -> PortFuture<'a, Result<UpsertOutcome, RepoError>> {
        Box::pin(async { panic!("the assistant never writes global matches") })
    }

    fn accepted_listings<'a>(
        &'a self,
        _: &'a Asin,
    ) -> PortFuture<'a, Result<Vec<(RetailerId, RetailerSku)>, RepoError>> {
        Box::pin(async {
            Ok(vec![(
                RetailerId::new("walmart_mx").unwrap(),
                RetailerSku::new("W1").unwrap(),
            )])
        })
    }
}

// ------------------------------------------------------------ fake sidecar

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// A plain reply (with a control character to strip).
    Reply,
    /// Calls the suggestion tool route, then replies.
    Suggest,
    Busy,
    Status504,
    Garbage,
    BadStop,
    /// Echoes the bearer token in an error body.
    EchoSecret,
    /// Never answers; flags when the request future is dropped.
    Slow,
}

struct Sidecar {
    mode: Mode,
    api: OnceLock<Router>,
    requests: Mutex<Vec<(HeaderMap, Value)>>,
    started: tokio::sync::Notify,
    dropped: Arc<AtomicBool>,
}

struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

async fn internal_call(
    api: &Router,
    method: Method,
    path: &str,
    grant: &str,
    body: Value,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {SERVICE_TOKEN}"))
        .header(api::GRANT_HEADER, grant)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    send(api, request).await
}

async fn send(api: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = api.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

async fn sidecar_chat(State(s): State<Arc<Sidecar>>, headers: HeaderMap, body: Bytes) -> Response {
    let body: Value = serde_json::from_slice(&body).unwrap();
    s.requests
        .lock()
        .unwrap()
        .push((headers.clone(), body.clone()));
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    if auth != format!("Bearer {SERVICE_TOKEN}") {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let ok = |reply: &str, stop: &str| {
        axum::Json(json!({
            "reply": reply,
            "stop": stop,
            "usage": { "input_tokens": 120, "output_tokens": 30 }
        }))
        .into_response()
    };
    match s.mode {
        Mode::Reply => ok("Hola\u{7}, el mínimo de 90 días fue $450.00", "done"),
        Mode::Suggest => {
            let grant = body["grant"].as_str().unwrap();
            let api = s.api.get().unwrap();
            let (status, _) = internal_call(
                api,
                Method::POST,
                "/internal/agent/match-suggestions",
                grant,
                json!({
                    "retailer": "liverpool",
                    "retailer_sku": "L1",
                    "asin": ASIN,
                    "decision": "accept",
                    "reason": "Mismo modelo y capacidad"
                }),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED);
            ok("Te propongo aceptar la coincidencia.", "done")
        }
        Mode::Busy => (StatusCode::CONFLICT, "{\"error\":\"agent_busy\"}").into_response(),
        Mode::Status504 => StatusCode::GATEWAY_TIMEOUT.into_response(),
        Mode::Garbage => (StatusCode::OK, "not json").into_response(),
        Mode::BadStop => ok("hola", "exploded"),
        Mode::EchoSecret => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("bad token {auth} {GRANT_KEY}"),
        )
            .into_response(),
        Mode::Slow => {
            let _flag = DropFlag(s.dropped.clone());
            s.started.notify_one();
            tokio::time::sleep(Duration::from_secs(60)).await;
            ok("too late", "done")
        }
    }
}

/// Starts a fake sidecar; returns it and its base URL.
async fn start_sidecar(mode: Mode) -> (Arc<Sidecar>, Url) {
    let sidecar = Arc::new(Sidecar {
        mode,
        api: OnceLock::new(),
        requests: Mutex::new(Vec::new()),
        started: tokio::sync::Notify::new(),
        dropped: Arc::new(AtomicBool::new(false)),
    });
    let router = Router::new()
        .route("/v1/chat", axum::routing::post(sidecar_chat))
        .with_state(sidecar.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (
        sidecar,
        Url::parse(&format!("http://127.0.0.1:{}", addr.port())).unwrap(),
    )
}

// ------------------------------------------------------------------ harness

struct World {
    app: TestApp,
    sidecar: Option<Arc<Sidecar>>,
    usage: Arc<FakeUsage>,
    decisions: Arc<MemoryDecisions>,
}

struct Options {
    mode: Option<Mode>,
    timeout: Duration,
    agent: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            mode: Some(Mode::Reply),
            timeout: Duration::from_secs(10),
            agent: true,
        }
    }
}

async fn world(opts: Options) -> World {
    let clock: Arc<dyn Clock> = Arc::new(FixedClock);
    let searches = Arc::new(FakeSearches::fixture());
    let decisions = Arc::new(MemoryDecisions::default());
    let usage = Arc::new(FakeUsage::default());
    let users = Arc::new(FakeUsers::default());
    let grants = Arc::new(grant_key());
    let (sidecar, engine) = match opts.mode {
        Some(mode) => {
            let (sidecar, url) = start_sidecar(mode).await;
            let client = GuardedClient::builder(HostAllowlist::new(["localhost"]).unwrap())
                .policy(TransportPolicy::LOOPBACK_TEST)
                .total_timeout(opts.timeout)
                .redactor(Redactor::new([&secret(SERVICE_TOKEN), &secret(GRANT_KEY)]))
                .build()
                .unwrap();
            let engine = api::SidecarEngine::with_client(
                client,
                &url,
                secret(SERVICE_TOKEN),
                grants.clone(),
                clock.clone(),
            )
            .unwrap();
            (
                Some(sidecar),
                Some(Arc::new(engine) as Arc<dyn app::AssistantEngine>),
            )
        }
        None => (None, None),
    };
    let stats = Arc::new(PriceStats::new(
        Arc::new(FakePrices),
        Arc::new(FakeMatches),
        decisions.clone(),
        clock.clone(),
    ));
    let series = Arc::new(PriceSeries::new(
        Arc::new(FakePrices),
        Arc::new(FakeMatches),
        decisions.clone(),
        clock.clone(),
    ));
    let assistant = AssistantState::new(AssistantDeps {
        engine,
        agent: opts.agent.then(|| AgentAuth {
            service_token: secret(SERVICE_TOKEN),
            grants: grants.clone(),
        }),
        searches: searches.clone(),
        decisions: decisions.clone(),
        usage: usage.clone(),
        users: users.clone(),
        stats: stats.clone(),
        series: series.clone(),
        clock,
        caps: TokenCaps {
            free: TokenCount::new(FREE_CAP),
            pro: TokenCount::new(FREE_CAP * 10),
        },
    });
    let app = TestApp::with_users(
        users,
        ProductsState::new(stats, series),
        SearchesState::new(
            StartSearch::new(searches.clone()),
            SearchResults::new(searches, decisions.clone()),
        ),
        assistant,
        generous_limits(),
    );
    if let Some(s) = &sidecar {
        s.api.set(app.router.clone()).ok().unwrap();
    }
    World {
        app,
        sidecar,
        usage,
        decisions,
    }
}

async fn signed_in(app: &TestApp, email: &str) -> Client {
    let mut client = app.client();
    client.signed_up(email).await;
    client
}

fn ask(text: &str) -> Value {
    json!({ "messages": [{ "role": "user", "content": text }] })
}

async fn chat(client: &mut Client, body: Value) -> common::Reply {
    client
        .send(Method::POST, "/assistant/chat", Some(body))
        .await
}

fn grant_for(user: i64, tenant: &str, now: i64) -> String {
    grant_key().sign(
        UserId::new(user).unwrap(),
        &TenantId::new(tenant).unwrap(),
        &ChatId::parse("chat-test-1").unwrap(),
        UnixMillis(now),
    )
}

// -------------------------------------------------------------------- tests

#[tokio::test]
async fn chat_needs_a_session_and_csrf() {
    let w = world(Options::default()).await;
    let mut anon = w.app.client();
    anon.fetch_csrf().await;
    let reply = chat(&mut anon, ask("hola")).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);

    let mut user = signed_in(&w.app, "a@example.com").await;
    user.csrf = None;
    let reply = chat(&mut user, ask("hola")).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(reply.error(), "csrf");
    assert!(w.sidecar.unwrap().requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn invalid_histories_are_422() {
    let w = world(Options::default()).await;
    let mut user = signed_in(&w.app, "a@example.com").await;
    let too_many: Vec<Value> = (0..21)
        .map(|_| json!({ "role": "user", "content": "x" }))
        .collect();
    for body in [
        json!({ "messages": [] }),
        json!({ "messages": [{ "role": "assistant", "content": "hola" }] }),
        json!({ "messages": [{ "role": "system", "content": "hola" }] }),
        json!({ "messages": [{ "role": "user", "content": "   " }] }),
        json!({ "messages": [{ "role": "user", "content": "x".repeat(2_001) }] }),
        json!({ "messages": too_many }),
    ] {
        let reply = chat(&mut user, body).await;
        assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY, "{reply:?}");
        assert_eq!(reply.error(), "invalid_messages");
    }
    assert!(w.sidecar.unwrap().requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn chat_proxies_with_tenant_budget_and_grant_but_no_cookie() {
    let w = world(Options::default()).await;
    w.usage.preset("user-1", 400);
    let mut user = signed_in(&w.app, "a@example.com").await;
    let reply = chat(
        &mut user,
        json!({ "messages": [
            { "role": "user", "content": "¿Qué tal el taladro?" },
            { "role": "assistant", "content": "¿Cuál?" },
            { "role": "user", "content": "El de Liverpool" }
        ]}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{reply:?}");
    assert_eq!(
        reply.body["reply"], "Hola, el mínimo de 90 días fue $450.00",
        "control characters are stripped"
    );
    assert_eq!(reply.body["stop"], "done");
    assert_eq!(reply.body["usage"]["input_tokens"], 120);
    assert_eq!(reply.body["suggestions"], json!([]));
    let chat_id = reply.body["chat_id"].as_str().unwrap();
    assert!(ChatId::parse(chat_id).is_some());

    let sidecar = w.sidecar.unwrap();
    let requests = sidecar.requests.lock().unwrap();
    let (headers, body) = &requests[0];
    assert!(headers.get(header::COOKIE).is_none(), "no session cookie");
    assert_eq!(body["tenant"], "user-1");
    assert_eq!(body["locale"], "es");
    assert_eq!(body["chat_id"], chat_id);
    assert_eq!(body["token_budget"], FREE_CAP - 400);
    assert_eq!(body["messages"].as_array().unwrap().len(), 3);
    assert_eq!(body["messages"][1]["role"], "assistant");
    let grant = grant_key()
        .verify(body["grant"].as_str().unwrap(), UnixMillis(NOW))
        .unwrap();
    assert_eq!(grant.tenant.as_str(), "user-1");
    assert_eq!(grant.chat_id.as_str(), chat_id);
}

#[tokio::test]
async fn spent_budget_is_429_without_calling_the_sidecar() {
    let w = world(Options::default()).await;
    w.usage.preset("user-1", FREE_CAP);
    let mut user = signed_in(&w.app, "a@example.com").await;
    let reply = chat(&mut user, ask("hola")).await;
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(reply.error(), "agent_budget");
    assert!(w.sidecar.unwrap().requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn sidecar_failures_map_to_fixed_codes() {
    for (mode, status, code) in [
        (Mode::Busy, StatusCode::CONFLICT, "agent_busy"),
        (
            Mode::Status504,
            StatusCode::GATEWAY_TIMEOUT,
            "agent_timeout",
        ),
        (Mode::Garbage, StatusCode::BAD_GATEWAY, "agent_failed"),
        (Mode::BadStop, StatusCode::BAD_GATEWAY, "agent_failed"),
        (Mode::EchoSecret, StatusCode::BAD_GATEWAY, "agent_failed"),
    ] {
        let w = world(Options {
            mode: Some(mode),
            ..Options::default()
        })
        .await;
        let mut user = signed_in(&w.app, "a@example.com").await;
        let reply = chat(&mut user, ask("hola")).await;
        assert_eq!(reply.status, status, "{mode:?}: {reply:?}");
        assert_eq!(reply.error(), code, "{mode:?}");
    }
}

#[tokio::test]
async fn missing_sidecar_is_503() {
    let w = world(Options {
        mode: None,
        ..Options::default()
    })
    .await;
    let mut user = signed_in(&w.app, "a@example.com").await;
    let reply = chat(&mut user, ask("hola")).await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(reply.error(), "agent_unavailable");
}

#[tokio::test]
async fn slow_sidecar_times_out_with_504() {
    let w = world(Options {
        mode: Some(Mode::Slow),
        timeout: Duration::from_millis(300),
        ..Options::default()
    })
    .await;
    let mut user = signed_in(&w.app, "a@example.com").await;
    let reply = chat(&mut user, ask("hola")).await;
    assert_eq!(reply.status, StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(reply.error(), "agent_timeout");
}

#[tokio::test]
async fn browser_disconnect_closes_the_sidecar_request() {
    let w = world(Options {
        mode: Some(Mode::Slow),
        ..Options::default()
    })
    .await;
    let mut user = signed_in(&w.app, "a@example.com").await;
    let sidecar = w.sidecar.clone().unwrap();
    let task = tokio::spawn(async move { chat(&mut user, ask("hola")).await });
    tokio::time::timeout(Duration::from_secs(5), sidecar.started.notified())
        .await
        .expect("the sidecar got the request");
    // The browser goes away: the api drops the handler future.
    task.abort();
    for _ in 0..100 {
        if sidecar.dropped.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the sidecar request was not cancelled");
}

#[tokio::test]
async fn tie_break_is_proposed_then_confirmed_for_this_tenant_only() {
    let w = world(Options {
        mode: Some(Mode::Suggest),
        ..Options::default()
    })
    .await;
    let mut a = signed_in(&w.app, "a@example.com").await;
    let mut b = signed_in(&w.app, "b@example.com").await;

    let reply = chat(&mut a, ask("¿El taladro de Liverpool es el mismo?")).await;
    assert_eq!(reply.status, StatusCode::OK, "{reply:?}");
    let suggestions = reply.body["suggestions"].as_array().unwrap();
    assert_eq!(suggestions.len(), 1);
    let s = &suggestions[0];
    assert_eq!(s["retailer"], "liverpool");
    assert_eq!(s["retailer_sku"], "L1");
    assert_eq!(s["asin"], ASIN);
    assert_eq!(s["title"], "Taladro L1", "title comes from storage");
    assert_eq!(s["decision"], "accept");

    // Nothing changed yet.
    let search = a.get("/searches/1").await;
    assert_eq!(
        search.body["results"][0]["amazon"]["match_status"],
        "needs_review"
    );
    assert!(w.decisions.decisions.lock().unwrap().is_empty());

    // Another tenant cannot confirm this suggestion.
    let body = json!({
        "suggestion_id": s["id"], "retailer": "liverpool", "retailer_sku": "L1",
        "asin": ASIN, "decision": "accept"
    });
    let reply = b
        .send(Method::POST, "/matches/decisions", Some(body.clone()))
        .await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(reply.error(), "suggestion_mismatch");

    let reply = a.send(Method::POST, "/matches/decisions", Some(body)).await;
    assert_eq!(reply.status, StatusCode::OK, "{reply:?}");
    {
        let stored = w.decisions.decisions.lock().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].decided_by, UserId::new(1).unwrap());
        assert_eq!(stored[0].tenant.as_str(), "user-1");
        assert!(stored[0].suggestion_id.is_some());
    }

    let search = a.get("/searches/1").await;
    assert_eq!(
        search.body["results"][0]["amazon"]["match_status"],
        "accepted"
    );
    let stats = a.get(&format!("/products/{ASIN}/price-stats")).await;
    let listings: Vec<&str> = stats.body["listings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["retailer_sku"].as_str().unwrap())
        .collect();
    assert_eq!(listings, ["L1", "W1"]);

    // Tenant B still sees the stored state.
    let search = b.get("/searches/1").await;
    assert_eq!(
        search.body["results"][0]["amazon"]["match_status"],
        "needs_review"
    );
    let stats = b.get(&format!("/products/{ASIN}/price-stats")).await;
    assert_eq!(stats.body["listings"].as_array().unwrap().len(), 1);

    // Rejecting removes the match for A only.
    let reply = a
        .send(
            Method::POST,
            "/matches/decisions",
            Some(json!({
                "retailer": "liverpool", "retailer_sku": "L1", "asin": ASIN, "decision": "reject"
            })),
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    let search = a.get("/searches/1").await;
    assert!(search.body["results"][0]["amazon"].is_null());
}

#[tokio::test]
async fn decisions_need_a_reviewable_listing_and_csrf() {
    let w = world(Options::default()).await;
    let mut a = signed_in(&w.app, "a@example.com").await;
    let decide = |retailer: &str, sku: &str, decision: &str| json!({ "retailer": retailer, "retailer_sku": sku, "asin": ASIN, "decision": decision });
    let reply = a
        .send(
            Method::POST,
            "/matches/decisions",
            Some(decide("walmart_mx", "W1", "reject")),
        )
        .await;
    assert_eq!(
        reply.status,
        StatusCode::CONFLICT,
        "accepted is not under review"
    );
    assert_eq!(reply.error(), "not_reviewable");
    let reply = a
        .send(
            Method::POST,
            "/matches/decisions",
            Some(decide("sears_mx", "S2", "accept")),
        )
        .await;
    assert_eq!(
        reply.status,
        StatusCode::CONFLICT,
        "another tenant's listing"
    );
    let reply = a
        .send(
            Method::POST,
            "/matches/decisions",
            Some(decide("liverpool", "L1", "maybe")),
        )
        .await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(reply.error(), "invalid_decision");
    let reply = a
        .send(
            Method::POST,
            "/matches/decisions",
            Some(json!({
                "suggestion_id": 99, "retailer": "liverpool", "retailer_sku": "L1",
                "asin": ASIN, "decision": "accept"
            })),
        )
        .await;
    assert_eq!(reply.error(), "suggestion_mismatch");
    a.csrf = None;
    let reply = a
        .send(
            Method::POST,
            "/matches/decisions",
            Some(decide("liverpool", "L1", "accept")),
        )
        .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(w.decisions.decisions.lock().unwrap().is_empty());
}

#[tokio::test]
async fn internal_routes_need_the_service_token_and_a_valid_grant() {
    let w = world(Options::default()).await;
    signed_in(&w.app, "a@example.com").await;
    signed_in(&w.app, "b@example.com").await;
    let api = &w.app.router;
    let path = "/internal/agent/offers";
    let get = |auth: Option<&str>, grant: Option<&str>| {
        let mut r = Request::builder().uri(path);
        if let Some(a) = auth {
            r = r.header(header::AUTHORIZATION, a);
        }
        if let Some(g) = grant {
            r = r.header(api::GRANT_HEADER, g);
        }
        r.body(Body::empty()).unwrap()
    };
    let good = grant_for(1, "user-1", NOW);
    let bearer = format!("Bearer {SERVICE_TOKEN}");

    let (status, body) = send(api, get(None, Some(&good))).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("unauthenticated"))
    );
    let (status, _) = send(api, get(Some("Bearer wrong"), Some(&good))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    for grant in [
        None,
        Some("garbage".to_owned()),
        // Expired.
        Some(grant_for(1, "user-1", NOW - 200_000)),
        // Signed with another key.
        Some(GrantKey::new(secret(&"z".repeat(40))).unwrap().sign(
            UserId::new(1).unwrap(),
            &TenantId::new("user-1").unwrap(),
            &ChatId::parse("chat-test-1").unwrap(),
            UnixMillis(NOW),
        )),
        // Tenant that is not the user's.
        Some(grant_for(1, "user-2", NOW)),
        // Unknown user.
        Some(grant_for(9, "user-9", NOW)),
    ] {
        let (status, body) = send(api, get(Some(&bearer), grant.as_deref())).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{grant:?}");
        assert_eq!(body["error"], "invalid_grant");
    }

    let (status, body) = send(api, get(Some(&bearer), Some(&good))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let skus: Vec<&str> = body["offers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["offer"]["retailer_sku"].as_str().unwrap())
        .collect();
    assert_eq!(skus, ["L1", "W1"]);

    // Tenant B's grant sees B's offers only, whatever the query says.
    let b = grant_for(2, "user-2", NOW);
    let (status, body) = internal_call(
        api,
        Method::GET,
        "/internal/agent/offers?query=taladro",
        &b,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let skus: Vec<&str> = body["offers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["offer"]["retailer_sku"].as_str().unwrap())
        .collect();
    assert_eq!(skus, ["L1", "S2"]);
}

#[tokio::test]
async fn internal_disabled_without_agent_secrets() {
    let w = world(Options {
        agent: false,
        ..Options::default()
    })
    .await;
    signed_in(&w.app, "a@example.com").await;
    let (status, _) = internal_call(
        &w.app.router,
        Method::GET,
        "/internal/agent/offers",
        &grant_for(1, "user-1", NOW),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn tool_routes_validate_and_answer() {
    let w = world(Options::default()).await;
    signed_in(&w.app, "a@example.com").await;
    let api = &w.app.router;
    let g = grant_for(1, "user-1", NOW);

    let (status, body) = internal_call(
        api,
        Method::GET,
        "/internal/agent/offers?match_status=needs_review&limit=5",
        &g,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["offers"].as_array().unwrap().len(), 1);
    for bad in ["?limit=0", "?limit=21", "?match_status=bogus"] {
        let (status, _) = internal_call(
            api,
            Method::GET,
            &format!("/internal/agent/offers{bad}"),
            &g,
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    }

    let (status, body) = internal_call(
        api,
        Method::GET,
        &format!("/internal/agent/products/{ASIN}/price-history?days=90"),
        &g,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["stats"]["listings"][0]["windows"][1]["low_centavos"],
        45_000
    );
    assert_eq!(
        body["history"]["listings"][0]["points"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let (status, body) = internal_call(
        api,
        Method::POST,
        "/internal/agent/roi",
        &g,
        json!({ "items": [{
            "sell_price_centavos": 90_000, "buy_price_centavos": 50_000, "amazon_fees_centavos": 15_000
        }]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["results"][0]["profit_centavos"], 25_000);

    let suggest = |sku: &str, reason: &str| json!({ "retailer": "liverpool", "retailer_sku": sku, "asin": ASIN, "decision": "reject", "reason": reason });
    let path = "/internal/agent/match-suggestions";
    let (status, _) =
        internal_call(api, Method::POST, path, &g, suggest("L1", &"x".repeat(301))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, body) = internal_call(api, Method::POST, path, &g, suggest("NOPE", "no")).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::CONFLICT, Some("not_reviewable"))
    );
    let (status, body) =
        internal_call(api, Method::POST, path, &g, suggest("L1", "Otro color")).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["decision"], "reject");
    let stored = w.decisions.suggestions.lock().unwrap();
    assert_eq!(stored[0].1.tenant.as_str(), "user-1");
    assert_eq!(stored[0].1.chat_id.as_str(), "chat-test-1");
}

#[tokio::test]
async fn usage_route_returns_the_authoritative_remainder() {
    let w = world(Options::default()).await;
    signed_in(&w.app, "a@example.com").await;
    let api = &w.app.router;
    let g = grant_for(1, "user-1", NOW);
    let path = "/internal/agent/usage";
    let (status, body) = internal_call(
        api,
        Method::POST,
        path,
        &g,
        json!({ "input_tokens": 300, "output_tokens": 100 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["remaining_tokens"], FREE_CAP - 400);
    let (_, body) = internal_call(
        api,
        Method::POST,
        path,
        &g,
        json!({ "input_tokens": 900, "output_tokens": 0 }),
    )
    .await;
    assert_eq!(body["remaining_tokens"], 0);
    let (status, _) = internal_call(
        api,
        Method::POST,
        path,
        &g,
        json!({ "input_tokens": -1, "output_tokens": 0 }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = internal_call(
        api,
        Method::POST,
        path,
        &g,
        json!({ "input_tokens": 2_000_000, "output_tokens": 0 }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

/// Collects formatted log lines.
#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn secrets_never_reach_logs_or_responses() {
    let logs = LogBuffer::default();
    let writer = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let mut bodies = Vec::new();
    for mode in [Mode::EchoSecret, Mode::Garbage, Mode::Reply, Mode::Suggest] {
        let w = world(Options {
            mode: Some(mode),
            ..Options::default()
        })
        .await;
        let mut user = signed_in(&w.app, "a@example.com").await;
        bodies.push(chat(&mut user, ask("hola")).await.body.to_string());
        let (_, body) = internal_call(
            &w.app.router,
            Method::GET,
            "/internal/agent/offers",
            "forged",
            Value::Null,
        )
        .await;
        bodies.push(body.to_string());
    }
    tracing::info!("marker");
    let mut text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(text.contains("marker"), "capture works");
    assert!(text.contains("the assistant answered an error"), "{text}");
    text.push_str(&bodies.join("\n"));
    assert!(!text.contains(SERVICE_TOKEN), "service token leaked");
    assert!(!text.contains(GRANT_KEY), "grant key leaked");
    let _ = std::io::stdout().flush();
}
