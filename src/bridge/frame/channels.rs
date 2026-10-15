//! The four channel histories as rows newest first.

use serde::Serialize;

#[derive(Serialize, Default)]
pub(crate) struct Channels {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<ChannelRows>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orders: Option<ChannelRows>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trades: Option<ChannelRows>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fills: Option<ChannelRows>,
}

impl Channels {
    pub(crate) fn is_empty(&self) -> bool {
        self.system.is_none()
            && self.orders.is_none()
            && self.trades.is_none()
            && self.fills.is_none()
    }
}

#[derive(Serialize)]
pub(crate) struct ChannelRows {
    pub appended: u64,
    #[serde(rename = "full")]
    pub is_full: bool,
    pub rows: Vec<ChannelRowLine>,
}

/// A key a row kind has no cell for is left out, not sent empty.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChannelRowLine {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<TagLine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_tone: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qty: Option<String>,
    pub detail: String,
}

#[derive(Serialize)]
pub(crate) struct TagLine {
    #[serde(rename = "liq", skip_serializing_if = "Option::is_none")]
    pub liquidity: Option<&'static str>,
    pub side: &'static str,
    pub tone: &'static str,
}
