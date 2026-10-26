//! The quote summary band: the bid's distance from the mid, the mid, and the ask's distance. The
//! distances follow the ladder's unit selector, and the mid is always a venue price.

use eframe::egui;

use crate::ids::Side;

use crate::workstation::browser::theme::{self, DARK, METRICS};
use crate::workstation::format;
use crate::workstation::monitor::view::{MonitorFrame, quote_summary};
use crate::workstation::text::{Lane, Tone, cell_tone, delta_hover, summary_caption, write_delta};

const CELLS: usize = 3;

/// Three cells parted by hairlines, each blank on its own when it has no reading. Hovering a delta
/// says it is a distance, not a price.
pub(super) fn paint(ui: &mut egui::Ui, rect: egui::Rect, frame: &MonitorFrame<'_>) {
    let summary = quote_summary(frame.model, frame.instrument, frame.tick);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, DARK.panel_raised);
    // The mid carries the longest value (a half-tick BTC mid is 10 glyphs at the largest font in
    // the band), so it claims the widest cell rather than an equal third. Ten is the bound this
    // split is solved for: an eleven-glyph mid needs the split worked out again, not a wider
    // fraction.
    let mid_w = rect.width() * METRICS.monitor_mid_fraction;
    let delta_w = ((rect.width() - mid_w) / 2.0).max(0.0);
    let edges = [
        rect.left(),
        rect.left() + delta_w,
        rect.left() + delta_w + mid_w,
        rect.right(),
    ];

    theme::rules_v(&painter, edges[1..CELLS].iter().copied(), rect.y_range());

    let mut scratch = String::new();
    let cells = [
        (
            0usize,
            Lane::Delta(Side::Buy),
            Tone::Bid,
            summary.bid_delta_half_ticks,
        ),
        (1, Lane::Mid, Tone::Text, summary.mid_half_ticks),
        (
            2,
            Lane::Delta(Side::Sell),
            Tone::Ask,
            summary.ask_delta_half_ticks,
        ),
    ];
    for (col, lane, tone, value) in cells {
        let cell = rect.with_min_x(edges[col]).with_max_x(edges[col + 1]);
        painter.text(
            egui::pos2(cell.center().x, cell.top() + METRICS.space_3),
            egui::Align2::CENTER_CENTER,
            summary_caption(lane, frame.dom_unit),
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
            DARK.tone(cell_tone(wrote, tone)),
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
