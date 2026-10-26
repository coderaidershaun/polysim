//! The center panel's bands: the status strip, then the account, quote summary, features, channel
//! tabs and the active channel. The strip never scrolls. Below it the bands scroll when the pane is
//! too short to show them whole, and the caller owns the tab, scroll and pin state.

use eframe::egui;
use egui::emath::GuiRounding as _;

use super::pin::Pins;
use super::sizes::{self, FEATURE_ROW_SPACING};
use super::{account, channels, summary};
use crate::msg::persist::FeatureId;
use crate::workstation::browser::theme::{self, DARK, METRICS};
use crate::workstation::monitor::fit::{BandHeights, fit_bands};
use crate::workstation::monitor::view::{
    FeatureRowView, MonitorDetail, MonitorFrame, MonitorUiState, feature_rows,
};
use crate::workstation::text::{feature_hover, feature_name, write_feature_cell};

/// Paints the bands and applies their clicks and taps to `state`.
pub fn paint(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    frame: &MonitorFrame<'_>,
    state: &mut MonitorUiState,
) {
    let sizes = sizes::band_sizes();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, DARK.panel);

    let strip = rect.with_max_y((rect.top() + sizes.status).min(rect.bottom()));
    painter.rect_filled(strip, 0.0, DARK.panel_raised);
    account::paint_status(ui, strip, frame);

    // Round the viewport the way the scroll area rounds its own, so a fractional pane edge cannot
    // leave the content a hair taller than the viewport and show a scroll bar on a pane that fits.
    let viewport = rect.with_min_y(strip.bottom()).round_ui();
    let heights = fit_bands(
        viewport.height() + sizes.status,
        frame.model.monitor().feature_count(),
        &sizes,
    );
    let mut pins = Pins::new(rect, state);
    let builder = egui::UiBuilder::new()
        .max_rect(viewport)
        .layout(egui::Layout::top_down(egui::Align::Min));
    ui.scope_builder(builder, |ui| {
        // One scroll area in both modes, so the lists below keep their ids and scroll positions
        // when the pane crosses the height at which the bands stop fitting.
        egui::ScrollArea::vertical()
            .id_salt("monitor-bands")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // When the bands fit, the content is exactly as tall as the viewport, so nothing
                // scrolls.
                let height =
                    if heights.is_scrolled { heights.content_height() } else { viewport.height() };
                let (content, _) = ui.allocate_exact_size(
                    egui::vec2(viewport.width(), height),
                    egui::Sense::hover(),
                );
                let bands = Bands::new(content, &heights);
                bands.paint(ui, frame, state, &mut pins);
            });
    });

    theme::rules_h(&painter, [strip.bottom()], rect.x_range());
    pins.resolve(ui, state);
}

/// The bands under the status strip, top to bottom, cut from one content rect.
struct Bands {
    content: egui::Rect,
    account: egui::Rect,
    summary: egui::Rect,
    features: egui::Rect,
    tabs: egui::Rect,
    channel: egui::Rect,
}

impl Bands {
    fn new(content: egui::Rect, heights: &BandHeights) -> Self {
        let band = |top: f32, height: f32| {
            egui::Rect::from_min_size(
                egui::pos2(content.left(), top),
                egui::vec2(content.width(), height),
            )
        };
        let account = band(content.top(), heights.account);
        let summary = band(account.bottom(), heights.summary);
        let features = band(summary.bottom(), heights.features);
        let tabs = band(features.bottom(), heights.tabs);
        Self {
            content,
            account,
            summary,
            features,
            tabs,
            // The channel runs to the bottom of the content, so rounding never leaves a gap under
            // the last band.
            channel: content.with_min_y(tabs.bottom()),
        }
    }

    fn paint(
        &self,
        ui: &mut egui::Ui,
        frame: &MonitorFrame<'_>,
        state: &mut MonitorUiState,
        pins: &mut Pins,
    ) {
        let painter = ui.painter().clone();
        for raised in [self.account, self.summary, self.tabs] {
            painter.rect_filled(raised, 0.0, DARK.panel_raised);
        }

        account::paint(ui, self.account, frame);
        summary::paint(ui, self.summary, frame, pins);
        paint_feature_list(ui, self.features, frame, pins);
        channels::paint_tabs(ui, self.tabs, state);
        channels::paint_channel(ui, self.channel, frame, state, pins);

        let seams = [
            self.account.bottom(),
            self.summary.bottom(),
            self.features.bottom(),
            self.tabs.bottom(),
        ];
        theme::rules_h(&painter, seams, self.content.x_range());
    }
}

/// One scrolling band of rows, each with a record that shows on hover and pins on a tap, and a note
/// when there are none. `forced_offset` scrolls once, for a channel that starts following again.
/// The final scroll offset comes back, so the caller can tell whether the list still shows the
/// newest row.
///
/// Rows arrive by index rather than as a slice, so a band that shows twenty rows of a 256-row
/// history builds twenty, not the whole history every frame.
pub(super) struct RowList<'a, S> {
    pub rect: egui::Rect,
    pub pins: &'a mut Pins,
    pub salt: S,
    pub row_count: usize,
    pub empty_note: &'a str,
    pub forced_offset: Option<f32>,
    pub row_spacing: f32,
}

pub(super) fn paint_row_list<S: egui::AsIdSalt>(
    ui: &mut egui::Ui,
    list: RowList<'_, S>,
    detail_of: impl Fn(usize) -> MonitorDetail,
    mut paint_row: impl FnMut(&egui::Painter, egui::Rect, usize),
    mut record_of: impl FnMut(usize) -> String,
) -> f32 {
    let row_h = METRICS.monitor_row_height;
    let RowList {
        rect,
        pins,
        salt,
        row_count,
        empty_note,
        forced_offset,
        row_spacing,
    } = list;
    let builder = egui::UiBuilder::new()
        .max_rect(rect)
        .layout(egui::Layout::top_down(egui::Align::Min));
    ui.scope_builder(builder, |ui| {
        if row_count == 0 {
            paint_empty_note(&ui.painter_at(rect), rect, empty_note);
            return 0.0;
        }
        ui.spacing_mut().item_spacing.y = row_spacing;
        let mut area = egui::ScrollArea::vertical()
            .id_salt(salt)
            .auto_shrink([false, false])
            // Without this a one-row list would still claim a tall area and overhang the band below.
            .min_scrolled_height(0.0);
        if let Some(offset) = forced_offset {
            area = area.vertical_scroll_offset(offset);
        }
        let output = area.show_rows(ui, row_h, row_count, |ui, range| {
            let width = ui.available_width();
            for index in range {
                let (_, row_rect) = ui.allocate_space(egui::vec2(width, row_h));
                pins.interact(ui, row_rect, detail_of(index), || record_of(index));
                paint_row(ui.painter(), row_rect, index);
            }
        });
        output.state.offset.y
    })
    .inner
}

/// The feature list scrolls on its own. A long name is cut with an ellipsis rather than running
/// into its value.
fn paint_feature_list(
    ui: &mut egui::Ui,
    band: egui::Rect,
    frame: &MonitorFrame<'_>,
    pins: &mut Pins,
) {
    let names = frame.feature_names;
    let mut scratch = String::new();
    let row_at = |index: usize| feature_rows(frame.model, frame.instrument).nth(index);
    paint_row_list(
        ui,
        RowList {
            rect: sizes::content_column(band),
            pins,
            salt: "monitor-features",
            row_count: frame.model.monitor().feature_count(),
            empty_note: "no features yet",
            forced_offset: None,
            row_spacing: FEATURE_ROW_SPACING,
        },
        |index| MonitorDetail::Feature(FeatureId(index as u16)),
        |painter, row_rect, index| {
            if let Some(row) = row_at(index) {
                paint_feature_row(painter, row_rect, &row, names, &mut scratch);
            }
        },
        |index| row_at(index).map_or_else(String::new, |row| feature_hover(&row, names)),
    );
}

fn paint_feature_row(
    painter: &egui::Painter,
    rect: egui::Rect,
    row: &FeatureRowView,
    names: &[Box<str>],
    scratch: &mut String,
) {
    if row.changed {
        // A thin bar on the left marks a change without flashing the whole row.
        let width = METRICS.accent_thickness;
        let bar = egui::Rect::from_min_max(
            egui::pos2(rect.left(), rect.top() + width),
            egui::pos2(rect.left() + width, rect.bottom() - width),
        );
        painter.rect_filled(bar, 0.0, DARK.selected);
    }

    let (name_color, value_color) = if row.stale {
        (
            DARK.text_secondary.gamma_multiply(0.65),
            DARK.text_secondary,
        )
    } else {
        (DARK.text_secondary, DARK.text_primary)
    };

    // Room for the widest value, a long negative with four decimals, so a name never runs into it.
    let value_col_w = METRICS.monitor_value_col_w;
    let pad = METRICS.space_2;
    let name_rect = rect
        .with_min_x(rect.left() + pad)
        .with_max_x(rect.right() - value_col_w - pad);
    paint_ellipsized(
        painter,
        name_rect,
        feature_name(row, names),
        egui::FontId::proportional(METRICS.monitor_feature_font),
        name_color,
    );

    write_feature_cell(scratch, row);
    painter.text(
        egui::pos2(rect.right() - pad, theme::crisp(painter, rect.center().y)),
        egui::Align2::RIGHT_CENTER,
        scratch.as_str(),
        egui::FontId::monospace(METRICS.monitor_feature_font),
        if row.value.is_some() { value_color } else { DARK.text_secondary },
    );
}

/// Paints `text` left-aligned in `rect`, cut with an ellipsis at its right end when it does not
/// fit, and returns the width it took.
pub(super) fn paint_ellipsized(
    painter: &egui::Painter,
    rect: egui::Rect,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
) -> f32 {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::text::TextFormat {
            font_id: font,
            color,
            ..Default::default()
        },
    );
    job.halign = egui::Align::LEFT;
    job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width().max(0.0));
    let galley = painter.layout_job(job);
    let width = galley.size().x;
    let pos = egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0);
    painter.galley(pos, galley, color);
    width
}

/// An empty band says so in a muted note, rather than showing a made-up row.
pub(super) fn paint_empty_note(painter: &egui::Painter, rect: egui::Rect, note: &str) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        note,
        egui::FontId::proportional(METRICS.monitor_channel_font),
        DARK.text_secondary,
    );
}
