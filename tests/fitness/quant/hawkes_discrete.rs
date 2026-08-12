//! Discrete-time (binned) Hawkes intensity: the memory window must weight exactly the last
//! `memory` bins and nothing older, and the stationarity readouts must agree with hand arithmetic.

use polysim::hot::quant::hawkes::{DiscreteCounts, DiscreteParams};

#[test]
fn intensity_next_weights_only_the_memory_window() {
    let params = DiscreteParams::new(0.5, 1.5, 0.4, 2);
    let mut counts = DiscreteCounts::new(16);
    for count in [3u32, 0, 5, 2] {
        counts.push(count);
    }

    let expected = 0.5 + 1.5 * (0.4 * 2.0 + 0.16 * 5.0);
    let rate = params.intensity_next(&counts);
    assert!((rate - expected).abs() < 1e-12, "rate {rate} vs {expected}");

    assert!((params.offspring_mean() - 0.84).abs() < 1e-12);
    assert!(params.is_stationary());
    assert!((params.long_run_rate().expect("stationary") - 3.125).abs() < 1e-9);
    assert!((params.half_life_bins() - 0.756_47).abs() < 1e-5);
    assert!(!DiscreteParams::new(0.5, 1.0, 0.7, 2).is_stationary());
    assert!(
        DiscreteParams::new(0.5, 1.0, 0.7, 2)
            .long_run_rate()
            .is_none()
    );
}
