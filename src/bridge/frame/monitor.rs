//! The monitor's fixed bands: account, features and the latency grid.

use serde::Serialize;

use super::Styled;

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Account {
    pub notes: Vec<Styled>,
    pub captions: AccountCaptions,
    pub base: AssetLine,
    pub quote: AssetLine,
    pub sides: Sides,
    pub gate: Option<&'static str>,
    pub halt_reason: Option<&'static str>,
    pub reject: Styled,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct AccountCaptions {
    pub free: String,
    pub locked: String,
    pub value: String,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct AssetLine {
    pub label: String,
    pub free: String,
    pub locked: String,
    pub value: String,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct Sides {
    pub bid: SideCounts,
    pub ask: SideCounts,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct SideCounts {
    pub open: usize,
    pub inflight: usize,
    pub lost: usize,
    pub leaked: u64,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct FeatureLine {
    pub name: String,
    pub value: String,
    #[serde(rename = "changed")]
    pub has_changed: bool,
    #[serde(rename = "stale")]
    pub is_stale: bool,
    pub hover: String,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct Latency {
    pub header: &'static str,
    pub columns: Vec<&'static str>,
    pub rows: Vec<LatencyRow>,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct LatencyRow {
    pub label: &'static str,
    pub cells: Vec<Styled>,
}
