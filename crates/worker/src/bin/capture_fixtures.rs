//! Capped manual capture of sanitized HTML fixtures for retailer adapters.
//!
//! `cargo run -p worker --bin capture_fixtures -- --retailer ID --kind
//! search|product|both [--url URL] [--query Q] [--out-dir DIR]`
//!
//! Checks robots.txt first (a disallowed page is skipped, never fetched),
//! then makes one Web Unlocker request per page (country MX, retries off),
//! at most [`HARD_CAP`] per run. `--browser` uses the Browser API instead
//! (needs `BRIGHTDATA_BROWSER_ZONE`); only for stores proven to need
//! JavaScript. `--no-block` (with `--browser`) lets the page download
//! images, fonts and stylesheets too, to check whether blocking breaks it;
//! it costs more. The body goes through
//! `retailers::fixture::sanitize` before it is written to
//! `DIR/<retailer>/<kind>.html` (default DIR: `crates/retailers/tests/fixtures`).
//! Without `--url`, search uses the adapter's `search_url(query)` (stores
//! with no keyword search, such as Sears, Sanborns and Waldo's, need
//! `--url <category URL>`) and product
//! uses the first offer of the adapter's `parse_search` on the search fixture.
//! Prints only status and sizes; never prints bodies or secrets.
//!
//! `--resanitize` makes no request: it re-runs the sanitizer over the saved
//! fixtures of the given kinds (after the sanitizer changes).

use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use brightdata::robots::{Access, HttpRobotsSource, RobotsChecker};
use brightdata::runs::SystemClock;
use brightdata::{
    BrightDataClient, BrightDataConfig, BrightDataError, CountryCode, TargetUrl, Transport,
    UnlockRequest,
};
use domain::UnixMillis;
use http_guard::{Redactor, Secret};
use retailers::{AdapterError, Page, Retailer, RetailerInfo};

/// Most pages one run may fetch, whatever the flags say.
const HARD_CAP: usize = 2;
const DEFAULT_OUT_DIR: &str = "crates/retailers/tests/fixtures";
const DEFAULT_QUERY: &str = "licuadora";
/// Smaller bodies are redirects or error stubs, not pages worth saving.
const MIN_BODY_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Search,
    Product,
}

impl Kind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Product => "product",
        }
    }
}

#[derive(Debug)]
struct Options {
    retailer: &'static RetailerInfo,
    kinds: Vec<Kind>,
    url: Option<String>,
    query: String,
    out_dir: PathBuf,
    resanitize: bool,
    browser: bool,
    no_block: bool,
}

fn parse_args(args: &[String]) -> anyhow::Result<Options> {
    let mut retailer = None;
    let mut kinds = None;
    let mut url = None;
    let mut query = DEFAULT_QUERY.to_owned();
    let mut out_dir = PathBuf::from(DEFAULT_OUT_DIR);
    let mut resanitize = false;
    let mut browser = false;
    let mut no_block = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = |flag: &str| iter.next().with_context(|| format!("{flag} needs a value"));
        match arg.as_str() {
            "--retailer" => {
                let id = value("--retailer")?;
                retailer = Some(
                    retailers::all()
                        .iter()
                        .find(|r| r.id == id.as_str())
                        .with_context(|| format!("unknown retailer: {id}"))?,
                );
            }
            "--kind" => {
                kinds = Some(match value("--kind")?.as_str() {
                    "search" => vec![Kind::Search],
                    "product" => vec![Kind::Product],
                    "both" => vec![Kind::Search, Kind::Product],
                    other => bail!("--kind must be search, product or both, not {other}"),
                });
            }
            "--url" => url = Some(value("--url")?.clone()),
            "--query" => query.clone_from(value("--query")?),
            "--out-dir" => out_dir = PathBuf::from(value("--out-dir")?),
            "--resanitize" => resanitize = true,
            "--browser" => browser = true,
            "--no-block" => no_block = true,
            other => bail!("unknown argument: {other}"),
        }
    }
    let kinds = kinds.context("--kind is required")?;
    if no_block && !browser {
        bail!("--no-block needs --browser");
    }
    if url.is_some() && kinds.len() > 1 {
        bail!("--url needs a single --kind");
    }
    Ok(Options {
        retailer: retailer.context("--retailer is required")?,
        kinds,
        url,
        query,
        out_dir,
        resanitize,
        browser,
        no_block,
    })
}

/// Validates an explicit URL: parseable and on one of the retailer's hosts.
fn checked_url(retailer: &RetailerInfo, url: &str) -> anyhow::Result<TargetUrl> {
    let target = TargetUrl::parse(url).map_err(|_| anyhow::anyhow!("--url is not a valid URL"))?;
    let host = target.host();
    if !retailer.hosts.iter().any(|h| h.eq_ignore_ascii_case(&host)) {
        bail!("--url host is not a {} host", retailer.id);
    }
    Ok(target)
}

fn fixture_path(out_dir: &Path, retailer: &RetailerInfo, kind: Kind) -> PathBuf {
    // `retailer.id` comes from the registry ([a-z0-9_]), so this stays in out_dir.
    out_dir
        .join(retailer.id)
        .join(format!("{}.html", kind.as_str()))
}

/// The product URL to capture: the first offer parsed from the search fixture.
fn product_from_search(
    adapter: &dyn Retailer,
    search_url: &TargetUrl,
    search_html: &str,
) -> anyhow::Result<TargetUrl> {
    let page = Page {
        url: search_url.clone(),
        body: search_html.to_owned(),
        fetched_at: UnixMillis(0),
    };
    let offers = adapter
        .parse_search(&page)
        .map_err(|e| anyhow::anyhow!("search fixture did not parse: {e}"))?;
    let first = offers.first().context("search fixture has no offers")?;
    checked_url(adapter.info(), &first.url)
}

/// Counts fetches against [`HARD_CAP`].
struct Budget(usize);

impl Budget {
    fn take(&mut self) -> anyhow::Result<()> {
        if self.0 >= HARD_CAP {
            bail!("hard cap of {HARD_CAP} pages per run reached");
        }
        self.0 += 1;
        Ok(())
    }
}

fn secret(name: &str) -> anyhow::Result<Option<Secret>> {
    Secret::from_lookup(name, |n: &str| std::env::var_os(n)).map_err(|e| anyhow::anyhow!("{e}"))
}

fn live_client(browser: bool, no_block: bool) -> anyhow::Result<(BrightDataClient, Redactor)> {
    let key = secret("BRIGHTDATA_API_KEY")?;
    let zone = secret("BRIGHTDATA_UNLOCKER_ZONE")?;
    let browser_zone = secret("BRIGHTDATA_BROWSER_ZONE")?;
    if browser && browser_zone.is_none() {
        bail!("--browser needs BRIGHTDATA_BROWSER_ZONE");
    }
    let redactor = Redactor::new(key.iter().chain(zone.iter()).chain(browser_zone.iter()));
    let mut config = BrightDataConfig::from_secrets(key, zone)?.with_browser_zone(browser_zone);
    config.retry.max_retries = 0;
    config.browser_block_downloads = !no_block;
    let client = BrightDataClient::new(config).map_err(|e| anyhow::anyhow!("{}", e.message()))?;
    Ok((client, redactor))
}

/// Search URL from `--url`, or the adapter's `search_url(query)`.
fn search_target(options: &Options) -> anyhow::Result<TargetUrl> {
    if let Some(url) = &options.url {
        return checked_url(options.retailer, url);
    }
    let adapter = retailers::adapter_for(options.retailer.id)
        .with_context(|| format!("{} has no adapter yet; pass --url", options.retailer.id))?;
    adapter
        .search_url(&options.query)
        .map_err(|e| search_url_error(options.retailer, &e))
}

/// Explains an adapter search-URL error; category-only stores need `--url`.
fn search_url_error(retailer: &RetailerInfo, e: &AdapterError) -> anyhow::Error {
    if *e == AdapterError::SearchUnavailable {
        anyhow::anyhow!(
            "{} has no keyword search (category listings only); pass --url <category URL>",
            retailer.id
        )
    } else {
        anyhow::anyhow!("{e}")
    }
}

/// Re-sanitizes saved fixtures in place; no network.
fn resanitize(options: &Options) -> anyhow::Result<()> {
    for kind in &options.kinds {
        let path = fixture_path(&options.out_dir, options.retailer, *kind);
        let html = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let clean = retailers::fixture::sanitize(&html);
        std::fs::write(&path, &clean).with_context(|| format!("writing {}", path.display()))?;
        println!(
            "{} {}: {} -> {} bytes",
            options.retailer.id,
            kind.as_str(),
            html.len(),
            clean.len()
        );
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = parse_args(&args)?;
    if options.resanitize {
        return resanitize(&options);
    }
    let robots = RobotsChecker::new(
        Box::new(HttpRobotsSource::new(retailers::hosts()).context("building robots client")?),
        Box::new(SystemClock),
    );
    let (client, redactor) = live_client(options.browser, options.no_block)?;
    let mut budget = Budget(0);
    let mut last_search: Option<(TargetUrl, String)> = None;
    let id = options.retailer.id;
    for kind in options.kinds.clone() {
        let target = match (kind, &options.url) {
            (Kind::Search, _) => search_target(&options)?,
            (Kind::Product, Some(url)) => checked_url(options.retailer, url)?,
            (Kind::Product, None) => {
                let adapter = retailers::adapter_for(id)
                    .with_context(|| format!("{id} has no adapter yet; pass --url"))?;
                let (url, html) = match &last_search {
                    Some(found) => found.clone(),
                    None => {
                        let path = fixture_path(&options.out_dir, options.retailer, Kind::Search);
                        let html = std::fs::read_to_string(&path)
                            .with_context(|| format!("reading {}", path.display()))?;
                        (search_target(&options)?, html)
                    }
                };
                product_from_search(adapter, &url, &html)?
            }
        };
        if robots.check(&target).await == Access::Disallowed {
            println!("{id} {}: skipped, disallowed by robots.txt", kind.as_str());
            continue;
        }
        budget.take()?;
        let request = UnlockRequest {
            url: target.clone(),
            country: Some(CountryCode::MX),
        };
        let fetched = if options.browser {
            client.browse(&request).await
        } else {
            client.unlock(&request).await
        };
        let page = match fetched {
            Ok(page) => page,
            Err(BrightDataError::BlockedNeedsKyc { reason, .. }) => {
                println!("{id} {}: blocked_needs_kyc ({reason:?})", kind.as_str());
                continue;
            }
            Err(error) => bail!(
                "{id} {}: {}",
                kind.as_str(),
                redactor.sanitize(error.message())
            ),
        };
        if page.body.trim().len() < MIN_BODY_BYTES {
            bail!(
                "{id} {}: status {}, body too small ({} bytes); not written",
                kind.as_str(),
                page.status,
                page.body.len()
            );
        }
        let clean = retailers::fixture::sanitize(&page.body);
        let path = fixture_path(&options.out_dir, options.retailer, kind);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        std::fs::write(&path, &clean).with_context(|| format!("writing {}", path.display()))?;
        println!(
            "{id} {}: status {}, {} bytes raw, {} bytes sanitized -> {}",
            kind.as_str(),
            page.status,
            page.body.len(),
            clean.len(),
            path.display()
        );
        if kind == Kind::Search {
            last_search = Some((target, clean));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "capture_fixtures/tests.rs"]
mod tests;
