//! Adapter actor: transport + parse + sequencers. Owns read loop (REST resync), stamps queued_ts_us.

mod depth_resync;
mod kline_backfill;
mod liveness;
mod tap;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use crate::adapters::IDLE_POLL;
use crate::adapters::backoff::BackoffCaps;
use crate::adapters::rest_quiet::SharedRestQuiet;
use crate::config::{BinanceMarket, KlineInterval};
use crate::hot::spawn::QueueProducer;
use crate::ids::{InstrumentId, StreamEpoch};
use crate::link::RunState;
use crate::msg::inbound::{InboundMessage, TappedMessage, VenueMeta};
use crate::registry::{ConnectionCategory, InstrumentRow, ProducerGroup};
use crate::shutdown::{FatalSignal, RunStateCell};
use crate::time::{EngineClock, TsUs};
use crate::{error, info, warn, warn_repeating};

use super::kline::KlineSequencer;
use super::parse::{ParseContext, ParseError, parse_agg_trade, parse_combined_frame};
use super::rest::{BinanceEnv, RestClient, RestError};
use super::ws::{self, StreamCategory};
use crate::adapters::socket::{Socket, connect};

use depth_resync::DepthState;
use liveness::LivenessMonitor;

fn rest_kline_limit(market: BinanceMarket) -> u32 {
    match market {
        BinanceMarket::Spot => 1000,
        BinanceMarket::Perpetual => 1500,
    }
}

#[derive(Clone, Copy)]
struct KlineTarget {
    instrument: InstrumentId,
    interval: KlineInterval,
    backfill_limit: u32,
}

// `serverShutdown` = planned reconnect (spot only), not malformed.
#[derive(PartialEq, Eq)]
enum HandleOutcome {
    Continue,
    Reconnect,
}

enum SessionEnd {
    Stopped,
    Parked,
    /// `has_carried_data` is the only thing that proves the endpoint healthy, so it is what decides
    /// whether the reconnect backoff starts over.
    Reconnect {
        has_carried_data: bool,
    },
}

#[derive(Clone)]
pub struct BinanceAdapterContext {
    pub env: BinanceEnv,
    pub clock: EngineClock,
    pub fatal: FatalSignal,
    // IDLE = don't connect, drop socket. QueueProducer stays alive -> no ring re-plumbing.
    pub run_state: RunStateCell,
    pub backoff: BackoffCaps,
    pub tap_heartbeat: Duration,
    /// The venue's cool-off window, shared with the signed order client against one per-IP budget.
    pub rest_quiet: SharedRestQuiet,
}

pub struct BinanceAdapterHandle {
    join: JoinHandle<()>,
}

impl BinanceAdapterHandle {
    pub async fn shutdown(self) {
        crate::shutdown::abort_and_warn(self.join, "binance adapter").await;
    }
}

pub struct BinanceAdapter;

impl BinanceAdapter {
    pub fn spawn(
        group: &ProducerGroup,
        market: BinanceMarket,
        instruments: &[InstrumentRow],
        producer: QueueProducer,
        context: BinanceAdapterContext,
        rt: &Handle,
    ) -> BinanceAdapterHandle {
        let actor = Actor::new(group, market, instruments, producer, context);
        let join = rt.spawn(crate::log::tag_task("binance", actor.run()));
        BinanceAdapterHandle { join }
    }
}

struct Actor {
    market: BinanceMarket,
    category: StreamCategory,
    clock: EngineClock,
    fatal: FatalSignal,
    run_state: RunStateCell,
    backoff: BackoffCaps,
    producer: QueueProducer,
    rest: RestClient,
    url: String,
    label: String,
    routing: HashMap<String, InstrumentId>,
    venue_symbols: HashMap<InstrumentId, Box<str>>,
    liveness: LivenessMonitor,
    depth_states: HashMap<InstrumentId, DepthState>,
    kline_states: HashMap<(InstrumentId, KlineInterval), KlineSequencer>,
    kline_targets: Vec<KlineTarget>,
    rest_quiet: SharedRestQuiet,
    malformed_frames: u64,
    unroutable_frames: u64,
    stream_epoch: StreamEpoch,
    tap_heartbeat: Duration,
}

impl Actor {
    fn new(
        group: &ProducerGroup,
        market: BinanceMarket,
        instruments: &[InstrumentRow],
        producer: QueueProducer,
        context: BinanceAdapterContext,
    ) -> Self {
        let category = stream_category(group.category);
        let members: Vec<&InstrumentRow> = instruments
            .iter()
            .filter(|row| group.instruments.contains(&row.instrument_id))
            .collect();

        let label = format!("{}/{}", market.as_str(), group.category.as_str());
        let mut routing = HashMap::new();
        let mut venue_symbols = HashMap::new();
        let mut liveness = LivenessMonitor::new();
        let mut depth_states = HashMap::new();
        let mut kline_states = HashMap::new();
        let mut kline_targets = Vec::new();

        for row in &members {
            let symbol = row.venue_symbol.to_string();
            routing.insert(symbol.clone(), row.instrument_id);
            venue_symbols.insert(row.instrument_id, row.venue_symbol.clone());
            match category {
                StreamCategory::Trades => liveness.watch_stream(ws::agg_trade_stream(&symbol)),
                StreamCategory::Depth => {
                    liveness.watch_stream(ws::depth_stream(&symbol));
                    depth_states.insert(
                        row.instrument_id,
                        DepthState::new(market, row.instrument_id),
                    );
                }
                StreamCategory::Klines => {
                    let backfill_limit = kline_backfill_limit(row, market, &label);
                    for interval in &row.kline_intervals {
                        liveness.watch_kline(ws::kline_stream(&symbol, *interval), *interval);
                        kline_states.insert(
                            (row.instrument_id, *interval),
                            KlineSequencer::new(*interval),
                        );
                        kline_targets.push(KlineTarget {
                            instrument: row.instrument_id,
                            interval: *interval,
                            backfill_limit,
                        });
                    }
                }
            }
        }

        let stream_names = liveness.stream_names();
        let url = ws::combined_stream_url(market, context.env, category, &stream_names);
        let rest = RestClient::new(market, context.env).expect("build binance rest client at init");
        Self {
            market,
            category,
            clock: context.clock,
            fatal: context.fatal,
            run_state: context.run_state,
            backoff: context.backoff,
            producer,
            rest,
            url,
            label,
            routing,
            venue_symbols,
            liveness,
            depth_states,
            kline_states,
            kline_targets,
            rest_quiet: context.rest_quiet,
            malformed_frames: 0,
            unroutable_frames: 0,
            tap_heartbeat: context.tap_heartbeat,
            stream_epoch: StreamEpoch::default(),
        }
    }

    async fn run(mut self) {
        let mut attempt: u32 = 0;
        loop {
            if self.fatal.is_tripped() {
                return;
            }
            if self.run_state.state() == RunState::Idle {
                // Parked -> don't inflate backoff before resume.
                tokio::time::sleep(IDLE_POLL).await;
                continue;
            }
            let mut socket = match connect(&self.url).await {
                Ok(socket) => {
                    let Some(epoch) = self.stream_epoch.next() else {
                        return self.on_epoch_exhausted();
                    };
                    self.stream_epoch = epoch;
                    info!("binance adapter connected {}", self.label);
                    socket
                }
                Err(error) => {
                    warn!("binance adapter connect failed {}: {error}", self.label);
                    tokio::time::sleep(self.backoff.delay(attempt)).await;
                    attempt = attempt.saturating_add(1);
                    continue;
                }
            };

            self.liveness.arm();
            if self.category == StreamCategory::Klines {
                self.backfill_klines().await;
            }

            match self.run_session(&mut socket).await {
                SessionEnd::Stopped => return,
                SessionEnd::Parked => {
                    info!("binance adapter {} parked — socket dropped", self.label);
                    attempt = 0;
                }
                SessionEnd::Reconnect { has_carried_data } => {
                    if has_carried_data {
                        attempt = 0;
                    }
                    warn!("binance adapter disconnected, reconnecting {}", self.label);
                    tokio::time::sleep(self.backoff.delay(attempt)).await;
                    attempt = attempt.saturating_add(1);
                }
            }
        }
    }

    async fn run_session(&mut self, socket: &mut Socket) -> SessionEnd {
        let mut has_carried_data = false;
        loop {
            if self.fatal.is_tripped() {
                return SessionEnd::Stopped;
            }
            if self.run_state.state() == RunState::Idle {
                return SessionEnd::Parked;
            }
            // Check liveness even with no frame.
            let poll = tokio::time::timeout(self.poll_period(), socket.next()).await;
            if matches!(poll, Ok(Some(Ok(_)))) {
                self.liveness.note_message();
            }
            match poll {
                Ok(Some(Ok(Message::Text(text)))) => {
                    has_carried_data = true;
                    let received_ts_us = self.clock.now();
                    if self.handle_text(text.as_str(), received_ts_us).await
                        == HandleOutcome::Reconnect
                    {
                        return SessionEnd::Reconnect { has_carried_data };
                    }
                }
                Ok(Some(Ok(Message::Ping(payload)))) => {
                    if ws::reply_to_ping(&mut *socket, payload).await.is_err() {
                        return SessionEnd::Reconnect { has_carried_data };
                    }
                }
                Ok(Some(Ok(Message::Close(_)))) | Ok(None) => {
                    return SessionEnd::Reconnect { has_carried_data };
                }
                Ok(Some(Ok(Message::Pong(_) | Message::Binary(_) | Message::Frame(_)))) => {}
                Ok(Some(Err(error))) => {
                    warn!("binance adapter stream {}: {error}", self.label);
                    return SessionEnd::Reconnect { has_carried_data };
                }
                // Watermarks distinguish quiet lanes from stalled ones.
                Err(_elapsed) => self.producer.push_tap_watermark(self.clock.now()),
            }
            if let Some(silent_for) = self.liveness.socket_silence() {
                warn!(
                    "binance adapter {} silent {}s — reconnecting",
                    self.label,
                    silent_for.as_secs()
                );
                return SessionEnd::Reconnect { has_carried_data };
            }
            self.liveness.warn_silent(&self.label);
        }
    }

    async fn handle_text(&mut self, raw: &str, received_ts_us: TsUs) -> HandleOutcome {
        let frame = match parse_combined_frame(raw) {
            Ok(frame) => frame,
            Err(error) => {
                if is_server_shutdown(raw) {
                    return self.on_server_shutdown();
                }
                self.on_parse_error(error);
                return HandleOutcome::Continue;
            }
        };
        if is_server_shutdown(&frame.stream) {
            return self.on_server_shutdown();
        }
        // Marking the stream live BEFORE this lookup would let a stream of frames nothing can route
        // hold the liveness guard open while delivering nothing.
        let Some(&instrument) = self.routing.get(stream_symbol(&frame.stream)) else {
            self.on_unroutable_frame(format_args!("stream {}", frame.stream));
            return HandleOutcome::Continue;
        };
        self.liveness.mark_seen(&frame.stream);
        let ctx = ParseContext {
            instrument,
            received_ts_us,
        };
        match self.category {
            StreamCategory::Trades => self.handle_trade(&frame.data, ctx),
            StreamCategory::Depth => self.handle_depth(&frame.data, ctx).await,
            StreamCategory::Klines => self.handle_kline(&frame.data, ctx).await,
        }
        HandleOutcome::Continue
    }

    fn on_server_shutdown(&self) -> HandleOutcome {
        info!(
            "binance adapter {} received serverShutdown — reconnecting",
            self.label
        );
        HandleOutcome::Reconnect
    }

    fn handle_trade(&mut self, data: &str, ctx: ParseContext) {
        let event = match parse_agg_trade(data, ctx) {
            Ok(event) => event,
            Err(error) => return self.on_parse_error(error),
        };
        self.flush_tapped(vec![TappedMessage {
            message: InboundMessage::Trade(event.trade),
            venue_meta: VenueMeta::Trade {
                aggregate_id: event.aggregate_id,
                first_trade_id: event.first_trade_id,
                last_trade_id: event.last_trade_id,
                stream_epoch: self.stream_epoch,
            },
        }]);
    }

    // Klines are not simulator inputs.
    fn flush(&mut self, out: Vec<InboundMessage>) {
        for mut message in out {
            message.set_queued_ts_us(self.clock.now());
            self.producer.push(message);
        }
    }

    // Mantissa overflow is fatal (never truncate market data).
    #[cold]
    fn on_parse_error(&mut self, error: ParseError) {
        if error.is_fatal() {
            error!("binance adapter {} fatal parse: {error}", self.label);
            self.fatal.trip(format!("binance {}: {error}", self.label));
            return;
        }
        warn_repeating!(
            self.malformed_frames,
            "binance adapter {} dropped {} malformed frames (latest: {error})",
            self.label,
            self.malformed_frames
        );
    }

    /// Counted apart from malformed frames: the venue sent something well-formed and this actor has
    /// nowhere to put it, which is a subscription or registry fault rather than a wire fault.
    #[cold]
    pub(super) fn on_unroutable_frame(&mut self, subject: std::fmt::Arguments<'_>) {
        warn_repeating!(
            self.unroutable_frames,
            "binance adapter {} has no sequencer for {subject} — {} such frames dropped",
            self.label,
            self.unroutable_frames
        );
    }

    #[cold]
    pub(super) fn on_unknown_symbol(&self, instrument: InstrumentId, operation: &str) {
        warn!(
            "binance adapter {} has no venue symbol for instrument {} — skipping {operation}",
            self.label, instrument.0
        );
    }

    pub(super) fn venue_symbol(&self, instrument: InstrumentId) -> Option<String> {
        self.venue_symbols
            .get(&instrument)
            .map(|symbol| symbol.to_string())
    }

    // A 429/418 quiets every client on this venue's IP budget, the signed order path included: the
    // venue counts both halves of the deployment against one allowance, so letting the other half
    // learn nothing would have it retry straight into a harder ban.
    fn on_rest_error(&mut self, error: &RestError, operation: &str) {
        if let RestError::RateLimited {
            retry_after_secs, ..
        } = error
        {
            let wait = self.rest_quiet.open(*retry_after_secs, Instant::now());
            warn!(
                "binance adapter {} rate limited on {} — rest quiet {}s",
                self.label,
                operation,
                wait.as_secs()
            );
        } else {
            warn!(
                "binance adapter {} rest {} failed: {error}",
                self.label, operation
            );
        }
    }
}

fn is_server_shutdown(raw: &str) -> bool {
    raw.contains("serverShutdown")
}

// Fetch keep+1 (last row usually open), capped at venue limit -> keep closed candles seed.
fn kline_backfill_limit(row: &InstrumentRow, market: BinanceMarket, label: &str) -> u32 {
    let limit = rest_kline_limit(market);
    let keep = row
        .tracker
        .candles
        .as_ref()
        .map_or(1, |candles| candles.keep);
    if keep > limit as usize {
        warn!(
            "binance adapter {} instrument {} kline backfill capped at {} (candles.keep {})",
            label, row.instrument_id.0, limit, keep
        );
    }
    (keep + 1).min(limit as usize).max(1) as u32
}

fn stream_category(category: ConnectionCategory) -> StreamCategory {
    match category {
        ConnectionCategory::Trades => StreamCategory::Trades,
        ConnectionCategory::Depth => StreamCategory::Depth,
        ConnectionCategory::Klines => StreamCategory::Klines,
        ConnectionCategory::Market => {
            unreachable!("polymarket market channel never routes to the binance adapter")
        }
    }
}

fn stream_symbol(stream: &str) -> &str {
    stream.split('@').next().unwrap_or(stream)
}
