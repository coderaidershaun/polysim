//! A workstation that attaches to a running engine is sent the chart window it missed. The test
//! drives the workstation's own link session over a real UDP socket, so it needs an engine already
//! running and reachable. Point it with `POLYSIM_LINK_PEER` (default `127.0.0.1:9310`),
//! `POLYSIM_LINK_STRATEGY` (default `strat-micro-recorder`) and `POLYSIM_LINK_TOKEN` (default
//! empty).

use std::collections::{HashMap, HashSet};
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use polysim::config::StrategyId;
use polysim::ids::InstrumentId;
use polysim::link::{LINK_MAX_DATAGRAM, LinkHash};
use polysim::msg::ui::{UI_CHART_WINDOW, UiEvent, UiHistory};
use polysim::time::TsUs;
use polysim::workstation::link_session::{Delivery, LinkSession, LinkTarget};

const DEADLINE: Duration = Duration::from_secs(20);
const QUIET_AFTER_LAST_CHUNK: Duration = Duration::from_millis(500);

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn wall_now() -> TsUs {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_micros();
    TsUs::from_micros(i64::try_from(micros).expect("microseconds fit an i64"))
}

#[derive(Default)]
struct Received {
    is_catalog_held: bool,
    is_catalog_held_at_first_history: Option<bool>,
    chunks: HashMap<InstrumentId, Vec<UiHistory>>,
    rotated: HashSet<InstrumentId>,
    last_chunk_at: Option<Instant>,
}

impl Received {
    fn take(&mut self, delivery: Delivery) {
        match delivery {
            Delivery::Catalog(_) => self.is_catalog_held = true,
            Delivery::Event(UiEvent::Rotation { instrument, .. }) => {
                self.rotated.insert(instrument);
            }
            Delivery::History(history) => {
                self.is_catalog_held_at_first_history
                    .get_or_insert(self.is_catalog_held);
                self.last_chunk_at = Some(Instant::now());
                self.chunks
                    .entry(history.instrument)
                    .or_default()
                    .push(history);
            }
            Delivery::Book(_) | Delivery::Event(_) => {}
        }
    }

    /// A rotation ends that instrument's backfill early on purpose, so it never counts as
    /// unfinished.
    fn is_backfill_complete(&self) -> bool {
        let Some(last) = self.last_chunk_at else { return false };
        let mut unrotated = self
            .chunks
            .iter()
            .filter(|(instrument, _)| !self.rotated.contains(instrument));
        let is_every_instrument_complete = unrotated.all(|(_, chunks)| {
            let cutoff = chunks
                .last()
                .expect("a listed instrument has a chunk")
                .cutoff_ts_us;
            highest_stamp(chunks) == Some(cutoff)
        });
        is_every_instrument_complete && last.elapsed() >= QUIET_AFTER_LAST_CHUNK
    }
}

fn highest_stamp(chunks: &[UiHistory]) -> Option<TsUs> {
    chunks
        .iter()
        .flat_map(|chunk| chunk.samples())
        .map(|sample| sample.event_ts_us())
        .max()
}

/// Real-network test: needs an engine on `POLYSIM_LINK_PEER` that has been running a while.
#[test]
#[ignore = "needs a running engine on POLYSIM_LINK_PEER (default 127.0.0.1:9310)"]
fn a_late_subscriber_receives_the_remembered_window() {
    let peer: SocketAddr = env_or("POLYSIM_LINK_PEER", "127.0.0.1:9310")
        .parse()
        .expect("POLYSIM_LINK_PEER is a socket address");
    let strategy = env_or("POLYSIM_LINK_STRATEGY", "strat-micro-recorder");
    let target = LinkTarget {
        strategy: StrategyId::new(&strategy).expect("POLYSIM_LINK_STRATEGY is a valid strategy id"),
        peers: vec![peer],
        token_hash: LinkHash::of_name(&env_or("POLYSIM_LINK_TOKEN", "")),
        sender_nonce: u64::from(std::process::id()),
    };
    let mut session = LinkSession::new(target, wall_now()).expect("one peer is named");
    let socket = UdpSocket::bind("127.0.0.1:0").expect("bind a local UDP socket");
    socket
        .set_read_timeout(Some(Duration::from_millis(50)))
        .expect("set the read timeout");

    let started = Instant::now();
    let mut received = Received::default();
    let mut buffer = [0u8; LINK_MAX_DATAGRAM];
    while started.elapsed() < DEADLINE && !received.is_backfill_complete() {
        if let Some(subscribe) = session.subscribe_due(wall_now()) {
            socket.send_to(subscribe, peer).expect("send the subscribe");
        }
        let Ok((len, from)) = socket.recv_from(&mut buffer) else { continue };
        if from != peer {
            continue;
        }
        if let Some(delivery) = session.on_datagram(&buffer[..len], wall_now()) {
            received.take(delivery);
        }
    }

    assert!(
        !received.chunks.is_empty(),
        "no history arrived within {DEADLINE:?} from {peer}: an engine built with the history topic must be running there and must hold chart samples"
    );
    assert_eq!(
        received.is_catalog_held_at_first_history,
        Some(true),
        "the first history chunk arrived before the catalog"
    );
    for (instrument, chunks) in &received.chunks {
        check_instrument(*instrument, chunks, received.rotated.contains(instrument));
    }
    assert!(
        received.is_backfill_complete(),
        "the backfill did not finish within {DEADLINE:?}"
    );
}

fn check_instrument(instrument: InstrumentId, chunks: &[UiHistory], is_rotated: bool) {
    for chunk in chunks {
        for sample in chunk.samples() {
            assert!(
                sample.event_ts_us() <= chunk.cutoff_ts_us,
                "{instrument:?}: a sample is stamped after its chunk's cutoff"
            );
        }
    }
    for pair in chunks.windows(2) {
        assert_eq!(
            pair[1].first_index,
            pair[0].first_index + u64::from(pair[0].len),
            "{instrument:?}: chunks are not contiguous"
        );
    }
    if is_rotated {
        return;
    }
    let first = &chunks[0];
    let oldest_allowed = first.cutoff_ts_us - UI_CHART_WINDOW;
    let oldest = first
        .samples()
        .first()
        .expect("a chunk holds a sample")
        .event_ts_us();
    assert!(
        oldest >= oldest_allowed,
        "{instrument:?}: the backfill starts before the chart window"
    );
    assert_eq!(
        highest_stamp(chunks),
        Some(chunks.last().expect("non-empty").cutoff_ts_us),
        "{instrument:?}: the backfill stopped before the end of the memory"
    );
}
