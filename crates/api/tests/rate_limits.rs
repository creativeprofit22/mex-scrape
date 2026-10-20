//! Tests for the per-plan and per-IP rate limits: 429 `rate_limited` with
//! `Retry-After`, separate buckets per user, per plan and per client IP,
//! and only the configured client-IP header read (never X-Forwarded-For).

mod common;

use api::{ProductsState, Quota, RateLimits, SearchesState};
use axum::http::{HeaderName, Method, StatusCode, header};
use common::{Client, TestApp};
use domain::Plan;

/// Anonymous burst 4: enough for CSRF, sign-up and a spare request.
fn limits(client_ip_header: Option<&'static str>) -> RateLimits {
    RateLimits {
        free: Quota::new(1, 2),
        pro: Quota::new(1, 5),
        anonymous: Quota::new(1, 4),
        client_ip_header: client_ip_header.map(HeaderName::from_static),
    }
}

fn app_with(client_ip_header: Option<&'static str>) -> TestApp {
    TestApp::new(
        ProductsState::disabled(),
        SearchesState::disabled(),
        limits(client_ip_header),
    )
}

/// `GET /auth/csrf` with extra `headers`.
async fn csrf_with(client: &mut Client, headers: &[(&str, &str)]) -> common::Reply {
    let mut request = axum::http::Request::get("/auth/csrf");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    client
        .raw(request.body(axum::body::Body::empty()).unwrap())
        .await
}

/// Spends the anonymous burst of `client`'s peer address.
async fn exhaust_peer(client: &mut Client) {
    for _ in 0..4 {
        assert_eq!(client.get("/auth/csrf").await.status, StatusCode::OK);
    }
    assert_limited(&client.get("/auth/csrf").await);
}

/// Statuses of `n` `GET /me` calls.
async fn me_statuses(client: &mut Client, n: usize) -> Vec<StatusCode> {
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(client.get("/me").await.status);
    }
    out
}

fn assert_limited(reply: &common::Reply) {
    assert_eq!(
        (reply.status, reply.error()),
        (StatusCode::TOO_MANY_REQUESTS, "rate_limited")
    );
    let retry: u64 = reply.headers[header::RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=60).contains(&retry), "{retry}");
}

#[tokio::test]
async fn free_users_get_429_with_retry_after_once_the_burst_is_spent() {
    let app = app_with(None);
    let mut ana = app.client_from([10, 0, 0, 1]);
    ana.signed_up("ana@example.com").await;
    assert_eq!(
        me_statuses(&mut ana, 2).await,
        [StatusCode::OK, StatusCode::OK]
    );
    assert_limited(&ana.get("/me").await);
    // Mutating routes share the bucket.
    let reply = ana
        .send(
            Method::PUT,
            "/me/locale",
            Some(serde_json::json!({"locale": "en"})),
        )
        .await;
    assert_limited(&reply);

    // Another free user (same IP) has their own bucket.
    let mut bea = app.client_from([10, 0, 0, 1]);
    bea.signed_up("bea@example.com").await;
    assert_eq!(bea.get("/me").await.status, StatusCode::OK);
}

#[tokio::test]
async fn pro_users_get_the_larger_quota() {
    let app = app_with(None);
    let mut ana = app.client_from([10, 0, 0, 2]);
    ana.signed_up("ana@example.com").await;
    app.users.set_plan(1, Plan::Pro);
    assert_eq!(me_statuses(&mut ana, 5).await, [StatusCode::OK; 5]);
    assert_limited(&ana.get("/me").await);
}

#[tokio::test]
async fn anonymous_requests_are_limited_per_client_ip() {
    let app = app_with(None);
    let mut first = app.client_from([10, 0, 0, 3]);
    let mut statuses = Vec::new();
    for _ in 0..4 {
        statuses.push(first.get("/auth/csrf").await.status);
    }
    assert_eq!(statuses, [StatusCode::OK; 4]);
    assert_limited(&first.get("/auth/csrf").await);
    // Sign-in attempts from that IP are refused before credentials are checked.
    first.csrf = Some("x".repeat(44));
    assert_limited(&first.sign_in("ana@example.com", common::PASSWORD).await);

    let mut other_ip = app.client_from([10, 0, 0, 4]);
    assert_eq!(other_ip.get("/auth/csrf").await.status, StatusCode::OK);
}

#[tokio::test]
async fn forwarded_headers_are_ignored_without_a_client_ip_header() {
    let app = app_with(None);
    let mut client = app.client_from([10, 0, 0, 5]);
    exhaust_peer(&mut client).await;
    for forged in [
        ("x-forwarded-for", "203.0.113.9"),
        ("x-real-ip", "203.0.113.9"),
    ] {
        assert_limited(&csrf_with(&mut client, &[forged]).await);
    }
}

#[tokio::test]
async fn forged_x_forwarded_for_is_ignored_when_only_x_real_ip_is_trusted() {
    let app = app_with(Some("x-real-ip"));
    let mut client = app.client_from([10, 0, 0, 6]);
    exhaust_peer(&mut client).await;
    for forged in [
        ("x-forwarded-for", "203.0.113.9"),
        ("forwarded", "for=203.0.113.9"),
    ] {
        assert_limited(&csrf_with(&mut client, &[forged]).await);
    }
}

#[tokio::test]
async fn the_configured_client_ip_header_picks_the_bucket() {
    let app = app_with(Some("x-real-ip"));
    // Every browser arrives from the web proxy's address.
    let mut proxy = app.client_from([10, 0, 0, 7]);
    let first = [("x-real-ip", "203.0.113.10")];
    for _ in 0..4 {
        assert_eq!(csrf_with(&mut proxy, &first).await.status, StatusCode::OK);
    }
    assert_limited(&csrf_with(&mut proxy, &first).await);
    // Another client behind the same proxy keeps its own bucket.
    let other = [("x-real-ip", "203.0.113.11")];
    assert_eq!(csrf_with(&mut proxy, &other).await.status, StatusCode::OK);
}

#[tokio::test]
async fn a_missing_or_invalid_client_ip_header_falls_back_to_the_peer() {
    let app = app_with(Some("x-real-ip"));
    let mut client = app.client_from([10, 0, 0, 8]);
    exhaust_peer(&mut client).await;
    assert_limited(&csrf_with(&mut client, &[("x-real-ip", "203.0.113.12, 198.51.100.1")]).await);
    assert_limited(&csrf_with(&mut client, &[("x-real-ip", "not-an-ip")]).await);
    let mut other_peer = app.client_from([10, 0, 0, 9]);
    assert_eq!(other_peer.get("/auth/csrf").await.status, StatusCode::OK);
}

#[tokio::test]
async fn spec_documents_429_on_user_routes() {
    let spec = serde_json::to_value(api::openapi()).unwrap();
    for (path, method) in [
        ("/auth/signin", "post"),
        ("/auth/signup", "post"),
        ("/me", "get"),
        ("/searches", "post"),
        ("/roi", "post"),
        ("/products/{asin}/price-history", "get"),
    ] {
        assert!(
            spec["paths"][path][method]["responses"]["429"].is_object(),
            "{method} {path}"
        );
    }
}
