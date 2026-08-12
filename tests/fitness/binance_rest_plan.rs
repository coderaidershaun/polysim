//! Signed-REST fitness: the documented IP weight, HTTP method and authentication of every request
//! this build can issue, and the verdict it draws from a venue failure. Both fail silently if wrong —
//! an under-charged weight earns an IP ban mid-session, and a mis-classified failure either retries a
//! rejected order or gives up on a transient one. The live integration suite never runs in CI, so
//! this is the only standing guard on the table.

use polysim::adapters::binance::rest::{
    AccountInfo, BinanceEnv, FailureVerdict, HttpMethod, KlineQuery, OrderCountWindow, OrderRecord,
    RequestAuth, RestClient, RestError, RestRequest,
};
use polysim::config::{BinanceMarket, KlineInterval};

/// Weights verified against <https://developers.binance.com/docs/binance-spot-api-docs/rest-api>
/// on 2026-07-27 — account 20, myTrades 20 without `orderId`, openOrders 6 for one symbol,
/// order 4, cancel 1.
#[test]
fn every_request_plans_its_documented_weight_method_and_auth() {
    let cases = [
        (
            RestRequest::AccountInfo,
            20,
            HttpMethod::Get,
            RequestAuth::Signed,
            "account",
        ),
        (
            RestRequest::MyTrades {
                symbol: "btcusdt".to_owned(),
                from_id: Some(42),
                limit: 500,
            },
            20,
            HttpMethod::Get,
            RequestAuth::Signed,
            "myTrades",
        ),
        (
            RestRequest::OpenOrders {
                symbol: "btcusdt".to_owned(),
            },
            6,
            HttpMethod::Get,
            RequestAuth::Signed,
            "openOrders",
        ),
        (
            RestRequest::OrderStatus {
                symbol: "btcusdt".to_owned(),
                orig_client_order_id: "poly-1".to_owned(),
            },
            4,
            HttpMethod::Get,
            RequestAuth::Signed,
            "order",
        ),
        (
            RestRequest::CancelOrder {
                symbol: "btcusdt".to_owned(),
                orig_client_order_id: "poly-1".to_owned(),
            },
            1,
            HttpMethod::Delete,
            RequestAuth::Signed,
            "cancelOrder",
        ),
    ];

    for (request, weight, method, auth, endpoint) in cases {
        let plan = request.plan(BinanceMarket::Spot);
        assert_eq!(plan.weight, weight, "weight for {endpoint}");
        assert_eq!(plan.method, method, "method for {endpoint}");
        assert_eq!(plan.auth, auth, "auth for {endpoint}");
        assert_eq!(plan.endpoint, endpoint);
    }
}

/// The public market-data half must stay exactly as it was — a weight change there would silently
/// re-tune the resync-storm guard the depth adapter leans on.
#[test]
fn the_public_half_stays_unsigned_and_unchanged() {
    let cases = [
        (
            RestRequest::DepthSnapshot {
                symbol: "btcusdt".to_owned(),
                limit: 1000,
            },
            50,
            "depth",
        ),
        (
            RestRequest::Klines(KlineQuery {
                symbol: "btcusdt".to_owned(),
                interval: KlineInterval::OneMinute,
                limit: 500,
                start_ts_ms: None,
                end_ts_ms: None,
            }),
            2,
            "klines",
        ),
        (
            RestRequest::ExchangeInfo {
                symbols: Vec::new(),
            },
            20,
            "exchangeInfo",
        ),
        (RestRequest::ServerTime, 1, "time"),
    ];

    for (request, weight, endpoint) in cases {
        let plan = request.plan(BinanceMarket::Spot);
        assert_eq!(plan.weight, weight, "weight for {endpoint}");
        assert_eq!(plan.method, HttpMethod::Get, "method for {endpoint}");
        assert_eq!(plan.auth, RequestAuth::Public, "auth for {endpoint}");
    }
}

/// The public client must refuse a private request rather than send it unsigned. Reached without a
/// socket: the guard fires before any connection is opened.
#[tokio::test]
async fn the_public_client_refuses_a_private_request() {
    let mut client = RestClient::new(BinanceMarket::Spot, BinanceEnv::Production)
        .expect("the tls backend builds");

    let refused = client.fetch_text(&RestRequest::AccountInfo).await;

    assert!(
        matches!(
            refused,
            Err(RestError::RequiresSignature {
                endpoint: "account"
            })
        ),
        "expected a signature refusal, got {refused:?}"
    );
}

#[test]
fn venue_codes_classify_into_retry_routine_and_fatal() {
    let verdict_of = |code: Option<i64>| {
        RestError::Status {
            url: "https://api.binance.com/api/v3/order".to_owned(),
            status: 400,
            code,
            message: None,
        }
        .verdict()
    };

    for code in [-1021, -1003, -1006, -1007, -1000, -1001] {
        assert_eq!(verdict_of(Some(code)), FailureVerdict::Retry, "code {code}");
    }
    // A cancel losing a race to a fill is normal traffic for a market maker, not a fault.
    for code in [-2013, -2011] {
        assert_eq!(
            verdict_of(Some(code)),
            FailureVerdict::Routine,
            "code {code}"
        );
    }
    for code in [-1022, -2014, -2015, -1100, -1101, -1102] {
        assert_eq!(verdict_of(Some(code)), FailureVerdict::Fatal, "code {code}");
    }
}

/// An unrecognised code must NOT be retried. Retrying an unhandled failure against an order endpoint
/// is how one bug becomes a burst of duplicate orders.
#[test]
fn an_unknown_venue_code_is_fatal_not_retryable() {
    let unknown = RestError::Status {
        url: "https://api.binance.com/api/v3/order".to_owned(),
        status: 400,
        code: Some(-9999),
        message: Some("something the docs never listed".to_owned()),
    };
    assert_eq!(unknown.verdict(), FailureVerdict::Fatal);

    let absent_code = RestError::Status {
        url: "https://api.binance.com/api/v3/order".to_owned(),
        status: 500,
        code: None,
        message: None,
    };
    assert_eq!(absent_code.verdict(), FailureVerdict::Fatal);
}

#[test]
fn a_rejected_key_is_fatal_and_rate_limiting_is_retryable() {
    let unauthorized = RestError::Unauthorized {
        url: "https://api.binance.com/api/v3/account".to_owned(),
        status: 401,
        code: Some(-2015),
        message: None,
    };
    assert_eq!(unauthorized.verdict(), FailureVerdict::Fatal);

    let limited = RestError::RateLimited {
        url: "https://api.binance.com/api/v3/account".to_owned(),
        status: 429,
        retry_after_secs: Some(5),
        code: None,
        message: None,
    };
    assert_eq!(limited.verdict(), FailureVerdict::Retry);
}

/// Order counts arrive over whatever intervals the venue chooses, so the interval is read from the
/// header name. Matching a fixed `-1m` the way IP weight does would silently observe nothing.
#[test]
fn order_count_intervals_come_from_the_header_name() {
    assert_eq!(
        OrderCountWindow::interval_of_header("x-mbx-order-count-10s"),
        Some("10s")
    );
    assert_eq!(
        OrderCountWindow::interval_of_header("x-mbx-order-count-1d"),
        Some("1d")
    );
    assert_eq!(
        OrderCountWindow::interval_of_header("x-mbx-order-count-"),
        None
    );
    assert_eq!(
        OrderCountWindow::interval_of_header("x-mbx-used-weight-1m"),
        None
    );
}

/// Shape recorded from a live `GET /api/v3/account` on 2026-07-27, trimmed to the fields the engine
/// reads plus enough neighbours to prove unknown ones are ignored rather than fatal.
const LIVE_ACCOUNT: &str = r#"{
  "makerCommission": 10,
  "takerCommission": 10,
  "buyerCommission": 0,
  "sellerCommission": 0,
  "commissionRates": {
    "maker": "0.00100000",
    "taker": "0.00100000",
    "buyer": "0.00000000",
    "seller": "0.00000000"
  },
  "canTrade": true,
  "canWithdraw": true,
  "canDeposit": true,
  "brokered": false,
  "requireSelfTradePrevention": false,
  "preventSor": false,
  "updateTime": 1769500000000,
  "accountType": "SPOT",
  "balances": [
    { "asset": "BTC", "free": "0.00135871", "locked": "0.00000000" },
    { "asset": "USDT", "free": "171.14535000", "locked": "0.00000000" },
    { "asset": "EDG", "free": "0.00000000", "locked": "0.00000000" },
    { "asset": "EON", "free": "0.00000000", "locked": "0.00000000" }
  ],
  "permissions": ["TRD_GRP_015"],
  "uid": 123456789
}"#;

#[test]
fn the_live_account_shape_decodes_and_ignores_unknown_fields() {
    let account: AccountInfo = serde_json::from_str(LIVE_ACCOUNT).expect("the live shape decodes");

    assert!(account.can_trade);
    assert_eq!(&*account.account_type, "SPOT");
    assert_eq!(&*account.commission_rates.maker, "0.00100000");
    assert_eq!(&*account.commission_rates.taker, "0.00100000");
    assert_eq!(account.permissions.len(), 1);
    assert_eq!(account.balances.len(), 4);
}

/// A live spot account carries dozens of zero-balance dust assets; surfacing them all would bury the
/// two that matter.
#[test]
fn only_funded_balances_are_surfaced() {
    let account: AccountInfo = serde_json::from_str(LIVE_ACCOUNT).expect("the live shape decodes");

    let funded: Vec<&str> = account
        .funded_balances()
        .map(|balance| &*balance.asset)
        .collect();

    assert_eq!(funded, ["BTC", "USDT"]);
}

/// The venue spells the field with two m's. Corrected to `cumulative`, serde would find nothing and
/// default the value to an empty string — a filled order reading as zero notional.
#[test]
fn an_order_record_reads_the_venues_own_misspelling() {
    let raw = r#"{
      "symbol": "BTCUSDT",
      "orderId": 28457,
      "orderListId": -1,
      "clientOrderId": "poly-1",
      "price": "118000.00",
      "origQty": "0.00008000",
      "executedQty": "0.00008000",
      "cummulativeQuoteQty": "9.44000000",
      "status": "FILLED",
      "timeInForce": "GTC",
      "type": "LIMIT",
      "side": "BUY",
      "time": 1769500000000,
      "updateTime": 1769500000500,
      "workingTime": 1769500000000,
      "selfTradePreventionMode": "EXPIRE_MAKER"
    }"#;

    let order: OrderRecord = serde_json::from_str(raw).expect("an order record decodes");

    assert_eq!(&*order.cumulative_quote_qty, "9.44000000");
    assert_eq!(&*order.status, "FILLED");
    assert_eq!(order.orig_client_order_id, None);
    assert_eq!(order.update_time_ms, Some(1769500000500));
}

/// A cancel acknowledgement is the same shape plus the id it cancelled — the field that tells the
/// engine WHICH of its orders is gone.
#[test]
fn a_cancel_acknowledgement_names_the_order_it_cancelled() {
    let raw = r#"{
      "symbol": "BTCUSDT",
      "origClientOrderId": "poly-1",
      "orderId": 28457,
      "orderListId": -1,
      "clientOrderId": "cancel-abc",
      "transactTime": 1769500001000,
      "price": "118000.00",
      "origQty": "0.00008000",
      "executedQty": "0.00000000",
      "cummulativeQuoteQty": "0.00000000",
      "status": "CANCELED",
      "timeInForce": "GTC",
      "type": "LIMIT",
      "side": "BUY"
    }"#;

    let order: OrderRecord = serde_json::from_str(raw).expect("a cancel ack decodes");

    assert_eq!(order.orig_client_order_id.as_deref(), Some("poly-1"));
    assert_eq!(&*order.client_order_id, "cancel-abc");
    assert_eq!(order.transact_time_ms, Some(1769500001000));
    assert_eq!(order.time_ms, None);
}

/// `-2010 NEW_ORDER_REJECTED` is TWO failures wearing one code, and only the message separates them.
/// "Order would immediately match and take" is the venue enforcing the post-only we asked for — the
/// ordinary outcome when the book moves between our snapshot and the venue's. "Account has
/// insufficient balance" is the same code and is a real problem. Classifying on the code alone files
/// a funding failure as expected traffic.
#[test]
fn the_two_meanings_of_2010_are_classified_apart() {
    let rejected = |message: &str| {
        RestError::Status {
            url: "https://api.binance.com/api/v3/order".to_owned(),
            status: 400,
            code: Some(-2010),
            message: Some(message.to_owned()),
        }
        .verdict()
    };

    assert_eq!(
        rejected("Order would immediately match and take."),
        FailureVerdict::Routine,
        "a post-only cross is the venue doing what we asked"
    );
    assert_eq!(
        rejected("Account has insufficient balance for requested action."),
        FailureVerdict::Fatal,
        "running out of money is not routine traffic"
    );
}

/// An unrecognised -2010 must not be filed as routine. Unknown means unhandled, and an unhandled
/// rejection logged as expected traffic is one nobody ever looks at.
#[test]
fn an_unrecognised_2010_is_not_routine() {
    let unknown = RestError::Status {
        url: "https://api.binance.com/api/v3/order".to_owned(),
        status: 400,
        code: Some(-2010),
        message: Some("Some reason the docs never listed.".to_owned()),
    };
    assert_eq!(unknown.verdict(), FailureVerdict::Fatal);

    let no_message = RestError::Status {
        url: "https://api.binance.com/api/v3/order".to_owned(),
        status: 400,
        code: Some(-2010),
        message: None,
    };
    assert_eq!(no_message.verdict(), FailureVerdict::Fatal);
}
