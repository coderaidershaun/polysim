//! The lines about the connection rather than the market: hello, link, lifecycle and catalog, and
//! the last word before the bridge exits.

use std::time::Duration;

use serde::Serialize;

use crate::desktop::attachment::Screen;
use crate::desktop::dom_view::{
    DEFAULT_ROWS_PER_SIDE, DomGrouping, MAX_ROWS_PER_SIDE, MIN_ROWS_PER_SIDE,
};
use crate::desktop::format;
use crate::desktop::link_model::{ConnectionState, LinkStatus};
use crate::desktop::monitor_model::{FillRow, OrderRow, SystemRow, TradeRow};
use crate::desktop::text::{
    BPS_GROUPINGS, ChannelRow, TICK_GROUPINGS, button_label, execution_badge, is_peer_clickable,
    link_chips, screen_text,
};
use crate::link::RunState;
use crate::msg::ui::UiCatalog;

use super::{Line, Styled};

const PROTOCOL: u32 = 1;
const BRIDGE: &str = "polysim-bridge";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Hello {
    protocol: u32,
    bridge: &'static str,
    polysim: String,
    tick_ms: u64,
    groupings: Groupings,
    levels: Levels,
    channels: ChannelWords,
}

#[derive(Serialize)]
struct Groupings {
    ticks: Vec<TickGrouping>,
    bps: Vec<BpsGrouping>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TickGrouping {
    per_bucket: i64,
    label: &'static str,
}

#[derive(Serialize)]
struct BpsGrouping {
    numerator: i64,
    denominator: i64,
    label: &'static str,
}

#[derive(Serialize)]
struct Levels {
    min: usize,
    max: usize,
    default: usize,
}

#[derive(Serialize)]
struct ChannelWords {
    system: ChannelWord,
    orders: ChannelWord,
    trades: ChannelWord,
    fills: ChannelWord,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChannelWord {
    label: &'static str,
    empty_note: &'static str,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct LinkLine {
    peer: String,
    peer_index: usize,
    peer_count: usize,
    session: u64,
    chips: Vec<Styled>,
    #[serde(rename = "peerClickable")]
    is_peer_clickable: bool,
    button: Button,
    connection: &'static str,
}

#[derive(Serialize, Clone, PartialEq)]
struct Button {
    label: &'static str,
    assert: &'static str,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct LifecycleLine {
    session: u64,
    screen: &'static str,
    title: String,
    title_tone: &'static str,
    detail: String,
    detail_tone: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CatalogLine {
    session: u64,
    strategy_id: String,
    title: String,
    badge: Styled,
    spin_interval_us: u64,
    instruments: Vec<InstrumentLine>,
    feature_names: Vec<String>,
}

#[derive(Serialize)]
struct InstrumentLine {
    id: u16,
    display: String,
    base: String,
    quote: String,
    tick: Option<String>,
}

#[derive(Serialize)]
pub(super) struct ByeLine {
    pub(super) reason: &'static str,
}

fn connection_word(connection: ConnectionState) -> &'static str {
    match connection {
        ConnectionState::Connecting => "connecting",
        ConnectionState::Live => "live",
        ConnectionState::Stale => "stale",
    }
}

fn screen_word(screen: &Screen) -> &'static str {
    match screen {
        Screen::Starting => "starting",
        Screen::Live => "live",
        Screen::Draining(_) => "draining",
        Screen::Stopped(_) => "stopped",
    }
}

fn run_word(state: RunState) -> &'static str {
    match state {
        RunState::Running => "running",
        RunState::Idle => "idle",
    }
}

/// The hello line: static vocabulary the page needs before any engine reports in.
pub fn hello_line(polysim: &str, tick: Duration) -> String {
    let mut groupings = Groupings {
        ticks: Vec::new(),
        bps: Vec::new(),
    };
    for (grouping, label) in TICK_GROUPINGS.iter().chain(BPS_GROUPINGS.iter()) {
        match *grouping {
            DomGrouping::Ticks { per_bucket } => {
                groupings.ticks.push(TickGrouping { per_bucket, label });
            }
            DomGrouping::Bps {
                numerator,
                denominator,
            } => groupings.bps.push(BpsGrouping {
                numerator,
                denominator,
                label,
            }),
        }
    }
    let hello = Hello {
        protocol: PROTOCOL,
        bridge: BRIDGE,
        polysim: polysim.to_owned(),
        tick_ms: u64::try_from(tick.as_millis()).unwrap_or(u64::MAX),
        groupings,
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
                    format::render(|out| {
                        format::write_venue_price(out, tick, format::price_decimals(tick))
                    })
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
