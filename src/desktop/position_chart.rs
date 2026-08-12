//! The risk chart: lower plot of the left stack, exposure or PnL as a line over the SAME bucket
//! window the mid chart drew, zero baseline always in view. Line only — a candle needs four prices
//! per interval and a position has one value per spin. The window is a caller's parameter, never
//! derived here; [`super::position_chart_view`] carries why.

use eframe::egui;

use crate::ids::InstrumentId;

use super::chart::{self, AxisLabels, Plot, paint_note};
use super::chart_view::ChartDomain;
use super::position_chart_model::{PositionBucket, PositionModel};
use super::position_chart_view::{self, QuoteBounds, RiskSeries};
use super::theme::{self, DARK, METRICS};

/// Paint input; domain = None when mid chart has no window (this chart's empty state).
pub struct PositionFrame<'a> {
    pub positions: &'a PositionModel,
    pub instrument: InstrumentId,
    pub series: RiskSeries,
    pub domain: Option<ChartDomain>,
}

/// Pixel transform on the quote axis; the unit newtype keeps it distinct from the mid chart's plot.
pub type PositionPlot = Plot<QuoteBounds>;

impl PositionPlot {
    fn point(&self, bucket: &PositionBucket, series: RiskSeries) -> egui::Pos2 {
        egui::pos2(self.x(bucket.index), self.y(series.value(bucket)))
    }
}

/// Paint result; geometry = None when no series plotted (crosshair can't read missing value).
#[must_use]
pub struct PositionPaint {
    pub response: egui::Response,
    pub geometry: Option<PositionPlot>,
}

pub fn paint(ui: &mut egui::Ui, rect: egui::Rect, frame: &PositionFrame<'_>) -> PositionPaint {
    let response = ui.interact(
        rect,
        egui::Id::new("polysim-position-chart"),
        egui::Sense::hover(),
    );
    let painter = ui.painter().with_clip_rect(rect);

    painter.rect_filled(rect, 0.0, DARK.panel);
    // Same inset as mid chart via shared fn -> equal x extents -> aligned stacked axes by construction.
    let geometry = chart::plot_rect(rect).and_then(|plot| paint_plot(&painter, plot, frame));
    PositionPaint { response, geometry }
}

fn paint_plot(
    painter: &egui::Painter,
    rect: egui::Rect,
    frame: &PositionFrame<'_>,
) -> Option<PositionPlot> {
    let Some(domain) = frame.domain else {
        paint_note(painter, rect, "no chart window");
        return None;
    };
    let Some(bounds) =
        position_chart_view::bounds(frame.positions, frame.instrument, frame.series, domain)
    else {
        paint_note(painter, rect, "no position samples yet");
        return None;
    };
    let plot = PositionPlot {
        rect,
        domain,
        bounds,
    };
    paint_zero_line(painter, &plot);
    paint_line(painter, &plot, frame);
    chart::paint_axis_labels(
        painter,
        plot.rect,
        plot.bounds.as_chart_bounds(),
        AxisLabels::QuoteAmount,
    );
    Some(plot)
}

/// Baseline at zero (drawn under series, always present since bounds include zero).
fn paint_zero_line(painter: &egui::Painter, plot: &PositionPlot) {
    painter.hline(
        plot.rect.x_range(),
        theme::crisp(painter, plot.y(0)),
        theme::hairline(painter, DARK.grid),
    );
}

fn paint_line(painter: &egui::Painter, plot: &PositionPlot, frame: &PositionFrame<'_>) {
    let stroke = egui::Stroke::new(METRICS.chart_line_width, DARK.selected);
    let mut run: Vec<egui::Pos2> = Vec::new();
    let mut previous: Option<u64> = None;
    for bucket in
        position_chart_view::visible_buckets(frame.positions, frame.instrument, plot.domain)
    {
        // Index gap = real hole (engine held no mark); line breaks. No book-continuity input, so gap alone decides.
        if previous.is_some_and(|previous| bucket.index != previous + 1) {
            chart::flush_run(painter, &mut run, stroke);
        }
        previous = Some(bucket.index);
        run.push(plot.point(bucket, frame.series));
    }
    chart::flush_run(painter, &mut run, stroke);
}
