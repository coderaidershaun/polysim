//! Paints the monitor's channel rows in fixed columns taken from each row's rect, so the numbers
//! never shift while the rows scroll.

use eframe::egui;

use super::monitor::paint_left_ellipsized;
use super::text::{Cell, TagColumn};
use super::theme::{self, DARK, METRICS};

fn mono_font() -> egui::FontId {
    egui::FontId::monospace(METRICS.monitor_channel_font)
}

fn text_font() -> egui::FontId {
    egui::FontId::proportional(METRICS.monitor_channel_font)
}

pub(super) fn paint_cells(painter: &egui::Painter, rect: egui::Rect, cells: &[Cell]) {
    let cols = ChannelColumns::new(painter, rect);
    let mut description_offset = 0.0;
    for cell in cells {
        match cell {
            Cell::Time(text) => cols.paint_time(painter, text),
            Cell::Tag(tag) => {
                cols.paint_tag(painter, tag);
                description_offset = METRICS.monitor_intent_tag_w;
            }
            Cell::Description { text, tone } => paint_left_ellipsized(
                painter,
                cols.text_rect(rect, cols.tag_x + description_offset),
                text,
                text_font(),
                DARK.tone(*tone),
            ),
            Cell::Price(text) => cols.paint_price(painter, text),
            Cell::Qty(text) => cols.paint_qty(painter, text),
            Cell::Detail(_) => {}
        }
    }
}

/// The column anchors and the text baseline, taken from the row's rect so the columns never move.
struct ChannelColumns {
    tag_x: f32,
    time_x: f32,
    price_x: f32,
    qty_x: f32,
    y: f32,
}

impl ChannelColumns {
    fn new(painter: &egui::Painter, rect: egui::Rect) -> Self {
        let pad = METRICS.space_2;
        let time_w = METRICS.monitor_time_col_w;
        let qty_w = METRICS.monitor_qty_col_w;
        Self {
            time_x: rect.left() + pad,
            tag_x: rect.left() + pad + time_w,
            price_x: rect.right() - pad - qty_w,
            qty_x: rect.right() - pad,
            y: theme::crisp(painter, rect.center().y),
        }
    }

    /// Liquidity note first when present, and the side word one tag width right of it.
    fn paint_tag(&self, painter: &egui::Painter, tag: &TagColumn) {
        let mut x = self.tag_x;
        if let Some(liquidity) = tag.liquidity {
            painter.text(
                egui::pos2(x, self.y),
                egui::Align2::LEFT_CENTER,
                liquidity,
                text_font(),
                DARK.text_secondary,
            );
            x += METRICS.monitor_fill_tag_w;
        }
        painter.text(
            egui::pos2(x, self.y),
            egui::Align2::LEFT_CENTER,
            tag.side,
            mono_font(),
            DARK.tone(tag.tone),
        );
    }

    fn paint_time(&self, painter: &egui::Painter, text: &str) {
        painter.text(
            egui::pos2(self.time_x, self.y),
            egui::Align2::LEFT_CENTER,
            text,
            mono_font(),
            DARK.text_secondary,
        );
    }

    fn paint_price(&self, painter: &egui::Painter, text: &str) {
        painter.text(
            egui::pos2(self.price_x, self.y),
            egui::Align2::RIGHT_CENTER,
            text,
            mono_font(),
            DARK.text_primary,
        );
    }

    fn paint_qty(&self, painter: &egui::Painter, text: &str) {
        painter.text(
            egui::pos2(self.qty_x, self.y),
            egui::Align2::RIGHT_CENTER,
            text,
            mono_font(),
            DARK.text_primary,
        );
    }

    /// A left-anchored rect from `left` to the row's right pad — the free-text description band.
    fn text_rect(&self, rect: egui::Rect, left: f32) -> egui::Rect {
        egui::Rect::from_min_max(
            egui::pos2(left, rect.top()),
            egui::pos2(self.qty_x, rect.bottom()),
        )
    }
}
