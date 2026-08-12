//! Design system: semantic color + metric tokens (avoid inline values).

use eframe::egui;

use crate::msg::exec::RejectClass;
use crate::time::DurationUs;

/// Semantic palette: field names describe role (not hue), design system decides value. Gamma-space sRGBA.
#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub canvas: egui::Color32,
    pub panel: egui::Color32,
    pub panel_raised: egui::Color32,
    pub border: egui::Color32,
    pub grid: egui::Color32,
    pub text_primary: egui::Color32,
    pub text_secondary: egui::Color32,
    pub bid: egui::Color32,
    pub ask: egui::Color32,
    /// Behind the halt banner's text, not behind an ask: a red wash the invalid colour reads on.
    pub halt_fill: egui::Color32,
    /// SELL fill: orange (distinct from ask red), buy fills reuse bid green.
    pub sell_fill: egui::Color32,
    pub positive: egui::Color32,
    pub negative: egui::Color32,
    pub warning: egui::Color32,
    pub invalid: egui::Color32,
    pub stale: egui::Color32,
    pub selected: egui::Color32,
    pub focus: egui::Color32,
}

pub(crate) const DARK: Palette = Palette {
    canvas: egui::Color32::from_rgb(10, 13, 18),
    panel: egui::Color32::from_rgb(15, 19, 26),
    panel_raised: egui::Color32::from_rgb(21, 27, 36),
    border: egui::Color32::from_rgb(44, 53, 67),
    grid: egui::Color32::from_rgb(31, 39, 50),
    text_primary: egui::Color32::from_rgb(229, 234, 241),
    text_secondary: egui::Color32::from_rgb(145, 157, 174),
    bid: egui::Color32::from_rgb(64, 205, 150),
    ask: egui::Color32::from_rgb(244, 101, 118),
    halt_fill: egui::Color32::from_rgba_unmultiplied_const(190, 54, 74, 58),
    sell_fill: egui::Color32::from_rgb(233, 138, 45),
    positive: egui::Color32::from_rgb(72, 211, 153),
    negative: egui::Color32::from_rgb(255, 105, 120),
    warning: egui::Color32::from_rgb(245, 184, 72),
    invalid: egui::Color32::from_rgb(255, 92, 92),
    stale: egui::Color32::from_rgb(221, 165, 66),
    selected: egui::Color32::from_rgb(76, 139, 245),
    focus: egui::Color32::from_rgb(126, 178, 255),
};

/// Layout metrics in logical points; authored alongside palette for unified scale.
#[derive(Clone, Copy)]
pub(crate) struct Metrics {
    pub space_1: f32,
    pub space_2: f32,
    pub space_3: f32,
    pub toolbar_height: f32,
    pub link_bar_height: f32,
    pub corner_radius: f32,
    /// Selected state is a solid bar on one container edge — shared by toolbar, DOM header, tabs.
    pub accent_thickness: f32,
    pub segment_font: f32,
    pub dom_header_height: f32,
    pub dom_separator_fraction: f32,
    pub dom_row_height: f32,
    pub dom_row_height_floor: f32,
    pub dom_min_font: f32,
    pub dom_cell_pad: f32,
    pub dom_price_font: f32,
    pub dom_qty_font: f32,
    pub dom_mid_font: f32,
    pub dom_tag_font: f32,
    pub monitor_summary_height: f32,
    pub monitor_tab_height: f32,
    pub monitor_row_height: f32,
    pub monitor_feature_fraction: f32,
    /// The mid's share of the summary band; the two deltas split the rest. 0.40 is forced, not
    /// chosen — at the minimum window it is the only split where neither cell's widest realistic
    /// value overlaps a separator.
    pub monitor_mid_fraction: f32,
    pub monitor_mid_font: f32,
    pub monitor_delta_font: f32,
    pub monitor_micro_font: f32,
    pub monitor_feature_font: f32,
    pub monitor_channel_font: f32,
    pub monitor_tab_font: f32,
    pub monitor_badge_font: f32,
    pub monitor_value_col_w: f32,
    pub monitor_time_col_w: f32,
    pub monitor_qty_col_w: f32,
    pub monitor_intent_tag_w: f32,
    pub monitor_asset_label_w: f32,
    pub monitor_fill_tag_w: f32,
    /// The latency grid's share of the left panel below the toolbar, sized against the exposure
    /// chart it sits above rather than picked. That chart gets 30% of what this grid LEAVES, less
    /// the stack's sub-header, so 0.22 is the share holding the two within 3-18% of each other
    /// across the window's size range: a peer of the exposure chart, not competition for the mid.
    pub latency_fraction: f32,
    pub latency_label_col_w: f32,
    pub latency_header_font: f32,
    pub latency_row_font: f32,
    pub latency_value_font: f32,
    pub chart_pad: f32,
    pub chart_line_width: f32,
    pub chart_wick_width: f32,
    pub chart_min_body_width: f32,
    pub chart_body_gap: f32,
    pub chart_marker_radius: f32,
    pub chart_marker_outline: f32,
    pub chart_note_font: f32,
    pub chart_caption_font: f32,
    pub chart_axis_gutter: f32,
    pub chart_axis_font: f32,
}

pub(crate) const METRICS: Metrics = Metrics {
    space_1: 4.0,
    space_2: 8.0,
    space_3: 12.0,
    toolbar_height: 40.0,
    link_bar_height: 24.0,
    corner_radius: 3.0,
    accent_thickness: 2.0,
    segment_font: 12.0,
    // Control strip (unit toggle + grouping) under 40pt toolbar.
    dom_header_height: 30.0,
    dom_separator_fraction: 0.08,
    // Reference row: fonts are full size here and shrink proportionally below it, down to the
    // floor that clamps how many levels the operator can ask for.
    dom_row_height: 18.0,
    dom_row_height_floor: 8.5,
    dom_min_font: 7.0,
    dom_cell_pad: 8.0,
    dom_price_font: 13.0,
    dom_qty_font: 13.0,
    dom_mid_font: 16.0,
    dom_tag_font: 9.0,
    // Monitor: quote summary + tabs (fixed), features + channel share remainder at 35:47 ratio.
    monitor_summary_height: 56.0,
    monitor_tab_height: 30.0,
    monitor_row_height: 18.0,
    monitor_feature_fraction: 0.43,
    monitor_mid_fraction: 0.40,
    monitor_mid_font: 18.0,
    monitor_delta_font: 15.0,
    monitor_micro_font: 9.0,
    monitor_feature_font: 12.0,
    monitor_channel_font: 11.0,
    monitor_tab_font: 12.0,
    monitor_badge_font: 10.0,
    monitor_value_col_w: 108.0,
    monitor_time_col_w: 84.0,
    monitor_qty_col_w: 72.0,
    monitor_intent_tag_w: 42.0,
    monitor_asset_label_w: 56.0,
    monitor_fill_tag_w: 30.0,
    latency_fraction: 0.22,
    latency_label_col_w: 112.0,
    latency_header_font: 10.0,
    latency_row_font: 11.0,
    latency_value_font: 12.0,
    chart_pad: 10.0,
    chart_line_width: 1.25,
    chart_wick_width: 1.0,
    chart_min_body_width: 1.0,
    chart_body_gap: 1.0,
    chart_marker_radius: 3.0,
    chart_marker_outline: 1.0,
    chart_note_font: 13.0,
    chart_caption_font: 10.0,
    chart_axis_gutter: 72.0,
    chart_axis_font: 11.0,
};

/// DOM stale threshold (feed stall indicator).
pub(crate) const DOM_STALE_THRESHOLD: DurationUs = DurationUs::from_millis(2_000);

/// How alarming a venue refusal is, stated once. The account band and the Orders channel report the
/// same rejection at the same moment, so a severity that lived in two tables could disagree on screen.
/// A post-only cross is an ordinary cost of quoting as maker, and `Gone` is the reconciler working.
pub(crate) fn reject_color(class: RejectClass) -> egui::Color32 {
    match class {
        RejectClass::Refused | RejectClass::Gone => DARK.text_secondary,
        RejectClass::StillLive | RejectClass::Ambiguous => DARK.warning,
        RejectClass::Fatal => DARK.invalid,
    }
}

/// One physical pixel in logical points.
pub(crate) fn one_physical_pixel(painter: &egui::Painter) -> f32 {
    1.0 / painter.pixels_per_point()
}

/// Hairline stroke (one physical pixel).
pub(crate) fn hairline(painter: &egui::Painter, color: egui::Color32) -> egui::Stroke {
    egui::Stroke::new(one_physical_pixel(painter), color)
}

/// Snap to pixel centre (hairline sharpness).
pub(crate) fn crisp(painter: &egui::Painter, value: f32) -> f32 {
    painter.round_to_pixel_center(value)
}

/// Snap a region's OWN bottom edge to the last pixel row inside it. Plain [`crisp`] lands a
/// boundary on the row beyond the edge, which the scissor of a painter clipped to that region
/// discards — the separator would then draw or vanish depending on the window's height.
pub(crate) fn crisp_bottom_edge(painter: &egui::Painter, bottom: f32) -> f32 {
    crisp(painter, bottom - one_physical_pixel(painter))
}

/// Clear color (avoid grey flash).
pub(crate) fn clear_color() -> [f32; 4] {
    DARK.canvas.to_normalized_gamma_f32()
}

/// Install design system into egui context.
pub(crate) fn install_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.visuals.override_text_color = Some(DARK.text_primary);
        style.visuals.panel_fill = DARK.panel;
        style.visuals.window_fill = DARK.panel_raised;
        style.visuals.extreme_bg_color = DARK.canvas;
        style.spacing.item_spacing = egui::vec2(METRICS.space_2, METRICS.space_1);
        style.animation_time = 0.10;
    });
}
