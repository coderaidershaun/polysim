//! Multivariate (cross-exciting) Hawkes: the pair recursion must equal the direct double loop, the
//! closed-form compensator must equal a fine trapezoid, and the spectral radius must find the known
//! Perron root. The radius is the stationarity gate — reading it high or low silently reclassifies
//! an explosive fit as usable.

use polysim::hot::quant::hawkes::{MultivariateEvents, MultivariateHawkes, MultivariateParams};
use polysim::time::{DurationUs, TsUs};

fn ts(secs: f64) -> TsUs {
    TsUs::from_micros((secs * 1e6).round() as i64)
}

fn params() -> MultivariateParams {
    MultivariateParams::new(
        vec![0.3, 0.5],
        vec![0.4, 0.2, 0.1, 0.6],
        vec![1.5, 2.0, 2.5, 1.0],
    )
}

#[test]
fn pair_recursion_matches_the_direct_double_loop() {
    let params = params();
    let path = [(0.0, 0usize), (0.4, 1), (0.9, 0), (1.3, 1), (2.2, 0)];
    let mut hawkes = MultivariateHawkes::new(params.clone());
    let mut events = MultivariateEvents::new(2, 16);
    for &(secs, component) in &path {
        hawkes.on_event(ts(secs), component);
        events.push(ts(secs), component);
    }

    let now = 3.0;
    let mut direct = [0.0; 2];
    for (target, rate) in direct.iter_mut().enumerate() {
        *rate = params.mu[target];
        for &(secs, source) in &path {
            let cell = target * 2 + source;
            *rate += params.alpha[cell] * (-params.beta[cell] * (now - secs)).exp();
        }
    }

    let mut recursive = [0.0; 2];
    hawkes.intensities_into(ts(now), &mut recursive);
    for target in 0..2 {
        assert!(
            (recursive[target] - direct[target]).abs() < 1e-12,
            "component {target}: recursion {} vs direct {}",
            recursive[target],
            direct[target]
        );
    }
    assert!((hawkes.total_intensity(ts(now)) - (direct[0] + direct[1])).abs() < 1e-12);

    let mut reseeded = MultivariateHawkes::new(params);
    reseeded.reseed_from(&events);
    assert!((reseeded.intensity(ts(now), 0) - direct[0]).abs() < 1e-12);
    assert!((reseeded.intensity(ts(now), 1) - direct[1]).abs() < 1e-12);

    let horizon = 2.0;
    let steps = 20_000;
    let dt = horizon / steps as f64;
    for target in 0..2 {
        let mut integral =
            0.5 * (hawkes.intensity(ts(now), target) + hawkes.intensity(ts(now + horizon), target));
        for step in 1..steps {
            integral += hawkes.intensity(ts(now + step as f64 * dt), target);
        }
        integral *= dt;
        let closed = hawkes.expected_events(ts(now), DurationUs::from_secs(2), target);
        assert!(
            (closed - integral).abs() < 1e-6,
            "component {target}: closed form {closed} vs trapezoid {integral}"
        );
    }
}

#[test]
fn spectral_radius_finds_the_known_perron_root() {
    let symmetric = MultivariateParams::new(
        vec![1.0, 1.0],
        vec![0.5, 0.2, 0.2, 0.5],
        vec![1.0, 1.0, 1.0, 1.0],
    );
    let mut scratch = [0.0; 4];
    let radius = symmetric.spectral_radius(&mut scratch);
    assert!((radius - 0.7).abs() < 1e-12, "radius {radius}");
    assert!(symmetric.is_stationary(&mut scratch));

    let explosive = MultivariateParams::new(
        vec![1.0, 1.0],
        vec![1.0, 0.4, 0.4, 1.0],
        vec![1.0, 1.0, 1.0, 1.0],
    );
    assert!((explosive.spectral_radius(&mut scratch) - 1.4).abs() < 1e-12);
    assert!(!explosive.is_stationary(&mut scratch));
}
