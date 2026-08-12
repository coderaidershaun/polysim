//! Discrete-time maximum likelihood. The kernel's memory window rolls in O(1) per bin, so an
//! objective evaluation costs one pass regardless of how deep the memory is set.

use super::{bounded, penalised};
use crate::hot::quant::MIN_RATE;
use crate::hot::quant::hawkes::univariate::{DiscreteCounts, DiscreteParams};
use crate::hot::quant::optimise::NelderMead;

const PARAMS: usize = 3;
const SIMPLEX: usize = PARAMS + 1;

/// Ordered as [`DiscreteParams::new`] takes them, so every seed and unpack below is
/// positional-identical to the constructor. Baseline and amplitude are searched in log space to
/// fence off overflow; decay is searched raw because the open unit interval IS its constraint, and
/// the box enforces it directly.
const BOUNDS: [(f64, f64); PARAMS] = [(-25.0, 10.0), (-25.0, 10.0), (1e-6, 0.999_999)];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiscreteEstimate {
    pub mu: f64,
    pub amplitude: f64,
    pub decay: f64,
    pub offspring_mean: f64,
    pub log_likelihood: f64,
    pub bins: usize,
    pub converged: bool,
    pub iterations: usize,
    /// Not a fresh solve: the window could not support one, so these are the previous fit's
    /// numbers re-dated.
    pub is_stale: bool,
}

impl DiscreteEstimate {
    #[inline]
    pub fn is_stationary(&self) -> bool {
        self.offspring_mean < 1.0
    }
}

/// An accidental move forks the warm-start cache, so keep the type non-`Copy`.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscreteMle {
    memory: usize,
    min_bins: usize,
    cached: Option<[f64; PARAMS]>,
    last: Option<DiscreteEstimate>,
}

impl DiscreteMle {
    /// # Panics
    /// `memory == 0` or `min_bins <= memory` — need 1+ bin past memory for meaningful fit.
    pub fn new(memory: usize, min_bins: usize) -> Self {
        assert!(memory != 0, "discrete mle memory must be at least one bin");
        assert!(
            min_bins > memory,
            "discrete mle needs more than {memory} bins to fit a {memory}-bin memory, got {min_bins}"
        );
        Self {
            memory,
            min_bins,
            cached: None,
            last: None,
        }
    }

    pub fn fit(&mut self, counts: &DiscreteCounts) -> Option<DiscreteEstimate> {
        let bins = counts.counts();
        if bins.len() < self.min_bins {
            return self.last.map(|last| DiscreteEstimate {
                is_stale: true,
                ..last
            });
        }
        let memory = self.memory;
        let seed = self.cached.unwrap_or_else(|| cold_seed(bins));
        let optimum = NelderMead::new(seed, BOUNDS)
            .minimize::<SIMPLEX>(|params| discrete_nll(params, bins, memory));

        let [mu, amplitude, decay] = optimum.x;
        let fitted = DiscreteParams::new(mu.exp(), amplitude.exp(), decay, memory);
        let estimate = DiscreteEstimate {
            mu: fitted.mu,
            amplitude: fitted.amplitude,
            decay: fitted.decay,
            offspring_mean: fitted.offspring_mean(),
            log_likelihood: -optimum.value,
            bins: bins.len(),
            converged: optimum.converged,
            iterations: optimum.iterations,
            is_stale: false,
        };
        self.cached = Some(optimum.x);
        self.last = Some(estimate);
        Some(estimate)
    }
}

fn cold_seed(bins: &[i64]) -> [f64; PARAMS] {
    let mean = bins.iter().sum::<i64>() as f64 / bins.len() as f64;
    [bounded((0.5 * mean).ln(), BOUNDS[0]), 0.5f64.ln(), 0.5]
}

fn discrete_nll(params: &[f64; PARAMS], bins: &[i64], memory: usize) -> f64 {
    let (mu, amplitude, decay) = (params[0].exp(), params[1].exp(), params[2]);
    let dropped_weight = decay.powi(memory as i32 + 1);
    let mut excitation = 0.0;
    let mut total = 0.0;
    for (index, &count) in bins.iter().enumerate() {
        let rate = mu + amplitude * excitation;
        let observed = count as f64;
        total += rate - observed * rate.max(MIN_RATE).ln();
        let dropped = if index >= memory { bins[index - memory] as f64 } else { 0.0 };
        excitation = decay * (excitation + observed) - dropped_weight * dropped;
    }
    penalised(total)
}
