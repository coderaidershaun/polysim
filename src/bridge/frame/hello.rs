//! The opening line: the vocabulary the page needs before any engine reports in.

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Hello {
    pub protocol: u32,
    pub bridge: &'static str,
    pub polysim: String,
    pub tick_ms: u64,
    pub groupings: Groupings,
    pub levels: Levels,
    pub channels: ChannelWords,
}

#[derive(Serialize)]
pub(crate) struct Groupings {
    pub ticks: Vec<TickGrouping>,
    pub bps: Vec<BpsGrouping>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TickGrouping {
    pub per_bucket: i64,
    pub label: &'static str,
}

#[derive(Serialize)]
pub(crate) struct BpsGrouping {
    pub numerator: i64,
    pub denominator: i64,
    pub label: &'static str,
}

#[derive(Serialize)]
pub(crate) struct Levels {
    pub min: usize,
    pub max: usize,
    pub default: usize,
}

#[derive(Serialize)]
pub(crate) struct ChannelWords {
    pub system: ChannelWord,
    pub orders: ChannelWord,
    pub trades: ChannelWord,
    pub fills: ChannelWord,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChannelWord {
    pub label: &'static str,
    pub empty_note: &'static str,
}
