//! The mid chart and the risk chart under it. A full window resends hundreds of buckets, so their
//! wire keys are one or two letters while the field names here stay whole.

use serde::Serialize;

use crate::ids::Side;

use super::Styled;

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Chart {
    pub generation: u64,
    pub capacity: usize,
    pub first: Option<u64>,
    pub last: Option<u64>,
    pub low: Option<i64>,
    pub high: Option<i64>,
    pub axis: Vec<AxisTick>,
    pub buckets: Vec<ChartBucketLine>,
    pub fills: Vec<ChartFillLine>,
    pub note: Option<&'static str>,
    #[serde(rename = "fillsCaption")]
    pub has_fills_caption: bool,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct AxisTick {
    #[serde(rename = "v")]
    pub value: i64,
    pub label: String,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct ChartBucketLine {
    #[serde(rename = "i")]
    pub index: u64,
    #[serde(rename = "o")]
    pub open_half_ticks: i64,
    #[serde(rename = "h")]
    pub high_half_ticks: i64,
    #[serde(rename = "l")]
    pub low_half_ticks: i64,
    #[serde(rename = "c")]
    pub close_half_ticks: i64,
    #[serde(rename = "cl")]
    pub close_label: String,
    #[serde(rename = "t")]
    pub open_time: String,
    #[serde(rename = "gap")]
    pub has_gap_before: bool,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct ChartFillLine {
    #[serde(rename = "i")]
    pub index: u64,
    #[serde(rename = "ht")]
    pub half_ticks: i64,
    pub side: &'static str,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct Risk {
    pub generation: u64,
    pub series: &'static str,
    pub latest: Option<Styled>,
    pub bounds: RiskBounds,
    pub axis: RiskAxis,
    pub buckets: Vec<RiskBucketLine>,
    pub note: Option<&'static str>,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct RiskBounds {
    pub exposure: Option<Range>,
    pub pnl: Option<Range>,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct Range {
    pub low: i64,
    pub high: i64,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct RiskAxis {
    pub exposure: Vec<AxisTick>,
    pub pnl: Vec<AxisTick>,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct RiskBucketLine {
    #[serde(rename = "i")]
    pub index: u64,
    #[serde(rename = "e")]
    pub exposure_quote: i64,
    #[serde(rename = "p")]
    pub pnl_quote: i64,
    #[serde(rename = "el")]
    pub exposure_label: String,
    #[serde(rename = "pl")]
    pub pnl_label: String,
}

pub(crate) fn side_word(side: Side) -> &'static str {
    match side {
        Side::Buy => "buy",
        Side::Sell => "sell",
    }
}
