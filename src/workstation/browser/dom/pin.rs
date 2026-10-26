//! Touch has no hover, so a finger tap on a row pins the readout there, and the next tap or click
//! anywhere in the ladder clears it. A mouse click never pins: hovering already shows the readout.

use eframe::egui;

use super::geometry::DomLayout;
use crate::workstation::dom_view::DomView;

/// A pin marks a spot on the screen, not a price level. As the book moves, the readout shows
/// whatever row is under that spot, as it would under a resting mouse. A resize, a moved pane or a
/// single frame without the ladder drops the pin.
#[derive(Clone, Copy)]
struct Pin {
    body: egui::Rect,
    at: egui::Pos2,
    last_painted: u64,
}

/// Applies this frame's click and returns where the readout is pinned, if anywhere. `ladder` is the
/// response of the ladder's whole body.
pub(super) fn pinned_point(
    ladder: &egui::Response,
    layout: &DomLayout,
    view: &DomView,
) -> Option<egui::Pos2> {
    let id = ladder.id.with("pin");
    let body = ladder.rect;
    let this_frame = ladder.ctx.cumulative_frame_nr();
    let pin = ladder
        .ctx
        .data(|data| data.get_temp::<Option<Pin>>(id))
        .flatten()
        .filter(|pin| pin.body == body && pin.last_painted + 1 >= this_frame);
    let next = match read_click(ladder) {
        Some(Click::Touch(at)) if pin.is_none() && layout.row_at(view, at).is_some() => Some(Pin {
            body,
            at,
            last_painted: this_frame,
        }),
        Some(_) => None,
        None => pin.map(|pin| Pin {
            last_painted: this_frame,
            ..pin
        }),
    };
    ladder.ctx.data_mut(|data| data.insert_temp(id, next));
    next.map(|pin| pin.at)
}

enum Click {
    Touch(egui::Pos2),
    Mouse,
}

/// Returns the click that landed inside the ladder this frame, unless it landed on a window drawn
/// over it. The browser sends a finger's pointer release before its touch end, so a finger's click
/// arrives in a frame that still holds the touch, and a mouse click's frame holds none.
fn read_click(ladder: &egui::Response) -> Option<Click> {
    let (at, is_touch) = ladder.ctx.input(|input| {
        if !input.pointer.primary_clicked() {
            return None;
        }
        let is_touch = input.any_touches()
            || input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::Touch { .. }));
        Some((input.pointer.interact_pos()?, is_touch))
    })?;
    let is_on_top = ladder.ctx.layer_id_at(at) == Some(ladder.layer_id);
    if !ladder.rect.contains(at) || !is_on_top {
        return None;
    }
    Some(if is_touch { Click::Touch(at) } else { Click::Mouse })
}
