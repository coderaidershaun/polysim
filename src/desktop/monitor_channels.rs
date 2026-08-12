//! Tab strip + channel rows, newest-first. Independent scroll; scrolled away -> resume badge. Venue state verbatim.

use eframe::egui;

use super::monitor::{Channel, MonitorFrame, MonitorUiState, RowList, paint_row_list};
use super::monitor_model::{FillRow, OrderRow, SystemRow, TradeRow};
use super::monitor_rows::{Cell, ChannelRow, paint_cells, record_row};
use super::monitor_view::unseen;
use super::theme::{self, DARK, METRICS};

/// Scroll offset threshold for "pinned to newest". Hair above zero absorbs sub-pixel jitter.
const FOLLOW_EPS: f32 = 1.0;

/// Paint the tab strip and fold clicks. No wrap or overflow: the strip is one equal cell per channel.
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
            painter.rect_filled(cell, 0.0, DARK.selected.gamma_multiply(0.28));
            // Selected tab claims the strip's bottom edge: the channel it governs sits below it.
            let accent = egui::Rect::from_min_max(
                egui::pos2(cell.left(), cell.bottom() - METRICS.accent_thickness),
                cell.max,
            );
            painter.rect_filled(accent, 0.0, DARK.selected);
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

    let stroke = theme::hairline(&painter, DARK.border);
    for boundary in 1..Channel::ALL.len() {
        let x = rect.left() + boundary as f32 * cell_w;
        painter.vline(theme::crisp(&painter, x), rect.y_range(), stroke);
    }
}

/// Paint active channel rows. Each channel keeps bounded history newest-first (tabs don't clear).
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

/// Scaffold: fold watermark/rows/badge. Row cells drive both the paint and the hover record.
fn render_channel<R: ChannelRow>(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    frame: &MonitorFrame<'_>,
    state: &mut MonitorUiState,
) {
    let appended = R::appended(frame);
    let slot = state.channel(R::CHANNEL);
    let forced = slot.take_pending_offset();
    let following = slot.following();
    let unseen_count = if following { 0 } else { unseen(appended, slot.seen()) };
    if following {
        slot.mark_seen(appended);
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
    state
        .channel(R::CHANNEL)
        .set_following(offset <= FOLLOW_EPS);

    if unseen_count > 0 {
        paint_unseen_badge(ui, rect, R::CHANNEL, unseen_count, state);
    }
}

/// Resume-follow badge: compact pill showing pending count. Click jumps to newest (no yank).
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
    painter.add(up_triangle(marker_center, marker_size));
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

/// The badge's up-marker, painted as a shape rather than set as a glyph: the bundled default fonts
/// have no face for the arrows and triangles this would otherwise be written with.
fn up_triangle(center: egui::Pos2, size: f32) -> egui::Shape {
    let half = size / 2.0;
    egui::Shape::convex_polygon(
        vec![
            egui::pos2(center.x, center.y - half),
            egui::pos2(center.x - half, center.y + half),
            egui::pos2(center.x + half, center.y + half),
        ],
        DARK.text_primary,
        egui::Stroke::NONE,
    )
}
