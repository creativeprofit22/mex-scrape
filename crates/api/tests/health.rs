//! Integration tests for `GET /health` and the published OpenAPI spec.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

#[tokio::test]
async fn health_returns_ok() {
    let (router, _) = api::router();
    let response = router
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json, serde_json::json!({ "status": "ok" }));
}

#[tokio::test]
async fn spec_is_served_and_documents_health() {
    let (router, _) = api::router();
    let response = router
        .oneshot(Request::get(api::OPENAPI_PATH).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["paths"]["/health"]["get"].is_object());
    assert!(json["components"]["schemas"]["HealthResponse"].is_object());
}
