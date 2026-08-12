//! Multivariate EM fitter. The point of a cross-exciting fit is the ASYMMETRY — which side leads
//! which — so a fitter that recovers the right total excitation but smears it symmetrically across
//! the off-diagonal reports a research column that is exactly backwards, and converges cleanly
//! while doing it.

use polysim::hot::quant::hawkes::{
    MultivariateEm, MultivariateEvents, MultivariateParams, MultivariateSimulation,
};
use polysim::time::{DurationUs, TsUs};

const START: TsUs = TsUs::from_micros(0);

#[test]
fn em_recovers_asymmetric_cross_excitation() {
    // Perron root 0.273 -> subcritical, fit must report stationary.
    let truth = MultivariateParams::new(
        vec![0.3, 0.3],
        vec![0.2, 1.2, 0.1, 0.2],
        vec![2.0, 2.0, 2.0, 2.0],
    );
    let horizon = DurationUs::from_secs(20_000);
    let path = MultivariateSimulation {
        params: truth.clone(),
        start_ts: START,
        horizon,
        seed: 0xC0FF_EE06,
        max_events: 60_000,
    }
    .run();
    assert!(path.len() > 10_000, "thin path: {} events", path.len());

    let mut events = MultivariateEvents::new(2, 65_536);
    for &(stamp, component) in &path {
        events.push(stamp, component);
    }
    let now = START + horizon;

    let mut fitter = MultivariateEm::new(2, 64);
    let cold = fitter.fit(&events, now).expect("cold fit").clone();
    assert!(cold.converged, "em hit its iteration cap");
    assert!(cold.is_stationary(), "radius {}", cold.spectral_radius);
    assert!(
        (cold.spectral_radius - 0.273).abs() < 0.08,
        "radius {}",
        cold.spectral_radius
    );
    for (index, name) in [
        (0, "alpha_00"),
        (1, "alpha_01"),
        (2, "alpha_10"),
        (3, "alpha_11"),
    ] {
        assert!(
            (cold.params.alpha[index] - truth.alpha[index]).abs() < 0.3,
            "{name} {} against {}",
            cold.params.alpha[index],
            truth.alpha[index]
        );
    }
    assert!(
        cold.params.alpha[1] > 4.0 * cold.params.alpha[2],
        "cross terms {} and {} are not asymmetric",
        cold.params.alpha[1],
        cold.params.alpha[2]
    );
    for (index, decay) in cold.params.beta.iter().enumerate() {
        assert!((decay - 2.0).abs() < 0.8, "beta[{index}] {decay}");
    }

    let warm = fitter.fit(&events, now).expect("warm refit");
    assert!(!warm.is_stale && warm.converged);
    assert!(warm.iterations <= cold.iterations);
    assert!(warm.log_likelihood >= cold.log_likelihood - 1e-6);

    let mut thin = MultivariateEvents::new(2, 16);
    thin.push(START, 0);
    thin.push(START + DurationUs::from_secs(1), 1);
    assert!(fitter.fit(&thin, now).expect("stale reissue").is_stale);
}
