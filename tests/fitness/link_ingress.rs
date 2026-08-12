//! The link's engine seam: a peer frame crossing the edge into `on_link`, the tape dispatch tees on
//! the way past, and a payload a strategy banks on its way out.
//!
//! The edge pipeline is exercised without a socket, which is deliberate: the wire format and the
//! gate LOGIC belong in this suite, while socket behaviour belongs to contract-seam and
//! integration tests. What is architectural here is the composition — that the gate sits
//! upstream of the ring, so the sequence the hot thread consumes IS the record.

use polysim::config::{RecordedTables, TableKind};
use polysim::hot::strategy::{Registration, Strategy, StrategyCtx};
use polysim::ids::InstrumentId;
use polysim::link::{
    Envelope, FrameGuard, InboundLink, LINK_MAX_DATAGRAM, LINK_MAX_FIELDS, LinkBody, LinkDatagram,
    LinkFrame, LinkHash, LinkIdentity, LinkOrigin, LinkPayload, SequenceGate, TopicId,
    schema_hash_of_fields,
};
use polysim::msg::inbound::{InboundMessage, SpinTick};
use polysim::msg::persist::{FeatureId, LinkFrameRow, LinkRowKind, PersistRecord};
use polysim::time::{DurationUs, TsUs};

use crate::engine_support::{
    LinkedEngine, LinkedSetup, engine_with_link, idle_at, instrument_row, pop, run_control, spin,
    tracker_spec_all, ts,
};

const FIELDS: [&str; 2] = ["peer_mid", "peer_confidence"];
const TOPICS: [&str; 1] = ["signals"];

/// Echoes whatever a peer sent as features, and publishes one payload per spin — so both directions
/// of the seam are observable in the persist tape and the outbound ring.
struct LinkProbe {
    received: Option<FeatureId>,
    topic: Option<TopicId>,
}

impl Strategy for LinkProbe {
    fn features(&self) -> &'static [&'static str] {
        &["received"]
    }

    fn link_fields(&self) -> &'static [&'static str] {
        &FIELDS
    }

    fn link_topics(&self) -> &'static [&'static str] {
        &TOPICS
    }

    fn register(&mut self, registration: Registration<'_>) {
        self.received = registration.features.first().copied();
        self.topic = registration.link_topics.first().copied();
    }

    fn on_link(&mut self, ctx: &mut StrategyCtx<'_>, frame: &LinkFrame) {
        for value in frame.payload.values() {
            ctx.emit(self.received.expect("registered"), InstrumentId(0), *value);
        }
    }

    fn on_spin(&mut self, ctx: &mut StrategyCtx<'_>, tick: &SpinTick) {
        ctx.link_send(self.topic.expect("registered"), &[tick.seq as f64, 0.5]);
    }
}

fn linked(tables: &[TableKind]) -> LinkedEngine {
    let instruments = [instrument_row(0, tracker_spec_all(1), 64)];
    engine_with_link(LinkedSetup {
        instruments: &instruments,
        strategy: Box::new(LinkProbe {
            received: None,
            topic: None,
        }),
        tables: RecordedTables::new(tables),
        warmup: DurationUs::ZERO,
    })
}

const SENDER: LinkHash = LinkHash::of_name("peer-te");

fn identity() -> LinkIdentity {
    LinkIdentity {
        token_hash: LinkHash::of_name(""),
        strategy_hash: LinkHash::of_name("strat"),
        sender_te_hash: SENDER,
        boot_ts_us: ts(1_000),
    }
}

fn guard() -> FrameGuard {
    FrameGuard {
        token_hash: LinkHash::of_name(""),
        strategy_hash: LinkHash::of_name("strat"),
        schema_hash: schema_hash_of_fields(&FIELDS),
    }
}

/// The edge the link actor runs, minus the syscall: encode as a peer would, decode against the
/// guard, gate the sequence, then stamp LOCAL ingress times. The sender's `event_ts_us` rides along
/// as data and is never the ordering key.
struct Edge {
    gate: SequenceGate,
    buffer: [u8; LINK_MAX_DATAGRAM],
}

impl Edge {
    fn new() -> Self {
        Self {
            gate: SequenceGate::new(),
            buffer: [0; LINK_MAX_DATAGRAM],
        }
    }

    fn admit(&mut self, seq: u64, values: &[f64], received: i64) -> Option<InboundMessage> {
        let payload = LinkPayload::new(schema_hash_of_fields(&FIELDS), ts(received - 1), values);
        let datagram = LinkDatagram {
            envelope: Envelope::new(identity(), TopicId::FIRST_STRATEGY, seq),
            body: LinkBody::Payload(payload),
        };
        let len = datagram.encode(&mut self.buffer);
        let decoded = LinkDatagram::decode(&self.buffer[..len], &guard()).expect("peer frame");
        if !self
            .gate
            .admit(&decoded.envelope, ts(received))
            .is_accepted()
        {
            return None;
        }
        let LinkBody::Payload(payload) = decoded.body else {
            panic!("a strategy topic decodes to a payload");
        };
        Some(InboundMessage::Link(InboundLink {
            frame: LinkFrame {
                origin: LinkOrigin::from(&decoded.envelope),
                payload,
            },
            received_ts_us: ts(received),
            queued_ts_us: ts(received),
        }))
    }
}

fn drain(linked: &mut LinkedEngine) -> Vec<PersistRecord> {
    let mut records = Vec::new();
    while let Ok(record) = linked.persist.pop() {
        records.push(record);
    }
    records
}

fn link_rows(records: &[PersistRecord]) -> Vec<LinkFrameRow> {
    records
        .iter()
        .filter_map(|record| match record {
            PersistRecord::LinkFrame(row) => Some(*row),
            _ => None,
        })
        .collect()
}

fn features(records: &[PersistRecord]) -> Vec<f64> {
    records
        .iter()
        .filter_map(|record| match record {
            PersistRecord::Feature(row) => Some(row.value),
            _ => None,
        })
        .collect()
}

/// FITNESS: a peer frame reaches the strategy AND the tape, and the tape is written by dispatch on
/// the hot side.
///
/// Recording at the link actor instead would sit upstream of the drop-and-count input ring, so a
/// dropped frame would leave the tape holding one the hot thread never consumed — and a replay of
/// that tape would diverge from the run it claims to describe.
#[test]
fn a_peer_frame_reaches_the_strategy_and_the_tape() {
    let mut linked = linked(&[TableKind::Features, TableKind::LinkFrames]);
    let mut edge = Edge::new();
    let message = edge.admit(1, &[42.5, 0.25], 100).expect("gate admits");
    linked.engine.dispatch(pop(0, 0), &message);

    let records = drain(&mut linked);
    assert_eq!(
        features(&records),
        vec![42.5, 0.25],
        "on_link saw the sender's slots, in slot order"
    );

    let rows = link_rows(&records);
    assert_eq!(rows.len(), 2, "one tape row per value slot");
    assert!(
        rows.iter().all(|row| row.kind == LinkRowKind::Payload
            && row.sender_te_hash == SENDER.0
            && row.topic == TopicId::FIRST_STRATEGY.0
            && row.seq == 1
            && row.count == 2
            && row.received_ts_us == ts(100)
            && row.event_ts_us == ts(99)),
        "every row carries the frame's identity and BOTH clocks, {rows:?}"
    );
    assert_eq!(
        rows.iter()
            .map(|row| (row.slot, row.value))
            .collect::<Vec<_>>(),
        vec![(0, 42.5), (1, 0.25)],
        "slots keep their index"
    );
}

/// FITNESS: the sequence gate sits UPSTREAM of the hot thread, so the sequence the engine consumes
/// is already deduplicated and in order — which is what makes the consumed sequence the record.
#[test]
fn the_sequence_gate_keeps_duplicates_and_reorders_off_the_hot_thread() {
    let mut linked = linked(&[TableKind::Features, TableKind::LinkFrames]);
    let mut edge = Edge::new();
    let mut admitted = Vec::new();
    for (seq, value) in [(1, 1.0), (2, 2.0), (2, 22.0), (1, 11.0), (3, 3.0)] {
        let Some(message) = edge.admit(seq, &[value], 100 + seq as i64) else {
            continue;
        };
        admitted.push(seq);
        linked.engine.dispatch(pop(0, 0), &message);
    }
    assert_eq!(
        admitted,
        vec![1, 2, 3],
        "the duplicate and the reorder never entered the ring"
    );

    let records = drain(&mut linked);
    assert_eq!(features(&records), vec![1.0, 2.0, 3.0]);
    assert_eq!(
        link_rows(&records)
            .iter()
            .map(|row| row.seq)
            .collect::<Vec<_>>(),
        vec![1, 2, 3],
        "and the tape holds exactly what was consumed"
    );
}

/// FITNESS: the tape is a table like any other, so `strategy.tables` decides whether it exists —
/// and a run that does not record it still delivers every frame.
#[test]
fn the_tape_is_gated_by_the_table_set_but_delivery_is_not() {
    let mut linked = linked(&[TableKind::Features]);
    let mut edge = Edge::new();
    let message = edge.admit(1, &[7.0], 100).expect("gate admits");
    linked.engine.dispatch(pop(0, 0), &message);

    let records = drain(&mut linked);
    assert_eq!(features(&records), vec![7.0], "on_link fired regardless");
    assert!(
        link_rows(&records).is_empty(),
        "no link_frames sink, so no rows"
    );
}

/// FITNESS: the tape records what the HOT THREAD consumed, whether or not the strategy was shown it.
///
/// A parked engine still consumes frames off its ring, so they belong in the record — a replay of a
/// tape missing them would start from a different input sequence than the run did.
#[test]
fn a_parked_engine_records_frames_it_does_not_deliver() {
    let mut linked = linked(&[TableKind::Features, TableKind::LinkFrames]);
    let mut edge = Edge::new();
    linked
        .engine
        .dispatch(pop(0, 0), &run_control(idle_at(1), 50));
    drain(&mut linked);

    let message = edge.admit(1, &[7.0], 100).expect("gate admits");
    linked.engine.dispatch(pop(0, 0), &message);
    let records = drain(&mut linked);
    assert!(
        features(&records).is_empty(),
        "parked: on_link is suppressed with every other callback"
    );
    assert_eq!(
        link_rows(&records).len(),
        1,
        "but the frame the hot thread consumed is in the record"
    );
}

/// FITNESS: `link_send` banks against the strategy's OWN declared schema and its registered topic,
/// so two engines of one strategy agree structurally rather than by convention.
#[test]
fn link_send_banks_the_declared_schema_on_a_registered_topic() {
    let mut linked = linked(&[TableKind::Features]);
    linked
        .engine
        .dispatch(pop(0, 0), &InboundMessage::SpinTick(spin(4, 200)));

    let outbound = linked.outbound.pop().expect("the spin banked one payload");
    assert_eq!(
        outbound.topic,
        TopicId::FIRST_STRATEGY,
        "the first declared topic takes the first strategy id"
    );
    assert_eq!(
        outbound.payload.schema_hash,
        schema_hash_of_fields(&FIELDS),
        "the digest a peer gates on comes from link_fields(), not from the call site"
    );
    assert_eq!(outbound.payload.values(), &[4.0, 0.5]);
    assert_eq!(
        outbound.payload.event_ts_us,
        TsUs::from_micros(200),
        "stamped with the event's own time rather than a clock read, so a replayed callback banks \
         a byte-identical frame"
    );
    assert!(
        linked.outbound.pop().is_err(),
        "one spin, one frame — nothing else leaked onto the lane"
    );
}

/// FITNESS: the slot count is a code-level fact from `link_fields()`, so overrunning it is an
/// invariant violation that must fail loud, never a silently truncated frame.
#[test]
#[should_panic(expected = "capacity")]
fn an_overlong_payload_panics_rather_than_truncating() {
    LinkPayload::new(
        LinkHash::of_name("x"),
        ts(1),
        &vec![0.0; LINK_MAX_FIELDS + 1],
    );
}

/// FITNESS: engine-reserved topic ids are not a strategy's to publish on — a payload landing on the
/// books or lifecycle topic would be decoded by the far side as the wrong body entirely.
#[test]
#[should_panic(expected = "engine-reserved topic")]
fn link_send_refuses_an_engine_topic() {
    struct Rogue;
    impl Strategy for Rogue {
        fn on_spin(&mut self, ctx: &mut StrategyCtx<'_>, _tick: &SpinTick) {
            ctx.link_send(TopicId::BOOKS, &[1.0]);
        }
    }
    dispatch_one_spin(Box::new(Rogue));
}

/// FITNESS: a topic id above the ones registration handed out is refused on the HOT thread. The
/// link actor sizes its per-topic sequence array from the same declared count and indexes it by raw
/// id, so an id nobody declared would take the actor's task down — a link that dies silently while
/// the engine keeps trading, instead of a panic naming the strategy that made the id up.
#[test]
#[should_panic(expected = "undeclared link topic")]
fn link_send_refuses_a_topic_past_the_declared_count() {
    struct Rogue;
    impl Strategy for Rogue {
        fn link_topics(&self) -> &'static [&'static str] {
            &TOPICS
        }

        fn on_spin(&mut self, ctx: &mut StrategyCtx<'_>, _tick: &SpinTick) {
            ctx.link_send(TopicId::strategy(TOPICS.len()), &[1.0]);
        }
    }
    dispatch_one_spin(Box::new(Rogue));
}

fn dispatch_one_spin(strategy: Box<dyn Strategy>) {
    let instruments = [instrument_row(0, tracker_spec_all(1), 64)];
    let mut linked = engine_with_link(LinkedSetup {
        instruments: &instruments,
        strategy,
        tables: RecordedTables::default(),
        warmup: DurationUs::ZERO,
    });
    linked
        .engine
        .dispatch(pop(0, 0), &InboundMessage::SpinTick(spin(1, 1)));
}
