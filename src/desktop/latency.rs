//! Latency grid between the toolbar and the mid chart: 10-minute mean microseconds per flow, plus
//! the 1-minute half-life EWMA of unprocessed input-queue slots. Every cell is a number to scan, so
//! this paints and senses nothing.

use eframe::egui;

use crate::msg::ui::UiLatencySummary;

use super::format;
use super::theme::{self, DARK, METRICS};

const MICROS_COLUMNS: usize = 6;

/// Column headers in paint order, the last one over the slot count rather than a latency.
const VALUE_HEADERS: [&str; MICROS_COLUMNS + 1] = [
    "EXCH>RECV",
    "RECV>QUEUE",
    "QUEUE WAIT",
    "PROCESS",
    "END-TO-END",
    "RND TRIP",
    "SLOTS",
];

const TITLE: &str = "LATENCY  us";

const ROWS: [(&str, Flow); 3] = [
    ("MARKET DATA", Flow::MarketData),
    ("ORDER TRIP", Flow::Execution),
    ("HOT PATH", Flow::HotPath),
];

pub struct LatencyFrame {
    pub summary: Option<UiLatencySummary>,
}

#[derive(Debug, Clone, Copy)]
enum Flow {
    MarketData,
    Execution,
    HotPath,
}

impl Flow {
    fn means(self, summary: &UiLatencySummary) -> [Option<f64>; MICROS_COLUMNS] {
        let row = match self {
            Flow::MarketData => &summary.market_data,
            Flow::Execution => &summary.execution,
            Flow::HotPath => &summary.hot_path,
        };
        [
            row.exchange_to_received.mean_us(),
            row.received_to_queued.mean_us(),
            row.queue_wait.mean_us(),
            row.processing.mean_us(),
            row.end_to_end.mean_us(),
            row.order_round_trip.mean_us(),
        ]
    }

    /// Backlog is one figure across every input queue, so it reads on the hot path's row alone.
    fn slots(self, summary: &UiLatencySummary) -> Option<f64> {
        match self {
            Flow::HotPath => summary.backlog_ema,
            _ => None,
        }
    }
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
        theme::crisp(&painter, value_column(rect, MICROS_COLUMNS).left()),
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
        TITLE,
        font.clone(),
        DARK.text_secondary,
    );
    for (index, label) in VALUE_HEADERS.iter().enumerate() {
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
    let height = rect.height() / ROWS.len() as f32;
    let separator = theme::hairline(painter, DARK.grid);
    let mut scratch = String::new();
    for (index, (label, flow)) in ROWS.iter().enumerate() {
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
        paint_values(painter, row, *flow, summary, &mut scratch);
    }
}

fn paint_values(
    painter: &egui::Painter,
    row: egui::Rect,
    flow: Flow,
    summary: Option<&UiLatencySummary>,
    scratch: &mut String,
) {
    let means = summary.map_or([None; MICROS_COLUMNS], |summary| flow.means(summary));
    for (index, mean) in means.iter().enumerate() {
        format::write_opt_latency_micros(scratch, *mean);
        paint_value(
            painter,
            value_column(row, index),
            scratch,
            value_color(*mean),
        );
    }

    let slots = summary.and_then(|summary| flow.slots(summary));
    format::write_opt_slots(scratch, slots);
    paint_value(
        painter,
        value_column(row, MICROS_COLUMNS),
        scratch,
        value_color(slots),
    );
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

fn value_color(value: Option<f64>) -> egui::Color32 {
    match value {
        Some(_) => DARK.text_primary,
        None => DARK.text_secondary,
    }
}

fn value_column(row: egui::Rect, index: usize) -> egui::Rect {
    let left = row.left() + METRICS.latency_label_col_w;
    let width = ((row.right() - left) / VALUE_HEADERS.len() as f32).max(0.0);
    egui::Rect::from_min_max(
        egui::pos2(left + index as f32 * width, row.top()),
        egui::pos2(left + (index + 1) as f32 * width, row.bottom()),
    )
}
