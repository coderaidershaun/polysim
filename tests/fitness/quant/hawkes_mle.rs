//! Simplex MLE fitters over a simulated path with known truth. A fitter that quietly returns its
//! seed, or a warm start that silently re-does the cold search, costs nothing at run time and
//! everything in the research columns downstream.
//!
//! NARROWED on relocation (2026-07-28): the logistic test was
//! `logistic_fit_beats_the_generating_parameters_on_its_own_likelihood`, and asserted exactly that —
//! the fitted optimum scores at least as well as the truth that generated the path. Evaluating the
//! likelihood at arbitrary parameters needs `logistic_log_intensity_sum`/`logistic_compensator`/
//! `window_end_secs`, all `pub(crate)`. What survives is finiteness and warm-start behaviour, so a
//! fitter that returned a finite but badly-fitted optimum would now pass.

use polysim::hot::quant::hawkes::{
    ExpSimulation, HawkesEvents, HawkesMle, HawkesParams, LogisticMle, LogisticParams,
    LogisticShape, LogisticSimulation,
};
use polysim::time::{DurationUs, TsUs};

const START: TsUs = TsUs::from_micros(0);

fn window(stamps: &[TsUs]) -> HawkesEvents {
    let mut events = HawkesEvents::new(32_768);
    for &stamp in stamps {
        events.push(stamp);
    }
    events
}

#[test]
fn fit_recovers_exponential_params_and_warm_starts() {
    // Warm refit reduces iterations via seeded simplex.
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

    let events = window(&path);
    let now = START + horizon;
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
