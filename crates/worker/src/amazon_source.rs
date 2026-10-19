//! Builds the Amazon.com.mx source: SP-API first, Bright Data's Amazon
//! dataset as the fallback (ADR 0010). Logs which sources are configured
//! (names only, never values).

use std::sync::Arc;

use amazon::{BrightDataAmazonSource, SpApiAmazonSource, SpApiClient, SpApiConfig};
use app::{AmazonSource, FallbackAmazonSource};
use brightdata::Transport;
use brightdata::runs::{RunStore, SystemClock};
use http_guard::{Redactor, Secret};

/// Which Amazon sources were built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AmazonSources {
    pub(crate) sp_api: bool,
    pub(crate) bright_data: bool,
}

/// The SP-API source, or `None` (logged) when a credential is missing or the
/// client cannot be built.
fn spapi_source(
    (client_id, client_secret, refresh_token): (Option<Secret>, Option<Secret>, Option<Secret>),
    redactor: &Redactor,
) -> Option<Arc<dyn AmazonSource>> {
    let any_set = client_id.is_some() || client_secret.is_some() || refresh_token.is_some();
    let config = match SpApiConfig::from_secrets(client_id, client_secret, refresh_token) {
        Ok(config) => config,
        Err(error) if any_set => {
            tracing::warn!(error = %error, "SP-API partly configured; using the fallback only");
            return None;
        }
        Err(error) => {
            tracing::info!(error = %error, "SP-API not configured");
            return None;
        }
    };
    match SpApiClient::new(config) {
        Ok(client) => Some(Arc::new(SpApiAmazonSource::new(
            Arc::new(client),
            SystemClock,
        ))),
        Err(error) => {
            let message = redactor.sanitize(&error.to_string());
            tracing::warn!(error = %message, "SP-API client could not be built");
            None
        }
    }
}

/// The Bright Data fallback parts, only for the worker holding the driver
/// lock: fallback lookups start dataset runs, and a worker without the lock
/// could have them failed by the lock holder's start-up recovery. Without
/// the lock it logs (no secrets) and returns `None`; SP-API is unaffected.
pub(crate) fn driver_fallback<T, S>(
    holds_driver_lock: bool,
    client: Option<Arc<T>>,
    store: Arc<S>,
) -> Option<(Arc<T>, Arc<S>)> {
    if !holds_driver_lock {
        tracing::info!("another worker drives Bright Data runs; Amazon fallback disabled here");
        return None;
    }
    client.map(|client| (client, store))
}

/// `SP-API?` with `fallback?` (a Bright Data transport and run store),
/// combined by [`FallbackAmazonSource`]. Builds clients only; no request is
/// made here.
pub(crate) fn amazon_source<T, S>(
    spapi: (Option<Secret>, Option<Secret>, Option<Secret>),
    fallback: Option<(Arc<T>, Arc<S>)>,
    redactor: &Redactor,
) -> (FallbackAmazonSource, AmazonSources)
where
    T: Transport + 'static,
    S: RunStore + 'static,
{
    let primary = spapi_source(spapi, redactor);
    let secondary: Option<Arc<dyn AmazonSource>> = fallback.and_then(|(transport, store)| {
        match BrightDataAmazonSource::new(transport, store, SystemClock) {
            Ok(source) => Some(Arc::new(source) as Arc<dyn AmazonSource>),
            Err(error) => {
                tracing::warn!(error = %error, "Bright Data Amazon fallback could not be built");
                None
            }
        }
    });
    let sources = AmazonSources {
        sp_api: primary.is_some(),
        bright_data: secondary.is_some(),
    };
    tracing::info!(
        sp_api = sources.sp_api,
        bright_data = sources.bright_data,
        "Amazon sources configured"
    );
    (FallbackAmazonSource::new(primary, secondary), sources)
}

#[cfg(test)]
mod tests {
    use app::{AmazonSource, AmazonSourceError};
    use brightdata::BrightDataClient;
    use domain::Gtin;
    use http_guard::{Redactor, Secret};

    use std::sync::Arc;

    use super::{AmazonSources, amazon_source, driver_fallback};
    use crate::brightdata_store::PgRunStore;

    fn secret(value: &str) -> Option<Secret> {
        Some(Secret::new(value.to_owned()).unwrap())
    }

    #[tokio::test]
    async fn without_any_credentials_nothing_is_configured() {
        let (source, sources) = amazon_source::<BrightDataClient, PgRunStore>(
            (None, None, None),
            None,
            &Redactor::default(),
        );
        assert_eq!(
            sources,
            AmazonSources {
                sp_api: false,
                bright_data: false
            }
        );
        let gtin = Gtin::new("7501055363063").unwrap();
        assert_eq!(
            source.catalog_by_gtin(&gtin).await.unwrap_err(),
            AmazonSourceError::NotConfigured
        );
    }

    #[test]
    fn partial_spapi_credentials_leave_only_the_fallback() {
        let (_, sources) = amazon_source::<BrightDataClient, PgRunStore>(
            (secret("id"), None, secret("refresh")),
            None,
            &Redactor::default(),
        );
        assert!(!sources.sp_api);
    }

    #[test]
    fn full_spapi_credentials_build_the_primary_without_requests() {
        let (_, sources) = amazon_source::<BrightDataClient, PgRunStore>(
            (secret("id"), secret("secret"), secret("refresh")),
            None,
            &Redactor::default(),
        );
        assert!(sources.sp_api);
        assert!(!sources.bright_data);
    }

    #[test]
    fn fallback_needs_the_driver_lock() {
        let client = Some(Arc::new(1_u8));
        let store = Arc::new(2_u8);
        assert!(driver_fallback(false, client.clone(), store.clone()).is_none());
        let (client, store) = driver_fallback(true, client, store).unwrap();
        assert_eq!((*client, *store), (1, 2));
        assert!(driver_fallback::<u8, u8>(true, None, Arc::new(2)).is_none());
    }
}
