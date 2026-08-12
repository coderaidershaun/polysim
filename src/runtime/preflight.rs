//! Startup scale preflight: confirm each instrument's venue max price and quantity fit the i64
//! 1e-8 range, and stamp the venue tick/step grid plus order floors and counts onto the registry.
//! Anything out of range, unlisted, absent or unreachable refuses the start loudly. Polymarket
//! reads from Gamma but shares the taxonomy.

use crate::adapters::binance::rest::{BinanceEnv, ExchangeInfo, RestClient, SymbolFilter};
use crate::adapters::polymarket::rest::{GammaError, GammaMarket, PolyRest};
use crate::config::{BinanceMarket, PolySeries, VenueMarket};
use crate::hot::exec::MAX_QUOTE_LEVELS;
use crate::ids::{DecimalError, FIXED_SCALE, InstrumentId, Price, Qty};
use crate::info;
use crate::registry::{BinanceLimits, InstrumentRow, PolyLimits, Registry};
use crate::time::EngineClock;

use super::EngineError;
use super::rate_limits::check_order_rate_limits;

/// Quote ticks for btc-updown-5m: 0.01 normally, and 0.001 near the 0/1 bounds at resolution.
/// Any other tick refuses the start.
const POLY_ACCEPTED_TICKS: [Price; 2] = [Price(FIXED_SCALE / 100), Price(FIXED_SCALE / 1000)];

/// Polymarket sizes in shares to two decimals, whatever the tick.
const POLY_SHARE_STEP: Qty = Qty(FIXED_SCALE / 100);

/// Our own ceiling, not the venue's: Polymarket publishes no per-market order count, and an
/// unstamped one leaves the edge's mirror sized zero and the first placement fatal. Four full
/// ladders is room for the working set plus orders the venue has not yet confirmed gone.
const POLY_MAX_ORDERS: u32 = 4 * MAX_QUOTE_LEVELS as u32;

pub(super) async fn preflight_scales(
    registry: &mut Registry,
    env: BinanceEnv,
) -> Result<(), EngineError> {
    for binance_market in [BinanceMarket::Spot, BinanceMarket::Perpetual] {
        let members: Vec<InstrumentId> = registry
            .instruments()
            .iter()
            .filter(|row| row.market == VenueMarket::Binance(binance_market))
            .map(|row| row.instrument_id)
            .collect();
        if members.is_empty() {
            continue;
        }
        let mut client = RestClient::new(binance_market, env).map_err(|source| {
            EngineError::ScaleUnreachable {
                market: binance_market.as_str(),
                source,
            }
        })?;
        let info =
            client
                .exchange_info(&[])
                .await
                .map_err(|source| EngineError::ScaleUnreachable {
                    market: binance_market.as_str(),
                    source,
                })?;
        for id in members {
            let scales = check_symbol_scale(registry.instrument(id), &info)?;
            registry.set_scales(id, scales.tick_size, scales.step_size);
            registry.set_binance_limits(id, scales.limits);
        }
        registry.set_order_rate_limits(check_order_rate_limits(binance_market, &info)?);
    }
    Ok(())
}

/// Confirm ladder fits venue's per-symbol order ceiling before threads start.
///
/// # Errors
/// [`EngineError::ExecutionOrderCapacity`] on the first instrument whose ladder overruns.
pub(super) fn check_execution_order_capacity(
    registry: &Registry,
    max_orders_per_side: u32,
) -> Result<(), EngineError> {
    for row in registry.instruments() {
        check_symbol_order_capacity(row, max_orders_per_side)?;
    }
    Ok(())
}

/// Pure per-symbol half of the startup order-capacity check.
///
/// # Errors
/// [`EngineError::ExecutionOrderCapacity`] when both sides of the ladder exceed the venue ceiling.
pub fn check_symbol_order_capacity(
    row: &InstrumentRow,
    max_orders_per_side: u32,
) -> Result<(), EngineError> {
    let Some(venue_max) = row.max_num_orders else {
        return Ok(());
    };
    let required_total = max_orders_per_side.saturating_mul(2);
    if required_total <= venue_max {
        return Ok(());
    }
    Err(EngineError::ExecutionOrderCapacity {
        instrument: row.instrument_id.0,
        symbol: row.venue_symbol.clone(),
        configured_per_side: max_orders_per_side,
        required_total,
        venue_max,
    })
}

/// The trading grid and order limits the scale preflight parsed from one instrument's
/// `exchangeInfo` entry — the venue increments a strategy quantises to, plus the floors and counts
/// below which the venue rejects an order outright.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SymbolScales {
    pub tick_size: Price,
    pub step_size: Qty,
    pub limits: BinanceLimits,
}

/// Confirm exchangeInfo entry carries max price/qty that parse within i64 1e-8, return grid + limits. Pure.
///
/// # Errors
/// [`EngineError::ScaleSymbolUnknown`], [`EngineError::ScaleFieldMissing`],
/// [`EngineError::ScaleOutOfRange`], [`EngineError::ScaleNotPositive`], [`EngineError::ScaleLimitNotPositive`].
pub fn check_symbol_scale(
    row: &InstrumentRow,
    info: &ExchangeInfo,
) -> Result<SymbolScales, EngineError> {
    let VenueMarket::Binance(binance_market) = row.market else {
        return Err(symbol_unknown(row));
    };
    let Some(symbol) = info
        .symbols
        .iter()
        .find(|entry| entry.symbol.eq_ignore_ascii_case(&row.venue_symbol))
    else {
        return Err(symbol_unknown(row));
    };

    let Some(max_price) = filter_field(&symbol.filters, "PRICE_FILTER", |f| f.max_price.as_deref())
    else {
        return Err(scale_field_missing(row, "PRICE_FILTER.maxPrice"));
    };
    Price::parse_decimal(max_price)
        .map_err(|source| scale_out_of_range(row, "maxPrice", max_price, source))?;

    let Some(max_qty) = filter_field(&symbol.filters, "LOT_SIZE", |f| f.max_qty.as_deref()) else {
        return Err(scale_field_missing(row, "LOT_SIZE.maxQty"));
    };
    Qty::parse_decimal(max_qty)
        .map_err(|source| scale_out_of_range(row, "maxQty", max_qty, source))?;

    let Some(tick) = filter_field(&symbol.filters, "PRICE_FILTER", |f| f.tick_size.as_deref())
    else {
        return Err(scale_field_missing(row, "PRICE_FILTER.tickSize"));
    };
    let tick_size = Price::parse_decimal(tick)
        .map_err(|source| scale_out_of_range(row, "tickSize", tick, source))?;
    if tick_size.0 <= 0 {
        return Err(scale_not_positive(row, "tickSize", tick));
    }

    let Some(step) = filter_field(&symbol.filters, "LOT_SIZE", |f| f.step_size.as_deref()) else {
        return Err(scale_field_missing(row, "LOT_SIZE.stepSize"));
    };
    let step_size = Qty::parse_decimal(step)
        .map_err(|source| scale_out_of_range(row, "stepSize", step, source))?;
    if step_size.0 <= 0 {
        return Err(scale_not_positive(row, "stepSize", step));
    }

    let limits = check_symbol_limits(row, binance_market, &symbol.filters)?;

    Ok(SymbolScales {
        tick_size,
        step_size,
        limits,
    })
}

/// Order floors/counts -> venue rejects. Spot/futures differ. Market chooses names.
fn check_symbol_limits(
    row: &InstrumentRow,
    market: BinanceMarket,
    filters: &[SymbolFilter],
) -> Result<BinanceLimits, EngineError> {
    // LOT_SIZE not MARKET_LOT_SIZE: latter quotes 0.00000000 for spot, reads as no floor.
    let Some(min_qty) = filter_field(filters, "LOT_SIZE", |f| f.min_qty.as_deref()) else {
        return Err(scale_field_missing(row, "LOT_SIZE.minQty"));
    };
    let min_qty = Qty::parse_decimal(min_qty)
        .map_err(|source| scale_out_of_range(row, "minQty", min_qty, source))?;
    if min_qty.0 <= 0 {
        return Err(scale_limit_not_positive(
            row,
            "minQty",
            min_qty.0.to_string(),
        ));
    }

    let (notional, notional_field) = match market {
        BinanceMarket::Spot => (
            filter_field(filters, "NOTIONAL", |f| f.min_notional.as_deref()),
            "NOTIONAL.minNotional",
        ),
        BinanceMarket::Perpetual => (
            filter_field(filters, "MIN_NOTIONAL", |f| f.notional.as_deref()),
            "MIN_NOTIONAL.notional",
        ),
    };
    let Some(notional) = notional else {
        return Err(scale_field_missing(row, notional_field));
    };
    // Notional is quote-unit money amount, uses same 1e-8 mantissa as quote_volume.
    let min_notional = Qty::parse_decimal(notional)
        .map(|value| value.0)
        .map_err(|source| scale_out_of_range(row, "minNotional", notional, source))?;
    if min_notional <= 0 {
        return Err(scale_limit_not_positive(
            row,
            "minNotional",
            min_notional.to_string(),
        ));
    }

    let (max_num_orders, orders_field) = match market {
        BinanceMarket::Spot => (
            filter_field(filters, "MAX_NUM_ORDERS", |f| f.max_num_orders),
            "MAX_NUM_ORDERS.maxNumOrders",
        ),
        BinanceMarket::Perpetual => (
            filter_field(filters, "MAX_NUM_ORDERS", |f| f.limit),
            "MAX_NUM_ORDERS.limit",
        ),
    };
    let Some(max_num_orders) = max_num_orders else {
        return Err(scale_field_missing(row, orders_field));
    };
    if max_num_orders == 0 {
        return Err(scale_limit_not_positive(
            row,
            "maxNumOrders",
            "0".to_owned(),
        ));
    }

    let max_num_order_amends = match market {
        BinanceMarket::Spot => Some(spot_amend_cap(row, filters)?),
        BinanceMarket::Perpetual => None, // Futures schema, not field left unread.
    };

    Ok(BinanceLimits {
        min_qty,
        min_notional,
        max_num_orders,
        max_num_order_amends,
    })
}

fn spot_amend_cap(row: &InstrumentRow, filters: &[SymbolFilter]) -> Result<u32, EngineError> {
    let Some(amends) = filter_field(filters, "MAX_NUM_ORDER_AMENDS", |f| f.max_num_order_amends)
    else {
        return Err(scale_field_missing(
            row,
            "MAX_NUM_ORDER_AMENDS.maxNumOrderAmends",
        ));
    };
    if amends == 0 {
        return Err(scale_limit_not_positive(
            row,
            "maxNumOrderAmends",
            "0".to_owned(),
        ));
    }
    Ok(amends)
}

/// Named field if filter + field both present.
fn filter_field<'a, T>(
    filters: &'a [SymbolFilter],
    filter_type: &str,
    pick: impl Fn(&'a SymbolFilter) -> Option<T>,
) -> Option<T> {
    filters
        .iter()
        .find(|filter| filter.filter_type.as_ref() == filter_type)
        .and_then(pick)
}

fn symbol_unknown(row: &InstrumentRow) -> EngineError {
    EngineError::ScaleSymbolUnknown {
        instrument: row.instrument_id.0,
        symbol: row.venue_symbol.clone(),
        market: row.market.as_str(),
    }
}

fn scale_field_missing(row: &InstrumentRow, field: &'static str) -> EngineError {
    EngineError::ScaleFieldMissing {
        instrument: row.instrument_id.0,
        symbol: row.venue_symbol.clone(),
        field,
    }
}

fn scale_out_of_range(
    row: &InstrumentRow,
    field: &'static str,
    value: &str,
    source: DecimalError,
) -> EngineError {
    EngineError::ScaleOutOfRange {
        instrument: row.instrument_id.0,
        symbol: row.venue_symbol.clone(),
        field,
        value: value.into(),
        source,
    }
}

fn scale_not_positive(row: &InstrumentRow, field: &'static str, value: &str) -> EngineError {
    EngineError::ScaleNotPositive {
        instrument: row.instrument_id.0,
        symbol: row.venue_symbol.clone(),
        field,
        value: value.into(),
    }
}

// Takes the parsed value, not the venue's text: a limit that refuses here has already survived the
// parse, so the message carries the number the engine would have acted on.
fn scale_limit_not_positive(
    row: &InstrumentRow,
    field: &'static str,
    value: String,
) -> EngineError {
    EngineError::ScaleLimitNotPositive {
        instrument: row.instrument_id.0,
        symbol: row.venue_symbol.clone(),
        field,
        value: value.into(),
    }
}

/// Resolve current+next btc-updown-5m via Gamma, confirm accepted tick, stamp onto Polymarket rows.
/// No Polymarket source -> return immediately. Failure refuses start. Stamp is one-shot; mid-run
/// tick changes NOT re-stamped. Price off grid renders invalid, not silently rounded.
pub(super) async fn preflight_poly(registry: &mut Registry) -> Result<(), EngineError> {
    let Some(series) = poly_series(registry) else {
        return Ok(());
    };
    let rest = PolyRest::new(series).map_err(|error| classify_poly_resolve(series, error))?;
    let now = EngineClock::start().now();
    let (current, next) = rest
        .resolve_current_and_next(now)
        .await
        .map_err(|error| classify_poly_resolve(series, error))?;
    check_poly_market(&current)?;
    check_poly_market(&next)?;
    stamp_poly_scales(registry, &current);
    info!(
        "polymarket preflight ok — {} current tick {} next tick {} (current grid stamped on all slots)",
        series.as_str(),
        current.tick_size.to_f64(),
        next.tick_size.to_f64()
    );
    Ok(())
}

/// Stamp the trading grid and order limits onto every Polymarket row. One grid across slots.
///
/// `orderMinSize` lands on `min_qty`, where a floor belongs, and `lot_size` carries the venue's
/// two-decimal share step. It used to carry the floor instead, which left the engine quantising
/// every size to whole multiples of the minimum and reading no floor at all.
pub fn stamp_poly_scales(registry: &mut Registry, market: &GammaMarket) {
    let poly_ids: Vec<InstrumentId> = registry
        .instruments()
        .iter()
        .filter(|row| matches!(row.market, VenueMarket::Polymarket(_)))
        .map(|row| row.instrument_id)
        .collect();
    for id in poly_ids {
        registry.set_scales(id, market.tick_size, POLY_SHARE_STEP);
        registry.set_poly_limits(
            id,
            PolyLimits {
                min_qty: market.min_order_size,
                max_num_orders: POLY_MAX_ORDERS,
                max_price: Price(FIXED_SCALE - market.tick_size.0),
            },
        );
    }
}

/// Confirm resolved market quotes at accepted tick + positive min order size. Pure.
///
/// # Errors
/// [`EngineError::ScalePolyTick`], [`EngineError::ScalePolyMinSize`].
pub fn check_poly_market(market: &GammaMarket) -> Result<(), EngineError> {
    if !POLY_ACCEPTED_TICKS.contains(&market.tick_size) {
        return Err(EngineError::ScalePolyTick {
            symbol: market.slug.clone(),
            expected: POLY_ACCEPTED_TICKS,
            actual: market.tick_size,
        });
    }
    if market.min_order_size.0 <= 0 {
        return Err(EngineError::ScalePolyMinSize {
            symbol: market.slug.clone(),
            value: market.min_order_size,
        });
    }
    Ok(())
}

/// Map Gamma resolve failure: missing market/too-few windows -> series-not-found, else unreachable.
pub fn classify_poly_resolve(series: PolySeries, error: GammaError) -> EngineError {
    match error {
        GammaError::MarketNotFound { .. } | GammaError::FallbackTooFew { .. } => {
            EngineError::ScalePolySeriesUnknown {
                series: series.as_str(),
                source: error,
            }
        }
        other => EngineError::ScalePolyUnreachable {
            series: series.as_str(),
            source: other,
        },
    }
}

/// Polymarket series present in registry (v1 single variant).
fn poly_series(registry: &Registry) -> Option<PolySeries> {
    registry
        .instruments()
        .iter()
        .find_map(|row| match row.market {
            VenueMarket::Polymarket(series) => Some(series),
            VenueMarket::Binance(_) => None,
        })
}
