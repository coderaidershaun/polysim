//! Harness overlay. Split for line cap.

use eframe::egui;

use polysim::desktop::chart_view::ChartMode;
use polysim::desktop::monitor::Channel;
use polysim::desktop::position_chart_view::RiskSeries;

use crate::LeftPanel;
use crate::scenes::Scene;

const TEXT: egui::Color32 = egui::Color32::from_rgb(145, 157, 174);
const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(96, 106, 120);

/// Band at top, charts below -> no overlap.
pub const HEIGHT: f32 = 140.0;

pub struct Hud<'a> {
    pub left: egui::Rect,
    pub dom_index: usize,
    pub dom_count: usize,
    pub dom: &'a Scene,
    pub variant: &'a str,
    pub monitor: &'a str,
    pub tab: Channel,
    pub chart: &'a str,
    pub mode: ChartMode,
    pub position: &'a str,
    pub check: &'a str,
    pub series: RiskSeries,
    pub left_panel: LeftPanel,
}

pub fn paint(painter: &egui::Painter, hud: &Hud<'_>) {
    let origin = hud.left.min + egui::vec2(12.0, 10.0);
    painter.text(
        origin,
        egui::Align2::LEFT_TOP,
        format!(
            "dom {}/{}  {}",
            hud.dom_index + 1,
            hud.dom_count,
            hud.dom.name
        ),
        egui::FontId::monospace(13.0),
        TEXT,
    );
    let line = |row: f32, color: egui::Color32, text: String| {
        painter.text(
            origin + egui::vec2(0.0, row),
            egui::Align2::LEFT_TOP,
            text,
            egui::FontId::monospace(11.0),
            color,
        );
    };
    if !hud.variant.is_empty() {
        line(18.0, TEXT_DIM, format!("variant: {}", hud.variant));
    }
    line(
        36.0,
        TEXT,
        format!("monitor: {}  |  tab {}", hud.monitor, hud.tab.label()),
    );
    // The line for the left panel that is not on screen is dimmed, so a reviewer sees at a glance
    // which panel a key press switched to.
    line(
        54.0,
        active_text(hud.left_panel, LeftPanel::Chart),
        format!("chart: {}  |  {}", hud.chart, mode_label(hud.mode)),
    );
    line(
        72.0,
        active_text(hud.left_panel, LeftPanel::Positions),
        format!("risk: {}  |  {}", hud.position, series_label(hud.series)),
    );
    // While the positions panel is on screen, show what its scene asks the reviewer to verify, so
    // they do not have to look it up.
    if hud.left_panel == LeftPanel::Positions {
        line(90.0, TEXT, format!("check: {}", hud.check));
    }
    line(
        108.0,
        TEXT_DIM,
        "1-9 dom | left/right var | up/down step | M mon | T tab | C chart | V line/candles | P risk | X series".to_string(),
    );
}

/// Below HUD: empty on short, never inverted.
pub fn below(left: egui::Rect) -> egui::Rect {
    let top = (left.top() + HEIGHT).min(left.bottom());
    egui::Rect::from_min_max(egui::pos2(left.left(), top), left.max)
}

fn active_text(active: LeftPanel, line: LeftPanel) -> egui::Color32 {
    if active == line { TEXT } else { TEXT_DIM }
}

fn series_label(series: RiskSeries) -> &'static str {
    match series {
        RiskSeries::Exposure => "exposure",
        RiskSeries::Pnl => "pnl",
    }
}

fn mode_label(mode: ChartMode) -> &'static str {
    match mode {
        ChartMode::Line => "line",
        ChartMode::Candles => "candles",
    }
}
