//! The DOM ladder and the quote summary band above the account.

use serde::Serialize;

use crate::desktop::dom_view::DomStatus;
use crate::desktop::text::Ring;

use super::Styled;

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Ladder {
    pub status: &'static str,
    pub pill: Option<Styled>,
    #[serde(rename = "muted")]
    pub is_muted: bool,
    pub mid: Styled,
    pub mid_half_ticks: Option<i64>,
    pub ticks_per_bucket: i64,
    pub rows: usize,
    pub asks: Vec<LadderRow>,
    pub bids: Vec<LadderRow>,
    pub chevrons: Chevrons,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct LadderRow {
    pub tick: i64,
    pub price: String,
    pub public: Option<String>,
    pub order: Option<OrderCell>,
    pub want: Option<String>,
    pub ring: Option<&'static str>,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct OrderCell {
    pub qty: String,
    pub word: &'static str,
    pub tone: &'static str,
}

#[derive(Serialize, Clone, PartialEq, Default)]
pub(crate) struct Chevrons {
    pub up: Option<Chevron>,
    pub down: Option<Chevron>,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct Chevron {
    pub text: String,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct Summary {
    pub captions: SummaryCaptions,
    pub bid: Styled,
    pub mid: Styled,
    pub ask: Styled,
    pub hover: &'static str,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct SummaryCaptions {
    pub bid: &'static str,
    pub mid: &'static str,
    pub ask: &'static str,
}

pub(crate) fn status_word(status: DomStatus) -> &'static str {
    match status {
        DomStatus::Live => "live",
        DomStatus::Stale => "stale",
        DomStatus::AwaitingBook => "awaiting",
        DomStatus::Disconnected => "disconnected",
    }
}

pub(crate) fn ring_word(ring: Ring) -> &'static str {
    match ring {
        Ring::Confirmed => "confirmed",
        Ring::InFlight => "inflight",
        Ring::Lost => "lost",
        Ring::Desired => "desired",
    }
}
