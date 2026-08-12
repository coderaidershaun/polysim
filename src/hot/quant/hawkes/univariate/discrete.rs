//! Discrete-time Hawkes: per-bin counts with geometric memory kernel.
//! Query O(memory), likelihood O(1) per bin (with fitter).

use crate::hot::series::FastQueue;

/// Full window -> evict oldest.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscreteCounts {
    bins: FastQueue<i64>,
}

impl DiscreteCounts {
    /// # Panics
    /// `max_bins == 0` — config bug.
    pub fn new(max_bins: usize) -> Self {
        assert!(max_bins != 0, "discrete hawkes bin window must be non-zero");
        Self {
            bins: FastQueue::new(max_bins, 2),
        }
    }

    pub fn push(&mut self, count: u32) {
        self.bins.push(i64::from(count));
    }

    pub fn clear(&mut self) {
        self.bins.clear();
    }

    pub fn len(&self) -> usize {
        self.bins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bins.is_empty()
    }

    pub(crate) fn counts(&self) -> &[i64] {
        self.bins.as_slice()
    }
}

/// Kernel λ_i = mu + Σ amplitude·decay^Δ · N_{i-Δ}. amplitude/decay comparable across regimes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiscreteParams {
    pub mu: f64,
    /// One event in previous bin adds amplitude·decay to current rate.
    pub amplitude: f64,
    /// Discrete decay per bin (0, 1), named apart from continuous beta.
    pub decay: f64,
    pub memory: usize,
}

impl DiscreteParams {
    /// # Panics
    /// Non-finite, mu <= 0, amplitude < 0, decay outside (0, 1), or memory == 0.
    pub fn new(mu: f64, amplitude: f64, decay: f64, memory: usize) -> Self {
        assert!(
            mu.is_finite() && mu > 0.0,
            "discrete hawkes baseline must be finite and positive, got {mu}"
        );
        assert!(
            amplitude.is_finite() && amplitude >= 0.0,
            "discrete hawkes amplitude must be finite and non-negative, got {amplitude}"
        );
        assert!(
            decay > 0.0 && decay < 1.0,
            "discrete hawkes decay must lie in (0, 1), got {decay}"
        );
        assert!(
            memory != 0,
            "discrete hawkes memory must be at least one bin"
        );
        Self {
            mu,
            amplitude,
            decay,
            memory,
        }
    }

    pub fn offspring_mean(&self) -> f64 {
        self.amplitude * self.decay * (1.0 - self.decay.powi(self.memory as i32))
            / (1.0 - self.decay)
    }

    pub fn is_stationary(&self) -> bool {
        self.offspring_mean() < 1.0
    }

    #[inline]
    pub fn half_life_bins(&self) -> f64 {
        0.5f64.ln() / self.decay.ln()
    }

    pub fn long_run_rate(&self) -> Option<f64> {
        self.is_stationary()
            .then(|| self.mu / (1.0 - self.offspring_mean()))
    }

    pub fn intensity_next(&self, counts: &DiscreteCounts) -> f64 {
        let mut rate = self.mu;
        let mut weight = self.amplitude * self.decay;
        for &count in counts.counts().iter().rev().take(self.memory) {
            rate += weight * count as f64;
            weight *= self.decay;
        }
        rate
    }
}
