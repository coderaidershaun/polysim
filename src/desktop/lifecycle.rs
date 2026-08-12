//! Deterministic full-surface screens for the phases without a live workstation: starting, draining
//! and stopped. Each is honest — no fabricated numbers — and the stopped reason renders as
//! selectable text so an operator can read or copy the cause.

use eframe::egui;

use crate::runtime::ExitReport;

use super::theme::{DARK, METRICS};

pub(crate) fn starting(ui: &mut egui::Ui) {
    centered(
        ui,
        "STARTING",
        DARK.text_primary,
        "waiting for engine",
        DARK.text_secondary,
    );
}

pub(crate) fn draining(ui: &mut egui::Ui, reason: &str) {
    centered(ui, "DRAINING", DARK.warning, reason, DARK.text_secondary);
}

pub(crate) fn stopped(ui: &mut egui::Ui, report: &ExitReport) {
    let (title, color) = if report.graceful {
        ("STOPPED - graceful", DARK.positive)
    } else {
        ("STOPPED - fatal", DARK.invalid)
    };
    centered(
        ui,
        title,
        color,
        report.reason.as_ref(),
        DARK.text_secondary,
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
