//! The one strip about the CONNECTION rather than the market. Spans the full width below the three
//! panels because it belongs to the window, and paints on the waiting screen too — that is the
//! moment an operator most needs to know whether the engine is answering at all.

use eframe::egui;

use crate::link::{RunPhase, RunState};

use super::format::MISSING;
use super::link_model::{ConnectionState, ControlVerdict, LinkStatus};
use super::theme::{self, DARK, METRICS};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum LinkAction {
    #[default]
    None,
    Assert(RunState),
    NextPeer,
}

const FONT_SIZE: f32 = 11.0;
const CHIP_PAD: f32 = 7.0;
const BUTTON_WIDTH: f32 = 64.0;

pub(crate) fn paint(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    status: Option<&LinkStatus>,
) -> LinkAction {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, DARK.panel_raised);
    painter.hline(
        rect.x_range(),
        theme::crisp(&painter, rect.top()),
        theme::hairline(&painter, DARK.border),
    );

    let Some(status) = status else {
        chip(
            &painter,
            rect.left() + METRICS.space_3,
            rect.center().y,
            "no engine attached",
            DARK.text_secondary,
        );
        return LinkAction::None;
    };

    let mut cursor = rect.left() + METRICS.space_3;
    let middle = rect.center().y;
    let peer_width = chip(
        &painter,
        cursor,
        middle,
        &peer_label(status),
        DARK.text_primary,
    );
    let peer_hit = egui::Rect::from_min_max(
        egui::pos2(cursor - CHIP_PAD, rect.top() + 2.0),
        egui::pos2(cursor + peer_width + CHIP_PAD, rect.bottom() - 2.0),
    );
    cursor += peer_width + METRICS.space_3;

    let (connection, connection_color) = connection_label(status.connection);
    cursor += chip(&painter, cursor, middle, connection, connection_color) + METRICS.space_3;

    if let Some(phase) = status.phase {
        let (label, color) = phase_label(phase);
        cursor += chip(&painter, cursor, middle, label, color) + METRICS.space_3;
    }

    cursor += chip(
        &painter,
        cursor,
        middle,
        &run_label(status),
        run_color(status),
    ) + METRICS.space_3;
    chip(
        &painter,
        cursor,
        middle,
        &control_label(status),
        control_color(status),
    );

    let action = paint_stop_start(ui, rect, status);
    if action != LinkAction::None {
        return action;
    }
    paint_peer_picker(ui, peer_hit, status)
}

fn paint_peer_picker(ui: &mut egui::Ui, hit: egui::Rect, status: &LinkStatus) -> LinkAction {
    if status.peer_count < 2 {
        return LinkAction::None;
    }
    let response = ui
        .interact(
            hit,
            egui::Id::new("polysim-link-peer"),
            egui::Sense::click(),
        )
        .on_hover_text("attach to the next trading engine");
    if response.hovered() {
        // Fill then REPAINT the label: the caller already drew it, and an accent bar here would
        // speak the selected vocabulary to mean merely hoverable.
        let painter = ui.painter_at(hit);
        painter.rect_filled(hit, 0.0, DARK.panel);
        chip(
            &painter,
            hit.left() + CHIP_PAD,
            hit.center().y,
            &peer_label(status),
            DARK.text_primary,
        );
    }
    if response.clicked() { LinkAction::NextPeer } else { LinkAction::None }
}

fn paint_stop_start(ui: &mut egui::Ui, rect: egui::Rect, status: &LinkStatus) -> LinkAction {
    let target = status.next_assertion();
    let button = egui::Rect::from_min_max(
        egui::pos2(rect.right() - BUTTON_WIDTH, rect.top()),
        rect.max,
    );
    let label = match target {
        RunState::Idle => "STOP",
        RunState::Running => "START",
    };
    let response = ui.interact(
        button,
        egui::Id::new("polysim-link-assert"),
        egui::Sense::click(),
    );
    let painter = ui.painter_at(button);
    painter.rect_filled(
        button,
        0.0,
        if response.hovered() { DARK.panel } else { DARK.panel_raised },
    );
    painter.vline(
        theme::crisp(&painter, button.left()),
        button.y_range(),
        theme::hairline(&painter, DARK.border),
    );
    painter.text(
        egui::pos2(button.center().x, theme::crisp(&painter, button.center().y)),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::monospace(FONT_SIZE),
        DARK.text_primary,
    );
    if response.clicked() { LinkAction::Assert(target) } else { LinkAction::None }
}

fn peer_label(status: &LinkStatus) -> String {
    if status.peer_count < 2 {
        return format!("engine {}", status.peer);
    }
    format!(
        "engine {} [{}/{}]",
        status.peer,
        status.peer_index + 1,
        status.peer_count
    )
}

fn connection_label(connection: ConnectionState) -> (&'static str, egui::Color32) {
    match connection {
        ConnectionState::Connecting => ("CONNECTING", DARK.text_secondary),
        ConnectionState::Live => ("LIVE", DARK.positive),
        ConnectionState::Stale => ("STALE", DARK.stale),
    }
}

fn phase_label(phase: RunPhase) -> (&'static str, egui::Color32) {
    match phase {
        RunPhase::Starting => ("starting", DARK.text_secondary),
        RunPhase::Ready => ("ready", DARK.text_secondary),
        RunPhase::Draining => ("draining", DARK.warning),
        RunPhase::Stopped => ("stopped", DARK.invalid),
    }
}

fn run_label(status: &LinkStatus) -> String {
    match status.reported_state {
        Some(RunState::Running) => "run RUNNING".to_owned(),
        Some(RunState::Idle) => "run IDLE".to_owned(),
        None => format!("run {MISSING}"),
    }
}

fn run_color(status: &LinkStatus) -> egui::Color32 {
    match status.reported_state {
        Some(RunState::Running) => DARK.positive,
        Some(RunState::Idle) => DARK.warning,
        None => DARK.text_secondary,
    }
}

fn control_label(status: &LinkStatus) -> String {
    match (status.control, status.asserted_state) {
        (ControlVerdict::NoOpinion, _) => "control none".to_owned(),
        (ControlVerdict::Applied, Some(state)) => format!("control applied {}", state_word(state)),
        (ControlVerdict::Pending, Some(state)) => {
            format!("control asserting {}", state_word(state))
        }
        (ControlVerdict::Lost { holder_epoch }, _) => {
            format!("control LOST to epoch {holder_epoch}")
        }
        (_, None) => "control none".to_owned(),
    }
}

fn control_color(status: &LinkStatus) -> egui::Color32 {
    match status.control {
        ControlVerdict::NoOpinion => DARK.text_secondary,
        ControlVerdict::Pending => DARK.warning,
        ControlVerdict::Applied => DARK.positive,
        ControlVerdict::Lost { .. } => DARK.invalid,
    }
}

fn state_word(state: RunState) -> &'static str {
    match state {
        RunState::Running => "RUNNING",
        RunState::Idle => "IDLE",
    }
}

fn chip(painter: &egui::Painter, left: f32, middle: f32, text: &str, color: egui::Color32) -> f32 {
    let galley = painter.layout_no_wrap(text.to_owned(), egui::FontId::monospace(FONT_SIZE), color);
    let width = galley.size().x;
    painter.galley(
        egui::pos2(left, middle - galley.size().y / 2.0),
        galley,
        color,
    );
    width
}
