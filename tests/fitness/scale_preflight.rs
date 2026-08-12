//! Startup scale check: using the committed exchangeInfo fixtures, it passes a real symbol —
//! pinning the exact `tickSize`/`stepSize` mantissas it returns — and REFUSES one whose price/qty
//! bounds or trading grid are absent, non-positive, or unparseable, so a venue schema rename can
//! never make the fixed-point guard vacuously pass. The Polymarket arm pins the same taxonomy:
//! both venue ticks (0.01 and the 0.001 endgame tick) pass, a drifted tick or absent min size
//! refuses, and a Gamma resolve failure maps to either unreachable or series-not-found.
//! `check_symbol_scale`, `check_poly_market`, `classify_poly_resolve` are pure.

use polysim::adapters::binance::rest::{ExchangeInfo, SymbolFilter, SymbolInfo};
use polysim::adapters::polymarket::rest::{GammaError, GammaMarket};
use polysim::config::{BinanceEnv, BinanceMarket, PolySeries, VenueMarket};
use polysim::hot::exec::{MAX_ORDER_BUDGET_WINDOWS, OrderBudgetWindow};
use polysim::ids::{AssetId, Price, Qty};
use polysim::registry::{InstrumentRow, OrderRateLimit, RateInterval, Registry};
use polysim::runtime::{
    EngineError, check_order_rate_limits, check_poly_market, check_symbol_order_capacity,
    check_symbol_scale, classify_poly_resolve, order_budget, stamp_poly_scales,
};
use polysim::time::{DurationUs, TsUs};

use crate::engine_support::{ONE, instrument_row, tracker_spec_all};

const PERP_EXCHANGE_INFO: &str = include_str!("../../fixtures/binance/perp_exchange_info.json");
const SPOT_EXCHANGE_INFO: &str = include_str!("../../fixtures/binance/spot_exchange_info.json");

fn perp_info() -> ExchangeInfo {
    serde_json::from_str(PERP_EXCHANGE_INFO).expect("parse committed perp exchangeInfo fixture")
}

fn spot_info() -> ExchangeInfo {
    serde_json::from_str(SPOT_EXCHANGE_INFO).expect("parse committed spot exchangeInfo fixture")
}

/// The fixture's instrument is BTCUSDT perp; `instrument_row` builds exactly that venue symbol.
fn btc_perp() -> InstrumentRow {
    instrument_row(0, tracker_spec_all(1), 64)
}

/// The same BTCUSDT symbol declared spot. The two markets publish the same limits under different
/// filter and field names, so the market ON THE ROW decides which names the preflight reads — a
/// spot row against the perp fixture (or the reverse) must not silently half-validate.
fn btc_spot() -> InstrumentRow {
    InstrumentRow {
        market: VenueMarket::Binance(BinanceMarket::Spot),
        ..btc_perp()
    }
}

#[test]
fn passes_real_bounds_and_refuses_when_any_bound_is_absent() {
    // The perp fixture quotes tickSize "0.10" and stepSize "0.001" — exact at the 1e-8 scale.
    let scales = check_symbol_scale(&btc_perp(), &perp_info()).expect("real BTCUSDT bounds fit");
    assert_eq!(scales.tick_size, Price(10_000_000));
    assert_eq!(scales.step_size, Qty(100_000));

    let no_price = strip_filter(perp_info(), "PRICE_FILTER");
    match check_symbol_scale(&btc_perp(), &no_price).expect_err("absent maxPrice must refuse") {
        EngineError::ScaleFieldMissing { field, .. } => assert_eq!(field, "PRICE_FILTER.maxPrice"),
        other => panic!("expected ScaleFieldMissing, got {other:?}"),
    }

    let no_lot = strip_filter(perp_info(), "LOT_SIZE");
    match check_symbol_scale(&btc_perp(), &no_lot).expect_err("absent maxQty must refuse") {
        EngineError::ScaleFieldMissing { field, .. } => assert_eq!(field, "LOT_SIZE.maxQty"),
        other => panic!("expected ScaleFieldMissing, got {other:?}"),
    }

    let empty = ExchangeInfo {
        rate_limits: Vec::new(),
        symbols: Vec::new(),
    };
    let error = check_symbol_scale(&btc_perp(), &empty).expect_err("symbol not listed must refuse");
    assert!(matches!(error, EngineError::ScaleSymbolUnknown { .. }));
}

/// The venue's grid, order floors and counts, read from the real spot payload. Without them the
/// engine builds orders it believes are legal and the venue rejects — a failure that surfaces as an
/// unexplained rejection at runtime rather than a refusal to start, so the exact mantissas are
/// pinned here.
#[test]
fn spot_order_limits_parse_from_the_real_fixture() {
    let scales = check_symbol_scale(&btc_spot(), &spot_info())
        .expect("real spot BTCUSDT grid and limits parse");

    // The spot fixture writes the grid in trailing-zero form ("0.01000000" / "0.00001000"); its
    // symbol is BTCUSDT, which `btc_spot`'s venue symbol matches case-insensitively.
    assert_eq!(scales.tick_size, Price(1_000_000));
    assert_eq!(scales.step_size, Qty(1_000));

    let limits = scales.limits;
    // LOT_SIZE.minQty "0.00001000" — NOT MARKET_LOT_SIZE.minQty, which the same fixture quotes as
    // "0.00000000"; reading the wrong filter yields a floor of zero, i.e. no floor at all.
    assert_eq!(limits.min_qty, Qty(1_000));
    // NOTIONAL.minNotional "5.00000000" as an exact 1e-8 quote mantissa.
    assert_eq!(limits.min_notional, 500_000_000);
    assert_eq!(limits.max_num_orders, 200);
    assert_eq!(limits.max_num_order_amends, Some(10));
}

/// Futures publishes the same limits under `MIN_NOTIONAL.notional` and `MAX_NUM_ORDERS.limit`, and
/// publishes no amend-count filter at all. Reading spot's names here would refuse a valid symbol
/// and refuse the whole run with it.
#[test]
fn perp_order_limits_read_the_futures_filter_schema() {
    let limits = check_symbol_scale(&btc_perp(), &perp_info())
        .expect("real perp BTCUSDT limits parse")
        .limits;

    assert_eq!(limits.min_qty, Qty(100_000), "LOT_SIZE.minQty 0.001");
    assert_eq!(limits.min_notional, 50 * ONE, "MIN_NOTIONAL.notional 50");
    assert_eq!(limits.max_num_orders, 200, "MAX_NUM_ORDERS.limit 200");
    assert_eq!(
        limits.max_num_order_amends, None,
        "futures publishes no MAX_NUM_ORDER_AMENDS filter — None is its schema, not an unread field"
    );

    // The mirror of the same claim: spot's names must not resolve against a futures payload.
    match check_symbol_scale(&btc_spot(), &perp_info())
        .expect_err("a spot row must not half-validate against futures filters")
    {
        EngineError::ScaleFieldMissing { field, .. } => assert_eq!(field, "NOTIONAL.minNotional"),
        other => panic!("expected ScaleFieldMissing, got {other:?}"),
    }
}

#[test]
fn configured_two_sided_ladder_must_fit_the_symbol_order_limit() {
    let mut row = btc_spot();
    row.max_num_orders = Some(16);
    check_symbol_order_capacity(&row, 8).expect("eight orders on both sides exactly fit sixteen");

    row.max_num_orders = Some(15);
    match check_symbol_order_capacity(&row, 8)
        .expect_err("the requested steady-state ladder cannot fit")
    {
        EngineError::ExecutionOrderCapacity {
            configured_per_side,
            required_total,
            venue_max,
            ..
        } => {
            assert_eq!(configured_per_side, 8);
            assert_eq!(required_total, 16);
            assert_eq!(venue_max, 15);
        }
        other => panic!("expected ExecutionOrderCapacity, got {other:?}"),
    }
}

/// Every limit is refused when absent or non-positive. A zero here is the dangerous case: it reads
/// as "the venue has no floor", so the engine would size orders below what can ever fill.
#[test]
fn refuses_absent_or_non_positive_order_limits() {
    let no_notional = strip_filter(spot_info(), "NOTIONAL");
    match check_symbol_scale(&btc_spot(), &no_notional)
        .expect_err("an absent NOTIONAL filter must refuse, not default to zero")
    {
        EngineError::ScaleFieldMissing { field, .. } => assert_eq!(field, "NOTIONAL.minNotional"),
        other => panic!("expected ScaleFieldMissing, got {other:?}"),
    }

    let zero_notional = set_field(spot_info(), "NOTIONAL", |filter| {
        filter.min_notional = Some("0.00000000".into())
    });
    match check_symbol_scale(&btc_spot(), &zero_notional)
        .expect_err("a zero minNotional must refuse")
    {
        EngineError::ScaleLimitNotPositive { field, value, .. } => {
            assert_eq!(field, "minNotional");
            assert_eq!(value.as_ref(), "0");
        }
        other => panic!("expected ScaleLimitNotPositive, got {other:?}"),
    }

    let zero_min_qty = set_field(spot_info(), "LOT_SIZE", |filter| {
        filter.min_qty = Some("0.00000000".into())
    });
    match check_symbol_scale(&btc_spot(), &zero_min_qty).expect_err("a zero minQty must refuse") {
        EngineError::ScaleLimitNotPositive { field, .. } => assert_eq!(field, "minQty"),
        other => panic!("expected ScaleLimitNotPositive, got {other:?}"),
    }

    let no_amends = strip_filter(spot_info(), "MAX_NUM_ORDER_AMENDS");
    match check_symbol_scale(&btc_spot(), &no_amends)
        .expect_err("spot losing its amend filter must refuse")
    {
        EngineError::ScaleFieldMissing { field, .. } => {
            assert_eq!(field, "MAX_NUM_ORDER_AMENDS.maxNumOrderAmends")
        }
        other => panic!("expected ScaleFieldMissing, got {other:?}"),
    }

    let zero_orders = set_field(spot_info(), "MAX_NUM_ORDERS", |filter| {
        filter.max_num_orders = Some(0)
    });
    match check_symbol_scale(&btc_spot(), &zero_orders)
        .expect_err("a zero order cap must refuse — no order could ever rest")
    {
        EngineError::ScaleLimitNotPositive { field, .. } => assert_eq!(field, "maxNumOrders"),
        other => panic!("expected ScaleLimitNotPositive, got {other:?}"),
    }
}

/// `MARKET_LOT_SIZE` mirrors every `LOT_SIZE` field under a different filter type, and spot quotes
/// its `stepSize`/`minQty` as `0.00000000`. Falling back to it would hand the engine a zero grid and
/// a zero floor — silently, since both parse fine. So with `LOT_SIZE` gone the check must refuse
/// even though the mirror is still there.
#[test]
fn market_lot_size_is_never_mistaken_for_lot_size() {
    let mirror_only = strip_filter(spot_info(), "LOT_SIZE");
    assert!(
        mirror_only.symbols[0]
            .filters
            .iter()
            .any(|filter| filter.filter_type.as_ref() == "MARKET_LOT_SIZE"),
        "the mirror filter must survive, or this test proves nothing"
    );
    match check_symbol_scale(&btc_spot(), &mirror_only)
        .expect_err("MARKET_LOT_SIZE must not stand in for LOT_SIZE")
    {
        EngineError::ScaleFieldMissing { field, .. } => assert_eq!(field, "LOT_SIZE.maxQty"),
        other => panic!("expected ScaleFieldMissing, got {other:?}"),
    }
}

#[test]
fn refuses_absent_zero_or_unparseable_grid() {
    let no_tick = set_field(perp_info(), "PRICE_FILTER", |filter| {
        filter.tick_size = None
    });
    match check_symbol_scale(&btc_perp(), &no_tick).expect_err("absent tickSize must refuse") {
        EngineError::ScaleFieldMissing { field, .. } => assert_eq!(field, "PRICE_FILTER.tickSize"),
        other => panic!("expected ScaleFieldMissing, got {other:?}"),
    }

    let no_step = set_field(perp_info(), "LOT_SIZE", |filter| filter.step_size = None);
    match check_symbol_scale(&btc_perp(), &no_step).expect_err("absent stepSize must refuse") {
        EngineError::ScaleFieldMissing { field, .. } => assert_eq!(field, "LOT_SIZE.stepSize"),
        other => panic!("expected ScaleFieldMissing, got {other:?}"),
    }

    let zero_tick = set_field(perp_info(), "PRICE_FILTER", |filter| {
        filter.tick_size = Some("0.00".into())
    });
    match check_symbol_scale(&btc_perp(), &zero_tick).expect_err("a zero tick must refuse") {
        EngineError::ScaleNotPositive { field, .. } => assert_eq!(field, "tickSize"),
        other => panic!("expected ScaleNotPositive, got {other:?}"),
    }

    let zero_step = set_field(perp_info(), "LOT_SIZE", |filter| {
        filter.step_size = Some("0.00".into())
    });
    match check_symbol_scale(&btc_perp(), &zero_step).expect_err("a zero step must refuse") {
        EngineError::ScaleNotPositive { field, .. } => assert_eq!(field, "stepSize"),
        other => panic!("expected ScaleNotPositive, got {other:?}"),
    }

    let bad_tick = set_field(perp_info(), "PRICE_FILTER", |filter| {
        filter.tick_size = Some("abc".into())
    });
    match check_symbol_scale(&btc_perp(), &bad_tick).expect_err("an unparseable tick must refuse") {
        EngineError::ScaleOutOfRange { field, .. } => assert_eq!(field, "tickSize"),
        other => panic!("expected ScaleOutOfRange, got {other:?}"),
    }
}

fn strip_filter(mut info: ExchangeInfo, filter_type: &str) -> ExchangeInfo {
    for symbol in &mut info.symbols {
        symbol
            .filters
            .retain(|filter| filter.filter_type.as_ref() != filter_type);
    }
    // Guard the test itself: the fixture must actually contain the symbol we mutate.
    assert!(
        info.symbols
            .iter()
            .any(|s: &SymbolInfo| !s.filters.is_empty()),
        "fixture symbol lost all filters — test setup wrong"
    );
    info
}

/// Mutate one field on the named filter across every symbol — targets the tickSize/stepSize path
/// specifically, leaving the maxPrice/maxQty checks it follows intact so the refusal is the grid's.
fn set_field(
    mut info: ExchangeInfo,
    filter_type: &str,
    edit: impl Fn(&mut SymbolFilter),
) -> ExchangeInfo {
    let mut edited = false;
    for symbol in &mut info.symbols {
        for filter in &mut symbol.filters {
            if filter.filter_type.as_ref() == filter_type {
                edit(filter);
                edited = true;
            }
        }
    }
    assert!(
        edited,
        "fixture has no {filter_type} filter — test setup wrong"
    );
    info
}

/// A resolved market with the given tick and min size; other fields are grid-consistent filler.
pub(crate) fn poly_market(tick_size: Price, min_order_size: Qty) -> GammaMarket {
    GammaMarket {
        slug: "btc-updown-5m-1784439600".into(),
        condition_id: "0xabc".into(),
        token_up: "111".into(),
        token_down: "222".into(),
        tick_size,
        min_order_size,
        window_open_ts_us: TsUs::from_micros(1_784_439_600_000_000),
        window_close_ts_us: TsUs::from_micros(1_784_439_900_000_000),
    }
}

#[test]
fn poly_preflight_accepts_both_venue_ticks_and_refuses_drift() {
    // 0.01 at the 1e-8 scale = 1_000_000 — the tick book_capacity 128 and the 0..1 scale assume.
    check_poly_market(&poly_market(Price(1_000_000), Qty(500_000_000)))
        .expect("the 0.01 design tick with a positive min size passes");

    // 0.001 = 100_000: the venue's endgame tick near the 0/1 bounds must not refuse a boot.
    check_poly_market(&poly_market(Price(100_000), Qty(500_000_000)))
        .expect("the 0.001 endgame tick passes");

    assert!(
        matches!(
            check_poly_market(&poly_market(Price(500_000), Qty(500_000_000)))
                .expect_err("a 0.005 tick must refuse the start"),
            EngineError::ScalePolyTick { .. }
        ),
        "a tick outside the accepted set refuses",
    );

    match check_poly_market(&poly_market(Price(2_000_000), Qty(500_000_000)))
        .expect_err("a 0.02 tick must refuse the start")
    {
        EngineError::ScalePolyTick {
            actual, expected, ..
        } => {
            assert_eq!(actual, Price(2_000_000));
            assert_eq!(expected, [Price(1_000_000), Price(100_000)]);
        }
        other => panic!("expected ScalePolyTick, got {other:?}"),
    }

    assert!(
        matches!(
            check_poly_market(&poly_market(Price(1_000_000), Qty(0)))
                .expect_err("an absent orderMinSize must refuse"),
            EngineError::ScalePolyMinSize { .. }
        ),
        "a non-positive min order size refuses",
    );
}

/// A registry carrying one polymarket source, its four slots unstamped as they leave `build`.
pub(crate) fn build_poly_registry() -> Registry {
    let yaml = "engine:
  hot_core_id: 0
  spin_interval_us: 100000
queues:
  input_capacity: 65536
  persistence_capacity: 65536
source:
  exchange: polymarket
  max_exposure_quote: 500
  series: btc-updown-5m
  tracker: {}
strategy:
  instruments: all
persistence:
  dir: ./data
logging:
  dir: ./logs
";
    let config: polysim::config::Config =
        polysim::config::Config::from_yaml(yaml).expect("poly config parses and validates");
    Registry::build(&config).expect("registry builds")
}

/// After an accepted poly preflight the current window's grid is stamped onto every slot — the same
/// [`polysim::registry::Registry::set_scales`] mechanism binance uses — so downstream code
/// quantises poly prices against a real tick and lot.
#[test]
fn poly_preflight_stamps_current_grid_onto_all_slots() {
    let mut registry = build_poly_registry();
    assert!(
        registry
            .instruments()
            .iter()
            .all(|row| row.tick_size.is_none() && row.lot_size.is_none()),
        "poly rows leave build unstamped"
    );

    // The 0.01 design tick and a 5.0 min order size, both exact at the 1e-8 scale.
    let current = poly_market(Price(1_000_000), Qty(500_000_000));
    stamp_poly_scales(&mut registry, &current);

    let poly_rows: Vec<&InstrumentRow> = registry
        .instruments()
        .iter()
        .filter(|row| matches!(row.market, VenueMarket::Polymarket(_)))
        .collect();
    assert_eq!(poly_rows.len(), 4, "the series fans out to four slots");
    for row in poly_rows {
        assert_eq!(row.tick_size, Some(Price(1_000_000)), "slot tick stamped");
        // The venue's two-decimal share step, NOT the 5.0 minimum: a floor stamped as a step
        // quantises every order to whole multiples of the minimum. `poly_exec_rows` pins where the
        // minimum went and what else the same pass stamps.
        assert_eq!(row.lot_size, Some(Qty(1_000_000)), "slot step stamped");
        assert_eq!(row.min_qty, Some(Qty(500_000_000)), "slot floor stamped");
    }
}

/// A registry carrying one binance spot source, BTC/USDT. `env_line` goes into the source block, so
/// a caller can pin what an omitted `env:` means as well as an explicit one.
fn build_binance_registry(env_line: &str) -> Registry {
    let yaml = format!(
        "engine:
  hot_core_id: 0
  spin_interval_us: 100000
queues:
  input_capacity: 65536
  persistence_capacity: 65536
source:
  exchange: binance
  market: spot
{env_line}  base: BTC
  quote: USDT
  max_exposure_quote: 500
  tracker: {{}}
strategy:
  instruments: all
persistence:
  dir: ./data
logging:
  dir: ./logs
"
    );
    let config: polysim::config::Config =
        polysim::config::Config::from_yaml(&yaml).expect("binance config parses and validates");
    Registry::build(&config).expect("registry builds")
}

/// FITNESS: the buckets the venue published are the ones the engine paces against. The parse and
/// the gate are two halves of one promise, and a payload read correctly into a budget nobody carries
/// is the shape this whole seam was missing — every window has to survive the crossing, at its own
/// length and its own cap.
///
/// Both markets publish the budget under the same `rateLimitType` but over different windows, and it
/// is ACCOUNT-scoped, so it lands on the registry rather than being copied onto every instrument row
/// — a two-symbol engine reading it per row would believe it had twice the budget.
#[test]
fn every_parsed_bucket_reaches_the_budget_the_engine_paces_against() {
    let spot = order_budget(
        &check_order_rate_limits(BinanceMarket::Spot, &spot_info()).expect("spot buckets parse"),
    )
    .expect("two buckets are inside the model");
    assert_eq!(
        spot.windows(),
        [
            OrderBudgetWindow {
                window: DurationUs::from_secs(10),
                max_places: 100,
            },
            OrderBudgetWindow {
                window: DurationUs::from_secs(86_400),
                max_places: 200_000,
            },
        ],
        "spot grants 100 placements per 10 seconds and 200000 a day"
    );

    let perp = order_budget(
        &check_order_rate_limits(BinanceMarket::Perpetual, &perp_info())
            .expect("perp buckets parse"),
    )
    .expect("two buckets are inside the model");
    assert_eq!(
        perp.windows(),
        [
            OrderBudgetWindow {
                window: DurationUs::from_secs(60),
                max_places: 1_200,
            },
            OrderBudgetWindow {
                window: DurationUs::from_secs(10),
                max_places: 300,
            },
        ],
        "the perp windows are different lengths from spot's, and a budget that carried spot's would \
         pace against a grant this market never made"
    );
}

/// FITNESS: a venue declaring more buckets than the engine models refuses the run. Keeping a subset
/// would pace against a budget larger than the granted one, which is the same silent over-admission
/// the gate exists to prevent — only harder to see, because every remaining window would look right.
#[test]
fn more_buckets_than_the_engine_models_refuses_the_run() {
    let bucket = OrderRateLimit {
        interval: RateInterval::Second,
        interval_num: 10,
        limit: 100,
    };
    let modelled = vec![bucket; MAX_ORDER_BUDGET_WINDOWS];
    assert!(
        order_budget(&modelled).is_ok(),
        "the model's own capacity must be accepted, or the case below is refusing everything"
    );

    let mut beyond = modelled;
    beyond.push(bucket);
    match order_budget(&beyond).expect_err("one bucket past the model refuses") {
        EngineError::ExecutionTooManyOrderWindows { found, max } => {
            assert_eq!(
                (found, max),
                (MAX_ORDER_BUDGET_WINDOWS + 1, MAX_ORDER_BUDGET_WINDOWS)
            );
        }
        other => panic!("wrong refusal: {other:?}"),
    }
}

/// The REQUEST_WEIGHT and RAW_REQUESTS buckets ride the same array and are not an order budget.
/// Counting one would let the engine believe it may send 6000 orders a minute.
#[test]
fn only_orders_buckets_are_read_as_an_order_budget() {
    let spot_info = spot_info();
    assert!(
        spot_info
            .rate_limits
            .iter()
            .any(|entry| entry.rate_limit_type.as_ref() != "ORDERS"),
        "the fixture must carry non-ORDERS buckets, or this test proves nothing"
    );
    let read =
        check_order_rate_limits(BinanceMarket::Spot, &spot_info).expect("spot buckets parse");
    assert_eq!(read.len(), 2, "only the two ORDERS buckets are read");
}

/// Every way the budget could be read wrong refuses the start instead. Each of these fails SILENTLY
/// if tolerated: a dropped bucket leaves the engine pacing against a larger budget than it has.
#[test]
fn refuses_an_unreadable_or_absent_order_budget() {
    let mut none = spot_info();
    none.rate_limits
        .retain(|entry| entry.rate_limit_type.as_ref() != "ORDERS");
    assert!(
        matches!(
            check_order_rate_limits(BinanceMarket::Spot, &none)
                .expect_err("no ORDERS bucket must refuse"),
            EngineError::ScaleOrderLimitsMissing { .. }
        ),
        "a payload with no ORDERS bucket refuses rather than pacing blind",
    );

    let mut unknown_interval = spot_info();
    for entry in &mut unknown_interval.rate_limits {
        if entry.rate_limit_type.as_ref() == "ORDERS" {
            entry.interval = "FORTNIGHT".into();
        }
    }
    match check_order_rate_limits(BinanceMarket::Spot, &unknown_interval)
        .expect_err("an unrecognised interval must refuse")
    {
        EngineError::ScaleRateLimitUnreadable { field, value, .. } => {
            assert_eq!(field, "interval");
            assert_eq!(value.as_ref(), "FORTNIGHT");
        }
        other => panic!("expected ScaleRateLimitUnreadable, got {other:?}"),
    }

    let mut zero_limit = spot_info();
    for entry in &mut zero_limit.rate_limits {
        if entry.rate_limit_type.as_ref() == "ORDERS" {
            entry.limit = 0;
        }
    }
    match check_order_rate_limits(BinanceMarket::Spot, &zero_limit)
        .expect_err("a zero order budget must refuse")
    {
        EngineError::ScaleRateLimitUnreadable { field, .. } => assert_eq!(field, "limit"),
        other => panic!("expected ScaleRateLimitUnreadable, got {other:?}"),
    }

    let mut zero_interval_num = spot_info();
    for entry in &mut zero_interval_num.rate_limits {
        if entry.rate_limit_type.as_ref() == "ORDERS" {
            entry.interval_num = 0;
        }
    }
    match check_order_rate_limits(BinanceMarket::Spot, &zero_interval_num)
        .expect_err("a zero intervalNum must refuse")
    {
        EngineError::ScaleRateLimitUnreadable { field, .. } => assert_eq!(field, "intervalNum"),
        other => panic!("expected ScaleRateLimitUnreadable, got {other:?}"),
    }
}

/// One deployment choice for the whole run. The preflight and the market-data adapters both read it
/// here, so the engine cannot validate production's grid and then connect to testnet's hosts —
/// which is silent: both host sets answer, with different tick sizes and a different account.
#[test]
fn the_deployment_comes_from_config_and_defaults_to_production() {
    assert_eq!(
        build_binance_registry("").binance_env(),
        Some(BinanceEnv::Production),
        "an omitted env: keeps every config written before the field existed on production"
    );
    assert_eq!(
        build_binance_registry("  env: testnet\n").binance_env(),
        Some(BinanceEnv::Testnet)
    );
    assert_eq!(
        build_binance_registry("  env: production\n").binance_env(),
        Some(BinanceEnv::Production)
    );
    assert_eq!(
        build_poly_registry().binance_env(),
        None,
        "the deployment choice is binance's — a polymarket source has none"
    );
}

/// An [`AssetId`] is the identity the edge resolves a wire asset STRING into once, so nothing inward
/// of it compares text. Two builds of the same config must agree: an id recorded on one run is
/// read back against a registry rebuilt from that config, and a shifted assignment would silently
/// rename the asset it points at.
#[test]
fn asset_dictionary_interns_every_leg_and_is_stable_across_builds() {
    let first = build_binance_registry("");
    let row = &first.instruments()[0];
    assert_eq!(first.assets().name(row.base_asset), Some("BTC"));
    assert_eq!(first.assets().name(row.quote_asset), Some("USDT"));
    assert_ne!(row.base_asset, row.quote_asset, "the legs are distinct");
    assert_eq!(first.assets().len(), 2, "ids are dense 0..len");

    let second = build_binance_registry("");
    assert_eq!(second.instruments()[0].base_asset, row.base_asset);
    assert_eq!(second.instruments()[0].quote_asset, row.quote_asset);

    // How a venue cases its asset codes is its choice; an asset NO instrument names must resolve to
    // the sentinel, so its balance is counted and ignored rather than landing on a real one.
    assert_eq!(first.assets().id("usdt"), row.quote_asset);
    assert_eq!(first.assets().id("BNB"), AssetId::UNKNOWN);
    assert_eq!(first.assets().name(AssetId::UNKNOWN), None);

    // Four polymarket slots are four DIFFERENT conditional tokens over one shared collateral, so
    // the dictionary holds a base per slot plus one quote. A shared base would have every leg's
    // share balance read as every other leg's, which the sell-side funds gate spends against.
    let poly = build_poly_registry();
    assert_eq!(poly.assets().len(), 5);
    let legs: Vec<(AssetId, AssetId)> = poly
        .instruments()
        .iter()
        .map(|slot| (slot.base_asset, slot.quote_asset))
        .collect();
    let quotes: Vec<AssetId> = legs.iter().map(|(_, quote)| *quote).collect();
    assert_eq!(
        quotes,
        vec![quotes[0]; 4],
        "one collateral across the series"
    );
    let mut bases: Vec<AssetId> = legs.iter().map(|(base, _)| *base).collect();
    bases.sort_by_key(|asset| asset.0);
    bases.dedup();
    assert_eq!(bases.len(), 4, "each slot's shares are its own asset");
    for slot in poly.instruments() {
        assert_eq!(
            poly.assets().name(slot.base_asset),
            Some(&*slot.venue_symbol),
            "a leg's asset is named for the leg, so a balance can be routed back to it"
        );
    }
}

#[test]
fn poly_resolve_errors_map_to_the_preflight_taxonomy() {
    // A missing market or a short fallback both mean the series did not resolve.
    for not_found in [
        GammaError::MarketNotFound {
            slug: "btc-updown-5m-1784439600".to_owned(),
        },
        GammaError::FallbackTooFew { found: 1 },
    ] {
        assert!(matches!(
            classify_poly_resolve(PolySeries::BtcUpDown5m, not_found),
            EngineError::ScalePolySeriesUnknown { .. }
        ));
    }

    // Anything else (here a 5xx) means the venue could not be reached to validate.
    let unreachable = classify_poly_resolve(
        PolySeries::BtcUpDown5m,
        GammaError::Status {
            url: "https://gamma-api.polymarket.com/events".to_owned(),
            status: 503,
        },
    );
    assert!(matches!(
        unreachable,
        EngineError::ScalePolyUnreachable { .. }
    ));
}
