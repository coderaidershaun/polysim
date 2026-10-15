//! The latency grid's words and roles: the header, one row per flow, and each cell's reading.

use crate::desktop::format;
use crate::msg::ui::UiLatencySummary;

use super::tone::Tone;

const MICROS_COLUMNS: usize = 6;

/// Column headers in paint order, the last one over the slot count rather than a latency.
pub(crate) const LATENCY_COLUMNS: [&str; MICROS_COLUMNS + 1] = [
    "EXCH>RECV",
    "RECV>QUEUE",
    "QUEUE WAIT",
    "PROCESS",
    "END-TO-END",
    "RND TRIP",
    "SLOTS",
];

pub(crate) const LATENCY_TITLE: &str = "LATENCY  us";

pub(crate) const LATENCY_ROWS: [(&str, Flow); 3] = [
    ("MARKET DATA", Flow::MarketData),
    ("ORDER TRIP", Flow::Execution),
    ("HOT PATH", Flow::HotPath),
];

#[derive(Debug, Clone, Copy)]
pub(crate) enum Flow {
    MarketData,
    Execution,
    HotPath,
}

impl Flow {
    fn means(self, summary: &UiLatencySummary) -> [Option<f64>; MICROS_COLUMNS] {
        let row = match self {
            Flow::MarketData => &summary.market_data,
            Flow::Execution => &summary.execution,
            Flow::HotPath => &summary.hot_path,
        };
        [
            row.exchange_to_received.mean_us(),
            row.received_to_queued.mean_us(),
            row.queue_wait.mean_us(),
            row.processing.mean_us(),
            row.end_to_end.mean_us(),
            row.order_round_trip.mean_us(),
        ]
    }

    /// Backlog is one figure across every input queue, so it reads on the hot path's row alone.
    fn slots(self, summary: &UiLatencySummary) -> Option<f64> {
        match self {
            Flow::HotPath => summary.backlog_ema,
            _ => None,
        }
    }
}

fn value_tone(value: Option<f64>) -> Tone {
    match value {
        Some(_) => Tone::Text,
        None => Tone::Text2,
    }
}

/// One row's cells, left to right: the six latency means, then the queue backlog.
pub(crate) fn latency_cells(
    flow: Flow,
    summary: Option<&UiLatencySummary>,
) -> [(String, Tone); MICROS_COLUMNS + 1] {
    let means = summary.map_or([None; MICROS_COLUMNS], |summary| flow.means(summary));
    let slots = summary.and_then(|summary| flow.slots(summary));
    std::array::from_fn(|index| {
        let mut text = String::new();
        match means.get(index) {
            Some(mean) => {
                format::write_opt_latency_micros(&mut text, *mean);
                (text, value_tone(*mean))
            }
            None => {
                format::write_opt_slots(&mut text, slots);
                (text, value_tone(slots))
            }
        }
    })
}
