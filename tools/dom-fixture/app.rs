//! The fixture page: the workstation's 50/25/25 split with every panel on a fixed scene, stepped by
//! keys.

use eframe::egui;

use polysim::workstation::browser::chart::mid::{self, ChartFrame};
use polysim::workstation::browser::chart::stack::{self, ChartStackFrame};
use polysim::workstation::browser::dom::ladder::{self, DomFrame};
use polysim::workstation::browser::monitor::bands;
use polysim::workstation::chart::position_view::RiskSeries;
use polysim::workstation::chart::view::ChartMode;
use polysim::workstation::dom_view::{DEFAULT_ROWS_PER_SIDE, DomOverlay};
use polysim::workstation::monitor::view::{Channel, MonitorFrame};

use crate::chart_scenes::{ChartScene, chart_scenes};
use crate::hud;
use crate::monitor_feed::MonitorScene;
use crate::monitor_scenes::monitor_scenes;
use crate::position_scenes::{PositionScene, position_scenes};
use crate::scenes::{Scene, scenes};

// SMELL: these repeat colours from the workstation's palette, which the library keeps private, so a
// palette change leaves the fixture's own background and the HUD's text behind.
const CANVAS: egui::Color32 = egui::Color32::from_rgb(10, 13, 18);
const PANEL: egui::Color32 = egui::Color32::from_rgb(15, 19, 26);
const BORDER: egui::Color32 = egui::Color32::from_rgb(44, 53, 67);

/// A position scene brings its own mid chart, so its risk chart always shares that chart's window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftPanel {
    Chart,
    Positions,
}

pub struct DomFixture {
    scenes: Panel<Scene>,
    variant: usize,
    monitors: Panel<MonitorScene>,
    charts: Panel<ChartScene>,
    chart_mode: ChartMode,
    positions: Panel<PositionScene>,
    risk_series: RiskSeries,
    left_panel: LeftPanel,
}

/// The scenes of one panel and the one on show.
struct Panel<T> {
    items: Vec<T>,
    index: usize,
}

impl<T> Panel<T> {
    /// `number` is the operator's pick, counted from one. With no pick the panel starts on the
    /// first scene, and a pick past the end shows the last.
    fn new(items: Vec<T>, number: Option<usize>) -> Self {
        let index = number
            .map_or(0, |number| number.saturating_sub(1))
            .min(items.len() - 1);
        Self { items, index }
    }

    fn step(&mut self, delta: i32) {
        self.index = wrapped(self.index, delta, self.items.len());
    }

    fn current(&self) -> &T {
        &self.items[self.index]
    }

    fn current_mut(&mut self) -> &mut T {
        &mut self.items[self.index]
    }
}

impl DomFixture {
    /// Reads the starting selection from the page's query string, such as `?DOM_SCENE=3&MON_TAB=1`.
    pub fn new() -> Self {
        let scenes = Panel::new(scenes(), number("DOM_SCENE"));
        let variant = number("DOM_VARIANT")
            .unwrap_or(0)
            .min(scenes.current().variants.len() - 1);
        let mut monitors = Panel::new(monitor_scenes(), number("MON_SCENE"));
        if let Some(tab) = Channel::ALL.get(number("MON_TAB").unwrap_or(0)) {
            monitors.current_mut().state.active_tab = *tab;
        }
        // Choosing a risk scene also puts the stacked chart and positions view on the left.
        let risk_scene = number("RISK_SCENE");
        Self {
            scenes,
            variant,
            monitors,
            charts: Panel::new(chart_scenes(), number("CHART_SCENE")),
            // The query can start in either chart mode, so each one can be captured without a key
            // press.
            chart_mode: match query("CHART_MODE").as_deref() {
                Some("candles") => ChartMode::Candles,
                _ => ChartMode::Line,
            },
            positions: Panel::new(position_scenes(), risk_scene),
            risk_series: match query("RISK_SERIES").as_deref() {
                Some("pnl") => RiskSeries::Pnl,
                _ => RiskSeries::Exposure,
            },
            left_panel: match risk_scene {
                Some(_) => LeftPanel::Positions,
                None => LeftPanel::Chart,
            },
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        ctx.input(|input| {
            for (offset, key) in DIGIT_KEYS.iter().enumerate() {
                if input.key_pressed(*key) && offset < self.scenes.items.len() {
                    self.scenes.index = offset;
                    self.variant = 0;
                }
            }
            if input.key_pressed(egui::Key::ArrowDown) {
                self.step_scene(1);
            }
            if input.key_pressed(egui::Key::ArrowUp) {
                self.step_scene(-1);
            }
            if input.key_pressed(egui::Key::ArrowRight) {
                self.step_variant(1);
            }
            if input.key_pressed(egui::Key::ArrowLeft) {
                self.step_variant(-1);
            }
            if input.key_pressed(egui::Key::M) {
                self.monitors.step(1);
            }
            if input.key_pressed(egui::Key::T) {
                self.cycle_tab();
            }
            if input.key_pressed(egui::Key::C) {
                self.charts.step(1);
                self.left_panel = LeftPanel::Chart;
            }
            if input.key_pressed(egui::Key::V) {
                self.flip_chart_mode();
            }
            if input.key_pressed(egui::Key::P) {
                self.positions.step(1);
                self.left_panel = LeftPanel::Positions;
            }
            if input.key_pressed(egui::Key::X) {
                self.flip_risk_series();
            }
        });
    }

    fn step_scene(&mut self, delta: i32) {
        self.scenes.step(delta);
        self.variant = 0;
    }

    fn step_variant(&mut self, delta: i32) {
        self.variant = wrapped(self.variant, delta, self.scenes.current().variants.len());
    }

    fn flip_chart_mode(&mut self) {
        self.chart_mode = match self.chart_mode {
            ChartMode::Line => ChartMode::Candles,
            ChartMode::Candles => ChartMode::Line,
        };
    }

    fn flip_risk_series(&mut self) {
        self.risk_series = match self.risk_series {
            RiskSeries::Exposure => RiskSeries::Pnl,
            RiskSeries::Pnl => RiskSeries::Exposure,
        };
    }

    /// Paints through the workstation's own chart stack. Returns the series the operator clicked.
    fn paint_stack(&self, ui: &mut egui::Ui, body: egui::Rect) -> Option<RiskSeries> {
        let scene = self.positions.current();
        stack::paint(
            ui,
            body,
            &ChartStackFrame {
                chart: &scene.chart,
                positions: &scene.positions,
                instrument: scene.instrument,
                tick: scene.tick,
                mode: self.chart_mode,
                series: self.risk_series,
            },
        )
    }

    fn cycle_tab(&mut self) {
        let state = &mut self.monitors.current_mut().state;
        let index = Channel::ALL
            .iter()
            .position(|channel| *channel == state.active_tab)
            .unwrap_or(0);
        state.active_tab = Channel::ALL[(index + 1) % Channel::ALL.len()];
    }
}

impl eframe::App for DomFixture {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.handle_keys(&ui.ctx().clone());

        let full = ui.available_rect_before_wrap();
        let painter = ui.painter_at(full);
        painter.rect_filled(full, 0.0, CANVAS);
        let (left, center, right) = panels(full);
        painter.rect_filled(left, 0.0, PANEL);

        let dom_scene = self.scenes.current();
        let variant = &dom_scene.variants[self.variant];
        let dom_frame = DomFrame {
            snapshot: variant.snapshot.as_ref(),
            overlay: DomOverlay {
                desired: variant.quote,
                bid_orders: &variant.bid_orders,
                ask_orders: &variant.ask_orders,
            },
            tick: variant.tick,
            grouping: variant.grouping,
            price_decimals: variant.price_decimals,
            qty_scale: variant.qty_scale,
            qty_decimals: variant.qty_decimals,
            feed: variant.feed,
            stale_age: variant.stale_age,
            levels: dom_levels(),
        };
        ladder::paint(ui, right, &dom_frame);

        let monitor = self.monitors.current_mut();
        let monitor_name = monitor.name;
        let tab = monitor.state.active_tab;
        let monitor_frame = MonitorFrame {
            model: &monitor.model,
            instrument: monitor.instrument,
            tick: monitor.tick,
            qty_scale: monitor.qty_scale,
            qty_decimals: monitor.qty_decimals,
            // The ladder variant's own unit, so cycling scenes shows both delta captions.
            dom_unit: variant.grouping.unit(),
            feature_names: &monitor.feature_names,
            instrument_names: &monitor.instrument_names,
        };
        bands::paint(ui, center, &monitor_frame, &mut monitor.state);

        let chart_scene = self.charts.current();
        let position_scene = self.positions.current();
        let body = hud::below(left);
        match self.left_panel {
            LeftPanel::Chart => {
                // This panel draws no crosshair, so the geometry the mid chart hands back goes
                // unused.
                let _mid = mid::paint(
                    ui,
                    body,
                    &ChartFrame {
                        chart: &chart_scene.chart,
                        instrument: chart_scene.instrument,
                        mode: self.chart_mode,
                        tick: chart_scene.tick,
                        domain: None,
                    },
                );
            }
            // The workstation applies the operator's click. The fixture switches series by key.
            LeftPanel::Positions => {
                let _picked = self.paint_stack(ui, body);
            }
        }

        let overlay = ui.painter_at(full);
        for x in [left.right(), center.right()] {
            overlay.vline(x, full.y_range(), egui::Stroke::new(1.0, BORDER));
        }
        hud::paint(
            &overlay,
            &hud::Hud {
                left,
                dom_index: self.scenes.index,
                dom_count: self.scenes.items.len(),
                dom: dom_scene,
                variant: variant.label,
                monitor: monitor_name,
                tab,
                chart: chart_scene.name,
                mode: self.chart_mode,
                position: position_scene.name,
                check: position_scene.check,
                series: self.risk_series,
                left_panel: self.left_panel,
            },
        );
    }
}

fn panels(full: egui::Rect) -> (egui::Rect, egui::Rect, egui::Rect) {
    let left_right = full.left() + full.width() * 0.50;
    let center_right = full.left() + full.width() * 0.75;
    let left = egui::Rect::from_min_max(full.min, egui::pos2(left_right, full.bottom()));
    let center = egui::Rect::from_min_max(
        egui::pos2(left_right, full.top()),
        egui::pos2(center_right, full.bottom()),
    );
    let right = egui::Rect::from_min_max(egui::pos2(center_right, full.top()), full.max);
    (left, center, right)
}

/// `DOM_LEVELS` reproduces any position of the header's level stepper, which the fixture does not
/// paint — the point is to eyeball the ladder at the dense end without a live engine.
fn dom_levels() -> usize {
    number("DOM_LEVELS").unwrap_or(DEFAULT_ROWS_PER_SIDE)
}

/// A value that is not a number gives no pick.
fn number(name: &str) -> Option<usize> {
    query(name)?.parse().ok()
}

fn query(name: &str) -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    search
        .trim_start_matches('?')
        .split('&')
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
        .map(str::to_owned)
}

fn wrapped(index: usize, delta: i32, count: usize) -> usize {
    (index as i32 + delta).rem_euclid(count as i32) as usize
}

const DIGIT_KEYS: [egui::Key; 9] = [
    egui::Key::Num1,
    egui::Key::Num2,
    egui::Key::Num3,
    egui::Key::Num4,
    egui::Key::Num5,
    egui::Key::Num6,
    egui::Key::Num7,
    egui::Key::Num8,
    egui::Key::Num9,
];
