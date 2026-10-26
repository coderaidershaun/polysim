//! The workstation page: three full-height panels, split 50/25/25, all showing one selected
//! instrument. The two control strips hold view switches only, which never reach the engine: the
//! toolbar's instrument and chart mode, and the ladder header's unit, grouping and depth.

use eframe::egui;

use super::controls;
use super::latency::{self, LatencyFrame};
use super::theme::{self, DARK, METRICS};
use crate::workstation::browser::chart::stack::{self, ChartStackFrame};
use crate::workstation::browser::dom::ladder::{self, DomFrame};
use crate::workstation::browser::monitor::bands;
use crate::workstation::chart::view::ChartMode;
use crate::workstation::dom_view::{DomUnit, FeedStatus, MAX_ROWS_PER_SIDE, MIN_ROWS_PER_SIDE};
use crate::workstation::format;
use crate::workstation::model::UiModel;
use crate::workstation::monitor::view::{MonitorFrame, MonitorUiState, instrument_names};
use crate::workstation::text::{grouping_label, grouping_table};

const CHART_MODES: [(ChartMode, &str); 2] =
    [(ChartMode::Line, "Line"), (ChartMode::Candles, "Candles")];

const DOM_UNITS: [(DomUnit, &str); 2] = [(DomUnit::Ticks, "Ticks"), (DomUnit::Bps, "bps")];

const INSTRUMENT_WIDTH: f32 = 220.0;
const CHART_MODE_WIDTH: f32 = 112.0;
const DOM_UNIT_WIDTH: f32 = 96.0;
const DOM_GROUPING_WIDTH: f32 = 88.0;
const DOM_LEVELS_WIDTH: f32 = 128.0;

pub(crate) fn workstation(
    ui: &mut egui::Ui,
    model: &mut UiModel,
    monitor_state: &mut MonitorUiState,
    feed: FeedStatus,
) {
    let full = ui.available_rect_before_wrap();
    ui.allocate_rect(full, egui::Sense::hover());
    let (left, rest) = full.split_left_right_at_x(full.left() + full.width() * 0.50);
    let (center, right) = rest.split_left_right_at_x(full.left() + full.width() * 0.75);
    let painter = ui.painter_at(full);

    painter.rect_filled(full, 0.0, DARK.canvas);
    for rect in [left, center, right] {
        painter.rect_filled(rect, 0.0, DARK.panel);
    }

    paint_left_panel(ui, left, model);
    paint_monitor(ui, center, model, monitor_state);
    paint_dom(ui, right, model, feed);

    theme::rules_v(&painter, [left.right(), center.right()], full.y_range());
    painter.rect_stroke(
        full,
        0.0,
        egui::Stroke::new(1.0, DARK.border),
        egui::StrokeKind::Inside,
    );
}

pub(crate) fn split_link_bar(full: egui::Rect) -> (egui::Rect, egui::Rect) {
    full.split_top_bottom_at_y((full.bottom() - METRICS.link_bar_height).max(full.top()))
}

fn paint_left_panel(ui: &mut egui::Ui, rect: egui::Rect, model: &mut UiModel) {
    let (toolbar, body) = rect.split_top_bottom_at_y(rect.top() + METRICS.toolbar_height);
    paint_toolbar(ui, toolbar, model);
    paint_left_body(ui, body, model);
}

/// The left panel's control strip: everything above the rule.
fn paint_toolbar(ui: &mut egui::Ui, toolbar: egui::Rect, model: &mut UiModel) {
    let painter = ui.painter_at(toolbar);
    painter.rect_filled(toolbar, 0.0, DARK.panel_raised);

    let mut cursor = toolbar.left();
    let combo_rect = strip_cell(toolbar, &mut cursor, INSTRUMENT_WIDTH);
    paint_instrument_dropdown(ui, combo_rect, model);

    let mode_rect = strip_cell(toolbar, &mut cursor, CHART_MODE_WIDTH);
    paint_chart_mode_toggle(ui, mode_rect, model);

    let seams = [combo_rect.right(), mode_rect.right()];
    theme::rules_v(&painter, seams, toolbar.y_range());

    if let Some(catalog) = model.catalog() {
        painter.text(
            egui::pos2(mode_rect.right() + METRICS.space_3, toolbar.center().y),
            egui::Align2::LEFT_CENTER,
            catalog.strategy_id.as_ref(),
            egui::FontId::proportional(13.0),
            DARK.text_secondary,
        );
    }
    let execution_mode = model.catalog().and_then(|catalog| catalog.execution_mode);
    controls::paint_execution_mode_badge(&painter, toolbar, execution_mode);

    painter.hline(
        toolbar.x_range(),
        theme::crisp_bottom_edge(&painter, toolbar.bottom()),
        theme::hairline(&painter, DARK.border),
    );
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
        stack::paint(ui, charts, &frame)
    };
    if let Some(series) = picked {
        model.set_risk_series(series);
    }
}

/// Hand out the next full-height cell in a control strip, so controls abut instead of floating on it.
fn strip_cell(strip: egui::Rect, cursor: &mut f32, width: f32) -> egui::Rect {
    let left = *cursor;
    *cursor += width;
    strip.with_min_x(left).with_max_x(left + width)
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
    // The popup floats above the panels, so it keeps rounded corners. Only cells drawn inline sit
    // square.
    style.visuals.menu_corner_radius = egui::CornerRadius::same(METRICS.corner_radius as u8);
}

fn paint_monitor(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    model: &UiModel,
    monitor_state: &mut MonitorUiState,
) {
    let instrument_names = instrument_names(model);
    let frame = MonitorFrame::for_selected(model, &instrument_names);
    bands::paint(ui, rect, &frame, monitor_state);
}

fn paint_dom(ui: &mut egui::Ui, rect: egui::Rect, model: &mut UiModel, feed: FeedStatus) {
    let (header, body) = rect.split_top_bottom_at_y(rect.top() + METRICS.dom_header_height);
    paint_dom_header(ui, header, model, ladder::rows_that_fit(body));

    let scales = model.instrument_scales();
    let source = model.dom_source(feed);
    let frame = DomFrame {
        snapshot: model.book(model.selected()),
        overlay: source.overlay,
        tick: scales.tick,
        grouping: model.dom_grouping(),
        levels: model.dom_levels(),
        price_decimals: format::price_decimals(scales.tick),
        qty_scale: scales.qty_scale,
        qty_decimals: scales.qty_decimals,
        feed: source.feed,
        stale_age: source.stale_age,
    };
    ladder::paint(ui, body, &frame);
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

    // Right-aligned, but never allowed to overrun the grouping combo on a narrow page.
    let levels_rect = rect.with_min_x((rect.right() - DOM_LEVELS_WIDTH).max(combo.right()));
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

    let seams = [toggle.right(), combo.right(), levels_rect.left()];
    theme::rules_v(&painter, seams, rect.y_range());
    painter.hline(
        rect.x_range(),
        theme::crisp_bottom_edge(&painter, rect.bottom()),
        theme::hairline(&painter, DARK.border),
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
