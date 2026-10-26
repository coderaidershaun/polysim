//! The bars and rules that belong to the whole page rather than to one pane. The top bar keeps the
//! safety badges on screen at every width, and on a compact canvas the tab bar at the bottom takes
//! the place of the view switcher.

use eframe::egui;

use super::controls::{self, DropdownSpec};
use super::theme::{self, DARK, METRICS};
use crate::ids::InstrumentId;
use crate::msg::ui::UiCatalog;
use crate::workstation::format;
use crate::workstation::shell::{Area, Arrangement, Pane, Selection, View};
use crate::workstation::text::{self, HALT_BADGE, Tone};

const STRATEGY_FONT: f32 = 13.0;

impl From<Area> for egui::Rect {
    fn from(area: Area) -> Self {
        Self::from_min_size(
            egui::pos2(area.left, area.top),
            egui::vec2(area.width, area.height),
        )
    }
}

impl From<egui::Rect> for Area {
    fn from(rect: egui::Rect) -> Self {
        Self {
            left: rect.left(),
            top: rect.top(),
            width: rect.width(),
            height: rect.height(),
        }
    }
}

pub struct TopBarFrame<'a> {
    pub catalog: Option<&'a UiCatalog>,
    pub selected: InstrumentId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChromeAction {
    #[default]
    None,
    SelectInstrument(InstrumentId),
    Navigate(Selection),
}

pub fn paint_top_bar(
    ui: &mut egui::Ui,
    arrangement: &Arrangement,
    frame: &TopBarFrame<'_>,
) -> ChromeAction {
    let bar = arrangement.top_bar;
    let area = egui::Rect::from(bar.area);
    let painter = ui.painter_at(area);
    painter.rect_filled(area, 0.0, DARK.panel_raised);

    let mut action = ChromeAction::None;
    let picker = egui::Rect::from(bar.picker);
    if let Some(instrument) = paint_picker(ui, picker, frame) {
        action = ChromeAction::SelectInstrument(instrument);
    }
    theme::rules_v(&painter, [picker.right()], area.y_range());
    if let Some(slot) = bar.switcher {
        let slot = egui::Rect::from(slot);
        let entries = switcher_entries();
        let id_salt = "view-switcher";
        let chosen =
            controls::paint_segmented_toggle(ui, slot, id_salt, &entries, arrangement.shown);
        if let Some(selection) = chosen {
            action = ChromeAction::Navigate(selection);
        }
        theme::rules_v(&painter, [slot.right()], area.y_range());
    }
    if let (Some(slot), Some(catalog)) = (bar.strategy, frame.catalog) {
        paint_strategy_id(&painter, slot.into(), &catalog.strategy_id);
    }
    if let Some(slot) = bar.halt {
        let slot = egui::Rect::from(slot);
        Badge::of(HALT_BADGE).paint(&painter, slot);
        theme::rules_v(&painter, [slot.left()], area.y_range());
    }
    let mode = frame.catalog.and_then(|catalog| catalog.execution_mode);
    Badge::of(text::execution_badge(mode)).paint(&painter, bar.mode.into());

    painter.hline(
        area.x_range(),
        theme::crisp_bottom_edge(&painter, area.bottom()),
        theme::hairline(&painter, DARK.border),
    );
    action
}

pub fn paint_tab_bar(ui: &mut egui::Ui, arrangement: &Arrangement) -> ChromeAction {
    let Some(bar) = arrangement.tab_bar else {
        return ChromeAction::None;
    };
    let rect = egui::Rect::from(bar);
    let entries = switcher_entries();
    // The desk is first in the switcher's order and gets no tab, because a compact canvas has no
    // desk.
    let chosen =
        controls::paint_segmented_toggle(ui, rect, "view-tabs", &entries[1..], arrangement.shown);
    theme::rules_h(&ui.painter_at(rect), [rect.top()], rect.x_range());
    chosen.map_or(ChromeAction::None, ChromeAction::Navigate)
}

// SMELL: every caller must paint its panes between this and `paint_pane_rules`, and only the docs
// say so. One function that paints the frames, hands each pane back to the caller and then paints
// the rules would make the order impossible to get wrong.
/// Paints the panes' backgrounds and their empty header strips. Call it before painting the panes,
/// so their content lands on top.
pub fn paint_pane_frames(ui: &egui::Ui, arrangement: &Arrangement) {
    for pane in panes(arrangement) {
        ui.painter().rect_filled(pane_rect(pane), 0.0, DARK.panel);
        let Some(header) = pane.header else {
            continue;
        };
        let header = egui::Rect::from(header);
        let painter = ui.painter_at(header);
        painter.rect_filled(header, 0.0, DARK.panel_raised);
        painter.hline(
            header.x_range(),
            theme::crisp_bottom_edge(&painter, header.bottom()),
            theme::hairline(&painter, DARK.border),
        );
    }
}

/// Draws a rule between panes that touch and one down each outer side. Call it after painting the
/// panes, so no pane covers the rules. The bars above and below draw their own rules, so a full
/// outline would draw those lines twice.
pub fn paint_pane_rules(ui: &egui::Ui, arrangement: &Arrangement) {
    let painter = ui.painter();
    let rects = panes(arrangement).map(pane_rect);
    let Some(outline) = rects.clone().reduce(egui::Rect::union) else {
        return;
    };
    for rect in rects.filter(|rect| rect.left() > outline.left()) {
        theme::rules_v(painter, [rect.left()], rect.y_range());
    }
    let stroke = theme::hairline(painter, DARK.border);
    let half = stroke.width / 2.0;
    for x in [outline.left() + half, outline.right() - half] {
        painter.vline(x, outline.y_range(), stroke);
    }
}

fn panes(arrangement: &Arrangement) -> impl Iterator<Item = Pane> + Clone + '_ {
    [View::Chart, View::Monitor, View::Ladder]
        .into_iter()
        .filter_map(|view| arrangement.pane(view))
}

fn pane_rect(pane: Pane) -> egui::Rect {
    let body = egui::Rect::from(pane.body);
    pane.header.map_or(body, |header| body.union(header.into()))
}

fn switcher_entries() -> [(Selection, &'static str); 4] {
    Selection::ALL.map(|selection| (selection, text::selection_label(selection)))
}

fn paint_picker(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    frame: &TopBarFrame<'_>,
) -> Option<InstrumentId> {
    let instruments = frame
        .catalog
        .map_or(&[][..], |catalog| catalog.instruments.as_slice());
    let selected_text = frame
        .catalog
        .and_then(|catalog| catalog.instrument(frame.selected))
        .map_or(format::MISSING, |instrument| instrument.display.as_ref());
    let mut chosen = frame.selected;
    let spec = DropdownSpec {
        rect,
        id_salt: "polysim-instrument",
        selected_text,
        row_count: instruments.len(),
    };
    controls::paint_dropdown(ui, spec, |ui| {
        for instrument in instruments {
            ui.selectable_value(
                &mut chosen,
                instrument.instrument_id,
                instrument.display.as_ref(),
            );
        }
    });
    (chosen != frame.selected).then_some(chosen)
}

/// A long id is cut to its slot, so it gives way to the badges instead of running under them.
fn paint_strategy_id(painter: &egui::Painter, slot: egui::Rect, strategy_id: &str) {
    let room = (slot.width() - METRICS.space_3 - METRICS.space_2).max(0.0);
    let mut job = egui::text::LayoutJob::simple_singleline(
        strategy_id.to_owned(),
        egui::FontId::proportional(STRATEGY_FONT),
        DARK.text_secondary,
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(room);
    let galley = painter.layout_job(job);
    let top = slot.center().y - galley.size().y / 2.0;
    painter.galley(
        egui::pos2(slot.left() + METRICS.space_3, top),
        galley,
        DARK.text_secondary,
    );
}

struct Badge {
    word: &'static str,
    fill: egui::Color32,
    text: egui::Color32,
}

impl Badge {
    /// A solid cell in the role's colour, so a safety state reads at a glance. Only the neutral
    /// role stays grey.
    fn of((word, tone): (&'static str, Tone)) -> Self {
        let (fill, text) = match tone {
            Tone::Text => (DARK.border, DARK.text_primary),
            other => (DARK.tone(other), DARK.canvas),
        };
        Self { word, fill, text }
    }

    fn paint(&self, painter: &egui::Painter, rect: egui::Rect) {
        painter.rect_filled(rect, 0.0, self.fill);
        painter.text(
            egui::pos2(rect.center().x, theme::crisp(painter, rect.center().y)),
            egui::Align2::CENTER_CENTER,
            self.word,
            egui::FontId::proportional(METRICS.segment_font),
            self.text,
        );
    }
}
