//! Tests for `GET /products/{asin}/price-stats` and
//! `GET /products/{asin}/price-history`: shape and values on fixed fixture
//! data with an injected clock, the stored Amazon name, plus
//! 401/400/404/422/503/500.

mod common;

use std::sync::Arc;

use api::{ProductsState, SearchesState};
use app::{
    Clock, MatchRepo, PortFuture, PriceHistoryRepo, PricePoint, PriceSeries, PriceStats, RepoError,
    UpsertOutcome,
};
use axum::http::{Method, StatusCode};
use common::{MemoryProducts, TestApp, generous_limits};
use domain::{
    Asin, Availability, DAY_MS, Offer, PriceMxn, ProductMatch, RetailerId, RetailerSku, UnixMillis,
};
use serde_json::{Value, json};

const NOW: i64 = 1_760_000_000_000;
const ASIN: &str = "B0CHHSFMRL";

struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> UnixMillis {
        UnixMillis(NOW)
    }
}

/// The same fixture history for every listing; `history` honours `since`.
struct FakePrices(Result<Vec<PricePoint>, RepoError>);

impl PriceHistoryRepo for FakePrices {
    fn record<'a>(&'a self, _: &'a Offer) -> PortFuture<'a, Result<(), RepoError>> {
        Box::pin(async { Ok(()) })
    }

    fn history<'a>(
        &'a self,
        _: &'a RetailerId,
        _: &'a RetailerSku,
        since: UnixMillis,
    ) -> PortFuture<'a, Result<Vec<PricePoint>, RepoError>> {
        Box::pin(async move {
            Ok(self
                .0
                .clone()?
                .into_iter()
                .filter(|p| p.observed_at >= since)
                .collect())
        })
    }
}

/// The accepted listings of [`ASIN`]; other ASINs have no match.
struct FakeMatches(Vec<(RetailerId, RetailerSku)>);

impl MatchRepo for FakeMatches {
    fn upsert<'a>(
        &'a self,
        _: &'a ProductMatch,
    ) -> PortFuture<'a, Result<UpsertOutcome, RepoError>> {
        Box::pin(async { Ok(UpsertOutcome::Written) })
    }

    fn accepted_listings<'a>(
        &'a self,
        asin: &'a Asin,
    ) -> PortFuture<'a, Result<Vec<(RetailerId, RetailerSku)>, RepoError>> {
        let known = asin.as_str() == ASIN;
        Box::pin(async move { Ok(if known { self.0.clone() } else { vec![] }) })
    }

    fn has_any_match<'a>(&'a self, asin: &'a Asin) -> PortFuture<'a, Result<bool, RepoError>> {
        let known = asin.as_str() == ASIN && !self.0.is_empty();
        Box::pin(async move { Ok(known) })
    }
}

fn point(centavos: i64, days_ago: i64, availability: Availability) -> PricePoint {
    PricePoint {
        price_mxn: PriceMxn::from_centavos(centavos).unwrap(),
        list_price_mxn: None,
        availability,
        observed_at: UnixMillis(NOW - days_ago * DAY_MS),
    }
}

fn fixture() -> Vec<PricePoint> {
    vec![
        point(100_000, 200, Availability::InStock),
        point(90_000, 60, Availability::InStock),
        point(10_000, 20, Availability::OutOfStock),
        point(110_000, 10, Availability::InStock),
        point(105_000, 1, Availability::InStock),
    ]
}

fn listing(retailer: &str, sku: &str) -> (RetailerId, RetailerSku) {
    (
        RetailerId::new(retailer).unwrap(),
        RetailerSku::new(sku).unwrap(),
    )
}

fn enabled(prices: Result<Vec<PricePoint>, RepoError>) -> ProductsState {
    enabled_with(prices, vec![listing("walmart_mx", "00750105530007")])
}

fn enabled_with(
    prices: Result<Vec<PricePoint>, RepoError>,
    listings: Vec<(RetailerId, RetailerSku)>,
) -> ProductsState {
    with_products(prices, listings, MemoryProducts::default())
}

fn with_products(
    prices: Result<Vec<PricePoint>, RepoError>,
    listings: Vec<(RetailerId, RetailerSku)>,
    products: MemoryProducts,
) -> ProductsState {
    let prices = Arc::new(FakePrices(prices));
    let matches = Arc::new(FakeMatches(listings));
    let products = Arc::new(products);
    ProductsState::new(
        Arc::new(PriceStats::new(
            prices.clone(),
            matches.clone(),
            Arc::new(common::MemoryDecisions::default()),
            products.clone(),
            Arc::new(FixedClock),
        )),
        Arc::new(PriceSeries::new(
            prices,
            matches,
            Arc::new(common::MemoryDecisions::default()),
            products,
            Arc::new(FixedClock),
        )),
    )
}

/// `GET path` as a signed-in user.
async fn get(state: ProductsState, path: &str) -> common::Reply {
    let app = TestApp::new(state, SearchesState::disabled(), generous_limits());
    let mut client = app.client();
    client.signed_up("ana@example.com").await;
    client.get(path).await
}

async fn call(state: ProductsState, asin: &str) -> (StatusCode, Value) {
    let reply = get(state, &format!("/products/{asin}/price-stats")).await;
    (reply.status, reply.body)
}

/// Start of the UTC day `days_ago` days before `NOW`.
const fn day(days_ago: i64) -> i64 {
    (NOW - days_ago * DAY_MS).div_euclid(DAY_MS) * DAY_MS
}

#[tokio::test]
async fn returns_windows_from_fixture_and_clock() {
    let (status, body) = call(enabled(Ok(fixture())), ASIN).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({
            "asin": ASIN,
            "title": null,
            "brand": null,
            "as_of_ms": NOW,
            "listings": [{
                "retailer": "walmart_mx",
                "retailer_sku": "00750105530007",
                "windows": [
                    {"days": 30, "low_centavos": 105000, "high_centavos": 110000,
                     "average_centavos": 107500, "observations": 2},
                    {"days": 90, "low_centavos": 90000, "high_centavos": 110000,
                     "average_centavos": 101667, "observations": 3},
                    {"days": 365, "low_centavos": 90000, "high_centavos": 110000,
                     "average_centavos": 101250, "observations": 4}
                ]
            }],
            "hidden_listings": 0
        })
    );
}

#[tokio::test]
async fn empty_windows_have_null_prices() {
    let (status, body) = call(enabled(Ok(vec![])), &ASIN.to_lowercase()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["asin"], ASIN);
    assert_eq!(
        body["listings"][0]["windows"][0],
        json!({"days": 30, "low_centavos": null, "high_centavos": null,
               "average_centavos": null, "observations": 0})
    );
}

fn retailers(body: &Value) -> Vec<&str> {
    body["listings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["retailer"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn membership_listings_are_hidden_unless_the_user_has_it() {
    let state = enabled_with(
        Ok(fixture()),
        vec![
            listing("sams_mx", "S1"),
            listing("costco_mx", "C1"),
            listing("walmart_mx", "W1"),
        ],
    );
    let app = TestApp::new(state, SearchesState::disabled(), generous_limits());
    let mut client = app.client();
    client.signed_up("ana@example.com").await;
    let paths = [
        format!("/products/{ASIN}/price-stats"),
        format!("/products/{ASIN}/price-history"),
    ];
    for path in &paths {
        let reply = client.get(path).await;
        assert_eq!(reply.status, StatusCode::OK, "{path}");
        assert_eq!(retailers(&reply.body), ["walmart_mx"], "{path}");
        assert_eq!(reply.body["hidden_listings"], 2, "{path}");
    }

    let reply = client
        .send(
            Method::PUT,
            "/me/onboarding",
            Some(json!({"has_costco": true, "has_sams": false})),
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    for path in &paths {
        let reply = client.get(path).await;
        assert_eq!(
            retailers(&reply.body),
            ["costco_mx", "walmart_mx"],
            "{path}"
        );
        assert_eq!(reply.body["hidden_listings"], 1, "{path}");
    }
}

#[tokio::test]
async fn both_routes_return_the_stored_amazon_name() {
    for path in ["price-stats", "price-history"] {
        let state = with_products(
            Ok(fixture()),
            vec![listing("walmart_mx", "W1")],
            MemoryProducts::named(ASIN, "Licuadora Oster 600 W", Some("Oster")),
        );
        let reply = get(state, &format!("/products/{ASIN}/{path}")).await;
        assert_eq!(reply.status, StatusCode::OK, "{path}");
        assert_eq!(reply.body["title"], "Licuadora Oster 600 W", "{path}");
        assert_eq!(reply.body["brand"], "Oster", "{path}");
    }
}

#[tokio::test]
async fn a_stored_name_alone_makes_the_product_known() {
    // Named but no listing yet: 200 with no listings, not 404.
    let state = with_products(
        Ok(fixture()),
        vec![],
        MemoryProducts::named(ASIN, "Licuadora", None),
    );
    let reply = get(state, &format!("/products/{ASIN}/price-history")).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body["title"], "Licuadora");
    assert_eq!(reply.body["brand"], Value::Null);
    assert_eq!(reply.body["listings"], json!([]));
}

#[tokio::test]
async fn unknown_asin_is_404_product_not_found_on_both_routes() {
    for path in ["price-stats", "price-history"] {
        let reply = get(
            enabled(Ok(fixture())),
            &format!("/products/B0ZZZZZZZZ/{path}"),
        )
        .await;
        assert_eq!(
            (reply.status, reply.error()),
            (StatusCode::NOT_FOUND, "product_not_found"),
            "{path}"
        );
    }
    // Days are checked first: a bad `days` is still 422 for an unknown ASIN.
    let reply = get(
        enabled(Ok(fixture())),
        "/products/B0ZZZZZZZZ/price-history?days=0",
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn invalid_asin_is_400() {
    let (status, _) = call(enabled(Ok(fixture())), "not-an-asin").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn without_database_is_503() {
    let (status, _) = call(ProductsState::disabled(), ASIN).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn storage_failure_is_500_without_detail() {
    let (status, body) = call(enabled(Err(RepoError::Unavailable)), ASIN).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body, json!({"error": "internal"}));
}

#[tokio::test]
async fn product_routes_need_a_session() {
    let app = TestApp::new(
        enabled(Ok(fixture())),
        SearchesState::disabled(),
        generous_limits(),
    );
    let mut client = app.client();
    for path in ["price-stats", "price-history"] {
        let reply = client.get(&format!("/products/{ASIN}/{path}")).await;
        assert_eq!(
            (reply.status, reply.error()),
            (StatusCode::UNAUTHORIZED, "unauthenticated"),
            "{path}"
        );
    }
}

#[tokio::test]
async fn history_returns_one_point_per_day_oldest_first() {
    let reply = get(
        enabled(Ok(fixture())),
        &format!("/products/{ASIN}/price-history?days=30"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.body,
        json!({
            "asin": ASIN,
            "title": null,
            "brand": null,
            "since_ms": NOW - 30 * DAY_MS,
            "as_of_ms": NOW,
            "listings": [{
                "retailer": "walmart_mx",
                "retailer_sku": "00750105530007",
                "points": [
                    {"day_ms": day(20), "low_centavos": 10000, "high_centavos": 10000, "in_stock": false},
                    {"day_ms": day(10), "low_centavos": 110000, "high_centavos": 110000, "in_stock": true},
                    {"day_ms": day(1), "low_centavos": 105000, "high_centavos": 105000, "in_stock": true}
                ]
            }],
            "hidden_listings": 0
        })
    );
}

#[tokio::test]
async fn history_defaults_to_365_days() {
    let reply = get(
        enabled(Ok(fixture())),
        &format!("/products/{ASIN}/price-history"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body["since_ms"], NOW - 365 * DAY_MS);
    let points = reply.body["listings"][0]["points"].as_array().unwrap();
    assert_eq!(points.len(), 5);
    assert_eq!(points[0]["day_ms"], day(200));
}

#[tokio::test]
async fn history_rejects_bad_days_asin_and_storage_errors() {
    for (query, status, code) in [
        ("days=0", StatusCode::UNPROCESSABLE_ENTITY, "invalid_days"),
        ("days=366", StatusCode::UNPROCESSABLE_ENTITY, "invalid_days"),
        ("days=abc", StatusCode::BAD_REQUEST, "invalid_query"),
        ("days=-1", StatusCode::BAD_REQUEST, "invalid_query"),
    ] {
        let reply = get(
            enabled(Ok(fixture())),
            &format!("/products/{ASIN}/price-history?{query}"),
        )
        .await;
        assert_eq!((reply.status, reply.error()), (status, code), "{query}");
    }
    let reply = get(enabled(Ok(fixture())), "/products/nope/price-history").await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    let reply = get(
        enabled(Err(RepoError::Unavailable)),
        &format!("/products/{ASIN}/price-history"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(reply.body, json!({"error": "internal"}));
    let reply = get(
        ProductsState::disabled(),
        &format!("/products/{ASIN}/price-history"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn spec_documents_the_history_route() {
    let spec = serde_json::to_value(api::openapi()).unwrap();
    let route = &spec["paths"]["/products/{asin}/price-history"]["get"];
    assert_eq!(route["security"], json!([{ api::SESSION_SCHEME: [] }]));
    let params: Vec<&str> = route["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(params, ["asin", "days"]);
    let days = &route["parameters"][1];
    assert_eq!(days["in"], "query");
    assert_eq!(days["required"], false);
    assert!(spec["components"]["schemas"]["PriceHistoryResponse"].is_object());
    for path in ["price-history", "price-stats"] {
        let route = &spec["paths"][format!("/products/{{asin}}/{path}")]["get"];
        assert!(route["responses"]["404"].is_object(), "{path}");
    }
    let props = &spec["components"]["schemas"]["PriceHistoryResponse"]["properties"];
    assert!(props["title"].is_object() && props["brand"].is_object());
}
