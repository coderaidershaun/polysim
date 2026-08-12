//! Adapter between the simulated venue and the production execution lifecycle.

mod account;
mod correlation;
mod handshake;
mod summary;

use account::has_account_update;
use correlation::request_matches_answer;
use summary::SimRunSummary;

use crate::adapters::binance::exec::{DecodeContext, ResponseContext};
use crate::adapters::exec::{
    ExecCore, ExecEffect, ExecRequest, InFlightRequest, LifecycleFold, Phase, PlaceNotSentReason,
    RequestId, amend_not_sent, place_not_sent, stream_reset,
};
use crate::ids::{AssetId, ClientOrderId, InstrumentId};
use crate::msg::exec::{ExecEvent, StampedExecCommand};
use crate::msg::inbound::InboundMessage;
use crate::registry::AssetDictionary;
use crate::shutdown::FatalSignal;
use crate::time::TsUs;

use super::core::schedule::{
    DeliverySchedule, DueAnswer, Rejection, SynthesisedEvent, VenueAnswer, VenueReport, VenueVoice,
};
use super::core::{SimEmission, SimVenue};
use super::wire::{SimFill, VenueWire, response_messages, stream_messages};

/// The two timestamps an effect needs are both venue time and both `TsUs`, so only distinct field
/// names stop a caller swapping them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct EffectStamp {
    /// When a request the effect sends reaches the matching model.
    effective_ts_us: TsUs,
    /// What the driver stamps on events it synthesises for the engine.
    reported_ts_us: TsUs,
    recon_seq: u64,
}

impl EffectStamp {
    const fn at(ts_us: TsUs) -> Self {
        Self {
            effective_ts_us: ts_us,
            reported_ts_us: ts_us,
            recon_seq: 0,
        }
    }
}

enum Wired {
    Response {
        payloads: Vec<String>,
        request: ExecRequest,
        recon_seq: u64,
        sent_at: TsUs,
    },
    Stream(Vec<String>),
    Event(ExecEvent),
    Silent,
}

pub struct SimExecDriver {
    core: ExecCore,
    venue: SimVenue,
    schedule: DeliverySchedule,
    wire: VenueWire,
    instrument: InstrumentId,
    inflight: Vec<InFlightRequest>,
    edge_events: Vec<ExecEvent>,
    emissions: Vec<SimEmission>,
    due: Vec<DueAnswer>,
    has_opened_quoting: bool,
    has_swept: bool,
    summary: SimRunSummary,
}

impl SimExecDriver {
    pub fn new(setup: SimExecDriverSetup) -> Self {
        Self {
            core: setup.core,
            venue: setup.venue,
            schedule: setup.schedule,
            wire: setup.wire,
            instrument: setup.instrument,
            inflight: Vec::new(),
            edge_events: Vec::new(),
            emissions: Vec::new(),
            due: Vec::new(),
            has_opened_quoting: false,
            has_swept: false,
            summary: SimRunSummary::default(),
        }
    }

    pub fn on_command(
        &mut self,
        stamped: StampedExecCommand,
        effective_ts_us: TsUs,
        fatal: &FatalSignal,
    ) {
        let mut effects = Vec::new();
        self.core
            .on_command(stamped.command, &mut |effect| effects.push(effect));
        self.apply(
            &effects,
            EffectStamp {
                effective_ts_us,
                reported_ts_us: stamped.issued_ts_us,
                recon_seq: stamped.command.recon_seq(),
            },
            fatal,
        );
    }

    pub fn advance_to(
        &mut self,
        horizon: TsUs,
        effect_ts_us: TsUs,
        context: SimDriverContext<'_>,
    ) -> Vec<InboundMessage> {
        let Self {
            venue,
            schedule,
            summary,
            emissions,
            ..
        } = self;
        venue.advance_to_watermark(horizon, emissions);
        for emission in emissions.iter() {
            summary.observe(emission);
            schedule.accept(emission);
        }
        let mut forwarded: Vec<InboundMessage> = std::mem::take(&mut self.edge_events)
            .into_iter()
            .map(InboundMessage::Exec)
            .collect();
        let mut due = std::mem::take(&mut self.due);
        self.schedule.advance_to(horizon, &mut due);
        for answer in &due {
            let decode = DecodeContext {
                received_ts_us: answer.due_ts_us,
                ..context.decode
            };
            let has_account_update = has_account_update(&answer.voice);
            let messages = self.decode(&answer.voice, answer.event_ts_us, decode);
            self.fold(
                &messages,
                EffectStamp {
                    effective_ts_us: effect_ts_us,
                    reported_ts_us: answer.due_ts_us,
                    recon_seq: 0,
                },
                context.fatal,
            );
            forwarded.extend(messages);
            if has_account_update {
                forwarded.extend(self.settlement_messages(answer.due_ts_us, decode));
            }
        }
        self.due = due;
        forwarded
    }

    pub fn begin_sweep(
        &mut self,
        reason: crate::msg::exec::CancelReason,
        at_ts_us: TsUs,
        fatal: &FatalSignal,
    ) {
        let mut effects = Vec::new();
        self.core
            .begin_sweep(reason, None, &mut |effect| effects.push(effect));
        let cancelling: Vec<ClientOrderId> = effects
            .iter()
            .filter_map(|effect| match effect {
                ExecEffect::Send {
                    request: ExecRequest::Cancel { client_id, .. },
                    ..
                } => Some(*client_id),
                _ => None,
            })
            .collect();
        self.apply(&effects, EffectStamp::at(at_ts_us), fatal);
        let exited = self.venue.force_exit_open_orders(at_ts_us, &cancelling);
        self.schedule.force_sweep(at_ts_us, &exited);
    }

    pub fn venue(&self) -> &SimVenue {
        &self.venue
    }

    pub fn venue_mut(&mut self) -> &mut SimVenue {
        &mut self.venue
    }

    pub fn is_swept(&self) -> bool {
        self.has_swept
    }

    fn apply(&mut self, effects: &[ExecEffect], stamp: EffectStamp, fatal: &FatalSignal) {
        for effect in effects {
            match *effect {
                ExecEffect::Send {
                    request_id,
                    request,
                } => self.send(request_id, request, stamp),
                ExecEffect::PlaceNotSent {
                    instrument,
                    client_id,
                    side,
                    reason,
                } => self.refuse_placement(
                    instrument,
                    client_id,
                    side,
                    reason,
                    stamp.reported_ts_us,
                    fatal,
                ),
                ExecEffect::AmendNotSent {
                    instrument,
                    client_id,
                } => self.edge_events.push(amend_not_sent(
                    instrument,
                    client_id,
                    stamp.reported_ts_us,
                )),
                ExecEffect::SweepComplete { .. } => self.has_swept = true,
                ExecEffect::Skipped { .. } => {}
            }
        }
    }

    fn refuse_placement(
        &mut self,
        instrument: InstrumentId,
        client_id: ClientOrderId,
        side: crate::ids::Side,
        reason: PlaceNotSentReason,
        reported_ts_us: TsUs,
        fatal: &FatalSignal,
    ) {
        self.core.on_place_not_sent(client_id);
        self.edge_events
            .push(place_not_sent(instrument, client_id, side, reported_ts_us));
        if reason.is_fatal() {
            fatal.trip(format!(
                "simulated execution placement admission failed closed: {reason:?}"
            ));
        }
    }

    fn send(&mut self, request_id: RequestId, request: ExecRequest, stamp: EffectStamp) {
        if !matches!(request, ExecRequest::SubscribeUserStream) {
            // `sent_at` is venue time, never a clock reading: replaying the same tape has to
            // reproduce the same run.
            self.inflight.push(InFlightRequest {
                request_id,
                request,
                sent_at: stamp.effective_ts_us,
                recon_seq: stamp.recon_seq,
            });
        }
        self.venue.on_request(&request, stamp.effective_ts_us);
    }

    fn fold(&mut self, messages: &[InboundMessage], stamp: EffectStamp, fatal: &FatalSignal) {
        self.has_opened_quoting |= self.core.phase() == Phase::Quoting;
        let mut effects = Vec::new();
        for message in messages {
            let InboundMessage::Exec(event) = message else {
                continue;
            };
            LifecycleFold {
                core: &mut self.core,
                fatal,
                has_opened_quoting: self.has_opened_quoting,
            }
            .on_event(event, &mut |effect| effects.push(effect));
        }
        self.apply(&effects, stamp, fatal);
    }

    fn decode(
        &mut self,
        voice: &VenueVoice,
        event_ts_us: TsUs,
        decode: DecodeContext<'_>,
    ) -> Vec<InboundMessage> {
        match self.mint(voice, event_ts_us, decode) {
            Wired::Response {
                payloads,
                request,
                recon_seq,
                sent_at,
            } => {
                let mut answers = response_messages(
                    &payloads,
                    &ResponseContext {
                        decode,
                        request,
                        recon_seq,
                    },
                );
                for answer in &mut answers {
                    if let InboundMessage::Exec(event) = answer {
                        event.request_sent_ts_us = Some(sent_at);
                    }
                }
                answers
            }
            Wired::Stream(payloads) => stream_messages(&payloads, decode),
            Wired::Event(event) => vec![InboundMessage::Exec(event)],
            Wired::Silent => Vec::new(),
        }
    }

    fn mint(&mut self, voice: &VenueVoice, event_ts_us: TsUs, decode: DecodeContext<'_>) -> Wired {
        let at_ts_us = decode.received_ts_us;
        match voice {
            VenueVoice::Response(answer) => self.mint_answer(answer, event_ts_us),
            VenueVoice::Report(report) => {
                Wired::Stream(vec![self.mint_report(report, event_ts_us, decode.assets)])
            }
            VenueVoice::Synthesised(SynthesisedEvent::StreamReset) => {
                Wired::Event(stream_reset(at_ts_us))
            }
            VenueVoice::Synthesised(SynthesisedEvent::PlaceNotSent(order)) => Wired::Event(
                place_not_sent(self.instrument, order.client_id, order.side, at_ts_us),
            ),
            VenueVoice::Synthesised(SynthesisedEvent::StreamSubscribed) => Wired::Silent,
        }
    }

    fn mint_answer(&mut self, answer: &VenueAnswer, event_ts_us: TsUs) -> Wired {
        let Some(held) = self.take_inflight(answer) else {
            return Wired::Silent;
        };
        let wire = self.wire.at(event_ts_us);
        let payloads = match answer {
            VenueAnswer::PlaceAccepted(order) => vec![wire.place_ack(order)],
            VenueAnswer::CancelAccepted(order) => vec![wire.cancel_ack(order)],
            VenueAnswer::AmendAccepted(order) => vec![wire.amend_ack(order)],
            VenueAnswer::Status { order, status } => {
                vec![wire.order_status_as(order, *status)]
            }
            VenueAnswer::OpenOrders(rows) => vec![wire.open_orders(rows)],
            VenueAnswer::Refused { rejection, .. } => vec![self.mint_refusal(*rejection)],
        };
        Wired::Response {
            payloads,
            request: held.request,
            recon_seq: held.recon_seq,
            sent_at: held.sent_at,
        }
    }

    fn mint_report(
        &self,
        report: &VenueReport,
        event_ts_us: TsUs,
        assets: &AssetDictionary,
    ) -> String {
        let wire = self.wire.at(event_ts_us);
        match report {
            VenueReport::New(order) => wire.new_report(order),
            VenueReport::Trade {
                order,
                trade_id,
                settlement,
            } => wire.trade_report(
                order,
                SimFill {
                    trade_id: *trade_id,
                    settlement,
                    fee_asset: match settlement.fee {
                        0 => "",
                        _ => asset_name(assets, settlement.fee_asset),
                    },
                },
            ),
            VenueReport::Canceled(order) => wire.cancel_report(order),
            VenueReport::Rejected(order) => wire.rejected_report(order),
        }
    }

    fn mint_refusal(&self, rejection: Rejection) -> String {
        match rejection {
            Rejection::WouldMatchImmediately => self.wire.would_match_error(),
            Rejection::InsufficientBalance => self.wire.insufficient_balance_error(),
            Rejection::FilterFailure | Rejection::TooManyOrders => self.wire.filter_failure_error(),
            Rejection::CancelRejected => self.wire.unknown_order_error(),
            Rejection::NoSuchOrder => self.wire.no_such_order_error(),
            Rejection::AmendBudgetSpent => self.wire.amend_budget_spent_error(),
            Rejection::AmendQuantityIncrease => self.wire.amend_quantity_increase_error(),
            Rejection::AmendFilterFailure => self.wire.amend_filter_failure_error(),
        }
    }

    fn take_inflight(&mut self, answer: &VenueAnswer) -> Option<InFlightRequest> {
        let at = self
            .inflight
            .iter()
            .position(|held| request_matches_answer(held.request, answer))?;
        Some(self.inflight.remove(at))
    }
}

pub struct SimExecDriverSetup {
    pub core: ExecCore,
    pub venue: SimVenue,
    pub schedule: DeliverySchedule,
    pub wire: VenueWire,
    pub instrument: InstrumentId,
}

#[derive(Clone, Copy)]
pub struct SimDriverContext<'a> {
    pub decode: DecodeContext<'a>,
    pub fatal: &'a FatalSignal,
}

fn asset_name(assets: &AssetDictionary, asset: AssetId) -> &str {
    assets
        .name(asset)
        .unwrap_or_else(|| panic!("simulated wallet names unknown asset {}", asset.0))
}

fn venue_millis(at_ts_us: TsUs) -> u64 {
    u64::try_from(at_ts_us.micros() / 1_000).unwrap_or_default()
}
