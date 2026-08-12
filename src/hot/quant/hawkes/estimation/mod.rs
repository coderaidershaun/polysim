//! Fitters: window -> parameters. Caches warm-start simplex (zero-alloc refits), reissues stale estimates
//! below min_events. Split by model: continuous MLE, discrete MLE, univariate EM, multivariate EM.

mod discrete;
mod em;
mod mle;
mod multivariate_em;

pub use discrete::{DiscreteEstimate, DiscreteMle};
pub use em::HawkesEm;
pub use mle::{HawkesEstimate, HawkesMle, LogisticEstimate, LogisticMle};
pub use multivariate_em::{MultivariateEm, MultivariateEstimate};

// EM linear ascent + multivariate crawl -> high cap needed for convergence flag credibility.
const EM_MAX_ITERATIONS: usize = 2000;
const EM_TOLERANCE: f64 = 1e-7;
const EM_EPSILON: f64 = 1e-12;

fn relative_change(previous: f64, current: f64) -> f64 {
    (current - previous).abs() / previous.abs().max(EM_EPSILON)
}

fn bounded(value: f64, bound: (f64, f64)) -> f64 {
    if value.is_finite() {
        value.clamp(bound.0, bound.1)
    } else if value > 0.0 {
        bound.1
    } else {
        bound.0
    }
}

fn penalised(nll: f64) -> f64 {
    if nll.is_finite() { nll } else { f64::MAX }
}
