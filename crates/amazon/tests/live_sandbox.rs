//! Manual live check against Amazon's SP-API **sandbox**
//! (`sandbox.sellingpartnerapi-na.amazon.com`). Ignored by default.
//!
//! ```text
//! SPAPI_LWA_CLIENT_ID=... SPAPI_LWA_CLIENT_SECRET=... SPAPI_REFRESH_TOKEN=... \
//!   cargo test -p amazon --test live_sandbox -- --ignored --nocapture
//! ```
//!
//! Hard cap: one LWA token refresh and one catalog search (retries off).
//! The static sandbox answers only the request shapes Amazon documents, so
//! a 400 here still proves the token exchange and authorization work; a
//! 401/403 means the credentials or roles are wrong. Prints only the result
//! kind and redacted errors.

use amazon::{SpApiClient, SpApiConfig, SpApiEndpoint, SpApiError};
use http_guard::Secret;

fn secret(name: &str) -> Option<Secret> {
    Secret::from_env(name).unwrap_or_else(|error| panic!("{error}"))
}

#[tokio::test]
#[ignore = "live: needs SPAPI_LWA_CLIENT_ID, SPAPI_LWA_CLIENT_SECRET and SPAPI_REFRESH_TOKEN"]
async fn live_sandbox_catalog_search() {
    let mut config = SpApiConfig::from_secrets(
        secret("SPAPI_LWA_CLIENT_ID"),
        secret("SPAPI_LWA_CLIENT_SECRET"),
        secret("SPAPI_REFRESH_TOKEN"),
    )
    .unwrap_or_else(|error| panic!("{error}"))
    .with_endpoint(SpApiEndpoint::Sandbox);
    config.retry.max_retries = 0;
    let client = SpApiClient::new(config).expect("client");
    match client.search_catalog_by_keyword("red").await {
        Ok(items) => println!("sandbox search: ok, {} items", items.len()),
        Err(SpApiError::NotAuthorized(message)) => {
            panic!("not authorized (check credentials and roles): {message}")
        }
        Err(SpApiError::Invalid(message)) => {
            println!("sandbox search: token accepted; sandbox rejected the shape: {message}");
        }
        Err(other) => panic!("sandbox search failed: {other}"),
    }
}
