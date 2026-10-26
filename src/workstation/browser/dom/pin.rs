//! Touch has no hover, so a tap on a row pins the readout there, and the next tap anywhere in the
//! ladder clears it.

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

/// Applies this frame's tap and returns where the readout is pinned, if anywhere. `ladder` is the
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
    let next = match touch_tap(ladder) {
        Some(at) if pin.is_none() && layout.row_at(view, at).is_some() => Some(Pin {
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

/// Returns where a finger tapped inside the ladder this frame, unless the tap landed on a window
/// drawn over it. The browser sends the pointer release before the touch end, so egui reports the
/// click in a frame that already holds a touch. A mouse click has no touch, so it never pins.
fn touch_tap(ladder: &egui::Response) -> Option<egui::Pos2> {
    let at = ladder.ctx.input(|input| {
        let is_touch = input.any_touches()
            || input
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::Touch { .. }));
        if input.pointer.primary_clicked() && is_touch {
            input.pointer.interact_pos()
        } else {
            None
        }
    })?;
    let is_on_top = ladder.ctx.layer_id_at(at) == Some(ladder.layer_id);
    (ladder.rect.contains(at) && is_on_top).then_some(at)
}
