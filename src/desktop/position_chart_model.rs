//! One rolling five-minute exposure/PnL series per instrument, bucketed through the mid chart's own
//! spin arithmetic. The wire carries these as `f64` and they convert back to quote mantissas ONCE
//! here, so every bound, tick and label downstream is the exact integer maths the mid chart already
//! runs — its helpers serve unchanged rather than growing `f64` twins.

use super::chart_model::{bucket_capacity, bucket_index};
use super::history::BoundedHistory;
use crate::ids::{InstrumentId, fixed_mantissa};
use crate::msg::ui::UiEvent;
use crate::time::{DurationUs, TsUs};

/// Absolute state at spin interval end, never summed; dropped frame costs resolution only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PositionBucket {
    pub index: u64,
    pub exposure_quote: i64,
    pub pnl_quote: i64,
}

/// Rolling 5-min exposure/PnL per instrument, sized from catalog, grows on demand.
pub struct PositionModel {
    series: Vec<BoundedHistory<PositionBucket>>,
    spin_interval: DurationUs,
    bucket_capacity: usize,
    rejected_frames: u64,
}

impl PositionModel {
    pub fn with_capacity(instrument_count: usize, spin_interval: DurationUs) -> Self {
        let bucket_capacity = bucket_capacity(spin_interval);
        Self {
            series: (0..instrument_count)
                .map(|_| BoundedHistory::new(bucket_capacity))
                .collect(),
            spin_interval,
            bucket_capacity,
            rejected_frames: 0,
        }
    }

    pub fn configure(&mut self, instrument_count: usize, spin_interval: DurationUs) {
        self.spin_interval = spin_interval;
        self.bucket_capacity = bucket_capacity(spin_interval);
        self.series = (0..instrument_count)
            .map(|_| BoundedHistory::new(self.bucket_capacity))
            .collect();
    }

    pub fn apply_event(&mut self, event: &UiEvent) {
        match *event {
            UiEvent::Position {
                instrument,
                event_ts_us,
                exposure_quote,
                pnl_quote,
                ..
            } => self.bank_position(instrument, event_ts_us, exposure_quote, pnl_quote),
            UiEvent::Rotation { instrument, .. } => self.clear(instrument),
            _ => {}
        }
    }

    pub fn buckets(&self, instrument: InstrumentId) -> impl Iterator<Item = &PositionBucket> {
        self.series
            .get(instrument.0 as usize)
            .into_iter()
            .flat_map(BoundedHistory::iter_oldest_first)
    }

    pub fn latest(&self, instrument: InstrumentId) -> Option<PositionBucket> {
        self.series
            .get(instrument.0 as usize)?
            .iter_newest_first()
            .next()
            .copied()
    }

    /// Buckets per instrument (5-min window, same cadence as mid chart for shared domain).
    pub fn capacity(&self) -> usize {
        self.bucket_capacity
    }

    /// Position frames a peer sent as inf, NaN or past the quote-mantissa range. Surfaced beside the
    /// account band's other silent-loss count: a chart that just goes flat says nothing.
    pub fn rejected_frames(&self) -> u64 {
        self.rejected_frames
    }

    fn bank_position(
        &mut self,
        instrument: InstrumentId,
        at: TsUs,
        exposure_quote: f64,
        pnl_quote: f64,
    ) {
        let slot = instrument.0 as usize;
        self.ensure_instruments(slot + 1);
        let (Some(index), Some(exposure_quote), Some(pnl_quote)) = (
            bucket_index(at, self.spin_interval),
            fixed_mantissa(exposure_quote),
            fixed_mantissa(pnl_quote),
        ) else {
            self.rejected_frames += 1;
            return;
        };
        self.bank(
            slot,
            PositionBucket {
                index,
                exposure_quote,
                pnl_quote,
            },
        );
    }

    fn bank(&mut self, slot: usize, bucket: PositionBucket) {
        let series = &mut self.series[slot];
        if let Some(newest) = series.last_mut() {
            // Non-monotonic event time -> drop if inside settled bucket (can't rewrite painted history).
            if bucket.index < newest.index {
                return;
            }
            if bucket.index == newest.index {
                *newest = bucket;
                return;
            }
        }
        series.push(bucket);
    }

    /// Rotation clears mark until 2-sided book re-marks; engine emits Position only while marked, so silence must never read as no-change.
    fn clear(&mut self, instrument: InstrumentId) {
        let capacity = self.bucket_capacity;
        let Some(series) = self.series.get_mut(instrument.0 as usize) else { return };
        *series = BoundedHistory::new(capacity);
    }

    fn ensure_instruments(&mut self, len: usize) {
        while self.series.len() < len {
            self.series.push(BoundedHistory::new(self.bucket_capacity));
        }
    }
}
