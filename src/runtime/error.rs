//! Every way a run can refuse to start, after the config itself parsed: an unavailable core, an
//! unreachable venue, a scale or limit the engine cannot represent, a lease another process holds.

use crate::adapters::binance::exec::{ProbeError, SignError};
use crate::adapters::binance::rest::RestError;
use crate::adapters::exchange_sim::SimVenueError;
use crate::adapters::polymarket::exec::handle::{PolymarketExecError, PolymarketPreflightError};
use crate::adapters::polymarket::rest::GammaError;
use crate::config::ConfigError;
use crate::exposure::ExposureError;
use crate::ids::{DecimalError, Price, Qty};
use std::path::PathBuf;

#[derive(thiserror::Error, Debug)]
pub enum EngineError {
    #[error("config invalid")]
    Config(#[from] ConfigError),
    #[error("exposure could not be restored")]
    Exposure(#[from] ExposureError),
    /// One variant per venue: their probes fail for different reasons and `#[from]` serves only one.
    #[error(
        "the binance execution startup gate failed — refusing to trade without proof the key can"
    )]
    BinanceExecutionPreflight(#[source] ProbeError),
    #[error(
        "the polymarket execution startup gate failed — refusing to trade without proof the wallet can"
    )]
    PolymarketExecutionPreflight(#[source] PolymarketPreflightError),
    #[error("the polymarket execution edge cannot be driven")]
    PolymarketExecutionUnavailable(#[source] PolymarketExecError),
    #[error(
        "execution.mode is {mode} but {detail} — refusing to start, because a run that reports itself armed and places nothing is worse than one that does not start"
    )]
    ExecutionNotWired {
        mode: &'static str,
        detail: &'static str,
    },
    #[error(
        "{found} instruments exceed the {max}-instrument order table — an engine that cannot track its own orders must not place any"
    )]
    ExecutionTooManyInstruments { found: usize, max: usize },
    #[error(
        "the venue declares {found} ORDERS rate-limit buckets and the engine paces against {max} — pacing against only some of them would spend a budget the venue never granted"
    )]
    ExecutionTooManyOrderWindows { found: usize, max: usize },
    #[error(
        "execution.{field} is {value}, but {venue_fact} — the setting reaches nothing on this venue, and a config that appears to tune one is worse than one that leaves it out"
    )]
    ExecutionFieldInert {
        field: &'static str,
        value: Box<str>,
        venue_fact: &'static str,
    },
    #[error(
        "execution.min_base_balance is {value}, but on this venue the base asset IS the position — a floor under it comes off every exit, so an offer sized to the whole position reads underfunded and a flatten rounds below the venue's minimum and strands what it was meant to close"
    )]
    ExecutionBaseFloorOnPosition { value: Box<str> },
    #[error(
        "execution.recv_window_ms is outside what the venue signs inside — every request would be rejected, which reads on the wire like a dead key rather than a bad number"
    )]
    BinanceRecvWindow(#[source] SignError),
    #[error(
        "the simulated venue holds one instrument and this run configures {found} — a synthesised venue matches one price queue against one market, and splitting it would give each instrument its own clock and readiness"
    )]
    SimulatedVenueOneInstrument { found: usize },
    #[error(transparent)]
    SimulatedVenue(#[from] SimVenueError),
    #[error(
        "instrument {instrument} ({symbol}) configures {configured_per_side} orders per side ({required_total} total), exceeding the venue MAX_NUM_ORDERS limit {venue_max}"
    )]
    ExecutionOrderCapacity {
        instrument: u16,
        symbol: Box<str>,
        configured_per_side: u32,
        required_total: u32,
        venue_max: u32,
    },
    #[error(
        "execution identity is already held at {path} — refusing a second process that could exceed the per-side order cap"
    )]
    ExecutionIdentityInUse { path: PathBuf },
    #[error("execution identity state at {path} could not be read or written")]
    ExecutionIdentityIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "execution identity state at {path} contains {value:?}, expected an unsigned run nonce"
    )]
    ExecutionIdentityState { path: PathBuf, value: Box<str> },
    #[error("execution identity at {path} exhausted its 32-bit run nonce")]
    ExecutionIdentityExhausted { path: PathBuf },
    #[error("tokio runtime build failed")]
    Runtime(#[source] std::io::Error),
    #[error("hot_core_id {core_id} is not an available core (found {available})")]
    CoreId { core_id: usize, available: usize },
    #[error(
        "instrument {instrument} ({symbol}) {field} {value} is not representable as an i64 1e-8 mantissa"
    )]
    ScaleOutOfRange {
        instrument: u16,
        symbol: Box<str>,
        field: &'static str,
        value: Box<str>,
        #[source]
        source: DecimalError,
    },
    #[error("instrument {instrument} ({symbol}) is not listed in {market} exchangeInfo")]
    ScaleSymbolUnknown {
        instrument: u16,
        symbol: Box<str>,
        market: &'static str,
    },
    #[error(
        "instrument {instrument} ({symbol}) exchangeInfo is missing {field} — cannot validate scale"
    )]
    ScaleFieldMissing {
        instrument: u16,
        symbol: Box<str>,
        field: &'static str,
    },
    #[error(
        "instrument {instrument} ({symbol}) {field} {value} is not positive — a zero grid cannot quantise"
    )]
    ScaleNotPositive {
        instrument: u16,
        symbol: Box<str>,
        field: &'static str,
        value: Box<str>,
    },
    #[error(
        "instrument {instrument} ({symbol}) {field} {value} is not positive — an order limit of zero means the venue would reject every order the engine could build"
    )]
    ScaleLimitNotPositive {
        instrument: u16,
        symbol: Box<str>,
        field: &'static str,
        value: Box<str>,
    },
    #[error(
        "{market} exchangeInfo carries no ORDERS rate limit — the venue has always published one, so a payload missing it is not the document this build knows how to read"
    )]
    ScaleOrderLimitsMissing { market: &'static str },
    #[error(
        "{market} ORDERS rate limit has an unreadable {field} {value} — dropping the bucket would overstate the order budget"
    )]
    ScaleRateLimitUnreadable {
        market: &'static str,
        field: &'static str,
        value: Box<str>,
    },
    #[error(
        "scale preflight could not reach {market} exchangeInfo — refusing to start unvalidated"
    )]
    ScaleUnreachable {
        market: &'static str,
        #[source]
        source: RestError,
    },
    #[error("gamma preflight failed for {series} — refusing to start unvalidated")]
    ScalePolyUnreachable {
        series: &'static str,
        #[source]
        source: GammaError,
    },
    #[error("polymarket {series} not resolvable — gamma returned no current/next window")]
    ScalePolySeriesUnknown {
        series: &'static str,
        #[source]
        source: GammaError,
    },
    #[error(
        "polymarket {symbol} tick {} not in accepted set {{{}, {}}} — book capacity and 1e-8 scale assume these",
        actual.to_f64(),
        expected[0].to_f64(),
        expected[1].to_f64()
    )]
    ScalePolyTick {
        symbol: Box<str>,
        expected: [Price; 2],
        actual: Price,
    },
    #[error(
        "polymarket {symbol} min order size {} is not positive — orderMinSize absent or malformed",
        value.to_f64()
    )]
    ScalePolyMinSize { symbol: Box<str>, value: Qty },
    #[error(
        "trading engine {trading_engine} could not bind link.bind {bind} — another process holds that port, or this host has no such address"
    )]
    LinkBind {
        bind: Box<str>,
        trading_engine: Box<str>,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "{name:?} is {found} bytes and the link's catalog frames carry {max} — shorten the name, or drop the link: block"
    )]
    LinkNameTooLong {
        name: Box<str>,
        found: usize,
        max: usize,
    },
}
