//! Tests for sign-up, sign-in, sign-out, `/me`, onboarding and locale:
//! cookie flags, session rotation, CSRF and origin checks (403), and 503
//! without accounts.

mod common;

use api::{ProductsState, SearchesState};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use common::{ORIGIN, PASSWORD, TestApp};
use serde_json::json;

#[tokio::test]
async fn sign_up_creates_a_free_spanish_account_and_a_secure_session() {
    let app = TestApp::accounts_only();
    let mut client = app.client();
    let reply = client.sign_up("  Ana@Example.COM ", PASSWORD).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{reply:?}");
    assert_eq!(
        reply.body["user"],
        json!({
            "id": 1, "email": "ana@example.com", "plan": "free",
            "has_costco": false, "has_sams": false, "onboarded": false, "locale": "es"
        })
    );
    assert_eq!(reply.body["csrf_token"].as_str().unwrap().len(), 44);
    let cookie = reply.session_cookie().unwrap().to_owned();
    for flag in ["HttpOnly", "SameSite=Lax", "Secure", "Path=/", "Max-Age="] {
        assert!(cookie.contains(flag), "{flag} missing in {cookie}");
    }
    assert_eq!(app.users.get(1).tenant.as_str(), "user-1");

    let me = client.get("/me").await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.body, reply.body["user"]);
}

#[tokio::test]
async fn sign_up_rejects_bad_input_and_duplicates() {
    let app = TestApp::accounts_only();
    let mut client = app.client();
    client.signed_up("ana@example.com").await;

    let mut other = app.client();
    let cases = [
        (
            "not-an-email",
            PASSWORD,
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_email",
        ),
        (
            "bea@example.com",
            "short",
            StatusCode::UNPROCESSABLE_ENTITY,
            "weak_password",
        ),
        (
            "ANA@example.com",
            PASSWORD,
            StatusCode::CONFLICT,
            "email_taken",
        ),
    ];
    for (email, password, status, code) in cases {
        let reply = other.sign_up(email, password).await;
        assert_eq!((reply.status, reply.error()), (status, code), "{email}");
    }
    let reply = other
        .send(
            Method::POST,
            "/auth/signup",
            Some(json!({"email": "c@example.com", "password": PASSWORD, "locale": "fr"})),
        )
        .await;
    assert_eq!(reply.error(), "invalid_locale");
    let reply = other
        .send(
            Method::POST,
            "/auth/signup",
            Some(json!({"email": "c@example.com", "password": PASSWORD, "locale": "en"})),
        )
        .await;
    assert_eq!(reply.status, StatusCode::CREATED);
    assert_eq!(reply.body["user"]["locale"], "en");
}

#[tokio::test]
async fn sign_in_rotates_the_session_id_and_sign_out_ends_it() {
    let app = TestApp::accounts_only();
    client_signs_up(&app).await;

    let mut client = app.client();
    client.fetch_csrf().await;
    let anonymous_cookie = client.cookie.clone().unwrap();
    let reply = client.sign_in("ANA@example.com", PASSWORD).await;
    assert_eq!(reply.status, StatusCode::OK, "{reply:?}");
    assert_eq!(reply.body["user"]["email"], "ana@example.com");
    let signed_in_cookie = client.cookie.clone().unwrap();
    assert_ne!(anonymous_cookie, signed_in_cookie, "session id must rotate");
    let old_id = anonymous_cookie.split_once('=').unwrap().1;
    assert!(!app.sessions.ids().iter().any(|id| id == old_id));
    assert_eq!(client.get("/me").await.status, StatusCode::OK);

    let reply = client.send(Method::POST, "/auth/signout", None).await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT, "{reply:?}");
    assert!(client.cookie.is_none(), "cookie must be cleared");
    let new_id = signed_in_cookie.split_once('=').unwrap().1;
    assert!(!app.sessions.ids().iter().any(|id| id == new_id));

    // Replaying the old cookie does not bring the session back.
    client.cookie = Some(signed_in_cookie);
    let reply = client.get("/me").await;
    assert_eq!(
        (reply.status, reply.error()),
        (StatusCode::UNAUTHORIZED, "unauthenticated")
    );
}

#[tokio::test]
async fn authenticated_requests_slide_the_session_expiry() {
    let app = TestApp::accounts_only();
    let mut client = app.client();
    client.signed_up("ana@example.com").await;
    let id = client
        .cookie
        .clone()
        .unwrap()
        .split_once('=')
        .unwrap()
        .1
        .to_owned();

    // Pretend the session was signed in six days ago.
    let near_end = time::OffsetDateTime::now_utc() + time::Duration::days(1);
    app.sessions.set_expiry(&id, near_end);

    let reply = client.get("/me").await;
    assert_eq!(reply.status, StatusCode::OK, "{reply:?}");
    let cookie = reply.session_cookie().expect("session cookie refreshed");
    assert!(cookie.contains("Max-Age="), "{cookie}");
    assert_eq!(
        client.cookie.as_deref(),
        Some(format!("{}={id}", api::SESSION_COOKIE).as_str())
    );
    let floor = time::OffsetDateTime::now_utc() + time::Duration::days(api::auth::SESSION_DAYS - 1);
    assert!(
        app.sessions.expiry(&id) > floor,
        "stored expiry not moved forward"
    );
}

async fn client_signs_up(app: &TestApp) {
    app.client().signed_up("ana@example.com").await;
}

#[tokio::test]
async fn wrong_password_and_unknown_email_look_the_same() {
    let app = TestApp::accounts_only();
    client_signs_up(&app).await;
    let mut client = app.client();
    let wrong = client.sign_in("ana@example.com", "wrong password!").await;
    let unknown = client.sign_in("nobody@example.com", PASSWORD).await;
    for reply in [&wrong, &unknown] {
        assert_eq!(
            (reply.status, reply.error()),
            (StatusCode::UNAUTHORIZED, "invalid_credentials")
        );
    }
    assert_eq!(wrong.body, unknown.body);
    assert_eq!(client.get("/me").await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn mutating_requests_need_the_session_csrf_token() {
    let app = TestApp::accounts_only();
    let mut client = app.client();
    client.signed_up("ana@example.com").await;
    let body = json!({"has_costco": true, "has_sams": false});

    let token = client.csrf.take();
    let reply = client
        .send(Method::PUT, "/me/onboarding", Some(body.clone()))
        .await;
    assert_eq!(
        (reply.status, reply.error()),
        (StatusCode::FORBIDDEN, "csrf")
    );

    client.csrf = Some("x".repeat(44));
    let reply = client
        .send(Method::PUT, "/me/onboarding", Some(body.clone()))
        .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);

    // Another session's token does not work either.
    let mut other = app.client();
    client.csrf = Some(other.fetch_csrf().await);
    let reply = client
        .send(Method::PUT, "/me/onboarding", Some(body.clone()))
        .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(!app.users.get(1).onboarded);

    client.csrf = token;
    let reply = client.send(Method::PUT, "/me/onboarding", Some(body)).await;
    assert_eq!(reply.status, StatusCode::OK, "{reply:?}");
    assert!(app.users.get(1).onboarded);
}

#[tokio::test]
async fn sign_in_without_a_csrf_token_is_forbidden() {
    let app = TestApp::accounts_only();
    client_signs_up(&app).await;
    let mut client = app.client();
    let reply = client
        .send(
            Method::POST,
            "/auth/signin",
            Some(json!({"email": "ana@example.com", "password": PASSWORD})),
        )
        .await;
    assert_eq!(
        (reply.status, reply.error()),
        (StatusCode::FORBIDDEN, "csrf")
    );
    assert!(client.cookie.is_none());
}

#[tokio::test]
async fn cross_site_requests_are_forbidden_even_with_the_token() {
    let app = TestApp::accounts_only();
    let mut client = app.client();
    client.signed_up("ana@example.com").await;
    let token = client.csrf.clone().unwrap();
    let body = json!({"locale": "en"}).to_string();
    for (name, value) in [
        ("origin", "https://evil.example"),
        ("origin", "null"),
        ("sec-fetch-site", "cross-site"),
        ("sec-fetch-site", "same-site"),
    ] {
        let request = Request::put("/me/locale")
            .header(header::CONTENT_TYPE, "application/json")
            .header(api::CSRF_HEADER, &token)
            .header(name, value)
            .body(Body::from(body.clone()))
            .unwrap();
        let reply = client.raw(request).await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN, "{name}: {value}");
    }
    // Same token from the allowed origin works.
    let request = Request::put("/me/locale")
        .header(header::CONTENT_TYPE, "application/json")
        .header(api::CSRF_HEADER, &token)
        .header("origin", ORIGIN)
        .body(Body::from(body))
        .unwrap();
    assert_eq!(client.raw(request).await.status, StatusCode::OK);
}

#[tokio::test]
async fn onboarding_and_locale_update_the_account() {
    let app = TestApp::accounts_only();
    let mut client = app.client();
    client.signed_up("ana@example.com").await;
    let reply = client
        .send(
            Method::PUT,
            "/me/onboarding",
            Some(json!({"has_costco": false, "has_sams": true})),
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        (&reply.body["has_sams"], &reply.body["onboarded"]),
        (&json!(true), &json!(true))
    );
    let reply = client
        .send(Method::PUT, "/me/locale", Some(json!({"locale": "en"})))
        .await;
    assert_eq!(reply.body["locale"], "en");
    let reply = client
        .send(Method::PUT, "/me/locale", Some(json!({"locale": "de"})))
        .await;
    assert_eq!(
        (reply.status, reply.error()),
        (StatusCode::UNPROCESSABLE_ENTITY, "invalid_locale")
    );
    let reply = client
        .send(
            Method::PUT,
            "/me/onboarding",
            Some(json!({"has_costco": true})),
        )
        .await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(client.get("/me").await.body["locale"], "en");
}

#[tokio::test]
async fn account_routes_need_a_session() {
    let app = TestApp::accounts_only();
    let mut client = app.client();
    client.fetch_csrf().await;
    assert_eq!(client.get("/me").await.status, StatusCode::UNAUTHORIZED);
    let reply = client
        .send(Method::PUT, "/me/locale", Some(json!({"locale": "en"})))
        .await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn without_accounts_every_user_route_is_503() {
    let (router, _) = api::build(api::ApiConfig {
        products: ProductsState::disabled(),
        searches: SearchesState::disabled(),
        ..api::ApiConfig::disabled()
    });
    for (method, path) in [
        (Method::GET, "/auth/csrf"),
        (Method::POST, "/auth/signin"),
        (Method::GET, "/me"),
        (Method::GET, "/searches/1"),
        (Method::POST, "/roi"),
        (Method::GET, "/products/B0CHHSFMRL/price-history"),
    ] {
        let response = tower::ServiceExt::oneshot(
            router.clone(),
            Request::builder()
                .method(method.clone())
                .uri(path)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "{method} {path}"
        );
    }
}

#[tokio::test]
async fn spec_documents_the_session_and_csrf_schemes() {
    let spec = serde_json::to_value(api::openapi()).unwrap();
    let schemes = &spec["components"]["securitySchemes"];
    assert_eq!(
        schemes[api::SESSION_SCHEME],
        json!({"type": "apiKey", "in": "cookie", "name": api::SESSION_COOKIE})
    );
    assert_eq!(
        schemes[api::CSRF_SCHEME],
        json!({"type": "apiKey", "in": "header", "name": "X-CSRF-Token"})
    );
    let both = json!([{ api::SESSION_SCHEME: [], api::CSRF_SCHEME: [] }]);
    assert_eq!(spec["paths"]["/me/onboarding"]["put"]["security"], both);
    assert_eq!(spec["paths"]["/auth/signout"]["post"]["security"], both);
    assert_eq!(
        spec["paths"]["/me"]["get"]["security"],
        json!([{ api::SESSION_SCHEME: [] }])
    );
    for path in ["/auth/signup", "/auth/signin", "/auth/csrf", "/me/locale"] {
        assert!(spec["paths"][path].is_object(), "{path}");
    }
    for (path, method) in [
        ("/auth/signup", "post"),
        ("/auth/signin", "post"),
        ("/auth/signout", "post"),
        ("/auth/csrf", "get"),
    ] {
        assert!(
            spec["paths"][path][method]["responses"]["500"].is_object(),
            "{method} {path} documents 500"
        );
    }
    for schema in [
        "SessionResponse",
        "MeResponse",
        "ErrorResponse",
        "CsrfResponse",
    ] {
        assert!(
            spec["components"]["schemas"][schema].is_object(),
            "{schema}"
        );
    }
}
