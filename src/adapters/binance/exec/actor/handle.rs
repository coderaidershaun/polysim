//! The runtime's seam onto this actor: what it needs to start, and the handle it hands back.

use std::time::Duration;

use tokio::runtime::Handle;

use crate::adapters::backoff::BackoffCaps;
use crate::adapters::binance::rest::{BinanceEnv, SignedRestClient};
use crate::adapters::edge::run_edge;
use crate::adapters::exec::{EdgeHandle, EngineIdentity, ExecStop};
use crate::hot::spawn::QueueProducer;
use crate::msg::exec::ExecLaneItem;
use crate::registry::{AssetDictionary, InstrumentRow};
use crate::secrets::Credentials;
use crate::shutdown::{FatalSignal, RunStateCell};
use crate::time::{DurationUs, EngineClock};

use super::super::RecvWindow;
use super::Actor;

pub struct BinanceExecAdapterContext {
    pub env: BinanceEnv,
    pub clock: EngineClock,
    pub fatal: FatalSignal,
    // When run_state goes IDLE, quotes are pulled and the socket drops — resting orders
    // can't simply be parked.
    pub run_state: RunStateCell,
    pub backoff: BackoffCaps,
    pub identity: EngineIdentity,
    pub recv_window: RecvWindow,
    // Must stay inside the engine's drain deadline, since the watchdog force-exits once
    // that deadline is reached.
    pub sweep_deadline: Duration,
    // How long to stay blind before pulling orders; the venue gives no guidance here, so
    // this is operator judgment.
    pub disconnect_sweep_after: DurationUs,
    // Clock skew beyond recvWindow makes the venue refuse every signed request.
    pub loud_clock_skew: DurationUs,
    pub max_orders_per_side: usize,
}

pub struct BinanceExecAdapterSetup {
    pub instruments: Vec<InstrumentRow>,
    pub assets: AssetDictionary,
    // The WS API signs api_key as a parameter, so it cannot borrow the REST client's copy.
    pub credentials: Credentials,
    // Shares one weight budget and clock offset with the REST client, since the venue
    // charges all calls against the same IP.
    pub rest: SignedRestClient,
    pub commands: rtrb::Consumer<ExecLaneItem>,
    pub producer: QueueProducer,
    pub context: BinanceExecAdapterContext,
}

pub struct BinanceExecAdapter;

impl BinanceExecAdapter {
    pub(crate) fn spawn(setup: BinanceExecAdapterSetup, rt: &Handle) -> EdgeHandle {
        let stop = ExecStop::new();
        let sweep_deadline = setup.context.sweep_deadline;
        let actor = Actor::new(setup, stop.clone(), rt);
        EdgeHandle {
            join: rt.spawn(crate::log::tag_task("binance-exec", run_edge(actor))),
            stop,
            sweep_deadline,
            venue: "binance execution adapter",
            missed_sweep_cost: "orders may still rest on the venue",
        }
    }
}
