//! Bright Data Browser API (ADR 0007): a remote Chrome driven over CDP.
//!
//! Credentials are looked up as the Bright Data MCP server does
//! (`browser_tools.js`, `calculate_cdp_endpoint`): `GET /status` gives the
//! customer id and `GET /zone/passwords?zone=…` the zone password; the
//! WebSocket user is `brd-customer-<c>-zone-<z>[-country-<cc>]` on
//! `brd.superproxy.io:9222`. The credentials travel only in a sensitive
//! `Authorization: Basic` header, never in the URL.
//!
//! One fetch is a minimal CDP exchange: `Target.createTarget` →
//! `Target.attachToTarget {flatten}` → `Page.enable` → `Network.enable` →
//! `Network.setBlockedURLs` (images, fonts, media, stylesheets and ad or
//! analytics hosts: the Browser API bills per byte) → `Page.navigate` →
//! wait for `Page.loadEventFired` → settle → `Runtime.evaluate(outerHTML)` →
//! `Target.closeTarget`.
//!
//! Settle: many store pages build their product list with a request made
//! after `load`, so the HTML is read only once the page is quiet: no network
//! request in flight and an unchanged element count for
//! [`BrowserSettle::quiet_polls`] checks in a row, or after
//! [`BrowserSettle::max_polls`] checks at most.

use http_guard::{
    GuardedWebSocket, HostAllowlist, SMALL_RESPONSE_BYTES, Secret, truncate_error_message,
};
use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::{BrightDataClient, UnlockRequest, UnlockedPage, read_json};
use crate::config::BrowserSettle;
use crate::error::{BrightDataError, Failure};
use crate::rate::RateKey;
use crate::refusal::{RefusalReason, classify_browser_error};

/// The Browser API host.
pub const BRIGHTDATA_BROWSER_HOST: &str = "brd.superproxy.io";
/// The Browser API CDP port (allowed for this host only).
pub const BRIGHTDATA_BROWSER_PORT: u16 = 9222;
/// The production CDP endpoint.
pub(crate) const BRIGHTDATA_BROWSER_ENDPOINT: &str = "wss://brd.superproxy.io:9222";
/// Cap for the returned HTML (same as an Unlocker page).
const MAX_HTML_BYTES: usize = 8 * 1024 * 1024;
/// Cap for one CDP message: the HTML arrives JSON-escaped, so allow room.
const MAX_CDP_MESSAGE_BYTES: usize = 3 * MAX_HTML_BYTES;
/// Longest accepted customer id.
const MAX_CUSTOMER_CHARS: usize = 64;
/// Downloads the HTML never needs (`Network.setBlockedURLs` patterns, as in
/// Bright Data's bandwidth guide). Scripts and data requests stay allowed:
/// they build the product list.
pub(crate) const BLOCKED_URL_PATTERNS: &[&str] = &[
    "*.png*",
    "*.jpg*",
    "*.jpeg*",
    "*.gif*",
    "*.webp*",
    "*.avif*",
    "*.svg*",
    "*.ico*",
    "*.woff*",
    "*.ttf*",
    "*.otf*",
    "*.eot*",
    "*.mp4*",
    "*.webm*",
    "*.mp3*",
    "*.css*",
    "*doubleclick.net*",
    "*google-analytics.com*",
    "*googletagmanager.com*",
    "*facebook.net*",
    "*hotjar.com*",
];
/// The settle check: how many elements the page has right now.
const ELEMENT_COUNT_EXPRESSION: &str = "document.getElementsByTagName('*').length";
/// Most in-flight requests tracked; beyond it the page is waited out by
/// [`BrowserSettle::max_polls`].
const MAX_TRACKED_REQUESTS: usize = 10_000;

impl BrightDataClient {
    /// `(user, password)` for the Browser API WebSocket.
    async fn browser_credentials(
        &self,
        zone: &Secret,
        request: &UnlockRequest,
    ) -> Result<(Secret, Secret), Failure> {
        let status_url = self.endpoint("/status");
        let response = self
            .send(RateKey::Browser, self.datasets.get(&status_url))
            .await?;
        let response = self.ensure_success(response).await?;
        let status = read_json(response, SMALL_RESPONSE_BYTES).await?;
        let customer = status
            .get("customer")
            .and_then(Value::as_str)
            .filter(|c| {
                !c.is_empty()
                    && c.len() <= MAX_CUSTOMER_CHARS
                    && c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            })
            .ok_or_else(|| Failure::terminal("Bright Data did not return a valid customer id"))?
            .to_owned();

        let mut passwords_url = self.endpoint("/zone/passwords");
        passwords_url
            .query_pairs_mut()
            .append_pair("zone", zone.expose_secret());
        let response = self
            .send(RateKey::Browser, self.datasets.get(&passwords_url))
            .await?;
        let response = self.ensure_success(response).await?;
        let passwords = read_json(response, SMALL_RESPONSE_BYTES).await?;
        let password = passwords
            .get("passwords")
            .and_then(Value::as_array)
            .and_then(|list| list.first())
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
            .ok_or_else(|| {
                Failure::terminal("Bright Data returned no Browser API zone password")
            })?;
        let password = Secret::new(password.to_owned())
            .map_err(|_| Failure::terminal("the Browser API zone password is not usable"))?;

        let country = request
            .country
            .as_ref()
            .map(|c| format!("-country-{}", c.as_str().to_ascii_lowercase()))
            .unwrap_or_default();
        let user = Secret::new(format!(
            "brd-customer-{customer}-zone-{}{country}",
            zone.expose_secret()
        ))
        .map_err(|_| Failure::terminal("the Browser API user name is not usable"))?;
        Ok((user, password))
    }

    /// One Browser API page fetch (no retries).
    pub(super) async fn browse_once(
        &self,
        request: &UnlockRequest,
    ) -> Result<UnlockedPage, Failure> {
        let zone = self
            .browser_zone
            .as_ref()
            .ok_or_else(|| Failure::terminal("Bright Data Browser API is not configured"))?;
        let (user, password) = self.browser_credentials(zone, request).await?;
        let redactor = self.datasets.redactor().with(&user).with(&password);
        let allowlist = HostAllowlist::new([BRIGHTDATA_BROWSER_HOST])
            .map_err(|error| Failure::from_guard(&error))?;
        let session = async {
            let mut socket = GuardedWebSocket::builder(allowlist, BRIGHTDATA_BROWSER_PORT)
                .policy(self.browser_policy)
                .max_message_bytes(MAX_CDP_MESSAGE_BYTES)
                .redactor(redactor.clone())
                .connect(&self.browser_endpoint, Some((&user, &password)))
                .await
                .map_err(|error| Failure::from_guard(&error))?;
            let result = Cdp::new(&mut socket, &redactor, self.browser_settle)
                .fetch_html(request.url.as_str(), self.browser_block_downloads)
                .await;
            socket.close().await;
            result
        };
        let body = match tokio::time::timeout(self.browser_timeout, session).await {
            Ok(result) => result?,
            Err(_) => {
                return Err(Failure {
                    error: BrightDataError::Transient(
                        "Bright Data Browser API page load timed out".to_owned(),
                    ),
                    retry_after: None,
                    not_accepted: false,
                });
            }
        };
        Ok(UnlockedPage { status: 200, body })
    }
}

/// A minimal CDP client over one WebSocket.
struct Cdp<'a> {
    socket: &'a mut GuardedWebSocket,
    redactor: &'a http_guard::Redactor,
    settle: BrowserSettle,
    next_id: u64,
    load_fired: bool,
    session: Option<String>,
    /// Request ids of our session that have not finished or failed yet.
    in_flight: BTreeSet<String>,
}

impl<'a> Cdp<'a> {
    fn new(
        socket: &'a mut GuardedWebSocket,
        redactor: &'a http_guard::Redactor,
        settle: BrowserSettle,
    ) -> Self {
        Self {
            socket,
            redactor,
            settle,
            next_id: 0,
            load_fired: false,
            session: None,
            in_flight: BTreeSet::new(),
        }
    }

    async fn fetch_html(&mut self, url: &str, block_downloads: bool) -> Result<String, Failure> {
        let target = self
            .call(
                "Target.createTarget",
                json!({ "url": "about:blank" }),
                false,
            )
            .await?;
        let target_id = str_field(&target, "targetId")?.to_owned();
        let attached = self
            .call(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
                false,
            )
            .await?;
        self.session = Some(str_field(&attached, "sessionId")?.to_owned());
        self.call("Page.enable", json!({}), true).await?;
        self.call("Network.enable", json!({}), true).await?;
        if block_downloads {
            self.call(
                "Network.setBlockedURLs",
                json!({ "urls": BLOCKED_URL_PATTERNS }),
                true,
            )
            .await?;
        }
        let navigated = self
            .call("Page.navigate", json!({ "url": url }), true)
            .await?;
        if let Some(error_text) = navigated.get("errorText").and_then(Value::as_str) {
            return Err(self.cdp_failure(error_text));
        }
        while !self.load_fired {
            self.next_message().await?;
        }
        self.settle().await?;
        let evaluated = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": "document.documentElement.outerHTML",
                    "returnByValue": true,
                }),
                true,
            )
            .await?;
        if evaluated.get("exceptionDetails").is_some() {
            return Err(Failure::terminal(
                "Bright Data Browser API could not read the page HTML",
            ));
        }
        let html = evaluated
            .pointer("/result/value")
            .and_then(Value::as_str)
            .ok_or_else(|| Failure::terminal("Bright Data Browser API returned no page HTML"))?
            .to_owned();
        if html.len() > MAX_HTML_BYTES {
            return Err(Failure::terminal(format!(
                "the rendered page exceeded the {MAX_HTML_BYTES}-byte limit"
            )));
        }
        // Best effort: the session is closed right after anyway.
        let _ = self
            .call(
                "Target.closeTarget",
                json!({ "targetId": target_id }),
                false,
            )
            .await;
        Ok(html)
    }

    /// Waits until the page is quiet (see the module docs). Never fails just
    /// because the page keeps changing: after `max_polls` checks the HTML is
    /// read as it is.
    async fn settle(&mut self) -> Result<(), Failure> {
        let mut last_count = None;
        let mut quiet = 0;
        for _ in 0..self.settle.max_polls {
            tokio::time::sleep(self.settle.poll_interval).await;
            let count = self.element_count().await?;
            if self.in_flight.is_empty() && last_count == Some(count) {
                quiet += 1;
                if quiet >= self.settle.quiet_polls {
                    return Ok(());
                }
            } else {
                quiet = 0;
            }
            last_count = Some(count);
        }
        Ok(())
    }

    async fn element_count(&mut self) -> Result<u64, Failure> {
        let evaluated = self
            .call(
                "Runtime.evaluate",
                json!({ "expression": ELEMENT_COUNT_EXPRESSION, "returnByValue": true }),
                true,
            )
            .await?;
        evaluated
            .pointer("/result/value")
            .and_then(Value::as_u64)
            .filter(|_| evaluated.get("exceptionDetails").is_none())
            .ok_or_else(|| {
                Failure::terminal("Bright Data Browser API could not check the page state")
            })
    }

    /// Sends one command and waits for its reply, noting events meanwhile.
    async fn call(
        &mut self,
        method: &str,
        params: Value,
        in_session: bool,
    ) -> Result<Value, Failure> {
        self.next_id += 1;
        let id = self.next_id;
        let mut message = json!({ "id": id, "method": method, "params": params });
        if in_session && let Some(session) = &self.session {
            message["sessionId"] = Value::String(session.clone());
        }
        self.socket
            .send_text(&message.to_string())
            .await
            .map_err(|error| Failure::from_guard(&error))?;
        loop {
            let reply = self.next_message().await?;
            if reply.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = reply.get("error") {
                let text = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown CDP error");
                return Err(self.cdp_failure(&format!("{method}: {text}")));
            }
            return Ok(reply.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// Reads one message; records `Page.loadEventFired` and the start and
    /// end of network requests for our session.
    async fn next_message(&mut self) -> Result<Value, Failure> {
        let text = self
            .socket
            .recv_text()
            .await
            .map_err(|error| Failure::from_guard(&error))?
            .ok_or_else(|| Failure {
                error: BrightDataError::Transient(
                    "Bright Data Browser API closed the session".to_owned(),
                ),
                retry_after: None,
                not_accepted: false,
            })?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|_| Failure::terminal("Bright Data Browser API sent invalid CDP JSON"))?;
        let ours = self.session.is_some()
            && value.get("sessionId").and_then(Value::as_str) == self.session.as_deref();
        if ours {
            let request_id = || {
                value
                    .pointer("/params/requestId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            };
            match value.get("method").and_then(Value::as_str) {
                Some("Page.loadEventFired") => self.load_fired = true,
                Some("Network.requestWillBeSent") => {
                    if let Some(id) = request_id()
                        && self.in_flight.len() < MAX_TRACKED_REQUESTS
                    {
                        self.in_flight.insert(id);
                    }
                }
                Some("Network.loadingFinished" | "Network.loadingFailed") => {
                    if let Some(id) = request_id() {
                        self.in_flight.remove(&id);
                    }
                }
                _ => {}
            }
        }
        Ok(value)
    }

    /// A CDP error: `brob` means robots.txt (KYC), anything else is terminal.
    fn cdp_failure(&self, text: &str) -> Failure {
        let detail = self.redactor.sanitize(text);
        match classify_browser_error(&detail) {
            Some(reason @ RefusalReason::BrowserRobots) => Failure {
                error: BrightDataError::BlockedNeedsKyc {
                    reason,
                    message: truncate_error_message(&format!(
                        "Bright Data blocked this target until KYC is approved: {}",
                        reason.describe()
                    )),
                },
                retry_after: None,
                not_accepted: false,
            },
            _ => Failure::terminal(truncate_error_message(&format!(
                "Bright Data Browser API error: {detail}"
            ))),
        }
    }
}

fn str_field<'v>(value: &'v Value, name: &str) -> Result<&'v str, Failure> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| Failure::terminal("Bright Data Browser API sent an unexpected CDP reply"))
}
