//! The center panel's five bands: account, quote summary, features, channel tabs and the active
//! channel. The account band has a fixed height so it is always in view, and the caller owns the tab
//! and scroll state.

use eframe::egui;

use super::account;
use super::channels;
use super::summary;
use crate::workstation::browser::theme::{self, DARK, METRICS};
use crate::workstation::monitor::view::{
    FeatureRowView, MonitorFrame, MonitorUiState, feature_rows,
};
use crate::workstation::text::{feature_hover, feature_name, write_feature_cell};

/// The five bands, top to bottom. The account, summary and tab bands have fixed heights, and the
/// feature list and the channel split the rest.
struct MonitorBands {
    account: egui::Rect,
    summary: egui::Rect,
    features: egui::Rect,
    tabs: egui::Rect,
    channel: egui::Rect,
}

impl MonitorBands {
    fn new(rect: egui::Rect) -> Self {
        let account_h = account::height();
        let summary_h = METRICS.monitor_summary_height;
        let tabs_h = METRICS.monitor_tab_height;
        let flexible = (rect.height() - account_h - summary_h - tabs_h).max(0.0);
        let feature_h = flexible * METRICS.monitor_feature_fraction;

        let account = band(rect, rect.top(), rect.top() + account_h);
        let summary = band(rect, account.bottom(), account.bottom() + summary_h);
        let features = band(rect, summary.bottom(), summary.bottom() + feature_h);
        let tabs = band(rect, features.bottom(), features.bottom() + tabs_h);
        let channel = band(rect, tabs.bottom(), rect.bottom());
        Self {
            account,
            summary,
            features,
            tabs,
            channel,
        }
    }
}

fn band(rect: egui::Rect, top: f32, bottom: f32) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(rect.left(), top),
        egui::pos2(rect.right(), bottom.max(top)),
    )
}

/// Paints the five bands and applies their clicks to `state`.
pub fn paint(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    frame: &MonitorFrame<'_>,
    state: &mut MonitorUiState,
) {
    let bands = MonitorBands::new(rect);
    ui.painter_at(rect).rect_filled(rect, 0.0, DARK.panel);

    account::paint(ui, bands.account, frame);
    summary::paint(ui, bands.summary, frame);
    paint_feature_list(ui, bands.features, frame);
    channels::paint_tabs(ui, bands.tabs, state);
    channels::paint_channel(ui, bands.channel, frame, state);

    let seams = [
        bands.account.bottom(),
        bands.summary.bottom(),
        bands.features.bottom(),
        bands.tabs.bottom(),
    ];
    theme::rules_h(&ui.painter_at(rect), seams, rect.x_range());
}

/// One scrolling band of rows, each with a hover record, and a note when there are none.
/// `forced_offset` scrolls once, for a channel that starts following again. The final scroll offset
/// comes back, so the caller can tell whether the list still shows the newest row.
///
/// Rows arrive by index rather than as a slice, so a band that shows twenty rows of a 256-row
/// history builds twenty, not the whole history every frame.
pub(super) struct RowList<'a, S> {
    pub salt: S,
    pub row_count: usize,
    pub empty_note: &'a str,
    pub forced_offset: Option<f32>,
    pub row_spacing: f32,
}

pub(super) fn paint_row_list<S: egui::AsIdSalt>(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    list: RowList<'_, S>,
    mut paint_row: impl FnMut(&egui::Painter, egui::Rect, usize),
    mut hover_text: impl FnMut(usize) -> String,
) -> f32 {
    let row_h = METRICS.monitor_row_height;
    let builder = egui::UiBuilder::new()
        .max_rect(rect)
        .layout(egui::Layout::top_down(egui::Align::Min));
    ui.scope_builder(builder, |ui| {
        if list.row_count == 0 {
            paint_empty_note(&ui.painter_at(rect), rect, list.empty_note);
            return 0.0;
        }
        ui.spacing_mut().item_spacing.y = list.row_spacing;
        let mut area = egui::ScrollArea::vertical()
            .id_salt(list.salt)
            .auto_shrink([false, false]);
        if let Some(offset) = list.forced_offset {
            area = area.vertical_scroll_offset(offset);
        }
        let output = area.show_rows(ui, row_h, list.row_count, |ui, range| {
            let width = ui.available_width();
            for index in range {
                let (row_rect, response) =
                    ui.allocate_exact_size(egui::vec2(width, row_h), egui::Sense::hover());
                paint_row(ui.painter(), row_rect, index);
                // The hover text is built only while the row is hovered, not every frame.
                response.on_hover_ui(|ui| {
                    ui.label(hover_text(index));
                });
            }
        });
        output.state.offset.y
    })
    .inner
}

/// The feature list scrolls on its own. A long name is cut with an ellipsis rather than running
/// into its value.
fn paint_feature_list(ui: &mut egui::Ui, rect: egui::Rect, frame: &MonitorFrame<'_>) {
    let names = frame.feature_names;
    let mut scratch = String::new();
    let row_spacing = ui.spacing().item_spacing.y;
    let row_at = |index: usize| feature_rows(frame.model, frame.instrument).nth(index);
    paint_row_list(
        ui,
        rect,
        RowList {
            salt: "monitor-features",
            row_count: frame.model.monitor().feature_count(),
            empty_note: "no features yet",
            forced_offset: None,
            row_spacing,
        },
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
    paint_left_ellipsized(
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

pub(super) fn paint_left_ellipsized(
    painter: &egui::Painter,
    rect: egui::Rect,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
) {
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
    let pos = egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0);
    painter.galley(pos, galley, color);
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
