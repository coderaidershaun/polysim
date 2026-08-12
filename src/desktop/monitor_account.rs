//! Always-visible account band (real money, peripheral vision). Projection-driven, values independently absent.

use eframe::egui;

use crate::hot::exec::{ExecHalt, RejectOrigin};

use super::format::{self, MISSING};
use super::monitor::{MonitorFrame, paint_left_ellipsized};
use super::monitor_view::{AccountView, AssetRole, AssetRowView, SideCountView, account};
use super::theme::{self, DARK, METRICS};

/// Balance fraction digits: 5 (Binance 0.00001 BTC stepSize) instead of 8 (too noisy).
const BALANCE_DECIMALS: usize = 5;

/// A notional is money, and money reads at two places whatever precision its asset carries.
const VALUE_DECIMALS: usize = 2;

/// Fixed row count (header, base, quote, bid/ask counts, status) -> height stable.
const ROWS: usize = 6;

/// Band height from fixed row count (metric change can't desync layout/painter).
pub(super) fn height() -> f32 {
    ROWS as f32 * METRICS.monitor_row_height + 2.0 * METRICS.space_1
}

/// Paint the account band. Caller owns rect (no fill).
pub fn paint(ui: &mut egui::Ui, rect: egui::Rect, frame: &MonitorFrame<'_>) {
    let view = account(frame.model, frame.instrument, frame.tick);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, DARK.panel_raised);

    let grid = Grid::new(rect);
    let mut scratch = String::new();
    let mut row = RowCursor::new(rect);

    let header = row.next();
    paint_header(&painter, &grid, header, &view);
    paint_asset(&painter, &grid, row.next(), view.base, &mut scratch);
    let assets_bottom = row.next();
    paint_asset(&painter, &grid, assets_bottom, view.quote, &mut scratch);
    paint_counts(&painter, &grid, row.next(), "BID", view.bid);
    paint_counts(&painter, &grid, row.next(), "ASK", view.ask);
    paint_status(&painter, &grid, row.next(), &view);

    // The container separates its own rows; no cell wears a box. The first rule closes the captions,
    // the second closes the asset table so the counts below it read as a separate list.
    let stroke = theme::hairline(&painter, DARK.border);
    for y in [header.bottom(), assets_bottom.bottom()] {
        painter.hline(rect.x_range(), theme::crisp(&painter, y), stroke);
    }
}

/// Hands out fixed-height rows in order (no painter computes y).
struct RowCursor {
    rect: egui::Rect,
    index: usize,
}

impl RowCursor {
    fn new(rect: egui::Rect) -> Self {
        Self { rect, index: 0 }
    }

    fn next(&mut self) -> egui::Rect {
        let top =
            self.rect.top() + METRICS.space_1 + self.index as f32 * METRICS.monitor_row_height;
        self.index += 1;
        egui::Rect::from_min_max(
            egui::pos2(self.rect.left(), top),
            egui::pos2(self.rect.right(), top + METRICS.monitor_row_height),
        )
    }
}

/// Label column plus three equal numeric columns, so every row lands on the same x positions and the
/// captions describe what sits under them.
struct Grid {
    label_left: f32,
    numbers_left: f32,
    column_width: f32,
}

impl Grid {
    const COLUMNS: usize = 3;

    fn new(rect: egui::Rect) -> Self {
        let label_left = rect.left() + METRICS.space_2;
        let numbers_left = label_left + METRICS.monitor_asset_label_w;
        let width = (rect.right() - METRICS.space_2 - numbers_left).max(0.0);
        Self {
            label_left,
            numbers_left,
            column_width: width / Self::COLUMNS as f32,
        }
    }

    fn label(&self, row: egui::Rect) -> egui::Rect {
        egui::Rect::from_min_max(
            egui::pos2(self.label_left, row.top()),
            egui::pos2(self.numbers_left, row.bottom()),
        )
    }

    fn column(&self, row: egui::Rect, index: usize) -> egui::Rect {
        let left = self.numbers_left + index as f32 * self.column_width;
        egui::Rect::from_min_max(
            egui::pos2(left, row.top()),
            egui::pos2(left + self.column_width, row.bottom()),
        )
    }
}

/// Band title, dust note and the asset columns' captions. Unknown assets collapse to one id (show
/// count, not wrong number), and the note rides beside the title because the right side is captions.
fn paint_header(painter: &egui::Painter, grid: &Grid, rect: egui::Rect, view: &AccountView<'_>) {
    let y = theme::crisp(painter, rect.center().y);
    let title = painter.text(
        egui::pos2(rect.left() + METRICS.space_2, y),
        egui::Align2::LEFT_CENTER,
        "ACCOUNT",
        egui::FontId::proportional(METRICS.monitor_micro_font),
        DARK.text_secondary,
    );
    let mut note_left = title.right();
    for (count, noun) in [
        (view.unknown_asset_balances, "untracked"),
        (view.rejected_position_frames, "exposure dropped"),
    ] {
        if count == 0 {
            continue;
        }
        let note = painter.text(
            egui::pos2(note_left + METRICS.space_2, y),
            egui::Align2::LEFT_CENTER,
            format!("+{count} {noun}"),
            egui::FontId::proportional(METRICS.monitor_micro_font),
            DARK.text_secondary,
        );
        note_left = note.right();
    }

    let mut value_caption = String::from("value");
    if !view.quote.label.is_empty() {
        value_caption.push(' ');
        value_caption.push_str(view.quote.label);
    }
    for (index, caption) in ["free", "locked", value_caption.as_str()]
        .into_iter()
        .enumerate()
    {
        paint_caption(painter, grid.column(rect, index), caption);
    }
}

fn paint_caption(painter: &egui::Painter, cell: egui::Rect, caption: &str) {
    painter.text(
        egui::pos2(cell.right(), theme::crisp(painter, cell.center().y)),
        egui::Align2::RIGHT_CENTER,
        caption,
        egui::FontId::proportional(METRICS.monitor_micro_font),
        DARK.text_secondary,
    );
}

/// One row per asset: free, locked and what the whole holding is worth. The label paints FIRST so an
/// oversized balance overruns the asset name rather than a digit — losing "USDT" is recoverable.
fn paint_asset(
    painter: &egui::Painter,
    grid: &Grid,
    rect: egui::Rect,
    asset: AssetRowView<'_>,
    scratch: &mut String,
) {
    painter.text(
        egui::pos2(
            grid.label(rect).left(),
            theme::crisp(painter, rect.center().y),
        ),
        egui::Align2::LEFT_CENTER,
        asset_label(asset),
        egui::FontId::monospace(METRICS.monitor_channel_font),
        DARK.text_primary,
    );

    let free = asset.balance.map(|balance| balance.free);
    let locked = asset.balance.map(|balance| balance.locked);
    write_amount(scratch, free, BALANCE_DECIMALS);
    paint_number(painter, grid.column(rect, 0), scratch, DARK.text_primary);
    // Locked dimmer: venue-held (not spendable) prevents misreading.
    write_amount(scratch, locked, BALANCE_DECIMALS);
    paint_number(painter, grid.column(rect, 1), scratch, DARK.text_secondary);
    // Two places against the balances' five: obviously the same money, obviously a total.
    write_amount(scratch, asset.value, VALUE_DECIMALS);
    paint_number(painter, grid.column(rect, 2), scratch, DARK.text_primary);
}

/// Catalog asset name, or role if no name. Old engines (no names in catalog) -> label role not index.
fn asset_label(asset: AssetRowView<'_>) -> &str {
    if !asset.label.is_empty() {
        return asset.label;
    }
    match asset.role {
        AssetRole::Base => "BASE",
        AssetRole::Quote => "QUOTE",
    }
}

fn write_amount(scratch: &mut String, mantissa: Option<i64>, decimals: usize) {
    match mantissa {
        Some(mantissa) => format::write_quote_amount(scratch, mantissa, decimals),
        None => {
            format::write_missing(scratch);
        }
    }
}

fn paint_number(painter: &egui::Painter, cell: egui::Rect, text: &str, color: egui::Color32) {
    painter.text(
        egui::pos2(cell.right(), theme::crisp(painter, cell.center().y)),
        egui::Align2::RIGHT_CENTER,
        text,
        egui::FontId::monospace(METRICS.monitor_channel_font),
        color,
    );
}

/// Side's working-order counts on the asset grid's columns, each cell tagged because no caption
/// describes them. `lost` renders even at zero — a real zero is not an absence — and colours only
/// when non-zero. Leaked orders qualify the side itself, so the marker rides beside its label.
fn paint_counts(
    painter: &egui::Painter,
    grid: &Grid,
    rect: egui::Rect,
    label: &str,
    counts: SideCountView,
) {
    let y = theme::crisp(painter, rect.center().y);
    let label_rect = painter.text(
        egui::pos2(grid.label(rect).left(), y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::monospace(METRICS.monitor_channel_font),
        DARK.text_secondary,
    );
    if counts.leaked > 0 {
        painter.text(
            egui::pos2(label_rect.right() + METRICS.space_1, y),
            egui::Align2::LEFT_CENTER,
            format!("+{}", counts.leaked),
            egui::FontId::monospace(METRICS.monitor_channel_font),
            DARK.invalid,
        );
    }

    let lost_color = if counts.lost > 0 { DARK.invalid } else { DARK.text_secondary };
    let cells = [
        ("open", counts.open, DARK.text_primary),
        ("inflt", counts.in_flight, DARK.text_primary),
        ("lost", counts.lost, lost_color),
    ];
    for (index, (tag, count, color)) in cells.into_iter().enumerate() {
        let cell = grid.column(rect, index);
        painter.text(
            egui::pos2(cell.left(), y),
            egui::Align2::LEFT_CENTER,
            tag,
            egui::FontId::proportional(METRICS.monitor_micro_font),
            DARK.text_secondary,
        );
        paint_number(painter, cell, &count.to_string(), color);
    }
}

/// Last refusal and the kill switch share a row. A HALT takes the whole row instead: `HALTED` beside
/// a reject would collide exactly when it matters, and every refusal is already in the Orders
/// channel. Pre-first-frame gate = `—` not "armed".
fn paint_status(painter: &egui::Painter, grid: &Grid, rect: egui::Rect, view: &AccountView<'_>) {
    let y = theme::crisp(painter, rect.center().y);
    if let Some(ExecHalt::Halted { reason, .. }) = view.halt {
        // The halt takes the whole row rather than sharing it with the reject: filled, not outlined,
        // so it is findable in peripheral vision, and never colliding with a refusal beside it.
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left() + METRICS.space_2, rect.top() + 1.0),
                egui::pos2(rect.right() - METRICS.space_2, rect.bottom() - 1.0),
            ),
            METRICS.corner_radius,
            DARK.halt_fill,
        );
        painter.text(
            egui::pos2(rect.right() - METRICS.space_2 - METRICS.space_1, y),
            egui::Align2::RIGHT_CENTER,
            format!("HALTED  {}", reason.label()),
            egui::FontId::monospace(METRICS.monitor_channel_font),
            DARK.invalid,
        );
        return;
    }

    painter.text(
        egui::pos2(grid.label(rect).left(), y),
        egui::Align2::LEFT_CENTER,
        "reject",
        egui::FontId::proportional(METRICS.monitor_micro_font),
        DARK.text_secondary,
    );
    let (text, color) = match view.last_reject {
        Some(reject) => reject_display(reject.origin),
        None => (MISSING.to_owned(), DARK.text_secondary),
    };
    let reject_text_rect = egui::Rect::from_min_max(
        grid.column(rect, 0).min,
        egui::pos2(grid.column(rect, 1).right(), rect.bottom()),
    );
    paint_left_ellipsized(
        painter,
        reject_text_rect,
        &text,
        egui::FontId::monospace(METRICS.monitor_channel_font),
        color,
    );

    let (gate, gate_color) = match view.halt {
        Some(ExecHalt::Armed) => ("ARMED", DARK.positive),
        _ => (MISSING, DARK.text_secondary),
    };
    paint_number(painter, grid.column(rect, 2), gate, gate_color);
}

fn reject_display(origin: RejectOrigin) -> (String, egui::Color32) {
    match origin {
        RejectOrigin::Local(reason) => (format!("gate {}", reason.label()), DARK.warning),
        RejectOrigin::Venue { class, code } => (
            format!("{code}  {}", class.as_str()),
            theme::reject_color(class),
        ),
    }
}
