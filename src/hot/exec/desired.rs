//! Strategy desired quotes per instrument/side. Level-triggered (survives one spin), not per-message (cleared every drain). Expired declaration = cancel. No escape hatch: declare every spin or absent.

use crate::ids::{InstrumentId, Price, Qty, Side};
use crate::msg::exec::OrderStyle;

use super::level::{MAX_QUOTE_LEVELS, QuoteLevel};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DesiredQuote {
    pub price: Price,
    pub qty: Qty,
    /// Only PostOnly admitted. Reject (not downgrade) prevents unwanted position.
    pub style: OrderStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DesiredSide {
    quote: Option<DesiredQuote>,
    declared_at_spin: u64,
}

impl DesiredSide {
    const EMPTY: DesiredSide = DesiredSide {
        quote: None,
        declared_at_spin: 0,
    };
}

pub struct DesiredBook {
    sides: Vec<[[DesiredSide; MAX_QUOTE_LEVELS]; 2]>,
    /// Spin each instrument's flatten was last declared on. Same level-triggered contract as a
    /// quote, and for the same reason: a market order that outlives the intent behind it is one
    /// nobody asked for.
    flatten: Vec<Option<u64>>,
}

impl DesiredBook {
    pub fn new(instrument_count: usize) -> Self {
        Self {
            sides: vec![[[DesiredSide::EMPTY; MAX_QUOTE_LEVELS]; 2]; instrument_count],
            flatten: vec![None; instrument_count],
        }
    }

    #[inline]
    pub fn declare(
        &mut self,
        instrument: InstrumentId,
        side: Side,
        level: QuoteLevel,
        quote: Option<DesiredQuote>,
        spin: u64,
    ) {
        self.sides[usize::from(instrument.0)][side.index()][level.index()] = DesiredSide {
            quote,
            declared_at_spin: spin,
        };
    }

    /// Expiry checked at read-time (not swept) -> stale never observed.
    #[inline]
    pub fn quote(
        &self,
        instrument: InstrumentId,
        side: Side,
        level: QuoteLevel,
        spin: u64,
    ) -> Option<DesiredQuote> {
        let declared = self.sides[usize::from(instrument.0)][side.index()][level.index()];
        (declared.declared_at_spin == spin)
            .then_some(declared.quote)
            .flatten()
    }

    #[inline]
    pub fn declare_flatten(&mut self, instrument: InstrumentId, spin: u64) {
        self.flatten[usize::from(instrument.0)] = Some(spin);
    }

    /// Expiry checked at read-time, exactly as [`Self::quote`] does it.
    #[inline]
    pub fn is_flattening(&self, instrument: InstrumentId, spin: u64) -> bool {
        self.flatten[usize::from(instrument.0)] == Some(spin)
    }

    #[inline]
    pub fn instrument_count(&self) -> usize {
        self.sides.len()
    }
}
