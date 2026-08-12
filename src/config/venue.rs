//! Venue-scoped market identity: which venue an instrument trades on plus that venue's sub-market
//! discriminant. The seam that lets one frozen registry hold Binance and Polymarket rows without
//! either venue's wire vocabulary leaking into the other.

use super::sources::{BinanceMarket, PolySeries};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VenueMarket {
    Binance(BinanceMarket),
    Polymarket(PolySeries),
}

impl VenueMarket {
    pub fn as_str(self) -> &'static str {
        match self {
            VenueMarket::Binance(market) => market.as_str(),
            VenueMarket::Polymarket(series) => series.as_str(),
        }
    }
}
