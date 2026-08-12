//! Pure projection: window, bounds, lerp fractions. Exact integer work → f64 normalisation → f32 result.

use super::chart_model::{ChartBucket, ChartFill, ChartModel};
use crate::ids::InstrumentId;

/// Air above and below the visible extremes, as a percentage of their span, so the line never runs
/// along the plot border.
const PADDING_PERCENT: i128 = 5;

/// How the mid series is drawn. Both modes read the same buckets, so the toggle changes nothing but
/// the geometry; Line is the spec's default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChartMode {
    #[default]
    Line,
    Candles,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChartDomain {
    pub first: u64,
    pub last: u64,
}

impl ChartDomain {
    pub fn contains(self, index: u64) -> bool {
        index >= self.first && index <= self.last
    }

    pub fn width(self) -> u64 {
        self.last - self.first + 1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChartBounds {
    pub low: i64,
    pub high: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentPoint<'a> {
    pub bucket: &'a ChartBucket,
    pub is_run_start: bool,
}

pub fn domain(chart: &ChartModel, instrument: InstrumentId) -> Option<ChartDomain> {
    let mut buckets = chart.buckets(instrument);
    let oldest = buckets.next()?.index;
    let newest = buckets.last().map_or(oldest, |bucket| bucket.index);
    Some(domain_from_range(oldest, newest, chart.capacity()))
}

pub(crate) fn domain_from_range(oldest: u64, newest: u64, capacity: usize) -> ChartDomain {
    debug_assert!(
        oldest <= newest,
        "a series spans oldest to newest, got {oldest} > {newest}"
    );
    let capacity = capacity.max(1) as u64;
    let filled = newest - oldest + 1 >= capacity;
    let first = if filled { newest + 1 - capacity } else { oldest };
    ChartDomain {
        first,
        last: first + capacity - 1,
    }
}

pub fn bounds(
    chart: &ChartModel,
    instrument: InstrumentId,
    domain: ChartDomain,
) -> Option<ChartBounds> {
    let mut low = i64::MAX;
    let mut high = i64::MIN;
    for bucket in visible_buckets(chart, instrument, domain) {
        low = low.min(bucket.low_half_ticks);
        high = high.max(bucket.high_half_ticks);
    }
    for fill in visible_fills(chart, instrument, domain) {
        low = low.min(fill.half_ticks);
        high = high.max(fill.half_ticks);
    }
    if low > high {
        return None;
    }
    Some(pad(low, high))
}

pub fn segment_points(
    chart: &ChartModel,
    instrument: InstrumentId,
    domain: ChartDomain,
) -> impl Iterator<Item = SegmentPoint<'_>> {
    let mut previous: Option<u64> = None;
    visible_buckets(chart, instrument, domain).map(move |bucket| {
        let follows = previous.is_some_and(|previous| bucket.index == previous + 1);
        previous = Some(bucket.index);
        SegmentPoint {
            bucket,
            is_run_start: !follows || bucket.has_gap_before,
        }
    })
}

fn visible_buckets(
    chart: &ChartModel,
    instrument: InstrumentId,
    domain: ChartDomain,
) -> impl Iterator<Item = &ChartBucket> {
    chart
        .buckets(instrument)
        .filter(move |bucket| domain.contains(bucket.index))
}

pub fn visible_fills(
    chart: &ChartModel,
    instrument: InstrumentId,
    domain: ChartDomain,
) -> impl Iterator<Item = &ChartFill> {
    chart
        .fills(instrument)
        .filter(move |fill| domain.contains(fill.index))
}

pub fn x_fraction(index: u64, domain: ChartDomain) -> f32 {
    let span = domain.last.saturating_sub(domain.first);
    if span == 0 {
        return 0.0;
    }
    let offset = index.saturating_sub(domain.first) as f64;
    (offset / span as f64).clamp(0.0, 1.0) as f32
}

pub fn bucket_at_fraction(fraction: f32, domain: ChartDomain) -> u64 {
    let span = domain.last.saturating_sub(domain.first);
    let offset = (f64::from(fraction).clamp(0.0, 1.0) * span as f64).round() as u64;
    domain.first + offset
}

pub fn y_fraction(value: i64, bounds: ChartBounds) -> f32 {
    let span = i128::from(bounds.high) - i128::from(bounds.low);
    if span <= 0 {
        return 0.5;
    }
    let offset = (i128::from(value) - i128::from(bounds.low)) as f64;
    (offset / span as f64).clamp(0.0, 1.0) as f32
}

pub(crate) fn pad(low: i64, high: i64) -> ChartBounds {
    if low == high {
        return ChartBounds {
            low: low.saturating_sub(1),
            high: high.saturating_add(1),
        };
    }
    let span = i128::from(high) - i128::from(low);
    let padding = i64::try_from(span * PADDING_PERCENT / 100)
        .unwrap_or(i64::MAX)
        .max(1);
    ChartBounds {
        low: low.saturating_sub(padding),
        high: high.saturating_add(padding),
    }
}
