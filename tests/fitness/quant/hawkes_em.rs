//! EM fitter against the simplex MLE on the same window. Two independent optimisers over one
//! likelihood must land on the same parameters; if they diverge, one of them is wrong and neither
//! run reports anything unusual.
//!
//! NARROWED on relocation (2026-07-28): the inline original also drove 40 `em_step` calls asserting
//! the likelihood never fell — per-step monotonic ascent, the property that makes EM EM. That went
//! with the private `em_step`/`seed`/`exp_nll`, and `HawkesEm::new` takes `min_events` rather than
//! an iteration cap, so ascent is not observable at any granularity from outside the crate. A
//! monotonicity bug that still converged to the same optimum would now pass here.

use polysim::hot::quant::hawkes::{ExpSimulation, HawkesEm, HawkesEvents, HawkesMle, HawkesParams};
use polysim::time::{DurationUs, TsUs};

const START: TsUs = TsUs::from_micros(0);

#[test]
fn em_agrees_with_the_simplex() {
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
    let mut events = HawkesEvents::new(32_768);
    for &stamp in &path {
        events.push(stamp);
    }
    let now = START + horizon;

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
