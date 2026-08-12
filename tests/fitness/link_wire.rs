//! Link wire fitness: every frame kind survives an encode-then-decode round trip unchanged,
//! every envelope rejection reports a distinct cause, and the sequence gate drops duplicates
//! and reorders while letting a restarted peer back in. The failure this suite exists to
//! prevent is silent: a trading engine and a separately built UI misdecoding each other's
//! bytes into a plausible-looking book ladder.

use std::net::SocketAddr;

use polysim::config::ExecutionMode;
use polysim::hot::exec::{
    CloseReason, ExecHalt, HaltReason, OrderState, QuoteLevel, ReadinessGap, RejectOrigin,
    RejectReason,
};
use polysim::ids::{AssetId, ClientOrderId, InstrumentId, Price, Qty, Side};
use polysim::link::{
    CatalogFeature, CatalogInstrument, Envelope, FrameGuard, GateVerdict, LINK_MAGIC,
    LINK_MAX_DATAGRAM, LINK_MAX_FIELDS, LINK_MAX_GATE_KEYS, LINK_MAX_SUBSCRIBERS, LINK_MAX_TOPICS,
    LINK_SUBSCRIPTION_TTL, LINK_VERSION, Lifecycle, LinkBody, LinkDatagram, LinkDecodeError,
    LinkHash, LinkIdentity, LinkPayload, RefreshOutcome, RunPhase, RunState, SequenceGate,
    Subscribe, SubscriberTable, TopicId, TopicSet, WireName,
};
use polysim::msg::exec::{Liquidity, RejectClass};
use polysim::msg::inbound::Level;
use polysim::msg::persist::FeatureId;
use polysim::msg::ui::{
    DomQuote, UI_BOOK_LEVELS, UI_ORDER_SNAPSHOT_CAPACITY, UI_ORDER_SNAPSHOT_MAX_TOTAL,
    UiBookSnapshot, UiBookState, UiEvent, UiLatencyCell, UiLatencyRow, UiLatencySummary,
    UiWorkingOrder,
};
use polysim::time::{DurationUs, TsUs};
use proptest::prelude::*;

const TOKEN: LinkHash = LinkHash::of_name("fitness-run-token");
const STRATEGY: LinkHash = LinkHash::of_name("strat-micro-recorder");
const SCHEMA: LinkHash = LinkHash::of_fields(&["mid", "microprice", "imbalance"]);
const SENDER: LinkHash = LinkHash::of_name("te-binance-spot-btcusdt");

/// The topic field sits at bytes 6..8 of the envelope. Patching the wire is the only way to build a
/// datagram this build cannot construct through its own types — which is exactly what a peer on a
/// newer wire does.
const TOPIC_OFFSET: usize = 6;
/// `count` follows the 80-byte envelope, the 8-byte schema hash and the 8-byte event stamp.
const PAYLOAD_COUNT_OFFSET: usize = 96;
/// `bid_len` follows the 80-byte envelope, the 2-byte instrument, the 8-byte sequence, the 8-byte
/// event stamp and the 1-byte book state; `ask_len` is the two bytes after it.
const BOOK_BID_LEN_OFFSET: usize = 99;

fn guard() -> FrameGuard {
    FrameGuard {
        token_hash: TOKEN,
        strategy_hash: STRATEGY,
        schema_hash: SCHEMA,
    }
}

fn identity() -> LinkIdentity {
    LinkIdentity {
        token_hash: TOKEN,
        strategy_hash: STRATEGY,
        sender_te_hash: SENDER,
        boot_ts_us: TsUs::from_micros(1_700_000_000_000_000),
    }
}

fn envelope(topic: TopicId, seq: u64) -> Envelope {
    Envelope::new(identity(), topic, seq)
}

fn encode(datagram: &LinkDatagram) -> Vec<u8> {
    let mut buffer = [0; LINK_MAX_DATAGRAM];
    let len = datagram.encode(&mut buffer);
    buffer[..len].to_vec()
}

fn decode(bytes: &[u8]) -> Result<LinkDatagram, LinkDecodeError> {
    LinkDatagram::decode(bytes, &guard())
}

fn round_trip(datagram: &LinkDatagram) -> Result<LinkDatagram, LinkDecodeError> {
    decode(&encode(datagram))
}

fn book_datagram() -> LinkDatagram {
    LinkDatagram {
        envelope: envelope(TopicId::BOOKS, 1),
        body: LinkBody::Book(UiBookSnapshot {
            instrument: InstrumentId(0),
            seq: 7,
            event_ts_us: TsUs::from_micros(42),
            state: UiBookState::Valid,
            bid_len: 1,
            ask_len: 1,
            bids: [Level {
                price: Price(100),
                qty: Qty(5),
            }; UI_BOOK_LEVELS],
            asks: [Level {
                price: Price(101),
                qty: Qty(5),
            }; UI_BOOK_LEVELS],
        }),
    }
}

fn event_datagram(event: UiEvent) -> LinkDatagram {
    LinkDatagram {
        envelope: envelope(TopicId::EVENTS, 1),
        body: LinkBody::Event(event),
    }
}

fn order_snapshot(detail_len: u8, total_working: u16, details: &[UiWorkingOrder]) -> UiEvent {
    let mut orders = [UiWorkingOrder::EMPTY; UI_ORDER_SNAPSHOT_CAPACITY];
    orders[..details.len()].copy_from_slice(details);
    UiEvent::OrderSnapshot {
        instrument: InstrumentId(3),
        seq: 41,
        event_ts_us: TsUs::from_micros(77),
        side: Side::Buy,
        detail_len,
        total_working,
        orders,
    }
}

fn working_order(client_id: u64, state: OrderState) -> UiWorkingOrder {
    UiWorkingOrder {
        client_id: ClientOrderId(client_id),
        quote_level: QuoteLevel::new((client_id % 8) as u8),
        state,
        price: Price(100 + client_id as i64),
        qty: Qty(5),
        filled: Qty(1),
    }
}

fn payload_datagram(schema_hash: LinkHash) -> LinkDatagram {
    LinkDatagram {
        envelope: envelope(TopicId::FIRST_STRATEGY, 1),
        body: LinkBody::Payload(LinkPayload::new(
            schema_hash,
            TsUs::from_micros(99),
            &[1.5, -2.5],
        )),
    }
}

fn arb_envelope(topic: TopicId) -> impl Strategy<Value = Envelope> {
    (
        any::<u64>(),
        any::<i64>(),
        any::<u64>(),
        prop::collection::vec(any::<u8>(), 32),
    )
        .prop_map(move |(sender, boot, seq, mac)| {
            let mut envelope = Envelope::new(
                LinkIdentity {
                    token_hash: TOKEN,
                    strategy_hash: STRATEGY,
                    sender_te_hash: LinkHash(sender),
                    boot_ts_us: TsUs::from_micros(boot),
                },
                topic,
                seq,
            );
            envelope.mac = mac.try_into().expect("32 generated bytes");
            envelope
        })
}

fn arb_topic_set() -> impl Strategy<Value = TopicSet> {
    prop::collection::vec(any::<u16>().prop_map(TopicId), 0..=LINK_MAX_TOPICS)
        .prop_map(|topics| TopicSet::new(&topics).expect("generated within capacity"))
}

fn arb_run_state() -> impl Strategy<Value = RunState> {
    prop_oneof![Just(RunState::Running), Just(RunState::Idle)]
}

fn arb_levels() -> impl Strategy<Value = [Level; UI_BOOK_LEVELS]> {
    prop::collection::vec(
        (any::<i64>(), any::<i64>()).prop_map(|(price, qty)| Level {
            price: Price(price),
            qty: Qty(qty),
        }),
        UI_BOOK_LEVELS,
    )
    .prop_map(|levels| levels.try_into().expect("generated exactly one side"))
}

fn arb_optional_level() -> impl Strategy<Value = Option<(Price, Qty)>> {
    prop::option::of((any::<i64>(), any::<i64>()).prop_map(|(price, qty)| (Price(price), Qty(qty))))
}

fn arb_side() -> impl Strategy<Value = Side> {
    prop_oneof![Just(Side::Buy), Just(Side::Sell)]
}

/// The float domain the round-trip generator puts on the wire. `any::<f64>()` alone is
/// `POSITIVE|NEGATIVE|ZERO|SUBNORMAL|NORMAL` (proptest's `Arbitrary` impl, which deliberately omits
/// `INFINITE` and `QUIET_NAN`), so no non-finite value would ever ride an event body — while a quant
/// feature genuinely can be infinite, as Kyle's lambda has already shipped. The infinities are added
/// back here. NaN cannot join them: it is unequal to itself, so NO structural round-trip can express
/// it however perfect the codec; [`event_floats_preserve_exact_bits`] covers that half bitwise.
fn arb_wire_f64() -> impl Strategy<Value = f64> {
    prop_oneof![
        8 => any::<f64>(),
        1 => Just(f64::INFINITY),
        1 => Just(f64::NEG_INFINITY),
    ]
}

/// Every non-finite bit pattern: a sign, an all-ones exponent, and an arbitrary mantissa — zero for
/// an infinity, anything else for a NaN, quiet or signalling.
fn arb_non_finite_bits() -> impl Strategy<Value = u64> {
    (any::<bool>(), 0u64..(1 << 52)).prop_map(|(negative, mantissa)| {
        (u64::from(negative) << 63) | 0x7ff0_0000_0000_0000 | mantissa
    })
}

/// Half arbitrary patterns, half deliberately non-finite. A uniform `any::<u64>()` lands on an
/// all-ones exponent once in 2048 draws, so the case this generator exists to reach would be absent
/// from most runs — measured, not assumed: an encoder mutated to normalise non-finite values passed
/// the bit test while it drew from `any::<u64>()` alone.
fn arb_f64_bits() -> impl Strategy<Value = u64> {
    prop_oneof![any::<u64>(), arb_non_finite_bits()]
}

fn arb_name() -> impl Strategy<Value = WireName> {
    "[a-z_]{0,32}".prop_map(|name| WireName::new(&name))
}

fn arb_book() -> impl Strategy<Value = LinkBody> {
    (
        any::<u16>(),
        any::<u64>(),
        any::<i64>(),
        prop_oneof![
            Just(UiBookState::AwaitingSnapshot),
            Just(UiBookState::Valid)
        ],
        0..=UI_BOOK_LEVELS as u16,
        0..=UI_BOOK_LEVELS as u16,
        arb_levels(),
        arb_levels(),
    )
        .prop_map(
            |(instrument, seq, stamp, state, bid_len, ask_len, bids, asks)| {
                LinkBody::Book(UiBookSnapshot {
                    instrument: InstrumentId(instrument),
                    seq,
                    event_ts_us: TsUs::from_micros(stamp),
                    state,
                    bid_len,
                    ask_len,
                    bids,
                    asks,
                })
            },
        )
}

/// Every order state, including all six close reasons: the wire flattens `Closed(_)` into the same
/// byte as the open states to fit the tail, so a reason that failed to round-trip would silently
/// report a cancel as a fill.
fn arb_order_state() -> impl Strategy<Value = OrderState> {
    prop_oneof![
        Just(OrderState::Free),
        Just(OrderState::PendingNew),
        Just(OrderState::Live),
        Just(OrderState::CancelInFlight),
        Just(OrderState::AmendInFlight),
        Just(OrderState::Unknown),
        Just(OrderState::Closed(CloseReason::Filled)),
        Just(OrderState::Closed(CloseReason::Canceled)),
        Just(OrderState::Closed(CloseReason::Rejected)),
        Just(OrderState::Closed(CloseReason::Expired)),
        Just(OrderState::Closed(CloseReason::ReconciledGone)),
    ]
}

fn arb_quote_level() -> impl Strategy<Value = Option<QuoteLevel>> {
    prop_oneof![Just(None), (0_u8..8).prop_map(QuoteLevel::new),]
}

fn arb_liquidity() -> impl Strategy<Value = Option<Liquidity>> {
    prop_oneof![
        Just(None),
        Just(Some(Liquidity::Maker)),
        Just(Some(Liquidity::Taker)),
    ]
}

const REJECT_REASONS: [RejectReason; 19] = [
    RejectReason::QtyBelowMin,
    RejectReason::NotionalBelowMin,
    RejectReason::NotionalAboveMax,
    RejectReason::WouldCross,
    RejectReason::OutsideBand,
    RejectReason::Underfunded,
    RejectReason::StyleNotPermitted,
    RejectReason::NotReady(ReadinessGap::Stream),
    RejectReason::NotReady(ReadinessGap::Balances),
    RejectReason::NotReady(ReadinessGap::OpenOrders),
    RejectReason::Halted,
    RejectReason::SessionReducingOnly,
    RejectReason::ExposureCeiling,
    RejectReason::NoQuoteDeclared,
    RejectReason::BookNotQuotable,
    RejectReason::DuplicatePrice,
    RejectReason::OrderLimit,
    RejectReason::OutsideWindow,
    RejectReason::RateBudget,
];

/// The slot each refusal claims in [`REJECT_REASONS`], which is what stops that list rotting into a
/// subset of the enum. A new `RejectReason` lands here as a missing arm, and the arm it needs
/// indexes one past the array, so it will not compile until the array grows with it and the
/// round-trip below starts generating it.
///
/// The `const` blocks are load-bearing, not decoration: a bare `REJECT_REASONS[19]` compiles, since
/// the out-of-bounds lint does not reach a const of this shape. Only const evaluation refuses it.
fn listed_as(reason: RejectReason) -> RejectReason {
    match reason {
        RejectReason::QtyBelowMin => const { REJECT_REASONS[0] },
        RejectReason::NotionalBelowMin => const { REJECT_REASONS[1] },
        RejectReason::NotionalAboveMax => const { REJECT_REASONS[2] },
        RejectReason::WouldCross => const { REJECT_REASONS[3] },
        RejectReason::OutsideBand => const { REJECT_REASONS[4] },
        RejectReason::Underfunded => const { REJECT_REASONS[5] },
        RejectReason::StyleNotPermitted => const { REJECT_REASONS[6] },
        RejectReason::NotReady(ReadinessGap::Stream) => const { REJECT_REASONS[7] },
        RejectReason::NotReady(ReadinessGap::Balances) => const { REJECT_REASONS[8] },
        RejectReason::NotReady(ReadinessGap::OpenOrders) => const { REJECT_REASONS[9] },
        RejectReason::Halted => const { REJECT_REASONS[10] },
        RejectReason::SessionReducingOnly => const { REJECT_REASONS[11] },
        RejectReason::ExposureCeiling => const { REJECT_REASONS[12] },
        RejectReason::NoQuoteDeclared => const { REJECT_REASONS[13] },
        RejectReason::BookNotQuotable => const { REJECT_REASONS[14] },
        RejectReason::DuplicatePrice => const { REJECT_REASONS[15] },
        RejectReason::OrderLimit => const { REJECT_REASONS[16] },
        RejectReason::OutsideWindow => const { REJECT_REASONS[17] },
        RejectReason::RateBudget => const { REJECT_REASONS[18] },
    }
}

#[test]
fn every_reject_reason_claims_its_own_slot() {
    for (slot, reason) in REJECT_REASONS.into_iter().enumerate() {
        assert_eq!(
            listed_as(reason),
            reason,
            "reject reason at slot {slot} disagrees with the slot it claims, so at least one \
             refusal is generated twice and another not at all"
        );
    }
}

/// Both refusal origins. The local arm pads the venue arm's code field, so a decoder that read the
/// padding as a code would report a venue rejection that never happened.
fn arb_reject_origin() -> impl Strategy<Value = RejectOrigin> {
    prop_oneof![
        prop::sample::select(REJECT_REASONS.to_vec()).prop_map(RejectOrigin::Local),
        (
            prop_oneof![
                Just(RejectClass::StillLive),
                Just(RejectClass::Refused),
                Just(RejectClass::Gone),
                Just(RejectClass::Ambiguous),
                Just(RejectClass::Fatal),
            ],
            any::<i32>()
        )
            .prop_map(|(class, code)| RejectOrigin::Venue { class, code }),
    ]
}

fn arb_halt() -> impl Strategy<Value = ExecHalt> {
    prop_oneof![
        Just(ExecHalt::Armed),
        (
            prop_oneof![
                Just(HaltReason::RejectStreak),
                Just(HaltReason::RealisedLoss),
                Just(HaltReason::FatalReject),
                Just(HaltReason::SlotLeak),
                Just(HaltReason::FilterViolation),
                Just(HaltReason::CommandBankOverflow),
                Just(HaltReason::DuplicateResting),
            ],
            any::<i64>()
        )
            .prop_map(|(reason, at)| ExecHalt::Halted {
                reason,
                halted_ts_us: TsUs::from_micros(at)
            }),
    ]
}

/// Counts stay inside `u32` because that is what the tail carries: the encoder saturates above it,
/// and a generator that drew wider would be asserting a round-trip the wire never promised.
fn arb_latency_cell() -> impl Strategy<Value = UiLatencyCell> {
    (any::<u32>(), any::<i64>()).prop_map(|(count, sum_us)| UiLatencyCell {
        count: u64::from(count),
        sum_us,
    })
}

fn arb_latency_row() -> impl Strategy<Value = UiLatencyRow> {
    (
        arb_latency_cell(),
        arb_latency_cell(),
        arb_latency_cell(),
        arb_latency_cell(),
        arb_latency_cell(),
        arb_latency_cell(),
    )
        .prop_map(
            |(
                exchange_to_received,
                received_to_queued,
                queue_wait,
                processing,
                end_to_end,
                order_round_trip,
            )| UiLatencyRow {
                exchange_to_received,
                received_to_queued,
                queue_wait,
                processing,
                end_to_end,
                order_round_trip,
            },
        )
}

fn arb_latency() -> impl Strategy<Value = UiLatencySummary> {
    (
        arb_latency_row(),
        arb_latency_row(),
        arb_latency_row(),
        prop::option::of(arb_wire_f64()),
    )
        .prop_map(
            |(market_data, execution, hot_path, backlog_ema)| UiLatencySummary {
                market_data,
                execution,
                hot_path,
                backlog_ema,
            },
        )
}

fn arb_event() -> impl Strategy<Value = LinkBody> {
    let head = (any::<u16>(), any::<u64>(), any::<i64>());
    prop_oneof![
        (head, arb_optional_level(), arb_optional_level()).prop_map(
            |((instrument, seq, stamp), bid, ask)| UiEvent::Quote {
                instrument: InstrumentId(instrument),
                seq,
                event_ts_us: TsUs::from_micros(stamp),
                quote: DomQuote::top(bid, ask),
            }
        ),
        (head, arb_side(), any::<i64>(), any::<i64>()).prop_map(
            |((instrument, seq, stamp), aggressor, price, qty)| UiEvent::Trade {
                instrument: InstrumentId(instrument),
                seq,
                event_ts_us: TsUs::from_micros(stamp),
                aggressor,
                price: Price(price),
                qty: Qty(qty),
            }
        ),
        (
            head,
            any::<u64>(),
            arb_quote_level(),
            arb_side(),
            arb_order_state(),
            (any::<i64>(), any::<i64>(), any::<i64>())
        )
            .prop_map(
                |(
                    (instrument, seq, stamp),
                    client_id,
                    quote_level,
                    side,
                    state,
                    (price, qty, filled),
                )| {
                    UiEvent::OrderUpdate {
                        instrument: InstrumentId(instrument),
                        seq,
                        event_ts_us: TsUs::from_micros(stamp),
                        client_id: ClientOrderId(client_id),
                        quote_level,
                        side,
                        state,
                        price: Price(price),
                        qty: Qty(qty),
                        filled: Qty(filled),
                    }
                }
            ),
        (head, any::<u16>(), any::<i64>(), any::<i64>()).prop_map(
            |((_, seq, stamp), asset, free, locked)| UiEvent::Balance {
                asset: AssetId(asset),
                seq,
                event_ts_us: TsUs::from_micros(stamp),
                free,
                locked,
            }
        ),
        (head, arb_side(), arb_reject_origin()).prop_map(
            |((instrument, seq, stamp), side, origin)| UiEvent::Reject {
                instrument: InstrumentId(instrument),
                seq,
                event_ts_us: TsUs::from_micros(stamp),
                side,
                origin,
            }
        ),
        (any::<u64>(), any::<i64>(), arb_halt()).prop_map(|(seq, stamp, halt)| {
            UiEvent::Execution {
                seq,
                event_ts_us: TsUs::from_micros(stamp),
                halt,
            }
        }),
        (head, any::<u16>(), arb_wire_f64()).prop_map(
            |((instrument, seq, stamp), feature, value)| UiEvent::Feature {
                instrument: InstrumentId(instrument),
                seq,
                event_ts_us: TsUs::from_micros(stamp),
                feature: FeatureId(feature),
                value,
            }
        ),
        (
            head,
            arb_quote_level(),
            arb_side(),
            (any::<i64>(), any::<i64>(), any::<i64>()),
            any::<u16>(),
            arb_liquidity()
        )
            .prop_map(
                |(
                    (instrument, seq, stamp),
                    quote_level,
                    side,
                    (price, qty, commission),
                    commission_asset,
                    liquidity,
                )| UiEvent::Fill {
                    instrument: InstrumentId(instrument),
                    seq,
                    event_ts_us: TsUs::from_micros(stamp),
                    quote_level,
                    side,
                    price: Price(price),
                    qty: Qty(qty),
                    commission,
                    commission_asset: AssetId(commission_asset),
                    liquidity,
                }
            ),
        head.prop_map(|(instrument, seq, stamp)| UiEvent::Rotation {
            instrument: InstrumentId(instrument),
            seq,
            event_ts_us: TsUs::from_micros(stamp),
        }),
        (head, arb_wire_f64(), arb_wire_f64()).prop_map(
            |((instrument, seq, stamp), exposure_quote, pnl_quote)| UiEvent::Position {
                instrument: InstrumentId(instrument),
                seq,
                event_ts_us: TsUs::from_micros(stamp),
                exposure_quote,
                pnl_quote,
            }
        ),
        (any::<u64>(), any::<i64>(), arb_latency()).prop_map(|(seq, stamp, summary)| {
            UiEvent::Latency {
                seq,
                event_ts_us: TsUs::from_micros(stamp),
                summary,
            }
        }),
    ]
    .prop_map(LinkBody::Event)
}

fn arb_catalog_instrument() -> impl Strategy<Value = LinkBody> {
    (
        any::<i64>(),
        any::<u16>(),
        any::<u16>(),
        arb_name(),
        prop::option::of(any::<i64>()),
        prop::option::of(any::<i64>()),
        any::<i64>(),
        (any::<u16>(), any::<u16>(), arb_name(), arb_name()),
    )
        .prop_map(
            |(
                catalog_ts_us,
                total_count,
                instrument,
                display,
                tick,
                lot,
                qty_scale,
                (base_asset, quote_asset, base, quote),
            )| {
                LinkBody::CatalogInstrument(CatalogInstrument {
                    catalog_ts_us: TsUs::from_micros(catalog_ts_us),
                    total_count,
                    instrument: InstrumentId(instrument),
                    display,
                    tick_size: tick.map(Price),
                    lot_size: lot.map(Qty),
                    qty_scale,
                    base_asset: AssetId(base_asset),
                    quote_asset: AssetId(quote_asset),
                    base,
                    quote,
                })
            },
        )
}

fn arb_datagram() -> impl Strategy<Value = LinkDatagram> {
    prop_oneof![
        (
            arb_envelope(TopicId::SUBSCRIBE),
            arb_topic_set(),
            arb_run_state(),
            any::<u64>()
        )
            .prop_map(
                |(envelope, topics, desired_state, desired_epoch)| LinkDatagram {
                    envelope,
                    body: LinkBody::Subscribe(Subscribe {
                        topics,
                        desired_state,
                        desired_epoch,
                    }),
                }
            ),
        (arb_envelope(TopicId::BOOKS), arb_book())
            .prop_map(|(envelope, body)| LinkDatagram { envelope, body }),
        (arb_envelope(TopicId::EVENTS), arb_event())
            .prop_map(|(envelope, body)| LinkDatagram { envelope, body }),
        (
            arb_envelope(TopicId::CATALOG_INSTRUMENTS),
            arb_catalog_instrument()
        )
            .prop_map(|(envelope, body)| LinkDatagram { envelope, body }),
        (
            arb_envelope(TopicId::CATALOG_FEATURES),
            any::<i64>(),
            any::<u16>(),
            any::<u16>(),
            arb_name()
        )
            .prop_map(|(envelope, catalog_ts_us, total_count, feature, name)| {
                LinkDatagram {
                    envelope,
                    body: LinkBody::CatalogFeature(CatalogFeature {
                        catalog_ts_us: TsUs::from_micros(catalog_ts_us),
                        total_count,
                        feature: FeatureId(feature),
                        name,
                    }),
                }
            }),
        (
            arb_envelope(TopicId::LIFECYCLE),
            prop_oneof![
                Just(RunPhase::Starting),
                Just(RunPhase::Ready),
                Just(RunPhase::Draining),
                Just(RunPhase::Stopped)
            ],
            arb_run_state(),
            prop_oneof![
                Just(None),
                Just(Some(ExecutionMode::Off)),
                Just(Some(ExecutionMode::Sim)),
                Just(Some(ExecutionMode::Live))
            ],
            any::<u64>(),
            any::<i64>(),
            any::<u16>()
        )
            .prop_map(
                |(
                    envelope,
                    phase,
                    run_state,
                    execution_mode,
                    acknowledged_epoch,
                    spin_interval_us,
                    feature_count,
                )| {
                    LinkDatagram {
                        envelope,
                        body: LinkBody::Lifecycle(Lifecycle {
                            phase,
                            run_state,
                            execution_mode,
                            acknowledged_epoch,
                            spin_interval_us: DurationUs::from_micros(spin_interval_us),
                            feature_count,
                        }),
                    }
                }
            ),
        (
            arb_envelope(TopicId(LINK_MAX_TOPICS as u16)),
            any::<i64>(),
            prop::collection::vec(-1e12f64..1e12, 0..=LINK_MAX_FIELDS)
        )
            .prop_map(|(envelope, stamp, values)| LinkDatagram {
                envelope,
                body: LinkBody::Payload(LinkPayload::new(
                    SCHEMA,
                    TsUs::from_micros(stamp),
                    &values
                )),
            }),
    ]
}

proptest! {
    /// FITNESS: a decoded frame equals the frame that was encoded, for every kind including the
    /// reserved MAC bytes. A drift here corrupts a UI's book, a peer's signal or a subscription
    /// silently — nothing crashes, the numbers are just wrong.
    #[test]
    fn round_trips_every_frame_kind(datagram in arb_datagram()) {
        let decoded = round_trip(&datagram).expect("a canonical frame decodes");
        prop_assert_eq!(decoded, datagram);
    }

    /// FITNESS: float slots ride the wire bit-exactly. Normalising a NaN or losing a signed zero
    /// would make a receiver's statistics disagree with the sender's for no visible reason.
    #[test]
    fn payload_preserves_exact_float_bits(
        bits in prop::collection::vec(any::<u64>(), 1..=LINK_MAX_FIELDS),
    ) {
        let values: Vec<f64> = bits.iter().copied().map(f64::from_bits).collect();
        let datagram = LinkDatagram {
            envelope: envelope(TopicId::FIRST_STRATEGY, 3),
            body: LinkBody::Payload(LinkPayload::new(SCHEMA, TsUs::from_micros(1), &values)),
        };
        let decoded = round_trip(&datagram).expect("a canonical payload decodes");
        let LinkBody::Payload(payload) = decoded.body else {
            panic!("payload topic decoded as {:?}", decoded.body);
        };
        let decoded_bits: Vec<u64> = payload.values().iter().copied().map(f64::to_bits).collect();
        prop_assert_eq!(decoded_bits, bits);
    }

    /// FITNESS: the same bit-exactness for the EVENT body's float slots, which carry a strategy's
    /// feature values and the engine's exposure/PnL. The round-trip above cannot reach here: it
    /// compares structurally, and a NaN is unequal to itself, so a codec that quietly canonicalised
    /// one would pass every case it could generate. A normalised non-finite is the dangerous
    /// outcome — an infinity that arrives as 0 reads as a real measurement instead of the blown-up
    /// estimate it was, and a NaN payload that arrives as a different NaN breaks nothing visibly.
    #[test]
    fn event_floats_preserve_exact_bits(
        feature_bits in arb_f64_bits(),
        exposure_bits in arb_f64_bits(),
        pnl_bits in arb_f64_bits(),
    ) {
        let decoded = round_trip(&event_datagram(UiEvent::Feature {
            instrument: InstrumentId(3),
            seq: 8,
            event_ts_us: TsUs::from_micros(11),
            feature: FeatureId(2),
            value: f64::from_bits(feature_bits),
        }))
        .expect("a canonical event decodes");
        let LinkBody::Event(UiEvent::Feature { value, .. }) = decoded.body else {
            panic!("the events topic decoded as {:?}", decoded.body);
        };
        prop_assert_eq!(value.to_bits(), feature_bits);

        let decoded = round_trip(&event_datagram(UiEvent::Position {
            instrument: InstrumentId(3),
            seq: 9,
            event_ts_us: TsUs::from_micros(12),
            exposure_quote: f64::from_bits(exposure_bits),
            pnl_quote: f64::from_bits(pnl_bits),
        }))
        .expect("a canonical event decodes");
        let LinkBody::Event(UiEvent::Position {
            exposure_quote,
            pnl_quote,
            ..
        }) = decoded.body
        else {
            panic!("the events topic decoded as {:?}", decoded.body);
        };
        prop_assert_eq!(exposure_quote.to_bits(), exposure_bits);
        prop_assert_eq!(pnl_quote.to_bits(), pnl_bits);
    }
}

/// Every envelope field the decoder authenticates, corrupted one at a time. A peer on a skewed wire
/// version and a peer from someone else's run present the identical symptom — nothing decodes — so
/// each has to name its own cause or an operator is left guessing which of them is happening.
/// A book frame carries all four: the engine topics are the dangerous ones, since a `UiBookSnapshot`
/// layout moves whenever `UI_BOOK_LEVELS` or `Level` moves and the UI is built separately.
#[test]
fn each_authenticated_envelope_field_reports_its_own_rejection() {
    const OTHER_RUN: LinkHash = LinkHash::of_name("someone-elses-run");
    const OTHER_STRATEGY: LinkHash = LinkHash::of_name("strat-something-else");

    type Corrupt = fn(&mut Envelope);
    let cases: [(Corrupt, LinkDecodeError); 4] = [
        (
            |envelope| envelope.magic = LINK_MAGIC ^ 1,
            LinkDecodeError::MagicMismatch {
                found: LINK_MAGIC ^ 1,
            },
        ),
        (
            |envelope| envelope.version = LINK_VERSION + 1,
            LinkDecodeError::VersionMismatch {
                found: LINK_VERSION + 1,
            },
        ),
        (
            |envelope| envelope.token_hash = OTHER_RUN,
            LinkDecodeError::TokenMismatch {
                found: OTHER_RUN,
                expected: TOKEN,
            },
        ),
        (
            |envelope| envelope.strategy_hash = OTHER_STRATEGY,
            LinkDecodeError::StrategyMismatch {
                found: OTHER_STRATEGY,
                expected: STRATEGY,
            },
        ),
    ];

    for (corrupt, expected) in cases {
        let mut datagram = book_datagram();
        corrupt(&mut datagram.envelope);
        assert_eq!(decode(&encode(&datagram)), Err(expected));
    }
}

/// A reordered or renamed `link_fields()` list must fail on the first frame, not quietly feed slot 7
/// into slot 8.
#[test]
fn rejects_drifted_link_fields() {
    let drifted = LinkHash::of_fields(&["microprice", "mid", "imbalance"]);
    assert_eq!(
        decode(&encode(&payload_datagram(drifted))),
        Err(LinkDecodeError::SchemaMismatch {
            found: drifted,
            expected: SCHEMA,
        })
    );
    assert!(round_trip(&payload_datagram(SCHEMA)).is_ok());
}

#[test]
fn rejects_a_datagram_whose_length_is_not_its_kind() {
    let bytes = encode(&book_datagram());
    assert_eq!(
        decode(&bytes[..8]),
        Err(LinkDecodeError::Truncated { found: 8 })
    );
    let short = bytes.len() - 1;
    assert_eq!(
        decode(&bytes[..short]),
        Err(LinkDecodeError::LengthMismatch {
            topic: TopicId::BOOKS,
            expected: bytes.len(),
            found: short,
        })
    );
}

/// A peer on a newer wire announcing a topic this build has never heard of.
#[test]
fn rejects_an_unknown_engine_topic() {
    let mut bytes = encode(&book_datagram());
    let unknown = TopicId(TopicId::LIFECYCLE.0 + 1);
    bytes[TOPIC_OFFSET..TOPIC_OFFSET + 2].copy_from_slice(&unknown.0.to_le_bytes());
    assert_eq!(
        decode(&bytes),
        Err(LinkDecodeError::UnknownTopic { topic: unknown })
    );
}

/// The link actor indexes its per-topic sequence array by raw topic id, unchecked, on the send
/// path. Every id the engine can stamp — the fixed engine topics, and one per topic the strategy
/// declares — must land inside the array that the same declaration sized, or a send panics inside
/// the actor instead of the config being refused at startup.
#[test]
fn every_topic_the_engine_can_stamp_indexes_inside_its_sequence_array() {
    for declared in [0, 1, 2, 8, 1024] {
        let slots = TopicId::space_len(declared);
        for engine in [
            TopicId::SUBSCRIBE,
            TopicId::BOOKS,
            TopicId::EVENTS,
            TopicId::CATALOG_INSTRUMENTS,
            TopicId::CATALOG_FEATURES,
            TopicId::LIFECYCLE,
        ] {
            assert!(
                usize::from(engine.0) < slots,
                "engine topic {engine:?} falls outside the {slots} slots sized for {declared} strategy topics"
            );
        }
        for index in 0..declared {
            assert!(
                usize::from(TopicId::strategy(index).0) < slots,
                "strategy topic {index} falls outside the {slots} slots sized for {declared} strategy topics"
            );
        }
    }
}

/// A hostile `count` would make the payload's own slice indexing panic, handing anyone who can reach
/// the port a remote abort.
#[test]
fn rejects_a_payload_count_beyond_capacity() {
    let mut bytes = encode(&payload_datagram(SCHEMA));
    let forged = LINK_MAX_FIELDS as u8 + 1;
    bytes[PAYLOAD_COUNT_OFFSET] = forged;
    assert_eq!(
        decode(&bytes),
        Err(LinkDecodeError::FieldCountExceeded {
            count: forged,
            capacity: LINK_MAX_FIELDS,
        })
    );
}

/// The same hazard as [`rejects_a_payload_count_beyond_capacity`], on the topic that carries the most
/// bytes and was missed: `bid_len`/`ask_len` are read straight off the wire and the UI slices its
/// fixed `[Level; UI_BOOK_LEVELS]` arrays by them (`desktop::dom_view`), so a forged length is a
/// remote abort of the UI for anyone who can reach the port. The link's producer is UNTRUSTED by
/// design rather than by assumption, so a forged length is reachable input.
#[test]
fn rejects_book_lengths_beyond_capacity() {
    for offset in [BOOK_BID_LEN_OFFSET, BOOK_BID_LEN_OFFSET + 2] {
        let mut bytes = encode(&book_datagram());
        let forged = UI_BOOK_LEVELS as u16 + 1;
        bytes[offset..offset + 2].copy_from_slice(&forged.to_le_bytes());
        assert_eq!(
            decode(&bytes),
            Err(LinkDecodeError::BookLevelsExceeded {
                count: forged,
                capacity: UI_BOOK_LEVELS,
            }),
            "a forged length at byte {offset} must be refused, not sliced with"
        );
    }
}

/// Every `OrderState`, listed rather than generated. A thirteenth arm added to `state_tag` without a
/// matching arm in `read_state` has to fail HERE, on the run that adds it.
const EVERY_ORDER_STATE: [OrderState; 11] = [
    OrderState::Free,
    OrderState::PendingNew,
    OrderState::Live,
    OrderState::CancelInFlight,
    OrderState::AmendInFlight,
    OrderState::Unknown,
    OrderState::Closed(CloseReason::Filled),
    OrderState::Closed(CloseReason::Canceled),
    OrderState::Closed(CloseReason::Rejected),
    OrderState::Closed(CloseReason::Expired),
    OrderState::Closed(CloseReason::ReconciledGone),
];

/// FITNESS: every `OrderState` survives the tail, on every run.
///
/// `OrderUpdate` is the widest event kind and its tail is EXACTLY the budget — `ORDER_UPDATE_LEN`
/// and `EVENT_TAIL_LEN` are both 36, so there is no slack to absorb a mistake. The room came from
/// flattening `Closed(_)` into the same byte as the open states, which turned six close reasons into
/// six wire discriminants in one thirteen-arm match — and a thirteen-arm match is where a
/// copy-pasted tag hides.
///
/// [`round_trips_every_frame_kind`] generates these states too, so this is deliberately NOT a second
/// round-trip: it is the DETERMINISM the generator cannot give. One state sits behind four nested
/// choices there — datagram kind, then market-vs-exec, then event kind, then the state itself — so a
/// given state arrives in roughly one case in 900, and 256 cases reach it about a quarter of the
/// time. Measured, not assumed: against a mutant mapping `Closed(ReconciledGone)` to the `Expired`
/// tag, the proptest passed 6 clean runs out of 6 while this failed all 6.
///
/// A quarter of the time is a test that passes CI and loses the distinction between "we reconciled
/// it away" and "the venue took our order away for a reason we did not choose" — the distinction the
/// plan corrected the venue semantics to preserve.
#[test]
fn every_order_state_survives_the_event_tail() {
    for state in EVERY_ORDER_STATE {
        // Saturated, so a field written narrower than it is read cannot hide in a small value.
        let event = UiEvent::OrderUpdate {
            instrument: InstrumentId(u16::MAX),
            seq: u64::MAX,
            event_ts_us: TsUs::from_micros(i64::MAX),
            client_id: ClientOrderId(u64::MAX),
            quote_level: Some(QuoteLevel::new(7).expect("valid quote level")),
            side: Side::Sell,
            state,
            price: Price(i64::MIN),
            qty: Qty(i64::MAX),
            filled: Qty(i64::MIN),
        };
        let decoded = round_trip(&event_datagram(event)).expect("a canonical event decodes");
        assert_eq!(
            decoded.body,
            LinkBody::Event(event),
            "{state:?} did not survive a saturated OrderUpdate tail"
        );
    }
}

#[test]
fn complete_order_snapshot_round_trips_with_exact_overflow_count() {
    let details = [
        working_order(1, OrderState::Live),
        UiWorkingOrder {
            quote_level: None,
            ..working_order(2, OrderState::Unknown)
        },
    ];
    let event = order_snapshot(2, 5, &details);
    let decoded = round_trip(&event_datagram(event)).expect("a canonical snapshot decodes");
    assert_eq!(decoded.body, LinkBody::Event(event));
}

#[test]
fn malformed_order_snapshots_are_rejected_before_projection() {
    let live = working_order(7, OrderState::Live);
    for (detail_len, total_working) in [
        ((UI_ORDER_SNAPSHOT_CAPACITY + 1) as u8, 9),
        (2, 1),
        (0, UI_ORDER_SNAPSHOT_MAX_TOTAL + 1),
    ] {
        assert_eq!(
            round_trip(&event_datagram(order_snapshot(
                detail_len,
                total_working,
                &[live, working_order(8, OrderState::Live)],
            ))),
            Err(LinkDecodeError::OrderSnapshotCountsInvalid {
                detail_len,
                total_working,
                detail_capacity: UI_ORDER_SNAPSHOT_CAPACITY,
                total_capacity: UI_ORDER_SNAPSHOT_MAX_TOTAL,
            })
        );
    }

    let terminal = OrderState::Closed(CloseReason::Filled);
    assert_eq!(
        round_trip(&event_datagram(order_snapshot(
            1,
            1,
            &[working_order(7, terminal)],
        ))),
        Err(LinkDecodeError::OrderSnapshotTerminalState { state: terminal })
    );

    assert_eq!(
        round_trip(&event_datagram(order_snapshot(2, 2, &[live, live]))),
        Err(LinkDecodeError::OrderSnapshotDuplicate {
            client_id: live.client_id,
        })
    );
}

fn latency_cell(index: u64) -> UiLatencyCell {
    UiLatencyCell {
        count: index,
        sum_us: -(index as i64) * 1_000,
    }
}

fn latency_row(base: u64) -> UiLatencyRow {
    UiLatencyRow {
        exchange_to_received: latency_cell(base + 1),
        received_to_queued: latency_cell(base + 2),
        queue_wait: latency_cell(base + 3),
        processing: latency_cell(base + 4),
        end_to_end: latency_cell(base + 5),
        order_round_trip: latency_cell(base + 6),
    }
}

/// FITNESS: every self-timing cell keeps its own place in the tail, and an over-wide count saturates
/// rather than wrapping.
///
/// Eighteen structurally identical cells encode as one flat run of bytes, which is where a crossed
/// pair hides: a summary whose exec round-trip decoded into the market-data row still round-trips as
/// a whole. [`round_trips_every_frame_kind`] generates this kind too, but as one arm of eleven
/// inside one topic of seven — about one draw in seventy-seven, so a crossing would be caught on
/// most runs and missed on some. Distinct values in all eighteen cells make it every run.
#[test]
fn every_latency_cell_keeps_its_place_in_the_event_tail() {
    let event = UiEvent::Latency {
        seq: u64::MAX,
        event_ts_us: TsUs::from_micros(i64::MAX),
        summary: UiLatencySummary {
            market_data: latency_row(0),
            execution: latency_row(10),
            hot_path: latency_row(20),
            backlog_ema: Some(3.5),
        },
    };
    let decoded = round_trip(&event_datagram(event)).expect("a canonical event decodes");
    assert_eq!(decoded.body, LinkBody::Event(event));

    // A wrapped count would report a saturated ten-minute window as a nearly idle engine — the one
    // reading an operator would act on backwards.
    let overflowing = UiEvent::Latency {
        seq: 1,
        event_ts_us: TsUs::from_micros(1),
        summary: UiLatencySummary {
            market_data: UiLatencyRow {
                processing: UiLatencyCell {
                    count: u64::from(u32::MAX) + 5,
                    sum_us: 7,
                },
                ..UiLatencyRow::default()
            },
            ..UiLatencySummary::default()
        },
    };
    let LinkBody::Event(UiEvent::Latency { summary, .. }) =
        round_trip(&event_datagram(overflowing))
            .expect("a canonical event decodes")
            .body
    else {
        panic!("the events topic decoded as another body");
    };
    assert_eq!(
        summary.market_data.processing.count,
        u64::from(u32::MAX),
        "an over-wide count clamps to the tail's ceiling"
    );
    assert_eq!(
        summary.market_data.processing.sum_us, 7,
        "and the sum beside it is untouched"
    );
}

#[test]
fn all_sixteen_desired_levels_survive_the_quote_wire() {
    let mut quote = DomQuote::default();
    for level in 0..8 {
        quote.bids[level] = Some((Price(100 - level as i64), Qty(level as i64 + 1)));
        quote.asks[level] = Some((Price(101 + level as i64), Qty(level as i64 + 11)));
    }
    let event = UiEvent::Quote {
        instrument: InstrumentId(3),
        seq: 9,
        event_ts_us: TsUs::from_micros(77),
        quote,
    };
    let decoded = round_trip(&event_datagram(event)).expect("the full fixed ladder decodes");
    assert_eq!(decoded.body, LinkBody::Event(event));
}

/// A reused send buffer must not leak the previous datagram into an event frame's unused tail:
/// identical events would encode to different bytes, and a future MAC would reject them.
#[test]
fn event_encoding_is_independent_of_buffer_reuse() {
    let rotation = LinkDatagram {
        envelope: envelope(TopicId::EVENTS, 2),
        body: LinkBody::Event(UiEvent::Rotation {
            instrument: InstrumentId(1),
            seq: 5,
            event_ts_us: TsUs::from_micros(11),
        }),
    };
    let quote = LinkDatagram {
        envelope: envelope(TopicId::EVENTS, 1),
        body: LinkBody::Event(UiEvent::Quote {
            instrument: InstrumentId(1),
            seq: 4,
            event_ts_us: TsUs::from_micros(10),
            quote: DomQuote::top(
                Some((Price(i64::MAX), Qty(i64::MAX))),
                Some((Price(i64::MIN), Qty(i64::MIN))),
            ),
        }),
    };
    let mut dirty = [0xff; LINK_MAX_DATAGRAM];
    quote.encode(&mut dirty);
    let reused = rotation.encode(&mut dirty);
    assert_eq!(&dirty[..reused], encode(&rotation).as_slice());
}

/// The gate ages its slots, so every fixture stamps from one instant unless it is deliberately
/// walking the clock past the TTL.
const GATE_START: TsUs = TsUs::from_micros(1_000_000);

fn admit(gate: &mut SequenceGate, boot_ts_us: i64, seq: u64) -> GateVerdict {
    let mut envelope = envelope(TopicId::FIRST_STRATEGY, seq);
    envelope.boot_ts_us = TsUs::from_micros(boot_ts_us);
    gate.admit(&envelope, GATE_START)
}

fn admit_from(gate: &mut SequenceGate, sender: u64, seq: u64, now: TsUs) -> GateVerdict {
    let mut envelope = envelope(TopicId::FIRST_STRATEGY, seq);
    envelope.sender_te_hash = LinkHash(sender);
    gate.admit(&envelope, now)
}

fn admit_on(gate: &mut SequenceGate, topic: TopicId, seq: u64) -> GateVerdict {
    gate.admit(&envelope(topic, seq), GATE_START)
}

/// FITNESS: the slot key is `(sender, topic)`, so one peer's topics each carry their own sequence.
/// Collapsing them into a per-sender slot needs no error to do damage: two interleaved streams each
/// look like a reorder of the other, the gate drops most of both, and a topic simply goes quiet with
/// every frame accounted for as stale.
#[test]
fn gate_keys_a_slot_on_the_topic_as_well_as_the_sender() {
    let mut gate = SequenceGate::new();
    assert!(admit_on(&mut gate, TopicId::FIRST_STRATEGY, 1).is_accepted());
    assert!(
        admit_on(&mut gate, TopicId::BOOKS, 1).is_accepted(),
        "the same seq on another topic is a different stream, not a duplicate"
    );
    assert!(admit_on(&mut gate, TopicId::EVENTS, 1).is_accepted());
    assert_eq!(gate.counts().stale, 0);

    assert!(!admit_on(&mut gate, TopicId::BOOKS, 1).is_accepted());
    assert!(
        admit_on(&mut gate, TopicId::FIRST_STRATEGY, 2).is_accepted(),
        "and each topic's own sequence advanced independently of the others"
    );
    assert_eq!(gate.counts().stale, 1);
}

/// FITNESS: the consumed sequence is the record, so a duplicate or reordered datagram must never
/// reach the hot thread — a replayed tape would then not reproduce the run.
#[test]
fn gate_drops_duplicates_and_reordered_frames() {
    let mut gate = SequenceGate::new();
    for seq in 1..=3 {
        assert!(admit(&mut gate, 100, seq).is_accepted());
    }
    assert!(!admit(&mut gate, 100, 3).is_accepted());
    assert!(!admit(&mut gate, 100, 2).is_accepted());
    assert_eq!(gate.counts().stale, 2);
    assert!(admit(&mut gate, 100, 4).is_accepted());
}

/// A restarted peer's `seq` returns to 0. Without the boot stamp in the gate key the peer would stay
/// dark until its seq climbed past the old high-water mark — hours, on a slow topic.
#[test]
fn gate_accepts_a_restart_and_drops_an_older_boot() {
    let mut gate = SequenceGate::new();
    assert!(admit(&mut gate, 100, 5_000).is_accepted());
    assert!(admit(&mut gate, 200, 0).is_accepted());
    assert_eq!(gate.counts().restarts, 1);
    assert!(admit(&mut gate, 200, 1).is_accepted());

    assert!(!admit(&mut gate, 100, 9_999).is_accepted());
    assert_eq!(gate.counts().stale_boots, 1);
}

/// FITNESS: every workstation that attaches is a NEW `(sender, topic)` stream, so a gate that never
/// released a slot would refuse every peer after the capacity-th one for the rest of the run —
/// leaving a restart of the trading engine as the only recovery, which is the thing running the UI
/// in its own process exists to avoid.
#[test]
fn gate_evicts_a_stream_gone_quiet_and_spares_a_live_one() {
    let mut gate = SequenceGate::new();
    for sender in 0..LINK_MAX_GATE_KEYS as u64 {
        assert!(admit_from(&mut gate, sender, 1, GATE_START).is_accepted());
    }
    let ttl_later = GATE_START + LINK_SUBSCRIPTION_TTL;
    assert!(admit_from(&mut gate, 0, 2, ttl_later).is_accepted());

    let newcomer = LINK_MAX_GATE_KEYS as u64;
    assert!(admit_from(&mut gate, newcomer, 1, ttl_later).is_accepted());
    assert_eq!(gate.counts().evicted, 1);
    assert_eq!(gate.counts().untracked, 0);

    // Sender 0 kept its slot, so its next frame is gated on the seq that stream actually reached
    // rather than admitted as a fresh one.
    assert!(!admit_from(&mut gate, 0, 2, ttl_later).is_accepted());
    assert_eq!(gate.counts().stale, 1);
}

/// Eviction reclaims DEAD slots only: dropping a stream still carrying traffic would trade a
/// visible lockout for silent gaps in a feed nobody was told about.
#[test]
fn gate_refuses_a_new_stream_while_every_slot_is_live() {
    let mut gate = SequenceGate::new();
    for sender in 0..LINK_MAX_GATE_KEYS as u64 {
        assert!(admit_from(&mut gate, sender, 1, GATE_START).is_accepted());
    }
    let inside_ttl = GATE_START + LINK_SUBSCRIPTION_TTL - DurationUs::from_micros(1);
    let newcomer = LINK_MAX_GATE_KEYS as u64;
    assert!(!admit_from(&mut gate, newcomer, 1, inside_ttl).is_accepted());
    assert_eq!(gate.counts().untracked, 1);
    assert_eq!(gate.counts().evicted, 0);
}

fn subscriber(port: u16) -> SocketAddr {
    format!("127.0.0.1:{port}")
        .parse()
        .expect("a literal loopback address")
}

/// Soft state means a dead subscriber needs no teardown — but it also means the feed stops silently,
/// so the expiry boundary is the thing the staleness reporting is built on.
#[test]
fn subscription_expires_at_its_ttl() {
    let mut table = SubscriberTable::new();
    let start = TsUs::from_micros(1_000_000);
    assert_eq!(
        table.refresh(subscriber(9310), TopicSet::ALL, start),
        RefreshOutcome::Added
    );
    let alive = start + LINK_SUBSCRIPTION_TTL - DurationUs::from_micros(1);
    assert_eq!(table.recipients(TopicId::BOOKS, alive).count(), 1);
    let expired = start + LINK_SUBSCRIPTION_TTL;
    assert_eq!(table.recipients(TopicId::BOOKS, expired).count(), 0);

    assert_eq!(
        table.refresh(subscriber(9310), TopicSet::ALL, expired),
        RefreshOutcome::Added
    );
    assert_eq!(table.len(), 1);
}

#[test]
fn subscription_renews_in_place_and_honours_its_topic_set() {
    let mut table = SubscriberTable::new();
    let now = TsUs::from_micros(1_000_000);
    table.refresh(subscriber(9310), TopicSet::ALL, now);
    assert_eq!(
        table.refresh(
            subscriber(9310),
            TopicSet::new(&[TopicId::BOOKS]).expect("one topic"),
            now
        ),
        RefreshOutcome::Renewed
    );
    assert_eq!(table.len(), 1);
    assert_eq!(table.recipients(TopicId::BOOKS, now).count(), 1);
    assert_eq!(table.recipients(TopicId::EVENTS, now).count(), 0);
}

/// A capacity hit is a designed event: the table refuses the subscription and reports the refusal
/// for the actor to count, rather than growing without bound because a port is reachable.
#[test]
fn full_subscriber_table_rejects_and_counts() {
    let mut table = SubscriberTable::new();
    let now = TsUs::from_micros(1_000_000);
    for port in 0..LINK_MAX_SUBSCRIBERS as u16 {
        assert_eq!(
            table.refresh(subscriber(9310 + port), TopicSet::ALL, now),
            RefreshOutcome::Added
        );
    }
    let overflow = subscriber(9310 + LINK_MAX_SUBSCRIBERS as u16);
    assert_eq!(
        table.refresh(overflow, TopicSet::ALL, now),
        RefreshOutcome::Rejected
    );
    assert_eq!(table.len(), LINK_MAX_SUBSCRIBERS);

    let after_ttl = now + LINK_SUBSCRIPTION_TTL;
    assert_eq!(
        table.refresh(overflow, TopicSet::ALL, after_ttl),
        RefreshOutcome::Added
    );
    assert_eq!(table.len(), 1);
}
