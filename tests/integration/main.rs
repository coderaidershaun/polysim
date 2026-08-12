//! Live-network integration suite: real venue, real time, real
//! serialisation — never CI. Every test is `#[ignore]` and agent-run deliberately
//! (`cargo test --test integration -- --ignored`). Grouped as one target so the ignored set runs
//! together, mirroring the fitness `main.rs` convention.

mod binance_signed;
mod observer;
mod poly_exec;
mod poly_rotation;
mod rest_check;
mod rotations_parquet;

use std::time::{SystemTime, UNIX_EPOCH};

/// The venue's fixed 5-minute grid; window starts and rotation parity both derive from it.
pub(crate) const WINDOW_SECS: i64 = 300;

/// Wall-clock seconds since epoch — the venue's grid is UTC, so alignment and mid-window checks key
/// off real time (this is edge-side test orchestration, never hot-path state).
pub(crate) fn unix_now_s() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_secs() as i64
}
