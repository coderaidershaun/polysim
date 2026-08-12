//! The simulated venue's Binance-shaped payloads. Production mints every one of them through the
//! timestamped wire, so that is the only path on which the book-versus-wallet check can protect a
//! run: a fill report whose settlement totals disagree with the order it names is money invented
//! between the matching model and the wallet.

use polysim::adapters::exchange_sim::core::orders::SimOrder;
use polysim::adapters::exchange_sim::core::wallet::FillSettlement;
use polysim::adapters::exchange_sim::wire::{SimFill, VenueWire};
use polysim::adapters::exec::{EngineIdentity, TeTag};
use polysim::config::RunIdentity;
use polysim::ids::{AssetId, ClientOrderId, FIXED_SCALE, Price, Qty, Side, TradeId, VenueOrderId};
use polysim::time::TsUs;

const REPORTED_AT: TsUs = TsUs::from_micros(1_785_000_123_456_789);
const TRADE_ID: TradeId = TradeId(778_291);

#[test]
fn a_settlement_agreeing_with_the_order_mints_a_partial_fill_report() {
    let order = half_filled_order();
    let settlement = settlement_of(&order);
    let report = wire().at(REPORTED_AT).trade_report(
        &order,
        SimFill {
            trade_id: TRADE_ID,
            settlement: &settlement,
            fee_asset: "",
        },
    );

    assert!(report.contains("PARTIALLY_FILLED"), "{report}");
    assert!(
        report.contains(&format!("\"t\":{}", TRADE_ID.0)),
        "the report names its trade: {report}"
    );
}

#[test]
#[should_panic(expected = "the venue's book and its wallet disagree")]
fn a_settlement_disagreeing_with_the_order_stops_the_trade_report() {
    let order = half_filled_order();
    let mut settlement = settlement_of(&order);
    settlement.cumulative_qty = Qty(settlement.cumulative_qty.0 + 1);

    wire().at(REPORTED_AT).trade_report(
        &order,
        SimFill {
            trade_id: TRADE_ID,
            settlement: &settlement,
            fee_asset: "",
        },
    );
}

fn wire() -> VenueWire {
    let identity = RunIdentity::new("strat-micro-recorder", "te-binance-spot-btcusdt")
        .expect("the fixture strategy and trading engine ids are well formed");
    VenueWire::new(EngineIdentity {
        te_tag: TeTag::of(&identity),
        run_nonce: 1_785_000_000,
    })
}

fn half_filled_order() -> SimOrder {
    let price = Price(117_999 * FIXED_SCALE);
    let filled = Qty(5_000);
    SimOrder {
        client_id: ClientOrderId(41),
        venue_order_id: VenueOrderId(12_510_053_280),
        side: Side::Buy,
        price,
        qty: Qty(10_000),
        filled,
        filled_quote: price.notional(filled),
    }
}

fn settlement_of(order: &SimOrder) -> FillSettlement {
    FillSettlement {
        last_qty: order.filled,
        last_quote: order.filled_quote,
        cumulative_qty: order.filled,
        cumulative_quote: order.filled_quote,
        debit: order.filled_quote,
        received_gross: order.filled.0,
        received_net: order.filled.0,
        fee: 0,
        fee_asset: AssetId::UNKNOWN,
    }
}
