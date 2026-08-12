//! Engine assembly and lifecycle: wired in dependency order, torn down via one drain
//! under a watchdog deadline.
//!
//! Every submodule is either a PHASE of one run — `prepare`, `run`, `drain`, in that order — or the
//! bring-up of ONE subsystem, named for it: `adapters`, `exec`, `exposure`, `link`, `persist`,
//! `metrics`, `timer`, `queues`. The rest are leaves those two lean on: `error`, `preflight`,
//! `rate_limits`, `exec_identity`.

mod adapters;
mod drain;
mod error;
mod exec;
mod exec_identity;
mod exposure;
mod link;
mod metrics;
mod persist;
mod preflight;
mod prepare;
mod queues;
mod rate_limits;
mod run;
mod sim;
mod timer;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::thread::JoinHandle;
use std::time::Duration;

use rtrb::RingBuffer;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

use crate::adapters::exec::EdgeHandle;
use crate::adapters::polymarket::rotation::WindowAssignment;
use crate::config::{Config, ExecutionMode, RunIdentity};
use crate::exposure::ExposureHandle;
use crate::hot::dispatch::{ExposureWiring, HotEngine, HotEngineSetup, LinkWiring};
use crate::hot::metrics::MetricsSnapshot;
use crate::hot::spawn::{HotThreadConfig, SimTapGate, spawn_hot_thread};
use crate::hot::strategy::Strategy;
use crate::link::{LinkActor, LinkActorSetup, LinkHandle, OutboundLink};
use crate::log::{self, LogConfig, LogHandle, LogRecord};
use crate::msg::persist::RotationRow;
use crate::msg::ui::{UiCatalog, UiLifecycle, ui_channel};
use crate::persist::PersistHandle;
use crate::shutdown::{DrainSignal, FatalSignal, RunControlGate, ShutdownRequest};
use crate::sink::{LinkSink, MetricsSink, StrategyLogSink};
use crate::time::{DurationUs, EngineClock};
use crate::{error, info};

use adapters::{AdapterWiring, SpawnedAdapter, spawn_adapters, tap_heartbeat};
use drain::{
    Trigger, install_panic_hook, join_hot, report_drain_failure, spawn_watchdog,
    stop_edge_producers, wait_for_shutdown,
};
use exec::{ExecBringUp, ExecBringUpSetup, exec_bring_up};
use exposure::exposure_bring_up;
use link::link_bring_up;
use metrics::MetricsHandle;
use persist::{PersistBringUp, PersistBringUpSetup, persist_bring_up};
use prepare::{Prepared, prepare};
use queues::{InputQueues, attach_market_taps, build_input_queues};
use timer::TimerHandle;

pub use drain::decide_exit;
pub use error::EngineError;
pub use exec_identity::ExecutionLease;
pub use preflight::{
    SymbolScales, check_poly_market, check_symbol_order_capacity, check_symbol_scale,
    classify_poly_resolve, stamp_poly_scales,
};
pub use rate_limits::{check_order_rate_limits, order_budget};
pub use run::run_trading_engine;

const METRICS_RING_CAPACITY: usize = 64;
const ROTATIONS_CHANNEL_CAPACITY: usize = 256;

/// One rotation binding every five minutes, drained on every edge select pass; the depth exists so
/// a slow bring-up cannot lose the first window.
const ASSIGNMENTS_CHANNEL_CAPACITY: usize = 8;
const STRATEGY_LOG_RING_CAPACITY: usize = 65_536;
const LINK_RING_CAPACITY: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitReport {
    pub graceful: bool,
    pub reason: Box<str>,
}

struct Engine {
    /// One trading engine per host: while this is held, no second process under the same TE
    /// identity can place. The credential names the nonce file, not the lock, so swapping keys
    /// does not buy a second engine.
    _execution_lease: Option<ExecutionLease>,
    runtime: Runtime,
    log_handle: LogHandle,
    hot: JoinHandle<()>,
    adapters: Vec<SpawnedAdapter>,
    timer: TimerHandle,
    /// `None` when the config carries no `link:` block.
    link: Option<LinkHandle>,
    /// `None` when the config carries no `persistence:` block.
    persistence: Option<PersistHandle>,
    /// Always present, since position outlives any single run.
    exposure: ExposureHandle,
    /// `None` with execution off. Stopped first, so cancels go out before anything else stops.
    exec: Option<EdgeHandle>,
    /// Simulated venues only: the taps the venue matches against, held open across its sweep.
    exec_tap_gate: Option<Arc<SimTapGate>>,
    metrics: MetricsHandle,
    fatal: FatalSignal,
    drain: DrainSignal,
    shutdown: ShutdownRequest,
    lifecycle: SyncSender<UiLifecycle>,
    drain_deadline: Duration,
}

impl Engine {
    /// Brings everything up in dependency order: logging, registry, runtime, persistence,
    /// metrics, hot thread, timer, adapters.
    ///
    /// # Errors
    /// Invalid config, unavailable hot_core_id, runtime build failure, or failed preflights.
    fn start<P>(
        identity: RunIdentity,
        config: Config<P>,
        strategy: Box<dyn Strategy>,
    ) -> Result<Engine, EngineError> {
        // The UI seam is built here; its consuming end goes to the link actor, the only
        // place that knows about it.
        let (wiring, ui_channels) = ui_channel();
        let crate::msg::ui::UiWiring {
            shutdown,
            lifecycle,
            books: ui_book_sink,
            events: ui_event_sink,
        } = wiring;

        let mode = config.execution.as_ref().map(|execution| execution.mode);
        let log_handle = log::init(&LogConfig {
            dir: config.logging.dir.clone(),
            file_stem: ExecutionMode::artifact_stem(mode, &identity).into(),
            ..LogConfig::default()
        });
        log::register_thread("main");
        info!("polysim starting — trading engine {identity}");

        let Prepared {
            registry,
            runtime,
            exposure: restored_exposure,
            execution: execution_preflight,
            binance_rest_quiet,
        } = match prepare(&config, &identity, mode) {
            Ok(prepared) => prepared,
            Err(error) => {
                error!("startup failed before any thread spawned: {error}");
                log_handle.drain();
                return Err(error);
            }
        };
        let tokio_handle = runtime.handle().clone();

        let fatal = FatalSignal::new();
        let drain = DrainSignal::new();
        install_panic_hook(fatal.clone());

        let feature_names: Vec<Box<str>> = strategy
            .features()
            .iter()
            .map(|name| (*name).into())
            .collect();

        // The socket is bound before anything spawns, so a port already taken fails the run early.
        let bound_link =
            match link_bring_up(&config, &identity, strategy.as_ref(), &registry, &runtime) {
                Ok(bound) => bound,
                Err(error) => {
                    error!("startup failed before any thread spawned: {error}");
                    log_handle.drain();
                    return Err(error);
                }
            };
        let control = RunControlGate::new();
        let on_controller_loss = config
            .link
            .as_ref()
            .map_or_else(Default::default, |link| link.on_controller_loss);

        let (rotations_tx, rotations_rx) = mpsc::channel::<RotationRow>(ROTATIONS_CHANNEL_CAPACITY);
        // The rotation binding the execution edge trades against. Sized for a handful of windows:
        // one assignment every five minutes, and the edge drains it on every select pass.
        let (assignments_tx, assignments_rx) =
            mpsc::channel::<WindowAssignment>(ASSIGNMENTS_CHANNEL_CAPACITY);
        let PersistBringUp {
            wiring: persist_wiring,
            handle: persistence,
        } = persist_bring_up(PersistBringUpSetup {
            config: &config,
            identity: &identity,
            registry: &registry,
            feature_names: &feature_names,
            rotations: rotations_rx,
        });

        // This runs before the hot thread, so the writer knows the disk state before
        // anything changes it; the sink it produces goes to the hot engine.
        let (exposure, exposure_sink) = exposure_bring_up(
            &config.exposure,
            &identity,
            &registry,
            &restored_exposure,
            mode,
        );

        let (metrics_producer, metrics_consumer) =
            RingBuffer::<MetricsSnapshot>::new(METRICS_RING_CAPACITY);
        let metrics = {
            let _guard = tokio_handle.enter();
            MetricsHandle::spawn(metrics_consumer)
        };

        // A dedicated ring isolates a telemetry flood from engine WARN/ERROR records.
        let (strategy_log_producer, strategy_log_consumer) =
            RingBuffer::<LogRecord>::new(STRATEGY_LOG_RING_CAPACITY);
        let strategy_log_sink = StrategyLogSink::new(strategy_log_producer);
        // Leaked exactly once, since the id is only known now; log::init panics on a
        // second call, so this never repeats.
        let strategy_tag: &'static str =
            Box::leak(identity.strategy_id.as_str().to_owned().into_boxed_str());
        log::register_external_ring(
            strategy_tag,
            strategy_log_consumer,
            strategy_log_sink.drops_handle(),
        );

        let queues = build_input_queues(&registry, config.queues.input_capacity, &fatal);
        let InputQueues {
            ingress,
            groups: group_producers,
            timer: timer_producer,
            link: link_producer,
            exec: exec_producer,
        } = queues;

        let (group_producers, market_taps) = match mode.is_some_and(ExecutionMode::is_simulated) {
            true => {
                let (tapped, taps) =
                    attach_market_taps(&registry, config.queues.input_capacity, group_producers);
                (tapped, Some(taps))
            }
            false => (group_producers, None),
        };

        // One clock serves every edge producer's received/queued stamps; the hot engine
        // keeps its own separate one.
        let clock = EngineClock::start();
        let drain_deadline = Duration::from_millis(config.engine.drain_deadline_ms);
        let ExecBringUp {
            wiring: exec_wiring,
            spawn: exec_spawn,
            tap_gate: exec_tap_gate,
            lease: execution_lease,
        } = match exec_bring_up(ExecBringUpSetup {
            execution: config.execution.as_ref(),
            preflight: execution_preflight,
            registry: &registry,
            identity: &identity,
            producer: exec_producer,
            taps: market_taps,
            window_assignments: Some(assignments_rx),
            clock: &clock,
            fatal: &fatal,
            desired_run_state: control.desired().clone(),
            drain_deadline,
            spin_interval: DurationUs::from_micros(config.engine.spin_interval_us as i64),
            input_capacity: config.queues.input_capacity,
            lease_dir: &config.exposure.dir,
        }) {
            Ok(brought_up) => brought_up,
            Err(error) => {
                error!("startup failed before the hot thread spawned: {error}");
                log_handle.drain();
                return Err(error);
            }
        };

        let (link_wiring, link_feed) = match &bound_link {
            None => (None, None),
            Some(_) => {
                let (producer, consumer) = RingBuffer::<OutboundLink>::new(LINK_RING_CAPACITY);
                (
                    Some(LinkWiring {
                        sink: LinkSink::new(producer),
                        acknowledged: control.acknowledged().clone(),
                    }),
                    Some(consumer),
                )
            }
        };

        // Hot engine gets typed sinks, not raw producers.
        let metrics_sink = MetricsSink::new(metrics_producer);
        let mut hot_engine = HotEngine::new(HotEngineSetup {
            exec: exec_wiring,
            instruments: registry.instruments(),
            strategy,
            persistence: persist_wiring,
            strategy_log_sink,
            metrics_sink,
            ui_book_sink,
            ui_event_sink,
            link: link_wiring,
            exposure: ExposureWiring {
                restored: restored_exposure.instruments(),
                sink: exposure_sink,
            },
            warmup: DurationUs::from_secs(config.engine.warmup_secs as i64),
        });
        let hot = spawn_hot_thread(
            HotThreadConfig {
                core_id: config.engine.hot_core_id,
                tag: "hot",
            },
            ingress,
            fatal.clone(),
            drain.clone(),
            move |pop, message| hot_engine.dispatch(pop, &message),
        );

        let interval = DurationUs::from_micros(config.engine.spin_interval_us as i64);
        let timer = TimerHandle::spawn(interval, timer_producer, &clock, &tokio_handle);
        // This runs after the hot thread, so input-queue producers start only after their
        // consumer exists.
        let exec = exec_spawn.map(|spawn| spawn(&tokio_handle));

        let adapters = spawn_adapters(AdapterWiring {
            registry: &registry,
            producers: group_producers,
            clock: &clock,
            fatal: &fatal,
            desired_run_state: control.desired(),
            rotations_tx: &rotations_tx,
            window_assignments: Some(assignments_tx),
            tap_heartbeat: tap_heartbeat(&config),
            binance_rest_quiet,
            tokio_handle: &tokio_handle,
        });
        info!(
            "polysim up — {} adapters, spin every {} us",
            adapters.len(),
            config.engine.spin_interval_us
        );

        // Hands the UI its catalog and feature names so the monitor can label columns.
        // Uses try_send only.
        let catalog = UiCatalog::from_registry(
            &identity.strategy_id,
            mode,
            config.engine.spin_interval_us,
            feature_names,
            &registry,
        );
        lifecycle.try_send(UiLifecycle::Ready(catalog)).ok();

        // The link both feeds the hot thread and drains the UI rings; it starts only once
        // the sinks it needs are live.
        let link = bound_link.map(|bound| {
            LinkActor::spawn(
                LinkActorSetup {
                    socket: bound.socket,
                    identity: bound.identity,
                    guard: bound.guard,
                    peers: bound.peers,
                    on_controller_loss,
                    inbound: link_producer.expect("a bound link always builds its input queue"),
                    outbound: link_feed.expect("a bound link always builds its outbound ring"),
                    channels: ui_channels,
                    control,
                    clock: clock.clone(),
                    spin_interval: interval,
                    topic_count: bound.topic_count,
                    feature_count: bound.feature_count,
                },
                &tokio_handle,
            )
        });

        Ok(Engine {
            _execution_lease: execution_lease,
            runtime,
            log_handle,
            hot,
            adapters,
            timer,
            link,
            persistence,
            exposure,
            exec,
            exec_tap_gate,
            metrics,
            fatal,
            drain,
            shutdown,
            lifecycle,
            drain_deadline,
        })
    }

    /// Blocks until a signal, a shutdown request, or a fatal trip, then drains under a
    /// hard deadline enforced by a watchdog.
    fn run_until_shutdown(self) -> ExitReport {
        let Engine {
            _execution_lease: execution_lease,
            runtime,
            log_handle,
            hot,
            adapters,
            timer,
            link,
            persistence,
            exposure,
            exec,
            exec_tap_gate,
            metrics,
            fatal,
            drain,
            shutdown,
            lifecycle,
            drain_deadline,
        } = self;

        let trigger = runtime.block_on(wait_for_shutdown(&fatal, &shutdown));
        let drain_reason: Box<str> = match &trigger {
            Trigger::Signal(name) => {
                info!("received {name} — draining");
                format!("received {name}").into()
            }
            Trigger::Fatal => {
                let reason = fatal
                    .reason()
                    .unwrap_or_else(|| Box::<str>::from("fatal signal tripped"));
                error!("fatal: {reason} — draining");
                reason
            }
        };
        // Uses try_send only, so a dead receiver never blocks the drain.
        lifecycle
            .try_send(UiLifecycle::Draining {
                reason: drain_reason,
            })
            .ok();

        let done = Arc::new(AtomicBool::new(false));
        spawn_watchdog(drain_deadline, Arc::clone(&done));

        let (exposure_result, persist_result) = runtime.block_on(async {
            stop_edge_producers(exec, exec_tap_gate, timer, adapters, link).await;
            drain.request();
            join_hot(hot).await;
            // Position is drained before research data: closing Parquet takes time, and
            // running out of it still leaves a recoverable position.
            let exposure = exposure.drain().await;
            let persist = match persistence {
                Some(handle) => handle.drain().await,
                None => Ok(()),
            };
            metrics.abort();
            (exposure, persist)
        });
        let exposure_failure = report_drain_failure("exposure", exposure_result);
        let persist_failure = report_drain_failure("persistence", persist_result);
        // Exposure first: position is what the next run inherits, so it names the exit.
        let drain_error = exposure_failure.or(persist_failure);

        // The fatal latch is re-read after the drain, since a hot-thread panic during it
        // must override an otherwise graceful shutdown.
        let signal_name = match &trigger {
            Trigger::Signal(name) => Some(*name),
            Trigger::Fatal => None,
        };
        let report = decide_exit(signal_name, fatal.reason(), drain_error);

        info!(
            "drain complete ({})",
            if report.graceful { "graceful" } else { "fatal" }
        );
        // The last word to the UI is the authoritative exit report, sent with try_send so
        // a gone receiver simply drops it.
        lifecycle
            .try_send(UiLifecycle::Stopped(report.clone()))
            .ok();
        log_handle.drain();
        done.store(true, Ordering::Release);
        // Released only after every order-related component has stopped, so a replacement
        // process cannot start while this one is still draining.
        drop(execution_lease);
        report
    }
}
