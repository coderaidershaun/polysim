//! Deterministic full-surface screens for the phases without a live workstation: starting, draining
//! and stopped. Each is honest — no fabricated numbers — and the stopped reason renders as
//! selectable text so an operator can read or copy the cause.

use eframe::egui;

use super::text::ScreenText;
use super::theme::{DARK, METRICS};

pub(crate) fn paint(ui: &mut egui::Ui, text: &ScreenText<'_>) {
    centered(
        ui,
        text.title,
        DARK.tone(text.title_tone),
        text.detail,
        DARK.tone(text.detail_tone),
    );
}

fn centered(
    ui: &mut egui::Ui,
    title: &str,
    title_color: egui::Color32,
    detail: &str,
    detail_color: egui::Color32,
) {
    let full = ui.available_rect_before_wrap();
    ui.painter().rect_filled(full, 0.0, DARK.canvas);
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(full)
            .layout(egui::Layout::top_down(egui::Align::Center)),
        |ui| {
            ui.add_space(full.height() * 0.40);
            ui.label(
                egui::RichText::new(title)
                    .color(title_color)
                    .monospace()
                    .size(22.0),
            );
            ui.add_space(METRICS.space_2);
            ui.label(egui::RichText::new(detail).color(detail_color).monospace());
        },
    );
}
