//! Zero-allocation fitness for the Hawkes package: once the windows and the warm-start caches
//! exist, streaming events into the three accumulators and refitting all five fitters touches the
//! allocator not at all. Every cold fit is primed OUTSIDE the measured window, because a cold fit is
//! an initialisation event, not steady state.
//!
//! Liveness counters ride alongside the allocation assertion: a fitter that quietly stopped
//! producing estimates would satisfy "allocated nothing" perfectly, so each must be shown to have
//! issued estimates AND to have converged at least once inside the measured window.

use polysim::hot::quant::hawkes::{
    DiscreteCounts, DiscreteMle, DiscreteParams, DiscreteSimulation, ExpSimulation, HawkesEm,
    HawkesEvents, HawkesMle, HawkesParams, LogisticMle, LogisticShape, MultivariateEm,
    MultivariateEvents, MultivariateParams, MultivariateSimulation,
};
use polysim::time::{DurationUs, TsUs};

const START: TsUs = TsUs::from_micros(0);

/// Rolling window every fitter reads. Shorter than a production window on purpose: the logistic
/// simplex and the multivariate EM both scale with it, and what this test proves — no allocation,
/// live fits — is independent of window length, so the suite buys nothing by being slower.
const WINDOW: usize = 1024;

/// Pushes before the measured region: enough to fill the windows twice over, so the fits inside the
/// region see a fully-populated, already-evicting buffer.
const PRIME: usize = 2 * WINDOW;

/// Pushes inside the measured region.
const MEASURED: usize = 50_000;

/// Pushes between refits: 20 rounds of all five fitters across the region. Each round's window has
/// turned over more than twice, so no refit is trivially warm on unchanged data.
const FIT_STRIDE: usize = 2_500;

/// Estimates and convergences a fitter produced inside the measured region.
#[derive(Default)]
struct Liveness {
    estimates: u64,
    converged: u64,
}

impl Liveness {
    fn record(&mut self, converged: bool) {
        self.estimates += 1;
        self.converged += u64::from(converged);
    }

    fn assert_live(&self, name: &str) {
        assert!(
            self.estimates > 0,
            "{name} produced no estimate in the measured window"
        );
        assert!(
            self.converged > 0,
            "{name} never converged in the measured window"
        );
    }
}

/// Repeats a simulated path end-to-end, shifting each lap past the previous one so the stamps stay
/// non-decreasing — an out-of-order stamp would be clamped and collapse the fit window's span.
fn tile(path: &[TsUs], wanted: usize) -> Vec<TsUs> {
    let lap = path[path.len() - 1].micros() - path[0].micros() + 1_000_000;
    (0..wanted)
        .map(|index| {
            TsUs::from_micros(path[index % path.len()].micros() + (index / path.len()) as i64 * lap)
        })
        .collect()
}

fn exponential_path() -> Vec<TsUs> {
    ExpSimulation {
        params: HawkesParams::new(0.5, 0.8, 2.0),
        start_ts: START,
        horizon: DurationUs::from_secs(20_000),
        seed: 0xF17E_5501,
        max_events: 40_000,
    }
    .run()
}

fn cross_excited_path() -> Vec<(TsUs, usize)> {
    MultivariateSimulation {
        params: MultivariateParams::new(
            vec![0.3, 0.3],
            vec![0.2, 1.2, 0.1, 0.2],
            vec![2.0, 2.0, 2.0, 2.0],
        ),
        start_ts: START,
        horizon: DurationUs::from_secs(20_000),
        seed: 0xF17E_5502,
        max_events: 40_000,
    }
    .run()
}

#[test]
fn hawkes_accumulators_and_warm_fits_do_not_allocate() {
    let univariate_stamps = tile(&exponential_path(), PRIME + MEASURED);
    let cross = cross_excited_path();
    let components: Vec<usize> = cross.iter().map(|(_, component)| *component).collect();
    let cross_stamps = tile(
        &cross.iter().map(|(stamp, _)| *stamp).collect::<Vec<_>>(),
        PRIME + MEASURED,
    );
    let bins = DiscreteSimulation {
        params: DiscreteParams::new(2.0, 1.0, 0.3, 3),
        bins: 4096,
        seed: 0xF17E_5503,
    }
    .run();

    let mut events = HawkesEvents::new(WINDOW);
    let mut cross_events = MultivariateEvents::new(2, WINDOW);
    let mut counts = DiscreteCounts::new(2048);
    let mut mle = HawkesMle::new(64);
    let mut logistic = LogisticMle::new(
        64,
        LogisticShape {
            theta: 3.0,
            delta: 4.0,
        },
    );
    let mut em = HawkesEm::new(64);
    let mut cross_em = MultivariateEm::new(2, 64);
    let mut discrete = DiscreteMle::new(3, 64);

    for index in 0..PRIME {
        events.push(univariate_stamps[index]);
        cross_events.push(cross_stamps[index], components[index % components.len()]);
        counts.push(bins[index % bins.len()]);
    }
    // Cold fits: allocation is allowed here, and every warm-start cache is populated by them.
    let univariate_now = univariate_stamps[PRIME - 1];
    let cross_now = cross_stamps[PRIME - 1];
    mle.fit(&events, univariate_now).expect("cold mle fit");
    logistic
        .fit(&events, univariate_now)
        .expect("cold logistic fit");
    em.fit(&events, univariate_now).expect("cold em fit");
    cross_em
        .fit(&cross_events, cross_now)
        .expect("cold multivariate fit");
    discrete.fit(&counts).expect("cold discrete fit");

    let mut mle_fits = Liveness::default();
    let mut logistic_fits = Liveness::default();
    let mut em_fits = Liveness::default();
    let mut cross_fits = Liveness::default();
    let mut discrete_fits = Liveness::default();

    let before = crate::alloc_count();
    for index in PRIME..PRIME + MEASURED {
        events.push(univariate_stamps[index]);
        cross_events.push(cross_stamps[index], components[index % components.len()]);
        counts.push(bins[index % bins.len()]);
        if index % FIT_STRIDE != 0 {
            continue;
        }
        let now = univariate_stamps[index];
        let cross_now = cross_stamps[index];
        if let Some(estimate) = mle.fit(&events, now) {
            mle_fits.record(estimate.converged);
        }
        if let Some(estimate) = logistic.fit(&events, now) {
            logistic_fits.record(estimate.converged);
        }
        if let Some(estimate) = em.fit(&events, now) {
            em_fits.record(estimate.converged);
        }
        if let Some(estimate) = cross_em.fit(&cross_events, cross_now) {
            cross_fits.record(estimate.converged);
        }
        if let Some(estimate) = discrete.fit(&counts) {
            discrete_fits.record(estimate.converged);
        }
    }
    let after = crate::alloc_count();

    assert_eq!(
        after, before,
        "hawkes accumulators or warm fits allocated in steady state"
    );
    mle_fits.assert_live("hawkes mle");
    logistic_fits.assert_live("logistic mle");
    em_fits.assert_live("hawkes em");
    cross_fits.assert_live("multivariate em");
    discrete_fits.assert_live("discrete mle");

    // The windows really were evicting throughout, so the fits ran on a sliding buffer rather than
    // a frozen one that would have made every refit trivially warm.
    assert_eq!(events.len(), WINDOW);
    assert_eq!(cross_events.len(), WINDOW);
    assert_eq!(events.out_of_order_count(), 0);
    assert_eq!(cross_events.out_of_order_count(), 0);
}
