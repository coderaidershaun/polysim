//! Why a side is not quoting, reported when the answer CHANGES.
//!
//! Every gate in the quote pass already knows its reason and, until now, threw it away: a local
//! refusal was a `warn!` and nothing else, and the gates that block indefinitely — readiness, the
//! session permission, the exposure ceiling — did not even reach that. An operator watching an
//! engine place nothing had no way to ask why, which is how a readiness gate that could never arm
//! survived a whole milestone.
//!
//! Edge-triggered rather than level-triggered because the spin is one second and a refusal is
//! usually persistent: reporting every spin would bury the transitions that matter under two rows a
//! second, and reporting once forever would leave the panel claiming a reason that has since
//! changed. The latch reports the first spin of a reason and the first spin of its replacement, and
//! clears when the side quotes — so the row on screen is always the current answer.
//!
//! State is per (instrument, side) and message-driven only; no clock is read, so replay reproduces
//! the same reports in the same order.

use crate::ids::{InstrumentId, Side};
use crate::time::TsUs;
use crate::warn;

use super::engine::ExecEngine;
use super::order::MAX_ORDER_INSTRUMENTS;
use super::reconcile::RejectReason;
use super::spin::SpinInput;
use super::view::{OrderReject, RejectOrigin};

const SIDES: usize = 2;
const MAX_REFUSALS: usize = MAX_ORDER_INSTRUMENTS * SIDES;

pub struct RefusalLatch {
    current: [[Option<RejectReason>; SIDES]; MAX_ORDER_INSTRUMENTS],
    pending: [Option<OrderReject>; MAX_REFUSALS],
    len: usize,
}

impl RefusalLatch {
    pub fn new() -> Self {
        Self {
            current: [[None; SIDES]; MAX_ORDER_INSTRUMENTS],
            pending: [None; MAX_REFUSALS],
            len: 0,
        }
    }

    fn observe(
        &mut self,
        instrument: InstrumentId,
        side: Side,
        reason: Option<RejectReason>,
        at: TsUs,
    ) {
        let Some(cell) = self.slot_mut(instrument, side) else {
            return;
        };
        if *cell == reason {
            return;
        }
        *cell = reason;
        let Some(reason) = reason else {
            return;
        };
        self.push(OrderReject {
            instrument,
            client_id: None,
            side,
            level: None,
            origin: RejectOrigin::Local(reason),
            event_ts_us: at,
        });
        report(instrument, side, reason);
    }

    pub fn drain_into(&mut self, sink: &mut impl FnMut(&OrderReject)) {
        for slot in self.pending.iter_mut().take(self.len) {
            if let Some(reject) = slot.take() {
                sink(&reject);
            }
        }
        self.len = 0;
    }

    pub fn forget(&mut self) {
        self.current = [[None; SIDES]; MAX_ORDER_INSTRUMENTS];
    }

    fn slot_mut(
        &mut self,
        instrument: InstrumentId,
        side: Side,
    ) -> Option<&mut Option<RejectReason>> {
        let row = self.current.get_mut(usize::from(instrument.0))?;
        Some(&mut row[side.index()])
    }

    fn push(&mut self, reject: OrderReject) {
        if self.len >= MAX_REFUSALS {
            return;
        }
        self.pending[self.len] = Some(reject);
        self.len += 1;
    }
}

impl Default for RefusalLatch {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecEngine {
    pub(super) fn record_refusal(
        &mut self,
        instrument: InstrumentId,
        side: Side,
        reason: Option<RejectReason>,
        input: &SpinInput<'_>,
    ) {
        let reason = reason.or_else(|| {
            let top = self.book_top(instrument, input);
            (!top.is_quotable(self.settings.limits.max_book_age))
                .then_some(RejectReason::BookNotQuotable)
        });
        self.refusals
            .observe(instrument, side, reason, input.tick.received_ts_us);
    }
}

#[cold]
fn report(instrument: InstrumentId, side: Side, reason: RejectReason) {
    warn!(
        "quote refused locally on instrument {} {side:?}: {reason:?}",
        instrument.0
    );
}
