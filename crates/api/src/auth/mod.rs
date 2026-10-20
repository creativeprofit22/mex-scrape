//! Accounts, cookie sessions and CSRF protection (ADR 0012).
//!
//! Users sign up and sign in with e-mail and password; `axum-login` keeps
//! the user id in a server-side session (see [`store`]) whose random id is
//! the only thing in the `HttpOnly`, `SameSite=Lax` cookie. The session id
//! rotates on every sign-in.
//!
//! Every non-GET user route needs an `X-CSRF-Token` header matching the
//! session's token (from `GET /auth/csrf` or the sign-in response), and a
//! `Sec-Fetch-Site`/`Origin`, when the browser sends them, that is
//! same-origin. Anything else answers 403 (fails closed).

pub mod hasher;
pub mod store;

use std::sync::Arc;

use app::{
    Profile, ProfileError, SignIn, SignInError, SignUp, SignUpError, UpdateOnboarding, UserRecord,
};
use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum_login::{AuthSession, AuthUser, AuthnBackend};
use domain::{Locale, Memberships, UserId};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq as _;
use tower_sessions::Session;
use utoipa::ToSchema;

pub use store::{PgSessionStore, SessionStoreHandle};

/// Name of the session cookie.
pub const SESSION_COOKIE: &str = "mex_session";

/// Request header carrying the CSRF token.
pub const CSRF_HEADER: &str = "x-csrf-token";

/// Session key holding the CSRF token.
const CSRF_KEY: &str = "csrf";

/// Session lifetime after the last request (the expiry slides on every
/// request that carries a non-empty session).
pub const SESSION_DAYS: i64 = 7;

/// The signed-in user, loaded fresh from storage on every request.
#[derive(Debug, Clone)]
pub struct SessionUser {
    /// The account.
    pub record: UserRecord,
    auth_hash: Vec<u8>,
}

impl SessionUser {
    fn new(record: UserRecord) -> Self {
        // Binds the session to the account id and address: a session whose
        // stored hash no longer matches is flushed by axum-login.
        let auth_hash = format!("{}:{}", record.id.get(), record.email.as_str()).into_bytes();
        Self { record, auth_hash }
    }
}

impl AuthUser for SessionUser {
    type Id = i64;

    fn id(&self) -> i64 {
        self.record.id.get()
    }

    fn session_auth_hash(&self) -> &[u8] {
        &self.auth_hash
    }
}

/// E-mail and password from a sign-in form. No `Debug`: holds a password.
pub struct Credentials {
    email: String,
    password: String,
}

/// Account storage or hashing failed.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("accounts unavailable")]
pub struct BackendError;

/// The `axum-login` backend over the account use cases.
#[derive(Clone)]
pub struct Accounts {
    sign_in: Arc<SignIn>,
    profile: Arc<Profile>,
}

impl AuthnBackend for Accounts {
    type User = SessionUser;
    type Credentials = Credentials;
    type Error = BackendError;

    async fn authenticate(&self, creds: Credentials) -> Result<Option<SessionUser>, BackendError> {
        match self.sign_in.sign_in(&creds.email, &creds.password).await {
            Ok(user) => Ok(Some(SessionUser::new(user))),
            Err(SignInError::InvalidCredentials) => Ok(None),
            Err(SignInError::Unavailable) => Err(BackendError),
        }
    }

    async fn get_user(&self, id: &i64) -> Result<Option<SessionUser>, BackendError> {
        let Some(id) = UserId::new(*id) else {
            return Ok(None);
        };
        match self.profile.get(id).await {
            Ok(user) => Ok(Some(SessionUser::new(user))),
            Err(ProfileError::NotFound) => Ok(None),
            Err(ProfileError::Unavailable) => Err(BackendError),
        }
    }
}

/// The session of the current request.
pub type Auth = AuthSession<Accounts>;

/// Cookie and origin settings.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Sets `Secure` on the cookie. Only turn off for local plain HTTP.
    pub secure_cookie: bool,
    /// Exact origins (`scheme://host[:port]`) allowed in an `Origin` header
    /// on mutating requests.
    pub allowed_origins: Vec<String>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            secure_cookie: true,
            allowed_origins: Vec::new(),
        }
    }
}

pub(crate) struct Enabled {
    pub(crate) backend: Accounts,
    sign_up: SignUp,
    onboarding: UpdateOnboarding,
    profile: Arc<Profile>,
    pub(crate) store: SessionStoreHandle,
    pub(crate) config: SessionConfig,
}

/// State of the account routes. Without it every user route answers 503.
#[derive(Clone)]
pub struct AccountsState {
    pub(crate) inner: Option<Arc<Enabled>>,
}

impl AccountsState {
    /// User routes answer 503.
    #[must_use]
    pub const fn disabled() -> Self {
        Self { inner: None }
    }

    /// Enabled over `users`, `hasher` and the session `store`.
    #[must_use]
    pub fn new(
        users: Arc<dyn app::UserRepo>,
        hasher: Arc<dyn app::PasswordHasher>,
        store: SessionStoreHandle,
        config: SessionConfig,
    ) -> Self {
        let profile = Arc::new(Profile::new(users.clone()));
        Self {
            inner: Some(Arc::new(Enabled {
                backend: Accounts {
                    sign_in: Arc::new(SignIn::new(users.clone(), hasher.clone())),
                    profile: profile.clone(),
                },
                sign_up: SignUp::new(users.clone(), hasher),
                onboarding: UpdateOnboarding::new(users),
                profile,
                store,
                config,
            })),
        }
    }

    fn enabled(&self) -> Result<&Enabled, ApiError> {
        self.inner
            .as_deref()
            .ok_or(ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))
    }
}

/// Error body: a fixed, machine-readable code (the web app translates it).
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorResponse {
    /// E.g. `invalid_email`, `weak_password`, `email_taken`,
    /// `invalid_credentials`, `unauthenticated`, `csrf`, `rate_limited`.
    pub error: String,
}

/// A status with a fixed error code.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ApiError(pub(crate) StatusCode, pub(crate) &'static str);

impl ApiError {
    pub(crate) const UNAUTHENTICATED: Self = Self(StatusCode::UNAUTHORIZED, "unauthenticated");
    pub(crate) const INTERNAL: Self = Self(StatusCode::INTERNAL_SERVER_ERROR, "internal");
    pub(crate) const UNPROCESSABLE: Self = Self(StatusCode::UNPROCESSABLE_ENTITY, "invalid_body");
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(ErrorResponse {
                error: self.1.to_owned(),
            }),
        )
            .into_response()
    }
}

/// Maps a JSON body rejection: wrong shape 422, not JSON 400.
pub(crate) fn json_body<T>(body: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    body.map(|Json(b)| b).map_err(|rejection| match rejection {
        JsonRejection::JsonDataError(_) => ApiError::UNPROCESSABLE,
        _ => ApiError(StatusCode::BAD_REQUEST, "invalid_json"),
    })
}

/// The signed-in user or 401.
pub(crate) fn signed_in(auth: &Auth) -> Result<&UserRecord, ApiError> {
    auth.user
        .as_ref()
        .map(|u| &u.record)
        .ok_or(ApiError::UNAUTHENTICATED)
}

/// 256 random bits (two session-style ids from a CSPRNG), URL-safe base64.
fn new_csrf_token() -> String {
    format!(
        "{}{}",
        tower_sessions::session::Id::default(),
        tower_sessions::session::Id::default()
    )
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Whether the browser-supplied provenance headers are acceptable. Absent
/// headers pass (non-browser clients); the token check still applies.
fn same_origin(headers: &HeaderMap, allowed: &[String]) -> bool {
    if let Some(site) = headers.get("sec-fetch-site")
        && site.as_bytes() != b"same-origin"
    {
        return false;
    }
    match headers.get(axum::http::header::ORIGIN) {
        None => true,
        Some(origin) => origin
            .to_str()
            .is_ok_and(|o| allowed.iter().any(|a| a == o)),
    }
}

/// CSRF guard for user routes: safe methods pass; others need a same-origin
/// request and the session's token in [`CSRF_HEADER`].
pub(crate) async fn csrf_guard(
    State(config): State<Arc<SessionConfig>>,
    session: Session,
    request: Request,
    next: Next,
) -> Response {
    if matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) {
        return next.run(request).await;
    }
    let forbidden = ApiError(StatusCode::FORBIDDEN, "csrf");
    if !same_origin(request.headers(), &config.allowed_origins) {
        return forbidden.into_response();
    }
    let Some(given) = header(request.headers(), CSRF_HEADER) else {
        return forbidden.into_response();
    };
    let expected = match session.get::<String>(CSRF_KEY).await {
        Ok(Some(token)) => token,
        Ok(None) => return forbidden.into_response(),
        Err(_) => {
            tracing::warn!("reading the session failed");
            return ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable").into_response();
        }
    };
    if bool::from(given.as_bytes().ct_eq(expected.as_bytes())) {
        next.run(request).await
    } else {
        forbidden.into_response()
    }
}

/// The account as shown to its owner.
#[derive(Debug, Serialize, ToSchema)]
pub struct MeResponse {
    /// User id.
    pub id: i64,
    /// Normalized e-mail address.
    pub email: String,
    /// `free` or `pro`.
    pub plan: String,
    /// Holds a Costco membership.
    pub has_costco: bool,
    /// Holds a Sam's Club membership.
    pub has_sams: bool,
    /// Whether onboarding (memberships) is done.
    pub onboarded: bool,
    /// Preferred language: `es` or `en`.
    pub locale: String,
}

impl From<&UserRecord> for MeResponse {
    fn from(u: &UserRecord) -> Self {
        Self {
            id: u.id.get(),
            email: u.email.as_str().to_owned(),
            plan: u.plan.as_str().to_owned(),
            has_costco: u.memberships.costco,
            has_sams: u.memberships.sams,
            onboarded: u.onboarded,
            locale: u.locale.as_str().to_owned(),
        }
    }
}

/// A new session: the account and its fresh CSRF token.
#[derive(Debug, Serialize, ToSchema)]
pub struct SessionResponse {
    /// The signed-in account.
    pub user: MeResponse,
    /// Send as `X-CSRF-Token` on every non-GET request.
    pub csrf_token: String,
}

/// The session's CSRF token.
#[derive(Debug, Serialize, ToSchema)]
pub struct CsrfResponse {
    /// Send as `X-CSRF-Token` on every non-GET request.
    pub csrf_token: String,
}

/// Body of `POST /auth/signup`.
#[derive(Deserialize, ToSchema)]
pub struct SignUpRequest {
    /// E-mail address.
    pub email: String,
    /// 8–128 characters.
    pub password: String,
    /// `es` (default) or `en`.
    pub locale: Option<String>,
}

/// Body of `POST /auth/signin`.
#[derive(Deserialize, ToSchema)]
pub struct SignInRequest {
    /// E-mail address.
    pub email: String,
    /// Password.
    pub password: String,
}

/// Body of `PUT /me/onboarding`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct OnboardingRequest {
    /// Holds a Costco membership.
    pub has_costco: bool,
    /// Holds a Sam's Club membership.
    pub has_sams: bool,
}

/// Body of `PUT /me/locale`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct LocaleRequest {
    /// `es` or `en`.
    pub locale: String,
}

fn parse_locale(raw: &str) -> Result<Locale, ApiError> {
    Locale::parse(raw).ok_or(ApiError(StatusCode::UNPROCESSABLE_ENTITY, "invalid_locale"))
}

/// Signs `user` in on a fresh session id with a fresh CSRF token.
async fn start_session(auth: &mut Auth, user: SessionUser) -> Result<SessionResponse, ApiError> {
    if auth.user.is_some() {
        // Flushing drops the old session (and its id) before switching.
        auth.logout().await.map_err(|_| ApiError::INTERNAL)?;
    }
    auth.login(&user).await.map_err(|_| ApiError::INTERNAL)?;
    let csrf_token = new_csrf_token();
    auth.session
        .insert(CSRF_KEY, &csrf_token)
        .await
        .map_err(|_| ApiError::INTERNAL)?;
    Ok(SessionResponse {
        user: (&user.record).into(),
        csrf_token,
    })
}

/// Creates an account and signs it in.
#[utoipa::path(
    post,
    path = "/auth/signup",
    operation_id = "sign_up",
    tag = "auth",
    security(("csrf_token" = [])),
    request_body = SignUpRequest,
    responses(
        (status = CREATED, description = "Account created and signed in", body = SessionResponse),
        (status = BAD_REQUEST, description = "Body is not JSON", body = ErrorResponse),
        (status = FORBIDDEN, description = "CSRF check failed", body = ErrorResponse),
        (status = CONFLICT, description = "`email_taken`", body = ErrorResponse),
        (status = UNPROCESSABLE_ENTITY, description = "`invalid_email`, `weak_password` or `invalid_locale`", body = ErrorResponse),
        (status = TOO_MANY_REQUESTS, description = "Rate limited", body = ErrorResponse),
        (status = INTERNAL_SERVER_ERROR, description = "Session could not be stored", body = ErrorResponse),
        (status = SERVICE_UNAVAILABLE, description = "Accounts are not configured", body = ErrorResponse)
    )
)]
pub(crate) async fn sign_up(
    State(state): State<AccountsState>,
    mut auth: Auth,
    body: Result<Json<SignUpRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<SessionResponse>), ApiError> {
    let inner = state.enabled()?;
    let body = json_body(body)?;
    let locale = body
        .locale
        .as_deref()
        .map_or(Ok(Locale::Es), parse_locale)?;
    let user = inner
        .sign_up
        .sign_up(&body.email, &body.password, locale)
        .await
        .map_err(|e| match e {
            SignUpError::InvalidEmail => {
                ApiError(StatusCode::UNPROCESSABLE_ENTITY, "invalid_email")
            }
            SignUpError::WeakPassword(_) => {
                ApiError(StatusCode::UNPROCESSABLE_ENTITY, "weak_password")
            }
            SignUpError::EmailTaken => ApiError(StatusCode::CONFLICT, "email_taken"),
            SignUpError::Unavailable => {
                tracing::warn!("creating an account failed");
                ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable")
            }
        })?;
    let session = start_session(&mut auth, SessionUser::new(user)).await?;
    Ok((StatusCode::CREATED, Json(session)))
}

/// Signs in with e-mail and password.
#[utoipa::path(
    post,
    path = "/auth/signin",
    operation_id = "sign_in",
    tag = "auth",
    security(("csrf_token" = [])),
    request_body = SignInRequest,
    responses(
        (status = OK, description = "Signed in", body = SessionResponse),
        (status = BAD_REQUEST, description = "Body is not JSON", body = ErrorResponse),
        (status = UNAUTHORIZED, description = "`invalid_credentials`", body = ErrorResponse),
        (status = FORBIDDEN, description = "CSRF check failed", body = ErrorResponse),
        (status = UNPROCESSABLE_ENTITY, description = "Missing fields", body = ErrorResponse),
        (status = TOO_MANY_REQUESTS, description = "Rate limited", body = ErrorResponse),
        (status = INTERNAL_SERVER_ERROR, description = "Session could not be stored", body = ErrorResponse),
        (status = SERVICE_UNAVAILABLE, description = "Accounts are not configured", body = ErrorResponse)
    )
)]
pub(crate) async fn sign_in(
    State(state): State<AccountsState>,
    mut auth: Auth,
    body: Result<Json<SignInRequest>, JsonRejection>,
) -> Result<Json<SessionResponse>, ApiError> {
    state.enabled()?;
    let body = json_body(body)?;
    let user = auth
        .authenticate(Credentials {
            email: body.email,
            password: body.password,
        })
        .await
        .map_err(|_| {
            tracing::warn!("sign-in failed: accounts unavailable");
            ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable")
        })?
        .ok_or(ApiError(StatusCode::UNAUTHORIZED, "invalid_credentials"))?;
    Ok(Json(start_session(&mut auth, user).await?))
}

/// Ends the session (deletes it server-side and expires the cookie).
#[utoipa::path(
    post,
    path = "/auth/signout",
    operation_id = "sign_out",
    tag = "auth",
    security(("session_cookie" = [], "csrf_token" = [])),
    responses(
        (status = NO_CONTENT, description = "Signed out"),
        (status = FORBIDDEN, description = "CSRF check failed", body = ErrorResponse),
        (status = TOO_MANY_REQUESTS, description = "Rate limited", body = ErrorResponse),
        (status = INTERNAL_SERVER_ERROR, description = "Session could not be stored", body = ErrorResponse),
        (status = SERVICE_UNAVAILABLE, description = "Accounts are not configured", body = ErrorResponse)
    )
)]
pub(crate) async fn sign_out(
    State(state): State<AccountsState>,
    mut auth: Auth,
) -> Result<StatusCode, ApiError> {
    state.enabled()?;
    auth.logout().await.map_err(|_| ApiError::INTERNAL)?;
    Ok(StatusCode::NO_CONTENT)
}

/// The session's CSRF token, created on first use (signed in or not).
#[utoipa::path(
    get,
    path = "/auth/csrf",
    operation_id = "csrf_token",
    tag = "auth",
    responses(
        (status = OK, description = "The token", body = CsrfResponse),
        (status = TOO_MANY_REQUESTS, description = "Rate limited", body = ErrorResponse),
        (status = INTERNAL_SERVER_ERROR, description = "Session could not be stored", body = ErrorResponse),
        (status = SERVICE_UNAVAILABLE, description = "Accounts are not configured", body = ErrorResponse)
    )
)]
pub(crate) async fn csrf_token(
    State(state): State<AccountsState>,
    auth: Auth,
) -> Result<Json<CsrfResponse>, ApiError> {
    state.enabled()?;
    let existing = auth
        .session
        .get::<String>(CSRF_KEY)
        .await
        .map_err(|_| ApiError::INTERNAL)?;
    let csrf_token = match existing {
        Some(token) => token,
        None => {
            let token = new_csrf_token();
            auth.session
                .insert(CSRF_KEY, &token)
                .await
                .map_err(|_| ApiError::INTERNAL)?;
            token
        }
    };
    Ok(Json(CsrfResponse { csrf_token }))
}

/// The signed-in account.
#[utoipa::path(
    get,
    path = "/me",
    operation_id = "me",
    tag = "account",
    security(("session_cookie" = [])),
    responses(
        (status = OK, description = "The account", body = MeResponse),
        (status = UNAUTHORIZED, description = "Not signed in", body = ErrorResponse),
        (status = TOO_MANY_REQUESTS, description = "Rate limited", body = ErrorResponse),
        (status = SERVICE_UNAVAILABLE, description = "Accounts are not configured", body = ErrorResponse)
    )
)]
pub(crate) async fn me(
    State(state): State<AccountsState>,
    auth: Auth,
) -> Result<Json<MeResponse>, ApiError> {
    state.enabled()?;
    Ok(Json(signed_in(&auth)?.into()))
}

fn profile_error(e: ProfileError) -> ApiError {
    match e {
        ProfileError::NotFound => ApiError::UNAUTHENTICATED,
        ProfileError::Unavailable => {
            tracing::warn!("updating an account failed");
            ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable")
        }
    }
}

/// Stores the user's store memberships and completes onboarding. Offers
/// that need a membership the user lacks are hidden from search results.
#[utoipa::path(
    put,
    path = "/me/onboarding",
    operation_id = "update_onboarding",
    tag = "account",
    security(("session_cookie" = [], "csrf_token" = [])),
    request_body = OnboardingRequest,
    responses(
        (status = OK, description = "Updated account", body = MeResponse),
        (status = UNAUTHORIZED, description = "Not signed in", body = ErrorResponse),
        (status = FORBIDDEN, description = "CSRF check failed", body = ErrorResponse),
        (status = UNPROCESSABLE_ENTITY, description = "Missing fields", body = ErrorResponse),
        (status = TOO_MANY_REQUESTS, description = "Rate limited", body = ErrorResponse),
        (status = SERVICE_UNAVAILABLE, description = "Accounts are not configured", body = ErrorResponse)
    )
)]
pub(crate) async fn update_onboarding(
    State(state): State<AccountsState>,
    auth: Auth,
    body: Result<Json<OnboardingRequest>, JsonRejection>,
) -> Result<Json<MeResponse>, ApiError> {
    let inner = state.enabled()?;
    let id = signed_in(&auth)?.id;
    let body = json_body(body)?;
    let memberships = Memberships {
        costco: body.has_costco,
        sams: body.has_sams,
    };
    let user = inner
        .onboarding
        .set(id, memberships)
        .await
        .map_err(profile_error)?;
    Ok(Json((&user).into()))
}

/// Stores the user's preferred language.
#[utoipa::path(
    put,
    path = "/me/locale",
    operation_id = "update_locale",
    tag = "account",
    security(("session_cookie" = [], "csrf_token" = [])),
    request_body = LocaleRequest,
    responses(
        (status = OK, description = "Updated account", body = MeResponse),
        (status = UNAUTHORIZED, description = "Not signed in", body = ErrorResponse),
        (status = FORBIDDEN, description = "CSRF check failed", body = ErrorResponse),
        (status = UNPROCESSABLE_ENTITY, description = "`invalid_locale`", body = ErrorResponse),
        (status = TOO_MANY_REQUESTS, description = "Rate limited", body = ErrorResponse),
        (status = SERVICE_UNAVAILABLE, description = "Accounts are not configured", body = ErrorResponse)
    )
)]
pub(crate) async fn update_locale(
    State(state): State<AccountsState>,
    auth: Auth,
    body: Result<Json<LocaleRequest>, JsonRejection>,
) -> Result<Json<MeResponse>, ApiError> {
    let inner = state.enabled()?;
    let id = signed_in(&auth)?.id;
    let locale = parse_locale(&json_body(body)?.locale)?;
    let user = inner
        .profile
        .set_locale(id, locale)
        .await
        .map_err(profile_error)?;
    Ok(Json((&user).into()))
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_static(v));
        }
        h
    }

    #[test]
    fn same_origin_checks_fetch_metadata_and_origin() {
        let allowed = vec!["http://localhost:3000".to_owned()];
        assert!(same_origin(&headers(&[]), &allowed));
        assert!(same_origin(
            &headers(&[
                ("sec-fetch-site", "same-origin"),
                ("origin", "http://localhost:3000")
            ]),
            &allowed
        ));
        assert!(!same_origin(
            &headers(&[("sec-fetch-site", "cross-site")]),
            &allowed
        ));
        assert!(!same_origin(
            &headers(&[("sec-fetch-site", "same-site")]),
            &allowed
        ));
        assert!(!same_origin(
            &headers(&[("origin", "https://evil.example")]),
            &allowed
        ));
        assert!(!same_origin(&headers(&[("origin", "null")]), &allowed));
    }

    #[test]
    fn csrf_tokens_are_long_and_distinct() {
        let a = new_csrf_token();
        assert_eq!(a.len(), 44);
        assert_ne!(a, new_csrf_token());
    }
}
