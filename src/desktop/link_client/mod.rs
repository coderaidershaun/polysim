//! The workstation's half of the link, and the seam the rest of it sees: what to attach to, how to
//! work the controls, how to stop. [`worker`] is the socket loop behind it. No parallel wire structs
//! — the engine's own `UiBookSnapshot`/`UiEvent` layouts cross the socket and land in the existing
//! `msg::ui` rings, so nothing below here can tell a datagram from an in-process ring.
//!
//! A blocking socket on a dedicated thread rather than a tokio actor: no runtime and no hot path to
//! protect here, and the only thing ever sent is a 2 s heartbeat, so the engine-side argument for
//! async (an outbound payload must not wait out a read timeout) does not transfer.

mod worker;

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::config::StrategyId;
use crate::link::RunState;
use crate::msg::ui::UiWiring;
use crate::shutdown::ShutdownRequest;
use crate::warn;

use super::link_model::LinkStatus;
use worker::{Worker, WorkerSetup};

const RECEIVE_TIMEOUT: Duration = Duration::from_millis(50);

const COMMAND_CAPACITY: usize = 16;

const STATUS_CAPACITY: usize = 8;

#[derive(Debug, Clone)]
pub struct LinkClientConfig {
    pub peers: Vec<SocketAddr>,
    pub strategy_id: StrategyId,
    pub token: Option<Box<str>>,
}

#[derive(thiserror::Error, Debug)]
pub enum LinkClientError {
    #[error(
        "no trading engine address given - the workstation attaches to a running engine's link port"
    )]
    NoPeers,
    #[error("failed to bind a local udp socket to reach {peer}")]
    Bind {
        peer: SocketAddr,
        #[source]
        source: io::Error,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkCommand {
    Assert(RunState),
    NextPeer,
}

pub(crate) struct LinkFeed {
    status: Receiver<LinkStatus>,
    commands: SyncSender<LinkCommand>,
}

impl LinkFeed {
    /// The most recent status, discarding any that piled up behind it — an older one has nothing
    /// left to say.
    pub(crate) fn poll(&self) -> Option<LinkStatus> {
        let mut latest = None;
        while let Ok(status) = self.status.try_recv() {
            latest = Some(status);
        }
        latest
    }

    pub(crate) fn send(&self, command: LinkCommand) {
        if self.commands.try_send(command).is_err() {
            warn!("link command {command:?} dropped - the client thread is not keeping up");
        }
    }
}

pub(crate) struct LinkClient {
    stop: ShutdownRequest,
    join: JoinHandle<()>,
}

impl LinkClient {
    /// Bind a local socket for the first peer and start serving `wiring` from it.
    ///
    /// # Errors
    /// [`LinkClientError::NoPeers`] with an empty peer list, and [`LinkClientError::Bind`] when no
    /// local socket can be opened for the first peer's address family.
    pub(crate) fn start(
        config: LinkClientConfig,
        wiring: UiWiring,
    ) -> Result<(Self, LinkFeed), LinkClientError> {
        let peer = *config.peers.first().ok_or(LinkClientError::NoPeers)?;
        let socket = bind_for(peer).map_err(|source| LinkClientError::Bind { peer, source })?;
        let (command_tx, command_rx) = sync_channel(COMMAND_CAPACITY);
        let (status_tx, status_rx) = sync_channel(STATUS_CAPACITY);
        let stop = ShutdownRequest::new();
        let worker = Worker::new(WorkerSetup {
            config,
            socket,
            wiring,
            commands: command_rx,
            status: status_tx,
            stop: stop.clone(),
        });
        let join = thread::Builder::new()
            .name("polysim-ui-link".to_owned())
            .spawn(move || worker.run())
            .expect("failed to spawn the link client thread");
        Ok((
            Self { stop, join },
            LinkFeed {
                status: status_rx,
                commands: command_tx,
            },
        ))
    }

    /// Bounded by [`RECEIVE_TIMEOUT`]: the thread is either in a receive that times out or between
    /// two of them.
    pub(crate) fn shutdown(self) {
        self.stop.request();
        if self.join.join().is_err() {
            warn!("link client thread panicked during shutdown - cause already logged");
        }
    }
}

/// A local socket of the peer's own address family, so an IPv6 engine is reachable without asking
/// the operator which family to bind.
fn bind_for(peer: SocketAddr) -> io::Result<UdpSocket> {
    let local: SocketAddr = match peer {
        SocketAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        SocketAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let socket = UdpSocket::bind(local)?;
    socket.set_read_timeout(Some(RECEIVE_TIMEOUT))?;
    Ok(socket)
}
