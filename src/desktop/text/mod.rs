//! The words and colour roles the workstation shows, toolkit-free, so the egui painters and the
//! bridge read one source.

mod chart;
mod dom;
mod latency;
mod link;
mod monitor;
mod rows;
mod screen;
mod tone;

pub(crate) use chart::{AxisLabels, axis_labels, mid_note, risk_note, sign_tone};
pub(crate) use dom::{
    BPS_GROUPINGS, Ring, TICK_GROUPINGS, grouping_label, grouping_table, mid_tone, offscreen_of,
    order_tone, ring, row_price, status_pill,
};
pub(crate) use latency::{LATENCY_COLUMNS, LATENCY_ROWS, LATENCY_TITLE, latency_cells};
pub(crate) use link::{button_label, is_peer_clickable, link_chips};
pub(crate) use monitor::{
    BALANCE_DECIMALS, Lane, VALUE_DECIMALS, account_captions, account_notes, asset_label,
    cell_tone, delta_hover, feature_hover, feature_name, reject_display, summary_caption,
    write_amount, write_delta, write_feature_cell,
};
pub(crate) use rows::{Cell, ChannelRow, NO_EVENT_TIME, record_row};
pub(crate) use screen::{execution_badge, screen_text};
pub(crate) use tone::Tone;

// Only the painters name these types; the bridge reads them through the functions above.
#[cfg(feature = "ui")]
pub(crate) use latency::Flow;
#[cfg(feature = "ui")]
pub(crate) use rows::TagColumn;
#[cfg(feature = "ui")]
pub(crate) use screen::ScreenText;
