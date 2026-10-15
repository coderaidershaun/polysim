//! The lines about the connection rather than the market: hello, link, lifecycle and catalog.

use std::time::Duration;

use crate::desktop::attachment::Screen;
use crate::desktop::dom_view::{
    DEFAULT_ROWS_PER_SIDE, DomGrouping, MAX_ROWS_PER_SIDE, MIN_ROWS_PER_SIDE,
};
use crate::desktop::format;
use crate::desktop::link_model::LinkStatus;
use crate::desktop::monitor_model::{FillRow, OrderRow, SystemRow, TradeRow};
use crate::desktop::text::{
    BPS_GROUPINGS, ChannelRow, TICK_GROUPINGS, button_label, execution_badge, is_peer_clickable,
    link_chips, screen_text,
};
use crate::msg::ui::UiCatalog;

use crate::bridge::frame::{
    BpsGrouping, Button, CatalogLine, ChannelWord, ChannelWords, Groupings, Hello, InstrumentLine,
    Levels, LifecycleLine, Line, LinkLine, Styled, TickGrouping, connection_word, run_word,
    screen_word,
};

const PROTOCOL: u32 = 1;
const BRIDGE: &str = "polysim-bridge";

/// The hello line: static vocabulary the page needs before any engine reports in.
pub fn hello_line(polysim: &str, tick: Duration) -> String {
    let hello = Hello {
        protocol: PROTOCOL,
        bridge: BRIDGE,
        polysim: polysim.to_owned(),
        tick_ms: u64::try_from(tick.as_millis()).unwrap_or(u64::MAX),
        groupings: Groupings {
            ticks: TICK_GROUPINGS
                .iter()
                .filter_map(|(grouping, label)| match *grouping {
                    DomGrouping::Ticks { per_bucket } => Some(TickGrouping { per_bucket, label }),
                    DomGrouping::Bps { .. } => None,
                })
                .collect(),
            bps: BPS_GROUPINGS
                .iter()
                .filter_map(|(grouping, label)| match *grouping {
                    DomGrouping::Bps {
                        numerator,
                        denominator,
                    } => Some(BpsGrouping {
                        numerator,
                        denominator,
                        label,
                    }),
                    DomGrouping::Ticks { .. } => None,
                })
                .collect(),
        },
        levels: Levels {
            min: MIN_ROWS_PER_SIDE,
            max: MAX_ROWS_PER_SIDE,
            default: DEFAULT_ROWS_PER_SIDE,
        },
        channels: ChannelWords {
            system: channel_word::<SystemRow>(),
            orders: channel_word::<OrderRow>(),
            trades: channel_word::<TradeRow>(),
            fills: channel_word::<FillRow>(),
        },
    };
    Line::Hello(&hello).to_json()
}

fn channel_word<R: ChannelRow>() -> ChannelWord {
    ChannelWord {
        label: R::CHANNEL.label(),
        empty_note: R::EMPTY_NOTE,
    }
}

pub(super) fn link_dto(status: &LinkStatus) -> LinkLine {
    let next = status.next_assertion();
    LinkLine {
        peer: status.peer.to_string(),
        peer_index: status.peer_index,
        peer_count: status.peer_count,
        session: status.session,
        chips: link_chips(status)
            .into_iter()
            .map(|(text, tone)| Styled::new(text, tone))
            .collect(),
        is_peer_clickable: is_peer_clickable(status),
        button: Button {
            label: button_label(next),
            assert: run_word(next),
        },
        connection: connection_word(status.connection),
    }
}

pub(super) fn lifecycle_dto(session: u64, screen: &Screen) -> LifecycleLine {
    let text = screen_text(screen);
    LifecycleLine {
        session,
        screen: screen_word(screen),
        title: text.title.to_owned(),
        title_tone: text.title_tone.as_str(),
        detail: text.detail.to_owned(),
        detail_tone: text.detail_tone.as_str(),
    }
}

pub(super) fn catalog_line(session: u64, catalog: &UiCatalog) -> String {
    let (badge, tone) = execution_badge(catalog.execution_mode);
    let line = CatalogLine {
        session,
        strategy_id: catalog.strategy_id.to_string(),
        title: catalog.window_title.to_string(),
        badge: Styled::new(badge.to_owned(), tone),
        spin_interval_us: catalog.spin_interval_us,
        instruments: catalog
            .instruments
            .iter()
            .map(|instrument| InstrumentLine {
                id: instrument.instrument_id.0,
                display: instrument.display.to_string(),
                base: instrument.base.to_string(),
                quote: instrument.quote.to_string(),
                tick: instrument.tick_size.map(|tick| {
                    let mut text = String::new();
                    format::write_venue_price(&mut text, tick, format::price_decimals(tick));
                    text
                }),
            })
            .collect(),
        feature_names: catalog
            .feature_names
            .iter()
            .map(ToString::to_string)
            .collect(),
    };
    Line::Catalog(&line).to_json()
}
