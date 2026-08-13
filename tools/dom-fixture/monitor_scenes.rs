//! Deterministic monitor scenes: ONLY place fabricated data lives.

use polysim::desktop::monitor::{Channel, MonitorUiState};
use polysim::desktop::monitor_model::SystemNote;
use polysim::hot::exec::{CloseReason, OrderState, RejectOrigin};
use polysim::ids::Side;
use polysim::msg::exec::RejectClass;
use polysim::msg::ui::UiBookState;

use crate::monitor_feed::{Feed, MonitorScene, QUOTE_QTY_MILLI, SPIN};

pub fn monitor_scenes() -> Vec<MonitorScene> {
    vec![full(), warmup(), long_values(), unseen()]
}

/// A live monitor: valid book + quote (BID Δ2 · MID 65992 · ASK Δ3), features with a recently-changed
/// row, a stale row, a legitimate zero and normal rows, and content in all four channels.
fn full() -> MonitorScene {
    let mut feed = Feed::new(1, FULL_FEATURES.len());
    feed.lifecycle(SystemNote::Starting);
    feed.lifecycle(SystemNote::Ready);

    // Reset -> System carries transitions.
    feed.book(
        UiBookState::Valid,
        1_000_000,
        &[(65990, 640)],
        &[(65994, 512)],
    );

    feed.feature(2, 1_000_000, 2.0000);
    feed.feature(3, 1_000_000, 0.7431);
    feed.feature(4, 1_000_000, 0.6120);
    feed.feature(5, 1_000_000, -0.0004);
    feed.feature(6, 1_000_000, 0.3300);
    feed.feature(7, 1_000_000, 0.4100);
    feed.feature(8, 1_000_000, 12.5000);
    feed.feature(9, 1_000_000, 1.8000);
    feed.feature(10, 1_000_000, 0.9200);
    feed.feature(11, 1_050_000, 0.0000);
    // Stale: -7 spins.
    feed.feature(1, 600_000, 42.0000);

    feed.trade(1_100_000, Side::Buy, 65994, 512);
    feed.book(
        UiBookState::AwaitingSnapshot,
        1_100_000,
        &[(65990, 640)],
        &[(65994, 512)],
    );
    // Confirmed bid + in-flight ask: two distinguishable states on screen together.
    feed.order(1_200_000, 1, Side::Buy, 65990, OrderState::Live);
    feed.order(1_200_000, 2, Side::Sell, 65995, OrderState::PendingNew);
    feed.trade(1_150_000, Side::Sell, 65990, 1_240);

    // Changed feature: re-emitted with different value at freshest time.
    feed.feature(0, 1_290_000, -0.5000);
    feed.feature(0, 1_300_000, -0.8000);
    feed.book(
        UiBookState::Valid,
        1_300_000,
        &[(65990, 640)],
        &[(65994, 512)],
    );
    feed.quote(1_300_000, Some(65990), Some(65995));

    feed.fill(1_260_000, Side::Buy, 65990, QUOTE_QTY_MILLI);
    feed.fill(1_280_000, Side::Sell, 65995, QUOTE_QTY_MILLI);
    feed.order(
        1_260_000,
        1,
        Side::Buy,
        65990,
        OrderState::Closed(CloseReason::Filled),
    );
    // Post-only cross: routine for maker, shown as normal not alarm.
    feed.refusal(
        1_265_000,
        Side::Sell,
        RejectOrigin::Venue {
            class: RejectClass::Gone,
            code: -2010,
        },
    );
    feed.rotation(1_290_000);
    feed.trade(1_250_000, Side::Buy, 65995, 88);
    // Deliberate gap -> System shows events-lost note.
    feed.skip(3);
    feed.trade(1_300_000, Side::Sell, 65989, 2_100);

    feed.finish("full live monitor", FULL_FEATURES, MonitorUiState::new())
}

/// Warmup: all values —.
fn warmup() -> MonitorScene {
    let mut feed = Feed::new(1, FULL_FEATURES.len());
    feed.lifecycle(SystemNote::Starting);
    feed.book(
        UiBookState::AwaitingSnapshot,
        1_000_000,
        &[(65990, 640)],
        &[(65994, 512)],
    );
    feed.finish(
        "warmup — all cells --",
        FULL_FEATURES,
        MonitorUiState::new(),
    )
}

/// Long names, extremes: stable.
fn long_values() -> MonitorScene {
    let mut feed = Feed::new(1, LONG_FEATURES.len());
    feed.lifecycle(SystemNote::Ready);
    feed.book(
        UiBookState::Valid,
        1_000_000,
        &[(65990, 640)],
        &[(65994, 512)],
    );
    feed.feature(0, 1_000_000, 99999999.9999);
    feed.feature(1, 1_000_000, -12345678.5000);
    feed.feature(2, 1_000_000, f64::INFINITY);
    feed.feature(3, 1_000_000, f64::NEG_INFINITY);
    feed.feature(4, 1_000_000, f64::NAN);
    feed.feature(5, 1_000_000, 0.0001);
    feed.quote(1_000_000, Some(65990), Some(65995));
    feed.trade(1_000_000, Side::Buy, 65994, 99_999_999);
    feed.finish("long names / values", LONG_FEATURES, MonitorUiState::new())
}

/// Scrolled away: badge shows count.
fn unseen() -> MonitorScene {
    let mut feed = Feed::new(1, FULL_FEATURES.len());
    feed.lifecycle(SystemNote::Starting);
    feed.lifecycle(SystemNote::Ready);
    for step in 0..40 {
        feed.rotation(1_000_000 + step * SPIN.micros());
    }
    let appended = feed.system_appended();
    let mut state = MonitorUiState::new();
    state.active_tab = Channel::System;
    state.set_scrolled_away(Channel::System, appended.saturating_sub(7), 220.0);
    feed.finish("unseen-count | scrolled away", FULL_FEATURES, state)
}

const FULL_FEATURES: &[&str] = &[
    "microprice_offset",
    "queue_ahead_ratio",
    "spread_ticks",
    "hawkes_buy_intensity",
    "hawkes_sell_intensity",
    "kyle_lambda",
    "vpin_short",
    "vpin_long",
    "gueant_a",
    "gueant_k",
    "resilience_ou",
    "net_inventory",
];

const LONG_FEATURES: &[&str] = &[
    "gueant_optimal_half_spread_reservation_offset_ticks",
    "hawkes_multivariate_cross_excitation_decay_estimate",
    "kyle_lambda_signed_dollar_volume_price_impact_slope",
    "vpin_long_bucketed_order_flow_toxicity_running_mean",
    "resilience_ornstein_uhlenbeck_mean_reversion_lambda",
    "microprice",
];
