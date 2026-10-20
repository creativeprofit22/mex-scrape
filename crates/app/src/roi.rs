//! Use case: ROI for many items in one call. The calculator page and the
//! search page both use it, so they always agree with
//! [`domain::calculate_roi`].

use domain::{RoiError, RoiInput, RoiResult, calculate_roi};

/// Most items accepted by one [`RoiBatch::calculate`] call.
pub const MAX_ROI_BATCH: usize = 100;

/// Why a batch was refused as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RoiBatchError {
    /// No items.
    #[error("no ROI items")]
    Empty,
    /// More than [`MAX_ROI_BATCH`] items.
    #[error("too many ROI items")]
    TooMany,
}

/// Computes ROI for up to [`MAX_ROI_BATCH`] items. Pure.
#[derive(Debug, Clone, Copy, Default)]
pub struct RoiBatch;

impl RoiBatch {
    /// One result per input, in input order; an item that cannot be
    /// computed gets its own [`RoiError`] without failing the batch.
    ///
    /// # Errors
    /// [`RoiBatchError`] if the batch is empty or too large.
    pub fn calculate(
        self,
        items: &[RoiInput],
    ) -> Result<Vec<Result<RoiResult, RoiError>>, RoiBatchError> {
        if items.is_empty() {
            return Err(RoiBatchError::Empty);
        }
        if items.len() > MAX_ROI_BATCH {
            return Err(RoiBatchError::TooMany);
        }
        Ok(items.iter().map(calculate_roi).collect())
    }
}

#[cfg(test)]
mod tests {
    use domain::PriceMxn;

    use super::*;

    fn input(units: u32) -> RoiInput {
        let p = |c| PriceMxn::from_centavos(c).unwrap();
        RoiInput {
            sell_price: p(50_000),
            buy_price: p(20_000),
            amazon_fees: p(7_500),
            prep_fee: p(1_000),
            shipping: p(500),
            monthly_fixed: p(10_000),
            units_per_month: units,
        }
    }

    #[test]
    fn matches_the_domain_calculation_item_by_item() {
        let items = [input(10), input(0), input(3)];
        let out = RoiBatch.calculate(&items).unwrap();
        assert_eq!(out.len(), 3);
        for (i, r) in items.iter().zip(&out) {
            assert_eq!(&calculate_roi(i), r);
        }
        assert_eq!(out[1], Err(RoiError::ZeroUnits));
    }

    #[test]
    fn refuses_empty_and_oversized_batches() {
        assert_eq!(RoiBatch.calculate(&[]), Err(RoiBatchError::Empty));
        let many = vec![input(1); MAX_ROI_BATCH + 1];
        assert_eq!(RoiBatch.calculate(&many), Err(RoiBatchError::TooMany));
        assert!(RoiBatch.calculate(&many[..MAX_ROI_BATCH]).is_ok());
    }
}
