//! Exact-rect workstation shell: three full-height panels (50/25/25) bound to one selected
//! instrument. Both control strips carry pure VIEW switches that never reach the engine — the
//! left toolbar's instrument and chart mode, the DOM header's row grouping.

use eframe::egui;

use crate::ids::{InstrumentId, Side};
use crate::time::DurationUs;

use super::chart_stack::{self, ChartStackFrame};
use super::chart_view::ChartMode;
use super::controls;
use super::dom::{self, DomFrame};
use super::dom_view::{
    DomGrouping, DomOverlay, DomUnit, FeedStatus, MAX_ROWS_PER_SIDE, MIN_ROWS_PER_SIDE,
};
use super::format;
use super::latency::{self, LatencyFrame};
use super::model::UiModel;
use super::monitor::{self, MonitorFrame, MonitorUiState};
use super::theme::{self, DARK, METRICS};

const CHART_MODES: [(ChartMode, &str); 2] =
    [(ChartMode::Line, "Line"), (ChartMode::Candles, "Candles")];

const DOM_UNITS: [(DomUnit, &str); 2] = [(DomUnit::Ticks, "Ticks"), (DomUnit::Bps, "bps")];

const ESC_WIDTH: f32 = 64.0;
const INSTRUMENT_WIDTH: f32 = 220.0;
const CHART_MODE_WIDTH: f32 = 112.0;
const DOM_UNIT_WIDTH: f32 = 96.0;
const DOM_GROUPING_WIDTH: f32 = 88.0;
const DOM_LEVELS_WIDTH: f32 = 128.0;

/// The groupings offered per unit, in dropdown order: tick multiples here, fractions of the mid
/// in [`BPS_GROUPINGS`].
const TICK_GROUPINGS: [(DomGrouping, &str); 7] = [
    (DomGrouping::Ticks { per_bucket: 1 }, "x1"),
    (DomGrouping::Ticks { per_bucket: 2 }, "x2"),
    (DomGrouping::Ticks { per_bucket: 5 }, "x5"),
    (DomGrouping::Ticks { per_bucket: 10 }, "x10"),
    (DomGrouping::Ticks { per_bucket: 25 }, "x25"),
    (DomGrouping::Ticks { per_bucket: 50 }, "x50"),
    (DomGrouping::Ticks { per_bucket: 100 }, "x100"),
];

const BPS_GROUPINGS: [(DomGrouping, &str); 6] = [
    (bps(1, 10), "0.1 bp"),
    (bps(1, 4), "0.25 bp"),
    (bps(1, 2), "0.5 bp"),
    (bps(1, 1), "1 bp"),
    (bps(2, 1), "2 bp"),
    (bps(5, 1), "5 bp"),
];

const fn bps(numerator: i64, denominator: i64) -> DomGrouping {
    DomGrouping::Bps {
        numerator,
        denominator,
    }
}

struct ShellRects {
    left: egui::Rect,
    center: egui::Rect,
    right: egui::Rect,
}

/// What the shell asks of its host after a frame. The toolbar's Esc cell is a click like any
/// other; naming the outcome keeps the caller from reading a bare `true` and guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShellAction {
    None,
    CloseRequested,
}

pub(crate) fn workstation(
    ui: &mut egui::Ui,
    model: &mut UiModel,
    monitor_state: &mut MonitorUiState,
    feed: FeedStatus,
) -> ShellAction {
    let full = ui.available_rect_before_wrap();
    ui.allocate_rect(full, egui::Sense::hover());
    let rects = shell_rects(full);
    let painter = ui.painter_at(full);

    painter.rect_filled(full, 0.0, DARK.canvas);
    for rect in [rects.left, rects.center, rects.right] {
        painter.rect_filled(rect, 0.0, DARK.panel);
    }

    let action = paint_left_panel(ui, rects.left, model);
    paint_monitor(ui, rects.center, model, monitor_state);
    paint_dom(ui, rects.right, model, feed);

    paint_dividers(&painter, &rects);
    painter.rect_stroke(
        full,
        0.0,
        egui::Stroke::new(1.0, DARK.border),
        egui::StrokeKind::Inside,
    );
    action
}

pub(crate) fn split_link_bar(full: egui::Rect) -> (egui::Rect, egui::Rect) {
    let split = (full.bottom() - METRICS.link_bar_height).max(full.top());
    (
        egui::Rect::from_min_max(full.min, egui::pos2(full.right(), split)),
        egui::Rect::from_min_max(egui::pos2(full.left(), split), full.max),
    )
}

fn shell_rects(full: egui::Rect) -> ShellRects {
    let split_left_center = full.left() + full.width() * 0.50;
    let split_center_right = full.left() + full.width() * 0.75;
    let left = egui::Rect::from_min_max(full.min, egui::pos2(split_left_center, full.bottom()));
    let center = egui::Rect::from_min_max(
        egui::pos2(split_left_center, full.top()),
        egui::pos2(split_center_right, full.bottom()),
    );
    let right = egui::Rect::from_min_max(egui::pos2(split_center_right, full.top()), full.max);
    ShellRects {
        left,
        center,
        right,
    }
}

fn paint_dividers(painter: &egui::Painter, rects: &ShellRects) {
    let stroke = theme::hairline(painter, DARK.border);
    for x in [rects.left.right(), rects.center.right()] {
        painter.vline(theme::crisp(painter, x), rects.left.y_range(), stroke);
    }
}

fn paint_left_panel(ui: &mut egui::Ui, rect: egui::Rect, model: &mut UiModel) -> ShellAction {
    let toolbar = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.right(), rect.top() + METRICS.toolbar_height),
    );
    let action = paint_toolbar(ui, toolbar, model);
    let body = egui::Rect::from_min_max(egui::pos2(rect.left(), toolbar.bottom()), rect.max);
    paint_left_body(ui, body, model);
    action
}

/// The left panel's control strip: everything above the rule, and the only thing up there a host
/// has to hear about.
fn paint_toolbar(ui: &mut egui::Ui, toolbar: egui::Rect, model: &mut UiModel) -> ShellAction {
    let painter = ui.painter_at(toolbar);
    painter.rect_filled(toolbar, 0.0, DARK.panel_raised);

    let mut cursor = toolbar.left();
    let esc_rect = strip_cell(toolbar, &mut cursor, ESC_WIDTH);
    let is_esc_clicked = paint_esc(ui, esc_rect);

    let combo_rect = strip_cell(toolbar, &mut cursor, INSTRUMENT_WIDTH);
    paint_instrument_dropdown(ui, combo_rect, model);

    let mode_rect = strip_cell(toolbar, &mut cursor, CHART_MODE_WIDTH);
    paint_chart_mode_toggle(ui, mode_rect, model);

    let stroke = theme::hairline(&painter, DARK.border);
    for x in [esc_rect.right(), combo_rect.right(), mode_rect.right()] {
        painter.vline(theme::crisp(&painter, x), toolbar.y_range(), stroke);
    }

    if let Some(catalog) = model.catalog() {
        painter.text(
            egui::pos2(mode_rect.right() + METRICS.space_3, toolbar.center().y),
            egui::Align2::LEFT_CENTER,
            catalog.strategy_id.as_ref(),
            egui::FontId::proportional(13.0),
            DARK.text_secondary,
        );
    }
    // No catalog yet means nothing has claimed to be armed, which reads the same as `off`.
    let execution_mode = model.catalog().and_then(|catalog| catalog.execution_mode);
    controls::paint_execution_mode_badge(&painter, toolbar, execution_mode);

    painter.hline(
        toolbar.x_range(),
        theme::crisp_bottom_edge(&painter, toolbar.bottom()),
        stroke,
    );

    if is_esc_clicked { ShellAction::CloseRequested } else { ShellAction::None }
}

/// The left panel below the rule: the latency grid over the chart stack.
fn paint_left_body(ui: &mut egui::Ui, body: egui::Rect, model: &mut UiModel) {
    let (latency_rect, charts) = latency::split(body);
    latency::paint(
        ui,
        latency_rect,
        &LatencyFrame {
            summary: model.latency(),
        },
    );

    let picked = {
        let frame = ChartStackFrame {
            chart: model.chart(),
            positions: model.positions(),
            instrument: model.selected(),
            tick: model.instrument_scales().tick,
            mode: model.chart_mode(),
            series: model.risk_series(),
        };
        chart_stack::paint(ui, charts, &frame)
    };
    if let Some(series) = picked {
        model.set_risk_series(series);
    }
}

/// Hand out the next full-height cell in a control strip, so controls abut instead of floating on it.
fn strip_cell(strip: egui::Rect, cursor: &mut f32, width: f32) -> egui::Rect {
    let left = *cursor;
    *cursor += width;
    egui::Rect::from_min_max(
        egui::pos2(left, strip.top()),
        egui::pos2(left + width, strip.bottom()),
    )
}

/// Hand-painted rather than an `egui::Button`: `ui.put` paints its own label immediately, so a
/// hover fill applied afterwards would erase it.
fn paint_esc(ui: &mut egui::Ui, rect: egui::Rect) -> bool {
    let response = ui.interact(rect, egui::Id::new("polysim-esc"), egui::Sense::click());
    let painter = ui.painter_at(rect);
    if response.hovered() {
        painter.rect_filled(rect, 0.0, DARK.panel);
    }
    painter.text(
        egui::pos2(rect.center().x, theme::crisp(&painter, rect.center().y)),
        egui::Align2::CENTER_CENTER,
        "Esc",
        egui::FontId::proportional(METRICS.segment_font),
        DARK.text_secondary,
    );
    response.clicked()
}

fn paint_chart_mode_toggle(ui: &mut egui::Ui, rect: egui::Rect, model: &mut UiModel) {
    let current = model.chart_mode();
    if let Some(mode) =
        controls::paint_segmented_toggle(ui, rect, "chart-mode", &CHART_MODES, current)
    {
        model.set_chart_mode(mode);
    }
}

fn paint_instrument_dropdown(ui: &mut egui::Ui, rect: egui::Rect, model: &mut UiModel) {
    let mut chosen = model.selected();
    let builder = egui::UiBuilder::new()
        .max_rect(rect)
        .layout(egui::Layout::left_to_right(egui::Align::Center));
    ui.scope_builder(builder, |ui| {
        style_combo(ui.style_mut(), rect.height());
        let selected_text = model
            .catalog()
            .and_then(|catalog| catalog.instrument(chosen))
            .map_or(format::MISSING, |instrument| instrument.display.as_ref());
        egui::ComboBox::from_id_salt("polysim-instrument")
            .width(rect.width())
            .selected_text(selected_text)
            .show_ui(ui, |ui| {
                let Some(catalog) = model.catalog() else { return };
                for instrument in &catalog.instruments {
                    ui.selectable_value(
                        &mut chosen,
                        instrument.instrument_id,
                        instrument.display.as_ref(),
                    );
                }
            });
    });
    if chosen != model.selected() {
        model.select(chosen);
    }
}

fn style_combo(style: &mut egui::Style, height: f32) {
    style.spacing.interact_size.y = height;
    let widgets = &mut style.visuals.widgets;
    for state in [
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        state.bg_stroke = egui::Stroke::NONE;
        state.corner_radius = egui::CornerRadius::ZERO;
    }
    widgets.inactive.weak_bg_fill = DARK.panel_raised;
    widgets.hovered.weak_bg_fill = DARK.panel;
    widgets.active.weak_bg_fill = DARK.panel;
    widgets.open.weak_bg_fill = DARK.panel;
    // The popup floats above the shell, so it keeps its radius (the flush law is for inline cells).
    style.visuals.menu_corner_radius = egui::CornerRadius::same(METRICS.corner_radius as u8);
}

fn paint_monitor(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    model: &UiModel,
    monitor_state: &mut MonitorUiState,
) {
    let selected = model.selected();
    let catalog = model.catalog();
    let scales = model.instrument_scales();

    let feature_names: &[Box<str>] = catalog.map_or(&[], |catalog| &catalog.feature_names);
    let instrument_names: Vec<Box<str>> = catalog.map_or_else(Vec::new, |catalog| {
        catalog
            .instruments
            .iter()
            .map(|instrument| instrument.display.clone())
            .collect()
    });

    let frame = MonitorFrame {
        model,
        instrument: selected,
        tick: scales.tick,
        qty_scale: scales.qty_scale,
        qty_decimals: scales.qty_decimals,
        dom_unit: model.dom_unit(),
        feature_names,
        instrument_names: &instrument_names,
    };
    monitor::paint(ui, rect, &frame, monitor_state);
}

fn paint_dom(ui: &mut egui::Ui, rect: egui::Rect, model: &mut UiModel, feed: FeedStatus) {
    let header = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.right(), rect.top() + METRICS.dom_header_height),
    );
    let body = egui::Rect::from_min_max(egui::pos2(rect.left(), header.bottom()), rect.max);
    paint_dom_header(ui, header, model, dom::rows_that_fit(body));

    let selected = model.selected();
    let scales = model.instrument_scales();
    let snapshot = model.book(selected);
    let desired = model
        .is_quote_live(selected)
        .then(|| model.quote(selected).map(|(quote, _)| quote))
        .flatten();
    let stale_age = book_staleness(model, selected);
    let feed = match (feed, stale_age) {
        (FeedStatus::Live, Some(_)) => FeedStatus::Stale,
        (feed, _) => feed,
    };

    let frame = DomFrame {
        snapshot,
        overlay: DomOverlay {
            desired,
            bid_orders: model.exec().working(selected, Side::Buy),
            ask_orders: model.exec().working(selected, Side::Sell),
        },
        tick: scales.tick,
        grouping: model.dom_grouping(),
        levels: model.dom_levels(),
        price_decimals: format::price_decimals(scales.tick),
        qty_scale: scales.qty_scale,
        qty_decimals: scales.qty_decimals,
        feed,
        stale_age,
    };
    dom::paint(ui, body, &frame);
}

fn paint_dom_header(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    model: &mut UiModel,
    rows_that_fit: usize,
) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, DARK.panel_raised);

    let mut cursor = rect.left();
    let toggle = strip_cell(rect, &mut cursor, DOM_UNIT_WIDTH);
    let current = model.dom_unit();
    if let Some(unit) =
        controls::paint_segmented_toggle(ui, toggle, "dom-unit", &DOM_UNITS, current)
    {
        model.set_dom_unit(unit);
    }

    let combo = strip_cell(rect, &mut cursor, DOM_GROUPING_WIDTH);
    paint_grouping_dropdown(ui, combo, model);

    // Right-aligned, but never allowed to overrun the grouping combo on a narrow window.
    let levels_rect = egui::Rect::from_min_max(
        egui::pos2(
            (rect.right() - DOM_LEVELS_WIDTH).max(combo.right()),
            rect.top(),
        ),
        rect.max,
    );
    let spec = controls::StepperSpec {
        label: "LVL",
        value: model.dom_levels(),
        min: MIN_ROWS_PER_SIDE,
        max: MAX_ROWS_PER_SIDE,
        capped_to: rows_that_fit,
    };
    if let Some(chosen) = controls::paint_stepper(ui, levels_rect, "dom-levels", spec) {
        model.set_dom_levels(chosen);
    }

    let stroke = theme::hairline(&painter, DARK.border);
    for x in [toggle.right(), combo.right(), levels_rect.left()] {
        painter.vline(theme::crisp(&painter, x), rect.y_range(), stroke);
    }
    painter.hline(
        rect.x_range(),
        theme::crisp_bottom_edge(&painter, rect.bottom()),
        stroke,
    );
}

fn paint_grouping_dropdown(ui: &mut egui::Ui, rect: egui::Rect, model: &mut UiModel) {
    let current = model.dom_grouping();
    let mut chosen = current;
    let builder = egui::UiBuilder::new()
        .max_rect(rect)
        .layout(egui::Layout::left_to_right(egui::Align::Center));
    ui.scope_builder(builder, |ui| {
        style_combo(ui.style_mut(), rect.height());
        egui::ComboBox::from_id_salt("polysim-dom-grouping")
            .width(rect.width())
            .selected_text(grouping_label(current))
            .show_ui(ui, |ui| {
                for (grouping, label) in grouping_table(current.unit()) {
                    ui.selectable_value(&mut chosen, *grouping, *label);
                }
            });
    });
    if chosen != current {
        model.set_dom_grouping(chosen);
    }
}

fn grouping_table(unit: DomUnit) -> &'static [(DomGrouping, &'static str)] {
    match unit {
        DomUnit::Ticks => &TICK_GROUPINGS,
        DomUnit::Bps => &BPS_GROUPINGS,
    }
}

fn grouping_label(grouping: DomGrouping) -> &'static str {
    grouping_table(grouping.unit())
        .iter()
        .find(|(candidate, _)| *candidate == grouping)
        .map_or(format::MISSING, |(_, label)| *label)
}

/// How far behind the ladder is, once that is far enough to say so. Both sides of the subtraction
/// are the ENGINE's clock: the workstation is a separate process reached over UDP at an arbitrary
/// address, and reading its own clock here would let a few seconds of skew between two machines
/// paint a frozen ladder as a live one — the exact reading this badge exists to prevent.
fn book_staleness(model: &UiModel, instrument: InstrumentId) -> Option<DurationUs> {
    model
        .book_lag(instrument)
        .filter(|lag| *lag > theme::DOM_STALE_THRESHOLD)
}
