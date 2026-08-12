//! Microstructure primitives (pure functions, f64 derived statistics only).

mod orderbook_resilience;

pub use orderbook_resilience::{OrderbookResilience, ResilienceSample};

use crate::ids::Price;
use crate::msg::inbound::Level;

/// Size-weighted mid (weighted by opposite side's qty, leans to thinner side; falls back to mid if empty).
#[inline]
pub fn microprice(best_bid: Level, best_ask: Level) -> f64 {
    let bid_qty = best_bid.qty.to_f64();
    let ask_qty = best_ask.qty.to_f64();
    let total_qty = bid_qty + ask_qty;
    if total_qty <= 0.0 {
        return mid(best_bid.price, best_ask.price);
    }
    (best_bid.price.to_f64() * ask_qty + best_ask.price.to_f64() * bid_qty) / total_qty
}

/// Equilibrium = microprice; Π = M + λ(d−s) where λ = spread/(2(d+s)).
/// Deliberately an alias, not a pass-through to delete: the OU resilience model is written in Π, and
/// [`ResilienceSample::equilibrium`] is fed from here, so the name carries the model's own vocabulary.
#[inline]
pub fn orderbook_equilibrium(best_bid: Level, best_ask: Level) -> f64 {
    microprice(best_bid, best_ask)
}

#[inline]
pub fn spread(best_bid: Price, best_ask: Price) -> f64 {
    best_ask.to_f64() - best_bid.to_f64()
}

#[inline]
pub fn mid(best_bid: Price, best_ask: Price) -> f64 {
    (best_bid.to_f64() + best_ask.to_f64()) / 2.0
}

/// Imbalance = (bid_qty - ask_qty) / total, ∈ [-1, 1] (0.0 if empty).
#[inline]
pub fn imbalance(bids: &[Level], asks: &[Level], top_n: usize) -> f64 {
    let bid_qty: f64 = bids
        .iter()
        .take(top_n)
        .map(|level| level.qty.to_f64())
        .sum();
    let ask_qty: f64 = asks
        .iter()
        .take(top_n)
        .map(|level| level.qty.to_f64())
        .sum();
    let total_qty = bid_qty + ask_qty;
    if total_qty <= 0.0 {
        return 0.0;
    }
    (bid_qty - ask_qty) / total_qty
}
