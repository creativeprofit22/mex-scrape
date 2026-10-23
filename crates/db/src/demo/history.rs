//! A deterministic year of daily store prices per demo listing, shaped like
//! Mexican retail: noise around a slow drift, Hot Sale (late May) and Buen
//! Fin (mid-November) sales, higher December prices with post-Christmas
//! markdowns, and a few out-of-stock streaks. [`daily_points`] is pure;
//! [`seed_history`] writes it.
//!
//! All rates are basis points of the listing's price today (10 000 = 100 %).

use anyhow::Context as _;
use sqlx::{PgPool, Row as _};

use super::catalog::{DemoCatalog, ListingStock};

/// Days of history per listing, ending yesterday.
pub const HISTORY_DAYS: i64 = 365;

/// Milliseconds per day.
pub const DAY_MS: i64 = 86_400_000;

/// Lowest price, as basis points of today's price.
pub const MIN_BP: i64 = 7_000;
/// Highest price, as basis points of today's price.
pub const MAX_BP: i64 = 11_500;

const DRIFT_BP: i64 = 300;
const DRIFT_PERIOD: i64 = 14;
const JITTER_BP: i64 = 200;
const BLEND_DAYS: i64 = 14;
const HOT_SALE_DEPTH_BP: (i64, i64) = (1_700, 2_200);
const BUEN_FIN_DEPTH_BP: (i64, i64) = (2_000, 2_600);
const DECEMBER_UPLIFT_BP: (i64, i64) = (600, 1_000);
const MARKDOWN_BP: i64 = 500;
/// Out-of-stock streaks are placed this many days or more before today.
const STREAK_FREE_DAYS: i64 = 20;
const STREAKS: (i64, i64) = (1, 3);
const STREAK_LEN: (i64, i64) = (2, 5);
/// Days at the end of the history a sold-out listing is out of stock.
pub const SOLD_OUT_TAIL_DAYS: i64 = 3;

/// One day of a listing's history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayPoint {
    /// Days since the Unix epoch (UTC).
    pub day: i64,
    /// Store price in centavos, whole pesos.
    pub price_centavos: i64,
    /// Sold out that day.
    pub out_of_stock: bool,
}

/// `(year, month, day)` for days since the Unix epoch (UTC).
#[must_use]
pub fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    // In range for any day this module handles.
    (
        i32::try_from(year).unwrap_or(i32::MAX),
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}

/// Days since the Unix epoch for a civil date (UTC).
#[must_use]
pub fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(month);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Day of the week: 0 = Monday … 6 = Sunday.
#[must_use]
pub fn weekday(days: i64) -> i64 {
    // 1970-01-01 was a Thursday.
    (days + 3).rem_euclid(7)
}

/// Hot Sale: 9 days starting on the last Monday of May. Inclusive days.
#[must_use]
pub fn hot_sale(year: i32) -> (i64, i64) {
    let may_31 = days_from_civil(year, 5, 31);
    let start = may_31 - weekday(may_31);
    (start, start + 8)
}

/// Buen Fin: the 5 days ending on the third Monday of November. Inclusive.
#[must_use]
pub fn buen_fin(year: i32) -> (i64, i64) {
    let nov_1 = days_from_civil(year, 11, 1);
    let first_monday = nov_1 + (7 - weekday(nov_1)) % 7;
    let end = first_monday + 14;
    (end - 4, end)
}

/// A stable per-listing seed (FNV-1a), so listings do not move in lockstep.
#[must_use]
pub fn salt(retailer: &str, sku: &str) -> u64 {
    retailer
        .bytes()
        .chain([0])
        .chain(sku.bytes())
        .fold(0xcbf2_9ce4_8422_2325, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        })
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// A deterministic value in `lo..=hi` for `(salt, tag, n)`.
fn pick(salt: u64, tag: u64, n: i64, (lo, hi): (i64, i64)) -> i64 {
    let h = splitmix64(salt ^ splitmix64(tag.wrapping_mul(0x9e37) ^ n.cast_unsigned()));
    let span = (hi - lo + 1).cast_unsigned();
    lo + (h % span).cast_signed()
}

/// Slow drift: piecewise linear between per-fortnight anchors.
fn drift_bp(salt: u64, day: i64) -> i64 {
    let k = day.div_euclid(DRIFT_PERIOD);
    let into = day.rem_euclid(DRIFT_PERIOD);
    let a = pick(salt, 1, k, (-DRIFT_BP, DRIFT_BP));
    let b = pick(salt, 1, k + 1, (-DRIFT_BP, DRIFT_BP));
    a + (b - a) * into / DRIFT_PERIOD
}

/// The offset (basis points) a store sets for `day`: a season's list price
/// when one applies (sales are priced off the list, not the drift), else
/// the drift, marked down after Christmas.
fn offset_bp(salt: u64, day: i64) -> i64 {
    let (year, month, dom) = civil_from_days(day);
    let in_window = |(start, end): (i64, i64)| (start..=end).contains(&day);
    let season = pick(salt, 3, i64::from(year), (0, 1_000_000));
    if in_window(hot_sale(year)) {
        let (lo, hi) = HOT_SALE_DEPTH_BP;
        return -(lo + season % (hi - lo + 1));
    }
    if in_window(buen_fin(year)) {
        let (lo, hi) = BUEN_FIN_DEPTH_BP;
        return -(lo + season % (hi - lo + 1));
    }
    if month == 12 && dom <= 24 {
        let (lo, hi) = DECEMBER_UPLIFT_BP;
        return lo + season % (hi - lo + 1);
    }
    let drift = drift_bp(salt, day);
    if (month == 12 && dom >= 26) || (month == 1 && dom <= 6) {
        drift - MARKDOWN_BP
    } else {
        drift
    }
}

/// Out-of-stock days: `STREAKS` streaks of `STREAK_LEN` days, one per
/// equal segment of the history before the last `STREAK_FREE_DAYS`, plus
/// the last [`SOLD_OUT_TAIL_DAYS`] when the listing is sold out today.
fn out_of_stock(salt: u64, days_ago: i64, days: i64, sold_out: bool) -> bool {
    if sold_out && days_ago <= SOLD_OUT_TAIL_DAYS {
        return true;
    }
    let span = days - STREAK_FREE_DAYS;
    if span <= 0 || days_ago <= STREAK_FREE_DAYS {
        return false;
    }
    let streaks = pick(salt, 4, 0, STREAKS);
    let segment = span / streaks;
    (0..streaks).any(|i| {
        let len = pick(salt, 5, i, STREAK_LEN);
        let start =
            STREAK_FREE_DAYS + 1 + i * segment + pick(salt, 6, i, (0, (segment - len).max(0)));
        (start..start + len).contains(&days_ago)
    })
}

/// `days` daily points ending yesterday (`today_day - 1`), oldest first,
/// for a listing priced `base_centavos` today. Deterministic in all inputs.
///
/// Prices: drift of ±3 % over fortnights with ±2 % daily jitter; Hot Sale
/// −17…−22 %, Buen Fin −20…−26 %, Dec 1–24 +6…+10 % (depth per listing and
/// year); −5 % markdowns Dec 26–Jan 6. The last 14 days blend into
/// `base_centavos` so today's live price follows on. Clamped to
/// [`MIN_BP`]..=[`MAX_BP`] of base, whole pesos, at least $1.
#[must_use]
pub fn daily_points(
    base_centavos: i64,
    salt: u64,
    today_day: i64,
    days: i64,
    sold_out: bool,
) -> Vec<DayPoint> {
    (1..=days)
        .rev()
        .map(|days_ago| {
            let day = today_day - days_ago;
            let mut bp = offset_bp(salt, day) + pick(salt, 2, day, (-JITTER_BP, JITTER_BP));
            if days_ago <= BLEND_DAYS {
                bp = bp * days_ago / (BLEND_DAYS + 1);
            }
            let bp = (10_000 + bp).clamp(MIN_BP, MAX_BP);
            let centavos = base_centavos.saturating_mul(bp) / 10_000;
            DayPoint {
                day,
                price_centavos: ((centavos + 50) / 100).max(1) * 100,
                out_of_stock: out_of_stock(salt, days_ago, days, sold_out),
            }
        })
        .collect()
}

/// What [`seed_history`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HistoryReport {
    /// Listings given a history now.
    pub written: u32,
    /// Listings that already had one (observations are append-only).
    pub kept: u32,
}

/// Writes [`HISTORY_DAYS`] of [`daily_points`] for every catalog listing
/// that has no observations yet, at `now_ms`'s time of day.
///
/// # Errors
///
/// Database errors; redact before logging.
pub async fn seed_history(
    pool: &PgPool,
    catalog: &DemoCatalog,
    now_ms: i64,
) -> anyhow::Result<HistoryReport> {
    let today = now_ms.div_euclid(DAY_MS);
    let mut report = HistoryReport::default();
    for (_, listing) in catalog.listings() {
        let existing: i64 = sqlx::query(
            "SELECT count(*) AS n FROM price_observations WHERE retailer = $1 AND retailer_sku = $2",
        )
        .bind(&listing.retailer)
        .bind(&listing.sku)
        .fetch_one(pool)
        .await
        .context("counting observations")?
        .try_get("n")
        .context("counting observations")?;
        if existing > 0 {
            report.kept += 1;
            continue;
        }
        let points = daily_points(
            listing.price_centavos,
            salt(&listing.retailer, &listing.sku),
            today,
            HISTORY_DAYS,
            listing.availability == ListingStock::OutOfStock,
        );
        let prices: Vec<i64> = points.iter().map(|p| p.price_centavos).collect();
        let stock: Vec<&str> = points
            .iter()
            .map(|p| {
                if p.out_of_stock {
                    "out_of_stock"
                } else {
                    "in_stock"
                }
            })
            .collect();
        let times: Vec<i64> = points
            .iter()
            .map(|p| now_ms - (today - p.day) * DAY_MS)
            .collect();
        sqlx::query(
            "INSERT INTO price_observations \
             (retailer, retailer_sku, price_mxn, availability, observed_at) \
             SELECT $1, $2, p, a, to_timestamp(t / 1000.0) \
             FROM unnest($3::BIGINT[], $4::TEXT[], $5::BIGINT[]) AS x(p, a, t)",
        )
        .bind(&listing.retailer)
        .bind(&listing.sku)
        .bind(&prices)
        .bind(&stock)
        .bind(&times)
        .execute(pool)
        .await
        .context("storing observations")?;
        report.written += 1;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// 2026-10-23, the day the history model was written.
    fn today() -> i64 {
        days_from_civil(2026, 10, 23)
    }

    /// Mean price of the points whose civil date passes `keep`.
    fn mean_where(points: &[DayPoint], keep: impl Fn(i64) -> bool) -> Option<i64> {
        let picked: Vec<i64> = points
            .iter()
            .filter(|p| keep(p.day))
            .map(|p| p.price_centavos)
            .collect();
        let n = i64::try_from(picked.len()).ok().filter(|n| *n > 0)?;
        Some(picked.iter().sum::<i64>() / n)
    }

    type DayFilter = Box<dyn Fn(i64) -> bool>;

    /// Per-season mean prices of a history.
    fn season_means(points: &[DayPoint]) -> BTreeMap<&'static str, i64> {
        let mut out = BTreeMap::new();
        let Some(last) = points.last() else {
            return out;
        };
        let blend_start = last.day - BLEND_DAYS + 1;
        let windows: [(&'static str, DayFilter); 5] = [
            (
                "hot_sale",
                Box::new(|d| {
                    let (s, e) = hot_sale(civil_from_days(d).0);
                    (s..=e).contains(&d)
                }),
            ),
            (
                "before_hot_sale",
                Box::new(|d| {
                    let (_, m, dom) = civil_from_days(d);
                    (m == 4 && dom >= 20) || (m == 5 && dom <= 15)
                }),
            ),
            (
                "buen_fin",
                Box::new(|d| {
                    let (s, e) = buen_fin(civil_from_days(d).0);
                    (s..=e).contains(&d)
                }),
            ),
            (
                "october",
                Box::new(move |d| civil_from_days(d).1 == 10 && d < blend_start),
            ),
            (
                "december",
                Box::new(|d| {
                    let (_, m, dom) = civil_from_days(d);
                    m == 12 && dom <= 24
                }),
            ),
        ];
        for (name, keep) in windows {
            if let Some(mean) = mean_where(points, keep) {
                out.insert(name, mean);
            }
        }
        out
    }

    fn histories() -> Vec<(String, i64, bool, Vec<DayPoint>)> {
        let catalog = DemoCatalog::embedded().unwrap();
        catalog
            .listings()
            .map(|(_, l)| {
                let sold_out = l.availability == ListingStock::OutOfStock;
                (
                    format!("{}/{}", l.retailer, l.sku),
                    l.price_centavos,
                    sold_out,
                    daily_points(
                        l.price_centavos,
                        salt(&l.retailer, &l.sku),
                        today(),
                        HISTORY_DAYS,
                        sold_out,
                    ),
                )
            })
            .collect()
    }

    #[test]
    fn civil_dates_and_seasons_are_right() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(today()), (2026, 10, 23));
        assert_eq!(
            days_from_civil(2024, 2, 29) + 1,
            days_from_civil(2024, 3, 1)
        );
        assert_eq!(weekday(days_from_civil(2026, 10, 23)), 4); // a Friday
        assert_eq!(
            hot_sale(2025),
            (days_from_civil(2025, 5, 26), days_from_civil(2025, 6, 3))
        );
        assert_eq!(
            hot_sale(2026),
            (days_from_civil(2026, 5, 25), days_from_civil(2026, 6, 2))
        );
        assert_eq!(
            buen_fin(2025),
            (days_from_civil(2025, 11, 13), days_from_civil(2025, 11, 17))
        );
        assert_eq!(
            buen_fin(2026),
            (days_from_civil(2026, 11, 12), days_from_civil(2026, 11, 16))
        );
    }

    #[test]
    fn prices_are_positive_whole_pesos_within_bounds() {
        for (name, base, _, points) in histories() {
            assert_eq!(points.len(), 365, "{name}");
            assert_eq!(points.last().unwrap().day, today() - 1, "{name}");
            let low = base * MIN_BP / 10_000 - 100;
            let high = base * MAX_BP / 10_000 + 100;
            for p in &points {
                assert!(
                    p.price_centavos >= 100 && p.price_centavos % 100 == 0,
                    "{name}"
                );
                assert!((low..=high).contains(&p.price_centavos), "{name}");
            }
            let yesterday = points.last().unwrap().price_centavos;
            assert!(
                (yesterday - base).abs() * 100 <= base * 3,
                "{name}: {yesterday} vs {base}"
            );
        }
    }

    #[test]
    fn history_is_deterministic_and_not_periodic() {
        let a = daily_points(
            150_000,
            salt("liverpool", "1"),
            today(),
            HISTORY_DAYS,
            false,
        );
        let b = daily_points(
            150_000,
            salt("liverpool", "1"),
            today(),
            HISTORY_DAYS,
            false,
        );
        assert_eq!(a, b);
        assert_ne!(salt("liverpool", "1"), salt("liverpool", "2"));
        let prices: Vec<i64> = a.iter().map(|p| p.price_centavos).collect();
        for period in 2..=120 {
            assert!(
                prices.iter().zip(&prices[period..]).any(|(x, y)| x != y),
                "repeats every {period} days"
            );
        }
    }

    #[test]
    fn seasonal_drops_and_december_prices_appear() {
        for (name, _, _, points) in histories() {
            let m = season_means(&points);
            assert!(
                m["hot_sale"] * 100 <= m["before_hot_sale"] * 88,
                "{name}: {m:?}"
            );
            assert!(m["buen_fin"] * 100 <= m["october"] * 85, "{name}: {m:?}");
            assert!(m["december"] * 100 >= m["october"] * 102, "{name}: {m:?}");
        }
    }

    #[test]
    fn out_of_stock_streaks_are_rare_and_sold_out_listings_end_out() {
        let mut sold_out_listings = 0;
        for (name, _, sold_out, points) in histories() {
            let body = &points[..points.len() - 14];
            let mut runs = Vec::new();
            let mut run = 0;
            for p in body {
                if p.out_of_stock {
                    run += 1;
                } else if run > 0 {
                    runs.push(run);
                    run = 0;
                }
            }
            if run > 0 {
                runs.push(run);
            }
            assert!((1..=3).contains(&runs.len()), "{name}: {runs:?}");
            assert!(runs.iter().all(|r| (2..=5).contains(r)), "{name}: {runs:?}");
            let tail: Vec<bool> = points[points.len() - 3..]
                .iter()
                .map(|p| p.out_of_stock)
                .collect();
            assert_eq!(tail, [sold_out; 3], "{name}");
            sold_out_listings += usize::from(sold_out);
        }
        assert!(sold_out_listings >= 3);
    }
}
