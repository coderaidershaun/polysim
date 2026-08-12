//! Folds the engine's decisions and the venue's answers, and routes each one over the socket or
//! over REST.

use futures_util::SinkExt;
use tokio_tungstenite::tungstenite::Message;

use crate::adapters::exec::{
    ExecEffect, ExecRequest, LifecycleFold, MirroredOrder, Outgoing, PlaceNotSentReason, RequestId,
    SessionOutcome, SkipReason, TimeoutFallout, amend_not_sent, place_not_sent, request_timed_out,
};
use crate::ids::{ClientOrderId, InstrumentId, Side};
use crate::msg::exec::{CancelReason, ExecEvent};
use crate::{error, info, warn};

use super::super::EncodeContext;
use super::frame::FrameCredentials;
use super::rest::RestJob;
use super::{Actor, RECENT_ORDERS, RECENT_TRADES, RecentOrder, Writer, frame};

/// Three outcomes the caller must tell apart: bytes are queued and want a flush, nothing was sent
/// and the request has already been refused, or the socket is gone and the session with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeedOutcome {
    Fed,
    NotSent,
    SocketDead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    Socket,
    Rest,
    Refused,
}

fn transport_for(request: ExecRequest, has_socket: bool) -> Transport {
    match request {
        // Place and amend need the socket.
        ExecRequest::Place { .. }
        | ExecRequest::AmendQty { .. }
        | ExecRequest::SubscribeUserStream => match has_socket {
            true => Transport::Socket,
            false => Transport::Refused,
        },
        // Cancel and the reads are fine over REST.
        ExecRequest::Cancel { .. }
        | ExecRequest::OrderStatus { .. }
        | ExecRequest::OpenOrders { .. } => match has_socket {
            true => Transport::Socket,
            false => Transport::Rest,
        },
    }
}

impl Actor {
    pub(super) async fn plan_exit(&mut self, reason: CancelReason, writer: Option<&mut Writer>) {
        if !self.control.open_exit(reason) {
            return;
        }
        warn!("binance execution pulling every order this run owns: {reason:?}");
        let mut outgoing = Vec::new();
        self.core.begin_sweep(reason, None, &mut |effect| {
            outgoing.push(Outgoing {
                effect,
                recon_seq: 0,
            });
        });
        self.dispatch(outgoing, writer).await;
    }

    // Retries are paced to avoid tripping the venue's IP ban, and the latch filters out
    // cancels that already have a sweep request in flight.
    pub(super) async fn retry_sweep(&mut self, writer: Option<&mut Writer>) {
        let Some(reason) = self.control.claim_sweep_retry() else {
            return;
        };
        let mut outgoing = Vec::new();
        self.core.begin_sweep(reason, None, &mut |effect| {
            outgoing.push(Outgoing {
                effect,
                recon_seq: 0,
            });
        });
        self.dispatch(outgoing, writer).await;
    }

    // Binance Spot has no batch-order endpoint, so requests go out one at a time and
    // share a single flush instead.
    pub(super) async fn dispatch(
        &mut self,
        outgoing: Vec<Outgoing>,
        mut writer: Option<&mut Writer>,
    ) -> Option<SessionOutcome> {
        let mut is_fed = false;
        let mut session = None;
        for Outgoing { effect, recon_seq } in outgoing {
            match effect {
                ExecEffect::Send {
                    request_id,
                    request,
                } => match transport_for(request, writer.is_some()) {
                    Transport::Socket => match writer.as_deref_mut() {
                        Some(writer) => {
                            match self.feed(request_id, request, recon_seq, writer).await {
                                FeedOutcome::Fed => is_fed = true,
                                FeedOutcome::NotSent => {}
                                FeedOutcome::SocketDead => {
                                    session = Some(SessionOutcome::Reconnect);
                                }
                            }
                        }
                        None => session = Some(SessionOutcome::Reconnect),
                    },
                    Transport::Rest => self.submit_over_rest(request, recon_seq),
                    Transport::Refused => {
                        self.report_refused(request, PlaceNotSentReason::NoTransport)
                    }
                },
                ExecEffect::Skipped { client_id, reason } => self.on_skipped(client_id, reason),
                ExecEffect::PlaceNotSent {
                    instrument,
                    client_id,
                    side,
                    reason,
                } => self.on_place_not_sent(instrument, client_id, side, reason),
                ExecEffect::AmendNotSent {
                    instrument,
                    client_id,
                } => self.on_amend_not_sent(instrument, client_id),
                ExecEffect::SweepComplete { reason } => self.on_sweep_complete(reason),
            }
        }
        if is_fed
            && let Some(writer) = writer
            && writer.flush().await.is_err()
        {
            warn!("binance execution could not flush its request batch — reconnecting");
            session = Some(SessionOutcome::Reconnect);
        }
        session
    }

    async fn feed(
        &mut self,
        request_id: RequestId,
        request: ExecRequest,
        recon_seq: u64,
        writer: &mut Writer,
    ) -> FeedOutcome {
        let now = self.control.clock.now();
        let frame = {
            let context = EncodeContext {
                symbols: &self.symbols,
                identity: self.identity,
            };
            frame::frame_request(
                request_id,
                request,
                &context,
                FrameCredentials {
                    signer: &self.signer,
                    api_key: &self.api_key,
                    recv_window: self.recv_window,
                    stamp: self.clock_offset.stamp(now),
                },
            )
        };
        let frame = match frame {
            Ok(frame) => frame,
            Err(error) => {
                error!("binance execution could not build a request: {error}");
                self.report_refused(request, PlaceNotSentReason::Encoding);
                return FeedOutcome::NotSent;
            }
        };
        if writer.feed(Message::Text(frame.into())).await.is_err() {
            self.mark_transport_ambiguous(request);
            return FeedOutcome::SocketDead;
        }
        self.inflight.record(request_id, request, now, recon_seq);
        FeedOutcome::Fed
    }

    fn submit_over_rest(&mut self, request: ExecRequest, recon_seq: u64) {
        let job = match request {
            ExecRequest::Cancel {
                instrument,
                client_id,
            } => RestJob::Cancel {
                instrument,
                client_id,
                recon_seq,
            },
            ExecRequest::OrderStatus {
                instrument,
                client_id,
            } => RestJob::OrderStatus {
                instrument,
                client_id,
                recon_seq,
            },
            // resync_seq stays 0 because this request comes from the hot pass, not a
            // background resync sweep.
            ExecRequest::OpenOrders { instrument } => RestJob::OpenOrders {
                instrument,
                resync_seq: 0,
            },
            other => return self.report_refused(other, PlaceNotSentReason::NoTransport),
        };
        if self.submit(job) {
            return;
        }
        // The cancel latch re-arms so the next sweep pass retries this order.
        if let RestJob::Cancel { client_id, .. } = job {
            self.core.re_arm_cancel(client_id);
        }
    }

    #[cold]
    fn report_refused(&mut self, request: ExecRequest, reason: PlaceNotSentReason) {
        match request {
            ExecRequest::Place {
                instrument,
                client_id,
                side,
                ..
            } => {
                self.core.on_place_not_sent(client_id);
                self.on_place_not_sent(instrument, client_id, side, reason);
                return;
            }
            ExecRequest::Cancel { client_id, .. } => self.core.re_arm_cancel(client_id),
            ExecRequest::AmendQty {
                instrument,
                client_id,
                ..
            } => {
                self.on_amend_not_sent(instrument, client_id);
                return;
            }
            _ => {}
        }
        warn!(
            "binance execution has no connection for {:?} — refusing rather than guessing",
            std::mem::discriminant(&request)
        );
    }

    /// The order is untouched — the amend never left — so this reports only that the engine may stop
    /// waiting. Without it the hot slot stays mid-amend until its in-flight timeout, and the side
    /// quotes around a size change nobody was ever asked for.
    fn on_amend_not_sent(&mut self, instrument: InstrumentId, client_id: ClientOrderId) {
        warn!(
            "binance execution proved amend of {:016x} was not sent — the order rests unchanged",
            client_id.0
        );
        self.events.send_exec(amend_not_sent(
            instrument,
            client_id,
            self.control.clock.now(),
        ));
    }

    fn on_place_not_sent(
        &mut self,
        instrument: InstrumentId,
        client_id: ClientOrderId,
        side: Side,
        reason: PlaceNotSentReason,
    ) {
        warn!(
            "binance execution proved placement {:016x} was not sent ({reason:?})",
            client_id.0
        );
        self.events.send_exec(place_not_sent(
            instrument,
            client_id,
            side,
            self.control.clock.now(),
        ));
        if reason.is_fatal() {
            self.control.fatal.trip(format!(
                "binance execution placement admission failed closed: {reason:?}"
            ));
        }
    }

    // The request may already have put bytes on the wire, so its outcome is ambiguous.
    pub(super) fn mark_transport_ambiguous(&mut self, request: ExecRequest) {
        let TimeoutFallout::OrderInDoubt {
            instrument,
            client_id,
        } = request.timeout_fallout()
        else {
            return;
        };
        let now = self.control.clock.now();
        let side = self
            .mirrored(client_id)
            .map_or(Side::Buy, |order| order.side);
        self.core.mark_ambiguous(client_id);
        self.events
            .send_exec(request_timed_out(instrument, client_id, side, now));
    }

    fn on_skipped(&mut self, client_id: Option<ClientOrderId>, reason: SkipReason) {
        let described = client_id.and_then(|id| self.describe(id));
        match reason {
            SkipReason::ForeignOrder => warn!(
                "binance execution left a foreign order alone: {} — the engine never cancels an order it did not place",
                described.unwrap_or_else(|| "unknown order".to_owned())
            ),
            // A symptom of the cancel latch, so it is counted and warned about.
            SkipReason::AlreadyCancelling => {
                self.counts.cancels_skipped += 1;
                warn!(
                    "binance execution already has a cancel in flight for {} ({} skipped so far)",
                    described.unwrap_or_else(|| "an unknown order".to_owned()),
                    self.counts.cancels_skipped
                );
            }
        }
    }

    fn on_sweep_complete(&mut self, reason: CancelReason) {
        info!("binance execution swept every order it owns: {reason:?}");
        self.control.is_swept = true;
    }

    fn describe(&self, client_id: ClientOrderId) -> Option<String> {
        let order = self.mirrored(client_id)?;
        Some(format!(
            "{:?} {} {} @ {} (client {:016x})",
            order.side,
            order.qty.0,
            self.symbols.symbol(order.instrument).unwrap_or("?"),
            order.price.0,
            client_id.0
        ))
    }

    pub(super) fn mirrored(&self, client_id: ClientOrderId) -> Option<&MirroredOrder> {
        self.core
            .mirror()
            .iter()
            .find(|order| order.client_id == client_id)
    }

    pub(super) fn fold_mirror(
        &mut self,
        event: &ExecEvent,
        recon_seq: u64,
        outgoing: &mut Vec<Outgoing>,
    ) {
        LifecycleFold {
            core: &mut self.core,
            fatal: &self.control.fatal,
            has_opened_quoting: self.has_opened_quoting,
        }
        .on_event(event, &mut |effect| {
            outgoing.push(Outgoing { effect, recon_seq });
        });
    }

    // Tracked for the reconciler: the venue order id to client id mapping, and the fill ids.
    pub(super) fn note_event(&mut self, event: &ExecEvent) {
        if let Some(venue_id) = event.venue_order_id
            && event.client_id != ClientOrderId(0)
            && !self
                .recent_orders
                .iter()
                .any(|order| order.venue_id == venue_id)
        {
            if self.recent_orders.len() == RECENT_ORDERS {
                self.recent_orders.remove(0);
            }
            self.recent_orders.push(RecentOrder {
                venue_id,
                client_id: event.client_id,
            });
        }
        if let Some(trade_id) = event.trade_id {
            if self.recent_trades.len() == RECENT_TRADES {
                self.recent_trades.remove(0);
            }
            self.recent_trades.push(trade_id);
        }
    }
}
