//! Custom-painted DOM ladder with 3-column mirror layout. Ask: strategy | price | public.
//! Bid: public | price | strategy. Strategy column: REAL orders or desired quotes, never merged.

use eframe::egui;

use crate::ids::Price;
use crate::msg::ui::UiBookSnapshot;

use super::dom_overlay;
use super::dom_view::{
    DomGrouping, DomOverlay, DomRow, DomStatus, DomView, DomViewInput, FeedStatus,
    MAX_ROWS_PER_SIDE, StrategyCell, build_dom_view, fit_rows, price_for_row,
};
use super::exec_model::OrderStatus;
use super::format;
use super::theme::{self, DARK, METRICS};
use crate::ids::Qty;
use crate::time::DurationUs;

/// Floor on the row-rule fade: below it the ladder loses its structure rather than its noise.
const MIN_GRID_FADE: f32 = 0.35;

pub struct DomFrame<'a> {
    pub snapshot: Option<&'a UiBookSnapshot>,
    pub overlay: DomOverlay<'a>,
    pub tick: Price,
    pub grouping: DomGrouping,
    /// Rows per side asked for; the panel clamps it to what fits above the legibility floor.
    pub levels: usize,
    pub price_decimals: usize,
    pub qty_scale: i64,
    pub qty_decimals: usize,
    pub feed: FeedStatus,
    /// How far the book trails the engine's own newest stamp, when that is far enough to say so.
    pub stale_age: Option<DurationUs>,
}

impl DomFrame<'_> {
    pub(crate) fn write_row_price(&self, buf: &mut String, tick_index: i64) {
        match price_for_row(self.tick, tick_index) {
            Some(price) => format::write_venue_price(buf, price, self.price_decimals),
            None => {
                format::write_missing(buf);
            }
        }
    }
}

pub(crate) struct DomLayout {
    pub rect: egui::Rect,
    pub separator: egui::Rect,
    pub column_width: f32,
    pub rows_per_side: usize,
    pub row_height: f32,
    fonts: RowFonts,
    /// The row against the design's reference row — everything sized per-row shrinks by it.
    scale: f32,
}

/// Row fonts shrink with the row so a dense ladder stays legible rather than overlapping. `tag` is
/// `None` once the status word would fall under the legible floor: colour and the quote ring still
/// carry status there, and the hover readout still spells it out in words.
struct RowFonts {
    price: f32,
    qty: f32,
    tag: Option<f32>,
}

impl RowFonts {
    fn at(scale: f32) -> Self {
        let scaled = |size: f32| (size * scale).max(METRICS.dom_min_font);
        Self {
            price: scaled(METRICS.dom_price_font),
            qty: scaled(METRICS.dom_qty_font),
            tag: Some(METRICS.dom_tag_font * scale).filter(|size| *size >= METRICS.dom_min_font),
        }
    }
}

fn separator_of(rect: egui::Rect) -> egui::Rect {
    let height = rect.height() * METRICS.dom_separator_fraction;
    let center_y = rect.center().y;
    egui::Rect::from_min_max(
        egui::pos2(rect.left(), center_y - height / 2.0),
        egui::pos2(rect.right(), center_y + height / 2.0),
    )
}

fn side_height(rect: egui::Rect) -> f32 {
    (separator_of(rect).top() - rect.top()).max(0.0)
}

/// Rows per side `body` can paint without going under the legibility floor. The header's level
/// control reports this when it is below the operator's choice, so a clamped ladder is visible
/// rather than silently short.
pub(crate) fn rows_that_fit(body: egui::Rect) -> usize {
    fit_rows(
        side_height(body),
        MAX_ROWS_PER_SIDE,
        METRICS.dom_row_height_floor,
    )
    .rows
}

impl DomLayout {
    fn new(rect: egui::Rect, levels: usize) -> Self {
        let fit = fit_rows(side_height(rect), levels, METRICS.dom_row_height_floor);
        let scale = (fit.row_height / METRICS.dom_row_height).min(1.0);
        Self {
            rect,
            separator: separator_of(rect),
            column_width: rect.width() / 3.0,
            rows_per_side: fit.rows,
            row_height: fit.row_height,
            fonts: RowFonts::at(scale),
            scale,
        }
    }

    pub(crate) fn ask_row(&self, index: usize) -> egui::Rect {
        let bottom = self.separator.top() - index as f32 * self.row_height;
        egui::Rect::from_min_max(
            egui::pos2(self.rect.left(), bottom - self.row_height),
            egui::pos2(self.rect.right(), bottom),
        )
    }

    pub(crate) fn bid_row(&self, index: usize) -> egui::Rect {
        let top = self.separator.bottom() + index as f32 * self.row_height;
        egui::Rect::from_min_max(
            egui::pos2(self.rect.left(), top),
            egui::pos2(self.rect.right(), top + self.row_height),
        )
    }

    pub(crate) fn cell(&self, row: egui::Rect, column: usize) -> egui::Rect {
        let left = self.rect.left() + column as f32 * self.column_width;
        egui::Rect::from_min_max(
            egui::pos2(left, row.top()),
            egui::pos2(left + self.column_width, row.bottom()),
        )
    }
}

pub fn paint(ui: &mut egui::Ui, rect: egui::Rect, frame: &DomFrame<'_>) -> egui::Response {
    let response = ui.interact(rect, egui::Id::new("polysim-dom"), egui::Sense::hover());
    let painter = ui.painter().with_clip_rect(rect);
    let layout = DomLayout::new(rect, frame.levels);
    let view = build_dom_view(DomViewInput {
        snapshot: frame.snapshot,
        overlay: frame.overlay,
        tick: frame.tick,
        grouping: frame.grouping,
        rows_per_side: layout.rows_per_side,
        feed: frame.feed,
    });

    painter.rect_filled(rect, 0.0, DARK.panel);
    paint_grid(&painter, &layout);
    let mut ladder = LadderPainter {
        painter: &painter,
        layout: &layout,
        frame,
        status: view.status,
        scratch: String::new(),
    };
    ladder.rows(&view);
    paint_quote_rings(&painter, &layout, &view);
    ladder.separator(&view);

    dom_overlay::paint_offscreen(&painter, &layout, &view);
    dom_overlay::paint_hover(&painter, &layout, &view, frame, response.hover_pos());
    dom_overlay::paint_status(&painter, &layout, &view, frame);

    response
}

fn paint_grid(painter: &egui::Painter, layout: &DomLayout) {
    let columns = theme::hairline(painter, DARK.border);
    for column in 1..3 {
        let x = layout.rect.left() + column as f32 * layout.column_width;
        painter.vline(theme::crisp(painter, x), layout.rect.y_range(), columns);
    }
    // A ladder at the height floor rules twice the lines one at the reference row does; at full
    // strength that many read as hatching, so the row rules fade as the rows compress.
    let rows = theme::hairline(
        painter,
        DARK.border.gamma_multiply(layout.scale.max(MIN_GRID_FADE)),
    );
    for index in 0..=layout.rows_per_side {
        let above = layout.separator.top() - index as f32 * layout.row_height;
        let below = layout.separator.bottom() + index as f32 * layout.row_height;
        painter.hline(layout.rect.x_range(), theme::crisp(painter, above), rows);
        painter.hline(layout.rect.x_range(), theme::crisp(painter, below), rows);
    }
}

/// Everything that writes a number into the ladder shares a painter, a geometry and ONE string
/// buffer, so these are methods rather than free functions each re-taking the lot.
struct LadderPainter<'a> {
    painter: &'a egui::Painter,
    layout: &'a DomLayout,
    frame: &'a DomFrame<'a>,
    status: DomStatus,
    scratch: String,
}

impl LadderPainter<'_> {
    fn rows(&mut self, view: &DomView) {
        for (index, row) in view.ask_rows().iter().enumerate() {
            let rect = self.layout.ask_row(index);
            let (public, strategy) = (self.layout.cell(rect, 2), self.layout.cell(rect, 0));
            self.price(rect, row.tick_index);
            self.public(public, row, at_status(DARK.ask, self.status), Edge::Right);
            self.strategy(strategy, row, Edge::Left);
        }
        for (index, row) in view.bid_rows().iter().enumerate() {
            let rect = self.layout.bid_row(index);
            let (public, strategy) = (self.layout.cell(rect, 0), self.layout.cell(rect, 2));
            self.price(rect, row.tick_index);
            self.public(public, row, at_status(DARK.bid, self.status), Edge::Left);
            self.strategy(strategy, row, Edge::Right);
        }
    }

    fn price(&mut self, row_rect: egui::Rect, tick_index: i64) {
        self.frame.write_row_price(&mut self.scratch, tick_index);
        let center = self.layout.cell(row_rect, 1).center();
        self.painter.text(
            egui::pos2(center.x, theme::crisp(self.painter, center.y)),
            egui::Align2::CENTER_CENTER,
            self.scratch.as_str(),
            egui::FontId::monospace(self.layout.fonts.price),
            at_status(DARK.text_primary, self.status),
        );
    }

    fn public(&mut self, cell: egui::Rect, row: &DomRow, color: egui::Color32, edge: Edge) {
        let Some(qty) = row.public_qty else { return };
        self.write_qty(qty);
        self.edge_qty(cell, color, edge);
    }

    fn strategy(&mut self, cell: egui::Rect, row: &DomRow, edge: Edge) {
        let (qty, word, color) = match row.strategy_cell() {
            Some(StrategyCell::Order { qty, status }) => (qty, status.word(), order_color(status)),
            Some(StrategyCell::Desired { qty }) => (qty, "want", DARK.selected.gamma_multiply(0.6)),
            None => return,
        };
        let color = at_status(color, self.status);
        self.write_qty(qty);
        self.edge_qty(cell, color, edge);
        let Some(size) = self.layout.fonts.tag else { return };
        // The tag sits at the cell's far edge, opposite its quantity.
        let (x, align) = edge.flip().anchor(cell);
        self.painter.text(
            egui::pos2(x, theme::crisp(self.painter, cell.center().y)),
            align,
            word,
            egui::FontId::proportional(size),
            color,
        );
    }

    fn separator(&mut self, view: &DomView) {
        let separator = self.layout.separator;
        self.painter.rect_filled(separator, 0.0, DARK.panel_raised);
        let border = egui::Stroke::new(2.0, DARK.focus);
        for y in [separator.top(), separator.bottom()] {
            self.painter
                .hline(separator.x_range(), theme::crisp(self.painter, y), border);
        }

        format::write_opt_venue_mid(&mut self.scratch, view.mid_half_ticks, self.frame.tick);
        let color = match view.mid_half_ticks {
            Some(_) => DARK.text_primary,
            None => DARK.invalid,
        };
        let center = separator.center();
        self.painter.text(
            egui::pos2(center.x, theme::crisp(self.painter, center.y)),
            egui::Align2::CENTER_CENTER,
            self.scratch.as_str(),
            egui::FontId::monospace(METRICS.dom_mid_font),
            color,
        );
        self.painter.text(
            egui::pos2(
                separator.left() + METRICS.dom_cell_pad,
                theme::crisp(self.painter, center.y),
            ),
            egui::Align2::LEFT_CENTER,
            "MID",
            egui::FontId::proportional(METRICS.dom_tag_font),
            DARK.text_secondary,
        );
    }

    fn write_qty(&mut self, qty: Qty) {
        format::write_qty(
            &mut self.scratch,
            qty,
            self.frame.qty_scale,
            self.frame.qty_decimals,
        );
    }

    fn edge_qty(&self, cell: egui::Rect, color: egui::Color32, edge: Edge) {
        let (x, align) = edge.anchor(cell);
        self.painter.text(
            egui::pos2(x, theme::crisp(self.painter, cell.center().y)),
            align,
            self.scratch.as_str(),
            egui::FontId::monospace(self.layout.fonts.qty),
            color,
        );
    }
}

#[derive(Clone, Copy)]
enum Edge {
    Left,
    Right,
}

impl Edge {
    fn flip(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }

    fn anchor(self, cell: egui::Rect) -> (f32, egui::Align2) {
        match self {
            Self::Left => (
                cell.left() + METRICS.dom_cell_pad,
                egui::Align2::LEFT_CENTER,
            ),
            Self::Right => (
                cell.right() - METRICS.dom_cell_pad,
                egui::Align2::RIGHT_CENTER,
            ),
        }
    }
}

fn order_color(status: OrderStatus) -> egui::Color32 {
    match status {
        OrderStatus::Confirmed => DARK.selected,
        OrderStatus::InFlight => DARK.warning,
        OrderStatus::Lost => DARK.invalid,
    }
}

fn paint_quote_rings(painter: &egui::Painter, layout: &DomLayout, view: &DomView) {
    if view.status != DomStatus::Live {
        return;
    }
    for (index, row) in view.ask_rows().iter().enumerate() {
        ring_for(painter, layout, layout.cell(layout.ask_row(index), 1), row);
    }
    for (index, row) in view.bid_rows().iter().enumerate() {
        ring_for(painter, layout, layout.cell(layout.bid_row(index), 1), row);
    }
}

fn ring_for(painter: &egui::Painter, layout: &DomLayout, cell: egui::Rect, row: &DomRow) {
    let (width, color) = match row.order_status {
        Some(OrderStatus::Confirmed) => (2.0, DARK.selected),
        Some(status) => (1.0, order_color(status)),
        None if row.is_quoted => (1.0, DARK.selected.gamma_multiply(0.6)),
        None => return,
    };
    // The ring carries status alone once the tags drop out, so it thins with the row rather than
    // swallowing a compressed cell — never below the one pixel that keeps it on screen at all.
    let width = (width * layout.scale).max(theme::one_physical_pixel(painter));
    painter.rect_stroke(
        cell,
        0.0,
        egui::Stroke::new(width, color),
        egui::StrokeKind::Inside,
    );
}

/// A ladder that is not live paints muted, so an operator reading it in peripheral vision sees it
/// is not current before reading a single number.
fn at_status(color: egui::Color32, status: DomStatus) -> egui::Color32 {
    match status {
        DomStatus::Live => color,
        DomStatus::Stale | DomStatus::AwaitingBook | DomStatus::Disconnected => {
            color.gamma_multiply(0.42)
        }
    }
}
