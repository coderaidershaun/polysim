//! Wire JSON shapes (Polymarket spelling). Request bodies serialised once (HMAC covers socket bytes).
//!
//! REST and stream spell the same order differently: REST created_at is int+seconds; stream is
//! string+milliseconds. Sharing struct fails at runtime not compile-time.

use serde::{Deserialize, Serialize};

/// In typehash order. salt is JSON NUMBER; rest numeric is string. expiration here, not in signed.
#[derive(Serialize)]
pub(super) struct SignedOrderBody<'a> {
    pub salt: u64,
    pub maker: &'a str,
    pub signer: &'a str,
    #[serde(rename = "tokenId")]
    pub token_id: &'a str,
    #[serde(rename = "makerAmount")]
    pub maker_amount: String,
    #[serde(rename = "takerAmount")]
    pub taker_amount: String,
    pub side: &'static str,
    pub expiration: &'static str,
    pub timestamp: String,
    #[serde(rename = "signatureType")]
    pub signature_type: u8,
    pub signature: &'a str,
    pub metadata: &'static str,
    pub builder: &'static str,
}

#[derive(Serialize)]
pub(super) struct PlaceOrderBody<'a> {
    pub order: SignedOrderBody<'a>,
    pub owner: &'a str,
    #[serde(rename = "orderType")]
    pub order_type: &'static str,
    #[serde(rename = "deferExec")]
    pub defer_exec: bool,
    #[serde(rename = "postOnly")]
    pub post_only: bool,
}

#[derive(Serialize)]
pub(super) struct CancelOrderBody<'a> {
    #[serde(rename = "orderID")]
    pub order_id: &'a str,
}

#[derive(Serialize)]
pub(super) struct CancelMarketOrdersBody<'a> {
    pub asset_id: &'a str,
}

#[derive(Serialize)]
pub(super) struct HeartbeatBody<'a> {
    pub heartbeat_id: &'a str,
}

/// Success and stale 400 both carry heartbeat_id; in 400 it's the expected id, so one shape reads both.
#[derive(Deserialize)]
pub(super) struct HeartbeatResponse {
    #[serde(default)]
    pub heartbeat_id: String,
}

/// Raw secret on this wire only, with no HMAC. Server closes if the socket goes silent.
#[derive(Serialize)]
pub(super) struct SubscribeBody<'a> {
    pub auth: SubscribeAuth<'a>,
    #[serde(rename = "type")]
    pub channel: &'static str,
}

#[derive(Serialize)]
pub(super) struct SubscribeAuth<'a> {
    #[serde(rename = "apiKey")]
    pub api_key: &'a str,
    pub secret: &'a str,
    pub passphrase: &'a str,
}

/// POST /order. HTTP 200 carries failures; amounts empty string when unmatched.
#[derive(Deserialize)]
pub(super) struct PlaceResponse {
    #[serde(default)]
    pub success: bool,
    #[serde(rename = "errorMsg", default)]
    pub error_msg: String,
    #[serde(rename = "orderID", default)]
    pub order_id: String,
    #[serde(default)]
    pub status: String,
    #[serde(rename = "makingAmount", default)]
    pub making_amount: String,
    #[serde(rename = "takingAmount", default)]
    pub taking_amount: String,
}

/// All cancel paths answer; partial success is design.
#[derive(Deserialize)]
pub(super) struct CancelResponse {
    #[serde(default)]
    pub canceled: Vec<String>,
    #[serde(default)]
    pub not_canceled: std::collections::BTreeMap<String, String>,
}

/// No numeric codes here.
#[derive(Deserialize)]
pub(super) struct ErrorResponse {
    #[serde(default)]
    pub error: String,
    #[serde(rename = "error_msg", default)]
    pub error_msg: String,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub retry_after_seconds: Option<i64>,
}

impl ErrorResponse {
    pub fn message(&self) -> &str {
        match self.error.is_empty() {
            true => &self.error_msg,
            false => &self.error,
        }
    }
}

/// Response is wrapped in a data field. The prose docs show a bare array, but that is stale.
#[derive(Deserialize)]
pub(super) struct OrdersPage {
    #[serde(default)]
    pub next_cursor: String,
    #[serde(default)]
    pub data: Vec<OrderRecord>,
}

/// created_at is an integer here. The stream sends it as a string.
#[derive(Deserialize)]
pub(super) struct OrderRecord {
    pub id: String,
    pub asset_id: String,
    pub side: String,
    pub price: String,
    pub original_size: String,
    #[serde(default)]
    pub size_matched: String,
    pub status: String,
    #[serde(default)]
    pub created_at: i64,
}

#[derive(Deserialize)]
pub(super) struct TradesPage {
    #[serde(default)]
    pub next_cursor: String,
    #[serde(default)]
    pub data: Vec<TradeRecord>,
}

/// The wire carries the taker's side, so this struct omits it. Reading it would flip attribution
/// on every trade we made. Use trader_side to tell which role we played.
#[derive(Deserialize)]
pub(super) struct TradeRecord {
    pub id: String,
    pub asset_id: String,
    #[serde(default)]
    pub taker_order_id: String,
    pub price: String,
    pub size: String,
    pub status: String,
    #[serde(default)]
    pub trader_side: String,
    #[serde(default)]
    pub fee_rate_bps: String,
    #[serde(default)]
    pub maker_orders: Vec<MakerOrderFill>,
    #[serde(default)]
    pub match_time: String,
}

#[derive(Deserialize)]
pub(super) struct MakerOrderFill {
    pub order_id: String,
    #[serde(default)]
    pub owner: String,
    pub matched_amount: String,
    pub price: String,
    #[serde(default)]
    pub fee_rate_bps: String,
}

/// event_type splits order/trade; type splits order kinds.
#[derive(Deserialize)]
#[serde(tag = "event_type")]
pub(super) enum StreamFrame {
    #[serde(rename = "order")]
    Order(Box<OrderEvent>),
    #[serde(rename = "trade")]
    Trade(Box<TradeRecord>),
    #[serde(other)]
    Unhandled,
}

/// Stream: timestamp milliseconds as string, created_at also string.
#[derive(Deserialize)]
pub(super) struct OrderEvent {
    pub id: String,
    pub asset_id: String,
    pub side: String,
    pub price: String,
    pub original_size: String,
    #[serde(default)]
    pub size_matched: String,
    #[serde(rename = "type")]
    pub event: String,
    pub status: String,
    #[serde(default)]
    pub timestamp: String,
}

/// Balance is a 6-decimal integer. Allowance is keyed by exchange address and read-only; we check
/// it to verify the CLOB cache is warm.
#[derive(Deserialize)]
pub(super) struct BalanceAllowance {
    #[serde(default)]
    pub balance: String,
}

/// Public endpoint, one call per rotation binding. mos/mts arrive as JSON numbers and are read as
/// such, not as floats, because a tick is a price-grid key.
#[derive(Deserialize)]
pub(super) struct ClobMarketResponse {
    #[serde(rename = "c")]
    pub condition_id: String,
    #[serde(rename = "t", default)]
    pub tokens: Vec<ClobTokenResponse>,
    #[serde(rename = "mos")]
    pub min_order_size: serde_json::Number,
    #[serde(rename = "mts")]
    pub min_tick_size: serde_json::Number,
    #[serde(rename = "mbf", default)]
    pub maker_fee_bps: i32,
    #[serde(rename = "tbf", default)]
    pub taker_fee_bps: i32,
    #[serde(rename = "ao", default)]
    pub is_accepting_orders: bool,
    /// Omitted when no taker delay.
    #[serde(rename = "itode", default)]
    pub is_taker_order_delay_enabled: bool,
}

#[derive(Deserialize)]
pub(super) struct ClobTokenResponse {
    #[serde(rename = "t")]
    pub token_id: String,
    #[serde(rename = "o", default)]
    pub outcome: String,
}

/// Separate call; flag picks signing contract.
#[derive(Deserialize)]
pub(super) struct NegRiskResponse {
    #[serde(default)]
    pub neg_risk: bool,
}

/// Venue answers `{version}` or bare number (see decode_protocol_version).
#[derive(Deserialize)]
pub(super) struct VersionResponse {
    pub version: u32,
}

/// closed_only is required here with no serde default. An absent field must not deserialise to
/// false, because the gate treats None as a refusal and will not arm on an absent field.
#[derive(Deserialize)]
pub(super) struct ClosedOnlyResponse {
    pub closed_only: bool,
}
