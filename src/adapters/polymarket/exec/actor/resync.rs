//! The quoting admission gate, and the only thing that rebuilds state after a disconnect: this
//! venue's stream replays nothing, so what is resting and what is held is re-read rather than
//! resumed.
//!
//! The hard question this file answers is what to do with an open order the run cannot name. At
//! BOOT every such order predates the process and is swept, which is the binance cold-start rule.
//! MID-RUN it is not: it may be the order whose placement answer was lost in transport, in which
//! case adopting it is the only way the engine ever regains the ability to cancel it — and if
//! nothing was placed there, it may be a person's order in the venue's own UI, which this engine
//! never touches.

use crate::adapters::exec::{
    MAX_RESYNC_ATTEMPTS, Outgoing, ResyncStep, SessionOutcome, mirrored_order,
    open_orders_snapshot_end,
};
use crate::ids::{AssetId, ClientOrderId, InstrumentId, Price, Qty};
use crate::msg::exec::{ExecCommand, ExecEvent, ExecKind, Provenance};
use crate::time::TsUs;
use crate::{error, info, warn};

use super::super::codec::{
    KnownOrder, OrdersRead, UnmappedOrder, VenueAnswer, cancel_venue_order, collateral_balance,
    conditional_balance, decode_orders_page, open_orders_for_token, open_orders_page, trades_page,
    venue_order_id_digest,
};
use super::super::correlate::{UnmappedVerdict, classify_unmapped};
use super::super::rest::ClobResponse;
use super::rest::{Auth, Lane, OpenOrdersRead, RestJob, RestPurpose, Submitted};
use super::{Actor, Writer};

/// Open orders, fills, and the balance sweep. The sweep counts as one read however many assets it
/// costs, because a partial sweep is not a partial answer.
const PASS_READS: usize = 3;

/// A balance sweep that belongs to no resync pass: a post-fill restatement answers the account
/// table, not the admission gate, so it settles nothing and schedules no retry of its own.
const RESTATEMENT: Option<u64> = None;

/// A page walk that never ends would hold a resync pass open for the life of the run, so it stops
/// and says so. This engine's own book is a handful of orders; anything near this many pages means
/// the account is being read for something other than what this run placed.
pub(super) const MAX_PAGES: u32 = 8;

impl Actor {
    /// Reads all three admission gates: open orders, fills, and balances. Any outstanding
    /// pass is abandoned here, since its answers would describe a socket that is now closed.
    pub(super) fn start_resync(&mut self) {
        self.is_readiness_pending = false;
        let resync_seq = self.resync.begin(PASS_READS);
        self.request_pass_reads(resync_seq);
    }

    /// Requests a resync when none is already outstanding — used after a lost answer, an
    /// unmirrored fill, or an unexplained frame.
    pub(super) fn nudge_resync(&mut self) {
        if self.resync.is_outstanding() {
            return;
        }
        self.start_resync();
    }

    fn request_pass_reads(&mut self, resync_seq: u64) {
        let mut is_queued = self
            .submit(
                Lane::Control,
                RestJob {
                    purpose: RestPurpose::OpenOrders {
                        read: OpenOrdersRead::Pass { resync_seq },
                        page: 0,
                        seen: Vec::new(),
                    },
                    request: open_orders_page(None),
                    auth: Auth::Signed,
                },
            )
            .is_queued();
        is_queued &= self
            .submit(
                Lane::Control,
                RestJob {
                    purpose: RestPurpose::Trades {
                        resync_seq: Some(resync_seq),
                        page: 0,
                    },
                    request: trades_page(None),
                    auth: Auth::Signed,
                },
            )
            .is_queued();
        is_queued &= self.request_balance_sweep(Some(resync_seq));
        if is_queued {
            return;
        }
        self.fail_resync(Some(resync_seq));
    }

    /// Re-reads balances after a fill. Defers if a sweep is outstanding (its reads were issued
    /// before the fill and can only answer with pre-fill balances); fires when that sweep lands.
    pub(super) fn restate_balances(&mut self) {
        if self.balances_outstanding > 0 {
            self.is_restatement_due = true;
            return;
        }
        // A restatement that could not be sent stays owed. Nothing else would ask again, and a
        // reservation is only released by a balance read carrying settlement the hot side has not
        // already seen.
        self.is_restatement_due = !self.request_balance_sweep(RESTATEMENT);
    }

    /// Reads each asset separately, since this venue has no multi-asset balance endpoint.
    ///
    /// Only reads that actually left are counted outstanding. Counting the intent instead leaves the
    /// gate waiting on an answer nobody asked for: the sweep never reaches zero, the account snapshot
    /// is never forwarded, and every later restatement defers behind it forever.
    fn request_balance_sweep(&mut self, resync_seq: Option<u64>) -> bool {
        self.balances.clear();
        self.is_balance_sweep_readable = true;
        let mut reads: Vec<(AssetId, super::super::codec::EncodedRequest)> =
            vec![(self.quote_asset(), collateral_balance(self.signature_type))];
        for instrument in self.instrument_ids() {
            let Some(token_id) = self
                .tokens
                .live_binding(instrument)
                .map(|binding| binding.token_id.clone())
            else {
                continue;
            };
            reads.push((
                self.base_asset(instrument),
                conditional_balance(&token_id, self.signature_type),
            ));
        }
        self.balances_outstanding = 0;
        let mut is_queued = true;
        for (asset, request) in reads {
            match self.submit(
                Lane::Control,
                RestJob {
                    purpose: RestPurpose::Balance { asset, resync_seq },
                    request,
                    auth: Auth::Signed,
                },
            ) {
                Submitted::Queued => self.balances_outstanding += 1,
                // An asset missing from the sweep makes the snapshot partial, and a partial account
                // is not an account: the reads that did land are discarded with the rest.
                Submitted::LaneFull => {
                    is_queued = false;
                    self.is_balance_sweep_readable = false;
                }
            }
        }
        is_queued
    }

    /// One page of open orders. Whether it settles a resync read is [`OpenOrdersRead`]'s to say, and
    /// the pass only settles on the page that exhausts the venue's cursor — a walk cut short mid-way
    /// would claim a completeness the read never had.
    pub(super) async fn on_open_orders_answer(
        &mut self,
        read: OpenOrdersRead,
        page: u32,
        mut seen: Vec<ClientOrderId>,
        response: &ClobResponse,
        writer: Option<&mut Writer>,
    ) -> Option<SessionOutcome> {
        let recon_seq = hot_recon_seq(read);
        let decoded = {
            let context = self.decode_context();
            decode_orders_page(
                response.answer(),
                OrdersRead {
                    instrument: read.instrument().unwrap_or(InstrumentId(0)),
                    recon_seq,
                },
                &context,
            )
        };
        let decoded = match decoded {
            Ok(VenueAnswer::Answered(decoded)) => decoded,
            Ok(VenueAnswer::Unavailable(availability)) => {
                self.on_unavailable(availability);
                self.fail_orders_read(read);
                return None;
            }
            Err(error) => {
                error!("polymarket execution could not read its open orders: {error}");
                self.fail_orders_read(read);
                return None;
            }
        };

        let mut outgoing = Vec::new();
        // The decoder closes each instrument's stream individually, but this read covers
        // every instrument at once, so those per-instrument SnapshotEnd markers are dropped here.
        let events: Vec<ExecEvent> = decoded
            .events
            .into_iter()
            .filter(|event| event.kind != ExecKind::SnapshotEnd)
            .collect();
        for event in &events {
            let mut mirrored = mirrored_order(event);
            if !self.has_opened_quoting {
                mirrored.provenance = Provenance::PriorRun;
            }
            seen.push(mirrored.client_id);
            if let Err(error) = self.core.observe_venue_order(mirrored) {
                error!(
                    "polymarket execution cannot retain venue order {:016x} in its possibly-live set ({error:?}) — quoting is disabled",
                    mirrored.client_id.0
                );
                self.control.fatal.trip(format!(
                    "polymarket execution possibly-live mirror failed: {error:?}"
                ));
                self.fail_orders_read(read);
                return None;
            }
        }
        for unmapped in &decoded.unmapped {
            self.on_unmapped_order(unmapped, &mut outgoing);
        }
        // At boot, an order on a token that isn't bound yet decodes to no instrument; the
        // cold-start rule cancels it by venue id anyway. Mid-run the same order is left
        // alone instead, since it simply belongs to a market this engine doesn't trade.
        if !self.has_opened_quoting {
            for record in &decoded.unattributable {
                self.cancel_prior_run_order(&record.venue_order_id);
            }
        }
        for event in events {
            self.fold_mirror(&event, recon_seq, &mut outgoing);
            self.forward_exec(event);
        }
        if self.next_orders_page(read, page, &seen, decoded.next_cursor.as_deref())
            == PageWalk::Complete
        {
            self.probe_missing(&seen, read, &mut outgoing);
            if let OpenOrdersRead::Pass { resync_seq } = read {
                self.settle_pass_read(Some(resync_seq));
            }
        }
        let session = self.dispatch(outgoing, writer).await;
        self.finish_readiness_if_clean();
        session
    }

    /// Asks for the page after this one when the venue says there is one.
    ///
    /// A walk that cannot go on ends here rather than hanging. Ending is not the same as lying about
    /// completeness: every order the read never named is then probed one at a time, so the mirror
    /// still converges on the truth — it just costs a status call each instead of a page.
    fn next_orders_page(
        &mut self,
        read: OpenOrdersRead,
        page: u32,
        seen: &[ClientOrderId],
        cursor: Option<&str>,
    ) -> PageWalk {
        let Some(cursor) = cursor else {
            return PageWalk::Complete;
        };
        if page + 1 >= MAX_PAGES {
            warn!(
                "polymarket execution stopped walking its open orders at {MAX_PAGES} pages — the rest of the account goes unread and its own orders are probed individually"
            );
            return PageWalk::Complete;
        }
        let request = match read.instrument() {
            None => open_orders_page(Some(cursor)),
            Some(instrument) => {
                let Some(binding) = self.tokens.live_binding(instrument) else {
                    warn!(
                        "polymarket execution lost instrument {}'s token binding mid page walk — the remaining pages go unread",
                        instrument.0
                    );
                    return PageWalk::Complete;
                };
                open_orders_for_token(&binding.token_id, Some(cursor))
            }
        };
        match self.submit(
            Lane::Control,
            RestJob {
                purpose: RestPurpose::OpenOrders {
                    read,
                    page: page + 1,
                    seen: seen.to_vec(),
                },
                request,
                auth: Auth::Signed,
            },
        ) {
            Submitted::Queued => PageWalk::Pending,
            Submitted::LaneFull => {
                self.fail_orders_read(read);
                PageWalk::Pending
            }
        }
    }

    /// Only a pass read has a retry to schedule; the other two are asked for again by whatever asks
    /// for them at all.
    pub(super) fn fail_orders_read(&mut self, read: OpenOrdersRead) {
        if let OpenOrdersRead::Pass { resync_seq } = read {
            self.fail_resync(Some(resync_seq));
        }
    }

    /// One of a pass's reads answered. A read belonging to no pass settles nothing, which is the
    /// whole reason membership is a type rather than a sentinel number.
    pub(super) fn settle_pass_read(&mut self, resync_seq: Option<u64>) {
        if let Some(resync_seq) = resync_seq
            && self.resync.on_read(resync_seq)
        {
            self.finish_resync_pass();
        }
    }

    /// An order resting under an id this run never recorded.
    fn on_unmapped_order(&mut self, unmapped: &UnmappedOrder, outgoing: &mut Vec<Outgoing>) {
        if !self.has_opened_quoting {
            self.cancel_prior_run_order(&unmapped.venue_order_id);
            return;
        }
        let verdict = classify_unmapped(self.core.mirror(), unmapped, |client_id| {
            self.orders.venue_order_id(client_id).is_some()
        });
        let client_id = match verdict {
            UnmappedVerdict::Adopt(client_id) => client_id,
            // A placement on this side is still in flight. The resync re-reads once it answers, and
            // by then our own order carries its venue id, so this one is left alone rather than
            // adopted into our slot.
            UnmappedVerdict::Defer => return,
            UnmappedVerdict::LeaveAlone => {
                self.counts.unmapped_left_alone += 1;
                warn!(
                    "polymarket execution left an unmapped order alone on instrument {}: {:?} {} @ {} (venue id {}) — nothing this run placed matches it",
                    unmapped.instrument.0,
                    unmapped.side,
                    unmapped.qty.0,
                    unmapped.price.0,
                    unmapped.venue_order_id
                );
                return;
            }
        };
        self.counts.adopted += 1;
        warn!(
            "polymarket execution adopted venue order {} as {:016x} — its placement answer never arrived",
            unmapped.venue_order_id, client_id.0
        );
        if let Err(error) = self.orders.record(
            &unmapped.venue_order_id,
            KnownOrder {
                client_id,
                instrument: unmapped.instrument,
            },
        ) {
            error!(
                "polymarket execution could not record the adopted order {}: {error}",
                unmapped.venue_order_id
            );
            return;
        }
        let event = self.adopted_event(unmapped, client_id);
        self.fold_mirror(&event, 0, outgoing);
        self.forward_exec(event);
        self.drain_pending(outgoing);
    }

    /// Cancels a prior run's order by its venue id, since there is no client id to mint for
    /// it. Readiness waits for the answer, and quoting never opens over it. Keying on the
    /// venue id rather than the instrument matters because the instrument may still be unbound.
    fn cancel_prior_run_order(&mut self, venue_order_id: &str) {
        let Ok(request) = cancel_venue_order(venue_order_id) else {
            return;
        };
        info!("polymarket execution cancelling a prior run's order {venue_order_id}");
        if self
            .submit(
                Lane::Control,
                RestJob {
                    purpose: RestPurpose::PriorRunCancel {
                        venue_order_id: venue_order_id.into(),
                    },
                    request,
                    auth: Auth::Signed,
                },
            )
            .is_queued()
        {
            self.prior_run_cancels += 1;
        }
    }

    pub(super) fn on_prior_run_cancelled(&mut self, venue_order_id: &str, is_answered: bool) {
        self.prior_run_cancels = self.prior_run_cancels.saturating_sub(1);
        if !is_answered {
            warn!(
                "polymarket execution could not cancel the prior run's order {venue_order_id} — it may still be resting"
            );
        }
        self.finish_readiness_if_clean();
    }

    fn adopted_event(&self, unmapped: &UnmappedOrder, client_id: ClientOrderId) -> ExecEvent {
        let now = self.control.clock.now();
        ExecEvent {
            instrument: unmapped.instrument,
            client_id,
            venue_order_id: Some(venue_order_id_digest(&unmapped.venue_order_id)),
            trade_id: None,
            kind: ExecKind::SnapshotOrder,
            status: Some(unmapped.status),
            reject: None,
            provenance: Provenance::Mine,
            side: unmapped.side,
            liquidity: None,
            price: unmapped.price,
            qty: unmapped.qty,
            last_price: Price(0),
            last_qty: Qty(0),
            cumulative_qty: unmapped.filled,
            cumulative_quote: unmapped.price.notional(unmapped.filled),
            commission: 0,
            commission_asset: AssetId::UNKNOWN,
            reject_code: 0,
            amends_remaining: ExecEvent::AMENDS_EXHAUSTED,
            recon_seq: 0,
            exchange_ts_us: now,
            request_sent_ts_us: None,
            received_ts_us: now,
            queued_ts_us: now,
        }
    }

    /// Absence from the read could mean fill or lag, so each mirrored order the read never named is
    /// probed. Scoped to what the read actually covered: an instrument-scoped page says nothing
    /// about the sibling leg, and probing it would cost one status call per resting order per
    /// reconcile.
    fn probe_missing(
        &mut self,
        seen_owned: &[ClientOrderId],
        read: OpenOrdersRead,
        outgoing: &mut Vec<Outgoing>,
    ) {
        if matches!(read, OpenOrdersRead::FreshBinding { .. }) {
            return;
        }
        let covered = read.instrument();
        let recon_seq = hot_recon_seq(read);
        let missing: Vec<(InstrumentId, ClientOrderId)> = self
            .core
            .mirror()
            .iter()
            .filter(|order| {
                covered.is_none_or(|instrument| order.instrument == instrument)
                    && order.provenance != Provenance::Foreign
                    && !seen_owned.contains(&order.client_id)
            })
            .map(|order| (order.instrument, order.client_id))
            .collect();
        for (instrument, client_id) in missing {
            self.core.mark_ambiguous(client_id);
            self.core.on_command(
                ExecCommand::ReconcileOrder {
                    instrument,
                    client_id,
                    recon_seq,
                },
                &mut |effect| {
                    outgoing.push(Outgoing { effect, recon_seq });
                },
            );
        }
    }

    /// Checks whether the pass has stalled. A stalled pass always ends by retrying or by
    /// dropping the connection, never by hanging.
    pub(super) fn poll_resync(&mut self, now: TsUs) -> Option<SessionOutcome> {
        if self.control.exit.is_some() {
            return None;
        }
        match self.resync.due(now) {
            ResyncStep::Wait => None,
            ResyncStep::Retry => {
                let resync_seq = self.resync.begin_retry(PASS_READS);
                warn!(
                    "polymarket execution re-reading its open orders, fills and balances — attempt {} of {}",
                    self.resync.attempts() + 1,
                    MAX_RESYNC_ATTEMPTS
                );
                self.request_pass_reads(resync_seq);
                None
            }
            ResyncStep::GiveUp => {
                error!(
                    "polymarket execution could not read its state in {MAX_RESYNC_ATTEMPTS} attempts — dropping the stream rather than quoting against a mirror it never verified"
                );
                Some(SessionOutcome::Reconnect)
            }
        }
    }

    /// A read belonging to no pass has no retry to schedule: nothing is waiting on it, and whatever
    /// asks for it asks again on its own cadence.
    pub(super) fn fail_resync(&mut self, resync_seq: Option<u64>) {
        let Some(resync_seq) = resync_seq else {
            return;
        };
        let retry_at = self.control.next_attempt_at(self.resync.attempts());
        self.resync.on_failure(resync_seq, retry_at);
    }

    pub(super) fn finish_resync_pass(&mut self) {
        self.is_readiness_pending = true;
        self.finish_readiness_if_clean();
    }

    /// Quoting opens when no prior-run orders are resting and all prior-run cancels answered.
    pub(super) fn finish_readiness_if_clean(&mut self) {
        if !self.is_readiness_pending
            || self.core.has_prior_run()
            || self.prior_run_cancels > 0
            || !self.core.on_stream_ready()
        {
            return;
        }
        self.is_readiness_pending = false;
        self.has_opened_quoting = true;
        let now = self.control.clock.now();
        for instrument in self.instrument_ids() {
            self.forward_exec(open_orders_snapshot_end(instrument, now));
        }
        info!("polymarket execution quoting — state re-read and nothing inherited is resting");
    }

    /// New bindings may hold orders from before a reconnect. Checked once outside the main pass.
    pub(super) fn read_bound_token(&mut self, instrument: InstrumentId, token_id: &str) {
        // A drop here costs the one-off check; the lane's own warning names it, and the next resync
        // pass reads the same orders account-wide.
        let _dropped = self.submit(
            Lane::Control,
            RestJob {
                purpose: RestPurpose::OpenOrders {
                    read: OpenOrdersRead::FreshBinding { instrument },
                    page: 0,
                    seen: Vec::new(),
                },
                request: open_orders_for_token(token_id, None),
                auth: Auth::Signed,
            },
        );
    }
}

/// Whether a paged read has answered. Only `Complete` settles anything: a page still on the way and
/// a walk that broke are both reads that have not answered yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PageWalk {
    Complete,
    Pending,
}

/// The hot side's reconcile counter, or zero when the read belongs to no hot pass. A resync pass
/// counter must never reach here: the hot side folds this number into the sequence its own sweep
/// compares against, and a foreign value there retires live orders or masks gone ones.
fn hot_recon_seq(read: OpenOrdersRead) -> u64 {
    match read {
        OpenOrdersRead::Instrument { recon_seq, .. } => recon_seq,
        OpenOrdersRead::Pass { .. } | OpenOrdersRead::FreshBinding { .. } => 0,
    }
}
