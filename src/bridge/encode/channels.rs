//! The four channel histories, as rows newest first. The page keeps a ring per channel, so a tick
//! sends only the rows appended since the last one, and a whole ring when the page has none to patch.

use serde::Serialize;

use crate::desktop::model::UiModel;
use crate::desktop::monitor_model::{FillRow, OrderRow, SystemRow, TradeRow};
use crate::desktop::monitor_view::{MonitorFrame, instrument_names};
use crate::desktop::text::{Cell, ChannelRow, record_row};

use super::Encoder;

#[derive(Serialize, Default)]
pub(super) struct Channels {
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<ChannelRows>,
    #[serde(skip_serializing_if = "Option::is_none")]
    orders: Option<ChannelRows>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trades: Option<ChannelRows>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fills: Option<ChannelRows>,
}

impl Channels {
    fn is_empty(&self) -> bool {
        self.system.is_none()
            && self.orders.is_none()
            && self.trades.is_none()
            && self.fills.is_none()
    }
}

#[derive(Serialize)]
struct ChannelRows {
    appended: u64,
    #[serde(rename = "full")]
    is_full: bool,
    rows: Vec<ChannelRowLine>,
}

/// A key a row kind has no cell for is left out, not sent empty.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct ChannelRowLine {
    #[serde(skip_serializing_if = "Option::is_none")]
    time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tag: Option<TagLine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text_tone: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    qty: Option<String>,
    detail: String,
}

#[derive(Serialize)]
struct TagLine {
    #[serde(rename = "liq", skip_serializing_if = "Option::is_none")]
    liquidity: Option<&'static str>,
    side: &'static str,
    tone: &'static str,
}

impl Encoder {
    pub(super) fn channels(&mut self, model: &UiModel) -> Option<Channels> {
        let instrument_names = instrument_names(model);
        let frame = MonitorFrame::for_selected(model, &instrument_names);
        let channels = Channels {
            system: self.channel_rows::<SystemRow>(&frame),
            orders: self.channel_rows::<OrderRow>(&frame),
            trades: self.channel_rows::<TradeRow>(&frame),
            fills: self.channel_rows::<FillRow>(&frame),
        };
        (!channels.is_empty()).then_some(channels)
    }

    fn channel_rows<R: ChannelRow>(&mut self, frame: &MonitorFrame<'_>) -> Option<ChannelRows> {
        let appended = R::appended(frame);
        let count = R::count(frame);
        let slot = &mut self.sent.channels[R::CHANNEL.index()];
        let (due, is_full) = match *slot {
            None => (count, true),
            Some(sent) if appended > sent => {
                let newer = usize::try_from(appended - sent).unwrap_or(usize::MAX);
                (newer.min(count), false)
            }
            Some(_) => return None,
        };
        *slot = Some(appended);
        let rows = (0..due)
            .filter_map(|index| R::at(frame, index))
            .map(|row| row_line(&row, frame))
            .collect();
        Some(ChannelRows {
            appended,
            is_full,
            rows,
        })
    }
}

fn row_line<R: ChannelRow>(row: &R, frame: &MonitorFrame<'_>) -> ChannelRowLine {
    let mut cells = Vec::new();
    row.cells(frame, &mut cells);
    let mut line = ChannelRowLine {
        detail: record_row(row, frame),
        ..ChannelRowLine::default()
    };
    for cell in cells {
        match cell {
            Cell::Time(text) => line.time = Some(text),
            Cell::Tag(tag) => {
                line.tag = Some(TagLine {
                    liquidity: tag.liquidity,
                    side: tag.side,
                    tone: tag.tone.as_str(),
                });
            }
            Cell::Description { text, tone } => {
                line.text = Some(text);
                line.text_tone = Some(tone.as_str());
            }
            Cell::Price(text) => line.price = Some(text),
            Cell::Qty(text) => line.qty = Some(text),
            Cell::Detail(_) => {}
        }
    }
    line
}
