//! The latency grid between the toolbar and the mid chart: the ten-minute mean in microseconds per
//! flow, plus a one-minute half-life moving average of unprocessed input-queue slots. Every cell is a
//! number to scan, so the grid only paints and never takes input.

use eframe::egui;

use crate::msg::ui::UiLatencySummary;

use super::theme::{self, DARK, METRICS};
use crate::workstation::text::{Flow, LATENCY_COLUMNS, LATENCY_ROWS, LATENCY_TITLE, latency_cells};

pub(super) struct LatencyFrame {
    pub summary: Option<UiLatencySummary>,
}

pub(super) fn split(body: egui::Rect) -> (egui::Rect, egui::Rect) {
    body.split_top_bottom_at_y(body.top() + (body.height() * METRICS.latency_fraction).round())
}

pub(super) fn paint(ui: &mut egui::Ui, rect: egui::Rect, frame: &LatencyFrame) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, DARK.panel);

    let (header, body) = rect.split_top_bottom_at_y(rect.top() + METRICS.dom_header_height);
    paint_header(&painter, header);
    paint_rows(&painter, body, frame.summary.as_ref());

    // The last column is a queue depth, not a latency: the divider runs the full panel height so a
    // reader stops comparing it against the microsecond columns beside it.
    let queue_column = value_column(rect, LATENCY_COLUMNS.len() - 1);
    theme::rules_v(&painter, [queue_column.left()], rect.y_range());

    painter.hline(
        rect.x_range(),
        theme::crisp_bottom_edge(&painter, rect.bottom()),
        theme::hairline(&painter, DARK.border),
    );
}

fn paint_header(painter: &egui::Painter, rect: egui::Rect) {
    painter.rect_filled(rect, 0.0, DARK.panel_raised);
    let font = egui::FontId::proportional(METRICS.latency_header_font);
    paint_label(painter, rect, LATENCY_TITLE, font.clone());
    for (index, label) in LATENCY_COLUMNS.iter().enumerate() {
        let cell = value_column(rect, index);
        paint_value(painter, cell, label, font.clone(), DARK.text_secondary);
    }
    theme::rules_h(painter, [rect.bottom()], rect.x_range());
}

fn paint_rows(painter: &egui::Painter, rect: egui::Rect, summary: Option<&UiLatencySummary>) {
    let height = rect.height() / LATENCY_ROWS.len() as f32;
    let separator = theme::hairline(painter, DARK.grid);
    let label_font = egui::FontId::proportional(METRICS.latency_row_font);
    for (index, (label, flow)) in LATENCY_ROWS.iter().enumerate() {
        let top = rect.top() + index as f32 * height;
        let row = rect.with_min_y(top).with_max_y(top + height);
        if index > 0 {
            painter.hline(rect.x_range(), theme::crisp(painter, row.top()), separator);
        }
        paint_label(painter, row, label, label_font.clone());
        paint_values(painter, row, *flow, summary);
    }
}

fn paint_values(
    painter: &egui::Painter,
    row: egui::Rect,
    flow: Flow,
    summary: Option<&UiLatencySummary>,
) {
    let font = egui::FontId::monospace(METRICS.latency_value_font);
    for (index, (text, tone)) in latency_cells(flow, summary).iter().enumerate() {
        let cell = value_column(row, index);
        paint_value(painter, cell, text, font.clone(), DARK.tone(*tone));
    }
}

fn paint_label(painter: &egui::Painter, cell: egui::Rect, text: &str, font: egui::FontId) {
    let anchor = egui::pos2(cell.left() + METRICS.space_2, cell.center().y);
    let align = egui::Align2::LEFT_CENTER;
    theme::text_at(painter, anchor, align, text, font, DARK.text_secondary);
}

fn paint_value(
    painter: &egui::Painter,
    cell: egui::Rect,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
) {
    let anchor = egui::pos2(cell.right() - METRICS.space_2, cell.center().y);
    let align = egui::Align2::RIGHT_CENTER;
    theme::text_at(painter, anchor, align, text, font, color);
}

fn value_column(row: egui::Rect, index: usize) -> egui::Rect {
    let left = row.left() + METRICS.latency_label_col_w;
    let width = ((row.right() - left) / LATENCY_COLUMNS.len() as f32).max(0.0);
    row.with_min_x(left + index as f32 * width)
        .with_max_x(left + (index + 1) as f32 * width)
}
