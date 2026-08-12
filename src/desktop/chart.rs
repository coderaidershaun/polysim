//! Custom-painted OHLC/line chart with fills. Shares window with stacked risk chart via returned geometry.

use std::cmp::Ordering;
use std::mem;

use eframe::egui;

use crate::ids::{InstrumentId, Price, Side};

use super::chart_model::{ChartBucket, ChartModel};
use super::chart_view::{self, ChartBounds, ChartDomain, ChartMode};
use super::format;
use super::theme::{self, DARK, METRICS};

const AXIS_LABEL_PITCH: f32 = 2.0;
const MIN_LEGIBLE_PITCH: f32 = 1.2;

pub struct ChartFrame<'a> {
    pub chart: &'a ChartModel,
    pub instrument: InstrumentId,
    pub mode: ChartMode,
    pub tick: Price,
    pub domain: Option<ChartDomain>,
}

/// Pixel transform for one plot of the stack. `B` names the value axis's unit, so the mid plot and
/// the risk plot below it cannot be handed to each other's painter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plot<B> {
    pub rect: egui::Rect,
    pub domain: ChartDomain,
    pub bounds: B,
}

impl<B: Copy + Into<ChartBounds>> Plot<B> {
    pub(crate) fn x(&self, index: u64) -> f32 {
        let fraction = chart_view::x_fraction(index, self.domain);
        egui::lerp(self.rect.left()..=self.rect.right(), fraction)
    }

    /// Screen y for a value on this plot's own axis (range inverted: screen grows downward).
    pub(crate) fn y(&self, value: i64) -> f32 {
        let fraction = chart_view::y_fraction(value, self.bounds.into());
        egui::lerp(self.rect.bottom()..=self.rect.top(), fraction)
    }
}

pub type PlotGeometry = Plot<ChartBounds>;

impl PlotGeometry {
    fn point(&self, index: u64, half_ticks: i64) -> egui::Pos2 {
        egui::pos2(self.x(index), self.y(half_ticks))
    }

    fn slot_width(&self) -> f32 {
        self.rect.width() / self.domain.width() as f32
    }
}

#[must_use]
pub struct ChartPaint {
    pub response: egui::Response,
    pub geometry: Option<PlotGeometry>,
}

pub fn paint(ui: &mut egui::Ui, rect: egui::Rect, frame: &ChartFrame<'_>) -> ChartPaint {
    let response = ui.interact(rect, egui::Id::new("polysim-chart"), egui::Sense::hover());
    let painter = ui.painter().with_clip_rect(rect);

    painter.rect_filled(rect, 0.0, DARK.panel);
    let geometry = plot_rect(rect).and_then(|plot_rect| paint_plot(&painter, plot_rect, frame));
    ChartPaint { response, geometry }
}

pub(crate) fn plot_rect(rect: egui::Rect) -> Option<egui::Rect> {
    let pad = egui::vec2(METRICS.chart_pad, METRICS.chart_pad);
    let gutter = egui::vec2(METRICS.chart_axis_gutter, 0.0);
    let plot = egui::Rect::from_min_max(rect.min + pad, rect.max - pad - gutter);
    plot.is_positive().then_some(plot)
}

fn paint_plot(
    painter: &egui::Painter,
    rect: egui::Rect,
    frame: &ChartFrame<'_>,
) -> Option<PlotGeometry> {
    if frame.tick.0 <= 0 {
        paint_note(painter, rect, "no tick grid for this instrument");
        return None;
    }
    let Some(plot) = project(rect, frame) else {
        paint_note(painter, rect, "no mid samples yet");
        return None;
    };
    match frame.mode {
        ChartMode::Line => paint_line(painter, &plot, frame),
        ChartMode::Candles => paint_candles(painter, &plot, frame),
    }
    paint_fills(painter, &plot, frame);
    paint_axis_labels(
        painter,
        plot.rect,
        plot.bounds,
        AxisLabels::VenueMid(frame.tick),
    );
    Some(plot)
}

fn project(rect: egui::Rect, frame: &ChartFrame<'_>) -> Option<PlotGeometry> {
    let domain = frame
        .domain
        .or_else(|| chart_view::domain(frame.chart, frame.instrument))?;
    let bounds = chart_view::bounds(frame.chart, frame.instrument, domain)?;
    Some(PlotGeometry {
        rect,
        domain,
        bounds,
    })
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum AxisLabels {
    VenueMid(Price),
    QuoteAmount,
}

pub(crate) fn paint_axis_labels(
    painter: &egui::Painter,
    rect: egui::Rect,
    bounds: ChartBounds,
    labels: AxisLabels,
) {
    let font = METRICS.chart_axis_font;
    let ceiling = format::legible_tick_ceiling(
        rect.height(),
        font * AXIS_LABEL_PITCH,
        font * MIN_LEGIBLE_PITCH,
    );
    let ticks = format::axis_ticks(bounds.low, bounds.high, ceiling);
    let decimals = format::quote_axis_decimals(ticks.step());
    let font = egui::FontId::monospace(METRICS.chart_axis_font);
    let stroke = theme::hairline(painter, DARK.grid);
    let left = rect.right() + METRICS.chart_pad;
    let mut label = String::new();
    for value in ticks {
        let y = egui::lerp(
            rect.bottom()..=rect.top(),
            chart_view::y_fraction(value, bounds),
        );
        painter.hline(rect.right()..=left, theme::crisp(painter, y), stroke);
        match labels {
            AxisLabels::VenueMid(tick) => format::write_venue_mid(&mut label, value, tick),
            AxisLabels::QuoteAmount => format::write_quote_amount(&mut label, value, decimals),
        }
        painter.text(
            egui::pos2(left, y),
            egui::Align2::LEFT_CENTER,
            &label,
            font.clone(),
            DARK.text_secondary,
        );
    }
}

fn paint_line(painter: &egui::Painter, plot: &PlotGeometry, frame: &ChartFrame<'_>) {
    let stroke = egui::Stroke::new(METRICS.chart_line_width, DARK.selected);
    let mut run: Vec<egui::Pos2> = Vec::new();
    for point in chart_view::segment_points(frame.chart, frame.instrument, plot.domain) {
        if point.is_run_start {
            flush_run(painter, &mut run, stroke);
        }
        run.push(plot.point(point.bucket.index, point.bucket.close_half_ticks));
    }
    flush_run(painter, &mut run, stroke);
}

pub(crate) fn flush_run(painter: &egui::Painter, run: &mut Vec<egui::Pos2>, stroke: egui::Stroke) {
    match run.len() {
        0 => {}
        1 => {
            painter.circle_filled(run[0], METRICS.chart_line_width, stroke.color);
            run.clear();
        }
        _ => {
            painter.add(egui::Shape::line(mem::take(run), stroke));
        }
    }
}

fn paint_candles(painter: &egui::Painter, plot: &PlotGeometry, frame: &ChartFrame<'_>) {
    let visible_count =
        chart_view::segment_points(frame.chart, frame.instrument, plot.domain).count();
    let mut bodies = egui::Mesh::default();
    bodies.reserve_vertices(visible_count * 4);
    bodies.reserve_triangles(visible_count * 2);
    let mut wicks = Vec::with_capacity(visible_count);
    let mut breaks = Vec::new();
    let candle = CandleGeometry::new(painter, plot);

    for (slot, point) in
        chart_view::segment_points(frame.chart, frame.instrument, plot.domain).enumerate()
    {
        let bucket = point.bucket;
        let color = body_color(bucket);
        let center_x = plot.x(bucket.index);
        if point.is_run_start && slot > 0 {
            breaks.push(egui::Shape::vline(
                theme::crisp(painter, center_x - plot.slot_width() / 2.0),
                plot.rect.y_range(),
                theme::hairline(painter, DARK.border),
            ));
        }
        bodies.add_colored_rect(candle.body(plot, bucket, center_x), color);
        wicks.push(egui::Shape::vline(
            theme::crisp(painter, center_x),
            egui::Rangef::new(
                plot.y(bucket.high_half_ticks),
                plot.y(bucket.low_half_ticks),
            ),
            egui::Stroke::new(METRICS.chart_wick_width, color),
        ));
    }
    painter.extend(breaks);
    painter.extend(wicks);
    painter.add(egui::Shape::mesh(bodies));
}

struct CandleGeometry {
    body_width: f32,
    thinnest: f32,
}

impl CandleGeometry {
    fn new(painter: &egui::Painter, plot: &PlotGeometry) -> Self {
        let thinnest = theme::one_physical_pixel(painter);
        Self {
            body_width: (plot.slot_width() - METRICS.chart_body_gap)
                .max(METRICS.chart_min_body_width)
                .max(thinnest),
            thinnest,
        }
    }

    fn body(&self, plot: &PlotGeometry, bucket: &ChartBucket, center_x: f32) -> egui::Rect {
        let top = plot.y(bucket.open_half_ticks.max(bucket.close_half_ticks));
        let bottom = plot
            .y(bucket.open_half_ticks.min(bucket.close_half_ticks))
            .max(top + self.thinnest);
        egui::Rect::from_min_max(
            egui::pos2(center_x - self.body_width / 2.0, top),
            egui::pos2(center_x + self.body_width / 2.0, bottom),
        )
    }
}

fn body_color(bucket: &ChartBucket) -> egui::Color32 {
    match bucket.close_half_ticks.cmp(&bucket.open_half_ticks) {
        Ordering::Greater => DARK.bid,
        Ordering::Less => DARK.ask,
        Ordering::Equal => DARK.text_secondary,
    }
}

/// REAL venue fills, each at its OWN bucket and price — never snapped to the mid line — and painted
/// after the series so a marker is never buried under a candle. The price is the one that executed,
/// so a marker sitting off the mid is the truth about where we traded, not a rendering artefact.
fn paint_fills(painter: &egui::Painter, plot: &PlotGeometry, frame: &ChartFrame<'_>) {
    let mut fills =
        chart_view::visible_fills(frame.chart, frame.instrument, plot.domain).peekable();
    if fills.peek().is_none() {
        return;
    }
    let outline = egui::Stroke::new(METRICS.chart_marker_outline, DARK.canvas);
    for fill in fills {
        let color = match fill.side {
            Side::Buy => DARK.bid,
            Side::Sell => DARK.sell_fill,
        };
        painter.circle(
            plot.point(fill.index, fill.half_ticks),
            METRICS.chart_marker_radius,
            color,
            outline,
        );
    }
    paint_fill_caption(painter, plot.rect);
}

fn paint_fill_caption(painter: &egui::Painter, rect: egui::Rect) {
    painter.text(
        rect.left_bottom(),
        egui::Align2::LEFT_BOTTOM,
        "fills",
        egui::FontId::proportional(METRICS.chart_caption_font),
        DARK.text_secondary,
    );
}

pub(crate) fn paint_note(painter: &egui::Painter, rect: egui::Rect, note: &str) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        note,
        egui::FontId::proportional(METRICS.chart_note_font),
        DARK.text_secondary,
    );
}
