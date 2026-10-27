//! Merges the engine's remembered chart inputs into the live charts. They go through the charts'
//! own fold into scratch models first, so a backfilled bucket equals the one a live tab built.

use std::ops::Range;

use super::model::{ChartModel, bucket_index};
use super::position_model::PositionModel;
use crate::ids::{InstrumentId, Price};
use crate::msg::ui::{UiHistory, UiHistorySample};
use crate::time::{DurationUs, TsUs};

/// Where one instrument's backfill has got to.
struct BackfillSpan {
    cutoff_ts_us: TsUs,
    next_index: u64,
}

#[derive(Default)]
struct InstrumentBackfill {
    span: Option<BackfillSpan>,
    rotated_ts_us: Option<TsUs>,
}

pub(crate) struct ChartBackfill {
    chart: ChartModel,
    positions: PositionModel,
    instruments: Vec<InstrumentBackfill>,
}

impl ChartBackfill {
    pub(crate) fn new() -> Self {
        Self {
            chart: ChartModel::with_capacity(0, DurationUs::ZERO),
            positions: PositionModel::with_capacity(0, DurationUs::ZERO),
            instruments: Vec::new(),
        }
    }

    pub(crate) fn configure(&mut self, tick_sizes: &[Option<Price>], spin_interval: DurationUs) {
        self.chart.configure(tick_sizes, spin_interval);
        self.positions.configure(tick_sizes.len(), spin_interval);
        self.instruments = tick_sizes
            .iter()
            .map(|_| InstrumentBackfill::default())
            .collect();
    }

    /// A chunk repeated or overlapped by an earlier one folds nothing twice, and a chunk after a
    /// lost one starts the instrument's merge over. The cutoff bucket and everything after it stay
    /// with the live feed. A chunk of an instrument outside the catalog is ignored.
    pub(crate) fn apply(
        &mut self,
        history: &UiHistory,
        live_chart: &mut ChartModel,
        live_positions: &mut PositionModel,
    ) {
        let instrument = history.instrument;
        let Some(entry) = self.instruments.get_mut(instrument.0 as usize) else { return };
        if entry
            .rotated_ts_us
            .is_some_and(|rotated| history.cutoff_ts_us < rotated)
        {
            return;
        }
        let spin_interval = self.chart.spin_interval();
        let Some(cutoff_bucket) = bucket_index(history.cutoff_ts_us, spin_interval) else { return };
        let samples = history.samples();
        let Some(end_index) = history.first_index.checked_add(samples.len() as u64) else {
            return;
        };
        let is_continuation = entry.span.as_ref().is_some_and(|held| {
            held.cutoff_ts_us == history.cutoff_ts_us && history.first_index <= held.next_index
        });
        if !is_continuation {
            self.chart.clear(instrument);
            self.positions.clear(instrument);
            entry.span = Some(BackfillSpan {
                cutoff_ts_us: history.cutoff_ts_us,
                next_index: history.first_index,
            });
        }
        let Some(held) = entry.span.as_mut() else { return };
        let already_folded =
            usize::try_from(held.next_index - history.first_index.min(held.next_index))
                .unwrap_or(usize::MAX);
        held.next_index = held.next_index.max(end_index);
        let fresh = samples.iter().skip(already_folded);
        let Some(span_buckets) = self.fold_before(instrument, fresh, cutoff_bucket) else { return };
        live_chart.replace_span(instrument, &span_buckets, &self.chart);
        live_positions.replace_span(instrument, &span_buckets, &self.positions);
    }

    /// Folds the samples that lie before the cutoff bucket into the scratch models, and returns
    /// the buckets they touched.
    fn fold_before<'a>(
        &mut self,
        instrument: InstrumentId,
        samples: impl Iterator<Item = &'a UiHistorySample>,
        cutoff_bucket: u64,
    ) -> Option<Range<u64>> {
        let spin_interval = self.chart.spin_interval();
        let mut folded: Option<Range<u64>> = None;
        for sample in samples {
            let Some(bucket) = bucket_index(sample.event_ts_us(), spin_interval) else { continue };
            if bucket >= cutoff_bucket {
                continue;
            }
            self.chart.apply_sample(instrument, sample);
            self.positions.apply_sample(instrument, sample);
            folded = Some(match folded {
                Some(range) => range.start.min(bucket)..range.end.max(bucket + 1),
                None => bucket..bucket + 1,
            });
        }
        folded
    }

    /// A rotation starts a new market, so any half-merged backfill of the old one is dropped. A
    /// chunk that describes the old market can still arrive after the live rotation, so the
    /// rotation's stamp is kept to refuse it.
    pub(crate) fn forget(&mut self, instrument: InstrumentId, at: TsUs) {
        let Some(entry) = self.instruments.get_mut(instrument.0 as usize) else { return };
        *entry = InstrumentBackfill {
            span: None,
            rotated_ts_us: Some(at),
        };
        self.chart.clear(instrument);
        self.positions.clear(instrument);
    }
}
