//! Simplex MLE fitters over a simulated path with known truth. A fitter that quietly returns its
//! seed, or a warm start that silently re-does the cold search, costs nothing at run time and
//! everything in the research columns downstream.
//!
//! The logistic fit is checked for finiteness and warm-start behaviour only. Scoring the fitted
//! optimum against the generating parameters on the likelihood needs crate-private helpers, so a
//! fitter that returned a finite but badly-fitted optimum would pass here.

use super::{START, exponential_fixture, window};
use polysim::hot::quant::hawkes::{
    HawkesMle, LogisticMle, LogisticParams, LogisticShape, LogisticSimulation,
};
use polysim::time::DurationUs;

#[test]
fn fit_recovers_exponential_params_and_warm_starts() {
    // Warm refit reduces iterations via seeded simplex.
    let (truth, events, now) = exponential_fixture();
    let mut fitter = HawkesMle::new(64);
    let cold = fitter.fit(&events, now).expect("cold fit");
    assert!(cold.converged, "cold fit hit max_iter");
    for (fitted, expected, name) in [
        (cold.mu, truth.mu, "mu"),
        (cold.alpha, truth.alpha, "alpha"),
        (cold.beta, truth.beta, "beta"),
    ] {
        assert!(
            (fitted / expected - 1.0).abs() < 0.25,
            "{name} {fitted} missed {expected}"
        );
    }
    assert!(cold.is_stationary() && (cold.branching_ratio - 0.4).abs() < 0.15);

    let warm = fitter.fit(&events, now).expect("warm refit");
    assert!(warm.iterations <= cold.iterations);
    assert!(!warm.is_stale);

    let thin = window(&[START, START + DurationUs::from_secs(1)]);
    assert!(fitter.fit(&thin, now).expect("stale reissue").is_stale);
}

#[test]
fn logistic_fit_converges_and_warm_starts() {
    let shape = LogisticShape {
        theta: 3.0,
        delta: 4.0,
    };
    let truth = LogisticParams::new(0.5, 6.0, 2.0, shape);
    assert!(
        truth.is_stationary(),
        "branching {}",
        truth.branching_ratio()
    );
    let horizon = DurationUs::from_secs(900);
    let path = LogisticSimulation {
        params: truth,
        start_ts: START,
        horizon,
        seed: 0xC0FF_EE04,
        max_events: 20_000,
    }
    .run();
    assert!(path.len() < 5000, "path ran away to {} events", path.len());
    let events = window(&path);
    let now = START + horizon;

    let mut fitter = LogisticMle::new(64, shape);
    let cold = fitter.fit(&events, now).expect("cold fit");
    assert!(cold.mu.is_finite() && cold.alpha.is_finite() && cold.beta.is_finite());

    let warm = fitter.fit(&events, now).expect("warm refit");
    assert!(warm.iterations <= cold.iterations && !warm.is_stale);
}
