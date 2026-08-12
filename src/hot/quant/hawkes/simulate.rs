//! Seeded simulators: paths for fitter measurement. House LCG (no rand). f64 time internal,
//! rounds only on output -> µs quantisation doesn't feedback.

use super::multivariate::MultivariateParams;
use super::univariate::{DiscreteParams, HawkesParams, LogisticParams};
use crate::time::{DurationUs, TsUs};

const MAX_POISSON_MEAN: f64 = 1e6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lcg(pub u64);

impl Lcg {
    pub fn unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (((self.0 >> 11) as f64) / ((1u64 << 53) as f64)).max(1e-12)
    }

    pub fn exp_draw(&mut self, rate: f64) -> f64 {
        -self.unit().ln() / rate
    }

    pub fn poisson(&mut self, lambda: f64) -> u64 {
        if lambda.is_nan() || lambda <= 0.0 {
            return 0;
        }
        let lambda = lambda.min(MAX_POISSON_MEAN);
        let mut count = 0;
        let mut accumulated = 0.0;
        loop {
            accumulated -= self.unit().ln();
            if accumulated > lambda {
                return count;
            }
            count += 1;
        }
    }
}

/// Ogata thinning: intensity monotone -> current-time bounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExpSimulation {
    pub params: HawkesParams,
    pub start_ts: TsUs,
    pub horizon: DurationUs,
    pub seed: u64,
    /// Hard cap (designed event).
    pub max_events: usize,
}

impl ExpSimulation {
    pub fn run(&self) -> Vec<TsUs> {
        let HawkesParams { mu, alpha, beta } = self.params;
        let horizon = self.horizon.to_secs();
        let mut generator = Lcg(self.seed);
        let mut times = Vec::new();
        let mut elapsed = 0.0;
        let mut excitation = 0.0;
        while times.len() < self.max_events {
            let bound = mu + alpha * excitation;
            let wait = generator.exp_draw(bound);
            elapsed += wait;
            if elapsed > horizon {
                break;
            }
            excitation *= (-beta * wait).exp();
            if generator.unit() <= (mu + alpha * excitation) / bound {
                times.push(elapsed);
                excitation += 1.0;
            }
        }
        stamps(self.start_ts, &times)
    }
}

/// Logistic kernel: monotone decay, current-time bound, O(W).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogisticSimulation {
    pub params: LogisticParams,
    pub start_ts: TsUs,
    pub horizon: DurationUs,
    pub seed: u64,
    pub max_events: usize,
}

impl LogisticSimulation {
    pub fn run(&self) -> Vec<TsUs> {
        let horizon = self.horizon.to_secs();
        let cut = self.params.tail_cut(horizon);
        let mut generator = Lcg(self.seed);
        let mut times: Vec<f64> = Vec::new();
        let mut elapsed = 0.0;
        let mut dropped = 0usize;
        while times.len() < self.max_events {
            let bound = self.rate(&times, dropped, elapsed);
            let wait = generator.exp_draw(bound);
            elapsed += wait;
            if elapsed > horizon {
                break;
            }
            while dropped < times.len() && elapsed - times[dropped] > cut {
                dropped += 1;
            }
            if generator.unit() <= self.rate(&times, dropped, elapsed) / bound {
                times.push(elapsed);
            }
        }
        stamps(self.start_ts, &times)
    }

    fn rate(&self, times: &[f64], dropped: usize, elapsed: f64) -> f64 {
        let mut rate = self.params.mu;
        for &time in &times[dropped..] {
            rate += self.params.kernel(elapsed - time);
        }
        rate
    }
}

/// Thinning: accepted candidate -> component k with prob λ_k/Λ.
#[derive(Debug, Clone, PartialEq)]
pub struct MultivariateSimulation {
    pub params: MultivariateParams,
    pub start_ts: TsUs,
    pub horizon: DurationUs,
    pub seed: u64,
    /// Hard cap (designed event).
    pub max_events: usize,
}

impl MultivariateSimulation {
    pub fn run(&self) -> Vec<(TsUs, usize)> {
        let dimension = self.params.dimension();
        let mut excitation = vec![0.0; dimension * dimension];
        let mut rates = vec![0.0; dimension];
        let mut generator = Lcg(self.seed);
        let mut path = Vec::new();
        let horizon = self.horizon.to_secs();
        let mut elapsed = 0.0;
        while path.len() < self.max_events {
            let bound = self.rates_into(&excitation, &mut rates);
            let wait = generator.exp_draw(bound);
            elapsed += wait;
            if elapsed > horizon {
                break;
            }
            for (state, decay) in excitation.iter_mut().zip(&self.params.beta) {
                *state *= (-decay * wait).exp();
            }
            let total = self.rates_into(&excitation, &mut rates);
            if generator.unit() > total / bound {
                continue;
            }
            let component = pick(&rates, generator.unit() * total);
            for target in 0..dimension {
                excitation[target * dimension + component] += 1.0;
            }
            path.push((
                self.start_ts + DurationUs::from_micros((elapsed * 1e6).round() as i64),
                component,
            ));
        }
        path
    }

    fn rates_into(&self, excitation: &[f64], rates: &mut [f64]) -> f64 {
        let dimension = self.params.dimension();
        let mut total = 0.0;
        for (target, rate) in rates.iter_mut().enumerate() {
            let row = target * dimension;
            *rate = self.params.mu[target];
            for source in 0..dimension {
                *rate += self.params.alpha[row + source] * excitation[row + source];
            }
            total += *rate;
        }
        total
    }
}

fn pick(weights: &[f64], draw: f64) -> usize {
    let mut cumulative = 0.0;
    for (component, weight) in weights.iter().enumerate() {
        cumulative += weight;
        if draw <= cumulative {
            return component;
        }
    }
    weights.len() - 1
}

fn stamps(start: TsUs, seconds: &[f64]) -> Vec<TsUs> {
    seconds
        .iter()
        .map(|secs| start + DurationUs::from_micros((secs * 1e6).round() as i64))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiscreteSimulation {
    pub params: DiscreteParams,
    pub bins: usize,
    pub seed: u64,
}

impl DiscreteSimulation {
    pub fn run(&self) -> Vec<u32> {
        let DiscreteParams {
            mu,
            amplitude,
            decay,
            memory,
        } = self.params;
        let dropped_weight = decay.powi(memory as i32 + 1);
        let mut generator = Lcg(self.seed);
        let mut counts: Vec<u32> = Vec::with_capacity(self.bins);
        let mut excitation = 0.0;
        for index in 0..self.bins {
            let drawn = generator.poisson(mu + amplitude * excitation);
            counts.push(u32::try_from(drawn).unwrap_or(u32::MAX));
            let dropped = if index >= memory { f64::from(counts[index - memory]) } else { 0.0 };
            excitation = decay * (excitation + drawn as f64) - dropped_weight * dropped;
        }
        counts
    }
}
