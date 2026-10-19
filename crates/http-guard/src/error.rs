//! The one error type callers see; it never wraps a raw `reqwest::Error`.

use std::error::Error as StdError;

use secrets::Redactor;

/// Errors from building a guarded client, validating a request or reading a
/// response. Messages never contain the request URL. Text from the transport
/// or a provider (`Connect`, `Transport`, `Build`, `InvalidJson`) is redacted
/// and truncated; `host`, `scheme` and header `name` fields and
/// `InvalidAllowlist` text echo the caller's own input.
#[derive(Debug, thiserror::Error)]
pub enum GuardError {
    /// The host allowlist is empty or names an invalid host.
    #[error("invalid host allowlist: {0}")]
    InvalidAllowlist(String),
    /// The request URL is malformed or not allowed (userinfo, port, no host).
    #[error("invalid request URL: {0}")]
    InvalidUrl(String),
    /// The URL scheme is not https.
    #[error("refusing {scheme}:// request: only https is allowed")]
    InsecureScheme {
        /// The refused scheme.
        scheme: String,
    },
    /// The URL host is not on this client's allowlist.
    #[error("host {host} is not on this client's allowlist")]
    HostNotAllowed {
        /// The refused host.
        host: String,
    },
    /// Callers may not set this header (e.g. `Host`).
    #[error("header {name} may not be set by callers")]
    ForbiddenHeader {
        /// The refused header name.
        name: String,
    },
    /// A header name or value is not valid HTTP.
    #[error("header {name} has an invalid name or value")]
    InvalidHeader {
        /// The header name as given.
        name: String,
    },
    /// The connect or total timeout elapsed.
    #[error("request timed out")]
    Timeout,
    /// The connection could not be made (DNS, TCP, TLS).
    #[error("could not connect: {0}")]
    Connect(String),
    /// The request or response failed after connecting.
    #[error("request failed: {0}")]
    Transport(String),
    /// The response body is larger than the caller's cap.
    #[error("response exceeded the {limit}-byte limit")]
    BodyTooLarge {
        /// The cap in bytes.
        limit: u64,
    },
    /// A JSON body could not be serialized or parsed.
    #[error("invalid JSON: {0}")]
    InvalidJson(String),
    /// The underlying HTTP client could not be created.
    #[error("HTTP client could not be created: {0}")]
    Build(String),
}

impl GuardError {
    /// True when the request hit a timeout.
    #[must_use]
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout)
    }

    /// True for failures that may succeed on retry (timeouts, connection and
    /// transport errors); false for policy and validation errors.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Timeout | Self::Connect(_) | Self::Transport(_))
    }

    /// Converts a reqwest error: strips the URL (it may carry query
    /// strings), joins the source chain, then redacts and truncates.
    pub(crate) fn from_reqwest(error: reqwest::Error, redactor: &Redactor) -> Self {
        let timeout = error.is_timeout();
        let connect = error.is_connect();
        let builder = error.is_builder();
        let message = redactor.sanitize(&describe(&error.without_url()));
        if timeout {
            Self::Timeout
        } else if connect {
            Self::Connect(message)
        } else if builder {
            Self::Build(message)
        } else {
            Self::Transport(message)
        }
    }
}

/// The error and its sources as `a: b: c`, skipping repeated text.
fn describe(error: &dyn StdError) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(inner) = source {
        let part = inner.to_string();
        if !part.is_empty() && !text.ends_with(&part) {
            text.push_str(": ");
            text.push_str(&part);
        }
        source = inner.source();
    }
    text
}
