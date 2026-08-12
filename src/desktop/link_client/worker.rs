//! The socket loop behind [`LinkClient`](super::LinkClient): subscribe, decode, gate, and push into
//! the `msg::ui` rings. One trading engine at a time, and everything about that engine — the
//! sequence gate, the catalog, the control assertion — resets when the operator picks another.

use std::io::{self, ErrorKind};
use std::net::{SocketAddr, UdpSocket};
use std::sync::mpsc::{Receiver, SyncSender};
use std::time::{Duration, Instant};

use crate::config::StrategyId;
use crate::link::{
    Envelope, FrameGuard, LINK_MAX_DATAGRAM, Lifecycle, LinkBody, LinkDatagram, LinkDecodeError,
    LinkHash, LinkIdentity, RunState, SequenceGate, Subscribe, TopicId, TopicSet,
};
use crate::msg::ui::{UiLifecycle, UiWiring};
use crate::shutdown::ShutdownRequest;
use crate::time::{EngineClock, TsUs, boot_stamp_us};
use crate::{info, warn, warn_repeating};

use super::super::link_model::{
    CatalogAssembly, ConnectionState, Controller, LinkStatus, PEER_SILENCE_LIMIT,
};
use super::{LinkClientConfig, LinkCommand, bind_for};

/// Comfortably inside the engine's subscription TTL, so two consecutive lost refreshes do not drop
/// this workstation off its subscriber table.
const SUBSCRIBE_INTERVAL: Duration = Duration::from_secs(2);

/// How often the session tally reaches the log while the window is up. Matches the engine link
/// actor's own rollup cadence, so the two sides of one link report at the same rhythm.
const REPORT_INTERVAL: Duration = Duration::from_secs(60);

/// Enumerated rather than [`TopicSet::ALL`]: "all" means every topic the sender offers, which would
/// have an engine unicast strategy payload frames at a process that declares no link fields and can
/// only count them as rejects.
const WANTED_TOPICS: [TopicId; 5] = [
    TopicId::BOOKS,
    TopicId::EVENTS,
    TopicId::CATALOG_INSTRUMENTS,
    TopicId::CATALOG_FEATURES,
    TopicId::LIFECYCLE,
];

pub(super) struct WorkerSetup {
    pub config: LinkClientConfig,
    pub socket: UdpSocket,
    pub wiring: UiWiring,
    pub commands: Receiver<LinkCommand>,
    pub status: SyncSender<LinkStatus>,
    pub stop: ShutdownRequest,
}

#[derive(Default)]
struct RejectCounts {
    decode: u64,
    gated: u64,
    foreign: u64,
    send_failed: u64,
    receive_failed: u64,
    /// Status frames the UI's own channel had no room for. Counted apart from the feed because a
    /// full status channel says the painter is behind, not that the engine is.
    status_dropped: u64,
}

#[derive(Default)]
struct DeliveredCounts {
    books: u64,
    events: u64,
    catalog: u64,
    lifecycle: u64,
}

/// One configured socket serves one trading engine run. The envelope names that run on EVERY
/// topic, so it is the earliest authoritative reset signal — waiting for the first catalog frame
/// would let a new run's events fold into the previous run's working-order model in the meantime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RemoteRun {
    sender_te_hash: LinkHash,
    boot_ts_us: TsUs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunObservation {
    First,
    Same,
    Restarted,
    Stale,
}

#[derive(Debug, Default)]
struct RemoteRunGate {
    current: Option<RemoteRun>,
}

impl RemoteRunGate {
    fn observe(&mut self, envelope: &Envelope) -> RunObservation {
        let incoming = RemoteRun {
            sender_te_hash: envelope.sender_te_hash,
            boot_ts_us: envelope.boot_ts_us,
        };
        let Some(current) = self.current else {
            self.current = Some(incoming);
            return RunObservation::First;
        };
        if incoming.boot_ts_us < current.boot_ts_us {
            return RunObservation::Stale;
        }
        if incoming == current {
            return RunObservation::Same;
        }
        self.current = Some(incoming);
        RunObservation::Restarted
    }

    fn clear(&mut self) {
        self.current = None;
    }
}

pub(super) struct Worker {
    socket: UdpSocket,
    peers: Vec<SocketAddr>,
    active: usize,
    session: u64,
    strategy_id: StrategyId,
    identity: LinkIdentity,
    guard: FrameGuard,
    topics: TopicSet,
    gate: SequenceGate,
    run_gate: RemoteRunGate,
    clock: EngineClock,
    catalog: CatalogAssembly,
    published_catalog_ts_us: Option<TsUs>,
    controller: Controller,
    reported: Option<Lifecycle>,
    last_seen: Option<Instant>,
    last_subscribe: Option<Instant>,
    subscribe_seq: u64,
    wiring: UiWiring,
    commands: Receiver<LinkCommand>,
    status: SyncSender<LinkStatus>,
    last_status: Option<LinkStatus>,
    stop: ShutdownRequest,
    outbox: [u8; LINK_MAX_DATAGRAM],
    last_report: Instant,
    is_engine_silent: bool,
    rejected: RejectCounts,
    delivered: DeliveredCounts,
}

impl Worker {
    pub(super) fn new(setup: WorkerSetup) -> Self {
        let token = setup.config.token.as_deref().unwrap_or_default();
        let boot_ts_us = boot_stamp_us();
        let strategy_hash = LinkHash::of_name(setup.config.strategy_id.as_str());
        Self {
            socket: setup.socket,
            peers: setup.config.peers,
            active: 0,
            session: 0,
            strategy_id: setup.config.strategy_id,
            identity: LinkIdentity {
                token_hash: LinkHash::of_name(token),
                strategy_hash,
                // A workstation is not a trading engine, so it has no te-id; the pid keeps two
                // windows on one box from presenting to the engine as one restarting sender.
                sender_te_hash: LinkHash::of_name(&format!("polysim-ui-{}", std::process::id())),
                boot_ts_us,
            },
            guard: FrameGuard {
                token_hash: LinkHash::of_name(token),
                strategy_hash,
                // The workstation declares no link fields, so a strategy payload frame reaching it
                // is another engine's traffic and this rejects it.
                schema_hash: LinkHash::of_fields(&[]),
            },
            topics: TopicSet::new(&WANTED_TOPICS).expect("the wanted topic list is a code literal"),
            gate: SequenceGate::new(),
            run_gate: RemoteRunGate::default(),
            clock: EngineClock::start(),
            catalog: CatalogAssembly::new(),
            published_catalog_ts_us: None,
            controller: Controller::new(boot_ts_us),
            reported: None,
            last_seen: None,
            last_subscribe: None,
            subscribe_seq: 0,
            wiring: setup.wiring,
            commands: setup.commands,
            status: setup.status,
            last_status: None,
            stop: setup.stop,
            outbox: [0; LINK_MAX_DATAGRAM],
            last_report: Instant::now(),
            is_engine_silent: false,
            rejected: RejectCounts::default(),
            delivered: DeliveredCounts::default(),
        }
    }

    pub(super) fn run(mut self) {
        crate::log::register_thread("link");
        info!("workstation attaching to trading engine {}", self.peer());
        let mut inbox = [0u8; LINK_MAX_DATAGRAM];
        while !self.stop.is_requested() {
            self.apply_commands();
            self.refresh_subscription();
            match self.socket.recv_from(&mut inbox) {
                Ok((len, from)) => self.on_datagram(&inbox[..len], from),
                Err(error) => self.on_receive_error(&error),
            }
            self.check_engine_silence();
            self.publish_status();
            if self.last_report.elapsed() >= REPORT_INTERVAL {
                self.last_report = Instant::now();
                self.report_session();
            }
        }
        self.report_session();
    }

    #[inline]
    fn peer(&self) -> SocketAddr {
        self.peers[self.active]
    }

    fn apply_commands(&mut self) {
        while let Ok(command) = self.commands.try_recv() {
            match command {
                LinkCommand::Assert(state) => self.assert_run_state(state),
                LinkCommand::NextPeer => self.attach_to_next_peer(),
            }
        }
    }

    fn assert_run_state(&mut self, state: RunState) {
        self.controller.assert(state);
        let assertion = self.controller.assertion();
        info!(
            "asserting {:?} on {} at epoch {}",
            assertion.state,
            self.peer(),
            assertion.epoch
        );
        self.last_subscribe = None;
    }

    fn attach_to_next_peer(&mut self) {
        if self.peers.len() < 2 {
            return;
        }
        let next = (self.active + 1) % self.peers.len();
        let peer = self.peers[next];
        let socket = match bind_for(peer) {
            Ok(socket) => socket,
            Err(error) => return self.on_rebind_error(peer, &error),
        };
        self.socket = socket;
        self.active = next;
        self.session += 1;
        self.gate = SequenceGate::new();
        self.run_gate.clear();
        self.catalog = CatalogAssembly::new();
        self.published_catalog_ts_us = None;
        self.controller.release();
        self.reported = None;
        self.last_seen = None;
        self.is_engine_silent = false;
        self.last_subscribe = None;
        self.subscribe_seq = 0;
        info!("workstation attaching to trading engine {peer}");
    }

    fn refresh_subscription(&mut self) {
        let is_due = self
            .last_subscribe
            .is_none_or(|at| at.elapsed() >= SUBSCRIBE_INTERVAL);
        if !is_due {
            return;
        }
        self.last_subscribe = Some(Instant::now());
        let assertion = self.controller.assertion();
        self.subscribe_seq += 1;
        let datagram = LinkDatagram {
            envelope: Envelope::new(self.identity, TopicId::SUBSCRIBE, self.subscribe_seq),
            body: LinkBody::Subscribe(Subscribe {
                topics: self.topics,
                desired_state: assertion.state,
                desired_epoch: assertion.epoch,
            }),
        };
        let len = datagram.encode(&mut self.outbox);
        let peer = self.peers[self.active];
        if self.socket.send_to(&self.outbox[..len], peer).is_err() {
            self.rejected.send_failed += 1;
        }
    }

    fn on_datagram(&mut self, bytes: &[u8], from: SocketAddr) {
        if from != self.peer() {
            self.rejected.foreign += 1;
            return;
        }
        let datagram = match LinkDatagram::decode(bytes, &self.guard) {
            Ok(datagram) => datagram,
            Err(error) => return self.on_decode_error(&error),
        };
        if !self
            .gate
            .admit(&datagram.envelope, self.clock.now())
            .is_accepted()
        {
            self.rejected.gated += 1;
            return;
        }
        match self.run_gate.observe(&datagram.envelope) {
            RunObservation::First | RunObservation::Same => {}
            RunObservation::Stale => {
                self.rejected.gated += 1;
                return;
            }
            RunObservation::Restarted => self.begin_engine_run(),
        }
        self.note_engine_heard();
        self.publish_status();
        match datagram.body {
            LinkBody::Book(snapshot) => {
                self.delivered.books += 1;
                self.wiring.books.push(snapshot);
            }
            LinkBody::Event(event) => {
                self.delivered.events += 1;
                self.wiring.events.push(event);
            }
            LinkBody::CatalogInstrument(frame) => {
                self.delivered.catalog += 1;
                let held = self.catalog.catalog_ts_us();
                self.catalog.accept_instrument(frame);
                self.on_catalog_frame(held);
            }
            LinkBody::CatalogFeature(frame) => {
                self.delivered.catalog += 1;
                let held = self.catalog.catalog_ts_us();
                self.catalog.accept_feature(frame);
                self.on_catalog_frame(held);
            }
            LinkBody::Lifecycle(lifecycle) => {
                self.delivered.lifecycle += 1;
                self.reported = Some(lifecycle);
                self.publish_catalog();
            }
            LinkBody::Subscribe(_) | LinkBody::Payload(_) => self.rejected.foreign += 1,
        }
    }

    fn begin_engine_run(&mut self) {
        self.session += 1;
        self.catalog = CatalogAssembly::new();
        self.published_catalog_ts_us = None;
        self.reported = None;
    }

    fn on_catalog_frame(&mut self, previous_catalog_ts_us: Option<TsUs>) {
        if previous_catalog_ts_us.is_some()
            && previous_catalog_ts_us != self.catalog.catalog_ts_us()
        {
            self.session += 1;
        }
        self.publish_catalog();
    }

    fn publish_catalog(&mut self) {
        let Some(catalog_ts_us) = self.catalog.catalog_ts_us() else {
            return;
        };
        if self.published_catalog_ts_us == Some(catalog_ts_us) {
            return;
        }
        let Some(reported) = self.reported else {
            return;
        };
        let Some(catalog) = self
            .catalog
            .build(self.strategy_id.as_str(), self.peer(), reported)
        else {
            return;
        };
        self.publish_status();
        if self
            .wiring
            .lifecycle
            .try_send(UiLifecycle::Ready(catalog))
            .is_ok()
        {
            self.published_catalog_ts_us = Some(catalog_ts_us);
            info!("catalog complete from {}", self.peer());
        }
    }

    /// Offer the shell the newest link status. Its own drop is counted here and nowhere else: an
    /// unsent status must never decide whether a book snapshot reaches the model.
    fn publish_status(&mut self) {
        let status = LinkStatus {
            peer: self.peer(),
            peer_index: self.active,
            peer_count: self.peers.len(),
            session: self.session,
            connection: ConnectionState::from_silence(self.last_seen.map(|at| at.elapsed())),
            phase: self.reported.map(|reported| reported.phase),
            reported_state: self.reported.map(|reported| reported.run_state),
            asserted_state: self.controller.asserted(),
            control: self.controller.verdict(self.reported),
        };
        if self.last_status == Some(status) {
            return;
        }
        if self.status.try_send(status).is_err() {
            self.rejected.status_dropped += 1;
            return;
        }
        self.last_status = Some(status);
    }

    fn note_engine_heard(&mut self) {
        self.last_seen = Some(Instant::now());
        if self.is_engine_silent {
            self.is_engine_silent = false;
            info!("trading engine {} is answering again", self.peer());
        }
    }

    fn check_engine_silence(&mut self) {
        let is_silent = self
            .last_seen
            .is_some_and(|at| at.elapsed() >= PEER_SILENCE_LIMIT);
        if !is_silent || self.is_engine_silent {
            return;
        }
        self.is_engine_silent = true;
        warn!(
            "trading engine {} silent for {}s - the feed on screen is stale, not merely quiet",
            self.peer(),
            PEER_SILENCE_LIMIT.as_secs()
        );
    }

    fn report_session(&self) {
        let rejected = &self.rejected;
        let delivered = &self.delivered;
        info!(
            "link session {} delivered[books={} events={} catalog={} lifecycle={}] rejected[decode={} gated={} foreign={} send_failed={} receive_failed={} status_dropped={}] gate{:?}",
            self.peer(),
            delivered.books,
            delivered.events,
            delivered.catalog,
            delivered.lifecycle,
            rejected.decode,
            rejected.gated,
            rejected.foreign,
            rejected.send_failed,
            rejected.receive_failed,
            rejected.status_dropped,
            self.gate.counts()
        );
    }

    #[cold]
    fn on_decode_error(&mut self, error: &LinkDecodeError) {
        warn_repeating!(
            self.rejected.decode,
            "workstation rejected {} datagrams from {} (latest: {error})",
            self.rejected.decode,
            self.peer()
        );
    }

    fn on_receive_error(&mut self, error: &io::Error) {
        if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) {
            return;
        }
        self.on_socket_error(error);
    }

    #[cold]
    fn on_socket_error(&mut self, error: &io::Error) {
        warn_repeating!(
            self.rejected.receive_failed,
            "workstation link socket failed {} reads (latest: {error})",
            self.rejected.receive_failed
        );
    }

    #[cold]
    fn on_rebind_error(&mut self, peer: SocketAddr, error: &io::Error) {
        warn!(
            "cannot open a socket for {peer} ({error}) - staying attached to {}",
            self.peer()
        );
    }
}
