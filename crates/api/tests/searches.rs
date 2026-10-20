//! Tests for `POST /searches` and `GET /searches/{id}`: session auth (401),
//! tenant isolation (404), membership hiding, 202/422/400/500/503 and the
//! response shape, over a fake search repo.

mod common;

use std::sync::{Arc, Mutex};

use api::{ProductsState, SearchesState};
use app::{
    PortFuture, RepoError, ResultMatchView, RetailerOutcomeView, SearchId, SearchRepo,
    SearchResultView, SearchResults, SearchView, StartSearch,
};
use axum::http::{Method, StatusCode};
use common::{Client, TestApp, generous_limits};
use domain::{
    Asin, Availability, Confidence, MatchStatus, PriceMxn, RetailerId, RetailerSearchOutcome,
    RetailerSku, SearchQuery, SearchStatus, TenantId, UnixMillis,
};
use serde_json::{Value, json};

/// Stores `(tenant, query)` pairs; ids start at 1. `fail` makes every call
/// fail with `Unavailable`.
#[derive(Default)]
struct FakeRepo {
    rows: Mutex<Vec<(String, String)>>,
    fail: bool,
}

fn result(retailer: &str, sku: &str, membership_required: bool) -> SearchResultView {
    SearchResultView {
        retailer: RetailerId::new(retailer).unwrap(),
        retailer_sku: RetailerSku::new(sku).unwrap(),
        title: "Taladro".to_owned(),
        url: format!("https://example.com/{retailer}/{sku}"),
        price_mxn: PriceMxn::from_centavos(50_000).unwrap(),
        availability: Availability::InStock,
        membership_required,
        amazon: None,
    }
}

fn view(id: i64, query: &str) -> SearchView {
    let mut liverpool = result("liverpool", "123", false);
    liverpool.url = "https://www.liverpool.com.mx/tienda/pdp/taladro/123".to_owned();
    liverpool.amazon = Some(ResultMatchView {
        asin: Asin::new("B0CHHSFMRL").unwrap(),
        status: MatchStatus::Accepted,
        confidence: Confidence::from_bp(9_500).unwrap(),
        amazon_price_mxn: Some(PriceMxn::from_centavos(90_000).unwrap()),
        fees_mxn: Some(PriceMxn::from_centavos(15_000).unwrap()),
        profit_centavos: Some(25_000),
        roi_bp: Some(5_000),
        margin_bp: Some(2_777),
    });
    SearchView {
        id: SearchId(id),
        query: query.to_owned(),
        status: SearchStatus::Done,
        reason: String::new(),
        created_at: UnixMillis(1_000),
        finished_at: Some(UnixMillis(2_000)),
        retailers: vec![
            RetailerOutcomeView {
                retailer: RetailerId::new("liverpool").unwrap(),
                outcome: RetailerSearchOutcome::Ok,
                offers: 1,
            },
            RetailerOutcomeView {
                retailer: RetailerId::new("sears_mx").unwrap(),
                outcome: RetailerSearchOutcome::CategoryOnly,
                offers: 0,
            },
        ],
        results: vec![
            liverpool,
            result("costco_mx", "C1", true),
            result("sams_mx", "S1", true),
        ],
        hidden_results: 0,
    }
}

impl SearchRepo for FakeRepo {
    fn create_and_enqueue<'a>(
        &'a self,
        tenant: &'a TenantId,
        query: &'a SearchQuery,
    ) -> PortFuture<'a, Result<SearchId, RepoError>> {
        Box::pin(async move {
            if self.fail {
                return Err(RepoError::Unavailable);
            }
            let mut rows = self.rows.lock().unwrap();
            rows.push((tenant.as_str().to_owned(), query.as_str().to_owned()));
            Ok(SearchId(i64::try_from(rows.len()).unwrap()))
        })
    }

    fn load<'a>(
        &'a self,
        tenant: &'a TenantId,
        id: SearchId,
    ) -> PortFuture<'a, Result<Option<SearchView>, RepoError>> {
        Box::pin(async move {
            if self.fail {
                return Err(RepoError::Unavailable);
            }
            let rows = self.rows.lock().unwrap();
            let row = usize::try_from(id.0 - 1)
                .ok()
                .and_then(|i| rows.get(i))
                .filter(|(t, _)| t == tenant.as_str());
            Ok(row.map(|(_, q)| view(id.0, q)))
        })
    }
}

fn app(repo: &Arc<FakeRepo>) -> TestApp {
    TestApp::new(
        ProductsState::disabled(),
        SearchesState::new(
            StartSearch::new(repo.clone()),
            SearchResults::new(repo.clone()),
        ),
        generous_limits(),
    )
}

async fn start(client: &mut Client, query: Value) -> common::Reply {
    client.send(Method::POST, "/searches", Some(query)).await
}

fn query(q: &str) -> Value {
    json!({ "query": q })
}

#[tokio::test]
async fn without_a_session_both_routes_are_401() {
    let repo = Arc::new(FakeRepo::default());
    let app = app(&repo);
    let mut client = app.client();
    client.fetch_csrf().await;
    let reply = start(&mut client, query("taladro")).await;
    assert_eq!(
        (reply.status, reply.error()),
        (StatusCode::UNAUTHORIZED, "unauthenticated")
    );
    assert_eq!(
        client.get("/searches/1").await.status,
        StatusCode::UNAUTHORIZED
    );
    assert!(repo.rows.lock().unwrap().is_empty());
}

#[tokio::test]
async fn post_without_csrf_is_403_before_anything_is_enqueued() {
    let repo = Arc::new(FakeRepo::default());
    let app = app(&repo);
    let mut client = app.client();
    client.signed_up("ana@example.com").await;
    client.csrf = None;
    let reply = start(&mut client, query("taladro")).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(repo.rows.lock().unwrap().is_empty());
}

#[tokio::test]
async fn post_enqueues_a_cleaned_query_for_the_users_tenant() {
    let repo = Arc::new(FakeRepo::default());
    let app = app(&repo);
    let mut client = app.client();
    client.signed_up("ana@example.com").await;
    let reply = start(&mut client, query("  taladro   inalámbrico ")).await;
    assert_eq!(reply.status, StatusCode::ACCEPTED);
    assert_eq!(reply.body, json!({"id": 1}));
    assert_eq!(
        *repo.rows.lock().unwrap(),
        vec![("user-1".to_owned(), "taladro inalámbrico".to_owned())]
    );
}

#[tokio::test]
async fn bad_queries_are_422_and_bad_json_is_400() {
    let repo = Arc::new(FakeRepo::default());
    let app = app(&repo);
    let mut client = app.client();
    client.signed_up("ana@example.com").await;
    let long = "a".repeat(101);
    for body in [query(""), query("   "), query(&long), json!({})] {
        let reply = start(&mut client, body.clone()).await;
        assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }
    let request = axum::http::Request::post("/searches")
        .header("content-type", "application/json")
        .header("origin", common::ORIGIN)
        .header(api::CSRF_HEADER, client.csrf.clone().unwrap())
        .body(axum::body::Body::from("{"))
        .unwrap();
    let reply = client.raw(request).await;
    assert_eq!(
        (reply.status, reply.error()),
        (StatusCode::BAD_REQUEST, "invalid_json")
    );
    assert!(repo.rows.lock().unwrap().is_empty());
}

#[tokio::test]
async fn get_returns_outcomes_roi_and_hides_membership_offers() {
    let repo = Arc::new(FakeRepo::default());
    let app = app(&repo);
    let mut client = app.client();
    client.signed_up("ana@example.com").await;
    start(&mut client, query("taladro")).await;
    let reply = client.get("/searches/1").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.body,
        json!({
            "id": 1, "query": "taladro", "status": "done", "reason": null,
            "created_at_ms": 1000, "finished_at_ms": 2000,
            "retailers": [
                {"retailer": "liverpool", "outcome": "ok", "offers": 1},
                {"retailer": "sears_mx", "outcome": "category_only", "offers": 0}
            ],
            "results": [{
                "retailer": "liverpool", "retailer_sku": "123", "title": "Taladro",
                "url": "https://www.liverpool.com.mx/tienda/pdp/taladro/123",
                "price_centavos": 50000, "availability": "in_stock",
                "membership_required": false,
                "amazon": {
                    "asin": "B0CHHSFMRL", "match_status": "accepted", "confidence_bp": 9500,
                    "amazon_price_centavos": 90000, "fees_centavos": 15000,
                    "profit_centavos": 25000, "roi_bp": 5000, "margin_bp": 2777
                }
            }],
            "hidden_results": 2
        })
    );
}

fn retailers(reply: &common::Reply) -> Vec<String> {
    reply.body["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["retailer"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn memberships_from_onboarding_reveal_their_store_only() {
    let repo = Arc::new(FakeRepo::default());
    let app = app(&repo);
    let mut client = app.client();
    client.signed_up("ana@example.com").await;
    start(&mut client, query("taladro")).await;
    let reply = client
        .send(
            Method::PUT,
            "/me/onboarding",
            Some(json!({"has_costco": true, "has_sams": false})),
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    let reply = client.get("/searches/1").await;
    assert_eq!(retailers(&reply), ["liverpool", "costco_mx"]);
    assert_eq!(reply.body["hidden_results"], 1);

    client
        .send(
            Method::PUT,
            "/me/onboarding",
            Some(json!({"has_costco": true, "has_sams": true})),
        )
        .await;
    let reply = client.get("/searches/1").await;
    assert_eq!(retailers(&reply), ["liverpool", "costco_mx", "sams_mx"]);
    assert_eq!(reply.body["hidden_results"], 0);
}

#[tokio::test]
async fn another_users_search_and_bad_ids_are_404() {
    let repo = Arc::new(FakeRepo::default());
    let app = app(&repo);
    let mut ana = app.client();
    ana.signed_up("ana@example.com").await;
    start(&mut ana, query("taladro")).await;

    let mut bea = app.client();
    bea.signed_up("bea@example.com").await;
    for id in ["1", "2", "0", "-1", "abc", "99999999999999999999"] {
        let reply = bea.get(&format!("/searches/{id}")).await;
        assert_eq!(
            (reply.status, reply.error()),
            (StatusCode::NOT_FOUND, "not_found"),
            "{id}"
        );
    }
    assert_eq!(ana.get("/searches/1").await.status, StatusCode::OK);
}

#[tokio::test]
async fn storage_failure_is_500_without_detail() {
    let repo = Arc::new(FakeRepo {
        fail: true,
        ..FakeRepo::default()
    });
    let app = app(&repo);
    let mut client = app.client();
    client.signed_up("ana@example.com").await;
    let reply = start(&mut client, query("taladro")).await;
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(reply.body, json!({"error": "internal"}));
    let reply = client.get("/searches/1").await;
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(reply.body, json!({"error": "internal"}));
}

#[tokio::test]
async fn without_a_database_search_routes_are_503() {
    let app = TestApp::accounts_only();
    let mut client = app.client();
    client.signed_up("ana@example.com").await;
    let reply = start(&mut client, query("taladro")).await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        client.get("/searches/1").await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn spec_documents_the_search_routes_as_session_routes() {
    let spec = serde_json::to_value(api::openapi()).unwrap();
    assert_eq!(
        spec["paths"]["/searches"]["post"]["security"],
        json!([{ api::SESSION_SCHEME: [], api::CSRF_SCHEME: [] }])
    );
    assert_eq!(
        spec["paths"]["/searches/{id}"]["get"]["security"],
        json!([{ api::SESSION_SCHEME: [] }])
    );
    let result = &spec["components"]["schemas"]["SearchResultResponse"]["properties"];
    assert!(result["availability"].is_object());
    assert!(result["membership_required"].is_object());
    for schema in [
        "SearchResponse",
        "SearchCreatedResponse",
        "StartSearchRequest",
    ] {
        assert!(
            spec["components"]["schemas"][schema].is_object(),
            "{schema}"
        );
    }
}
