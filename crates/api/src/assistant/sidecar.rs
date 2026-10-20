//! [`app::AssistantEngine`] over the gg-agent sidecar (ADR 0013).
//!
//! `POST {AGENT_URL}/v1/chat` with `Authorization: Bearer
//! AGENT_SERVICE_TOKEN` through an [`http_guard::GuardedClient`] using the
//! private-service policy (one port, allowlisted name, no redirects). The
//! reply is untrusted: its size, `stop` and usage are checked here, and the
//! use case cleans the text. Dropping the future (the browser went away)
//! drops the request, which closes the sidecar connection and aborts the run.

use std::sync::Arc;
use std::time::Duration;

use app::{AssistantEngine, Clock, EngineError, EngineReply, EngineRequest, PortFuture};
use domain::{AgentStop, MAX_AGENT_REPLY_CHARS, TokenCount, TokenUsage};
use http_guard::{
    GuardError, GuardedClient, HostAllowlist, Redactor, Secret, StatusCode, TransportPolicy, Url,
};
use serde::{Deserialize, Serialize};

use super::grant::GrantKey;

/// Total time allowed for one sidecar run (the sidecar's own run timeout
/// is shorter).
pub const SIDECAR_TOTAL_TIMEOUT: Duration = Duration::from_secs(75);

/// Largest sidecar response read.
const MAX_SIDECAR_RESPONSE_BYTES: u64 = 64 * 1024;

/// Why the sidecar client could not be configured. Never carries secrets.
#[derive(Debug, thiserror::Error)]
pub enum SidecarConfigError {
    /// `AGENT_URL` is not an absolute `http(s)://name:port` URL.
    #[error("AGENT_URL must be an absolute http(s) URL with a host name")]
    InvalidUrl,
    /// The HTTP client could not be built.
    #[error("assistant HTTP client could not be built")]
    Client,
}

/// The sidecar client.
pub struct SidecarEngine {
    client: GuardedClient,
    chat_url: Url,
    token: Secret,
    grants: Arc<GrantKey>,
    clock: Arc<dyn Clock>,
}

impl SidecarEngine {
    /// A client for the sidecar at `base` (e.g. `http://localhost:8090`)
    /// with the private-service transport policy on its port.
    ///
    /// # Errors
    /// [`SidecarConfigError`] when the URL or client is invalid.
    pub fn connect(
        base: &str,
        token: Secret,
        grants: Arc<GrantKey>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, SidecarConfigError> {
        let url = Url::parse(base).map_err(|_| SidecarConfigError::InvalidUrl)?;
        let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
            return Err(SidecarConfigError::InvalidUrl);
        };
        let allowlist = HostAllowlist::new([host]).map_err(|_| SidecarConfigError::InvalidUrl)?;
        let client = GuardedClient::builder(allowlist)
            .policy(TransportPolicy::private_service(port))
            .total_timeout(SIDECAR_TOTAL_TIMEOUT)
            .redactor(Redactor::new([&token, grants.secret()]))
            .build()
            .map_err(|_| SidecarConfigError::Client)?;
        Self::with_client(client, &url, token, grants, clock)
    }

    /// Like [`Self::connect`] over a caller-built `client` (tests use a
    /// loopback policy).
    ///
    /// # Errors
    /// [`SidecarConfigError::InvalidUrl`] when `base` cannot be joined.
    pub fn with_client(
        client: GuardedClient,
        base: &Url,
        token: Secret,
        grants: Arc<GrantKey>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, SidecarConfigError> {
        if !matches!(base.scheme(), "http" | "https") {
            return Err(SidecarConfigError::InvalidUrl);
        }
        let chat_url = base
            .join("/v1/chat")
            .map_err(|_| SidecarConfigError::InvalidUrl)?;
        Ok(Self {
            client,
            chat_url,
            token,
            grants,
            clock,
        })
    }

    async fn run(&self, request: &EngineRequest) -> Result<EngineReply, EngineError> {
        let grant = self.grants.sign(
            request.user,
            &request.tenant,
            &request.chat_id,
            self.clock.now(),
        );
        let body = WireRequest {
            chat_id: request.chat_id.as_str(),
            tenant: request.tenant.as_str(),
            locale: request.locale.as_str(),
            grant: &grant,
            token_budget: request.token_budget.get(),
            messages: request
                .history
                .messages()
                .iter()
                .map(|m| WireMessage {
                    role: m.role.as_str(),
                    content: m.text.as_str(),
                })
                .collect(),
        };
        let response = self
            .client
            .post(&self.chat_url)
            .and_then(|r| r.bearer(&self.token))
            .and_then(|r| r.json(&body))
            .map_err(|_| {
                tracing::warn!("building the assistant request failed");
                EngineError::Failed
            })?
            .send()
            .await
            .map_err(|e| match e {
                GuardError::Timeout => {
                    tracing::warn!("the assistant timed out");
                    EngineError::Timeout
                }
                GuardError::Connect(_) => {
                    tracing::warn!("the assistant is unreachable");
                    EngineError::Unavailable
                }
                _ => {
                    tracing::warn!("the assistant request failed");
                    EngineError::Failed
                }
            })?;
        match response.status() {
            StatusCode::OK => {}
            StatusCode::CONFLICT => return Err(EngineError::Busy),
            StatusCode::GATEWAY_TIMEOUT => {
                tracing::warn!("the assistant run timed out");
                return Err(EngineError::Timeout);
            }
            status => {
                tracing::warn!(status = status.as_u16(), "the assistant answered an error");
                return Err(EngineError::Failed);
            }
        }
        let wire: WireReply =
            response
                .json(MAX_SIDECAR_RESPONSE_BYTES)
                .await
                .map_err(|e| match e {
                    GuardError::Timeout => EngineError::Timeout,
                    _ => {
                        tracing::warn!("the assistant reply was invalid");
                        EngineError::Failed
                    }
                })?;
        wire.validate().ok_or_else(|| {
            tracing::warn!("the assistant reply was invalid");
            EngineError::Failed
        })
    }
}

impl AssistantEngine for SidecarEngine {
    fn chat<'a>(
        &'a self,
        request: &'a EngineRequest,
    ) -> PortFuture<'a, Result<EngineReply, EngineError>> {
        Box::pin(self.run(request))
    }
}

/// An engine that is not configured: every chat is `Unavailable`.
pub struct DisabledEngine;

impl AssistantEngine for DisabledEngine {
    fn chat<'a>(
        &'a self,
        _request: &'a EngineRequest,
    ) -> PortFuture<'a, Result<EngineReply, EngineError>> {
        Box::pin(async { Err(EngineError::Unavailable) })
    }
}

/// Body of `POST /v1/chat` (see `docs/modules/agent.md`).
#[derive(Serialize)]
struct WireRequest<'a> {
    chat_id: &'a str,
    /// Only keys the sidecar's one-chat-per-tenant guard; authority comes
    /// from the grant.
    tenant: &'a str,
    locale: &'a str,
    grant: &'a str,
    token_budget: u64,
    messages: Vec<WireMessage<'a>>,
}

#[derive(Serialize)]
struct WireMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct WireReply {
    reply: String,
    stop: String,
    usage: WireUsage,
}

#[derive(Deserialize)]
struct WireUsage {
    input_tokens: u64,
    output_tokens: u64,
}

impl WireReply {
    fn validate(self) -> Option<EngineReply> {
        if self.reply.chars().count() > MAX_AGENT_REPLY_CHARS {
            return None;
        }
        Some(EngineReply {
            stop: AgentStop::parse(&self.stop)?,
            usage: TokenUsage {
                input: TokenCount::new(self.usage.input_tokens),
                output: TokenCount::new(self.usage.output_tokens),
            },
            reply: self.reply,
        })
    }
}
