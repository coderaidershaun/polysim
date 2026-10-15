//! The protocol lines as data. Field order is wire order, and every name is part of what the page
//! codes against, so nothing here is renamed casually.

mod channels;
mod chart;
mod hello;
mod ladder;
mod link;
mod monitor;
mod tick;

use serde::Serialize;

use crate::desktop::text::Tone;

pub(crate) use channels::{ChannelRowLine, ChannelRows, Channels, TagLine};
pub(crate) use chart::{
    AxisTick, Chart, ChartBucketLine, ChartFillLine, Range, Risk, RiskAxis, RiskBounds,
    RiskBucketLine, side_word,
};
pub(crate) use hello::{
    BpsGrouping, ChannelWord, ChannelWords, Groupings, Hello, Levels, TickGrouping,
};
pub(crate) use ladder::{
    Chevron, Chevrons, Ladder, LadderRow, OrderCell, Summary, SummaryCaptions, ring_word,
    status_word,
};
pub(crate) use link::{
    Button, ByeLine, CatalogLine, InstrumentLine, LifecycleLine, LinkLine, connection_word,
    run_word, screen_word,
};
pub(crate) use monitor::{
    Account, AccountCaptions, AssetLine, FeatureLine, Latency, LatencyRow, SideCounts, Sides,
};
pub(crate) use tick::{Gaps, GroupingView, TickLine, View, mode_word, series_word, unit_word};

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(crate) enum Line<'a> {
    Hello(&'a Hello),
    Link(&'a LinkLine),
    Lifecycle(&'a LifecycleLine),
    Catalog(&'a CatalogLine),
    Tick(&'a TickLine),
    Bye(&'a ByeLine),
}

impl Line<'_> {
    /// One protocol line, without its newline.
    pub(crate) fn to_json(&self) -> String {
        match serde_json::to_string(self) {
            Ok(line) => line,
            Err(error) => {
                unreachable!("protocol lines hold plain data, which always serialises: {error}")
            }
        }
    }
}

/// A piece of text and the role it wears.
#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct Styled {
    pub text: String,
    pub tone: &'static str,
}

impl Styled {
    pub(crate) fn new(text: String, tone: Tone) -> Self {
        Self {
            text,
            tone: tone.as_str(),
        }
    }
}
