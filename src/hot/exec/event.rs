//! Venue event -> slot transition, money movement, settlement release, audit rows. Split from
//! `engine.rs` for readability.

use crate::error;
use crate::hot::ledger::{LedgerFill, PositionLedger};
use crate::hot::strategy::Actions;
use crate::ids::{AssetId, Side};
use crate::msg::exec::{ExecEvent, ExecKind, RejectClass};
use crate::msg::persist::OrderLifecycle;

use super::account::ReleaseOutcome;
use super::audit::bank_event_rows;
use super::engine::ExecEngine;
use super::gates::{HaltReason, RejectSeverity};
use super::order::{FillDelta, MAX_ORDER_SLOTS, OrderSlot, OrderState};
use super::prior_run::PRIOR_RUN_SLOTS;
use super::transition::apply_exec_event;
use super::view::{ExecCallback, Fill, OrderReject, OrderUpdate, RejectOrigin};

impl ExecEngine {
    /// Retry releases that beat account update. Fixed-slot scan, no alloc.
    pub(super) fn retry_reservation_releases(&mut self) {
        for index in 0..MAX_ORDER_SLOTS {
            let slot = self.orders.slot(index);
            if slot.reserved_amount == 0 || slot.state == OrderState::PendingNew {
                continue;
            }
            let assets = &self.instruments[usize::from(slot.instrument.0)];
            let (base_asset, quote_asset) = (assets.base_asset, assets.quote_asset);
            self.release_reservation(index, base_asset, quote_asset);
        }
    }

    pub(super) fn apply_to_slot(
        &mut self,
        index: usize,
        event: &ExecEvent,
        ledger: &mut PositionLedger,
        bank: &mut Actions,
    ) -> ExecCallback {
        let previous_state = OrderLifecycle::from(self.orders.slot(index).state);
        let applied = apply_exec_event(self.orders.slot_mut(index), event);
        if applied.is_resurrection {
            self.counters.resurrections += 1;
            self.resurrection_error(event);
        }
        let slot = *self.orders.slot(index);
        let assets = &self.instruments[usize::from(slot.instrument.0)];
        let (base_asset, quote_asset) = (assets.base_asset, assets.quote_asset);

        self.book_fill(&slot, event, applied.fill, ledger);
        if event.kind == ExecKind::PlaceNotSent {
            self.release_unsent_reservation(index, base_asset, quote_asset);
        } else {
            self.release_reservation(index, base_asset, quote_asset);
        }
        self.record_outcome(event, applied.state);
        bank_event_rows(bank, &slot, event, previous_state, applied.fill);

        if !applied.fill.is_empty() {
            return ExecCallback::Fill(Fill {
                instrument: slot.instrument,
                client_id: slot.client_id,
                side: slot.side,
                level: slot.level,
                price: event.last_price,
                qty: applied.fill.base,
                notional: applied.fill.quote,
                commission: event.commission,
                commission_asset: event.commission_asset,
                liquidity: event.liquidity,
                trade_id: event.trade_id,
                state: applied.state,
                event_ts_us: event.received_ts_us,
            });
        }
        if let Some(class) = event.reject {
            return ExecCallback::Reject(OrderReject {
                instrument: slot.instrument,
                client_id: Some(slot.client_id),
                side: slot.side,
                level: Some(slot.level),
                origin: RejectOrigin::Venue {
                    class,
                    code: event.reject_code,
                },
                event_ts_us: event.received_ts_us,
            });
        }
        ExecCallback::Update(OrderUpdate {
            instrument: slot.instrument,
            client_id: slot.client_id,
            side: slot.side,
            level: slot.level,
            state: applied.state,
            price: slot.price,
            qty: slot.qty,
            filled: slot.filled_base,
            event_ts_us: event.received_ts_us,
        })
    }

    /// Prior-run order event: no reservation release, no reject count, no callback (position already in ledger).
    #[cold]
    pub(super) fn apply_to_prior_run(
        &mut self,
        event: &ExecEvent,
        ledger: &mut PositionLedger,
        bank: &mut Actions,
    ) {
        let Some(index) = self.prior_orders.find_or_seat(event) else {
            self.counters.prior_run_overflows += 1;
            self.prior_run_overflow_error(event);
            return;
        };
        let previous_state = OrderLifecycle::from(self.prior_orders.slot(index).state);
        let applied = apply_exec_event(self.prior_orders.slot_mut(index), event);
        let slot = *self.prior_orders.slot(index);
        self.book_fill(&slot, event, applied.fill, ledger);
        bank_event_rows(bank, &slot, event, previous_state, applied.fill);
    }

    /// Fee + fill. No-op on empty (duplicates already folded).
    fn book_fill(
        &mut self,
        slot: &OrderSlot,
        event: &ExecEvent,
        fill: FillDelta,
        ledger: &mut PositionLedger,
    ) {
        if fill.is_empty() {
            return;
        }
        self.counters.fills_applied += 1;
        // Only commission in own quote_asset counts (foreign asset = wrong PnL forever).
        let quote_asset = self.instruments[usize::from(slot.instrument.0)].quote_asset;
        let commission_quote =
            if event.commission_asset == quote_asset { event.commission } else { 0 };
        ledger.apply_fill(&LedgerFill {
            instrument: slot.instrument,
            side: slot.side,
            base: fill.base,
            notional_quote: fill.quote,
            commission_quote,
        });
    }

    #[cold]
    fn prior_run_overflow_error(&self, event: &ExecEvent) {
        error!(
            "prior-run order {:x} has nowhere to fold — all {PRIOR_RUN_SLOTS} entries are still working, so its fills cannot reach the ledger",
            event.client_id.0
        );
    }

    #[cold]
    fn resurrection_error(&self, event: &ExecEvent) {
        error!(
            "order {:x} was believed closed and the venue still holds it — resurrected; an order believed dead is unbounded risk",
            event.client_id.0
        );
    }

    /// The release POLICY is a venue capability, [`ExecSettings::holds_reservations_until_settled`].
    ///
    /// A venue that locks funds on placement (Binance) folds the lock into every balance update, so
    /// the release waits for an update stamped later than the reservation — the watermark gate — on
    /// any slot past `PendingNew`, resting orders included. Ack alone would double-spend.
    ///
    /// A venue that does NOT lock (Polymarket) never moves balances for an open order, so `free`
    /// never subtracts a resting order: releasing while the slot still WORKS would over-admit against
    /// money it has committed, and a fill anywhere would then free a live order's reservation. There
    /// the reservation stands until the slot terminates — a zero-fill terminal moved no money and
    /// releases UNGATED; a filled terminal keeps the gate, because its post-fill restatement is what
    /// drops `free` and releasing before it lands would double-spend.
    fn release_reservation(&mut self, index: usize, base_asset: AssetId, quote_asset: AssetId) {
        let slot = self.orders.slot(index);
        if slot.reserved_amount == 0 {
            return;
        }
        let asset = match slot.side {
            Side::Buy => quote_asset,
            Side::Sell => base_asset,
        };
        if self.settings.holds_reservations_until_settled {
            if slot.state != OrderState::PendingNew {
                self.gated_release(index, asset);
            }
            return;
        }
        if slot.state.is_working() {
            return;
        }
        if slot.filled_base.0 == 0 {
            self.account.release_unsent(asset, slot.reserved_amount);
            self.orders.slot_mut(index).reserved_amount = 0;
            return;
        }
        self.gated_release(index, asset);
    }

    /// Release once the venue reports a balance stamped later than the reservation, else leave it
    /// held for the next spin to retry.
    fn gated_release(&mut self, index: usize, asset: AssetId) {
        let slot = self.orders.slot(index);
        let outcome = self
            .account
            .release(asset, slot.reserved_amount, slot.reserved_at);
        if outcome == ReleaseOutcome::Released {
            self.orders.slot_mut(index).reserved_amount = 0;
        }
    }

    pub(super) fn release_unsent_reservation(
        &mut self,
        index: usize,
        base_asset: AssetId,
        quote_asset: AssetId,
    ) {
        let slot = self.orders.slot(index);
        if slot.reserved_amount == 0 {
            return;
        }
        let asset = match slot.side {
            Side::Buy => quote_asset,
            Side::Sell => base_asset,
        };
        self.account.release_unsent(asset, slot.reserved_amount);
        self.orders.slot_mut(index).reserved_amount = 0;
    }

    /// A venue answer either ends the reject streak or extends it. `Gone` is the routine one: a
    /// post-only quote the book moved through is the venue doing exactly what post-only asks, and
    /// counting it toward the kill switch parks the engine precisely when the market is moving.
    /// This relies on the edge mapping `INSUFFICIENT_BALANCES` to `Fatal` and not to `Gone` — the
    /// two share venue code `-2010` and only the reject REASON separates them. Verified against
    /// `adapters/binance/exec/codec/reject.rs` and `codec/stream.rs`: both split on the message, and
    /// an unrecognised `-2010` classifies `Fatal`, which is the direction that parks loudly rather
    /// than placing orders it cannot fund.
    fn record_outcome(&mut self, event: &ExecEvent, state: OrderState) {
        let Some(class) = event.reject else {
            // Only the venue can end a streak. The kinds the EDGE synthesises for a request that
            // never left resolve a slot with nobody at the venue having accepted anything — and an
            // amend refused for want of a socket leaves its order reading `Live`, which would read
            // here as acceptance and reset the count exactly when the venue is least reachable.
            let is_venue_answer =
                !matches!(event.kind, ExecKind::PlaceNotSent | ExecKind::AmendNotSent);
            if is_venue_answer && matches!(state, OrderState::Live) {
                self.rejects.record_accepted();
            }
            return;
        };
        match class {
            RejectClass::Fatal => {
                self.counters.venue_rejects += 1;
                self.fatal_reject_error(event);
                self.halt(HaltReason::FatalReject, event.received_ts_us);
            }
            // The ordinary cost of quoting post-only, on its own loose counter. It must never touch
            // the consecutive-hard streak: a maker earns these all day, and counting them as hard
            // failures parks the engine exactly when the market is interesting.
            RejectClass::Refused => {
                self.counters.venue_rejects += 1;
                self.counters.routine_rejects += 1;
                self.rejects.record(RejectSeverity::Routine);
            }
            // Gone = reconciliation answer (order never existed), not venue reject. Prevents false kill-switch on reconcile.
            RejectClass::Gone => {}
            RejectClass::StillLive | RejectClass::Ambiguous => {
                self.counters.venue_rejects += 1;
                self.rejects.record(RejectSeverity::Hard);
                if self.rejects.consecutive_hard() >= self.settings.max_consecutive_rejects {
                    self.halt(HaltReason::RejectStreak, event.received_ts_us);
                }
            }
        }
    }

    /// Class determines branch. Same fatal class for different causes -> error message carries the code.
    #[cold]
    fn fatal_reject_error(&self, event: &ExecEvent) {
        error!(
            "venue refused order {:x} fatally with code {} — halting rather than retrying",
            event.client_id.0, event.reject_code
        );
    }
}
