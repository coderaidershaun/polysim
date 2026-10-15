//! The four channel histories. The page keeps a ring per channel, so a tick sends only the rows
//! appended since the last one, and a whole ring when the page has none to patch.

use crate::bridge::frame::{ChannelRowLine, ChannelRows, Channels, TagLine};
use crate::desktop::model::UiModel;
use crate::desktop::monitor_model::{FillRow, OrderRow, SystemRow, TradeRow};
use crate::desktop::monitor_view::{MonitorFrame, instrument_names};
use crate::desktop::text::{Cell, ChannelRow, record_row};

use super::Encoder;

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
