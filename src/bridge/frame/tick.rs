//! The tick line: one optional section per panel, present only when it changed.

use serde::Serialize;

use crate::desktop::chart_view::ChartMode;
use crate::desktop::dom_view::DomUnit;
use crate::desktop::position_chart_view::RiskSeries;

use super::channels::Channels;
use super::chart::{Chart, Risk};
use super::ladder::{Ladder, Summary};
use super::monitor::{Account, FeatureLine, Latency};

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TickLine {
    pub session: u64,
    pub seq: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<View>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ladder: Option<Ladder>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<Summary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<Account>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub features: Option<Vec<FeatureLine>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<Channels>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency: Option<Latency>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chart: Option<Chart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk: Option<Risk>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gaps: Option<Gaps>,
}

impl TickLine {
    /// True when no section changed, so there is nothing to say.
    pub(crate) fn is_empty(&self) -> bool {
        self.view.is_none()
            && self.ladder.is_none()
            && self.summary.is_none()
            && self.account.is_none()
            && self.features.is_none()
            && self.channels.is_none()
            && self.latency.is_none()
            && self.chart.is_none()
            && self.risk.is_none()
            && self.gaps.is_none()
    }
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct View {
    pub instrument: u16,
    pub chart_mode: &'static str,
    pub dom_unit: &'static str,
    pub dom_grouping: GroupingView,
    pub dom_levels: usize,
    pub risk_series: &'static str,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(tag = "unit", rename_all = "lowercase")]
pub(crate) enum GroupingView {
    Bps {
        numerator: i64,
        denominator: i64,
        label: &'static str,
    },
    Ticks {
        #[serde(rename = "perBucket")]
        per_bucket: i64,
        label: &'static str,
    },
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct Gaps {
    pub books: u64,
    pub events: u64,
}

pub(crate) fn mode_word(mode: ChartMode) -> &'static str {
    match mode {
        ChartMode::Line => "line",
        ChartMode::Candles => "candles",
    }
}

pub(crate) fn unit_word(unit: DomUnit) -> &'static str {
    match unit {
        DomUnit::Ticks => "ticks",
        DomUnit::Bps => "bps",
    }
}

pub(crate) fn series_word(series: RiskSeries) -> &'static str {
    match series {
        RiskSeries::Exposure => "exposure",
        RiskSeries::Pnl => "pnl",
    }
}
