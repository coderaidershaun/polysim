//! Seeded Hawkes simulators. Every fitter fixture in this suite is a simulated path, so a simulator
//! that is not reproducible from its seed silently turns every fit test into a flake, and a
//! simulator whose event count drifts from the stationary rate silently invalidates the parameters
//! those fits claim to recover.

use polysim::hot::quant::hawkes::{
    DiscreteParams, DiscreteSimulation, ExpSimulation, HawkesParams, LogisticParams, LogisticShape,
    LogisticSimulation, MultivariateParams, MultivariateSimulation,
};
use polysim::time::{DurationUs, TsUs};

#[test]
fn seeded_paths_replay_exactly_and_land_near_the_stationary_count() {
    // Stationary rate mu/(1 - alpha/beta) with alpha/beta = offspring mean.
    let exponential = ExpSimulation {
        params: HawkesParams::new(0.5, 0.8, 2.0),
        start_ts: TsUs::from_micros(0),
        horizon: DurationUs::from_secs(2000),
        seed: 0xC0FF_EE00,
        max_events: 20_000,
    };
    let path = exponential.run();
    assert_eq!(path, exponential.run(), "same seed diverged");
    assert_ne!(
        path,
        ExpSimulation {
            seed: 0x1234_5678,
            ..exponential
        }
        .run(),
        "distinct seeds produced the same path"
    );

    let expected = 0.5 * 2000.0 / (1.0 - 0.4);
    let observed = path.len() as f64;
    assert!(
        observed > 0.7 * expected && observed < 1.3 * expected,
        "{observed} events against an expected {expected}"
    );

    let logistic = LogisticSimulation {
        params: LogisticParams::new(
            0.2,
            1.0,
            2.0,
            LogisticShape {
                theta: 3.0,
                delta: 1.0,
            },
        ),
        start_ts: TsUs::from_micros(0),
        horizon: DurationUs::from_secs(600),
        seed: 0xC0FF_EE02,
        max_events: 5000,
    };
    let logistic_path = logistic.run();
    assert!(!logistic_path.is_empty());
    assert_eq!(logistic_path, logistic.run());

    let discrete = DiscreteSimulation {
        params: DiscreteParams::new(2.0, 1.0, 0.3, 3),
        bins: 500,
        seed: 0xC0FF_EE03,
    };
    let counts = discrete.run();
    assert_eq!(counts, discrete.run());
    assert!(counts.iter().any(|count| *count > 0));
}

#[test]
fn multivariate_paths_replay_exactly_and_carry_both_components() {
    // (I - Γ)^{-1}·mu with Γ = alpha/beta.
    let simulation = MultivariateSimulation {
        params: MultivariateParams::new(
            vec![0.3, 0.3],
            vec![0.2, 1.2, 0.1, 0.2],
            vec![2.0, 2.0, 2.0, 2.0],
        ),
        start_ts: TsUs::from_micros(0),
        horizon: DurationUs::from_secs(1000),
        seed: 0xC0FF_EE05,
        max_events: 20_000,
    };
    let path = simulation.run();
    assert_eq!(path, simulation.run(), "same seed diverged");
    assert_ne!(
        path,
        MultivariateSimulation {
            seed: 0x1234_5678,
            ..simulation.clone()
        }
        .run(),
        "distinct seeds produced the same path"
    );

    let zeros = path.iter().filter(|(_, component)| *component == 0).count();
    let ones = path.len() - zeros;
    assert!(ones > 0 && zeros > ones, "split {zeros}/{ones}");
    let expected = (0.577 + 0.365) * 1000.0;
    let observed = path.len() as f64;
    assert!(
        observed > 0.7 * expected && observed < 1.3 * expected,
        "{observed} events against an expected {expected}"
    );
}
