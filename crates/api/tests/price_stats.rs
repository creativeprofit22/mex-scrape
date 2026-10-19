//! Tests for `GET /products/{asin}/price-stats`: shape and values on fixed
//! fixture data with an injected clock, plus 400/503/500.

use std::sync::Arc;

use api::{AdminState, ProductsState};
use app::{
    Clock, MatchRepo, PortFuture, PriceHistoryRepo, PricePoint, PriceStats, RepoError,
    UpsertOutcome,
};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use domain::{
    Asin, Availability, DAY_MS, Offer, PriceMxn, ProductMatch, RetailerId, RetailerSku, UnixMillis,
};
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use tower::ServiceExt as _;

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

struct FakeMatches;

impl MatchRepo for FakeMatches {
    fn upsert<'a>(
        &'a self,
        _: &'a ProductMatch,
    ) -> PortFuture<'a, Result<UpsertOutcome, RepoError>> {
        Box::pin(async { Ok(UpsertOutcome::Written) })
    }

    fn accepted_listings<'a>(
        &'a self,
        _: &'a Asin,
    ) -> PortFuture<'a, Result<Vec<(RetailerId, RetailerSku)>, RepoError>> {
        Box::pin(async {
            Ok(vec![(
                RetailerId::new("walmart_mx").unwrap(),
                RetailerSku::new("00750105530007").unwrap(),
            )])
        })
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

fn enabled(prices: Result<Vec<PricePoint>, RepoError>) -> ProductsState {
    ProductsState::new(Arc::new(PriceStats::new(
        Arc::new(FakePrices(prices)),
        Arc::new(FakeMatches),
        Arc::new(FixedClock),
    )))
}

async fn call(state: ProductsState, asin: &str) -> (StatusCode, Vec<u8>) {
    let (router, _) = api::router_with_state(AdminState::disabled(), state);
    let response = router
        .oneshot(
            Request::get(format!("/products/{asin}/price-stats"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, body.to_vec())
}

#[tokio::test]
async fn returns_windows_from_fixture_and_clock() {
    let (status, body) = call(enabled(Ok(fixture())), ASIN).await;
    assert_eq!(status, StatusCode::OK);
    let got: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        got,
        json!({
            "asin": ASIN,
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
            }]
        })
    );
}

#[tokio::test]
async fn empty_windows_have_null_prices() {
    let (status, body) = call(enabled(Ok(vec![])), &ASIN.to_lowercase()).await;
    assert_eq!(status, StatusCode::OK);
    let got: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(got["asin"], ASIN);
    assert_eq!(
        got["listings"][0]["windows"][0],
        json!({"days": 30, "low_centavos": null, "high_centavos": null,
               "average_centavos": null, "observations": 0})
    );
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
    assert!(body.is_empty());
}
