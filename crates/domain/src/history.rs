//! Keepa-style price-window statistics over observed retailer prices.
//!
//! A window of `N` days covers `[now - N * DAY_MS, now]` (both ends
//! inclusive). Observations dated after `now` and out-of-stock observations
//! (sold-out prices are often stale) are ignored. The average is the
//! arithmetic mean of the counted observations, rounded half up to the
//! centavo.

use crate::{Availability, PriceMxn, UnixMillis};

/// Milliseconds in one day.
pub const DAY_MS: i64 = 86_400_000;

/// Window lengths, in days, reported by price stats.
pub const STATS_WINDOWS: [u16; 3] = [30, 90, 365];

/// One observed price used for stats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceSample {
    /// Observed selling price.
    pub price_mxn: PriceMxn,
    /// Stock state when observed.
    pub availability: Availability,
    /// When it was observed.
    pub observed_at: UnixMillis,
}

/// Low, high and average price over one window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceWindow {
    /// Window length in days.
    pub days: u16,
    /// Lowest counted price; `None` when nothing was counted.
    pub low: Option<PriceMxn>,
    /// Highest counted price; `None` when nothing was counted.
    pub high: Option<PriceMxn>,
    /// Mean counted price (half up to the centavo); `None` when empty.
    pub average: Option<PriceMxn>,
    /// Number of counted observations.
    pub observations: u32,
}

/// Stats over the last `days` days before `now`. Input order does not
/// matter.
#[must_use]
pub fn window_stats(samples: &[PriceSample], now: UnixMillis, days: u16) -> PriceWindow {
    let start = now.0.saturating_sub(i64::from(days).saturating_mul(DAY_MS));
    let mut low: Option<PriceMxn> = None;
    let mut high: Option<PriceMxn> = None;
    let mut sum: i128 = 0;
    let mut count: u32 = 0;
    for s in samples.iter().filter(|s| {
        s.availability != Availability::OutOfStock
            && s.observed_at.0 >= start
            && s.observed_at.0 <= now.0
    }) {
        low = Some(low.map_or(s.price_mxn, |l| l.min(s.price_mxn)));
        high = Some(high.map_or(s.price_mxn, |h| h.max(s.price_mxn)));
        sum += i128::from(s.price_mxn.centavos());
        count = count.saturating_add(1);
    }
    let average = (count > 0)
        .then(|| {
            let n = i128::from(count);
            // Mean of non-negative i64 values fits i64 and is non-negative.
            let mean = (2 * sum + n) / (2 * n);
            i64::try_from(mean)
                .ok()
                .and_then(|c| PriceMxn::from_centavos(c).ok())
        })
        .flatten();
    PriceWindow {
        days,
        low,
        high,
        average,
        observations: count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_760_000_000_000;

    fn s(centavos: i64, days_ago_ms: i64, availability: Availability) -> PriceSample {
        PriceSample {
            price_mxn: PriceMxn::from_centavos(centavos).unwrap(),
            availability,
            observed_at: UnixMillis(NOW - days_ago_ms),
        }
    }

    fn c(p: Option<PriceMxn>) -> Option<i64> {
        p.map(PriceMxn::centavos)
    }

    #[test]
    fn window_boundaries_are_inclusive() {
        let samples = [
            s(100, 30 * DAY_MS, Availability::InStock),
            s(999, 30 * DAY_MS + 1, Availability::InStock),
            s(300, 0, Availability::InStock),
        ];
        let w = window_stats(&samples, UnixMillis(NOW), 30);
        assert_eq!(w.days, 30);
        assert_eq!(w.observations, 2);
        assert_eq!(
            (c(w.low), c(w.high), c(w.average)),
            (Some(100), Some(300), Some(200))
        );
        assert_eq!(window_stats(&samples, UnixMillis(NOW), 90).observations, 3);
    }

    #[test]
    fn future_and_out_of_stock_rows_are_ignored() {
        let samples = [
            s(100, -1, Availability::InStock),
            s(50, DAY_MS, Availability::OutOfStock),
            s(200, DAY_MS, Availability::Unknown),
            s(400, DAY_MS, Availability::PreOrder),
        ];
        let w = window_stats(&samples, UnixMillis(NOW), 30);
        assert_eq!(w.observations, 2);
        assert_eq!(
            (c(w.low), c(w.high), c(w.average)),
            (Some(200), Some(400), Some(300))
        );
    }

    #[test]
    fn empty_window_has_no_prices() {
        let w = window_stats(&[], UnixMillis(NOW), 365);
        assert_eq!(
            w,
            PriceWindow {
                days: 365,
                low: None,
                high: None,
                average: None,
                observations: 0
            }
        );
    }

    #[test]
    fn mean_rounds_half_up() {
        let a = |prices: &[i64]| {
            let v: Vec<_> = prices
                .iter()
                .map(|p| s(*p, DAY_MS, Availability::InStock))
                .collect();
            c(window_stats(&v, UnixMillis(NOW), 30).average)
        };
        assert_eq!(a(&[100, 101]), Some(101)); // 100.5 -> 101
        assert_eq!(a(&[100, 100, 101]), Some(100)); // 100.33 -> 100
        assert_eq!(a(&[100, 101, 101]), Some(101)); // 100.67 -> 101
    }
}
