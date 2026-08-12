//! Turning a decision into a command: claim the slot, reserve the money, bank the command, record
//! the transition. Split from the spin loop because deciding WHAT to do and committing to it are
//! separate jobs — everything here runs after the argument is over, and every path through it must
//! leave the slot, the reservation and the audit trail agreeing.

use crate::hot::strategy::Actions;
use crate::ids::{ClientOrderId, InstrumentId, Side};
use crate::msg::exec::ExecCommand;
use crate::msg::persist::{OrderLifecycle, OrderTransition};
use crate::time::TsUs;

use super::audit::{OrderAudit, bank_order};
use super::engine::ExecEngine;
use super::gates::HaltReason;
use super::level::QuoteLevel;
use super::order::{CloseReason, OrderClaim, OrderState};
use super::reconcile::{PlaceIntent, ReconcileOutcome};
use super::spin::SpinInput;

impl ExecEngine {
    pub(super) fn act_on(
        &mut self,
        instrument: InstrumentId,
        side: Side,
        level: QuoteLevel,
        outcome: ReconcileOutcome,
        input: &mut SpinInput<'_>,
    ) {
        let at = input.tick.received_ts_us;
        match outcome {
            ReconcileOutcome::Nothing => {}
            ReconcileOutcome::Reject(_) => {}
            ReconcileOutcome::Place(intent) => {
                self.place(instrument, side, level, intent, at, input.bank)
            }
            ReconcileOutcome::Cancel => {
                let Some(index) = self.resting_index(instrument, side, level) else {
                    return;
                };
                let client_id = self.orders.slot(index).client_id;
                if self.bank(ExecCommand::Cancel {
                    instrument,
                    client_id,
                }) {
                    self.transition_sent(index, OrderState::CancelInFlight, at, input.bank);
                } else {
                    self.halt(HaltReason::CommandBankOverflow, at);
                }
            }
            ReconcileOutcome::AmendQty(qty) => {
                let Some(index) = self.resting_index(instrument, side, level) else {
                    return;
                };
                let client_id = self.orders.slot(index).client_id;
                if self.bank(ExecCommand::AmendQty {
                    instrument,
                    client_id,
                    qty,
                }) {
                    self.transition_sent(index, OrderState::AmendInFlight, at, input.bank);
                } else {
                    self.halt(HaltReason::CommandBankOverflow, at);
                }
            }
        }
    }

    #[inline]
    fn resting_index(
        &self,
        instrument: InstrumentId,
        side: Side,
        level: QuoteLevel,
    ) -> Option<usize> {
        let client_id = self.orders.resting(instrument, side, level)?.client_id;
        self.orders.find(client_id)
    }

    pub(super) fn transition_sent(
        &mut self,
        index: usize,
        next: OrderState,
        at: TsUs,
        bank: &mut Actions,
    ) {
        let slot = self.orders.slot_mut(index);
        let previous = OrderLifecycle::from(slot.state);
        slot.state = next;
        bank_order(
            bank,
            OrderAudit::engine_driven(slot, transition_for(next), previous, at),
        );
    }

    /// Every placement the engine mints passes through here, so every one of them spends the venue
    /// budget — a flatten is exempt from being REFUSED by it, never from consuming it. Counting the
    /// exits is what makes the quote gate refuse early enough to leave room for the next one.
    pub(super) fn place(
        &mut self,
        instrument: InstrumentId,
        side: Side,
        level: QuoteLevel,
        intent: PlaceIntent,
        at: TsUs,
        bank: &mut Actions,
    ) {
        let Some((index, client_id)) = self.claim(instrument, side, level, intent, at) else {
            return;
        };
        let banked = self.bank(ExecCommand::Place {
            instrument,
            client_id,
            side,
            price: intent.price,
            qty: intent.qty,
            style: intent.style,
        });
        if !banked {
            self.unwind(index, at, bank);
            return;
        }
        self.budget.record_place();
        bank_order(
            bank,
            OrderAudit::engine_driven(
                self.orders.slot(index),
                OrderTransition::Placed,
                OrderLifecycle::Free,
                at,
            ),
        );
    }

    fn claim(
        &mut self,
        instrument: InstrumentId,
        side: Side,
        level: QuoteLevel,
        intent: PlaceIntent,
        at: TsUs,
    ) -> Option<(usize, ClientOrderId)> {
        let Some((index, client_id)) = self.orders.claim(OrderClaim {
            instrument,
            side,
            level,
            price: intent.price,
            qty: intent.qty,
            style: intent.style,
            claimed_ts_us: at,
            recon_seq: self.recon_seq,
        }) else {
            self.halt(HaltReason::SlotLeak, at);
            return None;
        };
        let assets = &self.instruments[usize::from(instrument.0)];
        let (asset, amount) = match side {
            Side::Buy => (assets.quote_asset, intent.price.notional(intent.qty)),
            Side::Sell => (assets.base_asset, intent.qty.0),
        };
        let reserved_at = self.account.reserve(asset, amount);
        let slot = self.orders.slot_mut(index);
        slot.reserved_amount = amount;
        slot.reserved_at = reserved_at;
        Some((index, client_id))
    }

    #[cold]
    fn unwind(&mut self, index: usize, at: TsUs, bank: &mut Actions) {
        let slot = self.orders.slot(index);
        let assets = &self.instruments[usize::from(slot.instrument.0)];
        let (base_asset, quote_asset) = (assets.base_asset, assets.quote_asset);
        self.release_unsent_reservation(index, base_asset, quote_asset);
        let slot = self.orders.slot_mut(index);
        let previous = OrderLifecycle::from(slot.state);
        slot.state = OrderState::Closed(CloseReason::Rejected);
        slot.closed_ts_us = at;
        bank_order(
            bank,
            OrderAudit::engine_driven(slot, OrderTransition::SendAbandoned, previous, at),
        );
        self.halt(HaltReason::CommandBankOverflow, at);
    }

    /// `false` means the bank refused the command and nothing was sent. The bank holds a whole
    /// spin's worth of work, so a refusal means the edge has stopped draining and the engine can no
    /// longer act at all: every caller unwinds whatever it had already committed and halts. The one
    /// exception is the sweep the halt itself issues, which has nothing left to escalate to and says
    /// so out loud instead.
    #[inline]
    #[must_use]
    pub(super) fn bank(&mut self, command: ExecCommand) -> bool {
        let banked = self.pending.bank(command);
        if banked {
            self.counters.commands_banked += 1;
        }
        banked
    }
}

#[inline]
fn transition_for(state: OrderState) -> OrderTransition {
    match state {
        OrderState::CancelInFlight => OrderTransition::CancelSent,
        OrderState::AmendInFlight => OrderTransition::AmendSent,
        other => unreachable!("no command puts an order into {other:?}"),
    }
}
