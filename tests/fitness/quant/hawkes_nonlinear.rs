//! Non-linear Hawkes kernels. The quadratic form is evaluated through the linear recursion, and the
//! logistic form drops a dead tail and integrates by quadrature — both are optimisations whose
//! failure mode is a slightly wrong intensity, never an error. Each is pinned against the naive
//! definition it replaces.

use polysim::hot::quant::hawkes::{
    HawkesEvents, LogisticParams, LogisticShape, QuadraticParams, UnivariateHawkes,
};
use polysim::time::TsUs;

fn ts(secs: f64) -> TsUs {
    TsUs::from_micros((secs * 1e6).round() as i64)
}

#[test]
fn quadratic_is_a_rescaled_exponential_under_every_gamma_gauge() {
    let quadratic = QuadraticParams::new(0.3, 0.8, 1.5, 2.0);
    let linear = quadratic.to_linear();
    let times = [0.0, 0.7, 1.1, 3.4];
    let mut evaluator = UnivariateHawkes::new(linear);
    for &secs in &times {
        evaluator.on_event(ts(secs));
    }

    let now = 4.0;
    let hand_summed = quadratic.mu
        + times
            .iter()
            .map(|time| {
                let excitation = quadratic.alpha * (-quadratic.beta * (now - time)).exp();
                quadratic.gamma * excitation * excitation
            })
            .sum::<f64>();
    assert!(
        (evaluator.intensity(ts(now)) - hand_summed).abs() < 1e-12,
        "linear form {} vs quadratic sum {hand_summed}",
        evaluator.intensity(ts(now))
    );

    // Gauge invariance: (alpha, gamma) -> (c·alpha, gamma/c²) -> likelihood unchanged.
    let rescaled = QuadraticParams::new(0.3, 1.6, 1.5, 0.5).to_linear();
    assert!((rescaled.alpha - linear.alpha).abs() < 1e-15);
    assert!((rescaled.beta - linear.beta).abs() < 1e-15);
}

#[test]
fn logistic_tail_drop_and_quadrature_track_the_naive_evaluation() {
    // beta = 3 puts the tail cut ~6.9s inside this 10s window, so the drop path is live; naive =
    // raw phi difference over every event (independent of the excess() path).
    let params = LogisticParams::new(
        0.2,
        1.5,
        3.0,
        LogisticShape {
            theta: 3.0,
            delta: 0.5,
        },
    );
    let phi = |x: f64| 1.0 / (1.0 + (-params.shape.theta * (x - params.shape.delta)).exp());
    let psi = |s: f64| phi(params.alpha * (-params.beta * s).exp()) - phi(0.0);
    let times = [0.0, 0.4, 1.1, 2.6, 5.0, 9.0];
    let mut events = HawkesEvents::new(64);
    for &secs in &times {
        events.push(ts(secs));
    }

    let now = 10.0;
    let naive = params.mu + times.iter().map(|time| psi(now - time)).sum::<f64>();
    let dropped = params.intensity(&events, ts(now));
    assert!(
        (dropped - naive).abs() < 1e-8,
        "tail drop {dropped} vs naive {naive}"
    );

    // Branching ratio = ∫psi to infinity; psi(10) ~ 6e-14 so a trapezoid to 10 loses ~2e-14.
    let upper = 10.0;
    let steps = 1_000_000;
    let step = upper / steps as f64;
    let mut trapezoid = 0.5 * (psi(0.0) + psi(upper));
    for index in 1..steps {
        trapezoid += psi(index as f64 * step);
    }
    trapezoid *= step;
    let branching = params.branching_ratio();
    assert!(
        (branching - trapezoid).abs() < 1e-7,
        "branching {branching} vs trapezoid {trapezoid}"
    );
    assert!(params.is_stationary());
}
