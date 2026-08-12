//! Volatility: [`VolSeries`] toolkit + [`EwmaVol`] streaming resident. Pure functions of input.

mod egarch;
mod ewma;
mod realised;

pub use egarch::{Egarch, EgarchEstimate};
pub(crate) use ewma::EwmaVol;
pub use realised::{Returns, realised_vol_per_sec};

use crate::hot::series::FastQueue;
use crate::time::DurationUs;
use ewma::ewma_vol_over;

/// Volatility estimators over [`FastQueue<f64>`] window. Implemented only for f64 (one-way dep).
pub trait VolSeries {
    fn egarch(&self, state: &mut Egarch) -> Option<EgarchEstimate>;
    fn realised_volatility(&self, returns: Returns, interval: DurationUs) -> Option<f64>;
    /// Stateless RiskMetrics EWMA over window.
    fn ewma_volatility(&self, halflife_events: u32) -> Option<f64>;
}

impl VolSeries for FastQueue<f64> {
    fn egarch(&self, state: &mut Egarch) -> Option<EgarchEstimate> {
        state.fit(self.as_slice())
    }

    fn realised_volatility(&self, returns: Returns, interval: DurationUs) -> Option<f64> {
        realised_vol_per_sec(self.iter(), returns, interval)
    }

    fn ewma_volatility(&self, halflife_events: u32) -> Option<f64> {
        ewma_vol_over(self.iter(), halflife_events)
    }
}
