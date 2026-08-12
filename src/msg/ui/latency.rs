//! Engine self-timing distilled for display: the ten-minute stage rollups and the input-ring
//! backlog, folded to count+sum so a panel can render a mean without carrying every percentile.

use crate::hot::metrics::{Category, MetricsSnapshot, Stage, StageStat};

/// Count and sum, never a pre-divided mean: folding two categories together has to add before it
/// divides, and a mean cannot be added.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UiLatencyCell {
    pub count: u64,
    pub sum_us: i64,
}

impl UiLatencyCell {
    /// `None` when nothing was measured — a stage with no samples is not a stage that ran in zero.
    pub fn mean_us(self) -> Option<f64> {
        (self.count > 0).then(|| self.sum_us as f64 / self.count as f64)
    }

    fn add(&mut self, stat: StageStat) {
        self.count += stat.count;
        self.sum_us = self.sum_us.saturating_add(stat.sum_us);
    }
}

/// One display row: the stages a reader compares side by side. `ExchangeInternal` is deliberately
/// absent — it stays a log-only diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UiLatencyRow {
    pub exchange_to_received: UiLatencyCell,
    pub received_to_queued: UiLatencyCell,
    pub queue_wait: UiLatencyCell,
    pub processing: UiLatencyCell,
    pub end_to_end: UiLatencyCell,
    pub order_round_trip: UiLatencyCell,
}

impl UiLatencyRow {
    fn fold(snapshot: &MetricsSnapshot, categories: &[Category]) -> Self {
        let mut row = Self::default();
        for &category in categories {
            row.exchange_to_received
                .add(snapshot.stage(category, Stage::ExchangeToReceived));
            row.received_to_queued
                .add(snapshot.stage(category, Stage::ReceivedToQueued));
            row.queue_wait
                .add(snapshot.stage(category, Stage::QueueWait));
            row.processing
                .add(snapshot.stage(category, Stage::Processing));
            row.end_to_end
                .add(snapshot.stage(category, Stage::EndToEnd));
            row.order_round_trip
                .add(snapshot.stage(category, Stage::OrderRoundTrip));
        }
        row
    }

    /// The engine's own two stages across every category. The venue-facing four stay empty: summing
    /// exch->recv over categories the engine never received from venues answers no question.
    fn fold_engine_stages(snapshot: &MetricsSnapshot) -> Self {
        let mut row = Self::default();
        for category in Category::ALL {
            row.queue_wait
                .add(snapshot.stage(category, Stage::QueueWait));
            row.processing
                .add(snapshot.stage(category, Stage::Processing));
        }
        row
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UiLatencySummary {
    pub market_data: UiLatencyRow,
    pub execution: UiLatencyRow,
    pub hot_path: UiLatencyRow,
    /// One-minute half-life EWMA of the messages waiting across every input ring — a count, not a
    /// rate. `None` only before the first spin has been sampled; a keeping-up engine reads 0.0.
    pub backlog_ema: Option<f64>,
}

const MARKET_DATA: [Category; 4] = [
    Category::Trade,
    Category::BookDelta,
    Category::BookSnapshot,
    Category::Kline,
];

impl UiLatencySummary {
    pub fn from_snapshot(snapshot: &MetricsSnapshot) -> Self {
        Self {
            market_data: UiLatencyRow::fold(snapshot, &MARKET_DATA),
            execution: UiLatencyRow::fold(snapshot, &[Category::Exec]),
            hot_path: UiLatencyRow::fold_engine_stages(snapshot),
            backlog_ema: snapshot.backlog_ema,
        }
    }
}
