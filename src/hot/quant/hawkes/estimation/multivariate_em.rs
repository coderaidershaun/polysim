//! EM for d-component exponential kernel (only option: simplex not viable for runtime d + 2d²).
//! Per-pair decay fitting. Zero-alloc refit (buffers sized once, estimate passed by ref).
//! M-step closed-form per pair.

use super::{EM_EPSILON, EM_MAX_ITERATIONS, EM_TOLERANCE, relative_change};
use crate::hot::quant::MIN_RATE;
use crate::hot::quant::hawkes::multivariate::{MultivariateEvents, MultivariateParams};
use crate::time::TsUs;

/// Heap-backed, because the component count is a runtime value.
#[derive(Debug, Clone, PartialEq)]
pub struct MultivariateEstimate {
    pub params: MultivariateParams,
    pub spectral_radius: f64,
    pub log_likelihood: f64,
    pub events: usize,
    pub converged: bool,
    pub iterations: usize,
    /// Not a fresh solve: the window could not support one, so these are the previous fit's
    /// numbers re-dated.
    pub is_stale: bool,
}

impl MultivariateEstimate {
    #[inline]
    pub fn is_stationary(&self) -> bool {
        self.spectral_radius < 1.0
    }
}

/// E/M scratch plus the parameters themselves, updated in place so a refit allocates nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct MultivariateEm {
    dimension: usize,
    min_events: usize,
    excitation: Vec<f64>,
    lag_moment: Vec<f64>,
    triggered: Vec<f64>,
    lag_sum: Vec<f64>,
    background: Vec<f64>,
    counts: Vec<f64>,
    scratch: Vec<f64>,
    estimate: MultivariateEstimate,
    has_fit: bool,
}

impl MultivariateEm {
    /// # Panics
    /// `dimension == 0` or `min_events < 2` — need interval to fit.
    pub fn new(dimension: usize, min_events: usize) -> Self {
        assert!(
            dimension != 0,
            "multivariate em needs at least one component"
        );
        assert!(
            min_events >= 2,
            "multivariate em needs at least two events, got {min_events}"
        );
        let cells = dimension * dimension;
        Self {
            dimension,
            min_events,
            excitation: vec![0.0; cells],
            lag_moment: vec![0.0; cells],
            triggered: vec![0.0; cells],
            lag_sum: vec![0.0; cells],
            background: vec![0.0; dimension],
            counts: vec![0.0; dimension],
            scratch: vec![0.0; 2 * dimension],
            estimate: MultivariateEstimate {
                params: MultivariateParams::new(
                    vec![1.0; dimension],
                    vec![0.0; cells],
                    vec![1.0; cells],
                ),
                spectral_radius: 0.0,
                log_likelihood: 0.0,
                events: 0,
                converged: false,
                iterations: 0,
                is_stale: false,
            },
            has_fit: false,
        }
    }

    /// # Panics
    /// `events.dimension() != self.dimension` — wiring bug.
    pub fn fit(&mut self, events: &MultivariateEvents, now: TsUs) -> Option<&MultivariateEstimate> {
        assert!(
            events.dimension() == self.dimension,
            "fitter built for {} components, got a {}-component window",
            self.dimension,
            events.dimension()
        );
        match fit_window(events, now, self.min_events) {
            Some((times, components, end)) => self.refit(times, components, end),
            None => self.estimate.is_stale = true,
        }
        self.has_fit.then_some(&self.estimate)
    }

    fn refit(&mut self, times: &[f64], components: &[i64], end: f64) {
        let span = end - times[0];
        if !self.has_fit {
            self.cold_seed(times, components, span);
        }
        let mut iterations = 0;
        let mut converged = false;
        while iterations < EM_MAX_ITERATIONS {
            self.e_step(times, components);
            iterations += 1;
            if self.m_step(span) < EM_TOLERANCE {
                converged = true;
                break;
            }
        }
        let log_sum = self.e_step(times, components);
        let radius = self.estimate.params.spectral_radius(&mut self.scratch);
        let tail = end - times[times.len() - 1];
        self.estimate.log_likelihood = log_sum - self.compensator(span, tail);
        self.estimate.spectral_radius = radius;
        self.estimate.events = times.len();
        self.estimate.converged = converged;
        self.estimate.iterations = iterations;
        self.estimate.is_stale = false;
        self.has_fit = true;
    }

    fn cold_seed(&mut self, times: &[f64], components: &[i64], span: f64) {
        let count = times.len() as f64;
        let gap = (times[times.len() - 1] - times[0]) / (count - 1.0);
        let decay = if gap.is_finite() && gap > 0.0 { gap.recip() } else { count / span };
        self.tally(components);
        for (baseline, observed) in self.estimate.params.mu.iter_mut().zip(&self.counts) {
            *baseline = (0.5 * observed / span).max(EM_EPSILON);
        }
        self.estimate
            .params
            .alpha
            .fill(0.5 * decay / self.dimension as f64);
        self.estimate.params.beta.fill(decay);
    }

    fn e_step(&mut self, times: &[f64], components: &[i64]) -> f64 {
        let dimension = self.dimension;
        self.excitation.fill(0.0);
        self.lag_moment.fill(0.0);
        self.triggered.fill(0.0);
        self.lag_sum.fill(0.0);
        self.background.fill(0.0);
        self.tally(components);

        let params = &self.estimate.params;
        let mut previous = times[0];
        let mut log_sum = 0.0;
        for (index, (&time, &component)) in times.iter().zip(components).enumerate() {
            if index > 0 {
                let delta = time - previous;
                for cell in 0..dimension * dimension {
                    let decay = (-params.beta[cell] * delta).exp();
                    self.lag_moment[cell] =
                        decay * (self.lag_moment[cell] + delta * self.excitation[cell]);
                    self.excitation[cell] *= decay;
                }
                previous = time;
            }
            let target = component as usize;
            let row = target * dimension;
            let mut rate = params.mu[target];
            for source in 0..dimension {
                rate += params.alpha[row + source] * self.excitation[row + source];
            }
            let rate = rate.max(MIN_RATE);
            log_sum += rate.ln();
            self.background[target] += params.mu[target] / rate;
            for source in 0..dimension {
                let cell = row + source;
                self.triggered[cell] += params.alpha[cell] * self.excitation[cell] / rate;
                self.lag_sum[cell] += params.alpha[cell] * self.lag_moment[cell] / rate;
            }
            for source_row in 0..dimension {
                self.excitation[source_row * dimension + target] += 1.0;
            }
        }
        log_sum
    }

    fn m_step(&mut self, span: f64) -> f64 {
        let dimension = self.dimension;
        let mut moved: f64 = 0.0;
        for target in 0..dimension {
            let baseline = (self.background[target] / span).max(EM_EPSILON);
            moved = moved.max(relative_change(self.estimate.params.mu[target], baseline));
            self.estimate.params.mu[target] = baseline;
        }
        for cell in 0..dimension * dimension {
            let mass = self.triggered[cell];
            let lag = self.lag_sum[cell];
            let sources = self.counts[cell % dimension];
            let fitted = mass / lag;
            let collapse = mass <= EM_EPSILON
                || lag <= EM_EPSILON
                || sources <= 0.0
                || !fitted.is_finite()
                || fitted <= 0.0;
            let (jump, decay) = if collapse {
                (0.0, self.estimate.params.beta[cell])
            } else {
                (fitted * (mass / sources), fitted)
            };
            moved = moved.max(relative_change(self.estimate.params.alpha[cell], jump));
            moved = moved.max(relative_change(self.estimate.params.beta[cell], decay));
            self.estimate.params.alpha[cell] = jump;
            self.estimate.params.beta[cell] = decay;
        }
        moved
    }

    fn compensator(&self, span: f64, tail: f64) -> f64 {
        let dimension = self.dimension;
        let params = &self.estimate.params;
        let mut total = params.mu.iter().sum::<f64>() * span;
        for cell in 0..dimension * dimension {
            let residual = (-params.beta[cell] * tail).exp() * self.excitation[cell];
            total += (params.alpha[cell] / params.beta[cell])
                * (self.counts[cell % dimension] - residual);
        }
        total
    }

    fn tally(&mut self, components: &[i64]) {
        self.counts.fill(0.0);
        for &component in components {
            self.counts[component as usize] += 1.0;
        }
    }
}

fn fit_window(
    events: &MultivariateEvents,
    now: TsUs,
    min_events: usize,
) -> Option<(&[f64], &[i64], f64)> {
    let times = events.times_secs();
    if times.len() < min_events {
        return None;
    }
    let end = events.window_end_secs(now)?;
    if end - times[0] <= 0.0 {
        return None;
    }
    Some((times, events.components(), end))
}
