//! Deserialised Binance REST response shapes (public + private). Decimals stay as venue strings.

use serde::Deserialize;

/// exchangeInfo subset (startup 1e-8 scale check).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ExchangeInfo {
    #[serde(rename = "rateLimits", default)]
    pub rate_limits: Vec<RateLimit>,
    #[serde(default)]
    pub symbols: Vec<SymbolInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RateLimit {
    #[serde(rename = "rateLimitType")]
    pub rate_limit_type: Box<str>,
    pub interval: Box<str>,
    #[serde(rename = "intervalNum")]
    pub interval_num: u32,
    pub limit: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SymbolInfo {
    pub symbol: Box<str>,
    #[serde(default)]
    pub filters: Vec<SymbolFilter>,
}

/// Union of every filter shape engine reads (spot + futures use different names for same limits).
/// Key on filter_type first (several spell limit; MARKET_LOT_SIZE mirrors LOT_SIZE).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SymbolFilter {
    #[serde(rename = "filterType")]
    pub filter_type: Box<str>,
    #[serde(rename = "tickSize", default)]
    pub tick_size: Option<Box<str>>,
    #[serde(rename = "stepSize", default)]
    pub step_size: Option<Box<str>>,
    #[serde(rename = "minPrice", default)]
    pub min_price: Option<Box<str>>,
    #[serde(rename = "maxPrice", default)]
    pub max_price: Option<Box<str>>,
    #[serde(rename = "minQty", default)]
    pub min_qty: Option<Box<str>>,
    #[serde(rename = "maxQty", default)]
    pub max_qty: Option<Box<str>>,
    #[serde(rename = "minNotional", default)]
    pub min_notional: Option<Box<str>>,
    #[serde(default)]
    pub notional: Option<Box<str>>,
    #[serde(rename = "maxNumOrders", default)]
    pub max_num_orders: Option<u32>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(rename = "maxNumOrderAmends", default)]
    pub max_num_order_amends: Option<u32>,
}

/// GET /api/v3/account. Only fields engine acts on (venue returns ~20 more). deny_unknown_fields
/// absent (venue additions don't break startup).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AccountInfo {
    #[serde(rename = "canTrade")]
    pub can_trade: bool,
    #[serde(rename = "canWithdraw", default)]
    pub can_withdraw: bool,
    #[serde(rename = "canDeposit", default)]
    pub can_deposit: bool,
    #[serde(rename = "accountType")]
    pub account_type: Box<str>,
    /// Account-specific trading group (logged, never gated).
    #[serde(default)]
    pub permissions: Vec<Box<str>>,
    #[serde(rename = "commissionRates")]
    pub commission_rates: CommissionRates,
    #[serde(default)]
    pub balances: Vec<Balance>,
    #[serde(default)]
    pub uid: Option<u64>,
    #[serde(rename = "updateTime", default)]
    pub update_time_ms: i64,
}

impl AccountInfo {
    /// Balances worth showing (live accounts have dozens of dust zeros).
    pub fn funded_balances(&self) -> impl Iterator<Item = &Balance> {
        self.balances.iter().filter(|balance| balance.is_funded())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CommissionRates {
    pub maker: Box<str>,
    pub taker: Box<str>,
    pub buyer: Box<str>,
    pub seller: Box<str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Balance {
    pub asset: Box<str>,
    pub free: Box<str>,
    pub locked: Box<str>,
}

impl Balance {
    /// String test, not parse (venue pads to 8 decimals, "zeroes+dots" avoids dust conversion).
    pub fn is_funded(&self) -> bool {
        let has_value = |amount: &str| amount.chars().any(|digit| ('1'..='9').contains(&digit));
        has_value(&self.free) || has_value(&self.locked)
    }
}

/// Fill from GET /api/v3/myTrades.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AccountTrade {
    pub symbol: Box<str>,
    pub id: i64,
    #[serde(rename = "orderId")]
    pub order_id: i64,
    pub price: Box<str>,
    pub qty: Box<str>,
    #[serde(rename = "quoteQty")]
    pub quote_qty: Box<str>,
    pub commission: Box<str>,
    #[serde(rename = "commissionAsset")]
    pub commission_asset: Box<str>,
    #[serde(rename = "time")]
    pub time_ms: i64,
    #[serde(rename = "isBuyer")]
    pub is_buyer: bool,
    #[serde(rename = "isMaker")]
    pub is_maker: bool,
}

/// Order as venue reports (shared shape: GET order, GET openOrders, DELETE acknowledgement).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OrderRecord {
    pub symbol: Box<str>,
    #[serde(rename = "orderId")]
    pub order_id: i64,
    #[serde(rename = "clientOrderId", default)]
    pub client_order_id: Box<str>,
    #[serde(rename = "origClientOrderId", default)]
    pub orig_client_order_id: Option<Box<str>>,
    pub price: Box<str>,
    #[serde(rename = "origQty")]
    pub orig_qty: Box<str>,
    #[serde(rename = "executedQty")]
    pub executed_qty: Box<str>,
    #[serde(rename = "cummulativeQuoteQty", default)]
    pub cumulative_quote_qty: Box<str>,
    pub status: Box<str>,
    #[serde(rename = "timeInForce", default)]
    pub time_in_force: Box<str>,
    #[serde(rename = "type", default)]
    pub order_type: Box<str>,
    #[serde(default)]
    pub side: Box<str>,
    #[serde(rename = "time", default)]
    pub time_ms: Option<i64>,
    #[serde(rename = "updateTime", default)]
    pub update_time_ms: Option<i64>,
    #[serde(rename = "transactTime", default)]
    pub transact_time_ms: Option<i64>,
}
