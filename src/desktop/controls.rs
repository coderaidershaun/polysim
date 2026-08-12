//! Hand-painted controls shared across panels — consistent design language. Cells fill their
//! container and are parted by a hairline the container draws: selection is a fill plus an accent
//! bar on one edge, never a box around a cell.

use eframe::egui;

use crate::config::ExecutionMode;

use super::theme::{self, DARK, METRICS};

const EXECUTION_MODE_BADGE_WIDTH: f32 = 60.0;

pub(crate) fn paint_execution_mode_badge(
    painter: &egui::Painter,
    strip: egui::Rect,
    mode: Option<ExecutionMode>,
) {
    let rect = egui::Rect::from_min_max(
        egui::pos2(strip.right() - EXECUTION_MODE_BADGE_WIDTH, strip.top()),
        egui::pos2(strip.right(), strip.bottom()),
    );
    let (fill, text) = match mode {
        None | Some(ExecutionMode::Off) => (DARK.border, DARK.text_primary),
        Some(ExecutionMode::Sim) => (DARK.warning, DARK.canvas),
        Some(ExecutionMode::Live) => (DARK.negative, DARK.canvas),
    };
    painter.rect_filled(rect, 0.0, fill);
    painter.text(
        egui::pos2(rect.center().x, theme::crisp(painter, rect.center().y)),
        egui::Align2::CENTER_CENTER,
        ExecutionMode::badge(mode),
        egui::FontId::proportional(METRICS.segment_font),
        text,
    );
}

pub(crate) fn paint_segmented_toggle<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id_salt: &str,
    segments: &[(T, &str)],
    current: T,
) -> Option<T> {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, DARK.panel);

    let mut clicked = None;
    let cell_width = rect.width() / segments.len() as f32;
    for (index, (value, label)) in segments.iter().enumerate() {
        let cell = egui::Rect::from_min_size(
            egui::pos2(rect.left() + index as f32 * cell_width, rect.top()),
            egui::vec2(cell_width, rect.height()),
        );
        let response = ui.interact(cell, egui::Id::new((id_salt, index)), egui::Sense::click());

        let active = *value == current;
        if active {
            fill_selected(&painter, cell);
        } else if response.hovered() {
            painter.rect_filled(cell, 0.0, DARK.panel_raised);
        }
        painter.text(
            cell.center(),
            egui::Align2::CENTER_CENTER,
            *label,
            egui::FontId::proportional(METRICS.segment_font),
            if active { DARK.text_primary } else { DARK.text_secondary },
        );
        if index > 0 {
            part(&painter, cell.left(), rect);
        }

        if response.clicked() {
            clicked = Some(*value);
        }
    }

    enclose(&painter, rect);
    clicked
}

/// A value the operator steps or drags. `capped_to` is what the consumer will actually honour —
/// shown beside the value when it is lower, so a clamped choice is never silently ignored.
pub(crate) struct StepperSpec<'a> {
    pub label: &'a str,
    pub value: usize,
    pub min: usize,
    pub max: usize,
    pub capped_to: usize,
}

/// Three flush cells: step down, a draggable track, step up. Returns the newly chosen value.
pub(crate) fn paint_stepper(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id_salt: &str,
    spec: StepperSpec<'_>,
) -> Option<usize> {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, DARK.panel);

    let step_width = rect.height();
    let track = egui::Rect::from_min_max(
        egui::pos2(rect.left() + step_width, rect.top()),
        egui::pos2(rect.right() - step_width, rect.bottom()),
    );
    let down = egui::Rect::from_min_max(rect.min, egui::pos2(track.left(), rect.bottom()));
    let up = egui::Rect::from_min_max(egui::pos2(track.right(), rect.top()), rect.max);

    let dragged = paint_track(ui, &painter, track, id_salt, &spec);
    // Both step cells paint every frame: short-circuiting on the first click would leave the other
    // unpainted and un-hovered for that frame.
    let down_clicked = paint_step(ui, &painter, down, (id_salt, "down"), "-");
    let up_clicked = paint_step(ui, &painter, up, (id_salt, "up"), "+");
    let stepped = match (down_clicked, up_clicked) {
        (true, _) => Some(spec.value.saturating_sub(1)),
        (_, true) => Some(spec.value + 1),
        _ => None,
    };

    part(&painter, track.left(), rect);
    part(&painter, track.right(), rect);
    enclose(&painter, rect);

    stepped
        .or(dragged)
        .map(|chosen| chosen.clamp(spec.min, spec.max))
        .filter(|chosen| *chosen != spec.value)
}

fn paint_track(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    track: egui::Rect,
    id_salt: &str,
    spec: &StepperSpec<'_>,
) -> Option<usize> {
    let response = ui.interact(
        track,
        egui::Id::new((id_salt, "track")),
        egui::Sense::click_and_drag(),
    );
    if response.hovered() {
        painter.rect_filled(track, 0.0, DARK.panel_raised);
    }

    let span = spec.max.saturating_sub(spec.min).max(1) as f32;
    let filled = (spec.value.saturating_sub(spec.min) as f32 / span).clamp(0.0, 1.0);
    let grown = egui::Rect::from_min_max(
        track.min,
        egui::pos2(track.left() + track.width() * filled, track.bottom()),
    );
    fill_selected(painter, grown);

    paint_readout(painter, track, spec);

    let pointer = response.interact_pointer_pos()?;
    let along = ((pointer.x - track.left()) / track.width().max(1.0)).clamp(0.0, 1.0);
    Some(spec.min + (along * span).round() as usize)
}

/// Value right, label left. The clamp reads as `50 (34)`: what was asked for, then what the consumer
/// can actually give. The value is the load-bearing half, so a track too narrow for both drops the
/// label rather than painting them through each other.
fn paint_readout(painter: &egui::Painter, track: egui::Rect, spec: &StepperSpec<'_>) {
    let font = egui::FontId::proportional(METRICS.segment_font);
    let value = painter.layout_no_wrap(spec.value.to_string(), font.clone(), DARK.text_primary);
    let label = painter.layout_no_wrap(spec.label.to_owned(), font, DARK.text_secondary);

    let pad = METRICS.space_1;
    let mut left = track.right() - pad - value.size().x;
    if spec.capped_to < spec.value {
        let capped = painter.layout_no_wrap(
            format!("({})", spec.capped_to),
            egui::FontId::proportional(METRICS.segment_font - 2.0),
            DARK.warning,
        );
        left -= capped.size().x + pad;
        let top = track.center().y - capped.size().y / 2.0;
        painter.galley(
            egui::pos2(track.right() - pad - capped.size().x, top),
            capped,
            DARK.warning,
        );
    }

    if track.left() + pad + label.size().x + pad < left {
        let top = track.center().y - label.size().y / 2.0;
        painter.galley(
            egui::pos2(track.left() + pad, top),
            label,
            DARK.text_secondary,
        );
    }
    let top = track.center().y - value.size().y / 2.0;
    painter.galley(egui::pos2(left, top), value, DARK.text_primary);
}

fn paint_step(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    cell: egui::Rect,
    id_salt: (&str, &str),
    glyph: &str,
) -> bool {
    let response = ui.interact(cell, egui::Id::new(id_salt), egui::Sense::click());
    if response.hovered() {
        painter.rect_filled(cell, 0.0, DARK.panel_raised);
    }
    painter.text(
        cell.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::proportional(METRICS.segment_font),
        DARK.text_primary,
    );
    response.clicked()
}

/// Selection is a fill plus a bar along the cell's bottom edge — the shell's one selected idiom.
fn fill_selected(painter: &egui::Painter, cell: egui::Rect) {
    painter.rect_filled(cell, 0.0, DARK.selected.gamma_multiply(0.28));
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(cell.left(), cell.bottom() - METRICS.accent_thickness),
            cell.max,
        ),
        0.0,
        DARK.selected,
    );
}

fn part(painter: &egui::Painter, x: f32, rect: egui::Rect) {
    painter.vline(
        theme::crisp(painter, x),
        rect.y_range(),
        theme::hairline(painter, DARK.border),
    );
}

fn enclose(painter: &egui::Painter, rect: egui::Rect) {
    painter.rect_stroke(
        rect,
        0.0,
        theme::hairline(painter, DARK.border),
        egui::StrokeKind::Inside,
    );
}
