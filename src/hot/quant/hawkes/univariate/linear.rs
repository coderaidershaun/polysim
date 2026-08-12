//! Linear exponential-kernel Hawkes. Scalar excitation -> O(1) per event/query (no history scan).

use super::HawkesEvents;
use crate::time::{DurationUs, TsUs};

/// Exponential kernel, rates per second. Branching ratio = alpha/beta.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HawkesParams {
    pub mu: f64,
    pub alpha: f64,
    pub beta: f64,
}

impl HawkesParams {
    /// # Panics
    /// Non-finite, mu <= 0, alpha < 0, or beta <= 0.
    pub fn new(mu: f64, alpha: f64, beta: f64) -> Self {
        assert!(
            mu.is_finite() && alpha.is_finite() && beta.is_finite(),
            "hawkes params must be finite, got mu={mu} alpha={alpha} beta={beta}"
        );
        assert!(mu > 0.0, "hawkes baseline must be positive, got {mu}");
        assert!(
            alpha >= 0.0,
            "hawkes jump must be non-negative, got {alpha}"
        );
        assert!(beta > 0.0, "hawkes decay must be positive, got {beta}");
        Self { mu, alpha, beta }
    }

    #[inline]
    pub fn branching_ratio(&self) -> f64 {
        self.alpha / self.beta
    }

    #[inline]
    pub fn is_stationary(&self) -> bool {
        self.branching_ratio() < 1.0
    }

    #[inline]
    pub fn half_life_secs(&self) -> f64 {
        core::f64::consts::LN_2 / self.beta
    }

    pub fn long_run_rate(&self) -> Option<f64> {
        self.is_stationary()
            .then(|| self.mu / (1.0 - self.branching_ratio()))
    }
}

/// Live evaluator: (last stamp, excitation), no history scan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnivariateHawkes {
    params: HawkesParams,
    last_ts: Option<TsUs>,
    excitation: f64,
}

impl UnivariateHawkes {
    pub fn new(params: HawkesParams) -> Self {
        Self {
            params,
            last_ts: None,
            excitation: 0.0,
        }
    }

    #[inline]
    pub fn params(&self) -> HawkesParams {
        self.params
    }

    pub fn set_params(&mut self, params: HawkesParams) {
        self.params = params;
    }

    /// O(n), once per refit, off the per-event path.
    pub fn reseed_from(&mut self, events: &HawkesEvents) {
        let Some(last_ts) = events.last_ts() else {
            self.clear();
            return;
        };
        let times = events.times_secs();
        let mut excitation = 0.0;
        let mut previous = times[0];
        for &time in times {
            excitation = (-self.params.beta * (time - previous)).exp() * excitation + 1.0;
            previous = time;
        }
        self.excitation = excitation;
        self.last_ts = Some(last_ts);
    }

    #[inline]
    pub fn on_event(&mut self, ts: TsUs) {
        self.excitation = match self.last_ts {
            Some(last) => {
                (-self.params.beta * ts.diff(last).to_secs().max(0.0)).exp() * self.excitation + 1.0
            }
            None => 1.0,
        };
        self.last_ts = Some(ts);
    }

    #[inline]
    pub fn intensity(&self, now: TsUs) -> f64 {
        let Some(last) = self.last_ts else {
            return self.params.mu;
        };
        let elapsed = now.diff(last).to_secs().max(0.0);
        self.params.mu + self.params.alpha * (-self.params.beta * elapsed).exp() * self.excitation
    }

    /// Forward count per-quote (offspring excluded). Closed form, O(1).
    pub fn expected_events(&self, now: TsUs, horizon: DurationUs) -> f64 {
        let HawkesParams { mu, alpha, beta } = self.params;
        let span = horizon.to_secs().max(0.0);
        let decayed = match self.last_ts {
            Some(last) => (-beta * now.diff(last).to_secs().max(0.0)).exp() * self.excitation,
            None => 0.0,
        };
        mu * span + (alpha / beta) * decayed * (1.0 - (-beta * span).exp())
    }

    /// P(≥1 event). Exact: intensity monotone decays on no-event path.
    pub fn event_probability(&self, now: TsUs, horizon: DurationUs) -> f64 {
        1.0 - (-self.expected_events(now, horizon)).exp()
    }

    pub fn clear(&mut self) {
        self.last_ts = None;
        self.excitation = 0.0;
    }
}
