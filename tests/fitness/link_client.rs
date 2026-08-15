//! Workstation-side link fitness: what the UI does with the datagrams the engine sends it. Three
//! invariants, each one a silent-corruption risk rather than a crash. The catalog is announced as
//! loose per-item frames, so an assembly that completed early or adopted a stale epoch would label
//! every DOM row and feature value with the wrong name. The feed rides UDP, so loss must read as a
//! counted gap and never as a book that travelled backwards. And the control epoch is what decides
//! whether this workstation can stop an engine at all — or seizes one another controller holds.

use std::net::SocketAddr;

use polysim::config::ExecutionMode;
use polysim::desktop::link_model::{CatalogAssembly, ControlVerdict, Controller};
use polysim::desktop::model::UiModel;
use polysim::ids::{AssetId, InstrumentId, Price, Qty, Side};
use polysim::link::{
    CatalogFeature, CatalogInstrument, Envelope, FrameGuard, GateVerdict, Lifecycle, LinkBody,
    LinkDatagram, LinkHash, LinkIdentity, RunPhase, RunState, SequenceGate, TopicId, WireName,
};
use polysim::msg::inbound::Level;
use polysim::msg::persist::FeatureId;
use polysim::msg::ui::{UI_BOOK_LEVELS, UiBookSnapshot, UiBookState, UiEvent};
use polysim::time::{DurationUs, TsUs};
use proptest::prelude::*;

const CATALOG_EPOCH: TsUs = TsUs::from_micros(1_700_000_000_000_000);
const SPIN_INTERVAL: DurationUs = DurationUs::from_micros(1_000_000);
const SPIN_INTERVAL_US: u64 = SPIN_INTERVAL.micros() as u64;

/// One instant for the whole feed: this workstation tracks a single engine, so no gate slot is ever
/// competing for capacity and none can age out mid-property.
const GATE_NOW: TsUs = TsUs::from_micros(1_700_000_000_000_000);

fn peer() -> SocketAddr {
    "127.0.0.1:9310"
        .parse()
        .expect("the fixture peer address is a literal")
}

fn guard() -> FrameGuard {
    FrameGuard {
        token_hash: LinkHash::of_name(""),
        strategy_hash: LinkHash::of_name("strat-fitness"),
        schema_hash: LinkHash::of_fields(&[]),
    }
}

fn identity() -> LinkIdentity {
    LinkIdentity {
        token_hash: LinkHash::of_name(""),
        strategy_hash: LinkHash::of_name("strat-fitness"),
        sender_te_hash: LinkHash::of_name("te-fitness"),
        boot_ts_us: TsUs::from_micros(1_000),
    }
}

fn instrument_frame(index: u16, total: u16, epoch: TsUs) -> CatalogInstrument {
    CatalogInstrument {
        catalog_ts_us: epoch,
        total_count: total,
        instrument: InstrumentId(index),
        display: WireName::new(&format!("INSTRUMENT-{index}")),
        tick_size: Some(Price(10 * i64::from(index) + 1)),
        lot_size: Some(Qty(100 * i64::from(index) + 1)),
        qty_scale: 100_000_000,
        base_asset: AssetId(2 * index),
        quote_asset: AssetId(2 * index + 1),
        base: WireName::new(&format!("BASE{index}")),
        quote: WireName::new(&format!("QUOTE{index}")),
    }
}

fn feature_frame(index: u16, total: u16, epoch: TsUs) -> CatalogFeature {
    CatalogFeature {
        catalog_ts_us: epoch,
        total_count: total,
        feature: FeatureId(index),
        name: WireName::new(&format!("feature_{index}")),
    }
}

fn lifecycle(acknowledged_epoch: u64, run_state: RunState) -> Lifecycle {
    Lifecycle {
        phase: RunPhase::Ready,
        run_state,
        execution_mode: Some(ExecutionMode::Live),
        acknowledged_epoch,
        spin_interval_us: SPIN_INTERVAL,
        feature_count: 0,
    }
}

/// The heartbeat an engine declaring `features` columns sends. It is the heartbeat and not the
/// catalog frames that carries the total, so a subscriber knows how many features to wait for
/// before the first — or the zeroth — of them arrives.
fn reported(feature_count: u16) -> Lifecycle {
    Lifecycle {
        feature_count,
        ..lifecycle(0, RunState::Running)
    }
}

/// Which catalog frame an announcement carries. Instruments and features share a `catalog_ts_us`
/// but declare their own totals, so an assembly must satisfy both before it can be complete.
#[derive(Debug, Clone, Copy)]
enum Announcement {
    Instrument(u16),
    Feature(u16),
}

fn announcements(instruments: u16, features: u16) -> Vec<Announcement> {
    (0..instruments)
        .map(Announcement::Instrument)
        .chain((0..features).map(Announcement::Feature))
        .collect()
}

fn accept(assembly: &mut CatalogAssembly, item: Announcement, totals: (u16, u16), epoch: TsUs) {
    match item {
        Announcement::Instrument(index) => {
            assembly.accept_instrument(instrument_frame(index, totals.0, epoch));
        }
        Announcement::Feature(index) => {
            assembly.accept_feature(feature_frame(index, totals.1, epoch));
        }
    }
}

proptest! {
    /// FITNESS: the catalog completes exactly when every declared item has arrived, and never one
    /// frame earlier — whatever order the datagrams land in and however many are duplicated. An
    /// assembly that completed early would hand the monitor a short `feature_names` list and every
    /// feature row after the hole would be labelled with its neighbour's name.
    #[test]
    fn catalog_completes_only_once_every_declared_item_has_arrived(
        instruments in 1u16..6,
        features in 1u16..8,
        order in prop::collection::vec(0usize..64, 0..64),
        duplicates in prop::collection::vec(0usize..64, 0..24),
    ) {
        let items = announcements(instruments, features);
        let totals = (instruments, features);
        let mut assembly = CatalogAssembly::new();

        // Deliver an arbitrary (possibly repeating, possibly incomplete) prefix first.
        let mut delivered = vec![false; items.len()];
        for pick in order.iter().chain(duplicates.iter()) {
            let index = pick % items.len();
            accept(&mut assembly, items[index], totals, CATALOG_EPOCH);
            delivered[index] = true;
            let is_complete = delivered.iter().all(|seen| *seen);
            prop_assert_eq!(
                assembly.build("strat-fitness", peer(), reported(features)).is_some(),
                is_complete,
                "completion must track exactly the set of distinct items delivered"
            );
        }

        // Then everything, so the run always reaches a complete catalog and it matches the source.
        for (index, item) in items.iter().enumerate() {
            accept(&mut assembly, *item, totals, CATALOG_EPOCH);
            delivered[index] = true;
        }
        let catalog = assembly
            .build("strat-fitness", peer(), reported(features))
            .expect("every declared item has now arrived");
        prop_assert_eq!(catalog.instruments.len(), usize::from(instruments));
        prop_assert_eq!(catalog.feature_names.len(), usize::from(features));
        prop_assert_eq!(catalog.spin_interval_us, SPIN_INTERVAL_US);
        for (position, row) in catalog.instruments.iter().enumerate() {
            prop_assert_eq!(row.instrument_id, InstrumentId(position as u16));
            prop_assert_eq!(row.display.as_ref(), format!("INSTRUMENT-{position}"));
            prop_assert_eq!(row.tick_size, Some(Price(10 * position as i64 + 1)));
        }
        for (index, name) in catalog.feature_names.iter().enumerate() {
            prop_assert_eq!(name.as_ref(), format!("feature_{index}"));
        }
    }
}

/// Epoch discipline cuts both ways: a NEWER epoch discards whatever the old one gathered (a
/// restarted engine announcing fewer features must not inherit the old run's count), and an OLDER,
/// delayed datagram must not mix into the epoch that has already superseded it. `build` separately
/// validates a frame's claimed total two different ways, and an untrusted sender can get either
/// wrong without the workstation panicking: a dense feature list with a hole in it (an id past the
/// frame's OWN declared total) must never complete, and a sender whose per-frame total disagrees with
/// the heartbeat's yields ids that do not tile the assembled list — a peer's arithmetic must never
/// panic us, since anyone who can reach the port can forge a frame. A trading engine that declares no
/// feature columns (the polymarket publisher's own shape) announces no feature frame at all, so a
/// total learned from those frames alone would never arrive; the count must ride the once-a-second
/// heartbeat instead.
#[test]
fn catalog_assembly_epoch_and_total_validation() {
    {
        let mut assembly = CatalogAssembly::new();
        assembly.accept_instrument(instrument_frame(0, 1, CATALOG_EPOCH));
        assembly.accept_feature(feature_frame(0, 2, CATALOG_EPOCH));

        // The restarted engine announces ONE feature where the old run had two.
        let next = CATALOG_EPOCH + DurationUs::RESOLUTION;
        assembly.accept_instrument(instrument_frame(0, 1, next));
        assert!(
            assembly
                .build("strat-fitness", peer(), reported(1))
                .is_none(),
            "the previous epoch's feature must not count towards the new one"
        );

        assembly.accept_feature(feature_frame(0, 1, next));
        let catalog = assembly
            .build("strat-fitness", peer(), reported(1))
            .expect("the new epoch is complete");
        assert_eq!(catalog.feature_names.len(), 1);
    }

    let mut assembly = CatalogAssembly::new();
    let next = CATALOG_EPOCH + DurationUs::RESOLUTION;
    assembly.accept_instrument(instrument_frame(0, 1, next));
    assembly.accept_feature(feature_frame(0, 1, next));

    // A datagram from the previous epoch, delayed on the wire.
    assembly.accept_feature(feature_frame(0, 4, CATALOG_EPOCH));

    let catalog = assembly
        .build("strat-fitness", peer(), reported(1))
        .expect("the current epoch stays complete");
    assert_eq!(catalog.feature_names.len(), 1);
    assert_eq!(catalog.feature_names[0].as_ref(), "feature_0");

    totals_validation_cases();
}

fn totals_validation_cases() {
    struct Case {
        name: &'static str,
        instruments: &'static [(u16, u16)],
        features: &'static [(u16, u16)],
        heartbeat_features: u16,
        expect: Option<(usize, usize)>,
    }
    let cases = [
        Case {
            name: "feature id past its own frame's declared total is refused",
            instruments: &[(0, 1)],
            features: &[(0, 2), (7, 2)],
            heartbeat_features: 2,
            expect: None,
        },
        Case {
            name: "an engine with no feature columns completes from the heartbeat total",
            instruments: &[(0, 1)],
            features: &[],
            heartbeat_features: 0,
            expect: Some((1, 0)),
        },
        Case {
            name: "a sender whose per-frame total disagrees with the heartbeat never panics",
            instruments: &[(0, 1)],
            features: &[(0, 10), (7, 10)],
            heartbeat_features: 2,
            expect: None,
        },
    ];

    for case in cases {
        let mut assembly = CatalogAssembly::new();
        for &(index, total) in case.instruments {
            assembly.accept_instrument(instrument_frame(index, total, CATALOG_EPOCH));
        }
        for &(index, total) in case.features {
            assembly.accept_feature(feature_frame(index, total, CATALOG_EPOCH));
        }
        let built = assembly.build("strat-fitness", peer(), reported(case.heartbeat_features));
        match case.expect {
            None => assert!(built.is_none(), "case {}: expected no catalog", case.name),
            Some((instruments, features)) => {
                let catalog =
                    built.unwrap_or_else(|| panic!("case {}: expected a catalog", case.name));
                assert_eq!(
                    catalog.instruments.len(),
                    instruments,
                    "case {}: instrument count",
                    case.name
                );
                assert_eq!(
                    catalog.feature_names.len(),
                    features,
                    "case {}: feature count",
                    case.name
                );
            }
        }
    }
}

fn book_snapshot(seq: u64, best_bid: i64) -> UiBookSnapshot {
    let mut bids = [Level {
        price: Price(0),
        qty: Qty(0),
    }; UI_BOOK_LEVELS];
    bids[0] = Level {
        price: Price(best_bid),
        qty: Qty(1_000),
    };
    UiBookSnapshot {
        instrument: InstrumentId(0),
        seq,
        event_ts_us: TsUs::from_micros(seq as i64 * 1_000),
        state: UiBookState::Valid,
        bid_len: 1,
        ask_len: 0,
        bids,
        asks: [Level {
            price: Price(0),
            qty: Qty(0),
        }; UI_BOOK_LEVELS],
    }
}

fn trade_event(seq: u64) -> UiEvent {
    UiEvent::Trade {
        instrument: InstrumentId(0),
        seq,
        event_ts_us: TsUs::from_micros(seq as i64 * 1_000),
        aggressor: Side::Buy,
        price: Price(100 + seq as i64),
        qty: Qty(7),
    }
}

fn datagram(topic: TopicId, seq: u64, body: LinkBody) -> Vec<u8> {
    let mut buffer = [0u8; polysim::link::LINK_MAX_DATAGRAM];
    let written = LinkDatagram {
        envelope: Envelope::new(identity(), topic, seq),
        body,
    }
    .encode(&mut buffer);
    buffer[..written].to_vec()
}

proptest! {
    /// FITNESS: a lossy, duplicating, reordering network must reach the workstation's model as
    /// counted gaps and a monotonic latest — never as a book that travelled backwards. This is the
    /// whole justification for carrying the feed on UDP with no reliability layer, so it is pinned
    /// against the REAL decode + sequence gate + model fold, not a stand-in for them.
    #[test]
    fn a_lossy_feed_counts_its_gaps_and_never_travels_backwards(
        count in 2usize..40,
        keep in prop::collection::vec(any::<bool>(), 40),
        replay_last in any::<bool>(),
        deliver_stale in any::<bool>(),
    ) {
        let mut gate = SequenceGate::new();
        let mut model = UiModel::with_capacity(1, SPIN_INTERVAL);
        let guard = guard();

        let mut accepted_books: Vec<u64> = Vec::new();
        let mut accepted_events: Vec<u64> = Vec::new();
        for (index, is_delivered) in keep.iter().enumerate().take(count) {
            let seq = index as u64 + 1;
            if !is_delivered {
                continue;
            }
            for (topic, body, accepted) in [
                (TopicId::BOOKS, LinkBody::Book(book_snapshot(seq, 500 + seq as i64)), &mut accepted_books),
                (TopicId::EVENTS, LinkBody::Event(trade_event(seq)), &mut accepted_events),
            ] {
                let bytes = datagram(topic, seq, body);
                let decoded = LinkDatagram::decode(&bytes, &guard)
                    .expect("a frame this side encoded must decode");
                if gate.admit(&decoded.envelope, GATE_NOW) != GateVerdict::Accepted {
                    continue;
                }
                accepted.push(seq);
                match decoded.body {
                    LinkBody::Book(snapshot) => model.apply_book(snapshot),
                    LinkBody::Event(event) => model.apply_event(event),
                    other => panic!("unexpected body {other:?}"),
                }
            }
        }
        prop_assume!(!accepted_books.is_empty());

        // A duplicate and a rewind, the two things UDP hands a consumer that an in-process ring
        // cannot: both must die at the gate rather than reach the fold.
        let expected_latest = *accepted_books.last().expect("checked non-empty");
        if replay_last {
            let bytes = datagram(
                TopicId::BOOKS,
                expected_latest,
                LinkBody::Book(book_snapshot(expected_latest, 1)),
            );
            let decoded = LinkDatagram::decode(&bytes, &guard).expect("round trip");
            prop_assert!(!gate.admit(&decoded.envelope, GATE_NOW).is_accepted());
        }
        if deliver_stale {
            let bytes = datagram(TopicId::BOOKS, 1, LinkBody::Book(book_snapshot(1, 2)));
            let decoded = LinkDatagram::decode(&bytes, &guard).expect("round trip");
            prop_assert!(!gate.admit(&decoded.envelope, GATE_NOW).is_accepted());
        }

        let latest = model
            .book(InstrumentId(0))
            .expect("at least one snapshot was accepted");
        prop_assert_eq!(latest.seq, expected_latest);
        prop_assert_eq!(latest.bids[0].price, Price(500 + expected_latest as i64));

        let span = |accepted: &[u64]| match accepted.split_first() {
            Some((first, rest)) => {
                accepted.last().unwrap_or(first) - first - rest.len() as u64
            }
            None => 0,
        };
        prop_assert_eq!(model.book_gaps(), span(&accepted_books));
        prop_assert_eq!(model.event_gaps(), span(&accepted_events));
    }
}

/// The whole trust model of the control epoch in one place: 0 is the reserved "no opinion" that lets
/// a workstation watch an engine without taking control away from whoever holds it; asserting yields
/// an epoch strictly above 0, releasing drops the current assertion back to "no opinion" without
/// rewinding the underlying counter, and a later boot stamp — a restarted workstation — must outrank
/// this process's own previous run rather than reuse its epochs. Against an engine's acknowledged
/// epoch, an older one reads as still in flight, a match reads as applied, and a higher one reads as
/// lost to another controller.
#[test]
fn controller_epoch_rises_and_its_verdict_tracks_the_engines_acknowledgement() {
    let mut controller = Controller::new(TsUs::from_micros(1_700_000_000_000_000));

    assert_eq!(
        controller.assertion().epoch,
        0,
        "epoch 0 is the reserved 'no opinion' — it is what lets a workstation watch an engine \
         without taking control of it away from whoever holds it"
    );
    assert_eq!(controller.asserted(), None);
    assert_eq!(controller.verdict(None), ControlVerdict::NoOpinion);

    controller.assert(RunState::Idle);
    let opening = controller.assertion();
    assert_eq!(opening.state, RunState::Idle);
    assert!(
        opening.epoch > 0,
        "the engine accepts on a strict >, so 0 could never win"
    );

    controller.release();
    assert_eq!(controller.assertion().epoch, 0);
    assert_eq!(controller.verdict(None), ControlVerdict::NoOpinion);

    controller.assert(RunState::Running);
    assert!(
        controller.assertion().epoch > opening.epoch,
        "epochs are per-controller and monotonic — attaching to another engine must not rewind them"
    );
    let held = controller.assertion().epoch;

    // A later boot stamp is what makes a restarted workstation outrank its own previous run.
    let mut restarted = Controller::new(TsUs::from_micros(1_700_000_060_000_000));
    restarted.assert(RunState::Idle);
    assert!(restarted.assertion().epoch > held);

    let mut controller = Controller::new(TsUs::from_micros(1_000));
    controller.assert(RunState::Idle);
    let mine = controller.assertion().epoch;

    assert_eq!(
        controller.verdict(Some(lifecycle(mine - 1, RunState::Running))),
        ControlVerdict::Pending,
        "an older acknowledged epoch means our frame has not landed yet"
    );
    assert_eq!(
        controller.verdict(Some(lifecycle(mine, RunState::Idle))),
        ControlVerdict::Applied
    );
    assert_eq!(
        controller.verdict(Some(lifecycle(mine + 5, RunState::Running))),
        ControlVerdict::Lost {
            holder_epoch: mine + 5
        },
        "a higher epoch holds the engine, and re-asserting ours would never take it back"
    );
}
