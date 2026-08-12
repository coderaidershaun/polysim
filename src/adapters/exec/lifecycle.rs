//! Shared handling for live and simulated execution responses.

use crate::msg::exec::{ExecCommand, ExecEvent, Provenance, RejectClass};
use crate::shutdown::FatalSignal;

use super::core::ExecCore;
use super::effect::ExecEffect;
use super::mirror::MirroredOrder;

pub fn mirrored_order(event: &ExecEvent) -> MirroredOrder {
    MirroredOrder {
        instrument: event.instrument,
        client_id: event.client_id,
        side: event.side,
        price: event.price,
        qty: event.qty,
        provenance: event.provenance,
        has_sent_cancel: false,
        is_ambiguous: false,
    }
}

/// Venue truth consumed by [`ExecCore`].
pub struct LifecycleFold<'a> {
    pub core: &'a mut ExecCore,
    pub fatal: &'a FatalSignal,
    /// Whether this run has started quoting.
    pub has_opened_quoting: bool,
}

impl LifecycleFold<'_> {
    pub fn on_event(&mut self, event: &ExecEvent, emit: &mut dyn FnMut(ExecEffect)) {
        if event.provenance == Provenance::Foreign {
            return;
        }
        self.observe_if_unknown(event, emit);
        // Apply fills before terminal state.
        match (event.reject, event.status) {
            // A missing cancel target may already be filled.
            (Some(RejectClass::Ambiguous), _) => self.core.on_ambiguous(event.client_id, emit),
            (Some(RejectClass::Gone | RejectClass::Refused), _) => {
                self.core
                    .on_order_gone(event.client_id, event.cumulative_qty, emit);
            }
            (_, Some(status)) if status.is_terminal() => {
                self.core
                    .on_order_gone(event.client_id, event.cumulative_qty, emit);
            }
            // Re-arm untouched orders.
            (Some(RejectClass::StillLive), _) => self.core.re_arm_cancel(event.client_id),
            _ => {}
        }
    }

    fn observe_if_unknown(&mut self, event: &ExecEvent, emit: &mut dyn FnMut(ExecEffect)) {
        let is_possibly_live = event.status.is_some_and(|status| !status.is_terminal());
        if !is_possibly_live || self.core.is_mirrored(event.client_id) {
            return;
        }
        let mut order = mirrored_order(event);
        if !self.has_opened_quoting {
            order.provenance = Provenance::PriorRun;
        }
        if let Err(error) = self.core.observe_venue_order(order) {
            self.fatal.trip(format!(
                "execution edge cannot retain an observed order in its possibly-live set: {error}"
            ));
        } else if order.provenance == Provenance::PriorRun {
            self.core.on_command(
                ExecCommand::CancelPriorRun {
                    instrument: order.instrument,
                },
                emit,
            );
        }
    }
}
