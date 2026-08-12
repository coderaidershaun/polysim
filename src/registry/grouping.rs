//! Instruments sharing one venue connection -> one group, one input queue.
//! Binance splits by (market, category); Polymarket on one socket per series.

use crate::config::{BinanceMarket, PolySeries, Subscriptions, VenueMarket};
use crate::ids::{InstrumentId, QueueId, SourceId};
use crate::labelled_enum::labelled_enum;

use super::InstrumentRow;

labelled_enum! {
    /// Binance: 3 categories (one depth carries updates + snapshots). Polymarket: single `Market` channel.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum ConnectionCategory {
        Trades = "trades",
        Depth = "depth",
        Klines = "klines",
        Market = "market",
    }
    pub fn as_str;
}

impl ConnectionCategory {
    fn is_active_for(self, subscriptions: &Subscriptions) -> bool {
        match self {
            ConnectionCategory::Trades => subscriptions.trades,
            ConnectionCategory::Depth => subscriptions.book_updates || subscriptions.book_snapshots,
            ConnectionCategory::Klines => subscriptions.klines,
            ConnectionCategory::Market => {
                subscriptions.trades || subscriptions.book_updates || subscriptions.book_snapshots
            }
        }
    }
}

/// Instruments sharing (market, category) -> one connection + queue. Binance: 3 groups; Poly: 1.
#[derive(Debug, Clone)]
pub struct ProducerGroup {
    pub source_id: SourceId,
    pub queue_id: QueueId,
    pub market: VenueMarket,
    pub category: ConnectionCategory,
    pub instruments: Vec<InstrumentId>,
}

pub(super) fn group_producers(instruments: &[InstrumentRow]) -> Vec<ProducerGroup> {
    let mut groups = Vec::new();
    group_binance(instruments, &mut groups);
    group_polymarket(instruments, &mut groups);
    groups
}

fn group_binance(instruments: &[InstrumentRow], groups: &mut Vec<ProducerGroup>) {
    for binance_market in [BinanceMarket::Spot, BinanceMarket::Perpetual] {
        let market = VenueMarket::Binance(binance_market);
        let members: Vec<&InstrumentRow> = instruments
            .iter()
            .filter(|row| row.market == market)
            .collect();
        if members.is_empty() {
            continue;
        }
        for category in [
            ConnectionCategory::Trades,
            ConnectionCategory::Depth,
            ConnectionCategory::Klines,
        ] {
            let subscribed = subscribed_ids(&members, category);
            if subscribed.is_empty() {
                continue;
            }
            push_group(groups, market, category, subscribed);
        }
    }
}

fn group_polymarket(instruments: &[InstrumentRow], groups: &mut Vec<ProducerGroup>) {
    for series in poly_series_present(instruments) {
        let market = VenueMarket::Polymarket(series);
        let members: Vec<&InstrumentRow> = instruments
            .iter()
            .filter(|row| row.market == market)
            .collect();
        let subscribed = subscribed_ids(&members, ConnectionCategory::Market);
        if subscribed.is_empty() {
            continue;
        }
        push_group(groups, market, ConnectionCategory::Market, subscribed);
    }
}

fn subscribed_ids(members: &[&InstrumentRow], category: ConnectionCategory) -> Vec<InstrumentId> {
    members
        .iter()
        .filter(|row| category.is_active_for(&row.subscriptions))
        .map(|row| row.instrument_id)
        .collect()
}

fn poly_series_present(instruments: &[InstrumentRow]) -> Vec<PolySeries> {
    let mut seen = Vec::new();
    for row in instruments {
        if let VenueMarket::Polymarket(series) = row.market
            && !seen.contains(&series)
        {
            seen.push(series);
        }
    }
    seen
}

fn push_group(
    groups: &mut Vec<ProducerGroup>,
    market: VenueMarket,
    category: ConnectionCategory,
    instruments: Vec<InstrumentId>,
) {
    let ordinal = groups.len();
    groups.push(ProducerGroup {
        source_id: SourceId(ordinal as u16),
        queue_id: QueueId(ordinal as u8),
        market,
        category,
        instruments,
    });
}
