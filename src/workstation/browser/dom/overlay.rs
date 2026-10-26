//! What the ladder paints over its rows: the chevrons for orders off screen, the hover readout and
//! the status badge. None of it ever reorders the rows.

use eframe::egui;

use super::ladder::{DomFrame, DomLayout};
use crate::workstation::browser::theme::{DARK, METRICS};
use crate::workstation::dom_view::{DomRow, DomView, QuotePlacement};
use crate::workstation::format;
use crate::workstation::text;

pub(crate) fn paint_offscreen(painter: &egui::Painter, layout: &DomLayout, view: &DomView) {
    let ask = text::offscreen_of(view.ask_order_placement, view.ask_placement);
    let bid = text::offscreen_of(view.bid_order_placement, view.bid_placement);
    for placement in [ask, bid] {
        match placement {
            QuotePlacement::OffScreenAbove { delta_half_ticks } => {
                chevron(painter, layout, Direction::Up, delta_half_ticks);
            }
            QuotePlacement::OffScreenBelow { delta_half_ticks } => {
                chevron(painter, layout, Direction::Down, delta_half_ticks);
            }
            QuotePlacement::Visible | QuotePlacement::None => {}
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Up,
    Down,
}

fn chevron(
    painter: &egui::Painter,
    layout: &DomLayout,
    direction: Direction,
    delta_half_ticks: i64,
) {
    // The triangle beside the number is what says "delta": every `format::write_*` opens by clearing
    // the buffer, so a prefix written here would be discarded.
    let label = format::render(|out| format::write_mid(out, delta_half_ticks));
    let galley = painter.layout_no_wrap(
        label,
        egui::FontId::proportional(METRICS.dom_tag_font + 1.0),
        DARK.focus,
    );

    let arrow = 9.0;
    let pad = METRICS.dom_cell_pad;
    let width = arrow + METRICS.space_1 + galley.size().x + 2.0 * pad;
    let height = layout.row_height.clamp(16.0, 22.0);
    let center_x = layout.rect.center().x;
    let center_y = match direction {
        Direction::Up => layout.rect.top() + height / 2.0 + METRICS.space_1,
        Direction::Down => layout.rect.bottom() - height / 2.0 - METRICS.space_1,
    };
    let pill =
        egui::Rect::from_center_size(egui::pos2(center_x, center_y), egui::vec2(width, height));
    fill_pill(painter, pill, DARK.focus);

    let arrow_center = egui::pos2(pill.left() + pad + arrow / 2.0, pill.center().y);
    painter.add(triangle(arrow_center, arrow, direction, DARK.selected));
    painter.galley(
        egui::pos2(
            arrow_center.x + arrow / 2.0 + METRICS.space_1,
            pill.center().y - galley.size().y / 2.0,
        ),
        galley,
        DARK.focus,
    );
}

/// Painted as a shape rather than set as a glyph: the bundled default fonts have no face for the
/// arrows and triangles this would otherwise be written with.
pub(crate) fn triangle(
    center: egui::Pos2,
    size: f32,
    direction: Direction,
    color: egui::Color32,
) -> egui::Shape {
    let half = size / 2.0;
    let points = match direction {
        Direction::Up => vec![
            egui::pos2(center.x, center.y - half),
            egui::pos2(center.x - half, center.y + half),
            egui::pos2(center.x + half, center.y + half),
        ],
        Direction::Down => vec![
            egui::pos2(center.x, center.y + half),
            egui::pos2(center.x - half, center.y - half),
            egui::pos2(center.x + half, center.y - half),
        ],
    };
    egui::Shape::convex_polygon(points, color, egui::Stroke::NONE)
}

pub(crate) fn paint_hover(
    painter: &egui::Painter,
    layout: &DomLayout,
    view: &DomView,
    frame: &DomFrame<'_>,
    pointer: Option<egui::Pos2>,
) {
    let Some(pointer) = pointer else { return };
    let Some((row_rect, detail)) = hovered_row(layout, view, frame, pointer) else {
        return;
    };
    painter.rect_filled(row_rect, 0.0, DARK.selected.gamma_multiply(0.12));
    readout(painter, layout, pointer, &detail);
}

struct RowDetail {
    price: String,
    public: String,
    order: String,
    desired: String,
}

fn hovered_row(
    layout: &DomLayout,
    view: &DomView,
    frame: &DomFrame<'_>,
    pointer: egui::Pos2,
) -> Option<(egui::Rect, RowDetail)> {
    if !layout.rect.contains(pointer) {
        return None;
    }
    if pointer.y < layout.separator.top() {
        let index = ((layout.separator.top() - pointer.y) / layout.row_height).floor() as usize;
        let row = view.ask_rows().get(index)?;
        return Some((layout.ask_row(index), row_detail(row, frame)));
    }
    if pointer.y > layout.separator.bottom() {
        let index = ((pointer.y - layout.separator.bottom()) / layout.row_height).floor() as usize;
        let row = view.bid_rows().get(index)?;
        return Some((layout.bid_row(index), row_detail(row, frame)));
    }
    None
}

fn row_detail(row: &DomRow, frame: &DomFrame<'_>) -> RowDetail {
    let price = format::render(|out| frame.write_row_price(out, row.tick_index));
    let public = format::render(|out| {
        format::write_opt_qty(out, row.public_qty, frame.qty_scale, frame.qty_decimals);
    });
    let mut order = format::render(|out| {
        format::write_opt_qty(out, row.order_qty, frame.qty_scale, frame.qty_decimals);
    });
    if let Some(status) = row.order_status {
        order.push_str("  ");
        order.push_str(status.word());
    }
    let desired = format::render(|out| {
        format::write_opt_qty(out, row.strategy_qty, frame.qty_scale, frame.qty_decimals);
    });
    RowDetail {
        price,
        public,
        order,
        desired,
    }
}

fn readout(painter: &egui::Painter, layout: &DomLayout, pointer: egui::Pos2, detail: &RowDetail) {
    let font = egui::FontId::monospace(METRICS.dom_tag_font + 2.0);
    let lines = [
        format!("px    {}", detail.price),
        format!("pub   {}", detail.public),
        format!("order {}", detail.order),
        format!("want  {}", detail.desired),
    ];
    let galleys: Vec<_> = lines
        .into_iter()
        .map(|line| painter.layout_no_wrap(line, font.clone(), DARK.text_primary))
        .collect();

    let pad = METRICS.dom_cell_pad;
    let line_h = galleys[0].size().y;
    let width = galleys.iter().map(|g| g.size().x).fold(0.0_f32, f32::max) + 2.0 * pad;
    let height = line_h * galleys.len() as f32 + 2.0 * pad;
    let anchor = egui::pos2(pointer.x + METRICS.space_2, pointer.y + METRICS.space_2);
    let clamped = clamp_rect(
        layout.rect,
        egui::Rect::from_min_size(anchor, egui::vec2(width, height)),
    );

    fill_pill(painter, clamped, DARK.border);
    for (index, galley) in galleys.into_iter().enumerate() {
        painter.galley(
            egui::pos2(
                clamped.left() + pad,
                clamped.top() + pad + index as f32 * line_h,
            ),
            galley,
            DARK.text_primary,
        );
    }
}

pub(crate) fn paint_status(
    painter: &egui::Painter,
    layout: &DomLayout,
    view: &DomView,
    frame: &DomFrame<'_>,
) {
    let Some((label, tone)) = text::status_pill(view.status, frame.stale_age) else {
        return;
    };
    let color = DARK.tone(tone);
    let galley = painter.layout_no_wrap(
        label,
        egui::FontId::proportional(METRICS.dom_qty_font),
        color,
    );
    let pad = METRICS.dom_cell_pad;
    let size = egui::vec2(galley.size().x + 2.0 * pad, galley.size().y + pad);
    let center = egui::pos2(layout.rect.center().x, layout.rect.top() + size.y);
    let pill = egui::Rect::from_center_size(center, size);
    fill_pill(painter, pill, color);
    painter.galley(pill.min + egui::vec2(pad, pad / 2.0), galley, color);
}

pub(crate) fn fill_pill(painter: &egui::Painter, rect: egui::Rect, border: egui::Color32) {
    painter.rect_filled(rect, METRICS.corner_radius, DARK.panel_raised);
    painter.rect_stroke(
        rect,
        METRICS.corner_radius,
        egui::Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );
}

pub(crate) fn clamp_rect(bounds: egui::Rect, rect: egui::Rect) -> egui::Rect {
    let mut min = rect.min;
    if rect.right() > bounds.right() {
        min.x = bounds.right() - rect.width();
    }
    if rect.bottom() > bounds.bottom() {
        min.y = bounds.bottom() - rect.height();
    }
    min.x = min.x.max(bounds.left());
    min.y = min.y.max(bounds.top());
    egui::Rect::from_min_size(min, rect.size())
}
