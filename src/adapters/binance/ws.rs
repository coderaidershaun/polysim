//! Binance WS addressing: which host, which stream names, and the pong spot demands. Dialling is
//! shared (`adapters::socket`); reading and reconnect live in the actor.

use futures_util::SinkExt;
use tokio_tungstenite::tungstenite::{Bytes, Error as ProtocolError, Message};

use crate::config::{BinanceMarket, KlineInterval};

use super::rest::BinanceEnv;

/// Futures: depth→/public, trades/klines→/market. Spot ignores routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamCategory {
    Depth,
    Trades,
    Klines,
}

impl StreamCategory {
    fn futures_route(self) -> &'static str {
        match self {
            StreamCategory::Depth => "public",
            StreamCategory::Trades | StreamCategory::Klines => "market",
        }
    }
}

/// `@100ms` cadence (venue default is 1000ms on spot).
pub fn depth_stream(symbol: &str) -> String {
    format!("{}@depth@100ms", symbol.to_lowercase())
}

pub fn agg_trade_stream(symbol: &str) -> String {
    format!("{}@aggTrade", symbol.to_lowercase())
}

pub fn kline_stream(symbol: &str, interval: KlineInterval) -> String {
    format!("{}@kline_{}", symbol.to_lowercase(), interval.as_str())
}

/// Stamps arrive in milliseconds. Spot can be asked for microseconds instead, and that request is
/// deliberately absent: `parse.rs` scales every venue stamp by 1000, so asking for it without the
/// matching parse change multiplies spot timestamps by a thousand.
pub fn combined_stream_url(
    market: BinanceMarket,
    env: BinanceEnv,
    category: StreamCategory,
    streams: &[String],
) -> String {
    let host = ws_host(market, env);
    let joined = streams.join("/");
    match market {
        BinanceMarket::Spot => format!("{host}/stream?streams={joined}"),
        BinanceMarket::Perpetual => {
            format!(
                "{host}/{}/stream?streams={joined}",
                category.futures_route()
            )
        }
    }
}

fn ws_host(market: BinanceMarket, env: BinanceEnv) -> &'static str {
    match (env, market) {
        (BinanceEnv::Production, BinanceMarket::Spot) => "wss://stream.binance.com:9443",
        (BinanceEnv::Production, BinanceMarket::Perpetual) => "wss://fstream.binance.com",
        (BinanceEnv::Testnet, BinanceMarket::Spot) => "wss://stream.testnet.binance.vision",
        (BinanceEnv::Testnet, BinanceMarket::Perpetual) => "wss://demo-fstream.binance.com",
    }
}

/// Spot only; Futures WS API on different host. 24h max lifetime → force reconnect inside window.
/// # Warning: TESTNET host unverified (production-only deployment); verify before first testnet run.
pub fn ws_api_url(env: BinanceEnv) -> &'static str {
    match env {
        BinanceEnv::Production => "wss://ws-api.binance.com:443/ws-api/v3",
        BinanceEnv::Testnet => "wss://ws-api.testnet.binance.vision/ws-api/v3",
    }
}

/// Spot requires a pong within 1 min and tokio-tungstenite does not auto-pong.
///
/// Generic over the sink so a whole socket and the write half of a SPLIT one share one
/// implementation: an actor that both reads and writes has to split, and answering a ping is not a
/// thing worth having two versions of.
pub(crate) async fn reply_to_ping<S>(sink: &mut S, payload: Bytes) -> Result<(), ProtocolError>
where
    S: futures_util::Sink<Message, Error = ProtocolError> + Unpin,
{
    sink.send(Message::Pong(payload)).await
}
