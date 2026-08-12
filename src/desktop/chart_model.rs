//! One rolling five-minute mid series per instrument, bucketed by spin interval off the book lane,
//! with this engine's fills marked over it. Zero `f64` — prices stay the exact half-tick integers the
//! DOM separator and monitor mid cell show — and the bucket index comes from event time alone, so a
//! spin carrying no trustworthy mid leaves a hole rather than an invented sample.

use std::mem;

use super::dom_view::{snapshot_mid, tick_index};
use super::history::BoundedHistory;
use crate::ids::{InstrumentId, Price, Side};
use crate::msg::ui::{UiBookSnapshot, UiEvent};
use crate::time::{DurationUs, TsUs};

/// The chart's window: five minutes of mid, one polymarket up/down window. Buckets older than this
/// leave the visible domain whether or not the ring still holds them.
const WINDOW_US: u64 = 300_000_000;

/// Ceiling on buckets retained per instrument, so a fast spin cadence cannot size the ring by
/// accident: 3_000 is the whole window at a 100 ms spin and already ~3× the ~1_060 physical pixels
/// the plot spans at the minimum window size, while an unclamped 1 ms spin would ask for 300_000.
const MAX_BUCKETS_PER_INSTRUMENT: usize = 3_000;

/// Ceiling on markers one bucket may contribute — the multiplier turning the bucket window into the
/// fill window. Two is the recorder's REAL bound: it arms at most one quote per side per spin, and
/// `hot/quant/toxicity/markouts.rs` disarms a side the moment a print fills it. Four is double that,
/// headroom for a strategy quoting more often without sizing the ring for a rate nothing produces.
const MAX_FILLS_PER_BUCKET: usize = 4;

/// Floor under the derived fill ring. A coarse spin leaves a window only a handful of buckets wide,
/// where the per-bucket ceiling (measured on the recorder's cadence, not on a bucket's duration) is
/// far too tight to survive minutes of prints.
const MIN_FILL_CAPACITY: usize = 256;

/// One spin interval of mid, in half-ticks. `open_half_ticks`/`close_half_ticks` are the first and
/// last mids banked inside the interval and `high_half_ticks`/`low_half_ticks` their extremes;
/// `has_gap_before` marks a bucket the line must not be drawn back from, because the book lane lost
/// snapshots its own interval should have carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChartBucket {
    pub index: u64,
    pub open_half_ticks: i64,
    pub high_half_ticks: i64,
    pub low_half_ticks: i64,
    pub close_half_ticks: i64,
    pub has_gap_before: bool,
}

/// One real fill, marked at its OWN price in half-ticks — never snapped to the bucket's mid, so
/// a fill away from the line reads as exactly that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChartFill {
    pub index: u64,
    pub half_ticks: i64,
    pub side: Side,
}

/// Whether the book lane dropped snapshots between an instrument's previous commit and this one. The
/// loss marks the bucket whose own interval it fell in — the open one when the revealing commit folds
/// into it, the next one pushed when it does not — so the line splits at that bucket instead of
/// drawing a straight segment over data that never arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookContinuity {
    Continuous,
    GapBefore,
}

struct InstrumentChart {
    tick_size: Option<Price>,
    buckets: BoundedHistory<ChartBucket>,
    fills: BoundedHistory<ChartFill>,
    gap_pending: bool,
}

impl InstrumentChart {
    fn new(tick_size: Option<Price>, bucket_capacity: usize) -> Self {
        Self {
            tick_size,
            buckets: BoundedHistory::new(bucket_capacity),
            fills: BoundedHistory::new(fill_capacity(bucket_capacity)),
            gap_pending: false,
        }
    }

    fn bank(&mut self, index: u64, mid_half_ticks: i64) {
        if let Some(newest) = self.buckets.last_mut() {
            if index < newest.index {
                return;
            }
            if index == newest.index {
                newest.high_half_ticks = newest.high_half_ticks.max(mid_half_ticks);
                newest.low_half_ticks = newest.low_half_ticks.min(mid_half_ticks);
                newest.close_half_ticks = mid_half_ticks;
                newest.has_gap_before |= mem::take(&mut self.gap_pending);
                return;
            }
        }
        self.buckets.push(ChartBucket {
            index,
            open_half_ticks: mid_half_ticks,
            high_half_ticks: mid_half_ticks,
            low_half_ticks: mid_half_ticks,
            close_half_ticks: mid_half_ticks,
            has_gap_before: mem::take(&mut self.gap_pending),
        });
    }
}

pub struct ChartModel {
    charts: Vec<InstrumentChart>,
    spin_interval: DurationUs,
    bucket_capacity: usize,
}

impl ChartModel {
    pub fn with_capacity(instrument_count: usize, spin_interval: DurationUs) -> Self {
        let bucket_capacity = bucket_capacity(spin_interval);
        Self {
            charts: (0..instrument_count)
                .map(|_| InstrumentChart::new(None, bucket_capacity))
                .collect(),
            spin_interval,
            bucket_capacity,
        }
    }

    pub fn configure(&mut self, tick_sizes: &[Option<Price>], spin_interval: DurationUs) {
        self.spin_interval = spin_interval;
        self.bucket_capacity = bucket_capacity(spin_interval);
        self.charts = tick_sizes
            .iter()
            .map(|tick_size| InstrumentChart::new(*tick_size, self.bucket_capacity))
            .collect();
    }

    pub fn apply_book(&mut self, snapshot: &UiBookSnapshot, continuity: BookContinuity) {
        let slot = snapshot.instrument.0 as usize;
        self.ensure_instruments(slot + 1);
        if continuity == BookContinuity::GapBefore {
            self.charts[slot].gap_pending = true;
        }
        let Some(index) = bucket_index(snapshot.event_ts_us, self.spin_interval) else { return };
        let Some(tick) = self.charts[slot].tick_size else { return };
        let Some(mid_half_ticks) = snapshot_mid(snapshot, tick) else { return };
        self.charts[slot].bank(index, mid_half_ticks);
    }

    pub fn apply_event(&mut self, event: &UiEvent) {
        match *event {
            UiEvent::Fill {
                instrument,
                event_ts_us,
                side,
                price,
                ..
            } => self.bank_fill(instrument, event_ts_us, side, price),
            UiEvent::Rotation { instrument, .. } => self.clear(instrument),
            _ => {}
        }
    }

    pub fn buckets(&self, instrument: InstrumentId) -> impl Iterator<Item = &ChartBucket> {
        self.charts
            .get(instrument.0 as usize)
            .into_iter()
            .flat_map(|chart| chart.buckets.iter_oldest_first())
    }

    pub fn fills(&self, instrument: InstrumentId) -> impl Iterator<Item = &ChartFill> {
        self.charts
            .get(instrument.0 as usize)
            .into_iter()
            .flat_map(|chart| chart.fills.iter_oldest_first())
    }

    pub fn capacity(&self) -> usize {
        self.bucket_capacity
    }

    pub fn spin_interval(&self) -> DurationUs {
        self.spin_interval
    }

    fn bank_fill(&mut self, instrument: InstrumentId, at: TsUs, side: Side, price: Price) {
        let slot = instrument.0 as usize;
        self.ensure_instruments(slot + 1);
        let Some(index) = bucket_index(at, self.spin_interval) else { return };
        let Some(tick) = self.charts[slot].tick_size else { return };
        let Some(half_ticks) = tick_index(price, tick).and_then(|grid| grid.checked_mul(2)) else {
            return;
        };
        self.charts[slot].fills.push(ChartFill {
            index,
            half_ticks,
            side,
        });
    }

    fn clear(&mut self, instrument: InstrumentId) {
        let capacity = self.bucket_capacity;
        let Some(chart) = self.charts.get_mut(instrument.0 as usize) else { return };
        *chart = InstrumentChart::new(chart.tick_size, capacity);
    }

    fn ensure_instruments(&mut self, len: usize) {
        while self.charts.len() < len {
            self.charts
                .push(InstrumentChart::new(None, self.bucket_capacity));
        }
    }
}

pub(crate) fn bucket_index(at: TsUs, spin_interval: DurationUs) -> Option<u64> {
    let micros = at.micros();
    let interval = u64::try_from(spin_interval.micros()).ok()?;
    if micros < 0 || interval == 0 {
        return None;
    }
    Some(micros as u64 / interval)
}

pub fn bucket_open_ts(index: u64, spin_interval: DurationUs) -> Option<TsUs> {
    let interval = u64::try_from(spin_interval.micros()).ok()?;
    if interval == 0 {
        return None;
    }
    i64::try_from(index.checked_mul(interval)?)
        .ok()
        .map(TsUs::from_micros)
}

pub(crate) fn bucket_capacity(spin_interval: DurationUs) -> usize {
    let interval = u64::try_from(spin_interval.micros()).unwrap_or(0);
    let buckets = WINDOW_US / interval.max(1);
    (buckets as usize).clamp(1, MAX_BUCKETS_PER_INSTRUMENT)
}

fn fill_capacity(bucket_capacity: usize) -> usize {
    bucket_capacity
        .saturating_mul(MAX_FILLS_PER_BUCKET)
        .max(MIN_FILL_CAPACITY)
}
