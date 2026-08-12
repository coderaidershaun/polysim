//! Orders an EARLIER run of this trading engine left resting on the venue, in a table of their own.
//!
//! They are cancelled on sight, but the cancel takes hundreds of milliseconds to reach the venue and
//! whatever is resting fills in the meantime; the trade repair surfaces such a fill later still.
//! Those are real executions on this account, so they move the ledger and earn audit rows exactly
//! like any other — the engine simply never sent them.
//!
//! What they must never do is join THIS run's quoting, which is why they are not folded into
//! [`OrderTable`](super::order::OrderTable). A prior-run client id decodes to a slot index in that
//! table, so seating one there would either evict an order this run is about to place or — had the
//! run got there first — lose the fill to the orphan path, decided by nothing but arrival order.
//! Keeping them apart also keeps a cancel answered "unknown order", this path's ordinary outcome,
//! out of the reject streak that trips the kill switch.

use crate::ids::{ClientOrderId, InstrumentId, Side};
use crate::msg::exec::ExecEvent;

use super::level::MAX_QUOTE_LEVELS;
use super::order::{MAX_ORDER_INSTRUMENTS, OrderSlot, OrderState};

pub(super) const PRIOR_RUN_SLOTS: usize = MAX_QUOTE_LEVELS * 2 * MAX_ORDER_INSTRUMENTS;

pub(super) struct PriorRunOrders {
    slots: Box<[OrderSlot; PRIOR_RUN_SLOTS]>,
}

impl PriorRunOrders {
    pub(super) fn new() -> Self {
        Self {
            slots: Box::new([OrderSlot::EMPTY; PRIOR_RUN_SLOTS]),
        }
    }

    #[inline]
    pub(super) fn slot(&self, index: usize) -> &OrderSlot {
        &self.slots[index]
    }

    #[inline]
    pub(super) fn slot_mut(&mut self, index: usize) -> &mut OrderSlot {
        &mut self.slots[index]
    }

    pub(super) fn working(
        &self,
        instrument: InstrumentId,
        side: Side,
    ) -> impl Iterator<Item = &OrderSlot> {
        self.slots.iter().filter(move |slot| {
            slot.instrument == instrument && slot.side == side && slot.state.is_working()
        })
    }

    pub(super) fn find_or_seat(&mut self, event: &ExecEvent) -> Option<usize> {
        if let Some(index) = self.find(event.client_id) {
            return Some(index);
        }
        let index = self.free_or_oldest_closed()?;
        self.slots[index] = OrderSlot {
            client_id: event.client_id,
            venue_order_id: event.venue_order_id,
            state: OrderState::Unknown,
            side: event.side,
            instrument: event.instrument,
            price: event.price,
            qty: event.qty,
            last_event_ts_us: event.received_ts_us,
            ..OrderSlot::EMPTY
        };
        Some(index)
    }

    fn find(&self, id: ClientOrderId) -> Option<usize> {
        self.slots
            .iter()
            .position(|slot| slot.client_id == id && slot.state != OrderState::Free)
    }

    fn free_or_oldest_closed(&self) -> Option<usize> {
        if let Some(index) = self
            .slots
            .iter()
            .position(|slot| slot.state == OrderState::Free)
        {
            return Some(index);
        }
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| matches!(slot.state, OrderState::Closed(_)))
            .min_by_key(|(_, slot)| slot.closed_ts_us)
            .map(|(index, _)| index)
    }
}
