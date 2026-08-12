//! Quote summary band: bid Δ / mid / ask Δ. Deltas follow the DOM's unit selector; the mid is
//! always a venue price.

use eframe::egui;

use crate::ids::Side;

use super::dom_view::DomUnit;
use super::format::{self, Wrote};
use super::monitor::MonitorFrame;
use super::monitor_view::quote_summary;
use super::theme::{self, DARK, METRICS};

const CELLS: usize = 3;

#[derive(Clone, Copy)]
enum Lane {
    Delta(Side),
    Mid,
}

/// Three flush cells separated by hairlines. Each blanks independently. Hover says a delta is a
/// distance, not a price.
pub(super) fn paint(ui: &mut egui::Ui, rect: egui::Rect, frame: &MonitorFrame<'_>) {
    let summary = quote_summary(frame.model, frame.instrument, frame.tick);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, DARK.panel_raised);
    // The mid carries the longest value (a half-tick BTC mid is 10 glyphs at the largest font in
    // the band), so it claims the widest cell rather than an equal third. Ten is the bound this
    // split is solved for — an eleven-glyph mid wants a deliberate re-solve, not a wider fraction.
    let mid_w = rect.width() * METRICS.monitor_mid_fraction;
    let delta_w = ((rect.width() - mid_w) / 2.0).max(0.0);
    let edges = [
        rect.left(),
        rect.left() + delta_w,
        rect.left() + delta_w + mid_w,
        rect.right(),
    ];

    let stroke = theme::hairline(&painter, DARK.border);
    for edge in &edges[1..CELLS] {
        painter.vline(theme::crisp(&painter, *edge), rect.y_range(), stroke);
    }

    let mut scratch = String::new();
    let cells = [
        (
            0usize,
            Lane::Delta(Side::Buy),
            DARK.bid,
            summary.bid_delta_half_ticks,
        ),
        (1, Lane::Mid, DARK.text_primary, summary.mid_half_ticks),
        (
            2,
            Lane::Delta(Side::Sell),
            DARK.ask,
            summary.ask_delta_half_ticks,
        ),
    ];
    for (col, lane, color, value) in cells {
        let cell = egui::Rect::from_min_max(
            egui::pos2(edges[col], rect.top()),
            egui::pos2(edges[col + 1], rect.bottom()),
        );
        painter.text(
            egui::pos2(cell.center().x, cell.top() + METRICS.space_3),
            egui::Align2::CENTER_CENTER,
            caption(lane, frame.dom_unit),
            egui::FontId::proportional(METRICS.monitor_micro_font),
            DARK.text_secondary,
        );

        let font = match lane {
            Lane::Mid => METRICS.monitor_mid_font,
            Lane::Delta(_) => METRICS.monitor_delta_font,
        };
        let wrote = match lane {
            Lane::Mid => format::write_opt_venue_mid(&mut scratch, value, frame.tick),
            Lane::Delta(_) => {
                write_delta(&mut scratch, value, summary.mid_half_ticks, frame.dom_unit)
            }
        };
        painter.text(
            egui::pos2(cell.center().x, cell.top() + cell.height() * 0.62),
            egui::Align2::CENTER_CENTER,
            scratch.as_str(),
            egui::FontId::monospace(font),
            cell_color(wrote, color),
        );

        if let Lane::Delta(_) = lane {
            ui.interact(
                cell,
                egui::Id::new(("monitor-delta", col)),
                egui::Sense::hover(),
            )
            .on_hover_text(delta_hover(frame.dom_unit));
        }
    }
}

/// Caption carries the unit because the number alone lies: the same offset is 2 ticks or 0.0017 bp.
fn caption(lane: Lane, unit: DomUnit) -> &'static str {
    match (lane, unit) {
        (Lane::Mid, _) => "MID",
        (Lane::Delta(Side::Buy), DomUnit::Ticks) => "BID delta ticks",
        (Lane::Delta(Side::Buy), DomUnit::Bps) => "BID delta bps",
        (Lane::Delta(Side::Sell), DomUnit::Ticks) => "ASK delta ticks",
        (Lane::Delta(Side::Sell), DomUnit::Bps) => "ASK delta bps",
    }
}

fn delta_hover(unit: DomUnit) -> &'static str {
    match unit {
        DomUnit::Ticks => "delta in ticks from mid, not a price",
        DomUnit::Bps => "delta in basis points from mid, not a price",
    }
}

/// Bps divides the two half-tick counts, so the tick cancels and no tick size is needed.
fn write_delta(
    buf: &mut String,
    delta_half_ticks: Option<i64>,
    mid_half_ticks: Option<i64>,
    unit: DomUnit,
) -> Wrote {
    match (unit, delta_half_ticks) {
        (DomUnit::Bps, _) => format::write_opt_bps_delta(buf, delta_half_ticks, mid_half_ticks),
        (DomUnit::Ticks, Some(delta)) => {
            format::write_half_tick_delta(buf, delta);
            Wrote::Value
        }
        (DomUnit::Ticks, None) => format::write_missing(buf),
    }
}

/// A bound is a real reading the cell is too narrow to spell out, so it wears neither the live
/// side colour nor the absent grey.
fn cell_color(wrote: Wrote, live: egui::Color32) -> egui::Color32 {
    match wrote {
        Wrote::Value => live,
        Wrote::Absent => DARK.text_secondary,
        Wrote::Bound => DARK.warning,
    }
}
