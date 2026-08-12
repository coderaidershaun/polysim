//! Per-row painters: fixed columns from rect geometry (numbers never shift). Hover records untruncated.

use eframe::egui;

use crate::hot::exec::{CloseReason, OrderState, RejectOrigin};
use crate::ids::{Price, Qty, Side};
use crate::msg::exec::Liquidity;
use crate::time::TsUs;

use super::dom_view::tick_index;
use super::format::{self, MISSING};
use super::monitor::{Channel, MonitorFrame, paint_left_ellipsized};
use super::monitor_model::{
    FillRow, OrderEvent, OrderRow, SystemEvent, SystemNote, SystemRow, TradeRow,
};
use super::theme::{self, DARK, METRICS};

/// Fill commission fraction digits: 6 (10 bps on ~$10 = ~$0.0094; <6 shows every fill as zero).
const FEE_DECIMALS: usize = 6;

/// Stand-in for a lifecycle row's absent event time: the shape of a clock reading with no digits
/// in it, so the column stays aligned and nobody mistakes it for one.
const NO_EVENT_TIME: &str = "--:--:--.---";

/// The tag column: the venue's liquidity note, when it said one, and the side word beside it. One
/// cell because they are one column — a single space joins them in the record, as the eye reads them.
pub(super) struct TagColumn {
    liquidity: Option<&'static str>,
    side: &'static str,
    color: egui::Color32,
}

impl TagColumn {
    fn side(side: &'static str, color: egui::Color32) -> Self {
        Self {
            liquidity: None,
            side,
            color,
        }
    }

    fn fill(liquidity: Option<Liquidity>, side: &'static str, color: egui::Color32) -> Self {
        Self {
            liquidity: Some(liquidity_word(liquidity)),
            side,
            color,
        }
    }
}

/// One column of a channel row. The painter and the hover record read this same list, so a column
/// cannot be drawn one way and reported another.
pub(super) enum Cell {
    Time(String),
    Tag(TagColumn),
    /// Free-text band, left-ellipsized, from `offset` right of the tag anchor to the qty column.
    Description {
        text: String,
        offset: f32,
        color: egui::Color32,
    },
    Price(String),
    Qty(String),
    /// Hover-only trailer: too wide for a row, too useful to lose.
    Detail(String),
}

impl Cell {
    fn time(at: TsUs) -> Self {
        let mut text = String::new();
        format::write_time_of_day(&mut text, at);
        Self::Time(text)
    }

    fn tick_price(price: Price, tick: Price) -> Self {
        Self::Price(price_text(price, tick))
    }

    fn quantity(qty: Qty, frame: &MonitorFrame<'_>) -> Self {
        Self::Qty(qty_text(qty, frame))
    }

    fn write_record(&self, record: &mut String) {
        match self {
            Cell::Tag(tag) => {
                if let Some(liquidity) = tag.liquidity {
                    record.push_str(liquidity);
                    record.push(' ');
                }
                record.push_str(tag.side);
            }
            Cell::Time(text)
            | Cell::Description { text, .. }
            | Cell::Price(text)
            | Cell::Qty(text)
            | Cell::Detail(text) => record.push_str(text),
        }
    }
}

fn mono_font() -> egui::FontId {
    egui::FontId::monospace(METRICS.monitor_channel_font)
}

fn text_font() -> egui::FontId {
    egui::FontId::proportional(METRICS.monitor_channel_font)
}

/// A channel's row: where its history lives, what an empty channel says, and how a row draws.
/// Read by index newest-first, as the channel scrolls, so a band paints the rows it shows rather
/// than a copy of the whole retained history.
pub(super) trait ChannelRow: Clone {
    const CHANNEL: Channel;
    const EMPTY_NOTE: &'static str;

    fn count(frame: &MonitorFrame<'_>) -> usize;

    /// The `index`-th newest row, counting from zero.
    fn at(frame: &MonitorFrame<'_>, index: usize) -> Option<Self>;

    /// Rows ever appended (monotonic basis for the unseen count).
    fn appended(frame: &MonitorFrame<'_>) -> u64;

    fn cells(&self, frame: &MonitorFrame<'_>, out: &mut Vec<Cell>);
}

pub(super) fn paint_cells(painter: &egui::Painter, rect: egui::Rect, cells: &[Cell]) {
    let cols = ChannelColumns::new(painter, rect);
    for cell in cells {
        match cell {
            Cell::Time(text) => cols.paint_time(painter, text),
            Cell::Tag(tag) => cols.paint_tag(painter, tag),
            Cell::Description {
                text,
                offset,
                color,
            } => paint_left_ellipsized(
                painter,
                cols.text_rect(rect, cols.tag_x + offset),
                text,
                text_font(),
                *color,
            ),
            Cell::Price(text) => cols.paint_price(painter, text),
            Cell::Qty(text) => cols.paint_qty(painter, text),
            Cell::Detail(_) => {}
        }
    }
}

pub(super) fn record_row<R: ChannelRow>(row: &R, frame: &MonitorFrame<'_>) -> String {
    let mut cells = Vec::new();
    row.cells(frame, &mut cells);
    let mut record = String::new();
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            record.push_str("  ");
        }
        cell.write_record(&mut record);
    }
    record
}

/// System row: `HH:MM:SS.mmm` (or marker if no event time) + colored description.
impl ChannelRow for SystemRow {
    const CHANNEL: Channel = Channel::System;
    const EMPTY_NOTE: &'static str = "no system events";

    fn count(frame: &MonitorFrame<'_>) -> usize {
        frame.model.monitor().system().count()
    }

    fn at(frame: &MonitorFrame<'_>, index: usize) -> Option<Self> {
        frame.model.monitor().system().nth(index).cloned()
    }

    fn appended(frame: &MonitorFrame<'_>) -> u64 {
        frame.model.monitor().system_appended()
    }

    fn cells(&self, frame: &MonitorFrame<'_>, out: &mut Vec<Cell>) {
        out.push(match self.at {
            Some(at) => Cell::time(at),
            None => Cell::Time(NO_EVENT_TIME.to_owned()),
        });
        let (text, color) = system_text(&self.event, frame);
        out.push(Cell::Description {
            text,
            offset: 0.0,
            color,
        });
    }
}

/// Order row: side + venue answer (state word or refusal). Refusal color by codec RejectClass (post-only cross = routine).
impl ChannelRow for OrderRow {
    const CHANNEL: Channel = Channel::Orders;
    const EMPTY_NOTE: &'static str = "no orders";

    fn count(frame: &MonitorFrame<'_>) -> usize {
        frame.model.monitor().orders().count()
    }

    fn at(frame: &MonitorFrame<'_>, index: usize) -> Option<Self> {
        frame.model.monitor().orders().nth(index).copied()
    }

    fn appended(frame: &MonitorFrame<'_>) -> u64 {
        frame.model.monitor().orders_appended()
    }

    fn cells(&self, frame: &MonitorFrame<'_>, out: &mut Vec<Cell>) {
        let (word, side_color) = side_word(self.side, DARK.sell_fill);
        let (text, color) = order_text(&self.event, frame);
        out.push(Cell::time(self.at));
        out.push(Cell::Tag(TagColumn::side(word, side_color)));
        out.push(Cell::Description {
            text,
            offset: METRICS.monitor_intent_tag_w,
            color,
        });
    }
}

/// Public trade row: time, side-colored aggressor, tick price, qty (selected instrument tape).
impl ChannelRow for TradeRow {
    const CHANNEL: Channel = Channel::PubTrades;
    const EMPTY_NOTE: &'static str = "no public trades";

    fn count(frame: &MonitorFrame<'_>) -> usize {
        frame.model.monitor().trades(frame.instrument).count()
    }

    fn at(frame: &MonitorFrame<'_>, index: usize) -> Option<Self> {
        frame
            .model
            .monitor()
            .trades(frame.instrument)
            .nth(index)
            .copied()
    }

    fn appended(frame: &MonitorFrame<'_>) -> u64 {
        frame.model.monitor().trades_appended(frame.instrument)
    }

    fn cells(&self, frame: &MonitorFrame<'_>, out: &mut Vec<Cell>) {
        let (word, color) = side_word(self.aggressor, DARK.ask);
        out.push(Cell::time(self.at));
        out.push(Cell::Tag(TagColumn::side(word, color)));
        out.push(Cell::tick_price(self.price, frame.tick));
        out.push(Cell::quantity(self.qty, frame));
    }
}

/// Venue fill, tagged with reported liquidity (buy-green/sell-orange). No tag if venue didn't say (not guessed).
impl ChannelRow for FillRow {
    const CHANNEL: Channel = Channel::TradeFills;
    const EMPTY_NOTE: &'static str = "no fills";

    fn count(frame: &MonitorFrame<'_>) -> usize {
        frame.model.monitor().fills().count()
    }

    fn at(frame: &MonitorFrame<'_>, index: usize) -> Option<Self> {
        frame.model.monitor().fills().nth(index).copied()
    }

    fn appended(frame: &MonitorFrame<'_>) -> u64 {
        frame.model.monitor().fills_appended()
    }

    fn cells(&self, frame: &MonitorFrame<'_>, out: &mut Vec<Cell>) {
        let (word, color) = side_word(self.side, DARK.sell_fill);
        let mut fee = String::new();
        format::write_quote_amount(&mut fee, self.commission, FEE_DECIMALS);
        out.push(Cell::time(self.at));
        out.push(Cell::Tag(TagColumn::fill(self.liquidity, word, color)));
        out.push(Cell::tick_price(self.price, frame.tick));
        out.push(Cell::quantity(self.qty, frame));
        out.push(Cell::Detail(format!(
            "fee {fee} a{}",
            self.commission_asset.0
        )));
    }
}

/// Fixed x anchors + baseline (time left, tag/side, price/qty right). From rect so columns don't move.
struct ChannelColumns {
    tag_x: f32,
    time_x: f32,
    price_x: f32,
    qty_x: f32,
    y: f32,
}

impl ChannelColumns {
    fn new(painter: &egui::Painter, rect: egui::Rect) -> Self {
        let pad = METRICS.space_2;
        let time_w = METRICS.monitor_time_col_w;
        let qty_w = METRICS.monitor_qty_col_w;
        Self {
            time_x: rect.left() + pad,
            tag_x: rect.left() + pad + time_w,
            price_x: rect.right() - pad - qty_w,
            qty_x: rect.right() - pad,
            y: theme::crisp(painter, rect.center().y),
        }
    }

    /// Liquidity note first when present, and the side word one tag width right of it.
    fn paint_tag(&self, painter: &egui::Painter, tag: &TagColumn) {
        let mut x = self.tag_x;
        if let Some(liquidity) = tag.liquidity {
            painter.text(
                egui::pos2(x, self.y),
                egui::Align2::LEFT_CENTER,
                liquidity,
                text_font(),
                DARK.text_secondary,
            );
            x += METRICS.monitor_fill_tag_w;
        }
        painter.text(
            egui::pos2(x, self.y),
            egui::Align2::LEFT_CENTER,
            tag.side,
            mono_font(),
            tag.color,
        );
    }

    fn paint_time(&self, painter: &egui::Painter, text: &str) {
        painter.text(
            egui::pos2(self.time_x, self.y),
            egui::Align2::LEFT_CENTER,
            text,
            mono_font(),
            DARK.text_secondary,
        );
    }

    fn paint_price(&self, painter: &egui::Painter, text: &str) {
        painter.text(
            egui::pos2(self.price_x, self.y),
            egui::Align2::RIGHT_CENTER,
            text,
            mono_font(),
            DARK.text_primary,
        );
    }

    fn paint_qty(&self, painter: &egui::Painter, text: &str) {
        painter.text(
            egui::pos2(self.qty_x, self.y),
            egui::Align2::RIGHT_CENTER,
            text,
            mono_font(),
            DARK.text_primary,
        );
    }

    /// A left-anchored rect from `left` to the row's right pad — the free-text description band.
    fn text_rect(&self, rect: egui::Rect, left: f32) -> egui::Rect {
        egui::Rect::from_min_max(
            egui::pos2(left, rect.top()),
            egui::pos2(self.qty_x, rect.bottom()),
        )
    }
}

fn system_text(event: &SystemEvent, frame: &MonitorFrame<'_>) -> (String, egui::Color32) {
    let mut name = String::new();
    match event {
        SystemEvent::Lifecycle(note) => lifecycle_text(note),
        SystemEvent::Rotation { instrument } => {
            frame.instrument_label(*instrument, &mut name);
            (format!("{name} window rotated"), DARK.text_primary)
        }
        SystemEvent::BookResynced { instrument } => {
            frame.instrument_label(*instrument, &mut name);
            (format!("{name} book resynced"), DARK.text_secondary)
        }
        SystemEvent::EventsLost { count } => (format!("{count} ui events lost"), DARK.invalid),
        SystemEvent::BooksLost { count } => (format!("{count} book snapshots lost"), DARK.invalid),
    }
}

fn lifecycle_text(note: &SystemNote) -> (String, egui::Color32) {
    match note {
        SystemNote::Starting => ("engine starting".to_owned(), DARK.text_secondary),
        SystemNote::Ready => ("engine ready".to_owned(), DARK.positive),
        SystemNote::Draining { reason } => (format!("draining: {reason}"), DARK.warning),
        SystemNote::Stopped { graceful, reason } => {
            let verb = if *graceful { "stopped" } else { "halted" };
            let color = if *graceful { DARK.text_secondary } else { DARK.invalid };
            (format!("{verb}: {reason}"), color)
        }
    }
}

fn order_text(event: &OrderEvent, frame: &MonitorFrame<'_>) -> (String, egui::Color32) {
    match *event {
        OrderEvent::Transition {
            client_id,
            state,
            price,
            qty,
            filled,
        } => {
            let text = format!(
                "{} #{:x}  {} of {} @ {}",
                state_word(state),
                client_id.0,
                qty_text(filled, frame),
                qty_text(qty, frame),
                price_text(price, frame.tick),
            );
            (text, state_color(state))
        }
        OrderEvent::Refused { origin } => refusal_text(origin),
    }
}

fn refusal_text(origin: RejectOrigin) -> (String, egui::Color32) {
    match origin {
        RejectOrigin::Local(reason) => {
            (format!("refused by gate: {}", reason.label()), DARK.warning)
        }
        RejectOrigin::Venue { class, code } => (
            format!("venue refused {code} ({})", class.as_str()),
            theme::reject_color(class),
        ),
    }
}

/// Venue answer in one word (Closed variant says why: fill/cancel = most critical distinction).
fn state_word(state: OrderState) -> &'static str {
    match state {
        OrderState::Free => "free",
        OrderState::PendingNew => "placing",
        OrderState::Live => "resting",
        OrderState::CancelInFlight => "cancelling",
        OrderState::AmendInFlight => "amending",
        OrderState::Unknown => "UNKNOWN",
        OrderState::Closed(CloseReason::Filled) => "FILLED",
        OrderState::Closed(CloseReason::Canceled) => "cancelled",
        OrderState::Closed(CloseReason::Rejected) => "rejected",
        OrderState::Closed(CloseReason::Expired) => "expired",
        OrderState::Closed(CloseReason::ReconciledGone) => "gone",
    }
}

fn state_color(state: OrderState) -> egui::Color32 {
    match state {
        OrderState::Live => DARK.text_primary,
        OrderState::Closed(CloseReason::Filled) => DARK.positive,
        // Venue truth lost or exit we didn't choose -> invalid color (find in tape).
        OrderState::Unknown
        | OrderState::Closed(CloseReason::Expired)
        | OrderState::Closed(CloseReason::ReconciledGone) => DARK.invalid,
        OrderState::Closed(CloseReason::Rejected) => DARK.warning,
        _ => DARK.text_secondary,
    }
}

fn liquidity_word(liquidity: Option<Liquidity>) -> &'static str {
    match liquidity {
        Some(Liquidity::Maker) => "mkr",
        Some(Liquidity::Taker) => "tkr",
        None => MISSING,
    }
}

fn qty_text(qty: Qty, frame: &MonitorFrame<'_>) -> String {
    let mut buf = String::new();
    format::write_qty(&mut buf, qty, frame.qty_scale, frame.qty_decimals);
    buf
}

fn price_text(price: Price, tick: Price) -> String {
    let mut buf = String::new();
    write_tick_or_missing(&mut buf, price, tick);
    buf
}

/// Write tick-index label if on-grid, else MISSING (honest not rounded).
fn write_tick_or_missing(buf: &mut String, price: Price, tick: Price) {
    match tick_index(price, tick) {
        Some(index) => format::write_tick_price(buf, index),
        None => {
            format::write_missing(buf);
        }
    }
}

/// The side word and the colour it wears. A public print takes ask red, an own fill takes the
/// orange reserved for sells we made — the words are the same, the reading is not.
fn side_word(side: Side, sell_color: egui::Color32) -> (&'static str, egui::Color32) {
    match side {
        Side::Buy => ("BUY", DARK.bid),
        Side::Sell => ("SELL", sell_color),
    }
}
