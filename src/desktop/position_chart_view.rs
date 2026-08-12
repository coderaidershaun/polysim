//! Pure projection of the risk series: which series shows, the padded quote bounds, the visible
//! buckets in paint order. No numeric helpers of its own — values are quote mantissas, so the mid
//! chart's `pad` and `y_fraction` serve unchanged. And deliberately NO domain function: the mid banks
//! on every valid two-sided book while a position banks once per spin and only once the engine holds
//! a mark, so deriving `first` here would put one pointer fraction on different buckets in the two
//! stacked charts. Callers take the window as a parameter and are handed the mid chart's.

use super::chart_view::{ChartBounds, ChartDomain, pad};
use super::position_chart_model::{PositionBucket, PositionModel};
use crate::ids::{FIXED_SCALE, InstrumentId};

/// Min quote span: prevents nano-scale axis labels on flat instruments.
const MIN_SPAN_QUOTE: i64 = FIXED_SCALE;

/// Quote-mantissa window (newtype over ChartBounds to catch unit swaps at compile time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuoteBounds(ChartBounds);

impl QuoteBounds {
    pub fn as_chart_bounds(self) -> ChartBounds {
        self.0
    }
}

impl From<QuoteBounds> for ChartBounds {
    fn from(bounds: QuoteBounds) -> Self {
        bounds.as_chart_bounds()
    }
}

/// Which mark-to-market series to show (both use same buckets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RiskSeries {
    #[default]
    Exposure,
    Pnl,
}

impl RiskSeries {
    /// Fractional digits on a readout of this series. Two, because both series are money — stated
    /// on the type both readouts already share so the crosshair and the header cannot drift.
    pub const READOUT_DECIMALS: usize = 2;

    pub fn value(self, bucket: &PositionBucket) -> i64 {
        match self {
            RiskSeries::Exposure => bucket.exposure_quote,
            RiskSeries::Pnl => bucket.pnl_quote,
        }
    }
}

/// Padded quote window always including zero (baseline needed to read long/short).
pub fn bounds(
    positions: &PositionModel,
    instrument: InstrumentId,
    series: RiskSeries,
    domain: ChartDomain,
) -> Option<QuoteBounds> {
    let mut values =
        visible_buckets(positions, instrument, domain).map(|bucket| series.value(bucket));
    let first = values.next()?;
    let (low, high) = values.fold((first.min(0), first.max(0)), |(low, high), value| {
        (low.min(value), high.max(value))
    });
    let (low, high) = widen_to_minimum_span(low, high);
    Some(QuoteBounds(pad(low, high)))
}

/// Buckets inside window, oldest first; consecutive indices only are continuous (gaps = unbanked spins).
pub fn visible_buckets(
    positions: &PositionModel,
    instrument: InstrumentId,
    domain: ChartDomain,
) -> impl Iterator<Item = &PositionBucket> {
    positions
        .buckets(instrument)
        .filter(move |bucket| domain.contains(bucket.index))
}

/// Widen short span to MIN_SPAN_QUOTE (zero always stays in bounds).
fn widen_to_minimum_span(low: i64, high: i64) -> (i64, i64) {
    let span = high.saturating_sub(low);
    if span >= MIN_SPAN_QUOTE {
        return (low, high);
    }
    let deficit = MIN_SPAN_QUOTE - span;
    let below = deficit / 2;
    (
        low.saturating_sub(below),
        high.saturating_add(deficit - below),
    )
}
