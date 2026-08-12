//! Continuous-time MLE. Exponential compensator closed-form -> O(n) objective pass, no quadrature.
//! Logistic reuses exponential pieces.

use super::{bounded, penalised};
use crate::hot::quant::MIN_RATE;
use crate::hot::quant::hawkes::univariate::{
    HawkesEvents, HawkesParams, LogisticParams, LogisticShape, logistic_compensator,
    logistic_log_intensity_sum,
};
use crate::hot::quant::optimise::NelderMead;
use crate::time::TsUs;

const PARAMS: usize = 3;
const SIMPLEX: usize = PARAMS + 1;

const LOG_BOUNDS: [(f64, f64); PARAMS] = [(-25.0, 10.0), (-25.0, 10.0), (-12.0, 8.0)];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HawkesEstimate {
    pub mu: f64,
    pub alpha: f64,
    pub beta: f64,
    pub branching_ratio: f64,
    pub log_likelihood: f64,
    pub events: usize,
    pub converged: bool,
    pub iterations: usize,
    /// Not a fresh solve: the window could not support one, so these are the previous fit's
    /// numbers re-dated.
    pub is_stale: bool,
}

impl HawkesEstimate {
    pub fn params(&self) -> HawkesParams {
        HawkesParams::new(self.mu, self.alpha, self.beta)
    }

    #[inline]
    pub fn is_stationary(&self) -> bool {
        self.branching_ratio < 1.0
    }
}

/// An accidental move forks the warm-start cache, so keep the type non-`Copy`.
#[derive(Debug, Clone, PartialEq)]
pub struct HawkesMle {
    min_events: usize,
    cached: Option<[f64; PARAMS]>,
    last: Option<HawkesEstimate>,
}

impl HawkesMle {
    /// # Panics
    /// `min_events < 2` — need interval to fit.
    pub fn new(min_events: usize) -> Self {
        assert!(
            min_events >= 2,
            "hawkes mle needs at least two events, got {min_events}"
        );
        Self {
            min_events,
            cached: None,
            last: None,
        }
    }

    pub fn fit(&mut self, events: &HawkesEvents, now: TsUs) -> Option<HawkesEstimate> {
        let Some((times, end)) = fit_window(events, now, self.min_events) else {
            return self.last.map(|last| HawkesEstimate {
                is_stale: true,
                ..last
            });
        };
        let optimum = NelderMead::new(
            self.cached.unwrap_or_else(|| cold_seed(times, end)),
            LOG_BOUNDS,
        )
        .minimize::<SIMPLEX>(|params| exp_nll(params, times, end));

        let [mu, alpha, beta] = optimum.x.map(f64::exp);
        let estimate = HawkesEstimate {
            mu,
            alpha,
            beta,
            branching_ratio: alpha / beta,
            log_likelihood: -optimum.value,
            events: times.len(),
            converged: optimum.converged,
            iterations: optimum.iterations,
            is_stale: false,
        };
        self.cached = Some(optimum.x);
        self.last = Some(estimate);
        Some(estimate)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogisticEstimate {
    pub mu: f64,
    pub alpha: f64,
    pub beta: f64,
    pub shape: LogisticShape,
    /// ∫psi of fitted centred kernel.
    pub branching_ratio: f64,
    pub log_likelihood: f64,
    pub events: usize,
    pub converged: bool,
    pub iterations: usize,
    /// Not a fresh solve: the window could not support one, so these are the previous fit's
    /// numbers re-dated.
    pub is_stale: bool,
}

impl LogisticEstimate {
    /// The fit is never constrained to the stationary region, so an explosive kernel is reported as
    /// one rather than clamped into a plausible-looking fit.
    #[inline]
    pub fn is_stationary(&self) -> bool {
        self.branching_ratio < 1.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogisticMle {
    shape: LogisticShape,
    min_events: usize,
    cached: Option<[f64; PARAMS]>,
    last: Option<LogisticEstimate>,
}

impl LogisticMle {
    /// # Panics
    /// `min_events < 2` or shape non-finite/theta <= 0.
    pub fn new(min_events: usize, shape: LogisticShape) -> Self {
        assert!(
            min_events >= 2,
            "logistic mle needs at least two events, got {min_events}"
        );
        assert!(
            shape.theta.is_finite() && shape.theta > 0.0 && shape.delta.is_finite(),
            "logistic shape must be finite with positive steepness, got theta={} delta={}",
            shape.theta,
            shape.delta
        );
        Self {
            shape,
            min_events,
            cached: None,
            last: None,
        }
    }

    pub fn fit(&mut self, events: &HawkesEvents, now: TsUs) -> Option<LogisticEstimate> {
        let Some((times, end)) = fit_window(events, now, self.min_events) else {
            return self.last.map(|last| LogisticEstimate {
                is_stale: true,
                ..last
            });
        };
        let shape = self.shape;
        let optimum = NelderMead::new(
            self.cached
                .unwrap_or_else(|| logistic_cold_seed(times, end, shape)),
            LOG_BOUNDS,
        )
        .minimize::<SIMPLEX>(|params| logistic_nll(params, shape, times, end));

        let [mu, alpha, beta] = optimum.x.map(f64::exp);
        let fitted = LogisticParams {
            mu,
            alpha,
            beta,
            shape,
        };
        let branching_ratio = fitted.branching_ratio();
        let estimate = LogisticEstimate {
            mu,
            alpha,
            beta,
            shape,
            branching_ratio,
            log_likelihood: -optimum.value,
            events: times.len(),
            converged: optimum.converged,
            iterations: optimum.iterations,
            is_stale: false,
        };
        self.cached = Some(optimum.x);
        self.last = Some(estimate);
        Some(estimate)
    }
}

pub(super) fn fit_window(
    events: &HawkesEvents,
    now: TsUs,
    min_events: usize,
) -> Option<(&[f64], f64)> {
    let times = events.times_secs();
    if times.len() < min_events {
        return None;
    }
    let end = events.window_end_secs(now)?;
    if end - times[0] <= 0.0 {
        return None;
    }
    Some((times, end))
}

pub(super) fn cold_seed(times: &[f64], end: f64) -> [f64; PARAMS] {
    let count = times.len() as f64;
    let mean_gap = (times[times.len() - 1] - times[0]) / (count - 1.0);
    let decay = mean_gap.recip();
    [
        bounded((0.5 * count / (end - times[0])).ln(), LOG_BOUNDS[0]),
        bounded((0.5 * decay).ln(), LOG_BOUNDS[1]),
        bounded(decay.ln(), LOG_BOUNDS[2]),
    ]
}

fn logistic_cold_seed(times: &[f64], end: f64, shape: LogisticShape) -> [f64; PARAMS] {
    let mut seed = cold_seed(times, end);
    seed[1] = bounded(shape.delta.max(shape.theta.recip()).ln(), LOG_BOUNDS[1]);
    seed
}

pub(super) fn exp_nll(params: &[f64; PARAMS], times: &[f64], end: f64) -> f64 {
    let [mu, alpha, beta] = params.map(f64::exp);
    let mut excitation = 0.0;
    let mut previous = times[0];
    let mut log_sum = 0.0;
    for (index, &time) in times.iter().enumerate() {
        if index > 0 {
            excitation = (-beta * (time - previous)).exp() * (excitation + 1.0);
            previous = time;
        }
        log_sum += (mu + alpha * excitation).max(MIN_RATE).ln();
    }
    let tail = (-beta * (end - previous)).exp() * (excitation + 1.0);
    let compensator = mu * (end - times[0]) + (alpha / beta) * (times.len() as f64 - tail);
    penalised(compensator - log_sum)
}

fn logistic_nll(params: &[f64; PARAMS], shape: LogisticShape, times: &[f64], end: f64) -> f64 {
    let [mu, alpha, beta] = params.map(f64::exp);
    let candidate = LogisticParams {
        mu,
        alpha,
        beta,
        shape,
    };
    penalised(
        logistic_compensator(&candidate, times, end)
            - logistic_log_intensity_sum(&candidate, times),
    )
}
