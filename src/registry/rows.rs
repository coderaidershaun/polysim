//! `SourceSpec` -> instrument rows: one venue's config block becomes the dense, frozen rows every
//! later stage addresses by [`InstrumentId`]. Row order fixes those ids, so it is replay-visible.

use crate::config::{
    BinanceEnv, ConfigError, Instruments, PolySeries, PolySubscriptions, SourceSpec, Subscriptions,
    TrackerSpec, VenueMarket,
};
use crate::ids::{AssetId, FIXED_SCALE, InstrumentId, fixed_mantissa};

use super::InstrumentRow;
use super::assets::{AssetDictionary, intern_assets};
use super::validate::{validate_binance_tracker, validate_poly_tracker};

const MAX_INSTRUMENTS: usize = u16::MAX as usize + 1;

// Symbol naming no row = silently void config promise; reject here where config + rows coexist. Case-insensitive.
pub(super) fn check_strategy_instruments(
    filter: &Instruments,
    instruments: &[InstrumentRow],
) -> Result<(), ConfigError> {
    let Instruments::Explicit(symbols) = filter else {
        return Ok(());
    };
    for symbol in symbols {
        let wanted = symbol.to_lowercase();
        if !instruments
            .iter()
            .any(|row| row.venue_symbol.to_lowercase() == wanted)
        {
            return Err(ConfigError::UnknownStrategyInstrument {
                symbol: symbol.clone(),
            });
        }
    }
    Ok(())
}

// Binance: 1 instrument; Polymarket: 4 slots. IDs dense 0..N from row count. Asset IDs stamped end-of-pass.
pub(super) fn build_instrument_rows(
    source: &SourceSpec,
) -> Result<(Vec<InstrumentRow>, AssetDictionary), ConfigError> {
    let mut instruments = Vec::new();
    match source {
        SourceSpec::Binance {
            market,
            env: _,
            base,
            quote,
            subscriptions,
            kline_intervals,
            book_capacity,
            max_exposure_quote,
            tracker,
        } => {
            validate_binance_tracker(tracker, kline_intervals)?;
            let max_exposure_quote = exposure_mantissa(*max_exposure_quote)?;
            let instrument_id = issue_instrument_id(&instruments);
            let venue_symbol =
                format!("{}{}", base.to_lowercase(), quote.to_lowercase()).into_boxed_str();
            let display = format!(
                "{}/{} {}",
                base.to_uppercase(),
                quote.to_uppercase(),
                market.as_str()
            )
            .into_boxed_str();
            instruments.push(InstrumentRow {
                instrument_id,
                market: VenueMarket::Binance(*market),
                venue_symbol,
                display,
                base: base.to_uppercase().into_boxed_str(),
                quote: quote.to_uppercase().into_boxed_str(),
                base_asset: AssetId::UNKNOWN,
                quote_asset: AssetId::UNKNOWN,
                tick_size: None,
                lot_size: None,
                min_qty: None,
                min_notional: None,
                max_num_orders: None,
                max_num_order_amends: None,
                max_price: None,
                price_scale: FIXED_SCALE,
                qty_scale: FIXED_SCALE,
                subscriptions: *subscriptions,
                kline_intervals: kline_intervals.clone(),
                book_capacity: *book_capacity,
                max_exposure_quote,
                tracker: tracker.clone(),
            });
        }
        SourceSpec::Polymarket {
            series,
            subscriptions,
            book_capacity,
            max_exposure_quote,
            tracker,
        } => {
            validate_poly_subscriptions(subscriptions)?;
            validate_poly_tracker(tracker)?;
            let spec = PolyRowSpec {
                series: *series,
                subscriptions: Subscriptions::from(*subscriptions),
                book_capacity: *book_capacity,
                max_exposure_quote: exposure_mantissa(*max_exposure_quote)?,
                tracker: tracker.clone(),
            };
            for slot_symbol in series.slot_symbols() {
                let instrument_id = issue_instrument_id(&instruments);
                instruments.push(poly_row(instrument_id, slot_symbol, &spec));
            }
        }
    }
    let assets = intern_assets(&mut instruments);
    Ok((instruments, assets))
}

pub(super) fn binance_env(source: &SourceSpec) -> Option<BinanceEnv> {
    match source {
        SourceSpec::Binance { env, .. } => Some(*env),
        SourceSpec::Polymarket { .. } => None,
    }
}

/// Everything the slots of one Polymarket series share; only the symbol differs between them.
struct PolyRowSpec {
    series: PolySeries,
    subscriptions: Subscriptions,
    book_capacity: usize,
    max_exposure_quote: i64,
    tracker: TrackerSpec,
}

// Polymarket: no klines. Tick/lot unset here, stamped by poly preflight before registry freezes (one-shot).
//
// The base asset is the LEG, not the underlying. Every slot here is a distinct conditional token
// with its own share balance, and a shared "BTC" would have all four legs reading one balance —
// the sell-side funds gate would then see another leg's inventory as this one's, or zero forever.
fn poly_row(
    instrument_id: InstrumentId,
    venue_symbol: Box<str>,
    spec: &PolyRowSpec,
) -> InstrumentRow {
    let display = format!("polymarket {venue_symbol}").into_boxed_str();
    InstrumentRow {
        instrument_id,
        market: VenueMarket::Polymarket(spec.series),
        base: venue_symbol.clone(),
        venue_symbol,
        display,
        quote: "USD".into(),
        base_asset: AssetId::UNKNOWN,
        quote_asset: AssetId::UNKNOWN,
        tick_size: None,
        lot_size: None,
        min_qty: None,
        min_notional: None,
        max_num_orders: None,
        max_num_order_amends: None,
        max_price: None,
        price_scale: FIXED_SCALE,
        qty_scale: FIXED_SCALE,
        subscriptions: spec.subscriptions,
        kline_intervals: Vec::new(),
        book_capacity: spec.book_capacity,
        max_exposure_quote: spec.max_exposure_quote,
        tracker: spec.tracker.clone(),
    }
}

fn exposure_mantissa(quote_units: f64) -> Result<i64, ConfigError> {
    fixed_mantissa(quote_units)
        .filter(|_| quote_units > 0.0)
        .ok_or(ConfigError::Invalid {
            field: "source.max_exposure_quote",
            value: quote_units.to_string().into(),
            expected: "positive, finite quote units within i64 1e-8 range",
        })
}

// u16 id space unreachable by config; only row-building bug can violate.
fn issue_instrument_id(instruments: &[InstrumentRow]) -> InstrumentId {
    debug_assert!(
        instruments.len() < MAX_INSTRUMENTS,
        "{} instrument rows from one source, max {MAX_INSTRUMENTS}",
        instruments.len() + 1
    );
    InstrumentId(instruments.len() as u16)
}

// Polymarket single combined channel; partial subscriptions = silently void. Mixed rejects.
fn validate_poly_subscriptions(subscriptions: &PolySubscriptions) -> Result<(), ConfigError> {
    let false_flags: Vec<&str> = [
        ("trades", subscriptions.trades),
        ("book_updates", subscriptions.book_updates),
        ("book_snapshots", subscriptions.book_snapshots),
    ]
    .iter()
    .filter(|(_, enabled)| !enabled)
    .map(|(name, _)| *name)
    .collect();
    if false_flags.is_empty() || false_flags.len() == 3 {
        return Ok(());
    }
    let value = false_flags
        .iter()
        .map(|name| format!("{name}: false"))
        .collect::<Vec<_>>()
        .join(", ");
    Err(ConfigError::Invalid {
        field: "source.subscriptions",
        value: value.into(),
        expected: "all flags equal — polymarket's single combined channel delivers trades and books together, so v1 cannot honour a partial subscription",
    })
}
