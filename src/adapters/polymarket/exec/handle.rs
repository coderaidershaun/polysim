//! Runtime seam: actor setup and the gate that must pass before it. Preflight lives in preflight.rs
//! and is re-exported here so runtime doesn't need to know multiple modules.

use std::time::Duration;

use tokio::runtime::Handle;
use tokio::sync::mpsc;

use crate::adapters::backoff::BackoffCaps;
use crate::adapters::exec::{EdgeHandle, ExecStop};
use crate::adapters::polymarket::rotation::WindowAssignment;
use crate::hot::spawn::QueueProducer;
use crate::msg::exec::ExecLaneItem;
use crate::registry::InstrumentRow;
use crate::shutdown::{FatalSignal, RunStateCell};
use crate::time::{DurationUs, EngineClock};

use super::actor::PolymarketExecActor;
use super::sign::address::Address;
use super::sign::key::SigningKey;
use super::sign::l2::ApiCredentials;
use super::sign::order::SignatureType;

pub use super::preflight::{PolymarketPreflight, PolymarketPreflightError, preflight_polymarket};

// The maker is who the venue credits; the signer is whose key authenticates. They are the
// same address for an EOA wallet, but separate for a smart wallet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WalletIdentity {
    pub maker: Address,
    pub signer: Address,
    pub signature_type: SignatureType,
}

pub struct PolymarketExecAdapterContext {
    pub clock: EngineClock,
    pub fatal: FatalSignal,
    // When run_state goes IDLE, quotes are withdrawn and the socket drops — resting orders
    // can't simply be parked.
    pub run_state: RunStateCell,
    pub backoff: BackoffCaps,
    pub venue_clock_offset: DurationUs,
    pub max_orders_per_side: usize,
    // The shutdown sweep deadline, enforced by the engine's drain deadline.
    pub sweep_deadline: Duration,
    // How long to stay disconnected before pulling orders over HTTP; the venue offers no
    // SLA here, so this is an operator's choice.
    pub disconnect_sweep_after: DurationUs,
}

pub struct PolymarketExecAdapterSetup {
    pub instruments: Vec<InstrumentRow>,
    // L2 API credentials, kept separate from the key that signs orders.
    pub credentials: ApiCredentials,
    pub key: SigningKey,
    pub wallet: WalletIdentity,
    pub commands: rtrb::Consumer<ExecLaneItem>,
    pub producer: QueueProducer,
    /// Rotation bindings from the market-data actor, which is the single authority on how a
    /// token maps to a leg.
    pub assignments: mpsc::Receiver<WindowAssignment>,
    pub context: PolymarketExecAdapterContext,
}

pub struct PolymarketExecAdapter;

impl PolymarketExecAdapter {
    /// A pre-spawn check, run before a lease is acquired. A refusal here consumes no run nonce.
    ///
    /// # Errors
    /// [`PolymarketExecError::UnprovenWalletType`] for unproven signing paths.
    pub fn check_available(wallet: &WalletIdentity) -> Result<(), PolymarketExecError> {
        match wallet.signature_type {
            SignatureType::Eoa | SignatureType::GnosisSafe => Ok(()),
            unproven => Err(PolymarketExecError::UnprovenWalletType {
                code: unproven.code(),
            }),
        }
    }

    pub(crate) fn spawn(setup: PolymarketExecAdapterSetup, runtime: &Handle) -> EdgeHandle {
        let stop = ExecStop::new();
        let sweep_deadline = setup.context.sweep_deadline;
        EdgeHandle {
            join: PolymarketExecActor::spawn(setup, stop.clone(), runtime),
            stop,
            sweep_deadline,
            venue: "polymarket execution adapter",
            missed_sweep_cost: "orders may still rest on the venue",
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum PolymarketExecError {
    #[error(
        "this wallet signs orders as signatureType {code}, which needs the erc-7739 wrap — the wrap is implemented and pinned against a vector, but no live order has ever been signed with it, and an unproven signing path fails at the venue as a generic refusal"
    )]
    UnprovenWalletType { code: u8 },
}
