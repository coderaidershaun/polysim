//! The monitor's tab strip and the active channel's rows, newest first, each row in fixed columns
//! taken from its own rect so the numbers never shift while the rows scroll.

use eframe::egui;

use super::bands::{RowList, paint_left_ellipsized, paint_row_list};
use crate::workstation::browser::controls;
use crate::workstation::browser::dom::overlay::{Direction, triangle};
use crate::workstation::browser::theme::{self, DARK, METRICS};
use crate::workstation::monitor::model::{FillRow, OrderRow, SystemRow, TradeRow};
use crate::workstation::monitor::view::{Channel, MonitorFrame, MonitorUiState};
use crate::workstation::text::{Cell, ChannelRow, TagColumn, record_row};

/// A list scrolled less than this from the top still shows the newest row, which absorbs sub-pixel
/// jitter.
const FOLLOW_EPS: f32 = 1.0;

/// One equal cell per channel, so the strip never wraps or overflows.
pub(super) fn paint_tabs(ui: &mut egui::Ui, rect: egui::Rect, state: &mut MonitorUiState) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, DARK.panel_raised);
    let cell_w = rect.width() / Channel::ALL.len() as f32;

    for (index, channel) in Channel::ALL.iter().enumerate() {
        let active = *channel == state.active_tab;
        let cell = egui::Rect::from_min_size(
            egui::pos2(rect.left() + index as f32 * cell_w, rect.top()),
            egui::vec2(cell_w, rect.height()),
        );
        let response = ui.interact(
            cell,
            egui::Id::new(("monitor-tab", index)),
            egui::Sense::click(),
        );

        if active {
            controls::fill_selected(&painter, cell);
        } else if response.hovered() {
            painter.rect_filled(cell, 0.0, DARK.panel);
        }
        painter.text(
            cell.center(),
            egui::Align2::CENTER_CENTER,
            channel.label(),
            egui::FontId::proportional(METRICS.monitor_tab_font),
            if active { DARK.text_primary } else { DARK.text_secondary },
        );

        if response.clicked() {
            state.active_tab = *channel;
        }
    }

    let dividers = (1..Channel::ALL.len()).map(|boundary| rect.left() + boundary as f32 * cell_w);
    theme::rules_v(&painter, dividers, rect.y_range());
}

/// The active channel's rows, newest first. Switching tabs clears nothing.
pub(super) fn paint_channel(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    frame: &MonitorFrame<'_>,
    state: &mut MonitorUiState,
) {
    match state.active_tab {
        Channel::System => render_channel::<SystemRow>(ui, rect, frame, state),
        Channel::Orders => render_channel::<OrderRow>(ui, rect, frame, state),
        Channel::PubTrades => render_channel::<TradeRow>(ui, rect, frame, state),
        Channel::TradeFills => render_channel::<FillRow>(ui, rect, frame, state),
    }
}

/// A row's cells drive both the paint and the hover record, so the two cannot differ.
fn render_channel<R: ChannelRow>(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    frame: &MonitorFrame<'_>,
    state: &mut MonitorUiState,
) {
    let appended = R::appended(frame);
    let slot = state.channel(R::CHANNEL);
    let forced = slot.take_pending_offset();
    let unseen_count = if slot.is_following { 0 } else { appended.saturating_sub(slot.seen) };
    if slot.is_following {
        slot.seen = appended;
    }

    let mut cells: Vec<Cell> = Vec::new();
    let offset = paint_row_list(
        ui,
        rect,
        RowList {
            salt: ("monitor-channel", R::CHANNEL.index()),
            row_count: R::count(frame),
            empty_note: R::EMPTY_NOTE,
            forced_offset: forced,
            row_spacing: 0.0,
        },
        |painter, row_rect, index| {
            let Some(row) = R::at(frame, index) else { return };
            cells.clear();
            row.cells(frame, &mut cells);
            paint_cells(painter, row_rect, &cells);
        },
        |index| R::at(frame, index).map_or_else(String::new, |row| record_row(&row, frame)),
    );
    state.channel(R::CHANNEL).is_following = offset <= FOLLOW_EPS;

    if unseen_count > 0 {
        paint_unseen_badge(ui, rect, R::CHANNEL, unseen_count, state);
    }
}

/// A pill with the count of rows not yet seen. Clicking it jumps to the newest row. New rows never
/// pull a scrolled-away list there on their own.
fn paint_unseen_badge(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    channel: Channel,
    count: u64,
    state: &mut MonitorUiState,
) {
    let text = format!("{count} new");
    let font = egui::FontId::proportional(METRICS.monitor_badge_font);
    let galley = ui.painter().layout_no_wrap(text, font, DARK.text_primary);
    let galley_size = galley.size();
    let marker_size = METRICS.monitor_badge_font;
    let content = egui::vec2(marker_size + METRICS.space_1 + galley_size.x, galley_size.y);
    let size = content + egui::vec2(METRICS.space_2 * 2.0, METRICS.space_1);
    let badge = egui::Rect::from_min_size(
        egui::pos2(rect.center().x - size.x / 2.0, rect.top() + METRICS.space_1),
        size,
    );

    let response = ui.interact(
        badge,
        egui::Id::new(("monitor-unseen", channel.index())),
        egui::Sense::click(),
    );
    let painter = ui.painter_at(badge);
    let fill = if response.hovered() { DARK.selected } else { DARK.selected.gamma_multiply(0.85) };
    painter.rect_filled(badge, METRICS.corner_radius, fill);

    let marker_center = egui::pos2(
        badge.left() + METRICS.space_2 + marker_size / 2.0,
        badge.center().y,
    );
    let marker = triangle(marker_center, marker_size, Direction::Up, DARK.text_primary);
    painter.add(marker);
    painter.galley(
        egui::pos2(
            marker_center.x + marker_size / 2.0 + METRICS.space_1,
            badge.center().y - galley_size.y / 2.0,
        ),
        galley,
        DARK.text_primary,
    );

    if response.on_hover_text("jump to newest").clicked() {
        state.channel(channel).resume_follow();
    }
}

fn mono_font() -> egui::FontId {
    egui::FontId::monospace(METRICS.monitor_channel_font)
}

fn text_font() -> egui::FontId {
    egui::FontId::proportional(METRICS.monitor_channel_font)
}

fn paint_cells(painter: &egui::Painter, rect: egui::Rect, cells: &[Cell]) {
    let cols = ChannelColumns::new(painter, rect);
    let mut description_offset = 0.0;
    for cell in cells {
        match cell {
            Cell::Time(text) => cols.paint_time(painter, text),
            Cell::Tag(tag) => {
                cols.paint_tag(painter, tag);
                description_offset = METRICS.monitor_intent_tag_w;
            }
            Cell::Description { text, tone } => paint_left_ellipsized(
                painter,
                rect.with_min_x(cols.tag_x + description_offset)
                    .with_max_x(cols.qty_x),
                text,
                text_font(),
                DARK.tone(*tone),
            ),
            Cell::Price(text) => cols.paint_number(painter, cols.price_x, text),
            Cell::Qty(text) => cols.paint_number(painter, cols.qty_x, text),
            Cell::Detail(_) => {}
        }
    }
}

/// The column anchors and the text baseline, taken from the row's rect so the columns never move.
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
            DARK.tone(tag.tone),
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

    fn paint_number(&self, painter: &egui::Painter, x: f32, text: &str) {
        painter.text(
            egui::pos2(x, self.y),
            egui::Align2::RIGHT_CENTER,
            text,
            mono_font(),
            DARK.text_primary,
        );
    }
}
