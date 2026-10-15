//! Latency grid between the toolbar and the mid chart: 10-minute mean microseconds per flow, plus
//! the 1-minute half-life EWMA of unprocessed input-queue slots. Every cell is a number to scan, so
//! this paints and senses nothing.

use eframe::egui;

use crate::msg::ui::UiLatencySummary;

use super::text::{Flow, LATENCY_COLUMNS, LATENCY_ROWS, LATENCY_TITLE, latency_cells};
use super::theme::{self, DARK, METRICS};

pub struct LatencyFrame {
    pub summary: Option<UiLatencySummary>,
}

pub fn split(body: egui::Rect) -> (egui::Rect, egui::Rect) {
    let split = body.top() + (body.height() * METRICS.latency_fraction).round();
    (
        egui::Rect::from_min_max(body.min, egui::pos2(body.right(), split)),
        egui::Rect::from_min_max(egui::pos2(body.left(), split), body.max),
    )
}

pub fn paint(ui: &mut egui::Ui, rect: egui::Rect, frame: &LatencyFrame) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, DARK.panel);

    let header = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.right(), rect.top() + METRICS.dom_header_height),
    );
    paint_header(&painter, header);

    let body = egui::Rect::from_min_max(egui::pos2(rect.left(), header.bottom()), rect.max);
    paint_rows(&painter, body, frame.summary.as_ref());

    // The last column is a queue depth, not a latency: the divider runs the full panel height so a
    // reader stops comparing it against the microsecond columns beside it.
    painter.vline(
        theme::crisp(
            &painter,
            value_column(rect, LATENCY_COLUMNS.len() - 1).left(),
        ),
        rect.y_range(),
        theme::hairline(&painter, DARK.border),
    );

    painter.hline(
        rect.x_range(),
        theme::crisp_bottom_edge(&painter, rect.bottom()),
        theme::hairline(&painter, DARK.border),
    );
}

fn paint_header(painter: &egui::Painter, rect: egui::Rect) {
    painter.rect_filled(rect, 0.0, DARK.panel_raised);
    let font = egui::FontId::proportional(METRICS.latency_header_font);
    painter.text(
        egui::pos2(
            rect.left() + METRICS.space_2,
            theme::crisp(painter, rect.center().y),
        ),
        egui::Align2::LEFT_CENTER,
        LATENCY_TITLE,
        font.clone(),
        DARK.text_secondary,
    );
    for (index, label) in LATENCY_COLUMNS.iter().enumerate() {
        let cell = value_column(rect, index);
        painter.text(
            egui::pos2(
                cell.right() - METRICS.space_2,
                theme::crisp(painter, cell.center().y),
            ),
            egui::Align2::RIGHT_CENTER,
            *label,
            font.clone(),
            DARK.text_secondary,
        );
    }
    painter.hline(
        rect.x_range(),
        theme::crisp(painter, rect.bottom()),
        theme::hairline(painter, DARK.border),
    );
}

fn paint_rows(painter: &egui::Painter, rect: egui::Rect, summary: Option<&UiLatencySummary>) {
    let height = rect.height() / LATENCY_ROWS.len() as f32;
    let separator = theme::hairline(painter, DARK.grid);
    for (index, (label, flow)) in LATENCY_ROWS.iter().enumerate() {
        let top = rect.top() + index as f32 * height;
        let row = egui::Rect::from_min_max(
            egui::pos2(rect.left(), top),
            egui::pos2(rect.right(), top + height),
        );
        if index > 0 {
            painter.hline(rect.x_range(), theme::crisp(painter, row.top()), separator);
        }
        painter.text(
            egui::pos2(
                row.left() + METRICS.space_2,
                theme::crisp(painter, row.center().y),
            ),
            egui::Align2::LEFT_CENTER,
            *label,
            egui::FontId::proportional(METRICS.latency_row_font),
            DARK.text_secondary,
        );
        paint_values(painter, row, *flow, summary);
    }
}

fn paint_values(
    painter: &egui::Painter,
    row: egui::Rect,
    flow: Flow,
    summary: Option<&UiLatencySummary>,
) {
    for (index, (text, tone)) in latency_cells(flow, summary).iter().enumerate() {
        paint_value(painter, value_column(row, index), text, DARK.tone(*tone));
    }
}

fn paint_value(painter: &egui::Painter, cell: egui::Rect, text: &str, color: egui::Color32) {
    painter.text(
        egui::pos2(
            cell.right() - METRICS.space_2,
            theme::crisp(painter, cell.center().y),
        ),
        egui::Align2::RIGHT_CENTER,
        text,
        egui::FontId::monospace(METRICS.latency_value_font),
        color,
    );
}

fn value_column(row: egui::Rect, index: usize) -> egui::Rect {
    let left = row.left() + METRICS.latency_label_col_w;
    let width = ((row.right() - left) / LATENCY_COLUMNS.len() as f32).max(0.0);
    egui::Rect::from_min_max(
        egui::pos2(left + index as f32 * width, row.top()),
        egui::pos2(left + (index + 1) as f32 * width, row.bottom()),
    )
}
