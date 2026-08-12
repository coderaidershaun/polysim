//! Pure simulated-venue state transitions.

pub(crate) mod latency;
pub(crate) mod market;
pub(crate) mod ordering;
/// Public by decision: an order is what the wire contract describes, so anyone holding that
/// contract to account needs to name one.
pub mod orders;
pub(crate) mod queue;
pub(crate) mod request;
pub(crate) mod resting;
mod revise;
pub(crate) mod schedule;
/// Public by decision: balances and fee settlement are the other half of what the wire contract
/// carries, and a fill payload cannot be checked without the numbers behind it.
pub mod wallet;

use market::{BookVerdict, MarketFold, ResetReason, TradeEvidence, TradeVerdict};
use ordering::{BufferedMarket, MarketBuffer, Phase, Timeline};
use queue::{OwnFill, SimOrderIndex};
use request::{AdmitPlan, RequestFold, TimedAction, close_order};
use resting::{
    ClosedReason, InstrumentLimits, OrderPhase, OrderSnapshot, RefusalReason, RestingOrders,
};
pub(crate) use revise::ForcedOrderExit;
use wallet::{FillSettlement, SimWallet, SimWalletSetup, WalletError};

use crate::adapters::exec::ExecRequest;
use crate::ids::{ClientOrderId, InstrumentId, Price, Qty, TradeId};
use crate::msg::inbound::{BookChunkKind, InboundMessage, TappedMessage, VenueMeta};
use crate::time::{DurationUs, TsUs};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SimEmission {
    pub at_ts_us: TsUs,
    pub sequence: u64,
    pub event: VenueEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum VenueEvent {
    Rested {
        snapshot: OrderSnapshot,
        queue_ahead: queue::QueueAhead,
    },
    PostOnlyCrossed {
        snapshot: OrderSnapshot,
    },
    PlaceRefused {
        snapshot: OrderSnapshot,
        reason: RefusalReason,
    },
    Filled {
        snapshot: OrderSnapshot,
        last_qty: Qty,
        last_price: Price,
        trade_id: TradeId,
        settlement: FillSettlement,
    },
    Canceled {
        snapshot: OrderSnapshot,
    },
    CancelRefused {
        client_id: ClientOrderId,
        reason: RefusalReason,
    },
    Amended {
        snapshot: OrderSnapshot,
        total_qty: Qty,
    },
    AmendRefused {
        client_id: ClientOrderId,
        reason: RefusalReason,
    },
    OrderStatus {
        snapshot: OrderSnapshot,
    },
    NoSuchOrder {
        client_id: ClientOrderId,
    },
    OpenOrders {
        rows: Vec<OrderSnapshot>,
    },
    StreamSubscribed,
    MarketReset {
        reason: ResetReason,
    },
}

#[derive(Debug, Clone, Default)]
pub(crate) struct EmissionBook {
    pending: Vec<SimEmission>,
    sequence: u64,
}

impl EmissionBook {
    pub fn push(&mut self, at_ts_us: TsUs, event: VenueEvent) {
        self.pending.push(SimEmission {
            at_ts_us,
            sequence: self.sequence,
            event,
        });
        self.sequence += 1;
    }

    fn drain_through(&mut self, horizon: TsUs, out: &mut Vec<SimEmission>) {
        self.pending
            .sort_by_key(|emission| (emission.at_ts_us, emission.sequence));
        let split = self
            .pending
            .partition_point(|emission| emission.at_ts_us <= horizon);
        out.extend(self.pending.drain(..split));
    }
}

#[derive(Debug)]
pub(crate) struct SimVenue {
    market: MarketFold,
    inbox: MarketBuffer,
    orders: RestingOrders,
    limits: InstrumentLimits,
    timeline: Timeline,
    emissions: EmissionBook,
    wallet: SimWallet,
    verdict_retention: DurationUs,
    reap_due_ts_us: Option<TsUs>,
    reset_announced: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SimVenueSetup {
    pub instrument: InstrumentId,
    pub book_capacity: usize,
    pub market_inbox_capacity: usize,
    pub verdict_retention: DurationUs,
    pub limits: InstrumentLimits,
    pub wallet: SimWalletSetup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Market,
    Scheduled,
}

impl SimVenue {
    /// # Errors
    /// [`WalletError`] — a negative opening balance. The fee range is already checked by `FeeBps`.
    pub fn new(setup: SimVenueSetup) -> Result<Self, WalletError> {
        Ok(Self {
            market: MarketFold::new(setup.instrument, setup.book_capacity),
            inbox: MarketBuffer::new(setup.market_inbox_capacity),
            orders: RestingOrders::new(),
            limits: setup.limits,
            timeline: Timeline::default(),
            emissions: EmissionBook::default(),
            wallet: SimWallet::new(setup.wallet)?,
            verdict_retention: setup.verdict_retention,
            reap_due_ts_us: None,
            reset_announced: false,
        })
    }

    pub fn wallet_mut(&mut self) -> &mut SimWallet {
        &mut self.wallet
    }

    pub fn market(&self) -> &MarketFold {
        &self.market
    }

    pub fn on_request(&mut self, request: &ExecRequest, effective_ts_us: TsUs) {
        let plan = self.fold().admit(request, effective_ts_us);
        self.schedule(plan);
    }

    pub fn on_market_batch(&mut self, tapped: &[TappedMessage], latency: latency::LatencyBudget) {
        for item in tapped {
            let effective_ts_us = latency.market_effective(item.message.received_ts_us());
            self.on_market(item, effective_ts_us);
        }
    }

    pub fn on_market(&mut self, tapped: &TappedMessage, effective_ts_us: TsUs) {
        let Some(phase) = self.phase_of(&tapped.message, tapped.venue_meta) else {
            return;
        };
        self.inbox
            .push(effective_ts_us, phase, (tapped.message, tapped.venue_meta));
    }

    pub fn advance_to_watermark(&mut self, horizon: TsUs, due: &mut Vec<SimEmission>) {
        while let Some(source) = self.next_due(horizon) {
            match source {
                Source::Market => {
                    if let Some(event) = self.inbox.pop() {
                        self.apply_market(event);
                    }
                }
                Source::Scheduled => {
                    if let Some(entry) = self.timeline.pop() {
                        self.fold().run(entry.action, entry.at_ts_us);
                    }
                }
            }
        }
        self.reap_expired_verdicts(horizon);
        due.clear();
        self.emissions.drain_through(horizon, due);
    }

    fn reap_expired_verdicts(&mut self, horizon: TsUs) {
        let due = self.reap_due_ts_us.unwrap_or(horizon);
        if horizon < due {
            return;
        }
        self.orders.reap_through(horizon, self.verdict_retention);
        self.reap_due_ts_us = Some(latency::shifted(horizon, [self.verdict_retention]));
    }

    fn next_due(&self, horizon: TsUs) -> Option<Source> {
        let market = self.inbox.peek().filter(|key| key.0 <= horizon);
        let scheduled = self.timeline.peek().filter(|key| key.0 <= horizon);
        match (market, scheduled) {
            (Some(left), Some(right)) if right < left => Some(Source::Scheduled),
            (Some(_), _) => Some(Source::Market),
            (None, Some(_)) => Some(Source::Scheduled),
            (None, None) => None,
        }
    }

    fn phase_of(&self, message: &InboundMessage, venue_meta: VenueMeta) -> Option<Phase> {
        match (message, venue_meta) {
            (InboundMessage::Trade(_), VenueMeta::Trade { .. }) => Some(Phase::Trade),
            (InboundMessage::Book(chunk), VenueMeta::DepthDelta { .. })
                if chunk.kind == BookChunkKind::Delta =>
            {
                Some(Phase::DeltaCommit)
            }
            (InboundMessage::Book(chunk), VenueMeta::None)
                if chunk.kind == BookChunkKind::Snapshot =>
            {
                Some(Phase::SnapshotRebuild)
            }
            (InboundMessage::BookReset(reset), VenueMeta::DepthReset { .. }) => {
                assert_eq!(
                    reset.instrument,
                    self.market.instrument(),
                    "a book reset for another instrument reached this venue"
                );
                Some(Phase::DepthReset)
            }
            (
                InboundMessage::Trade(_) | InboundMessage::Book(_) | InboundMessage::BookReset(_),
                _,
            ) => {
                panic!("a matching market message reached the venue as {venue_meta:?}")
            }
            _ => None,
        }
    }

    fn apply_market(&mut self, event: BufferedMarket) {
        match &event.message {
            InboundMessage::Trade(trade) => {
                self.apply_trade(trade, event.venue_meta, event.at_ts_us)
            }
            InboundMessage::Book(chunk) => {
                self.apply_book_chunk(chunk, event.venue_meta, event.at_ts_us)
            }
            InboundMessage::BookReset(_) => {
                let VenueMeta::DepthReset { exchange_ts_us } = event.venue_meta else {
                    panic!("a book reset reached the venue without its exchange stamp")
                };
                let reason = self.market.on_book_reset(exchange_ts_us);
                self.record_reset(reason, event.at_ts_us);
            }
            other => panic!("a {other:?} reached the venue's market fold"),
        }
    }

    pub fn restore_matching(&mut self, at_ts_us: TsUs) {
        let was_suspended = !self.market.is_matching_live();
        self.market.restore_matching();
        if was_suspended {
            self.reset_announced = false;
        }
        let deferred: Vec<SimOrderIndex> = self
            .orders
            .iter()
            .filter(|(_, record)| {
                record.phase == OrderPhase::Pending && record.effective_ts_us <= at_ts_us
            })
            .map(|(index, _)| index)
            .collect();
        for index in deferred {
            self.timeline
                .schedule(at_ts_us, TimedAction::Activate(index));
        }
    }

    pub fn suspend_matching(&mut self, reason: ResetReason, at_ts_us: TsUs) {
        let reason = self.market.suspend_matching(reason);
        self.record_reset(reason, at_ts_us);
    }

    fn apply_book_chunk(
        &mut self,
        chunk: &crate::msg::inbound::BookChunk,
        venue_meta: VenueMeta,
        effective_ts_us: TsUs,
    ) {
        match self.market.on_book_chunk(chunk, venue_meta) {
            BookVerdict::Reset(reason) => {
                self.record_reset(reason, effective_ts_us);
            }
            BookVerdict::DeltaCommitted
            | BookVerdict::DeltaStaged
            | BookVerdict::SnapshotApplied => {}
        }
    }

    fn apply_trade(
        &mut self,
        trade: &crate::msg::inbound::TradeEvent,
        venue_meta: VenueMeta,
        effective_ts_us: TsUs,
    ) {
        let evidence = TradeEvidence::from_meta(venue_meta);
        let at_ts_us = effective_ts_us;
        let verdict = {
            let SimVenue {
                market,
                orders,
                emissions,
                wallet,
                ..
            } = self;
            let mut take = |index: SimOrderIndex, offered: Qty| {
                let Some(record) = orders.get_mut(index) else {
                    return VACATE;
                };
                record.prints_seen += 1;
                if record.phase != OrderPhase::Resting {
                    return VACATE;
                }
                let taken = record.order.take(offered);
                let last_price = record.order.price;
                let is_complete = record.order.is_complete();
                if taken.0 == 0 {
                    return OwnFill { taken, is_complete };
                }
                let settlement = wallet.fill(
                    record
                        .reservation
                        .as_mut()
                        .expect("a resting order filled with no reservation"),
                    taken,
                );
                if is_complete {
                    close_order(orders, wallet, index, ClosedReason::Filled, at_ts_us)
                        .expect("the filled order still exists");
                }
                let snapshot = orders
                    .snapshot(index)
                    .expect("the filled order still has a snapshot");
                let trade_id = orders.mint_trade_id();
                emissions.push(
                    at_ts_us,
                    VenueEvent::Filled {
                        snapshot,
                        last_qty: taken,
                        last_price,
                        trade_id,
                        settlement,
                    },
                );
                OwnFill { taken, is_complete }
            };
            market.on_trade(trade, evidence, &mut take)
        };
        if let TradeVerdict::Reset(reason) = verdict {
            self.record_reset(reason, at_ts_us);
        }
    }

    #[cold]
    fn record_reset(&mut self, reason: ResetReason, at_ts_us: TsUs) {
        for record in self
            .orders
            .iter_mut()
            .filter(|record| record.phase == OrderPhase::Resting)
        {
            record.resyncs_while_resting += 1;
        }
        if self.reset_announced {
            return;
        }
        self.reset_announced = true;
        self.emissions
            .push(at_ts_us, VenueEvent::MarketReset { reason });
    }

    fn schedule(&mut self, plan: AdmitPlan) {
        if let Some((at_ts_us, action)) = plan {
            self.timeline.schedule(at_ts_us, action);
        }
    }

    fn fold(&mut self) -> RequestFold<'_> {
        RequestFold {
            orders: &mut self.orders,
            market: &mut self.market,
            emissions: &mut self.emissions,
            limits: &self.limits,
            wallet: &mut self.wallet,
        }
    }
}

const VACATE: OwnFill = OwnFill {
    taken: Qty(0),
    is_complete: true,
};
