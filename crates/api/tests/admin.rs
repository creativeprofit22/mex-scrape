//! Tests for `GET /admin/brightdata/access`: auth (401/503/200) and shape.

use std::sync::Arc;

use api::AdminState;
use app::{AccessCountsRepo, AccessReport, AccessTotals, HostAccess, PortFuture, RepoError};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use retailers::RetailerOfferSource;
use tower::ServiceExt as _;

const TOKEN: &str = "test-admin-token-0123456789";
const PATH: &str = "/admin/brightdata/access";

struct Fake(Result<Vec<HostAccess>, RepoError>);

impl AccessCountsRepo for Fake {
    fn counts_by_host(&self) -> PortFuture<'_, Result<Vec<HostAccess>, RepoError>> {
        Box::pin(async move { self.0.clone() })
    }
}

fn report(result: Result<Vec<HostAccess>, RepoError>) -> Arc<AccessReport> {
    Arc::new(AccessReport::new(
        Arc::new(Fake(result)),
        Arc::new(RetailerOfferSource::lookup_only()),
    ))
}

fn counts() -> Vec<HostAccess> {
    vec![
        HostAccess {
            host: "www.liverpool.com.mx".to_owned(),
            totals: AccessTotals {
                ok: 1,
                blocked_needs_kyc: 2,
                skipped_robots: 3,
                failed: 4,
            },
        },
        HostAccess {
            host: "other.example".to_owned(),
            totals: AccessTotals::default(),
        },
    ]
}

async fn call(state: AdminState, auth: Option<&str>) -> (StatusCode, Vec<u8>) {
    let (router, _) = api::router_with(state);
    let mut request = Request::get(PATH);
    if let Some(value) = auth {
        request = request.header("authorization", value);
    }
    let response = router
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, body.to_vec())
}

fn enabled(result: Result<Vec<HostAccess>, RepoError>) -> AdminState {
    AdminState::new(TOKEN, report(result))
}

#[tokio::test]
async fn unconfigured_or_short_token_fails_closed_with_503() {
    let bearer = format!("Bearer {TOKEN}");
    let (status, _) = call(AdminState::disabled(), Some(&bearer)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let short = AdminState::new("short", report(Ok(vec![])));
    let (status, _) = call(short, Some("Bearer short")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn missing_or_wrong_token_is_401() {
    for auth in [None, Some("Bearer nope"), Some(TOKEN), Some("Basic abc")] {
        let (status, body) = call(enabled(Ok(counts())), auth).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{auth:?}");
        assert!(body.is_empty());
    }
}

#[tokio::test]
async fn valid_token_returns_counts_per_retailer() {
    let bearer = format!("Bearer {TOKEN}");
    let (status, body) = call(enabled(Ok(counts())), Some(&bearer)).await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json,
        serde_json::json!([
            {"retailer": "liverpool", "host": "www.liverpool.com.mx", "ok": 1,
             "blocked_needs_kyc": 2, "skipped_robots": 3, "failed": 4},
            {"retailer": "other.example", "host": "other.example", "ok": 0,
             "blocked_needs_kyc": 0, "skipped_robots": 0, "failed": 0}
        ])
    );
}

#[tokio::test]
async fn storage_failure_is_500_without_detail() {
    let bearer = format!("Bearer {TOKEN}");
    let (status, body) = call(enabled(Err(RepoError::Unavailable)), Some(&bearer)).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body.is_empty());
}

#[tokio::test]
async fn spec_documents_the_admin_route() {
    let spec = serde_json::to_value(api::openapi()).unwrap();
    assert!(spec["paths"][PATH]["get"].is_object());
    assert!(spec["components"]["schemas"]["RetailerAccessCounts"].is_object());
    assert_eq!(
        spec["paths"][PATH]["get"]["security"],
        serde_json::json!([{ api::ADMIN_BEARER: [] }])
    );
    assert_eq!(
        spec["components"]["securitySchemes"][api::ADMIN_BEARER],
        serde_json::json!({"type": "http", "scheme": "bearer"})
    );
    assert!(spec["paths"]["/health"]["get"].get("security").is_none());
}
