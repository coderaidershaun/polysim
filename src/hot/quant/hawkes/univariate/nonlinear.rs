//! Nonlinear kernels: quadratic (rescaled exponential) and logistic.
//! Logistic CENTRED: `psi(s) = phi(alpha·e^{-beta·s}) - phi(0)` decays to zero; avoids permanent floor.
//! 16-node Gauss-Legendre in excitation-level space (fixed nodes fail on time-domain spike).
//! Tail-cut at `s_cut` costs ≤ [`TAIL_EPS`] per event -> O(W) complexity.

use super::{HawkesEvents, HawkesParams};
use crate::hot::quant::MIN_RATE;
use crate::time::TsUs;

/// Tail tolerance — max intensity error from dropping one event beyond `s_cut`.
pub(crate) const TAIL_EPS: f64 = 1e-9;

pub(crate) const GAUSS_LEGENDRE_NODES: [f64; 16] = [
    -0.9894009349916499,
    -0.9445750230732326,
    -0.8656312023878318,
    -0.755404408355003,
    -0.6178762444026438,
    -0.4580167776572274,
    -0.2816035507792589,
    -0.09501250983763744,
    0.09501250983763744,
    0.2816035507792589,
    0.4580167776572274,
    0.6178762444026438,
    0.755404408355003,
    0.8656312023878318,
    0.9445750230732326,
    0.9894009349916499,
];

pub(crate) const GAUSS_LEGENDRE_WEIGHTS: [f64; 16] = [
    0.027152459411754096,
    0.062253523938647894,
    0.09515851168249279,
    0.12462897125553388,
    0.14959598881657674,
    0.16915651939500254,
    0.18260341504492359,
    0.1894506104550685,
    0.1894506104550685,
    0.18260341504492359,
    0.16915651939500254,
    0.14959598881657674,
    0.12462897125553388,
    0.09515851168249279,
    0.062253523938647894,
    0.027152459411754096,
];

/// Quadratic nonlinearity `phi(x) = gamma·x²` — each event contributes exponential `gamma·alpha²·e^{-2beta·τ}`.
/// Over-parameterised: likelihood invariant under `(alpha, gamma) -> (c·alpha, gamma/c²)` -> gamma fixed gauge, never fitted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuadraticParams {
    pub mu: f64,
    pub alpha: f64,
    pub beta: f64,
    pub gamma: f64,
}

impl QuadraticParams {
    /// # Panics
    /// Non-finite, `mu <= 0`, `alpha < 0`, `beta <= 0`, or `gamma <= 0` (config bug).
    pub fn new(mu: f64, alpha: f64, beta: f64, gamma: f64) -> Self {
        assert!(
            gamma.is_finite() && gamma > 0.0,
            "quadratic gauge must be finite and positive, got {gamma}"
        );
        let linear = HawkesParams::new(mu, alpha, beta);
        Self {
            mu: linear.mu,
            alpha: linear.alpha,
            beta: linear.beta,
            gamma,
        }
    }

    /// Identical linear model (compensator/MLE/simulation all use this).
    pub fn to_linear(&self) -> HawkesParams {
        HawkesParams::new(
            self.mu,
            self.gamma * self.alpha * self.alpha,
            2.0 * self.beta,
        )
    }
}

/// Steepness `theta` and inflection `delta` of sigmoid `phi(x) = 1/(1+e^{-theta(x-delta)})`.
/// Kernel applies phi centred -> event contribution decays to zero. Fixed from config (unidentifiable with alpha).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogisticShape {
    pub theta: f64,
    pub delta: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogisticParams {
    pub mu: f64,
    pub alpha: f64,
    pub beta: f64,
    pub shape: LogisticShape,
}

impl LogisticParams {
    /// # Panics
    /// Non-finite, `mu <= 0`, `alpha < 0`, `beta <= 0`, or `shape.theta <= 0` (config bug).
    pub fn new(mu: f64, alpha: f64, beta: f64, shape: LogisticShape) -> Self {
        assert!(
            shape.theta.is_finite() && shape.theta > 0.0 && shape.delta.is_finite(),
            "logistic shape must be finite with positive steepness, got theta={} delta={}",
            shape.theta,
            shape.delta
        );
        let linear = HawkesParams::new(mu, alpha, beta);
        Self {
            mu: linear.mu,
            alpha: linear.alpha,
            beta: linear.beta,
            shape,
        }
    }

    /// O(W) where W = events within tail cut; older events dropped (contribution < [`TAIL_EPS`]).
    pub fn intensity(&self, events: &HawkesEvents, now: TsUs) -> f64 {
        let Some(end) = events.window_end_secs(now) else {
            return self.mu;
        };
        let times = events.times_secs();
        let cut = self.tail_cut(end - times[0]);
        let mut rate = self.mu;
        for &time in times.iter().rev() {
            if end - time > cut {
                break;
            }
            rate += self.kernel(end - time);
        }
        rate
    }

    /// ∫_0^∞ psi(s) ds = expected direct offspring per event; >= 1 is explosive.
    pub fn branching_ratio(&self) -> f64 {
        tail_integral(self, f64::INFINITY)
    }

    pub fn is_stationary(&self) -> bool {
        self.branching_ratio() < 1.0
    }

    /// Event's centred contribution `psi(lag) = phi(alpha·e^{-beta·lag}) - phi(0)`; via excess() to avoid cancellation.
    #[inline]
    pub(crate) fn kernel(&self, lag: f64) -> f64 {
        excess(self.shape, self.alpha * (-self.beta * lag).exp())
    }

    /// Lag beyond which event contribution is dropped (clamped to window span).
    pub(crate) fn tail_cut(&self, span: f64) -> f64 {
        let raw = (self.shape.theta * self.alpha / (4.0 * TAIL_EPS)).ln() / self.beta;
        if raw.is_finite() { raw.clamp(0.0, span.max(0.0)) } else { 0.0 }
    }
}

#[inline]
fn logistic(shape: LogisticShape, x: f64) -> f64 {
    1.0 / (1.0 + (-shape.theta * (x - shape.delta)).exp())
}

/// Σ_i ln λ(tau_i) — tail boundary advances monotonically.
pub(crate) fn logistic_log_intensity_sum(params: &LogisticParams, times: &[f64]) -> f64 {
    let Some(&start) = times.first() else {
        return 0.0;
    };
    let cut = params.tail_cut(times[times.len() - 1] - start);
    let mut oldest = 0usize;
    let mut total = 0.0;
    for (index, &time) in times.iter().enumerate() {
        while time - times[oldest] > cut {
            oldest += 1;
        }
        let mut rate = params.mu;
        for &earlier in &times[oldest..index] {
            rate += params.kernel(time - earlier);
        }
        total += rate.max(MIN_RATE).ln();
    }
    total
}

/// ∫_w^T λ(s) ds; cut-integral evaluated once, reused for events past cut.
pub(crate) fn logistic_compensator(params: &LogisticParams, times: &[f64], end: f64) -> f64 {
    let Some(&start) = times.first() else {
        return 0.0;
    };
    let cut = params.tail_cut(end - start);
    let cut_integral = tail_integral(params, cut);
    let mut total = params.mu * (end - start);
    for &time in times {
        let elapsed = (end - time).max(0.0);
        total += if elapsed >= cut { cut_integral } else { tail_integral(params, elapsed) };
    }
    total
}

/// ∫_0^upper psi(s) ds via 16-node Gauss-Legendre.
/// Substitutes x = alpha·e^{-beta·s}: in s spike over dead tail, in x smooth+bounded -> quadrature excels.
fn tail_integral(params: &LogisticParams, upper: f64) -> f64 {
    if upper <= 0.0 || params.alpha <= 0.0 {
        return 0.0;
    }
    let lowest = params.alpha * (-params.beta * upper).exp();
    let half = 0.5 * (params.alpha - lowest);
    let centre = 0.5 * (params.alpha + lowest);
    let mut total = 0.0;
    for (node, weight) in GAUSS_LEGENDRE_NODES.iter().zip(GAUSS_LEGENDRE_WEIGHTS) {
        let level = centre + half * node;
        total += weight * excess(params.shape, level) / level;
    }
    half * total / params.beta
}

/// Centred logistic `phi(x) - phi(0)` — avoids cancellation near innermost quadrature nodes.
fn excess(shape: LogisticShape, x: f64) -> f64 {
    let offset = (shape.theta * shape.delta).exp();
    if !offset.is_finite() {
        return logistic(shape, x) - logistic(shape, 0.0);
    }
    let decayed = (-shape.theta * x).exp();
    -offset * (-shape.theta * x).exp_m1() / ((1.0 + offset * decayed) * (1.0 + offset))
}
