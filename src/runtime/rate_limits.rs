//! The venue's ORDERS budget, read at startup from `exchangeInfo.rateLimits` — account-scoped, so
//! it is read once per market rather than per symbol. An unreadable or absent bucket refuses the
//! run: a budget the engine never read is one it cannot avoid exhausting.
//!
//! Read here in the venue's own words, then handed to the engine as an [`OrderBudget`] the hot side
//! paces against without knowing whose buckets they are.

use crate::adapters::binance::rest::ExchangeInfo;
use crate::config::BinanceMarket;
use crate::hot::exec::{MAX_ORDER_BUDGET_WINDOWS, OrderBudget, OrderBudgetWindow};
use crate::registry::{OrderRateLimit, RateInterval};
use crate::time::DurationUs;

use super::EngineError;

/// Binance tags an order-count bucket with this `rateLimitType`; weight and raw-request buckets
/// ride the same array and are not ours to pace against.
const ORDERS_LIMIT_TYPE: &str = "ORDERS";

/// Reads every ORDERS bucket the payload carries. Purely a function of its input, with no
/// I/O of its own.
///
/// # Errors
/// [`EngineError::ScaleOrderLimitsMissing`], [`EngineError::ScaleRateLimitUnreadable`].
pub fn check_order_rate_limits(
    market: BinanceMarket,
    info: &ExchangeInfo,
) -> Result<Vec<OrderRateLimit>, EngineError> {
    let mut limits = Vec::new();
    for entry in info
        .rate_limits
        .iter()
        .filter(|entry| entry.rate_limit_type.as_ref() == ORDERS_LIMIT_TYPE)
    {
        let Some(interval) = RateInterval::parse(&entry.interval) else {
            return Err(unreadable(market, "interval", &entry.interval));
        };
        if entry.interval_num == 0 {
            return Err(unreadable(
                market,
                "intervalNum",
                &entry.interval_num.to_string(),
            ));
        }
        if entry.limit == 0 {
            return Err(unreadable(market, "limit", &entry.limit.to_string()));
        }
        limits.push(OrderRateLimit {
            interval,
            interval_num: entry.interval_num,
            limit: entry.limit,
        });
    }
    if limits.is_empty() {
        return Err(EngineError::ScaleOrderLimitsMissing {
            market: market.as_str(),
        });
    }
    Ok(limits)
}

/// What the engine paces against, from the buckets the venue published.
///
/// # Errors
/// [`EngineError::ExecutionTooManyOrderWindows`] when the venue declares more buckets than the
/// engine models.
pub fn order_budget(limits: &[OrderRateLimit]) -> Result<OrderBudget, EngineError> {
    let windows: Vec<OrderBudgetWindow> = limits
        .iter()
        .map(|limit| OrderBudgetWindow {
            window: DurationUs::from_secs(
                limit.interval.as_secs() as i64 * i64::from(limit.interval_num),
            ),
            // Saturating a cap DOWN refuses earlier, which is the safe direction; no venue
            // publishes an order count anywhere near this, so nothing real is being clamped.
            max_places: u32::try_from(limit.limit).unwrap_or(u32::MAX),
        })
        .collect();
    OrderBudget::of(&windows).ok_or(EngineError::ExecutionTooManyOrderWindows {
        found: windows.len(),
        max: MAX_ORDER_BUDGET_WINDOWS,
    })
}

fn unreadable(market: BinanceMarket, field: &'static str, value: &str) -> EngineError {
    EngineError::ScaleRateLimitUnreadable {
        market: market.as_str(),
        field,
        value: value.into(),
    }
}
