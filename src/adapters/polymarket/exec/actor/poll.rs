//! Time-driven arms: the command ring, the exit latches, request expiry, the correlation buffer's
//! TTL, the delayed-cancel release, the heartbeat, and the rotation bindings arriving from the
//! market-data actor.

use crate::adapters::exec::{Outgoing, SessionOutcome, TimeoutFallout, request_timed_out};
use crate::adapters::polymarket::rotation::WindowAssignment;
use crate::ids::Side;
use crate::msg::exec::CancelReason;
use crate::time::TsUs;
use crate::{info, warn};

use super::super::binding::{BindingStep, EnrichmentRead, TokenOfInstrument};
use super::super::codec::{cancel_market_orders, clob_market_request, heartbeat, neg_risk_request};
use super::rest::{Auth, Lane, RestJob, RestPurpose};
use super::{Actor, CLOSE_MARGIN, Writer};

impl Actor {
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
        self.clear_park(now);
        self.abandon_expired_frames();
        if let Some(session) = self.release_withheld_cancels(Some(writer)).await {
            return Some(session);
        }
        if let Some(session) = self.expire_requests(now, writer).await {
            return Some(session);
        }
        if let Some(session) = self.poll_resync(now) {
            return Some(session);
        }
        self.send_heartbeat(now);
        self.retry_binding_reads(now);
        if let Some(session) = self.sweep_expiring_windows(writer).await {
            return Some(session);
        }
        self.retry_sweep(Some(writer)).await;
        let exit = self.control.exit.as_ref()?;
        if self.control.is_swept {
            return Some(SessionOutcome::Swept);
        }
        if now > exit.deadline {
            warn!(
                "polymarket execution gave up waiting for its {:?} sweep — {} orders are still mirrored as resting",
                exit.reason,
                self.core.mirror().len()
            );
            return Some(SessionOutcome::Swept);
        }
        None
    }

    fn clear_park(&mut self, now: TsUs) {
        let Some(parked) = self.parked else {
            return;
        };
        if now < parked.until {
            return;
        }
        info!(
            "polymarket execution resuming after venue state {:?}",
            parked.availability
        );
        self.parked = None;
    }

    /// Unanswered requests past deadline are reconciled, never retried (venue may have accepted).
    async fn expire_requests(&mut self, now: TsUs, writer: &mut Writer) -> Option<SessionOutcome> {
        let expired = self.inflight.take_expired(now);
        if expired.is_empty() {
            return None;
        }
        let mut outgoing = Vec::new();
        for entry in expired {
            let TimeoutFallout::OrderInDoubt {
                instrument,
                client_id,
            } = entry.request.timeout_fallout()
            else {
                warn!(
                    "polymarket execution read went unanswered — waiting for the next pass rather than dropping the stream"
                );
                continue;
            };
            warn!(
                "polymarket execution request for order {client_id:016x} went unanswered — reconciling, never retrying",
                client_id = client_id.0
            );
            let side = self
                .mirrored(client_id)
                .map_or(Side::Buy, |order| order.side);
            self.forward_exec(request_timed_out(instrument, client_id, side, now));
            self.core
                .on_request_timeout(instrument, client_id, &mut |effect| {
                    outgoing.push(Outgoing {
                        effect,
                        recon_seq: entry.recon_seq,
                    });
                });
            // Unanswered placement may rest under an id we never learned; resync adoption recovers it.
            if matches!(
                entry.request,
                crate::adapters::exec::ExecRequest::Place { .. }
            ) {
                self.nudge_resync();
            }
        }
        self.dispatch(outgoing, Some(writer)).await
    }

    /// Reads fills to watch for settlement, paced within the venue's read budget.
    pub(super) fn on_reconcile_tick(&mut self) -> Option<SessionOutcome> {
        // A dropped poll costs one settlement look; the next tick is five seconds away.
        let _dropped = self.submit(
            Lane::Control,
            RestJob {
                purpose: RestPurpose::Trades {
                    resync_seq: None,
                    page: 0,
                },
                request: super::super::codec::trades_page(None),
                auth: Auth::Signed,
            },
        );
        None
    }

    /// Arms once orders become possible; once started, the venue cancels the book after 10s
    /// of silence.
    fn send_heartbeat(&mut self, now: TsUs) {
        if !self.heartbeat.is_started {
            if !self.core.phase().admits_new_orders() {
                return;
            }
            self.heartbeat.is_started = true;
            info!(
                "polymarket execution starting its dead-man's-switch heartbeat — the venue will cancel this book if the process stops"
            );
        }
        if now.diff(self.heartbeat.sent_at) < super::HEARTBEAT_PERIOD {
            return;
        }
        let id = self.heartbeat.id.clone().unwrap_or_default();
        let Ok(request) = heartbeat(&id) else {
            return;
        };
        // Stamped only once the beat is on its way. Stamping the attempt would hold the next one off
        // for five seconds against a ten-second deadline, and the venue cancels the whole book when
        // that deadline passes.
        if self
            .submit(
                Lane::Heartbeat,
                RestJob {
                    purpose: RestPurpose::Heartbeat,
                    request,
                    auth: Auth::Signed,
                },
            )
            .is_queued()
        {
            self.heartbeat.sent_at = now;
        }
    }

    /// A backstop that cancels resting orders on windows closing within CLOSE_MARGIN.
    async fn sweep_expiring_windows(&mut self, writer: &mut Writer) -> Option<SessionOutcome> {
        let expiring = self
            .bindings
            .close_margin_reached(self.control.clock.now(), CLOSE_MARGIN);
        if expiring.is_empty() {
            return None;
        }
        let mut session = None;
        for TokenOfInstrument {
            instrument,
            token_id,
        } in expiring
        {
            // In the normal case, the hot engine has already withdrawn on its own margin;
            // this check avoids sweeping an instrument that has nothing left resting.
            let is_resting = self
                .core
                .mirror()
                .iter()
                .any(|order| order.instrument == instrument);
            if !is_resting {
                continue;
            }
            if let Ok(request) = cancel_market_orders(&token_id) {
                warn!(
                    "polymarket execution sweeping instrument {} — its window closes within {}ms and {} order(s) are still mirrored",
                    instrument.0,
                    CLOSE_MARGIN.micros() / 1_000,
                    self.core.mirror().len()
                );
                // The per-order sweep below reaches the same orders, so a dropped bulk cancel costs
                // the shortcut, not the sweep.
                let _dropped = self.submit(
                    Lane::Control,
                    RestJob {
                        purpose: RestPurpose::MarketCancel { instrument },
                        request,
                        auth: Auth::Signed,
                    },
                );
            }
            // The mirror must also learn the orders are gone, so both sides are swept.
            let outgoing = self.sweep_effects(CancelReason::Halt, Some(instrument));
            session = session.or(self.dispatch(outgoing, Some(writer)).await);
        }
        session
    }

    /// A window assignment from the market-data actor; the binding completes only once its
    /// enrichment reads land.
    pub(super) fn on_assignment(
        &mut self,
        assignment: Option<WindowAssignment>,
    ) -> Option<SessionOutcome> {
        let assignment = assignment?;
        info!(
            "polymarket execution binding window {} on condition {}",
            assignment.window_open_ts_us.micros(),
            assignment.condition_id
        );
        let step = self.bindings.on_assignment(&assignment);
        let BindingStep::Enrich {
            condition_id,
            tokens,
        } = step
        else {
            return None;
        };
        // A dropped enrichment read is re-issued by `retry_binding_reads` until the binding
        // completes or gives up, so the drop costs one retry interval.
        let _dropped = self.submit(
            Lane::Control,
            RestJob {
                purpose: RestPurpose::Market {
                    condition_id: std::sync::Arc::clone(&condition_id),
                },
                request: clob_market_request(&condition_id),
                auth: Auth::Public,
            },
        );
        for TokenOfInstrument {
            instrument,
            token_id,
        } in &tokens
        {
            let _dropped = self.submit(
                Lane::Control,
                RestJob {
                    purpose: RestPurpose::NegRisk {
                        condition_id: std::sync::Arc::clone(&condition_id),
                        instrument: *instrument,
                    },
                    request: neg_risk_request(token_id),
                    auth: Auth::Public,
                },
            );
        }
        None
    }

    /// Retries the enrichment reads for bindings still waiting, recovering from transient failures.
    fn retry_binding_reads(&mut self, now: TsUs) {
        for read in self.bindings.due_enrichment_reads(now) {
            let job = match read {
                EnrichmentRead::Market { condition_id } => RestJob {
                    purpose: RestPurpose::Market {
                        condition_id: std::sync::Arc::clone(&condition_id),
                    },
                    request: clob_market_request(&condition_id),
                    auth: Auth::Public,
                },
                EnrichmentRead::NegRisk {
                    condition_id,
                    instrument,
                    token_id,
                } => RestJob {
                    purpose: RestPurpose::NegRisk {
                        condition_id,
                        instrument,
                    },
                    request: neg_risk_request(&token_id),
                    auth: Auth::Public,
                },
            };
            // The same retry that issued this one issues it again next interval.
            let _dropped = self.submit(Lane::Control, job);
        }
    }
}
