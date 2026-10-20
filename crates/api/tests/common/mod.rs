//! Shared harness for the api integration tests: in-memory accounts, a
//! reversible fake hasher, an in-memory session store and a client that
//! keeps the session cookie and CSRF token like a browser would.

#![allow(
    dead_code,
    reason = "each test binary uses a different subset of the harness"
)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use api::{
    AccountsState, AdminState, ApiConfig, ProductsState, Quota, RateLimits, SearchesState,
    SessionConfig, SessionStoreHandle,
};
use app::{
    CreateUserError, HashError, PasswordHasher, PortFuture, RepoError, UserCredentials, UserRecord,
    UserRepo,
};
use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use domain::{Email, Locale, Memberships, Plan, TenantId, UserId};
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{self, SessionStore};

/// The only origin the test config allows on mutating requests.
pub const ORIGIN: &str = "http://localhost:3000";

/// A password that passes the policy.
pub const PASSWORD: &str = "correct horse battery";

/// Reversible "hash" (`h:<password>`); fast, so tests stay quick.
pub struct FakeHasher;

impl PasswordHasher for FakeHasher {
    fn hash<'a>(&'a self, password: &'a str) -> PortFuture<'a, Result<String, HashError>> {
        Box::pin(async move { Ok(format!("h:{password}")) })
    }

    fn verify<'a>(&'a self, password: &'a str, hash: &'a str) -> PortFuture<'a, bool> {
        Box::pin(async move { hash.strip_prefix("h:") == Some(password) })
    }

    fn dummy_hash(&self) -> &str {
        "h:\u{0}never"
    }
}

/// Accounts in memory; ids start at 1.
#[derive(Default)]
pub struct FakeUsers {
    rows: Mutex<Vec<UserCredentials>>,
}

impl FakeUsers {
    fn update(
        &self,
        id: UserId,
        f: impl FnOnce(&mut UserRecord),
    ) -> Result<Option<UserRecord>, RepoError> {
        let mut rows = self.rows.lock().unwrap();
        Ok(rows.iter_mut().find(|c| c.user.id == id).map(|c| {
            f(&mut c.user);
            c.user.clone()
        }))
    }

    /// Moves user `id` to `plan` (there is no API for it yet).
    pub fn set_plan(&self, id: i64, plan: Plan) {
        self.update(UserId::new(id).unwrap(), |u| u.plan = plan)
            .unwrap()
            .unwrap();
    }

    /// The stored account `id`.
    pub fn get(&self, id: i64) -> UserRecord {
        self.update(UserId::new(id).unwrap(), |_| {})
            .unwrap()
            .unwrap()
    }
}

impl UserRepo for FakeUsers {
    fn create<'a>(
        &'a self,
        email: &'a Email,
        password_hash: &'a str,
        locale: Locale,
    ) -> PortFuture<'a, Result<UserRecord, CreateUserError>> {
        Box::pin(async move {
            let mut rows = self.rows.lock().unwrap();
            if rows.iter().any(|c| &c.user.email == email) {
                return Err(CreateUserError::EmailTaken);
            }
            let id = UserId::new(i64::try_from(rows.len()).unwrap() + 1).unwrap();
            let user = UserRecord {
                id,
                email: email.clone(),
                tenant: TenantId::for_user(id),
                plan: Plan::Free,
                memberships: Memberships::default(),
                onboarded: false,
                locale,
            };
            rows.push(UserCredentials {
                user: user.clone(),
                password_hash: password_hash.to_owned(),
            });
            Ok(user)
        })
    }

    fn find_credentials<'a>(
        &'a self,
        email: &'a Email,
    ) -> PortFuture<'a, Result<Option<UserCredentials>, RepoError>> {
        Box::pin(async move {
            let rows = self.rows.lock().unwrap();
            Ok(rows.iter().find(|c| &c.user.email == email).cloned())
        })
    }

    fn find_by_id(&self, id: UserId) -> PortFuture<'_, Result<Option<UserRecord>, RepoError>> {
        Box::pin(async move { self.update(id, |_| {}) })
    }

    fn set_memberships(
        &self,
        id: UserId,
        memberships: Memberships,
    ) -> PortFuture<'_, Result<Option<UserRecord>, RepoError>> {
        Box::pin(async move {
            self.update(id, |u| {
                u.memberships = memberships;
                u.onboarded = true;
            })
        })
    }

    fn set_locale(
        &self,
        id: UserId,
        locale: Locale,
    ) -> PortFuture<'_, Result<Option<UserRecord>, RepoError>> {
        Box::pin(async move { self.update(id, |u| u.locale = locale) })
    }
}

/// Session records in memory, shared between clones.
#[derive(Debug, Clone, Default)]
pub struct MemoryStore(Arc<Mutex<HashMap<Id, Record>>>);

impl MemoryStore {
    /// Ids of the stored sessions.
    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.0.lock().unwrap().keys().map(Id::to_string).collect();
        ids.sort();
        ids
    }

    /// Stored expiry of session `id`.
    pub fn expiry(&self, id: &str) -> time::OffsetDateTime {
        let id: Id = id.parse().unwrap();
        self.0.lock().unwrap()[&id].expiry_date
    }

    /// Overwrites the stored expiry of session `id`.
    pub fn set_expiry(&self, id: &str, expiry: time::OffsetDateTime) {
        let id: Id = id.parse().unwrap();
        self.0.lock().unwrap().get_mut(&id).unwrap().expiry_date = expiry;
    }
}

#[async_trait]
impl SessionStore for MemoryStore {
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        let mut map = self.0.lock().unwrap();
        while map.contains_key(&record.id) {
            record.id = Id::default();
        }
        map.insert(record.id, record.clone());
        Ok(())
    }

    async fn save(&self, record: &Record) -> session_store::Result<()> {
        self.0.lock().unwrap().insert(record.id, record.clone());
        Ok(())
    }

    async fn load(&self, session_id: &Id) -> session_store::Result<Option<Record>> {
        Ok(self.0.lock().unwrap().get(session_id).cloned())
    }

    async fn delete(&self, session_id: &Id) -> session_store::Result<()> {
        self.0.lock().unwrap().remove(session_id);
        Ok(())
    }
}

/// Quotas no test reaches unless it means to.
pub fn generous_limits() -> RateLimits {
    let q = Quota::new(100_000, 10_000);
    RateLimits {
        free: q,
        pro: q,
        anonymous: q,
        client_ip_header: None,
    }
}

/// A router over in-memory accounts and sessions.
pub struct TestApp {
    /// The router (cheap to clone; state is shared).
    pub router: Router,
    /// The accounts behind it.
    pub users: Arc<FakeUsers>,
    /// The sessions behind it.
    pub sessions: MemoryStore,
}

impl TestApp {
    /// Accounts enabled; the other route groups as given.
    pub fn new(products: ProductsState, searches: SearchesState, rate_limits: RateLimits) -> Self {
        let users = Arc::new(FakeUsers::default());
        let sessions = MemoryStore::default();
        let accounts = AccountsState::new(
            users.clone(),
            Arc::new(FakeHasher),
            SessionStoreHandle::new(sessions.clone()),
            SessionConfig {
                secure_cookie: true,
                allowed_origins: vec![ORIGIN.to_owned()],
            },
        );
        let (router, _) = api::build(ApiConfig {
            admin: AdminState::disabled(),
            products,
            searches,
            accounts,
            rate_limits,
        });
        Self {
            router,
            users,
            sessions,
        }
    }

    /// Only accounts enabled, with generous limits.
    pub fn accounts_only() -> Self {
        Self::new(
            ProductsState::disabled(),
            SearchesState::disabled(),
            generous_limits(),
        )
    }

    /// A browser-like client from `127.0.0.1`.
    pub fn client(&self) -> Client {
        self.client_from([127, 0, 0, 1])
    }

    /// A browser-like client from `ip`.
    pub fn client_from(&self, ip: [u8; 4]) -> Client {
        Client {
            router: self.router.clone(),
            cookie: None,
            csrf: None,
            peer: SocketAddr::from((ip, 40_000)),
        }
    }
}

/// A response with a parsed JSON body (`Null` when empty).
#[derive(Debug)]
pub struct Reply {
    /// Status code.
    pub status: StatusCode,
    /// Headers.
    pub headers: HeaderMap,
    /// JSON body.
    pub body: Value,
}

impl Reply {
    /// The `error` code of an error body.
    pub fn error(&self) -> &str {
        self.body["error"].as_str().unwrap_or("")
    }

    /// The `Set-Cookie` header for the session cookie, if any.
    pub fn session_cookie(&self) -> Option<&str> {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|v| v.starts_with(&format!("{}=", api::SESSION_COOKIE)))
    }
}

/// Keeps the session cookie and the CSRF token between requests.
pub struct Client {
    router: Router,
    /// `name=value` of the session cookie.
    pub cookie: Option<String>,
    /// CSRF token sent on non-GET requests.
    pub csrf: Option<String>,
    peer: SocketAddr,
}

impl Client {
    /// Sends `request` with the cookie and peer address; keeps any new
    /// session cookie.
    pub async fn raw(&mut self, mut request: Request<Body>) -> Reply {
        if let Some(cookie) = &self.cookie {
            request
                .headers_mut()
                .insert(header::COOKIE, cookie.parse().unwrap());
        }
        request.extensions_mut().insert(ConnectInfo(self.peer));
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        let reply = Reply {
            status,
            headers,
            body,
        };
        if let Some(set) = reply.session_cookie() {
            let pair = set.split(';').next().unwrap_or_default();
            let expired = set.contains("Max-Age=0") || pair.ends_with('=');
            self.cookie = (!expired).then(|| pair.to_owned());
        }
        reply
    }

    /// Like a same-origin `fetch`: JSON body, `Origin`, `Sec-Fetch-Site`
    /// and the CSRF token on non-GET requests.
    pub async fn send(&mut self, method: Method, path: &str, body: Option<Value>) -> Reply {
        let mut request = Request::builder()
            .method(method.clone())
            .uri(path)
            .header("sec-fetch-site", "same-origin");
        if method != Method::GET {
            request = request.header(header::ORIGIN, ORIGIN);
            if let Some(token) = &self.csrf {
                request = request.header(api::CSRF_HEADER, token);
            }
        }
        let body = match body {
            Some(json) => {
                request = request.header(header::CONTENT_TYPE, "application/json");
                Body::from(json.to_string())
            }
            None => Body::empty(),
        };
        self.raw(request.body(body).unwrap()).await
    }

    /// `GET path`.
    pub async fn get(&mut self, path: &str) -> Reply {
        self.send(Method::GET, path, None).await
    }

    /// Fetches (and keeps) the session's CSRF token.
    pub async fn fetch_csrf(&mut self) -> String {
        let reply = self.get("/auth/csrf").await;
        assert_eq!(reply.status, StatusCode::OK, "{reply:?}");
        let token = reply.body["csrf_token"].as_str().unwrap().to_owned();
        self.csrf = Some(token.clone());
        token
    }

    async fn start_session(&mut self, path: &str, body: Value) -> Reply {
        if self.csrf.is_none() {
            self.fetch_csrf().await;
        }
        let reply = self.send(Method::POST, path, Some(body)).await;
        if let Some(token) = reply.body["csrf_token"].as_str() {
            self.csrf = Some(token.to_owned());
        }
        reply
    }

    /// `POST /auth/signup` (fetching a CSRF token first if needed).
    pub async fn sign_up(&mut self, email: &str, password: &str) -> Reply {
        self.start_session(
            "/auth/signup",
            json!({ "email": email, "password": password }),
        )
        .await
    }

    /// `POST /auth/signin` (fetching a CSRF token first if needed).
    pub async fn sign_in(&mut self, email: &str, password: &str) -> Reply {
        self.start_session(
            "/auth/signin",
            json!({ "email": email, "password": password }),
        )
        .await
    }

    /// Signs up `email` and asserts it worked.
    pub async fn signed_up(&mut self, email: &str) -> Value {
        let reply = self.sign_up(email, PASSWORD).await;
        assert_eq!(reply.status, StatusCode::CREATED, "{reply:?}");
        reply.body["user"].clone()
    }
}
