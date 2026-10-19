//! Hardened outbound HTTP and server secrets for mex-scrape.
//!
//! Every outbound call goes through a [`GuardedClient`] (ported from linkgo
//! `net/transport.rs`, async):
//!
//! - https only, to exact hosts on a per-client [`HostAllowlist`], on the
//!   default port, with no userinfo in the URL;
//! - redirects are never followed and no environment proxy is used;
//! - connect and total timeouts;
//! - DNS answers pointing at loopback/private/link-local networks are
//!   dropped (DNS-rebinding guard);
//! - response bodies are read through a hard byte cap;
//! - error messages never contain the URL, are redacted with a
//!   [`Redactor`] and truncated to [`MAX_ERROR_MESSAGE_CHARS`].
//!
//! Secrets are [`Secret`] values (defined in the `secrets` crate and
//! re-exported here): no `Debug`, `Display` or `Serialize`, sent only as
//! sensitive headers.

mod client;
mod error;
mod ip;
mod policy;
mod resolver;
mod secret;

pub use client::{
    DEFAULT_CONNECT_TIMEOUT, DEFAULT_MAX_RESPONSE_BYTES, DEFAULT_TOTAL_TIMEOUT, GuardedClient,
    GuardedClientBuilder, GuardedRequest, GuardedResponse, SMALL_RESPONSE_BYTES,
};
pub use error::GuardError;
pub use ip::{is_non_public_ip, is_non_public_ipv4, is_non_public_ipv6};
pub use policy::{HostAllowlist, TransportPolicy};
pub use reqwest::{Method, StatusCode, Url, header};
pub use secrets::{MAX_ERROR_MESSAGE_CHARS, Redactor, Secret, SecretError, truncate_error_message};
