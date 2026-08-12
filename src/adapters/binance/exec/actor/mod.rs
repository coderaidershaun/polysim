//! WebSocket and REST together: effects become bytes, and bytes become messages. Correlation runs
//! on two paths, the request id and the stream event's CLIENT ORDER ID.

mod apply;
mod frame;
pub(super) mod handle;
mod inbound;
mod poll;
mod reconcile;
mod rest;
mod resync;

use std::time::Duration;

use futures_util::StreamExt;
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::adapters::binance::rest::BinanceEnv;
use crate::adapters::binance::ws;
use crate::adapters::edge::{EdgeControl, EdgeDriver, EventFunnel, OfflineStep, Reader, Writer};
use crate::adapters::exec::{
    EngineIdentity, ExecCore, ExecStop, InFlightTable, Outgoing, REQUEST_TIMEOUT, ResyncPass,
    SessionOutcome, stream_reset,
};
use crate::ids::{ClientOrderId, InstrumentId, TradeId, VenueOrderId};
use crate::msg::exec::{CancelReason, ExecLaneItem};
use crate::registry::AssetDictionary;
use crate::time::DurationUs;
use crate::{info, warn};

use super::{RecvWindow, RequestSigner, SymbolTable};

use handle::BinanceExecAdapterSetup;
use rest::{RestChannels, RestJob, RestOutcome};

// Reconnects before the venue's 24-hour connection limit is reached.
const CONNECTION_LIFETIME: Duration = Duration::from_secs(23 * 60 * 60);
// 1ms trades a little latency against waking on every rtrb push.
const COMMAND_POLL: Duration = Duration::from_millis(1);
// The cadence at which the drain deadline is checked.
const HOUSEKEEPING: Duration = Duration::from_millis(20);
// Five seconds is enough to catch recent fills without tripping the rate limits.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(5);
// Thirty minutes, expressed as a count of reconcile ticks so the resync check rides the
// existing tick stream rather than reading a clock of its own.
const CLOCK_RESYNC_TICKS: u32 = 360;
const RECENT_ORDERS: usize = 128;
// Sized to hold the fill history for the five-second reconcile window.
const RECENT_TRADES: usize = 256;

#[derive(Default)]
struct DriverCounts {
    dropped_frames: u64,
    unroutable_frames: u64,
    unmatched_responses: u64,
    ignored_events: u64,
    untracked_events: u64,
    missed_fills: u64,
    unrecovered_fills: u64,
    cancels_skipped: u64,
}

/// The reconciler's route from a fill back to the order that placed it: `myTrades` names orders by
/// the venue's id and never carries a client id.
struct RecentOrder {
    venue_id: VenueOrderId,
    client_id: ClientOrderId,
}

struct TradeCursor {
    instrument: InstrumentId,
    from_id: Option<i64>,
    // The first answer adopts the cursor, so a page of history does not trigger a burst of
    // reconciliations.
    is_primed: bool,
}

struct Actor {
    core: ExecCore,
    symbols: SymbolTable,
    assets: AssetDictionary,
    instruments: Vec<InstrumentId>,
    identity: EngineIdentity,
    env: BinanceEnv,
    control: EdgeControl,
    recv_window: RecvWindow,
    // Clock skew beyond the receive window makes the venue refuse every signed request.
    loud_clock_skew: DurationUs,
    events: EventFunnel,
    commands: rtrb::Consumer<ExecLaneItem>,
    signer: RequestSigner,
    // Travels as a WebSocket parameter, so it must never be logged or included in debug output.
    api_key: String,
    clock_offset: super::ClockOffset,
    inflight: InFlightTable,
    rest_jobs: mpsc::Sender<RestJob>,
    rest_outcomes: mpsc::Receiver<RestOutcome>,
    rest_join: JoinHandle<()>,
    cursors: Vec<TradeCursor>,
    recent_orders: Vec<RecentOrder>,
    recent_trades: Vec<TradeId>,
    // Tracks resync progress; while it has not completed, every order is refused.
    resync: ResyncPass,
    subscribe_failures: u32,
    is_rest_gone: bool,
    is_balance_snapshot_due: bool,
    reconcile_ticks: u32,
    counts: DriverCounts,
    // True once resync has landed and any inherited orders may still be awaiting cancel.
    is_readiness_pending: bool,
    // Before first opening, all venue-held orders are inherited.
    has_opened_quoting: bool,
}

impl Actor {
    fn new(setup: BinanceExecAdapterSetup, stop: ExecStop, rt: &Handle) -> Self {
        let BinanceExecAdapterSetup {
            instruments,
            assets,
            credentials,
            rest,
            commands,
            producer,
            context,
        } = setup;
        let symbols = SymbolTable::new(
            instruments
                .iter()
                .map(|row| (row.instrument_id, row.venue_symbol.clone())),
        );
        let signer = RequestSigner::new(credentials.api_secret());
        let api_key = frame::api_key_text(credentials.api_key())
            .expect("the binance api key must be text — it rides the ws api as a json param");
        let clock_offset = rest.clock_offset();
        let channels = rest::spawn_rest_worker(rest, symbols.clone(), context.identity, rt);
        let RestChannels {
            jobs: rest_jobs,
            outcomes: rest_outcomes,
            join: rest_join,
        } = channels;
        // Sums each symbol's order limit; falls back to zero if any instrument lacks a
        // stamped limit.
        let mirror_capacity = instruments
            .iter()
            .try_fold(0usize, |total, row| {
                let limit = usize::try_from(row.max_num_orders?).ok()?;
                total.checked_add(limit)
            })
            .unwrap_or(0);
        Self {
            core: ExecCore::with_limits(context.max_orders_per_side, mirror_capacity),
            symbols,
            assets,
            instruments: instruments.iter().map(|row| row.instrument_id).collect(),
            identity: context.identity,
            env: context.env,
            control: EdgeControl {
                clock: context.clock.clone(),
                fatal: context.fatal,
                run_state: context.run_state,
                stop,
                backoff: context.backoff,
                sweep_deadline: context.sweep_deadline,
                disconnect_sweep_after: context.disconnect_sweep_after,
                exit: None,
                is_swept: false,
            },
            recv_window: context.recv_window,
            loud_clock_skew: context.loud_clock_skew,
            events: EventFunnel::new(producer, context.clock),
            commands,
            signer,
            api_key,
            clock_offset,
            inflight: InFlightTable::new(REQUEST_TIMEOUT),
            rest_jobs,
            rest_outcomes,
            rest_join,
            cursors: instruments
                .iter()
                .map(|row| TradeCursor {
                    instrument: row.instrument_id,
                    from_id: None,
                    is_primed: false,
                })
                .collect(),
            recent_orders: Vec::with_capacity(RECENT_ORDERS),
            recent_trades: Vec::with_capacity(RECENT_TRADES),
            resync: ResyncPass::default(),
            subscribe_failures: 0,
            is_rest_gone: false,
            is_balance_snapshot_due: false,
            reconcile_ticks: 0,
            counts: DriverCounts::default(),
            is_readiness_pending: false,
            has_opened_quoting: false,
        }
    }

    fn on_connection_expiry(&mut self) -> Option<SessionOutcome> {
        info!(
            "binance execution reconnecting at {}h — the venue drops this connection at 24",
            CONNECTION_LIFETIME.as_secs() / 3600
        );
        Some(SessionOutcome::Reconnect)
    }

    fn on_disconnected(&mut self) {
        let outstanding = self.inflight.take_all();
        if !outstanding.is_empty() {
            warn!(
                "binance execution lost the connection with {} requests unanswered — their orders are unresolved until the resync",
                outstanding.len()
            );
        }
        for entry in outstanding {
            self.mark_transport_ambiguous(entry.request);
        }
        self.core.on_disconnected();
        let now = self.control.clock.now();
        self.events.send_exec(stream_reset(now));
    }

    #[must_use]
    fn submit(&self, job: RestJob) -> bool {
        rest::submit(&self.rest_jobs, job)
    }
}

impl EdgeDriver for Actor {
    fn venue(&self) -> &'static str {
        "binance"
    }

    fn stream_url(&self) -> &str {
        ws::ws_api_url(self.env)
    }

    fn control(&mut self) -> &mut EdgeControl {
        &mut self.control
    }

    // The clock, the balances and the open-order reads are what seed the mirror.
    async fn start(&mut self) {
        let mut expected = self.pass_reads();
        if self.submit(RestJob::SyncClock) {
            expected += 1;
        }
        self.start_resync();
        let deadline = tokio::time::Instant::now() + self.control.sweep_deadline;
        while expected > 0 {
            match tokio::time::timeout_at(deadline, self.rest_outcomes.recv()).await {
                Ok(Some(outcome)) => {
                    self.on_rest_outcome(outcome, None).await;
                    expected -= 1;
                }
                Ok(None) => break,
                Err(_elapsed) => {
                    warn!(
                        "binance execution bootstrap incomplete after {}ms — starting with {expected} answers outstanding",
                        self.control.sweep_deadline.as_millis()
                    );
                    break;
                }
            }
        }
    }

    async fn serve(&mut self, mut writer: Writer, mut reader: Reader) -> SessionOutcome {
        let mut command_ticker = tokio::time::interval(COMMAND_POLL);
        let mut housekeeping = tokio::time::interval(HOUSEKEEPING);
        let mut reconcile = tokio::time::interval(RECONCILE_INTERVAL);
        // The deadline is absolute so it cannot drift.
        let expiry = tokio::time::Instant::now() + CONNECTION_LIFETIME;

        let mut outgoing = Vec::new();
        self.core.on_connected(&mut |effect| {
            outgoing.push(Outgoing {
                effect,
                recon_seq: 0,
            });
        });
        if let Some(session) = self.dispatch(outgoing, Some(&mut writer)).await {
            self.on_disconnected();
            return session;
        }

        loop {
            let session = tokio::select! {
                frame = reader.next() => self.on_frame(frame, &mut writer).await,
                _ = command_ticker.tick() => self.on_command_tick(&mut writer).await,
                _ = housekeeping.tick() => self.on_housekeeping(&mut writer).await,
                _ = reconcile.tick() => self.on_reconcile_tick(),
                outcome = self.rest_outcomes.recv(), if !self.is_rest_gone => {
                    self.on_rest_answer(outcome, &mut writer).await
                }
                _ = tokio::time::sleep_until(expiry) => self.on_connection_expiry(),
            };
            if let Some(session) = session {
                self.on_disconnected();
                return session;
            }
        }
    }

    // While offline, REST answers are still drained as they arrive rather than just sleeping
    // until the deadline.
    async fn while_offline(&mut self, deadline: tokio::time::Instant) -> OfflineStep {
        match tokio::time::timeout_at(deadline, self.rest_outcomes.recv()).await {
            // If the answer is a refused cancel, the retry rides on a later answer rather
            // than firing on its own timer.
            Ok(Some(outcome)) => {
                self.on_rest_outcome(outcome, None).await;
                OfflineStep::Serviced
            }
            Ok(None) => {
                self.is_rest_gone = true;
                OfflineStep::Ended
            }
            Err(_elapsed) => OfflineStep::Ended,
        }
    }

    async fn sweep_step(&mut self) {
        self.retry_sweep(None).await;
    }

    async fn begin_exit(&mut self, reason: CancelReason) {
        self.plan_exit(reason, None).await;
    }

    fn report_stop(&self) {
        if let Some(exit) = &self.control.exit {
            info!(
                "binance execution stopping after a {:?} sweep — {} orders still mirrored, {} requests timed out",
                exit.reason,
                self.core.mirror().len(),
                self.inflight.timed_out()
            );
        }
        // Every count the driver keeps is reported here. A drop policy nobody can read the outcome
        // of is a variable, not a policy.
        let counts = &self.counts;
        info!(
            "binance execution counts: frames {} undecodable / {} unroutable, {} unmatched answers, {} events ignored ({} untracked), fills {} recovered / {} lost, {} cancels skipped",
            counts.dropped_frames,
            counts.unroutable_frames,
            counts.unmatched_responses,
            counts.ignored_events,
            counts.untracked_events,
            counts.missed_fills,
            counts.unrecovered_fills,
            counts.cancels_skipped
        );
    }

    fn abort_workers(&self) {
        self.rest_join.abort();
    }
}
