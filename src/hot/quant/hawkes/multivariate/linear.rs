//! Linear cross-exciting exponential kernels. O(d²) per event, O(d) per query, no history scan.

use super::MultivariateEvents;
use crate::time::{DurationUs, TsUs};

// Fixed iterations -> predictable cost.
const POWER_ITERATIONS: usize = 64;

/// `d` baselines plus `d×d` excitation and per-pair decay, row-major: `alpha[k*d + j]` is the effect
/// of a component-`j` event on component `k`.
///
/// Kernel convention per pair: `h_kj(u) = alpha_kj·e^{-beta_kj·u}`, so the branching matrix is
/// `alpha_kj/beta_kj` — as in the univariate case, other software may fold `beta` into the jump;
/// check before comparing estimates.
#[derive(Debug, Clone, PartialEq)]
pub struct MultivariateParams {
    pub mu: Vec<f64>,
    pub alpha: Vec<f64>,
    pub beta: Vec<f64>,
}

impl MultivariateParams {
    /// # Panics
    /// Empty `mu`, alpha/beta not d×d, non-finite or out-of-range values.
    pub fn new(mu: Vec<f64>, alpha: Vec<f64>, beta: Vec<f64>) -> Self {
        let dimension = mu.len();
        assert!(
            dimension != 0,
            "multivariate hawkes needs at least one component"
        );
        assert!(
            alpha.len() == dimension * dimension && beta.len() == dimension * dimension,
            "multivariate hawkes needs {}x{} alpha and beta, got {} and {}",
            dimension,
            dimension,
            alpha.len(),
            beta.len()
        );
        for baseline in &mu {
            assert!(
                baseline.is_finite() && *baseline > 0.0,
                "multivariate hawkes baseline must be finite and positive, got {baseline}"
            );
        }
        for jump in &alpha {
            assert!(
                jump.is_finite() && *jump >= 0.0,
                "multivariate hawkes jump must be finite and non-negative, got {jump}"
            );
        }
        for decay in &beta {
            assert!(
                decay.is_finite() && *decay > 0.0,
                "multivariate hawkes decay must be finite and positive, got {decay}"
            );
        }
        Self { mu, alpha, beta }
    }

    #[inline]
    pub fn dimension(&self) -> usize {
        self.mu.len()
    }

    /// Perron root of branching matrix Γ_kj = alpha_kj/beta_kj. Caller-owned scratch -> zero alloc.
    /// # Panics
    /// `scratch.len() < 2 * dimension`.
    pub fn spectral_radius(&self, scratch: &mut [f64]) -> f64 {
        let dimension = self.dimension();
        assert!(
            scratch.len() >= 2 * dimension,
            "spectral radius scratch needs {} slots, got {}",
            2 * dimension,
            scratch.len()
        );
        let (vector, rest) = scratch.split_at_mut(dimension);
        let next = &mut rest[..dimension];
        vector.fill((dimension as f64).recip());

        let mut radius = 0.0;
        for _ in 0..POWER_ITERATIONS {
            let mut mass = 0.0;
            for (target, image) in next.iter_mut().enumerate() {
                let row = target * dimension;
                let mut sum = 0.0;
                for (source, weight) in vector.iter().enumerate() {
                    sum += (self.alpha[row + source] / self.beta[row + source]) * weight;
                }
                *image = sum;
                mass += sum;
            }
            if mass <= 0.0 {
                return 0.0;
            }
            for (component, image) in vector.iter_mut().zip(next.iter()) {
                *component = image / mass;
            }
            radius = mass;
        }
        radius
    }

    pub fn is_stationary(&self, scratch: &mut [f64]) -> bool {
        self.spectral_radius(scratch) < 1.0
    }
}

/// Live evaluator, pair states in d² array, sized once at construction.
#[derive(Debug, Clone, PartialEq)]
pub struct MultivariateHawkes {
    params: MultivariateParams,
    excitation: Vec<f64>,
    last_ts: Option<TsUs>,
}

impl MultivariateHawkes {
    pub fn new(params: MultivariateParams) -> Self {
        let cells = params.dimension() * params.dimension();
        Self {
            params,
            excitation: vec![0.0; cells],
            last_ts: None,
        }
    }

    #[inline]
    pub fn params(&self) -> &MultivariateParams {
        &self.params
    }

    /// Keeps the decayed excitation under the new `beta`: the mismatch is a transient that decays at
    /// rate `beta`. Call [`MultivariateHawkes::reseed_from`] after a refit when exact state matters.
    ///
    /// # Panics
    /// A dimension change — the evaluator's state array is sized once (wiring bug).
    pub fn set_params(&mut self, params: MultivariateParams) {
        assert!(
            params.dimension() == self.params.dimension(),
            "evaluator built for {} components, got {}",
            self.params.dimension(),
            params.dimension()
        );
        self.params = params;
    }

    /// O(n·d²), once per refit, off the per-event path.
    /// # Panics
    /// `events.dimension() != self.params.dimension()` — wiring bug.
    pub fn reseed_from(&mut self, events: &MultivariateEvents) {
        assert!(
            events.dimension() == self.params.dimension(),
            "evaluator built for {} components, got a {}-component window",
            self.params.dimension(),
            events.dimension()
        );
        let Some(last_ts) = events.last_ts() else {
            self.clear();
            return;
        };
        let times = events.times_secs();
        self.excitation.fill(0.0);
        let mut previous = times[0];
        for (&time, &component) in times.iter().zip(events.components()) {
            self.decay(time - previous);
            previous = time;
            self.bump(component as usize);
        }
        self.last_ts = Some(last_ts);
    }

    /// # Panics
    /// `component >= dimension` — wiring bug.
    pub fn on_event(&mut self, ts: TsUs, component: usize) {
        assert!(
            component < self.params.dimension(),
            "component {component} outside the {}-component process",
            self.params.dimension()
        );
        if let Some(last) = self.last_ts {
            self.decay(ts.diff(last).to_secs().max(0.0));
        }
        self.bump(component);
        self.last_ts = Some(ts);
    }

    /// # Panics
    /// `component >= dimension` — wiring bug.
    pub fn intensity(&self, now: TsUs, component: usize) -> f64 {
        assert!(
            component < self.params.dimension(),
            "component {component} outside the {}-component process",
            self.params.dimension()
        );
        self.row_intensity(component, self.elapsed(now))
    }

    pub fn total_intensity(&self, now: TsUs) -> f64 {
        let elapsed = self.elapsed(now);
        (0..self.params.dimension())
            .map(|target| self.row_intensity(target, elapsed))
            .sum()
    }

    /// # Panics
    /// `out.len() != dimension` — wiring bug.
    pub fn intensities_into(&self, now: TsUs, out: &mut [f64]) {
        assert!(
            out.len() == self.params.dimension(),
            "intensity output needs {} slots, got {}",
            self.params.dimension(),
            out.len()
        );
        let elapsed = self.elapsed(now);
        for (target, rate) in out.iter_mut().enumerate() {
            *rate = self.row_intensity(target, elapsed);
        }
    }

    /// Forward count per-target (cross arrivals excluded, no per-component arrival prob). Closed form, O(d).
    /// # Panics
    /// `component >= dimension` — wiring bug.
    pub fn expected_events(&self, now: TsUs, horizon: DurationUs, component: usize) -> f64 {
        let dimension = self.params.dimension();
        assert!(
            component < dimension,
            "component {component} outside the {dimension}-component process"
        );
        let span = horizon.to_secs().max(0.0);
        let elapsed = self.elapsed(now);
        let row = component * dimension;
        let mut total = self.params.mu[component] * span;
        for source in 0..dimension {
            let cell = row + source;
            let beta = self.params.beta[cell];
            let decayed = (-beta * elapsed).exp() * self.excitation[cell];
            total += (self.params.alpha[cell] / beta) * decayed * (1.0 - (-beta * span).exp());
        }
        total
    }

    pub fn clear(&mut self) {
        self.excitation.fill(0.0);
        self.last_ts = None;
    }

    #[inline]
    fn elapsed(&self, now: TsUs) -> f64 {
        self.last_ts
            .map_or(0.0, |last| now.diff(last).to_secs().max(0.0))
    }

    fn row_intensity(&self, target: usize, elapsed: f64) -> f64 {
        let dimension = self.params.dimension();
        let row = target * dimension;
        let mut rate = self.params.mu[target];
        for source in 0..dimension {
            rate += self.params.alpha[row + source]
                * (-self.params.beta[row + source] * elapsed).exp()
                * self.excitation[row + source];
        }
        rate
    }

    fn decay(&mut self, delta: f64) {
        for (state, decay) in self.excitation.iter_mut().zip(&self.params.beta) {
            *state *= (-decay * delta).exp();
        }
    }

    fn bump(&mut self, source: usize) {
        let dimension = self.params.dimension();
        for target in 0..dimension {
            self.excitation[target * dimension + source] += 1.0;
        }
    }
}
