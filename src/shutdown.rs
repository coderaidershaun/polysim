//! The shutdown latches: `FatalSignal` stops NOW, `DrainSignal` finishes the input then stops, and
//! `ShutdownRequest` is the external graceful trigger, alongside `abort_and_warn`. `RunStateCell`
//! is a TWO-WAY toggle for trading on and off rather than an ending; SIGINT remains the only real
//! exit.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::link::RunState;
use crate::warn;

/// A one-way latch: the engine drains and exits immediately. Cheap to clone and to check, and the
/// first tripper's reason is the one kept.
#[derive(Clone)]
pub struct FatalSignal {
    tripped: Arc<AtomicBool>,
    reason: Arc<Mutex<Option<Box<str>>>>,
}

impl FatalSignal {
    pub fn new() -> Self {
        Self {
            tripped: Arc::new(AtomicBool::new(false)),
            reason: Arc::new(Mutex::new(None)),
        }
    }

    /// Records `reason` (only the first is kept) and latches the signal.
    #[cold]
    pub fn trip(&self, reason: impl Into<Box<str>>) {
        let mut slot = self.reason.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_none() {
            *slot = Some(reason.into());
        }
        drop(slot);
        self.tripped.store(true, Ordering::Release);
    }

    #[inline]
    pub fn is_tripped(&self) -> bool {
        self.tripped.load(Ordering::Acquire)
    }

    pub fn reason(&self) -> Option<Box<str>> {
        self.reason
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Default for FatalSignal {
    fn default() -> Self {
        Self::new()
    }
}

/// A one-way latch for a graceful drain: the hot loop processes until the queues are empty, then
/// exits.
#[derive(Clone, Default)]
pub struct DrainSignal {
    requested: Arc<AtomicBool>,
}

impl DrainSignal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn request(&self) {
        self.requested.store(true, Ordering::Release);
    }

    #[inline]
    pub fn is_requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }
}

/// An in-process graceful shutdown request that arrives without a signal. It is observed the same
/// way as SIGINT or SIGTERM, so it takes the same drain and flush path. Distinct from `DrainSignal`,
/// which is internal and post-trigger, and from `FatalSignal`, which means corrupt state and a
/// non-zero exit.
#[derive(Clone, Default)]
pub struct ShutdownRequest {
    requested: Arc<AtomicBool>,
}

impl ShutdownRequest {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn request(&self) {
        self.requested.store(true, Ordering::Release);
    }

    #[inline]
    pub fn is_requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }
}

/// The run state paired with a controller epoch. The highest epoch wins, so a stale or duplicate
/// controller cannot make the state oscillate. Epoch 0 means no controller yet; controller epochs
/// start at 1 and rise on each restart, stamped from the boot time in microseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RunAssertion {
    pub state: RunState,
    pub epoch: u64,
}

impl RunAssertion {
    pub const INITIAL: RunAssertion = RunAssertion {
        state: RunState::Running,
        epoch: 0,
    };
}

/// The state and epoch live in ONE atomic word, so a reader never sees a mismatched pair. There
/// are two per run: DESIRED, which the link actor writes and the adapters read, and ACKNOWLEDGED,
/// which the hot thread writes and the link reads to learn whether its marker landed.
#[derive(Clone)]
pub struct RunStateCell {
    packed: Arc<AtomicU64>,
}

/// The epoch occupies the low 63 bits and the state the sign bit, which keeps the whole assertion
/// in one word while still letting the epoch reach 2^63.
const IDLE_BIT: u64 = 1 << 63;

impl RunStateCell {
    pub fn new() -> Self {
        Self {
            packed: Arc::new(AtomicU64::new(pack(RunAssertion::INITIAL))),
        }
    }

    /// An unconditional store for the acknowledged cell, which has a single writer: the hot thread
    /// reporting the state it actually applied.
    #[inline]
    pub fn store(&self, assertion: RunAssertion) {
        self.packed.store(pack(assertion), Ordering::Release);
    }

    /// A highest-epoch-wins store for the desired cell. `false` means this controller lost the race
    /// and is not in charge.
    pub fn accept_if_newer(&self, assertion: RunAssertion) -> bool {
        if assertion.epoch <= self.load().epoch {
            return false;
        }
        self.store(assertion);
        true
    }

    #[inline]
    pub fn load(&self) -> RunAssertion {
        unpack(self.packed.load(Ordering::Acquire))
    }

    /// The state alone, for edge code that does not need the epoch.
    #[inline]
    pub fn state(&self) -> RunState {
        self.load().state
    }
}

impl Default for RunStateCell {
    fn default() -> Self {
        Self::new()
    }
}

const fn pack(assertion: RunAssertion) -> u64 {
    match assertion.state {
        RunState::Running => assertion.epoch,
        RunState::Idle => assertion.epoch | IDLE_BIT,
    }
}

const fn unpack(word: u64) -> RunAssertion {
    RunAssertion {
        state: if word & IDLE_BIT == 0 { RunState::Running } else { RunState::Idle },
        epoch: word & !IDLE_BIT,
    }
}

/// The desired and acknowledged pair is what makes control converge. `pending()` asks which marker
/// still needs pushing, which makes the control LEVEL-triggered. The marker rides a ring that drops
/// and counts when it is full, and an edge-triggered design would leave the adapters parked after
/// such a drop; asking again on the next tick repairs it instead.
#[derive(Clone, Default)]
pub struct RunControlGate {
    desired: RunStateCell,
    acknowledged: RunStateCell,
}

impl RunControlGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Written by the link actor, and read by the adapters.
    pub fn desired(&self) -> &RunStateCell {
        &self.desired
    }

    /// Written by dispatch as it applies a marker, and reports the hot thread's actual state.
    pub fn acknowledged(&self) -> &RunStateCell {
        &self.acknowledged
    }

    /// The marker to push, or `None` once the hot thread has caught up.
    pub fn pending(&self) -> Option<RunAssertion> {
        let desired = self.desired.load();
        (desired != self.acknowledged.load()).then_some(desired)
    }
}

/// Aborts a spawned task and notes whether it panicked. The panic hook has already logged the
/// cause, so this records only the fact and never re-raises.
pub(crate) async fn abort_and_warn(join: tokio::task::JoinHandle<()>, task_name: &str) {
    join.abort();
    if let Err(join_error) = join.await
        && join_error.is_panic()
    {
        warn!("{task_name} task panicked during shutdown — cause already logged");
    }
}
