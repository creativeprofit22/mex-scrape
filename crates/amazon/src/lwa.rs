//! Login with Amazon (LWA): exchanges the refresh token for short-lived
//! access tokens and caches them until shortly before they expire.

use std::time::Duration;

use brightdata::{RetryPolicy, parse_retry_after};
use http_guard::{FormValue, GuardError, GuardedClient, SMALL_RESPONSE_BYTES, Secret, Url, header};
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::error::SpApiError;

/// Refresh this long before the token expires.
pub(crate) const REFRESH_MARGIN: Duration = Duration::from_secs(60);
/// LWA errors meaning the credentials themselves are bad.
const AUTH_ERRORS: [&str; 4] = [
    "invalid_grant",
    "invalid_client",
    "unauthorized_client",
    "access_denied",
];

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

#[derive(Deserialize, Default)]
struct LwaErrorBody {
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: String,
}

struct CachedToken {
    token: Secret,
    refresh_at: Instant,
}

/// The LWA credentials and the cached access token. Concurrent callers
/// share one refresh (the cache lock is held while refreshing).
pub(crate) struct TokenCache {
    client: GuardedClient,
    token_url: Url,
    client_id: Secret,
    client_secret: Secret,
    refresh_token: Secret,
    retry: RetryPolicy,
    cached: Mutex<Option<CachedToken>>,
}

impl TokenCache {
    pub(crate) fn new(
        client: GuardedClient,
        token_url: Url,
        client_id: Secret,
        client_secret: Secret,
        refresh_token: Secret,
        retry: RetryPolicy,
    ) -> Self {
        Self {
            client,
            token_url,
            client_id,
            client_secret,
            refresh_token,
            retry,
            cached: Mutex::new(None),
        }
    }

    /// A valid access token, refreshing it when missing or about to expire.
    pub(crate) async fn token(&self) -> Result<Secret, SpApiError> {
        let mut cached = self.cached.lock().await;
        if let Some(entry) = cached.as_ref()
            && Instant::now() < entry.refresh_at
        {
            return Ok(entry.token.clone());
        }
        let (token, lifetime) = self.refresh().await?;
        let margin = if lifetime > REFRESH_MARGIN * 2 {
            REFRESH_MARGIN
        } else {
            lifetime / 2
        };
        *cached = Some(CachedToken {
            token: token.clone(),
            refresh_at: Instant::now() + lifetime - margin,
        });
        Ok(token)
    }

    /// Drops `stale` from the cache (after a 401) so the next call
    /// refreshes; a newer token cached meanwhile is kept.
    pub(crate) async fn invalidate(&self, stale: &Secret) {
        let mut cached = self.cached.lock().await;
        if cached
            .as_ref()
            .is_some_and(|entry| entry.token.expose_secret() == stale.expose_secret())
        {
            *cached = None;
        }
    }

    async fn refresh(&self) -> Result<(Secret, Duration), SpApiError> {
        let mut attempt = 0;
        loop {
            let (error, retry_after) = match self.refresh_once().await {
                Ok(done) => return Ok(done),
                Err(failure) => failure,
            };
            let retryable = matches!(error, SpApiError::Transient(_));
            if !retryable || attempt >= self.retry.max_retries {
                return Err(error);
            }
            tokio::time::sleep(self.retry.delay(attempt, retry_after)).await;
            attempt += 1;
        }
    }

    async fn refresh_once(&self) -> Result<(Secret, Duration), (SpApiError, Option<Duration>)> {
        let guard = |error: GuardError| {
            let message = format!("LWA token refresh failed: {error}");
            let mapped = if error.is_transient() {
                SpApiError::Transient(message)
            } else {
                SpApiError::Build(message)
            };
            (mapped, None)
        };
        let response = self
            .client
            .post(&self.token_url)
            .map_err(guard)?
            .form(&[
                ("grant_type", FormValue::Plain("refresh_token")),
                ("refresh_token", FormValue::Secret(&self.refresh_token)),
                ("client_id", FormValue::Secret(&self.client_id)),
                ("client_secret", FormValue::Secret(&self.client_secret)),
            ])
            .map_err(guard)?
            .send()
            .await
            .map_err(guard)?;
        let status = response.status();
        if status.is_success() {
            let body: TokenResponse =
                response.json(SMALL_RESPONSE_BYTES).await.map_err(|error| {
                    (
                        SpApiError::Parse(format!("LWA token response: {error}")),
                        None,
                    )
                })?;
            let token = Secret::new(body.access_token).map_err(|_| {
                (
                    SpApiError::Parse("LWA returned an unusable access token".to_owned()),
                    None,
                )
            })?;
            let lifetime = Duration::from_secs(body.expires_in.clamp(1, 86_400));
            return Ok((token, lifetime));
        }
        let retry_after = response
            .headers()
            .get(header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_retry_after);
        let text = response
            .error_text(SMALL_RESPONSE_BYTES)
            .await
            .unwrap_or_default();
        let body: LwaErrorBody = serde_json::from_str(&text).unwrap_or_default();
        // Only the fixed error code and a redacted description are kept.
        let detail = format!(
            "LWA {} {}: {}",
            status.as_u16(),
            body.error,
            self.client.sanitize(&body.error_description)
        );
        let error =
            if AUTH_ERRORS.contains(&body.error.as_str()) || matches!(status.as_u16(), 401 | 403) {
                SpApiError::NotAuthorized(detail)
            } else if status.as_u16() == 429 || status.as_u16() == 408 || status.is_server_error() {
                SpApiError::Transient(detail)
            } else {
                SpApiError::Invalid(detail)
            };
        Err((error, retry_after))
    }
}
