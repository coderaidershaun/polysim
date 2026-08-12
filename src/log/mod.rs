//! Bespoke logging: three levels, per-thread lock-free rings, drain thread. Producers never block on push; error! ships backtrace.

mod drain;
mod output;
mod producer;
mod record;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::time::TsUs;

pub use producer::register_thread;
pub use record::{Level, LogRecord};

pub(crate) use producer::{register_external_ring, tag_task};
pub(crate) use record::MSG_CAPACITY;

#[doc(hidden)]
pub use producer::{error_message, info_message, warn_message};

const DEFAULT_RING_CAPACITY: usize = 8192;
const DEFAULT_FILE_STEM: &str = "polysim";

#[derive(Debug, Clone)]
pub struct LogConfig {
    /// Log file directory (release only; debug -> stderr).
    pub dir: PathBuf,
    pub ring_capacity: usize,
    /// File stem for `{file_stem}-YYYY-MM-DD.log`; engine sets the two-part run identity.
    pub file_stem: Box<str>,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("logs"),
            ring_capacity: DEFAULT_RING_CAPACITY,
            file_stem: DEFAULT_FILE_STEM.into(),
        }
    }
}

/// Call drain() to flush + stop (drop alone is not enough).
#[derive(Debug)]
pub struct LogHandle {
    drain: Arc<AtomicBool>,
    join: JoinHandle<()>,
}

impl LogHandle {
    pub fn drain(self) {
        self.drain.store(true, Ordering::Release);
        if self.join.join().is_err() {
            eprintln!("polysim: logging drain thread panicked during shutdown");
        }
    }
}

/// Call at startup, before any thread logs.
///
/// # Panics
/// Panics if called more than once in a process.
pub fn init(config: &LogConfig) -> LogHandle {
    let channels = producer::install(config.ring_capacity);
    let output = output::Output::open(config);
    let drain = Arc::new(AtomicBool::new(false));
    let thread_drain = Arc::clone(&drain);
    let join = thread::Builder::new()
        .name("polysim-log".to_owned())
        .spawn(move || drain::run(channels, output, thread_drain))
        .expect("failed to spawn logging drain thread");
    LogHandle { drain, join }
}

fn wall_now() -> TsUs {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        // Pre-epoch -> 0 (logging must not crash).
        .map_or(0, |elapsed| elapsed.as_micros() as i64);
    TsUs::from_micros(micros)
}

/// Logs at INFO (format-arg syntax like std::format).
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        $crate::log::info_message(
            ::core::module_path!(),
            ::core::file!(),
            ::core::line!(),
            ::core::format_args!($($arg)*),
        )
    };
}

/// Logs at WARN (flushed immediately).
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        $crate::log::warn_message(
            ::core::module_path!(),
            ::core::file!(),
            ::core::line!(),
            ::core::format_args!($($arg)*),
        )
    };
}

/// Counts a repeating fault and logs at WARN on the 1st, 2nd, 4th, 8th … occurrence — a fault that
/// recurs per datagram must not become the log. A macro rather than a fn so [`warn`]'s file/line
/// stays on the caller.
#[macro_export]
macro_rules! warn_repeating {
    ($counter:expr, $($arg:tt)*) => {{
        $counter += 1;
        if $counter.is_power_of_two() {
            $crate::warn!($($arg)*);
        }
    }};
}

/// Logs at ERROR (captures backtrace, flushes immediately).
#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
        $crate::log::error_message(
            ::core::module_path!(),
            ::core::file!(),
            ::core::line!(),
            ::core::format_args!($($arg)*),
        )
    };
}
