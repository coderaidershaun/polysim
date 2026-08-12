//! Folding venue responses into mirror state. Order leaves only when venue confirms it's gone;
//! an ambiguous answer (a venue "no such order" that also spells "already filled", or no answer at
//! all) triggers reconciliation, never a guess.

use crate::ids::{ClientOrderId, InstrumentId, Qty};
use crate::msg::exec::Provenance;
use crate::warn;

use super::super::effect::{ExecEffect, ExecRequest};
use super::super::mirror::MirroredOrder;
use super::{ExecCore, ObserveOrderError};

impl ExecCore {
    /// Cancels carry fills. Caller must answer "filled?" or ledger wrong -> sweep completion.
    pub fn on_order_gone(
        &mut self,
        client_id: ClientOrderId,
        executed_qty: Qty,
        emit: &mut dyn FnMut(ExecEffect),
    ) {
        if executed_qty.0 > 0 {
            warn!(
                "order {} left the book having filled {} — the fill must reach the ledger",
                client_id.0, executed_qty.0
            );
        }
        self.mirror.remove(client_id);
        self.settle_if_swept(emit);
    }

    /// NOT called for ambiguous (cancel in-flight, probe out). Re-arms on probe answer.
    pub fn re_arm_cancel(&mut self, client_id: ClientOrderId) {
        self.mirror.re_arm_cancel(client_id);
    }

    /// Answer left state unknown — the venue's "unknown order" covers never-existed AND just-filled
    /// (binance -2011, polymarket `order not found`). Recon must resolve.
    pub fn on_ambiguous(&mut self, client_id: ClientOrderId, emit: &mut dyn FnMut(ExecEffect)) {
        let Some(order) = self.mirror.find_mut(client_id) else {
            return;
        };
        order.is_ambiguous = true;
        let instrument = order.instrument;
        self.send(
            ExecRequest::OrderStatus {
                instrument,
                client_id,
            },
            emit,
        );
    }

    /// Prior-run leftovers get one cancel. NOT for Foreign orders -> no distinct client ids
    /// -> all collide on ClientOrderId(0) -> driver reports them directly from open-orders.
    /// Defense in depth: [`ExecCore::on_command`] also refuses them.
    pub fn observe_venue_order(&mut self, order: MirroredOrder) -> Result<(), ObserveOrderError> {
        if matches!(order.provenance, Provenance::Foreign) {
            warn!(
                "refusing to mirror a foreign order — foreign ids are not distinct, so the driver reports them directly"
            );
            return Ok(());
        }
        if !self.mirror.refresh(order) {
            // Refresh returning false has just proved no entry holds this id, so the only insert
            // failure left is a full mirror.
            self.mirror.insert(order).map_err(|_| {
                self.stop_quoting();
                ObserveOrderError::MirrorStorageExhausted {
                    capacity: self.mirror.capacity(),
                }
            })?;
        }
        let count = self
            .mirror
            .possibly_live_count(order.instrument, order.side);
        if count > self.max_orders_per_side {
            self.stop_quoting();
            return Err(ObserveOrderError::OwnedSideOverLimit {
                instrument: order.instrument,
                side: order.side,
                count,
                limit: self.max_orders_per_side,
            });
        }
        Ok(())
    }

    /// Unanswered past REQUEST_TIMEOUT -> reconcile, never retry (order may exist -> second live one).
    pub fn on_request_timeout(
        &mut self,
        instrument: InstrumentId,
        client_id: ClientOrderId,
        emit: &mut dyn FnMut(ExecEffect),
    ) {
        if let Some(order) = self.mirror.find_mut(client_id) {
            order.is_ambiguous = true;
        }
        self.send(
            ExecRequest::OrderStatus {
                instrument,
                client_id,
            },
            emit,
        );
    }
}
