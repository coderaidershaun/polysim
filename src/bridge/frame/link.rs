//! The connection lines: link, lifecycle and catalog, and the last word before the bridge exits.

use serde::Serialize;

use crate::desktop::attachment::Screen;
use crate::desktop::link_model::ConnectionState;
use crate::link::RunState;

use super::Styled;

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LinkLine {
    pub peer: String,
    pub peer_index: usize,
    pub peer_count: usize,
    pub session: u64,
    pub chips: Vec<Styled>,
    #[serde(rename = "peerClickable")]
    pub is_peer_clickable: bool,
    pub button: Button,
    pub connection: &'static str,
}

#[derive(Serialize, Clone, PartialEq)]
pub(crate) struct Button {
    pub label: &'static str,
    pub assert: &'static str,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LifecycleLine {
    pub session: u64,
    pub screen: &'static str,
    pub title: String,
    pub title_tone: &'static str,
    pub detail: String,
    pub detail_tone: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogLine {
    pub session: u64,
    pub strategy_id: String,
    pub title: String,
    pub badge: Styled,
    pub spin_interval_us: u64,
    pub instruments: Vec<InstrumentLine>,
    pub feature_names: Vec<String>,
}

#[derive(Serialize)]
pub(crate) struct InstrumentLine {
    pub id: u16,
    pub display: String,
    pub base: String,
    pub quote: String,
    pub tick: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct ByeLine {
    pub reason: &'static str,
}

pub(crate) fn connection_word(connection: ConnectionState) -> &'static str {
    match connection {
        ConnectionState::Connecting => "connecting",
        ConnectionState::Live => "live",
        ConnectionState::Stale => "stale",
    }
}

pub(crate) fn screen_word(screen: &Screen) -> &'static str {
    match screen {
        Screen::Starting => "starting",
        Screen::Live => "live",
        Screen::Draining(_) => "draining",
        Screen::Stopped(_) => "stopped",
    }
}

pub(crate) fn run_word(state: RunState) -> &'static str {
    match state {
        RunState::Running => "running",
        RunState::Idle => "idle",
    }
}
