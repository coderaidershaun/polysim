//! The one strip about the CONNECTION rather than the market. Spans the full width below the three
//! panels because it belongs to the window, and paints on the waiting screen too — that is the
//! moment an operator most needs to know whether the engine is answering at all.

use eframe::egui;

use crate::link::RunState;

use super::link_model::LinkStatus;
use super::text;
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
    theme::rules_h(&painter, [rect.top()], rect.x_range());

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

    let chips = text::link_chips(status);
    let mut cursor = rect.left() + METRICS.space_3;
    let middle = rect.center().y;
    let mut peer_hit = egui::Rect::NOTHING;
    for (index, (label, tone)) in chips.iter().enumerate() {
        let width = chip(&painter, cursor, middle, label, DARK.tone(*tone));
        if index == 0 {
            peer_hit = egui::Rect::from_min_max(
                egui::pos2(cursor - CHIP_PAD, rect.top() + 2.0),
                egui::pos2(cursor + width + CHIP_PAD, rect.bottom() - 2.0),
            );
        }
        cursor += width + METRICS.space_3;
    }

    let action = paint_stop_start(ui, rect, status);
    if action != LinkAction::None {
        return action;
    }
    let peer = chips.first().map_or("", |(label, _)| label.as_str());
    paint_peer_picker(ui, peer_hit, status, peer)
}

fn paint_peer_picker(
    ui: &mut egui::Ui,
    hit: egui::Rect,
    status: &LinkStatus,
    label: &str,
) -> LinkAction {
    if !text::is_peer_clickable(status) {
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
            label,
            DARK.text_primary,
        );
    }
    if response.clicked() { LinkAction::NextPeer } else { LinkAction::None }
}

fn paint_stop_start(ui: &mut egui::Ui, rect: egui::Rect, status: &LinkStatus) -> LinkAction {
    let target = status.next_assertion();
    let button = rect.with_min_x(rect.right() - BUTTON_WIDTH);
    let label = text::button_label(target);
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
    theme::rules_v(&painter, [button.left()], button.y_range());
    painter.text(
        egui::pos2(button.center().x, theme::crisp(&painter, button.center().y)),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::monospace(FONT_SIZE),
        DARK.text_primary,
    );
    if response.clicked() { LinkAction::Assert(target) } else { LinkAction::None }
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
