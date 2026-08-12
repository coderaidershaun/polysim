//! Quant calculator pins: leaf numeric correctness for the estimators feeding the research tape.
//! One module per calculator, each named for the concept rather than the source path.

mod egarch;
mod gueant;
mod hawkes_discrete;
mod hawkes_discrete_estimation;
mod hawkes_em;
mod hawkes_mle;
mod hawkes_multivariate;
mod hawkes_multivariate_em;
mod hawkes_nonlinear;
mod hawkes_simulation;
mod hawkes_univariate;
mod intensity;
mod kyle_feed;
mod kyles_lambda;
mod markouts;
mod microprice;
mod optimise;
mod orderbook_resilience;
mod vpin;
