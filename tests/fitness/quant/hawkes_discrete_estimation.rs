//! Discrete-time Hawkes MLE. Stationarity is the load-bearing output: a fitter that recovers
//! the kernel but calls an explosive window stationary hands the strategy a long-run rate that does
//! not exist, and nothing downstream can tell.

use polysim::hot::quant::hawkes::{
    DiscreteCounts, DiscreteMle, DiscreteParams, DiscreteSimulation,
};

/// The fitter evaluates its objective through a rolling memory window that advances in O(1) per
/// bin. This pins that recursion against the definition it stands in for — the direct double sum
/// over the last `memory` bins — by driving it through its only caller: the reported
/// `log_likelihood` IS `-discrete_nll` at the reported parameters, so recomputing the sum from those
/// same parameters must agree. A recursion that drifted from the definition would still converge,
/// still report `converged`, and quietly fit a different model.
#[test]
fn likelihood_matches_the_direct_memory_window_sum() {
    let truth = DiscreteParams::new(2.0, 1.5, 0.3, 3);
    let path = DiscreteSimulation {
        params: truth,
        bins: 3000,
        seed: 0x5EED_0007,
    }
    .run();
    let bins: Vec<i64> = path.iter().map(|&count| i64::from(count)).collect();
    let mut counts = DiscreteCounts::new(4096);
    for count in path {
        counts.push(count);
    }

    let fitted = DiscreteMle::new(truth.memory, 64)
        .fit(&counts)
        .expect("cold fit");
    let direct: f64 = bins
        .iter()
        .enumerate()
        .map(|(index, &count)| {
            let rate = fitted.mu
                + fitted.amplitude
                    * (1..=truth.memory)
                        .filter(|lag| index >= *lag)
                        .map(|lag| fitted.decay.powi(lag as i32) * bins[index - lag] as f64)
                        .sum::<f64>();
            rate - count as f64 * rate.ln()
        })
        .sum();

    // Measured agreement is bit-exact; the tolerance is only here so a reassociated sum on another
    // target is not a failure. Anything a wrong recursion could do is orders of magnitude wider.
    let recursion = -fitted.log_likelihood;
    assert!(
        (recursion - direct).abs() < 1e-12 * direct.abs(),
        "recursion {recursion} vs direct {direct}"
    );
}

#[test]
fn fit_recovers_the_kernel_and_flags_an_explosive_window() {
    // mu amplitude decay trade-off in the fit tolerance bands.
    let truth = DiscreteParams::new(2.0, 1.5, 0.3, 3);
    let path = DiscreteSimulation {
        params: truth,
        bins: 3000,
        seed: 0x5EED_0007,
    }
    .run();
    let mut counts = DiscreteCounts::new(4096);
    for count in path {
        counts.push(count);
    }

    let mut fitter = DiscreteMle::new(truth.memory, 64);
    let estimate = fitter.fit(&counts).expect("cold fit");
    assert!(
        (estimate.mu / truth.mu - 1.0).abs() < 0.3,
        "mu {}",
        estimate.mu
    );
    assert!(
        (estimate.amplitude / truth.amplitude - 1.0).abs() < 0.3,
        "amplitude {}",
        estimate.amplitude
    );
    assert!(
        (estimate.decay - truth.decay).abs() < 0.1,
        "decay {}",
        estimate.decay
    );
    assert!(
        (estimate.offspring_mean / truth.offspring_mean() - 1.0).abs() < 0.15,
        "offspring {} vs {}",
        estimate.offspring_mean,
        truth.offspring_mean()
    );
    assert!(estimate.is_stationary());

    let explosive = DiscreteParams::new(1.0, 1.0, 0.7, 2);
    assert!(!explosive.is_stationary());
    let mut hot = DiscreteCounts::new(256);
    for count in (DiscreteSimulation {
        params: explosive,
        bins: 60,
        seed: 0x5EED_0009,
    })
    .run()
    {
        hot.push(count);
    }
    let mut explosive_fitter = DiscreteMle::new(explosive.memory, 16);
    assert!(
        !explosive_fitter
            .fit(&hot)
            .expect("explosive fit")
            .is_stationary()
    );
}
