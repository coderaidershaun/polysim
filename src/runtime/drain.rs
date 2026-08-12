//! Shutdown and drain: waits for a signal, a fatal trip, or a shutdown request, tears
//! everything down under a watchdog, catches panics, and decides the process exit.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use tokio::signal::unix::{SignalKind, signal};

use crate::adapters::exec::EdgeHandle;
use crate::hot::spawn::SimTapGate;
use crate::link::LinkHandle;
use crate::shutdown::{FatalSignal, ShutdownRequest};
use crate::{error, warn};

use super::ExitReport;
use super::adapters::SpawnedAdapter;
use super::timer::TimerHandle;

/// The poll frequency for latches that cannot be awaited directly, such as a fatal trip
/// or a shutdown request.
const LATCH_POLL: Duration = Duration::from_millis(20);

pub(super) enum Trigger {
    Signal(&'static str),
    Fatal,
}

pub(super) fn install_panic_hook(fatal: FatalSignal) {
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|loc| format!("{}:{}", loc.file(), loc.line()))
            .unwrap_or_else(|| "unknown location".to_owned());
        let message = panic_message(info.payload());
        error!("thread panicked at {location}: {message}");
        fatal.trip(format!("panic at {location}: {message}"));
    }));
}

pub(super) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

pub(super) async fn wait_for_shutdown(fatal: &FatalSignal, shutdown: &ShutdownRequest) -> Trigger {
    tokio::select! {
        () = wait_sigint() => Trigger::Signal("SIGINT"),
        () = wait_sigterm() => Trigger::Signal("SIGTERM"),
        () = poll_fatal(fatal) => Trigger::Fatal,
        () = poll_shutdown_request(shutdown) => Trigger::Signal("shutdown request"),
    }
}

async fn wait_sigint() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        error!("could not install SIGINT handler: {error} — SIGTERM and fatal still observed");
        never().await;
    }
}

async fn wait_sigterm() {
    let mut sigterm = match signal(SignalKind::terminate()) {
        Ok(sigterm) => sigterm,
        Err(error) => {
            error!("could not install SIGTERM handler: {error} — SIGINT and fatal still observed");
            return never().await;
        }
    };
    if sigterm.recv().await.is_none() {
        error!("SIGTERM stream closed — SIGINT and fatal still observed");
        never().await;
    }
}

async fn never() {
    std::future::pending::<()>().await;
}

async fn poll_fatal(fatal: &FatalSignal) {
    while !fatal.is_tripped() {
        tokio::time::sleep(LATCH_POLL).await;
    }
}

async fn poll_shutdown_request(shutdown: &ShutdownRequest) {
    while !shutdown.is_requested() {
        tokio::time::sleep(LATCH_POLL).await;
    }
}

/// Stops producers in dependency order.
pub(super) async fn stop_edge_producers(
    exec: Option<EdgeHandle>,
    exec_tap_gate: Option<Arc<SimTapGate>>,
    timer: TimerHandle,
    adapters: Vec<SpawnedAdapter>,
    link: Option<LinkHandle>,
) {
    timer.shutdown().await;
    if let Some(exec) = exec {
        stop_execution(exec, exec_tap_gate).await;
    }
    if let Some(link) = link {
        link.shutdown().await;
    }
    for adapter in adapters {
        adapter.shutdown().await;
    }
}

/// A gate is present only under a simulated venue. The taps are that venue's own market feed, so
/// they stay open across its forced sweep and close only once nothing is left to drain them.
async fn stop_execution(exec: EdgeHandle, tap_gate: Option<Arc<SimTapGate>>) {
    let Some(gate) = tap_gate else {
        exec.shutdown().await;
        return;
    };
    gate.begin_sweep();
    exec.shutdown().await;
    gate.disable();
}

pub(super) async fn join_hot(hot: JoinHandle<()>) {
    let joined = tokio::task::spawn_blocking(move || {
        if hot.join().is_err() {
            warn!("hot thread ended by panic — cause already logged");
        }
    })
    .await;
    // A JoinError here can only mean the shim itself panicked, which in practice is unreachable.
    if let Err(join_error) = joined
        && join_error.is_panic()
    {
        warn!("hot-thread join shim panicked — cause already logged");
    }
}

pub(super) fn spawn_watchdog(deadline: Duration, done: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("polysim-drain-watchdog".to_owned())
        .spawn(move || {
            std::thread::sleep(deadline);
            if !done.load(Ordering::Acquire) {
                // This is a forced exit, so the log drain never gets to flush; the cause
                // goes to both stderr and the log.
                eprintln!(
                    "polysim: drain deadline {} ms exceeded — forcing exit",
                    deadline.as_millis()
                );
                error!(
                    "drain deadline {} ms exceeded — forcing exit",
                    deadline.as_millis()
                );
                std::process::exit(1);
            }
        })
        .expect("failed to spawn drain watchdog thread");
}

pub(super) fn report_drain_failure<E: core::fmt::Display>(
    what: &str,
    result: Result<(), E>,
) -> Option<Box<str>> {
    let Err(error) = result else { return None };
    error!("{what} drain failed: {error}");
    Some(format!("{what} drain failed: {error}").into())
}

pub fn decide_exit(
    signal_name: Option<&str>,
    fatal_reason: Option<Box<str>>,
    drain_error: Option<Box<str>>,
) -> ExitReport {
    if let Some(reason) = fatal_reason {
        return ExitReport {
            graceful: false,
            reason,
        };
    }
    if let Some(reason) = drain_error {
        return ExitReport {
            graceful: false,
            reason,
        };
    }
    match signal_name {
        Some(name) => ExitReport {
            graceful: true,
            reason: format!("received {name}").into(),
        },
        None => ExitReport {
            graceful: false,
            reason: "fatal signal tripped".into(),
        },
    }
}
