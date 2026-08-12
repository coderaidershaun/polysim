//! Hawkes processes: self- and cross-exciting point-process calculators — O(1)-per-event intensity
//! recursions, closed-form-compensator likelihoods, MLE + EM fitters, seeded simulators.

mod estimation;
mod monitor;
mod multivariate;
mod simulate;
mod univariate;
mod window;

pub use estimation::{
    DiscreteEstimate, DiscreteMle, HawkesEm, HawkesEstimate, HawkesMle, LogisticEstimate,
    LogisticMle, MultivariateEm, MultivariateEstimate,
};
pub use monitor::{HawkesChoice, HawkesSide};
pub use multivariate::{MultivariateEvents, MultivariateHawkes, MultivariateParams};
pub use simulate::{
    DiscreteSimulation, ExpSimulation, Lcg, LogisticSimulation, MultivariateSimulation,
};
pub use univariate::{
    DiscreteCounts, DiscreteParams, HawkesEvents, HawkesParams, LogisticParams, LogisticShape,
    QuadraticParams, UnivariateHawkes,
};
