//! Capped manual check of which retailer pages Bright Data can reach.
//!
//! `cargo run -p worker --bin access_check` checks robots.txt only (free).
//! Add `--live` to also fetch each allowed sample page once (paid, retries
//! off) through the channel production uses: the Browser API for page kinds
//! where `RetailerInfo::access_for` is `Browser` (needs
//! `BRIGHTDATA_BROWSER_ZONE`), otherwise Web Unlocker. Writes a dated matrix to
//! `docs/brightdata-access.md` (or `--out PATH`). Flags: `--max-urls N`
//! (default 36, hard cap 36). Bodies and secrets are never written.

use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, bail};
use brightdata::robots::{Access, HttpRobotsSource, RobotsChecker};
use brightdata::runs::SystemClock;
use brightdata::{
    BrightDataClient, BrightDataConfig, BrightDataError, CountryCode, RefusalReason, TargetUrl,
    Transport, UnlockRequest,
};
use http_guard::{Redactor, Secret};
use retailers::{AccessMode, PageKind, RetailerInfo};

/// Most URLs one run may touch, whatever the flags say.
const HARD_CAP: usize = 36;
const DEFAULT_OUT: &str = "docs/brightdata-access.md";

/// Result for one sample page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cell {
    Allowed,
    BlockedRobots,
    BlockedPolicy,
    Error,
    /// Robots allow it but no live request was made.
    LivePending,
    /// Over the URL cap.
    NotChecked,
    /// Browser API page, but `BRIGHTDATA_BROWSER_ZONE` is unset; no request made.
    NeedsBrowserZone,
}

impl Cell {
    const fn label(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::BlockedRobots => "blocked by robots.txt",
            Self::BlockedPolicy => "blocked by policy",
            Self::Error => "error",
            Self::LivePending => "live check pending",
            Self::NotChecked => "not checked (cap)",
            Self::NeedsBrowserZone => "not checked (needs BRIGHTDATA_BROWSER_ZONE)",
        }
    }
}

/// Matrix cell for a live Bright Data KYC refusal: robots.txt refusals
/// (our local parser may be laxer than Bright Data's) vs policy refusals.
const fn cell_for_refusal(reason: RefusalReason) -> Cell {
    match reason {
        RefusalReason::RobotsDisallowed | RefusalReason::BrowserRobots => Cell::BlockedRobots,
        RefusalReason::Policy => Cell::BlockedPolicy,
    }
}

/// Which Bright Data product a live check goes through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    Unlocker,
    Browser,
    /// The page needs the Browser API but no browser zone is configured.
    NoBrowserZone,
}

/// Live channel for one sample page: the same choice production makes via
/// [`RetailerInfo::access_for`]. Browser pages never fall back to Unlocker.
fn channel_for(retailer: &RetailerInfo, kind: PageKind, has_browser_zone: bool) -> Channel {
    match retailer.access_for(kind) {
        AccessMode::Browser if has_browser_zone => Channel::Browser,
        AccessMode::Browser => Channel::NoBrowserZone,
        AccessMode::Unlocker => Channel::Unlocker,
    }
}

struct Options {
    live: bool,
    max_urls: usize,
    out: String,
}

fn parse_args(args: &[String]) -> anyhow::Result<Options> {
    let mut options = Options {
        live: false,
        max_urls: HARD_CAP,
        out: DEFAULT_OUT.to_owned(),
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--live" => options.live = true,
            "--max-urls" => {
                let value = iter.next().context("--max-urls needs a number")?;
                options.max_urls = value.parse().context("--max-urls must be a number")?;
            }
            "--out" => options.out = iter.next().context("--out needs a path")?.clone(),
            other => bail!("unknown argument: {other}"),
        }
    }
    options.max_urls = options.max_urls.min(HARD_CAP);
    Ok(options)
}

/// `YYYY-MM-DD` (UTC) for days since the Unix epoch.
fn civil_date(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

fn today() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    civil_date(secs.div_euclid(86_400))
}

/// Renders the matrix; one row per retailer, one column per page kind.
fn render(date: &str, live: bool, cells: &[(&RetailerInfo, Vec<(PageKind, Cell)>)]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Bright Data access matrix\n");
    let _ = writeln!(
        out,
        "Checked on {date} (UTC) by `cargo run -p worker --bin access_check`."
    );
    let mode = if live {
        "robots.txt plus one live request per allowed page, through Web Unlocker or, for cells marked *(Browser API)*, the Browser API"
    } else {
        "robots.txt only; the policy column is **live check pending** (run with `--live` and Bright Data credentials)"
    };
    let _ = writeln!(out, "Mode: {mode}.\n");
    let _ = writeln!(
        out,
        "Meaning: *blocked by robots.txt* pages need KYC approval before Bright Data serves them (`ub_bad_endpoint_robots`); *blocked by policy* is a `policy_` refusal; *error* is any other failure (including a sample URL that no longer exists). No page bodies are stored.\n"
    );
    let _ = writeln!(out, "| Retailer | Search | Category | Product |");
    let _ = writeln!(out, "| --- | --- | --- | --- |");
    for (retailer, row) in cells {
        let cell = |kind: PageKind| {
            let label = row
                .iter()
                .find(|(k, _)| *k == kind)
                .map_or("not checked (cap)", |(_, c)| c.label());
            if retailer.access_for(kind) == AccessMode::Browser {
                format!("{label} (Browser API)")
            } else {
                label.to_owned()
            }
        };
        let _ = writeln!(
            out,
            "| {} (`{}`) | {} | {} | {} |",
            retailer.name,
            retailer.hosts.join(", "),
            cell(PageKind::Search),
            cell(PageKind::Category),
            cell(PageKind::Product),
        );
    }
    out
}

fn secret(name: &str) -> anyhow::Result<Option<Secret>> {
    Secret::from_lookup(name, |n: &str| std::env::var_os(n)).map_err(|e| anyhow::anyhow!("{e}"))
}

/// Live client, its redactor, and whether a Browser API zone is configured.
fn live_client() -> anyhow::Result<(BrightDataClient, Redactor, bool)> {
    let key = secret("BRIGHTDATA_API_KEY")?;
    let zone = secret("BRIGHTDATA_UNLOCKER_ZONE")?;
    let browser_zone = secret("BRIGHTDATA_BROWSER_ZONE")?;
    let has_browser_zone = browser_zone.is_some();
    let redactor = Redactor::new(key.iter().chain(zone.iter()).chain(browser_zone.iter()));
    let mut config = BrightDataConfig::from_secrets(key, zone)?.with_browser_zone(browser_zone);
    config.retry.max_retries = 0;
    let client = BrightDataClient::new(config).map_err(|e| anyhow::anyhow!("{}", e.message()))?;
    Ok((client, redactor, has_browser_zone))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = parse_args(&args)?;
    let robots = RobotsChecker::new(
        Box::new(HttpRobotsSource::new(retailers::hosts()).context("building robots client")?),
        Box::new(SystemClock),
    );
    let live = if options.live {
        Some(live_client()?)
    } else {
        None
    };
    let mut budget = options.max_urls;
    let mut matrix = Vec::new();
    for retailer in retailers::all() {
        let mut row = Vec::new();
        for kind in PageKind::ALL {
            let Some((_, url)) = retailer.samples.iter().find(|(k, _)| *k == kind) else {
                continue;
            };
            if budget == 0 {
                row.push((kind, Cell::NotChecked));
                continue;
            }
            budget -= 1;
            let target = TargetUrl::parse(url).context("registry URL is invalid")?;
            let cell = if robots.check(&target).await == Access::Disallowed {
                Cell::BlockedRobots
            } else if let Some((client, redactor, has_browser_zone)) = &live {
                let request = UnlockRequest {
                    url: target,
                    country: Some(CountryCode::MX),
                };
                let result = match channel_for(retailer, kind, *has_browser_zone) {
                    Channel::Unlocker => Some(client.unlock(&request).await),
                    Channel::Browser => Some(client.browse(&request).await),
                    Channel::NoBrowserZone => None,
                };
                match result {
                    None => Cell::NeedsBrowserZone,
                    Some(Ok(_)) => Cell::Allowed,
                    Some(Err(BrightDataError::BlockedNeedsKyc { reason, .. })) => {
                        cell_for_refusal(reason)
                    }
                    Some(Err(error)) => {
                        eprintln!(
                            "{} {}: {}",
                            retailer.id,
                            kind.as_str(),
                            redactor.sanitize(error.message())
                        );
                        Cell::Error
                    }
                }
            } else {
                Cell::LivePending
            };
            row.push((kind, cell));
        }
        matrix.push((retailer, row));
    }
    let text = render(&today(), options.live, &matrix);
    std::fs::write(&options.out, text).with_context(|| format!("writing {}", options.out))?;
    println!("wrote {}", options.out);
    Ok(())
}

#[cfg(test)]
#[path = "access_check/tests.rs"]
mod tests;
