//! What the link actor remembers of the charts' inputs, per instrument, so a workstation that
//! attaches late can be sent the window it missed. The actor owns it alone.

use std::collections::VecDeque;
use std::ops::Range;

use crate::ids::InstrumentId;
use crate::msg::ui::{
    UI_CHART_WINDOW, UI_HISTORY_SAMPLES, UiBookSnapshot, UiEvent, UiHistory, UiHistorySample,
};
use crate::time::TsUs;

/// A busy instrument produces about 50 samples a second, so five minutes is about 15,000. When
/// this many are held the oldest goes first, and a backfill then covers less than the window.
const MEMORY_SAMPLES_PER_INSTRUMENT: usize = 16_384;

/// Sample indices count from the instrument's first sample ever and never reset, so a reader can
/// tell where a backfill is even after older samples left.
pub(super) struct InstrumentMemory {
    samples: VecDeque<UiHistorySample>,
    evicted: u64,
    newest_ts_us: Option<TsUs>,
    last_book_seq: Option<u64>,
    is_gap_pending: bool,
}

impl InstrumentMemory {
    fn new() -> Self {
        Self {
            samples: VecDeque::with_capacity(MEMORY_SAMPLES_PER_INSTRUMENT),
            evicted: 0,
            newest_ts_us: None,
            last_book_seq: None,
            is_gap_pending: false,
        }
    }

    /// The highest stamp pushed since the last clear. Its sample may have left the memory already.
    pub(super) fn newest_ts_us(&self) -> Option<TsUs> {
        self.newest_ts_us
    }

    pub(super) fn start_index(&self) -> u64 {
        self.evicted
    }

    pub(super) fn end_index(&self) -> u64 {
        self.evicted + self.samples.len() as u64
    }

    /// Up to a datagram of the samples whose indices lie in `indices`. The range must start at or
    /// past `start_index()`, because a sample that left can no longer be sent.
    pub(super) fn chunk(
        &self,
        instrument: InstrumentId,
        cutoff_ts_us: TsUs,
        indices: Range<u64>,
    ) -> UiHistory {
        let first_index = indices.start;
        let available = indices
            .end
            .min(self.end_index())
            .saturating_sub(first_index);
        let len =
            usize::try_from(available).map_or(UI_HISTORY_SAMPLES, |n| n.min(UI_HISTORY_SAMPLES));
        let offset = (first_index - self.evicted) as usize;
        let mut samples = [UiHistorySample::EMPTY; UI_HISTORY_SAMPLES];
        for (slot, held) in samples
            .iter_mut()
            .zip(self.samples.range(offset..offset + len))
        {
            *slot = *held;
        }
        UiHistory {
            instrument,
            cutoff_ts_us,
            first_index,
            len: len as u8,
            samples,
        }
    }

    fn push(&mut self, sample: UiHistorySample) {
        let at = sample.event_ts_us();
        let newest = self.newest_ts_us.map_or(at, |newest| newest.max(at));
        self.newest_ts_us = Some(newest);
        if self.samples.len() >= MEMORY_SAMPLES_PER_INSTRUMENT {
            self.evict_front();
        }
        let oldest_kept = newest - UI_CHART_WINDOW;
        while self
            .samples
            .front()
            .is_some_and(|front| front.event_ts_us() < oldest_kept)
        {
            self.evict_front();
        }
        self.samples.push_back(sample);
    }

    fn evict_front(&mut self) {
        self.samples.pop_front();
        self.evicted += 1;
    }

    fn clear(&mut self) {
        self.evicted += self.samples.len() as u64;
        self.samples.clear();
        self.newest_ts_us = None;
        self.is_gap_pending = false;
    }
}

pub(super) struct ChartMemory {
    instruments: Vec<InstrumentMemory>,
}

impl ChartMemory {
    pub(super) fn new() -> Self {
        Self {
            instruments: Vec::new(),
        }
    }

    pub(super) fn instrument(&self, slot: usize) -> Option<&InstrumentMemory> {
        self.instruments.get(slot)
    }

    /// A sequence jump means snapshots were lost, and the next mid carries that as its gap flag. A
    /// book with no usable top adds no sample, just as the chart draws no mid for it.
    pub(super) fn on_book(&mut self, snapshot: &UiBookSnapshot) {
        let memory = self.instrument_mut(snapshot.instrument);
        memory.is_gap_pending |= snapshot.lost_since(memory.last_book_seq) > 0;
        memory.last_book_seq = Some(snapshot.seq);
        let Some(top) = snapshot.top_of_book() else { return };
        let has_gap_before = std::mem::take(&mut memory.is_gap_pending);
        memory.push(UiHistorySample::Mid {
            event_ts_us: snapshot.event_ts_us,
            top,
            has_gap_before,
        });
    }

    pub(super) fn on_event(&mut self, event: &UiEvent) {
        match *event {
            UiEvent::Position {
                instrument,
                event_ts_us,
                exposure_quote,
                pnl_quote,
                ..
            } => self
                .instrument_mut(instrument)
                .push(UiHistorySample::Position {
                    event_ts_us,
                    exposure_quote,
                    pnl_quote,
                }),
            UiEvent::Fill {
                instrument,
                event_ts_us,
                side,
                price,
                ..
            } => self.instrument_mut(instrument).push(UiHistorySample::Fill {
                event_ts_us,
                side,
                price,
            }),
            UiEvent::Rotation { instrument, .. } => self.instrument_mut(instrument).clear(),
            _ => {}
        }
    }

    fn instrument_mut(&mut self, instrument: InstrumentId) -> &mut InstrumentMemory {
        let slot = usize::from(instrument.0);
        while self.instruments.len() <= slot {
            self.instruments.push(InstrumentMemory::new());
        }
        &mut self.instruments[slot]
    }
}
