//! Errors raised by the simulated execution edge.

use super::core::latency::LatencyBudgetError;
use super::core::wallet::WalletError;
use super::lanes::SimLane;

#[derive(thiserror::Error, Debug, Clone, Copy, PartialEq, Eq)]
#[error("{lane} lane buffer reached capacity {capacity}")]
pub struct LaneBufferFull {
    pub lane: SimLane,
    pub capacity: usize,
}

/// Why a run cannot stand a simulated venue up at all.
#[derive(thiserror::Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimVenueError {
    #[error("the simulated venue's opening account is not spendable")]
    Wallet(#[from] WalletError),
    #[error(
        "the simulated venue's latency budget cannot answer a command in time — config checks the same sum in whole milliseconds, so a sub-millisecond spin interval is binding here and nowhere else"
    )]
    Latency(#[from] LatencyBudgetError),
}
