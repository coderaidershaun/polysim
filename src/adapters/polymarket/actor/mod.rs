//! Driver actor: socket/tick/ping/REST shell. Single-owner loop → monotone input. 5m rotation: T-60s subscribe, 404 probe teardown.

mod core;
mod effects;
mod leg;

pub use core::{DriverEffect, PolyDriverCore, SlotLegs};

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{Error as ProtocolError, Message};

use crate::adapters::IDLE_POLL;
use crate::adapters::backoff::BackoffCaps;
use crate::adapters::rest_quiet::RestQuiet;
use crate::config::PolySeries;
use crate::hot::spawn::QueueProducer;
use crate::link::RunState;
use crate::msg::persist::RotationRow;
use crate::registry::{InstrumentRow, ProducerGroup};
use crate::shutdown::{FatalSignal, RunStateCell};
use crate::time::{EngineClock, TsUs};
use crate::{error, info, warn};

use super::discovery::{PolySchedule, Slot};
use super::parse::{is_frameless, parse_market_frame};
use super::rest::PolyRest;
use super::rotation::{WindowAssignment, WindowTokens};
use super::ws;
use crate::adapters::socket::{Socket, connect};

use effects::CoreEvent;

const TICK_INTERVAL: Duration = Duration::from_secs(1);
const RESULTS_CAPACITY: usize = 32;

/// The longest venue-supplied `Retry-After` this adapter will honour. A rotation is five minutes, so
/// parking REST past one window resolves nothing that the next window would not have to resolve
/// again — and an hour-long value from the venue would silently stop the rotation for an hour.
const MAX_REST_QUIET_SECS: u64 = 60;

type Writer = SplitSink<Socket, Message>;

#[derive(Clone)]
pub struct PolymarketAdapterContext {
    pub clock: EngineClock,
    pub fatal: FatalSignal,
    pub run_state: RunStateCell,
    pub backoff: BackoffCaps,
    pub rotations_tx: mpsc::Sender<RotationRow>,
    /// Where the execution edge learns which token each leg is trading this window. `None` when no
    /// edge is armed.
    pub window_assignments: Option<mpsc::Sender<WindowAssignment>>,
}

pub struct PolymarketAdapterHandle {
    join: JoinHandle<()>,
}

impl PolymarketAdapterHandle {
    pub async fn shutdown(self) {
        crate::shutdown::abort_and_warn(self.join, "polymarket adapter").await;
    }
}

pub struct PolymarketAdapter;

impl PolymarketAdapter {
    pub fn spawn(
        group: &ProducerGroup,
        series: PolySeries,
        instruments: &[InstrumentRow],
        producer: QueueProducer,
        context: PolymarketAdapterContext,
        rt: &Handle,
    ) -> PolymarketAdapterHandle {
        let (actor, results_rx) = Actor::new(group, series, instruments, producer, context);
        let join = rt.spawn(crate::log::tag_task("polymarket", actor.run(results_rx)));
        PolymarketAdapterHandle { join }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Continue,
    Reconnect,
}

struct Actor {
    core: PolyDriverCore,
    schedule: PolySchedule,
    clock: EngineClock,
    fatal: FatalSignal,
    run_state: RunStateCell,
    backoff: BackoffCaps,
    producer: QueueProducer,
    rest: Arc<PolyRest>,
    rest_quiet: RestQuiet,
    results_tx: mpsc::Sender<CoreEvent>,
    rotations_tx: mpsc::Sender<RotationRow>,
    window_assignments: Option<mpsc::Sender<WindowAssignment>>,
    dropped_rotations: u64,
    dropped_bindings: u64,
    inflight: Vec<TsUs>,
    probing: Vec<WindowTokens>,
    dropped_frames: u64,
    suppressed_rest: u64,
    divergences: u64,
    label: String,
}

impl Actor {
    fn new(
        group: &ProducerGroup,
        series: PolySeries,
        instruments: &[InstrumentRow],
        producer: QueueProducer,
        context: PolymarketAdapterContext,
    ) -> (Self, mpsc::Receiver<CoreEvent>) {
        let members: Vec<&InstrumentRow> = instruments
            .iter()
            .filter(|row| group.instruments.contains(&row.instrument_id))
            .collect();
        let slot_legs = slot_legs_from_rows(series, &members);
        let schedule = PolySchedule::for_series(series);
        let (results_tx, results_rx) = mpsc::channel(RESULTS_CAPACITY);
        let rest = PolyRest::new(series).expect("build polymarket rest client at init");
        let actor = Self {
            core: PolyDriverCore::new(slot_legs, schedule),
            schedule,
            clock: context.clock,
            fatal: context.fatal,
            run_state: context.run_state,
            backoff: context.backoff,
            producer,
            rest: Arc::new(rest),
            rest_quiet: RestQuiet::new(),
            results_tx,
            rotations_tx: context.rotations_tx,
            window_assignments: context.window_assignments,
            dropped_rotations: 0,
            dropped_bindings: 0,
            inflight: Vec::new(),
            probing: Vec::new(),
            dropped_frames: 0,
            suppressed_rest: 0,
            divergences: 0,
            label: series.as_str().to_owned(),
        };
        (actor, results_rx)
    }

    async fn run(mut self, mut results_rx: mpsc::Receiver<CoreEvent>) {
        let mut ticks = tokio::time::interval(TICK_INTERVAL);
        let mut ping = tokio::time::interval(ws::PING_INTERVAL);
        self.bootstrap().await;

        let mut attempt: u32 = 0;
        loop {
            if self.fatal.is_tripped() {
                return;
            }
            if self.run_state.state() == RunState::Idle {
                tokio::time::sleep(IDLE_POLL).await;
                continue;
            }
            let socket = match connect(ws::MARKET_URL).await {
                Ok(socket) => {
                    info!("polymarket adapter {} connected", self.label);
                    socket
                }
                Err(error) => {
                    warn!("polymarket adapter {} connect failed: {error}", self.label);
                    tokio::time::sleep(self.backoff.delay(attempt)).await;
                    attempt = attempt.saturating_add(1);
                    continue;
                }
            };
            let (mut writer, mut reader) = socket.split();
            if !self.prime_connection(&mut writer).await {
                tokio::time::sleep(self.backoff.delay(attempt)).await;
                attempt = attempt.saturating_add(1);
                continue;
            }
            attempt = 0;

            let mut is_parked = false;
            loop {
                if self.fatal.is_tripped() {
                    return;
                }
                if self.run_state.state() == RunState::Idle {
                    is_parked = true;
                    break;
                }
                let flow = tokio::select! {
                    frame = reader.next() => self.on_socket_message(frame, &mut writer).await,
                    _ = ticks.tick() => {
                        let now = self.clock.now();
                        self.drive_tick(now, &mut writer).await
                    }
                    _ = ping.tick() => self.send_ping(&mut writer).await,
                    event = results_rx.recv() => self.on_core_event(event, &mut writer).await,
                };
                if flow == Flow::Reconnect {
                    break;
                }
            }

            if is_parked {
                info!("polymarket adapter {} parked — socket dropped", self.label);
                attempt = 0;
                continue;
            }
            warn!(
                "polymarket adapter {} disconnected — reconnecting",
                self.label
            );
            tokio::time::sleep(self.backoff.delay(attempt)).await;
            attempt = attempt.saturating_add(1);
        }
    }

    /// Resolve current window for initial subscribe set. WS ops deferred to connect; Rotation/BookReset land now.
    async fn bootstrap(&mut self) {
        let now = self.clock.now();
        let current = self.schedule.current_window(now);
        match self.rest.resolve_slug(current.window_start_ts_us).await {
            Ok(market) => {
                let assignment = self
                    .core
                    .assignment_from_market(current.window_start_ts_us, &market);
                let effects =
                    collect_effects(|emit| self.core.on_window_resolved(now, assignment, emit));
                self.execute_offline(effects);
            }
            Err(error) => warn!(
                "polymarket adapter {} startup resolve of window {} failed: {error} — scheduler retries",
                self.label,
                current.window_start_ts_us.micros()
            ),
        }
        let now = self.clock.now();
        let effects = collect_effects(|emit| self.core.on_tick(now, emit));
        self.execute_offline(effects);
    }

    /// Re-baseline live legs, resend plain subscribe. False on socket write failure → backoff+reconnect.
    async fn prime_connection(&mut self, writer: &mut Writer) -> bool {
        let now = self.clock.now();
        let effects = collect_effects(|emit| self.core.on_reconnect(now, emit));
        if self.execute(effects, writer).await == Flow::Reconnect {
            return false;
        }
        let tokens = self.core.live_tokens();
        if tokens.is_empty() {
            return true;
        }
        let text = ws::subscribe_message(&tokens);
        if writer.send(Message::Text(text.into())).await.is_err() {
            warn!(
                "polymarket adapter {} initial subscribe failed — reconnecting",
                self.label
            );
            return false;
        }
        true
    }

    async fn on_socket_message(
        &mut self,
        frame: Option<Result<Message, ProtocolError>>,
        writer: &mut Writer,
    ) -> Flow {
        match frame {
            Some(Ok(Message::Text(text))) => {
                let now = self.clock.now();
                self.on_text(text.as_str(), now, writer).await
            }
            Some(Ok(Message::Close(_))) | None => Flow::Reconnect,
            Some(Ok(_)) => Flow::Continue,
            Some(Err(error)) => {
                warn!("polymarket adapter {} stream: {error}", self.label);
                Flow::Reconnect
            }
        }
    }

    async fn on_text(&mut self, text: &str, now: TsUs, writer: &mut Writer) -> Flow {
        if is_frameless(text) {
            return Flow::Continue;
        }
        match parse_market_frame(text, now) {
            Ok(frame) => {
                let effects = collect_effects(|emit| self.core.on_frame(now, &frame, emit));
                self.execute(effects, writer).await
            }
            Err(error) if error.is_fatal() => {
                error!("polymarket adapter {} fatal parse: {error}", self.label);
                self.fatal
                    .trip(format!("polymarket {}: {error}", self.label));
                Flow::Reconnect
            }
            Err(error) => {
                self.on_parse_error(error);
                Flow::Continue
            }
        }
    }

    async fn drive_tick(&mut self, now: TsUs, writer: &mut Writer) -> Flow {
        let effects = collect_effects(|emit| self.core.on_tick(now, emit));
        self.execute(effects, writer).await
    }

    async fn send_ping(&mut self, writer: &mut Writer) -> Flow {
        if writer.send(Message::Text(ws::PING.into())).await.is_err() {
            Flow::Reconnect
        } else {
            Flow::Continue
        }
    }

    async fn on_core_event(&mut self, event: Option<CoreEvent>, writer: &mut Writer) -> Flow {
        let Some(event) = event else {
            return Flow::Continue;
        };
        match event {
            CoreEvent::ResolveOk { start, market } => {
                self.clear_inflight(start);
                let now = self.clock.now();
                let assignment = self.core.assignment_from_market(start, &market);
                let effects =
                    collect_effects(|emit| self.core.on_window_resolved(now, assignment, emit));
                self.execute(effects, writer).await
            }
            CoreEvent::ResolveErr { start } => {
                self.clear_inflight(start);
                warn!(
                    "polymarket adapter {} resolve of window {} failed — scheduler retries",
                    self.label,
                    start.micros()
                );
                Flow::Continue
            }
            CoreEvent::ResolveRateLimited {
                start,
                retry_after_secs,
            } => {
                self.clear_inflight(start);
                let wait = self.open_rest_quiet(retry_after_secs);
                warn!(
                    "polymarket adapter {} gamma rate limited — rest quiet {}s",
                    self.label,
                    wait.as_secs()
                );
                Flow::Continue
            }
            CoreEvent::Probed { tokens, outcome } => {
                self.clear_probing(&tokens);
                let Some(outcome) = outcome else {
                    return Flow::Continue;
                };
                let now = self.clock.now();
                let effects =
                    collect_effects(|emit| self.core.on_probe_result(now, tokens, outcome, emit));
                self.execute(effects, writer).await
            }
            CoreEvent::ProbeRateLimited {
                tokens,
                retry_after_secs,
            } => {
                self.clear_probing(&tokens);
                let wait = self.open_rest_quiet(retry_after_secs);
                warn!(
                    "polymarket adapter {} clob rate limited — rest quiet {}s",
                    self.label,
                    wait.as_secs()
                );
                Flow::Continue
            }
        }
    }

    fn open_rest_quiet(&mut self, retry_after_secs: Option<u64>) -> Duration {
        let honoured = retry_after_secs.map(|seconds| seconds.min(MAX_REST_QUIET_SECS));
        self.rest_quiet.open(honoured, Instant::now())
    }
}

fn collect_effects(drive: impl FnOnce(&mut dyn FnMut(DriverEffect))) -> Vec<DriverEffect> {
    let mut effects = Vec::new();
    drive(&mut |effect| effects.push(effect));
    effects
}

/// Built through [`Slot::as_usize`], because that is how the core indexes the array it gets back: a
/// pair placed at the other position publishes each window's prices under the other slot's
/// instrument ids, and nothing downstream can tell.
fn slot_legs_from_rows(series: PolySeries, members: &[&InstrumentRow]) -> [SlotLegs; 2] {
    let leg = |symbol: &str| {
        members
            .iter()
            .find(|row| &*row.venue_symbol == symbol)
            .map(|row| row.instrument_id)
            .unwrap_or_else(|| panic!("polymarket group missing slot row {symbol}"))
    };
    let symbols = series.slot_leg_symbols();
    Slot::ALL.map(|slot| {
        let [up, down] = &symbols[slot.as_usize()];
        SlotLegs {
            up: leg(up),
            down: leg(down),
        }
    })
}
