//! EM fitter against the simplex MLE on the same window. Two independent optimisers over one
//! likelihood must land on the same parameters; if they diverge, one of them is wrong and neither
//! run reports anything unusual.
//!
//! Per-step likelihood ascent, the property that makes EM EM, cannot be observed from outside the
//! crate: the fitter takes `min_events` rather than an iteration cap, and its step is private. A
//! monotonicity bug that still converged to the same optimum would pass here.

use super::exponential_fixture;
use polysim::hot::quant::hawkes::{HawkesEm, HawkesMle};

#[test]
fn em_agrees_with_the_simplex() {
    let (_, events, now) = exponential_fixture();

    let mut em = HawkesEm::new(64);
    let cold = em.fit(&events, now).expect("cold fit");
    assert!(cold.converged, "em hit its iteration cap");
    let mle = HawkesMle::new(64).fit(&events, now).expect("simplex fit");
    for (fitted, reference, name) in [
        (cold.mu, mle.mu, "mu"),
        (cold.alpha, mle.alpha, "alpha"),
        (cold.beta, mle.beta, "beta"),
    ] {
        assert!(
            (fitted / reference - 1.0).abs() < 0.05,
            "{name}: em {fitted} against simplex {reference}"
        );
    }
    assert!(
        (cold.log_likelihood - mle.log_likelihood).abs() < 1.0,
        "em ll {} against simplex ll {}",
        cold.log_likelihood,
        mle.log_likelihood
    );
    assert!(!em.fit(&events, now).expect("warm refit").is_stale);
}
