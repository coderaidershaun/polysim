//! Hover crosshair spanning both stacked plots. Clipped to stack to cross both charts.
//! Painted last so it's over both plots, not inside either.

use eframe::egui;

use crate::ids::{InstrumentId, Price};

use super::chart::PlotGeometry;
use super::chart_model::{ChartModel, bucket_open_ts};
use super::chart_view;
use super::dom_overlay;
use super::format;
use super::position_chart::PositionPlot;
use super::position_chart_model::PositionModel;
use super::position_chart_view::RiskSeries;
use super::theme::{self, DARK, METRICS};

/// Air around a pill's text.
const PILL_PAD: egui::Vec2 = egui::vec2(6.0, 3.0);

/// How far a pill sits from the hairline, so the line it explains stays visible beside it.
const PILL_OFFSET: f32 = 8.0;

/// Everything the crosshair needs to resolve and label one bucket. The two geometries carry the pixel
/// transforms; the two models carry the values, which a transform does not.
pub struct CrosshairFrame<'a> {
    pub chart: &'a ChartModel,
    pub positions: &'a PositionModel,
    pub instrument: InstrumentId,
    pub tick: Price,
    pub series: RiskSeries,
    pub mid: Option<PlotGeometry>,
    pub lower: Option<PositionPlot>,
    /// The union of both chart components — the crosshair's clip and the bound pills are kept inside.
    pub stack: egui::Rect,
    pub pointer: Option<egui::Pos2>,
}

pub fn paint(ui: &egui::Ui, frame: &CrosshairFrame<'_>) {
    let Some(pointer) = frame.pointer else { return };
    let Some((rect, domain)) = frame
        .mid
        .map(|mid| (mid.rect, mid.domain))
        .or_else(|| frame.lower.map(|lower| (lower.rect, lower.domain)))
    else {
        return;
    };

    let fraction = (pointer.x - rect.left()) / rect.width().max(f32::EPSILON);
    let bucket = chart_view::bucket_at_fraction(fraction, domain);
    let painter = ui.painter().with_clip_rect(frame.stack);
    paint_hairline(&painter, frame, bucket);

    let mut label = String::new();
    if let Some(mid) = frame.mid
        && let Some(close) = mid_close(frame, bucket)
    {
        format::write_venue_mid(&mut label, close, frame.tick);
        paint_pill(
            &painter,
            frame,
            &label,
            egui::pos2(mid.x(bucket), mid.y(close)),
        );
    }
    if let Some(lower) = frame.lower
        && let Some(value) = position_value(frame, bucket)
    {
        format::write_quote_amount(&mut label, value, RiskSeries::READOUT_DECIMALS);
        paint_pill(
            &painter,
            frame,
            &label,
            egui::pos2(lower.x(bucket), lower.y(value)),
        );
    }
    paint_time(&painter, frame, bucket, &mut label);
}

fn paint_hairline(painter: &egui::Painter, frame: &CrosshairFrame<'_>, bucket: u64) {
    let mut span: Option<egui::Rangef> = None;
    let mut x = 0.0;
    if let Some(mid) = frame.mid {
        span = Some(mid.rect.y_range());
        x = mid.x(bucket);
    }
    if let Some(lower) = frame.lower {
        let range = lower.rect.y_range();
        span = Some(span.map_or(range, |span| {
            egui::Rangef::new(span.min.min(range.min), span.max.max(range.max))
        }));
        if frame.mid.is_none() {
            x = lower.x(bucket);
        }
    }
    let Some(span) = span else { return };
    painter.vline(
        theme::crisp(painter, x),
        span,
        theme::hairline(painter, DARK.grid),
    );
}

fn paint_pill(painter: &egui::Painter, frame: &CrosshairFrame<'_>, text: &str, anchor: egui::Pos2) {
    let font = egui::FontId::monospace(METRICS.chart_axis_font);
    let galley = painter.layout_no_wrap(text.to_owned(), font, DARK.text_primary);
    let pill = dom_overlay::clamp_rect(
        frame.stack,
        egui::Rect::from_min_size(
            egui::pos2(anchor.x + PILL_OFFSET, anchor.y - galley.size().y / 2.0),
            galley.size() + PILL_PAD * 2.0,
        ),
    );
    dom_overlay::fill_pill(painter, pill, DARK.border);
    painter.galley(pill.min + PILL_PAD, galley, DARK.text_primary);
}

fn paint_time(
    painter: &egui::Painter,
    frame: &CrosshairFrame<'_>,
    bucket: u64,
    label: &mut String,
) {
    let Some(at) = bucket_open_ts(bucket, frame.chart.spin_interval()) else {
        return;
    };
    format::write_time_of_day(label, at);
    painter.text(
        egui::pos2(frame.stack.center().x, frame.stack.bottom() - PILL_PAD.y),
        egui::Align2::CENTER_BOTTOM,
        &*label,
        egui::FontId::monospace(METRICS.chart_axis_font),
        DARK.text_secondary,
    );
}

fn mid_close(frame: &CrosshairFrame<'_>, bucket: u64) -> Option<i64> {
    frame
        .chart
        .buckets(frame.instrument)
        .find(|candidate| candidate.index == bucket)
        .map(|candidate| candidate.close_half_ticks)
}

fn position_value(frame: &CrosshairFrame<'_>, bucket: u64) -> Option<i64> {
    frame
        .positions
        .buckets(frame.instrument)
        .find(|candidate| candidate.index == bucket)
        .map(|candidate| frame.series.value(candidate))
}
