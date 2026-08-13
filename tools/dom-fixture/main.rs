//! GUI harness: 50/25/25 split. Keys: 1-9 scene, arrows/sides, shortcut keys.

mod chart_scenes;
mod hud;
mod monitor_feed;
mod monitor_scenes;
mod position_dump;
mod position_scenes;
mod scenes;

use eframe::egui;

use polysim::desktop::chart::{self, ChartFrame};
use polysim::desktop::chart_stack::{self, ChartStackFrame};
use polysim::desktop::chart_view::ChartMode;
use polysim::desktop::dom::{self, DomFrame};
use polysim::desktop::dom_view::{DEFAULT_ROWS_PER_SIDE, DomOverlay};
use polysim::desktop::monitor::{self, Channel, MonitorFrame};
use polysim::desktop::position_chart_view::RiskSeries;

use chart_scenes::{ChartScene, chart_scenes};
use monitor_feed::MonitorScene;
use monitor_scenes::monitor_scenes;
use position_scenes::{PositionScene, position_scenes};
use scenes::{Scene, scenes};

const CANVAS: egui::Color32 = egui::Color32::from_rgb(10, 13, 18);
const PANEL: egui::Color32 = egui::Color32::from_rgb(15, 19, 26);
const BORDER: egui::Color32 = egui::Color32::from_rgb(44, 53, 67);

fn main() -> eframe::Result {
    // RISK_DUMP text mode for locked machine.
    if std::env::var("RISK_DUMP").is_ok() {
        position_dump::dump();
        return Ok(());
    }
    let selection = initial_selection();
    // Maximised + focused for screencapture -x.
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("dom-fixture")
            .with_inner_size(egui::vec2(1_440.0, 860.0))
            .with_maximized(true)
            .with_active(true),
        renderer: eframe::Renderer::Wgpu,
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "dom-fixture",
        options,
        Box::new(move |_creation| Ok(Box::new(DomFixture::new(selection)))),
    )
}

/// Position scene carries its mid -> risk takes it, never derives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeftPanel {
    Chart,
    Positions,
}

struct DomFixture {
    scenes: Vec<Scene>,
    scene: usize,
    variant: usize,
    monitor_scenes: Vec<MonitorScene>,
    monitor_scene: usize,
    chart_scenes: Vec<ChartScene>,
    chart_scene: usize,
    chart_mode: ChartMode,
    position_scenes: Vec<PositionScene>,
    position_scene: usize,
    risk_series: RiskSeries,
    left_panel: LeftPanel,
}

impl DomFixture {
    fn new(selection: Selection) -> Self {
        let scenes = scenes();
        let scene = selection.scene.min(scenes.len() - 1);
        let variant = selection.variant.min(scenes[scene].variants.len() - 1);
        let mut monitor_scenes = monitor_scenes();
        let monitor_scene = selection.monitor_scene.min(monitor_scenes.len() - 1);
        if let Some(tab) = Channel::ALL.get(selection.monitor_tab) {
            monitor_scenes[monitor_scene].state.active_tab = *tab;
        }
        let chart_scenes = chart_scenes();
        let chart_scene = selection.chart_scene.min(chart_scenes.len() - 1);
        let position_scenes = position_scenes();
        let position_scene = selection.position_scene.min(position_scenes.len() - 1);
        Self {
            scenes,
            scene,
            variant,
            monitor_scenes,
            monitor_scene,
            chart_scenes,
            chart_scene,
            chart_mode: selection.chart_mode,
            position_scenes,
            position_scene,
            risk_series: selection.risk_series,
            left_panel: selection.left_panel,
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        ctx.input(|input| {
            for (offset, key) in DIGIT_KEYS.iter().enumerate() {
                if input.key_pressed(*key) && offset < self.scenes.len() {
                    self.scene = offset;
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
                self.step_monitor_scene(1);
            }
            if input.key_pressed(egui::Key::T) {
                self.cycle_tab();
            }
            if input.key_pressed(egui::Key::C) {
                self.step_chart_scene(1);
                self.left_panel = LeftPanel::Chart;
            }
            if input.key_pressed(egui::Key::V) {
                self.flip_chart_mode();
            }
            if input.key_pressed(egui::Key::P) {
                self.step_position_scene(1);
                self.left_panel = LeftPanel::Positions;
            }
            if input.key_pressed(egui::Key::X) {
                self.flip_risk_series();
            }
        });
    }

    fn step_scene(&mut self, delta: i32) {
        let count = self.scenes.len() as i32;
        self.scene = (self.scene as i32 + delta).rem_euclid(count) as usize;
        self.variant = 0;
    }

    fn step_variant(&mut self, delta: i32) {
        let count = self.scenes[self.scene].variants.len() as i32;
        self.variant = (self.variant as i32 + delta).rem_euclid(count) as usize;
    }

    fn step_monitor_scene(&mut self, delta: i32) {
        let count = self.monitor_scenes.len() as i32;
        self.monitor_scene = (self.monitor_scene as i32 + delta).rem_euclid(count) as usize;
    }

    fn step_chart_scene(&mut self, delta: i32) {
        let count = self.chart_scenes.len() as i32;
        self.chart_scene = (self.chart_scene as i32 + delta).rem_euclid(count) as usize;
    }

    fn step_position_scene(&mut self, delta: i32) {
        let count = self.position_scenes.len() as i32;
        self.position_scene = (self.position_scene as i32 + delta).rem_euclid(count) as usize;
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

    /// Stacked via shipped composer (all real). Returns operator's pick.
    fn paint_stack(&self, ui: &mut egui::Ui, body: egui::Rect) -> Option<RiskSeries> {
        let scene = &self.position_scenes[self.position_scene];
        chart_stack::paint(
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
        let state = &mut self.monitor_scenes[self.monitor_scene].state;
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

        let dom_scene = &self.scenes[self.scene];
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
        dom::paint(ui, right, &dom_frame);

        let monitor = &mut self.monitor_scenes[self.monitor_scene];
        let monitor_name = monitor.name;
        let tab = monitor.state.active_tab;
        let monitor_frame = MonitorFrame {
            model: &monitor.model,
            instrument: monitor.instrument,
            tick: monitor.tick,
            qty_scale: monitor.qty_scale,
            qty_decimals: monitor.qty_decimals,
            // The DOM variant's own unit, so cycling scenes exercises both delta captions.
            dom_unit: variant.grouping.unit(),
            feature_names: &monitor.feature_names,
            instrument_names: &monitor.instrument_names,
        };
        monitor::paint(ui, center, &monitor_frame, &mut monitor.state);

        let chart_scene = &self.chart_scenes[self.chart_scene];
        let position_scene = &self.position_scenes[self.position_scene];
        // Scene carries its mid -> never pair.
        let body = hud::below(left);
        match self.left_panel {
            LeftPanel::Chart => {
                // Bound: #[must_use] prevents orphan.
                let _mid = chart::paint(
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
            // The shipped shell applies the operator's pick; this fixture cycles series by key.
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
                dom_index: self.scene,
                dom_count: self.scenes.len(),
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

        // Fold toggle last once borrows drop: pure view switch applied by caller (mirrors layout.rs).
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

struct Selection {
    scene: usize,
    variant: usize,
    monitor_scene: usize,
    monitor_tab: usize,
    chart_scene: usize,
    chart_mode: ChartMode,
    position_scene: usize,
    risk_series: RiskSeries,
    left_panel: LeftPanel,
}

/// `DOM_LEVELS` reproduces any position of the header's level stepper, which the fixture does not
/// paint — the point is to eyeball the ladder at the dense end without a live engine.
fn dom_levels() -> usize {
    std::env::var("DOM_LEVELS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_ROWS_PER_SIDE)
}

fn initial_selection() -> Selection {
    let mut args = std::env::args().skip(1);
    let scene = args
        .next()
        .or_else(|| std::env::var("DOM_SCENE").ok())
        .and_then(|value| value.parse::<usize>().ok())
        .map(|number| number.saturating_sub(1))
        .unwrap_or(0);
    let variant = args
        .next()
        .or_else(|| std::env::var("DOM_VARIANT").ok())
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let monitor_scene = std::env::var("MON_SCENE")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .map(|number| number.saturating_sub(1))
        .unwrap_or(0);
    let monitor_tab = std::env::var("MON_TAB")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let chart_scene = std::env::var("CHART_SCENE")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .map(|number| number.saturating_sub(1))
        .unwrap_or(0);
    // Signed both modes -> env var picks.
    let chart_mode = match std::env::var("CHART_MODE").as_deref() {
        Ok("candles") => ChartMode::Candles,
        _ => ChartMode::Line,
    };
    // RISK_SCENE selects left panel -> stacked pair.
    let position = std::env::var("RISK_SCENE")
        .ok()
        .and_then(|value| value.parse::<usize>().ok());
    let risk_series = match std::env::var("RISK_SERIES").as_deref() {
        Ok("pnl") => RiskSeries::Pnl,
        _ => RiskSeries::Exposure,
    };
    Selection {
        scene,
        variant,
        monitor_scene,
        monitor_tab,
        chart_scene,
        chart_mode,
        position_scene: position.map_or(0, |number| number.saturating_sub(1)),
        risk_series,
        left_panel: match position {
            Some(_) => LeftPanel::Positions,
            None => LeftPanel::Chart,
        },
    }
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
