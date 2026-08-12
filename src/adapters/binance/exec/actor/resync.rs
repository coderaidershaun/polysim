//! The quoting admission gate: it reads the open orders for each instrument. A pass that stalls
//! ends by retrying or by reconnecting, never by wedging.

use crate::adapters::binance::rest::OrderRecord;
use crate::adapters::exec::{
    MAX_RESYNC_ATTEMPTS, Outgoing, ResyncStep, SessionOutcome, mirrored_order,
    open_orders_snapshot_end,
};
use crate::msg::exec::{ExecCommand, ExecKind, Provenance};
use crate::time::TsUs;
use crate::{error, info, warn};

use super::rest::RestJob;
use super::{Actor, Writer};

impl Actor {
    /// Asks every instrument for its open orders. This actor owns this pass itself, separate
    /// from hot-table reconciliation.
    pub(super) fn start_resync(&mut self) {
        self.is_readiness_pending = false;
        let resync_seq = self.resync.begin(self.pass_reads());
        self.request_pass_reads(resync_seq);
    }

    /// Counts the reads a full pass needs: open orders for every instrument, plus balances.
    /// Both gate quoting, and a reconnect needs a fresh snapshot of each.
    pub(super) fn pass_reads(&self) -> usize {
        self.instruments.len() + 1
    }

    pub(super) async fn on_open_orders(
        &mut self,
        job: RestJob,
        orders: &[OrderRecord],
        writer: Option<&mut Writer>,
    ) {
        let RestJob::OpenOrders {
            instrument,
            resync_seq,
        } = job
        else {
            return;
        };
        let mut seen_owned = Vec::new();
        for order in orders {
            // The codec already de-dupes, and this pass only ever decodes as SnapshotOrder —
            // it never needs a SnapshotEnd, because it feeds the mirror, not the hot table.
            // An order this build cannot read leaves the mirror incomplete, and quoting against an
            // incomplete mirror is what strands somebody's order.
            let Some(event) = self.decode_order(order, ExecKind::SnapshotOrder, 0) else {
                self.core.stop_quoting();
                self.fail_resync(resync_seq);
                self.control.fatal.trip(format!(
                    "binance execution cannot decode a possibly-owned open order {}",
                    order.order_id
                ));
                return;
            };
            let mut mirrored = mirrored_order(&event);
            if !self.has_opened_quoting && mirrored.provenance != Provenance::Foreign {
                // On a cold start, quoting has not opened yet, so any order already resting
                // must predate this process — that timing decides it, not the nonce in its
                // client id.
                mirrored.provenance = Provenance::PriorRun;
            }
            match mirrored.provenance {
                // Foreign orders are never mirrored, and since none carry this engine's
                // client id, the operator is warned directly instead.
                Provenance::Foreign => warn!(
                    "binance execution sees a foreign order resting on {}: {:?} {} @ {} — it will never be cancelled by this engine",
                    self.symbols.symbol(mirrored.instrument).unwrap_or("?"),
                    mirrored.side,
                    mirrored.qty.0,
                    mirrored.price.0
                ),
                provenance => {
                    seen_owned.push(mirrored.client_id);
                    info!(
                        "binance execution mirroring a resting {provenance:?} order {:016x}",
                        mirrored.client_id.0
                    );
                    if let Err(error) = self.core.observe_venue_order(mirrored) {
                        error!(
                            "binance execution cannot retain venue order {:016x} in its possibly-live set ({error:?}) — quoting is disabled",
                            mirrored.client_id.0
                        );
                        self.control.fatal.trip(format!(
                            "binance execution possibly-live mirror failed: {error:?}"
                        ));
                        self.fail_resync(resync_seq);
                        return;
                    }
                }
            }
        }
        // An order missing from this read may simply be gone, or the read may have raced or
        // lagged a fill, so each missing order is probed rather than assumed gone.
        if resync_seq != 0 {
            let missing: Vec<_> = self
                .core
                .mirror()
                .iter()
                .filter(|order| {
                    order.instrument == instrument
                        && order.provenance != Provenance::Foreign
                        && !seen_owned.contains(&order.client_id)
                })
                .map(|order| order.client_id)
                .collect();
            for client_id in missing {
                self.core.mark_ambiguous(client_id);
                if !self.submit(super::rest::RestJob::OrderStatus {
                    instrument,
                    client_id,
                    recon_seq: resync_seq,
                }) {
                    self.fail_resync(resync_seq);
                }
            }
        }
        // Every order is now accounted for in the mirror, mirrored or already there, so all
        // that remains is to retire this pass.
        if !self.resync.on_read(resync_seq) {
            return;
        }
        self.finish_resync(writer).await;
    }

    /// Advances the resync pass one tick. A stalled pass always ends by retrying or
    /// reconnecting, never by wedging.
    pub(super) fn poll_resync(&mut self, now: TsUs) -> Option<SessionOutcome> {
        // On the exit path the admission read is moot, and a reconnect starts a clean pass anyway.
        if self.control.exit.is_some() {
            return None;
        }
        match self.resync.due(now) {
            ResyncStep::Wait => None,
            ResyncStep::Retry => {
                let resync_seq = self.resync.begin_retry(self.pass_reads());
                warn!(
                    "binance execution re-reading its open orders and balances — attempt {} of {}",
                    self.resync.attempts() + 1,
                    MAX_RESYNC_ATTEMPTS
                );
                self.request_pass_reads(resync_seq);
                None
            }
            // Stalled reads force a reconnect. Holding on would only postpone the same death by
            // 23 hours, so the connection ends now.
            ResyncStep::GiveUp => {
                error!(
                    "binance execution could not read its open orders and balances in {MAX_RESYNC_ATTEMPTS} attempts — dropping the connection rather than quoting against a mirror it never verified"
                );
                Some(SessionOutcome::Reconnect)
            }
        }
    }

    /// A failed read and a refused queue both fail the pass. Backoff paces the retry.
    pub(super) fn fail_resync(&mut self, resync_seq: u64) {
        let retry_at = self.control.next_attempt_at(self.resync.attempts());
        self.resync.on_failure(resync_seq, retry_at);
    }

    fn request_pass_reads(&mut self, resync_seq: u64) {
        let mut is_queued = self.submit(RestJob::Account { resync_seq });
        for instrument in self.instruments.clone() {
            is_queued &= self.submit(RestJob::OpenOrders {
                instrument,
                resync_seq,
            });
        }
        if is_queued {
            return;
        }
        self.fail_resync(resync_seq);
    }

    pub(super) async fn finish_resync(&mut self, writer: Option<&mut Writer>) {
        self.is_readiness_pending = true;
        let mut outgoing = Vec::new();
        for instrument in self.instruments.clone() {
            self.core
                .on_command(ExecCommand::CancelPriorRun { instrument }, &mut |effect| {
                    outgoing.push(Outgoing {
                        effect,
                        recon_seq: 0,
                    });
                });
        }
        self.dispatch(outgoing, writer).await;
        self.finish_readiness_if_clean();
    }

    pub(super) fn finish_readiness_if_clean(&mut self) {
        if !self.is_readiness_pending || self.core.has_prior_run() {
            return;
        }
        if !self.core.on_stream_ready() {
            return;
        }
        self.is_readiness_pending = false;
        self.has_opened_quoting = true;
        let now = self.control.clock.now();
        for instrument in self.instruments.clone() {
            self.events
                .send_exec(open_orders_snapshot_end(instrument, now));
        }
    }
}
