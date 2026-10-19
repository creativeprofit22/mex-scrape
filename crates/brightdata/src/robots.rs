//! robots.txt pre-check: skip paths a retailer disallows before spending a
//! paid Web Unlocker request (Bright Data blocks them until KYC anyway).
//!
//! The file is fetched directly (free) through `http-guard`, parsed for the
//! `*` user-agent group and cached per host with a TTL.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use http_guard::{GuardError, GuardedClient, HostAllowlist, TransportPolicy, Url};

use crate::ids::TargetUrl;
use crate::runs::Clock;

/// How long a fetched robots.txt is trusted.
pub const ROBOTS_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// How long an unreachable (5xx/network) robots.txt is remembered.
pub const ROBOTS_ERROR_TTL: Duration = Duration::from_secs(5 * 60);
/// Largest robots.txt body read.
const MAX_ROBOTS_BYTES: u64 = 512 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// Result of looking at a target's robots.txt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Not disallowed (or robots.txt unavailable: Bright Data enforces).
    Allowed,
    /// Disallowed for the `*` user agent.
    Disallowed,
}

/// What fetching a robots.txt returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    /// 2xx body.
    Body(String),
    /// 4xx: no restrictions apply.
    Missing,
    /// 5xx, network or size trouble.
    Unavailable,
}

/// Boxed future so the source can be a trait object.
pub type FetchFuture<'a> = Pin<Box<dyn Future<Output = Fetched> + Send + 'a>>;

/// Where robots.txt files come from; faked in tests.
pub trait RobotsSource: Send + Sync {
    /// Fetches `/robots.txt` of `host`.
    fn fetch<'a>(&'a self, host: &'a str) -> FetchFuture<'a>;
}

/// Direct HTTPS fetch, allowlisted to the given retailer hosts.
pub struct HttpRobotsSource {
    client: GuardedClient,
}

impl HttpRobotsSource {
    /// Builds a client allowed to reach only `hosts`.
    ///
    /// # Errors
    ///
    /// [`GuardError`] if a host is invalid or the client cannot be built.
    pub fn new<I, S>(hosts: I) -> Result<Self, GuardError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let client = GuardedClient::builder(HostAllowlist::new(hosts)?)
            .policy(TransportPolicy::PUBLIC_HTTPS)
            .total_timeout(FETCH_TIMEOUT)
            .build()?;
        Ok(Self { client })
    }

    async fn fetch_inner(&self, host: &str) -> Fetched {
        let Ok(url) = Url::parse(&format!("https://{host}/robots.txt")) else {
            return Fetched::Unavailable;
        };
        let Ok(request) = self.client.get(&url) else {
            return Fetched::Unavailable;
        };
        let Ok(response) = request.send().await else {
            return Fetched::Unavailable;
        };
        let status = response.status();
        if status.is_success() {
            match response.text(MAX_ROBOTS_BYTES).await {
                Ok(body) => Fetched::Body(body),
                Err(_) => Fetched::Unavailable,
            }
        } else if status.is_client_error() {
            Fetched::Missing
        } else {
            Fetched::Unavailable
        }
    }
}

impl RobotsSource for HttpRobotsSource {
    fn fetch<'a>(&'a self, host: &'a str) -> FetchFuture<'a> {
        Box::pin(self.fetch_inner(host))
    }
}

/// Parsed rules of the `*` group.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rules {
    /// `(is_allow, pattern)`.
    rules: Vec<(bool, String)>,
}

impl Rules {
    /// Parses robots.txt text. Only groups naming `*` apply; several are
    /// merged.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut rules = Vec::new();
        let mut applies = false;
        let mut in_agents = false;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            let (name, value) = (name.trim().to_ascii_lowercase(), value.trim());
            match name.as_str() {
                "user-agent" => {
                    if !in_agents {
                        applies = false;
                    }
                    in_agents = true;
                    applies |= value == "*";
                }
                "allow" | "disallow" => {
                    in_agents = false;
                    if applies && !value.is_empty() {
                        rules.push((name == "allow", value.to_owned()));
                    }
                }
                _ => in_agents = false,
            }
        }
        Self { rules }
    }

    /// True if `path_and_query` may be fetched: the longest matching rule
    /// wins and `Allow` wins a tie.
    #[must_use]
    pub fn allows(&self, path_and_query: &str) -> bool {
        let mut best: Option<(usize, bool)> = None;
        for (allow, pattern) in &self.rules {
            if pattern_matches(pattern, path_and_query) {
                let candidate = (pattern.len(), *allow);
                best = match best {
                    Some(current) if current >= candidate => Some(current),
                    _ => Some(candidate),
                };
            }
        }
        best.is_none_or(|(_, allow)| allow)
    }
}

/// Prefix match with `*` (any run) and a trailing `$` (end of path).
fn pattern_matches(pattern: &str, path: &str) -> bool {
    let (pattern, anchored) = pattern
        .strip_suffix('$')
        .map_or((pattern, false), |rest| (rest, true));
    let (pattern, path) = (pattern.as_bytes(), path.as_bytes());
    // reach[j]: the pattern consumed so far matches path[..j].
    let mut reach = vec![false; path.len() + 1];
    reach[0] = true;
    for &p in pattern {
        let mut next = vec![false; path.len() + 1];
        if p == b'*' {
            let mut seen = false;
            for (slot, &matched) in next.iter_mut().zip(&reach) {
                seen |= matched;
                *slot = seen;
            }
        } else {
            for j in 0..path.len() {
                next[j + 1] = reach[j] && path[j] == p;
            }
        }
        reach = next;
    }
    if anchored {
        reach[path.len()]
    } else {
        reach.iter().any(|&matched| matched)
    }
}

type Cache = BTreeMap<String, (i64, Arc<Rules>)>;

/// Cached robots.txt checker.
pub struct RobotsChecker {
    source: Box<dyn RobotsSource>,
    clock: Box<dyn Clock>,
    cache: Mutex<Cache>,
}

impl RobotsChecker {
    /// A checker over `source` using `clock` for TTLs.
    #[must_use]
    pub fn new(source: Box<dyn RobotsSource>, clock: Box<dyn Clock>) -> Self {
        Self {
            source,
            clock,
            cache: Mutex::new(BTreeMap::new()),
        }
    }

    /// Whether `url` may be fetched. Unparseable URLs are `Allowed` (the
    /// caller validates them separately).
    pub async fn check(&self, url: &TargetUrl) -> Access {
        let Ok(parsed) = Url::parse(url.as_str()) else {
            return Access::Allowed;
        };
        let Some(host) = parsed.host_str().map(str::to_ascii_lowercase) else {
            return Access::Allowed;
        };
        let target = match parsed.query() {
            Some(query) => format!("{}?{query}", parsed.path()),
            None => parsed.path().to_owned(),
        };
        let rules = self.rules_for(&host).await;
        if rules.allows(&target) {
            Access::Allowed
        } else {
            Access::Disallowed
        }
    }

    async fn rules_for(&self, host: &str) -> Arc<Rules> {
        let now = self.clock.now_ms();
        let cached = self
            .lock()
            .get(host)
            .filter(|(expires, _)| *expires > now)
            .map(|(_, rules)| Arc::clone(rules));
        if let Some(rules) = cached {
            return rules;
        }
        let (rules, ttl) = match self.source.fetch(host).await {
            Fetched::Body(text) => (Rules::parse(&text), ROBOTS_TTL),
            Fetched::Missing => (Rules::default(), ROBOTS_TTL),
            Fetched::Unavailable => (Rules::default(), ROBOTS_ERROR_TTL),
        };
        let rules = Arc::new(rules);
        let ttl_ms = i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX);
        self.lock().insert(
            host.to_owned(),
            (now.saturating_add(ttl_ms), Arc::clone(&rules)),
        );
        rules
    }

    fn lock(&self) -> MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

    use super::*;

    const DAY_MS: i64 = 24 * 60 * 60 * 1000;
    const ROBOTS: &str = "User-agent: Googlebot\nDisallow: /\n\n\
        User-agent: *\n# comment\nDisallow: /search\nDisallow: /*?sort=\nDisallow: /cart$\n\
        Allow: /search/public\nDisallow:\n";

    #[test]
    fn parses_star_group_only() {
        let rules = Rules::parse(ROBOTS);
        assert!(rules.allows("/tienda/producto/1"));
        assert!(!rules.allows("/search?q=a"));
        assert!(!rules.allows("/search"));
    }

    #[test]
    fn longest_match_and_allow_tie() {
        let rules = Rules::parse(ROBOTS);
        assert!(rules.allows("/search/public/x"));
        let tie = Rules::parse("User-agent: *\nDisallow: /a\nAllow: /a\n");
        assert!(tie.allows("/a"));
    }

    #[test]
    fn wildcard_and_end_anchor() {
        let rules = Rules::parse(ROBOTS);
        assert!(!rules.allows("/cat/x?sort=price"));
        assert!(!rules.allows("/cart"));
        assert!(rules.allows("/cart/items"));
    }

    #[test]
    fn empty_and_unmatched_files_allow_everything() {
        assert!(Rules::parse("").allows("/anything"));
        assert!(Rules::parse("User-agent: bot\nDisallow: /\n").allows("/x"));
        assert!(Rules::parse("User-agent: *\nDisallow:\n").allows("/x"));
    }

    #[test]
    fn consecutive_agent_lines_share_a_group() {
        let rules = Rules::parse("User-agent: bot\nUser-agent: *\nDisallow: /p\n");
        assert!(!rules.allows("/p"));
    }

    struct FakeSource {
        reply: Fetched,
        calls: Arc<AtomicUsize>,
    }

    impl RobotsSource for FakeSource {
        fn fetch<'a>(&'a self, _host: &'a str) -> FetchFuture<'a> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let reply = self.reply.clone();
            Box::pin(async move { reply })
        }
    }

    struct SharedClock(Arc<AtomicI64>);

    impl Clock for SharedClock {
        fn now_ms(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    fn make(reply: Fetched) -> (RobotsChecker, Arc<AtomicUsize>, Arc<AtomicI64>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let now = Arc::new(AtomicI64::new(1_000));
        let source = FakeSource {
            reply,
            calls: Arc::clone(&calls),
        };
        let clock = SharedClock(Arc::clone(&now));
        let checker = RobotsChecker::new(Box::new(source), Box::new(clock));
        (checker, calls, now)
    }

    fn url(path: &str) -> TargetUrl {
        TargetUrl::parse(&format!("https://www.example.mx{path}")).unwrap()
    }

    #[tokio::test]
    async fn caches_until_ttl_expires() {
        let (checker, calls, now) = make(Fetched::Body(ROBOTS.to_owned()));
        assert_eq!(checker.check(&url("/search?q=1")).await, Access::Disallowed);
        assert_eq!(checker.check(&url("/p/1")).await, Access::Allowed);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        now.store(1_000 + DAY_MS - 1, Ordering::SeqCst);
        checker.check(&url("/p/1")).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        now.store(1_000 + DAY_MS, Ordering::SeqCst);
        checker.check(&url("/p/1")).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn client_error_allows_all_and_unavailable_retries_soon() {
        let (checker, _, _) = make(Fetched::Missing);
        assert_eq!(checker.check(&url("/search")).await, Access::Allowed);

        let (checker, calls, now) = make(Fetched::Unavailable);
        assert_eq!(checker.check(&url("/search")).await, Access::Allowed);
        now.store(1_000 + 5 * 60 * 1000, Ordering::SeqCst);
        checker.check(&url("/search")).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
