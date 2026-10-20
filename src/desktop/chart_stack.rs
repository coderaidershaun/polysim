//! The mid chart above the risk chart, sharing one window and one crosshair. The window is computed
//! once from the mid chart and handed to both, because deriving it twice would put the same pointer
//! position on different buckets.

use eframe::egui;

use crate::ids::{InstrumentId, Price};

use super::chart::{self, ChartFrame};
use super::chart_model::ChartModel;
use super::chart_view::{self, ChartMode};
use super::controls;
use super::crosshair::{self, CrosshairFrame};
use super::format;
use super::position_chart::{self, PositionFrame};
use super::position_chart_model::PositionModel;
use super::position_chart_view::RiskSeries;
use super::text;
use super::theme::{self, DARK, METRICS};

/// The risk selector's segments, in strip order. Exposure leads because it is the default series.
const RISK_SERIES: [(RiskSeries, &str); 2] =
    [(RiskSeries::Exposure, "EXPOSURE"), (RiskSeries::Pnl, "PNL")];

/// The mid chart's share of the body. The risk chart is a companion to the price, not its equal.
const MID_FRACTION: f32 = 0.7;

/// The selector's width inside the sub-header, leaving the rest of the strip to the readout.
const SELECTOR_WIDTH: f32 = 168.0;

pub struct ChartStackFrame<'a> {
    pub chart: &'a ChartModel,
    pub positions: &'a PositionModel,
    pub instrument: InstrumentId,
    pub tick: Price,
    pub mode: ChartMode,
    pub series: RiskSeries,
}

pub fn paint(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    frame: &ChartStackFrame<'_>,
) -> Option<RiskSeries> {
    let split = (rect.top() + rect.height() * MID_FRACTION).round();
    let (mid_rect, below_mid) = rect.split_top_bottom_at_y(split);
    let (header_rect, lower_rect) =
        below_mid.split_top_bottom_at_y(split + METRICS.dom_header_height);

    let domain = chart_view::domain(frame.chart, frame.instrument);

    let mid = chart::paint(
        ui,
        mid_rect,
        &ChartFrame {
            chart: frame.chart,
            instrument: frame.instrument,
            mode: frame.mode,
            tick: frame.tick,
            domain,
        },
    );

    let picked = paint_sub_header(ui, header_rect, frame);

    let lower = position_chart::paint(
        ui,
        lower_rect,
        &PositionFrame {
            positions: frame.positions,
            instrument: frame.instrument,
            series: frame.series,
            domain,
        },
    );

    paint_sub_header_seams(ui, rect, header_rect);

    crosshair::paint(
        ui,
        &CrosshairFrame {
            chart: frame.chart,
            positions: frame.positions,
            instrument: frame.instrument,
            tick: frame.tick,
            series: frame.series,
            mid: mid.geometry,
            lower: lower.geometry,
            stack: rect,
            pointer: mid.response.hover_pos().or(lower.response.hover_pos()),
        },
    );
    picked
}

/// The strip's own two edges, which are also the plots' only interior seams — the panel's OUTER
/// edges belong to the shell's frame and dividers, so neither chart strokes a rect of its own.
/// Clipped to the whole stack rather than the strip: `crisp` can snap a boundary line onto the
/// pixel row just outside it, and a clip at the strip would then swallow the line whole.
fn paint_sub_header_seams(ui: &egui::Ui, stack: egui::Rect, header: egui::Rect) {
    let painter = ui.painter_at(stack);
    theme::rules_h(&painter, [header.top(), header.bottom()], header.x_range());
}

fn paint_sub_header(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    frame: &ChartStackFrame<'_>,
) -> Option<RiskSeries> {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, DARK.panel);

    let selector = egui::Rect::from_min_size(
        egui::pos2(rect.left() + METRICS.space_2, rect.top() + METRICS.space_1),
        egui::vec2(SELECTOR_WIDTH, rect.height() - 2.0 * METRICS.space_1),
    );
    let picked =
        controls::paint_segmented_toggle(ui, selector, "risk-series", &RISK_SERIES, frame.series);
    paint_latest_value(&painter, rect, frame);
    picked
}

fn paint_latest_value(painter: &egui::Painter, rect: egui::Rect, frame: &ChartStackFrame<'_>) {
    let Some(latest) = frame.positions.latest(frame.instrument) else { return };
    let value = frame.series.value(&latest);
    let mut readout = String::new();
    format::write_quote_amount(&mut readout, value, RiskSeries::READOUT_DECIMALS);
    painter.text(
        egui::pos2(rect.right() - METRICS.space_2, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        &readout,
        egui::FontId::monospace(METRICS.chart_axis_font),
        DARK.tone(text::sign_tone(value)),
    );
}
