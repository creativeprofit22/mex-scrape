//! Return-on-investment calculation for one unit bought at a retailer and
//! sold on Amazon.
//!
//! All money is integer centavos and all ratios are integer basis points
//! (1 bp = 0.01 %), so results are exact and deterministic. Formulas:
//!
//! - `fixed_per_unit = round_half_up(monthly_fixed / units_per_month)`
//! - `total_cost = buy + prep + shipping + fixed_per_unit` (cash invested)
//! - `profit = sell - amazon_fees - total_cost` (may be negative)
//! - `roi_bp = round_half_away_from_zero(profit * 10000 / total_cost)`
//! - `margin_bp = round_half_away_from_zero(profit * 10000 / sell)`

use std::fmt;

use crate::PriceMxn;

/// Basis points in 100 %.
const BP: i128 = 10_000;

/// Inputs for [`calculate_roi`]. Every amount is per unit except
/// `monthly_fixed`, which is spread over `units_per_month`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoiInput {
    /// Amazon selling price.
    pub sell_price: PriceMxn,
    /// Retailer purchase price.
    pub buy_price: PriceMxn,
    /// Amazon fees (referral, FBA, ...) for one unit at `sell_price`.
    pub amazon_fees: PriceMxn,
    /// Prep-center fee per unit.
    pub prep_fee: PriceMxn,
    /// Inbound shipping per unit.
    pub shipping: PriceMxn,
    /// Monthly fixed costs (subscriptions, storage, ...).
    pub monthly_fixed: PriceMxn,
    /// Units sold per month, used to spread `monthly_fixed`.
    pub units_per_month: u32,
}

/// Result of [`calculate_roi`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoiResult {
    /// Share of the monthly fixed costs carried by one unit.
    pub fixed_cost_per_unit: PriceMxn,
    /// Cash invested per unit: buy + prep + shipping + fixed share.
    pub total_cost: PriceMxn,
    /// Profit per unit in centavos (negative for a loss).
    pub profit_centavos: i64,
    /// Profit over total cost, in basis points; `None` when the cost is 0.
    pub roi_bp: Option<i64>,
    /// Profit over selling price, in basis points; `None` when the price is 0.
    pub margin_bp: Option<i64>,
}

/// Why [`calculate_roi`] could not compute a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoiError {
    /// Monthly fixed costs are set but `units_per_month` is 0.
    ZeroUnits,
    /// An intermediate amount does not fit in 64 bits.
    Overflow,
}

impl fmt::Display for RoiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroUnits => f.write_str("monthly fixed costs need at least one unit per month"),
            Self::Overflow => f.write_str("ROI amounts overflow"),
        }
    }
}

impl std::error::Error for RoiError {}

/// Computes profit, ROI and margin for one unit. Pure and deterministic.
///
/// # Errors
/// [`RoiError::ZeroUnits`] if `monthly_fixed > 0` and `units_per_month == 0`;
/// [`RoiError::Overflow`] if an amount exceeds 64 bits.
pub fn calculate_roi(input: &RoiInput) -> Result<RoiResult, RoiError> {
    let c = |p: PriceMxn| i128::from(p.centavos());
    let fixed = if input.monthly_fixed.centavos() == 0 {
        0
    } else if input.units_per_month == 0 {
        return Err(RoiError::ZeroUnits);
    } else {
        div_round_half_away(c(input.monthly_fixed), i128::from(input.units_per_month))
    };
    let total = c(input.buy_price) + c(input.prep_fee) + c(input.shipping) + fixed;
    let profit = c(input.sell_price) - c(input.amazon_fees) - total;
    let ratio = |den: i128| -> Result<Option<i64>, RoiError> {
        if den == 0 {
            Ok(None)
        } else {
            to_i64(div_round_half_away(profit * BP, den)).map(Some)
        }
    };
    Ok(RoiResult {
        fixed_cost_per_unit: to_price(fixed)?,
        total_cost: to_price(total)?,
        profit_centavos: to_i64(profit)?,
        roi_bp: ratio(total)?,
        margin_bp: ratio(c(input.sell_price))?,
    })
}

/// `n / d` rounded half away from zero; `d` must be positive.
fn div_round_half_away(n: i128, d: i128) -> i128 {
    let q = n / d;
    let r = n % d;
    if 2 * r.abs() >= d { q + n.signum() } else { q }
}

fn to_i64(v: i128) -> Result<i64, RoiError> {
    i64::try_from(v).map_err(|_| RoiError::Overflow)
}

fn to_price(v: i128) -> Result<PriceMxn, RoiError> {
    PriceMxn::from_centavos(to_i64(v)?).map_err(|_| RoiError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(c: i64) -> PriceMxn {
        PriceMxn::from_centavos(c).unwrap()
    }

    fn input() -> RoiInput {
        RoiInput {
            sell_price: p(59_900),
            buy_price: p(30_000),
            amazon_fees: p(12_050),
            prep_fee: p(1_500),
            shipping: p(2_000),
            monthly_fixed: p(150_000),
            units_per_month: 300,
        }
    }

    #[test]
    fn worked_example() {
        let r = calculate_roi(&input()).unwrap();
        assert_eq!(r.fixed_cost_per_unit, p(500));
        assert_eq!(r.total_cost, p(34_000));
        assert_eq!(r.profit_centavos, 13_850);
        assert_eq!(r.roi_bp, Some(4_074));
        assert_eq!(r.margin_bp, Some(2_312));
    }

    #[test]
    fn fixed_share_rounds_to_the_centavo() {
        let mut i = input();
        i.units_per_month = 3;
        i.monthly_fixed = p(100_000);
        assert_eq!(calculate_roi(&i).unwrap().fixed_cost_per_unit, p(33_333));
        i.monthly_fixed = p(200_000);
        assert_eq!(calculate_roi(&i).unwrap().fixed_cost_per_unit, p(66_667));
    }

    #[test]
    fn loss_rounds_away_from_zero() {
        // cost 30000, sell 25000 => profit -5000 => roi -1666.67 bp => -1667.
        let i = RoiInput {
            sell_price: p(25_000),
            buy_price: p(30_000),
            amazon_fees: p(0),
            prep_fee: p(0),
            shipping: p(0),
            monthly_fixed: p(0),
            units_per_month: 0,
        };
        let r = calculate_roi(&i).unwrap();
        assert_eq!(r.profit_centavos, -5_000);
        assert_eq!(r.roi_bp, Some(-1_667));
        assert_eq!(r.margin_bp, Some(-2_000));
    }

    #[test]
    fn zero_cost_or_price_gives_no_ratio() {
        let zero = RoiInput {
            sell_price: p(0),
            buy_price: p(0),
            amazon_fees: p(0),
            prep_fee: p(0),
            shipping: p(0),
            monthly_fixed: p(0),
            units_per_month: 0,
        };
        let r = calculate_roi(&zero).unwrap();
        assert_eq!((r.roi_bp, r.margin_bp, r.profit_centavos), (None, None, 0));
    }

    #[test]
    fn fixed_costs_need_units() {
        let mut i = input();
        i.units_per_month = 0;
        assert_eq!(calculate_roi(&i), Err(RoiError::ZeroUnits));
    }

    #[test]
    fn overflow_is_an_error() {
        let mut i = input();
        i.buy_price = p(i64::MAX);
        i.shipping = p(i64::MAX);
        assert_eq!(calculate_roi(&i), Err(RoiError::Overflow));
    }
}
