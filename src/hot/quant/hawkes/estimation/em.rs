//! Expectation-maximisation for univariate exponential kernel. O(n) pass, no matrix materialisation.
//! Alternative to MLE: cheaper for large windows, MLE more robust for small.

use super::mle::{cold_seed, exp_nll, fit_window};
use super::{EM_EPSILON, EM_MAX_ITERATIONS, EM_TOLERANCE, HawkesEstimate, relative_change};
use crate::hot::quant::MIN_RATE;
use crate::hot::quant::hawkes::univariate::{HawkesEvents, HawkesParams};
use crate::time::TsUs;

/// An accidental move forks the warm-start cache, so keep the type non-`Copy`.
#[derive(Debug, Clone, PartialEq)]
pub struct HawkesEm {
    min_events: usize,
    cached: Option<HawkesParams>,
    last: Option<HawkesEstimate>,
}

impl HawkesEm {
    /// # Panics
    /// `min_events < 2` — need interval to fit.
    pub fn new(min_events: usize) -> Self {
        assert!(
            min_events >= 2,
            "hawkes em needs at least two events, got {min_events}"
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
        let span = end - times[0];
        let mut params = self.cached.unwrap_or_else(|| seed(times, end));
        let mut iterations = 0;
        let mut converged = false;
        while iterations < EM_MAX_ITERATIONS {
            let next = em_step(params, times, span);
            iterations += 1;
            let moved = relative_change(params.mu, next.mu)
                .max(relative_change(params.alpha, next.alpha))
                .max(relative_change(params.beta, next.beta));
            params = next;
            if moved < EM_TOLERANCE {
                converged = true;
                break;
            }
        }

        let estimate = HawkesEstimate {
            mu: params.mu,
            alpha: params.alpha,
            beta: params.beta,
            branching_ratio: params.branching_ratio(),
            log_likelihood: -exp_nll(
                &[
                    params.mu.ln(),
                    params.alpha.max(MIN_RATE).ln(),
                    params.beta.ln(),
                ],
                times,
                end,
            ),
            events: times.len(),
            converged,
            iterations,
            is_stale: false,
        };
        self.cached = Some(params);
        self.last = Some(estimate);
        Some(estimate)
    }
}

fn seed(times: &[f64], end: f64) -> HawkesParams {
    let [mu, alpha, beta] = cold_seed(times, end).map(f64::exp);
    HawkesParams::new(mu, alpha, beta)
}

fn em_step(params: HawkesParams, times: &[f64], span: f64) -> HawkesParams {
    let HawkesParams { mu, alpha, beta } = params;
    let mut excitation = 0.0;
    let mut lag_moment = 0.0;
    let mut previous = times[0];
    let mut background = 0.0;
    let mut triggered = 0.0;
    let mut lag_sum = 0.0;
    for (index, &time) in times.iter().enumerate() {
        if index > 0 {
            let delta = time - previous;
            let decay = (-beta * delta).exp();
            lag_moment = decay * (lag_moment + delta * (excitation + 1.0));
            excitation = decay * (excitation + 1.0);
            previous = time;
        }
        let rate = (mu + alpha * excitation).max(MIN_RATE);
        background += mu / rate;
        triggered += alpha * excitation / rate;
        lag_sum += alpha * lag_moment / rate;
    }

    let baseline = (background / span).max(EM_EPSILON);
    if triggered <= EM_EPSILON || lag_sum <= EM_EPSILON {
        return HawkesParams::new(baseline, 0.0, beta);
    }
    let decay = triggered / lag_sum;
    if !decay.is_finite() || decay <= 0.0 {
        return HawkesParams::new(baseline, 0.0, beta);
    }
    HawkesParams::new(baseline, decay * (triggered / times.len() as f64), decay)
}
