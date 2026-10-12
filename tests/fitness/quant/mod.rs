//! Quant calculator pins: leaf numeric correctness for the estimators feeding the research tape.
//! One module per calculator, each named for the concept rather than the source path.

use polysim::hot::quant::hawkes::{ExpSimulation, HawkesEvents, HawkesParams};
use polysim::time::{DurationUs, TsUs};

mod egarch;
mod gueant;
mod hawkes_discrete;
mod hawkes_discrete_estimation;
mod hawkes_em;
mod hawkes_mle;
mod hawkes_multivariate;
mod hawkes_multivariate_em;
mod hawkes_nonlinear;
mod hawkes_simulation;
mod hawkes_univariate;
mod intensity;
mod kyle_feed;
mod kyles_lambda;
mod markouts;
mod microprice;
mod obi;
mod optimise;
mod orderbook_resilience;
mod vpin;

/// Deterministic U(0,1) draws and Box-Muller normals; no `rand` dependency.
pub(crate) struct Lcg(pub(crate) u64);

impl Lcg {
    pub(crate) fn unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }

    pub(crate) fn next_normal(&mut self) -> f64 {
        let u1 = self.unit().max(1e-12);
        let u2 = self.unit();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

fn ts(secs: f64) -> TsUs {
    TsUs::from_micros((secs * 1e6).round() as i64)
}

const START: TsUs = TsUs::from_micros(0);

fn window(stamps: &[TsUs]) -> HawkesEvents {
    let mut events = HawkesEvents::new(32_768);
    for &stamp in stamps {
        events.push(stamp);
    }
    events
}

/// The exponential path both fitters are scored on: truth, its event window, and the fit time.
fn exponential_fixture() -> (HawkesParams, HawkesEvents, TsUs) {
    let truth = HawkesParams::new(0.5, 0.8, 2.0);
    let horizon = DurationUs::from_secs(6000);
    let path = ExpSimulation {
        params: truth,
        start_ts: START,
        horizon,
        seed: 0xC0FF_EE01,
        max_events: 20_000,
    }
    .run();
    assert!(path.len() > 3000, "thin path: {} events", path.len());
    (truth, window(&path), START + horizon)
}
