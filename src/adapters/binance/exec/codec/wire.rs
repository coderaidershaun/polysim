//! Venue JSON shapes (Binance spelling). Deserialise only; amounts = String -> mantissa once.
//! Structs permissive (Binance appends). Envelopes strict (absence = silent failure).

use serde::Deserialize;

/// Wrapped event (event required; legacy bare fails).
#[derive(Deserialize)]
pub(super) struct StreamEnvelope {
    pub event: StreamPayload,
}

#[derive(Deserialize)]
#[serde(tag = "e")]
pub(super) enum StreamPayload {
    /// Boxed. ExecutionReport wide. Unboxed = frame size tax.
    #[serde(rename = "executionReport")]
    ExecutionReport(Box<ExecutionReport>),
    #[serde(rename = "outboundAccountPosition")]
    AccountPosition(AccountPosition),
    /// Delta only. Loses frame -> wrong forever.
    #[serde(rename = "balanceUpdate")]
    BalanceUpdate,
    #[serde(other)]
    Unhandled,
}

#[derive(Deserialize)]
pub(super) struct ExecutionReport {
    /// Transact time. Event time deliberately omitted (2 stamps = 2 wrongs).
    #[serde(rename = "T")]
    pub transact_ts_ms: i64,
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "c")]
    pub client_order_id: String,
    /// Subject on cancel/amend (empty else). c = request.
    #[serde(rename = "C", default)]
    pub orig_client_order_id: String,
    #[serde(rename = "S")]
    pub side: String,
    #[serde(rename = "q")]
    pub qty: String,
    #[serde(rename = "p")]
    pub price: String,
    #[serde(rename = "x")]
    pub execution_type: String,
    #[serde(rename = "X")]
    pub order_status: String,
    #[serde(rename = "r")]
    pub reject_reason: String,
    #[serde(rename = "i")]
    pub order_id: i64,
    #[serde(rename = "l")]
    pub last_qty: String,
    #[serde(rename = "L")]
    pub last_price: String,
    #[serde(rename = "z")]
    pub cumulative_qty: String,
    #[serde(rename = "Z")]
    pub cumulative_quote: String,
    #[serde(rename = "n")]
    pub commission: String,
    /// null when no fee. Option (not empty string like other absences).
    #[serde(rename = "N")]
    pub commission_asset: Option<String>,
    #[serde(rename = "t")]
    pub trade_id: i64,
    #[serde(rename = "m")]
    pub is_maker: bool,
}

#[derive(Deserialize)]
pub(super) struct AccountPosition {
    #[serde(rename = "E")]
    pub event_ts_ms: i64,
    #[serde(rename = "u")]
    pub last_update_ms: i64,
    #[serde(rename = "B")]
    pub balances: Vec<WireBalance>,
}

#[derive(Deserialize)]
pub(super) struct WireBalance {
    #[serde(rename = "a")]
    pub asset: String,
    #[serde(rename = "f")]
    pub free: String,
    #[serde(rename = "l")]
    pub locked: String,
}

/// WS answer: status + result xor error (shapes vary by method).
#[derive(Deserialize)]
pub(super) struct ResponseEnvelope {
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<ResponseError>,
}

#[derive(Deserialize)]
pub(super) struct ResponseError {
    pub code: i32,
    #[serde(default)]
    pub msg: String,
}

/// Order in responses. Aliases matter: amend uses qty not origQty, one m not two.
#[derive(Deserialize)]
pub(super) struct OrderResponse {
    pub symbol: String,
    #[serde(rename = "orderId")]
    pub order_id: i64,
    #[serde(rename = "clientOrderId")]
    pub client_order_id: String,
    /// Cancel/amend target (clientOrderId = request).
    #[serde(rename = "origClientOrderId", default)]
    pub orig_client_order_id: Option<String>,
    #[serde(rename = "transactTime", default)]
    pub transact_ts_ms: Option<i64>,
    /// Used by order.status/openOrders.status (not transactTime).
    #[serde(rename = "updateTime", default)]
    pub update_ts_ms: Option<i64>,
    pub price: String,
    #[serde(rename = "origQty", alias = "qty")]
    pub qty: String,
    #[serde(rename = "executedQty")]
    pub executed_qty: String,
    #[serde(rename = "cummulativeQuoteQty", alias = "cumulativeQuoteQty")]
    pub cumulative_quote: String,
    pub status: String,
    pub side: String,
}

#[derive(Deserialize)]
pub(super) struct AmendResult {
    #[serde(rename = "amendedOrder")]
    pub amended_order: OrderResponse,
}
