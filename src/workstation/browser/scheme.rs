//! The operator switches between the dark and light palettes in the top bar, and the browser keeps
//! the choice for the next visit.

use eframe::egui;
use eframe::web::storage::{local_storage_get, local_storage_set};

use super::theme::Palette;
use crate::workstation::text::{TO_DARK_HOVER, TO_LIGHT_HOVER};

const KEY: &str = "polysim-scheme";
const ICON_RADIUS: f32 = 8.0;
const RAY_COUNT: u8 = 8;
const RAY_WIDTH: f32 = 2.0;

/// Dark unless the browser kept the light choice. A missing, unknown or unreadable value reads as
/// dark.
pub(super) fn stored() -> egui::Theme {
    match local_storage_get(KEY) {
        Some(word) if word == stored_word(egui::Theme::Light) => egui::Theme::Light,
        _ => egui::Theme::Dark,
    }
}

fn stored_word(theme: egui::Theme) -> &'static str {
    match theme {
        egui::Theme::Dark => "dark",
        egui::Theme::Light => "light",
    }
}

/// The icon shows what a click turns the page to. It is drawn from shapes because only the bundled
/// emoji fonts have sun and moon characters, and once those fonts go a character would draw as an
/// empty box.
pub(super) fn paint_switch(ui: &mut egui::Ui, rect: egui::Rect) {
    let palette = Palette::of(ui.ctx());
    let response = ui.interact(rect, egui::Id::new("scheme-switch"), egui::Sense::click());
    let is_dark = ui.ctx().theme() == egui::Theme::Dark;
    let fill = if response.hovered() { palette.panel } else { palette.panel_raised };
    let ink = if response.hovered() { palette.text_primary } else { palette.text_secondary };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, fill);
    if is_dark {
        paint_sun(&painter, rect.center(), ink);
    } else {
        paint_moon(&painter, rect.center(), ink, fill);
    }
    let hover = if is_dark { TO_LIGHT_HOVER } else { TO_DARK_HOVER };
    if response.on_hover_text(hover).clicked() {
        switch(ui.ctx());
    }
}

fn paint_sun(painter: &egui::Painter, center: egui::Pos2, ink: egui::Color32) {
    painter.circle_filled(center, ICON_RADIUS * 0.45, ink);
    let stroke = egui::Stroke::new(RAY_WIDTH, ink);
    for ray in 0..RAY_COUNT {
        let angle = f32::from(ray) * std::f32::consts::TAU / f32::from(RAY_COUNT);
        let direction = egui::vec2(angle.cos(), angle.sin());
        let inner = center + direction * (ICON_RADIUS * 0.72);
        let outer = center + direction * ICON_RADIUS;
        painter.line_segment([inner, outer], stroke);
    }
}

fn paint_moon(
    painter: &egui::Painter,
    center: egui::Pos2,
    ink: egui::Color32,
    fill: egui::Color32,
) {
    painter.circle_filled(center, ICON_RADIUS * 0.8, ink);
    let bite = center + egui::vec2(ICON_RADIUS * 0.45, -ICON_RADIUS * 0.45);
    painter.circle_filled(bite, ICON_RADIUS * 0.7, fill);
}

fn switch(ctx: &egui::Context) {
    let next = match ctx.theme() {
        egui::Theme::Dark => egui::Theme::Light,
        egui::Theme::Light => egui::Theme::Dark,
    };
    ctx.set_theme(next);
    local_storage_set(KEY, stored_word(next));
    // This frame was partly drawn in the old palette, so throw it away and draw it again.
    ctx.request_discard("the palette changed");
}
