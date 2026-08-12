//! Cancelling, amending, and force-exiting orders the simulated venue already holds.

use super::queue::SimOrderIndex;
use super::request::{AdmitPlan, RequestFold, TimedAction};
use super::resting::{ClosedReason, OrderPhase, OrderSnapshot, RefusalReason, RestingOrder};
use super::{SimVenue, VenueEvent};
use crate::ids::{ClientOrderId, Qty};
use crate::time::TsUs;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ForcedOrderExit {
    pub(super) snapshot: OrderSnapshot,
    pub(super) was_pending: bool,
}

impl SimVenue {
    pub fn force_exit_open_orders(
        &mut self,
        at_ts_us: TsUs,
        cancelling: &[ClientOrderId],
    ) -> Vec<ForcedOrderExit> {
        let open: Vec<(SimOrderIndex, bool)> = self
            .orders
            .iter()
            .filter(|(_, record)| record.is_open() && !cancelling.contains(&record.order.client_id))
            .map(|(index, record)| (index, record.phase == OrderPhase::Pending))
            .collect();
        let mut exited = Vec::with_capacity(open.len());
        for (index, was_pending) in open {
            self.fold().close(index, ClosedReason::Canceled, at_ts_us);
            let snapshot = self
                .orders
                .snapshot(index)
                .expect("a forced order exit keeps its stable row");
            exited.push(ForcedOrderExit {
                snapshot,
                was_pending,
            });
        }
        exited
    }
}

impl RequestFold<'_> {
    pub(super) fn admit_cancel(
        &mut self,
        client_id: ClientOrderId,
        effective_ts_us: TsUs,
    ) -> AdmitPlan {
        let Some(index) = self.orders.find(client_id) else {
            self.emissions.push(
                effective_ts_us,
                VenueEvent::CancelRefused {
                    client_id,
                    reason: RefusalReason::NoSuchOrder,
                },
            );
            return None;
        };
        Some((effective_ts_us, TimedAction::Cancel(index)))
    }

    pub(super) fn admit_amend(
        &mut self,
        client_id: ClientOrderId,
        total_qty: Qty,
        effective_ts_us: TsUs,
    ) -> AdmitPlan {
        let Some(index) = self.orders.find(client_id) else {
            self.emissions.push(
                effective_ts_us,
                VenueEvent::AmendRefused {
                    client_id,
                    reason: RefusalReason::NoSuchOrder,
                },
            );
            return None;
        };
        Some((effective_ts_us, TimedAction::Amend { index, total_qty }))
    }

    pub(super) fn cancel(&mut self, index: SimOrderIndex, at_ts_us: TsUs) {
        let Some(record) = self.orders.get(index) else {
            return;
        };
        let client_id = record.order.client_id;
        if !record.is_open() {
            self.emissions.push(
                at_ts_us,
                VenueEvent::CancelRefused {
                    client_id,
                    reason: RefusalReason::OrderGone,
                },
            );
            return;
        }
        self.close(index, ClosedReason::Canceled, at_ts_us);
        let snapshot = self
            .orders
            .snapshot(index)
            .expect("a canceled simulated order keeps its verdict");
        self.emissions
            .push(at_ts_us, VenueEvent::Canceled { snapshot });
    }

    pub(super) fn amend(&mut self, index: SimOrderIndex, total_qty: Qty, at_ts_us: TsUs) {
        let Some(record) = self.orders.get(index) else {
            return;
        };
        let client_id = record.order.client_id;
        if let Some(reason) = self.refuse_amend(record, total_qty) {
            self.emissions
                .push(at_ts_us, VenueEvent::AmendRefused { client_id, reason });
            return;
        }

        let record = self
            .orders
            .get_mut(index)
            .expect("an admitted amend keeps its order record");
        let reservation = record
            .reservation
            .as_mut()
            .expect("a resting order has a wallet reservation");
        self.wallet.amend(reservation, total_qty);
        record.order.qty = total_qty;
        record.amends_used += 1;
        let snapshot = record.snapshot(index);
        self.emissions.push(
            at_ts_us,
            VenueEvent::Amended {
                snapshot,
                total_qty,
            },
        );
    }

    fn refuse_amend(&self, record: &RestingOrder, total_qty: Qty) -> Option<RefusalReason> {
        if record.phase != OrderPhase::Resting {
            return Some(RefusalReason::OrderGone);
        }
        if record.amends_used >= self.limits.max_amends {
            return Some(RefusalReason::AmendBudgetSpent);
        }
        if total_qty.0 >= record.order.qty.0 {
            return Some(RefusalReason::AmendQuantityIncrease);
        }
        if total_qty.0 <= record.order.filled.0 {
            return Some(RefusalReason::AmendFilterFailure);
        }
        self.limits.refuse(record.order.price, total_qty)
    }
}
