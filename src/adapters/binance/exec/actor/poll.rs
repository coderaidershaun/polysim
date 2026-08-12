//! The time-driven arms: the command ring, the latches, the deadlines and the REST answers. A
//! timeout goes to reconcile rather than to a retry, because the command may already have executed
//! and the safe move is to ask. A refusal is resent instead, because it is proof that nothing
//! happened.

use crate::adapters::exec::{Outgoing, SessionOutcome, TimeoutFallout, request_timed_out};
use crate::ids::Side;
use crate::time::TsUs;
use crate::warn;

use super::rest::RestJob;
use super::{Actor, Writer};

impl Actor {
    /// Drains the whole command ring and fires it through dispatch, since Binance Spot has
    /// no batch-order endpoint to send it as one request.
    pub(super) async fn on_command_tick(&mut self, writer: &mut Writer) -> Option<SessionOutcome> {
        let outgoing = self.core.drain_commands(&mut self.commands);
        if outgoing.is_empty() {
            return None;
        }
        self.dispatch(outgoing, Some(writer)).await
    }

    pub(super) async fn on_housekeeping(&mut self, writer: &mut Writer) -> Option<SessionOutcome> {
        if self.control.exit.is_none()
            && let Some(reason) = self.control.wanted_exit()
        {
            self.plan_exit(reason, Some(writer)).await;
        }
        let now = self.control.clock.now();
        if let Some(session) = self.expire_requests(now, writer).await {
            return Some(session);
        }
        if let Some(session) = self.poll_resync(now) {
            return Some(session);
        }
        self.retry_sweep(Some(writer)).await;
        let exit = self.control.exit.as_ref()?;
        if self.control.is_swept {
            return Some(SessionOutcome::Swept);
        }
        if now > exit.deadline {
            warn!(
                "binance execution gave up waiting for its {:?} sweep — {} orders are still mirrored as resting",
                exit.reason,
                self.core.mirror().len()
            );
            return Some(SessionOutcome::Swept);
        }
        None
    }

    // A timeout goes to reconcile and is never resent, because resending would double the order.
    async fn expire_requests(&mut self, now: TsUs, writer: &mut Writer) -> Option<SessionOutcome> {
        let expired = self.inflight.take_expired(now);
        if expired.is_empty() {
            return None;
        }
        let mut outgoing = Vec::new();
        let mut session = None;
        for entry in expired {
            let fallout = entry.request.timeout_fallout();
            let TimeoutFallout::OrderInDoubt {
                instrument,
                client_id,
            } = fallout
            else {
                session = session.or(self.on_unanswered_read(fallout));
                continue;
            };
            warn!(
                "binance execution request for order {client_id:016x} went unanswered — reconciling, never retrying",
                client_id = client_id.0
            );
            let side = self
                .mirrored(client_id)
                .map_or(Side::Buy, |order| order.side);
            self.events
                .send_exec(request_timed_out(instrument, client_id, side, now));
            self.core
                .on_request_timeout(instrument, client_id, &mut |effect| {
                    outgoing.push(Outgoing {
                        effect,
                        recon_seq: entry.recon_seq,
                    });
                });
        }
        self.dispatch(outgoing, Some(writer)).await.or(session)
    }

    #[cold]
    fn on_unanswered_read(&self, fallout: TimeoutFallout) -> Option<SessionOutcome> {
        match fallout {
            TimeoutFallout::StreamUnusable => {
                warn!("binance execution subscribe went unanswered — reconnecting");
                Some(SessionOutcome::Reconnect)
            }
            // The hot pass re-asks on its own cadence, so no reconnect is needed here.
            TimeoutFallout::ReadAbandoned => {
                warn!(
                    "binance execution open-orders read went unanswered — waiting for the next pass rather than dropping the connection"
                );
                None
            }
            TimeoutFallout::OrderInDoubt { .. } => None,
        }
    }

    // Runs the reconcile pass and piggybacks the periodic clock resync onto the same tick.
    pub(super) fn on_reconcile_tick(&mut self) -> Option<SessionOutcome> {
        self.reconcile_ticks = self.reconcile_ticks.wrapping_add(1);
        if self
            .reconcile_ticks
            .is_multiple_of(super::CLOCK_RESYNC_TICKS)
        {
            // A refused resync asks again on the next multiple, and the offset already in hand
            // stays usable meanwhile.
            let _ = self.submit(RestJob::SyncClock);
        }
        if self.is_balance_snapshot_due {
            self.request_balance_snapshot();
        }
        for cursor in &self.cursors {
            // The next tick walks from the same cursor, so a refused trade read loses no fill.
            let _ = self.submit(RestJob::MyTrades {
                instrument: cursor.instrument,
                from_id: cursor.from_id,
            });
        }
        None
    }
}
