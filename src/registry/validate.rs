//! Tracker validation at startup: fails loud on traps that would otherwise panic hot thread or void promises.

use crate::config::{
    ConfigError, KlineInterval, SpinField, TrackerSpec, VolumeBarsSpec, VolumeThreshold,
    WindowsSpec,
};
use crate::hot::tracker::TARGET_WINDOW_CANDLES;
use crate::ids::FIXED_SCALE;

const MAX_THRESHOLD_USD: u64 = (i64::MAX / 4) as u64 / FIXED_SCALE as u64;

// Klines target averages trailing 1m volume; source must subscribe to 1m klines + keep closed candles.
pub(super) fn validate_binance_tracker(
    tracker: &TrackerSpec,
    kline_intervals: &[KlineInterval],
) -> Result<(), ConfigError> {
    validate_tracker(tracker)?;
    let is_kline_threshold = tracker
        .volume_bars
        .as_ref()
        .is_some_and(|bars| bars.threshold == VolumeThreshold::Klines);
    if !is_kline_threshold {
        return Ok(());
    }
    if !kline_intervals.contains(&KlineInterval::OneMinute) {
        return Err(tracker_invalid(
            "source.tracker.volume_bars.threshold",
            "klines",
            "kline_intervals to include 1m — a klines target is the mean quote volume of the trailing closed 1m candles",
        ));
    }
    let keep = tracker.candles.as_ref().map_or(0, |candles| candles.keep);
    if keep < TARGET_WINDOW_CANDLES {
        return Err(tracker_invalid(
            "source.tracker.candles.keep",
            &keep.to_string(),
            "at least 1440 alongside a klines volume target — the closed candles ARE the trailing average, so a shorter window silently shortens it",
        ));
    }
    Ok(())
}

// Polymarket: no klines -> no candles. Klines target has nothing to average. Reject both.
pub(super) fn validate_poly_tracker(tracker: &TrackerSpec) -> Result<(), ConfigError> {
    if tracker.candles.is_some() {
        return Err(tracker_invalid(
            "candles",
            "set",
            "unset for polymarket — candles derive from klines, which polymarket lacks",
        ));
    }
    let is_kline_threshold = tracker
        .volume_bars
        .as_ref()
        .is_some_and(|bars| bars.threshold == VolumeThreshold::Klines);
    if is_kline_threshold {
        return Err(tracker_invalid(
            "source.tracker.volume_bars.threshold",
            "klines",
            "a whole-dollar target for polymarket — a klines target averages 1m candles, which polymarket lacks",
        ));
    }
    validate_tracker(tracker)
}

fn validate_tracker(tracker: &TrackerSpec) -> Result<(), ConfigError> {
    let window_specs = [
        (
            "source.tracker.trades_all.windows",
            windows_of(&tracker.trades_all),
        ),
        (
            "source.tracker.trades_buy.windows",
            windows_of(&tracker.trades_buy),
        ),
        (
            "source.tracker.trades_sell.windows",
            windows_of(&tracker.trades_sell),
        ),
        (
            "source.tracker.microprice.windows",
            windows_of(&tracker.microprice),
        ),
        ("source.tracker.spread.windows", windows_of(&tracker.spread)),
    ];
    for (field, windows) in window_specs {
        check_windows(field, windows)?;
    }
    if let Some(imbalance) = &tracker.imbalance {
        // Empty windows = latest-only (block existence computes latest.imbalance).
        if imbalance.windows.contains(&0) {
            return Err(tracker_invalid(
                "source.tracker.imbalance.windows",
                "0",
                "all windows greater than 0",
            ));
        }
        if imbalance.top_n == 0 {
            return Err(tracker_invalid(
                "source.tracker.imbalance.top_n",
                "0",
                "greater than 0",
            ));
        }
    }
    if let Some(candles) = &tracker.candles
        && candles.keep == 0
    {
        return Err(tracker_invalid(
            "source.tracker.candles.keep",
            "0",
            "greater than 0",
        ));
    }
    if let Some(spin) = &tracker.spin_sampled {
        if spin.fields.is_empty() {
            return Err(tracker_invalid(
                "source.tracker.spin_sampled.fields",
                "[]",
                "at least one field",
            ));
        }
        if spin.window == 0 {
            return Err(tracker_invalid(
                "source.tracker.spin_sampled.window",
                "0",
                "greater than 0",
            ));
        }
        check_imbalance_available(
            "source.tracker.spin_sampled.fields",
            &spin.fields,
            tracker.imbalance.is_some(),
        )?;
    }
    if let Some(bars) = &tracker.volume_bars {
        check_volume_bars(bars, tracker.imbalance.is_some())?;
    }
    if let Some(ewma) = &tracker.ewma_vol
        && ewma.halflife_events < 1
    {
        return Err(tracker_invalid(
            "source.tracker.ewma_vol.halflife_events",
            "0",
            "greater than 0",
        ));
    }
    if let Some(intensity) = &tracker.intensity {
        // Fit needs interior bin + tail; positive finite half-life + event floor -> decay + gate well-defined.
        if intensity.max_depth_ticks < 2 {
            return Err(tracker_invalid(
                "source.tracker.intensity.max_depth_ticks",
                &intensity.max_depth_ticks.to_string(),
                "at least 2",
            ));
        }
        if !(intensity.half_life_secs.is_finite() && intensity.half_life_secs > 0.0) {
            return Err(tracker_invalid(
                "source.tracker.intensity.half_life_secs",
                &intensity.half_life_secs.to_string(),
                "a positive, finite number of seconds",
            ));
        }
        if !(intensity.min_events.is_finite() && intensity.min_events > 0.0) {
            return Err(tracker_invalid(
                "source.tracker.intensity.min_events",
                &intensity.min_events.to_string(),
                "a positive, finite decayed-count floor",
            ));
        }
    }
    Ok(())
}

fn check_volume_bars(bars: &VolumeBarsSpec, imbalance_configured: bool) -> Result<(), ConfigError> {
    if let VolumeThreshold::Fixed(usd) = bars.threshold
        && !(1..=MAX_THRESHOLD_USD).contains(&usd)
    {
        return Err(tracker_invalid(
            "source.tracker.volume_bars.threshold",
            &usd.to_string(),
            "at least 1 and within the i64 notional range",
        ));
    }
    if bars.keep == 0 {
        return Err(tracker_invalid(
            "source.tracker.volume_bars.keep",
            "0",
            "greater than 0",
        ));
    }
    let Some(sampled) = &bars.sampled else {
        return Ok(());
    };
    if sampled.fields.is_empty() {
        return Err(tracker_invalid(
            "source.tracker.volume_bars.sampled.fields",
            "[]",
            "at least one field",
        ));
    }
    if sampled.window == 0 {
        return Err(tracker_invalid(
            "source.tracker.volume_bars.sampled.window",
            "0",
            "greater than 0",
        ));
    }
    check_imbalance_available(
        "source.tracker.volume_bars.sampled.fields",
        &sampled.fields,
        imbalance_configured,
    )
}

fn check_imbalance_available(
    fields_field: &'static str,
    fields: &[SpinField],
    imbalance_configured: bool,
) -> Result<(), ConfigError> {
    if fields.contains(&SpinField::Imbalance) && !imbalance_configured {
        return Err(tracker_invalid(
            fields_field,
            "imbalance",
            "an imbalance block, required to sample imbalance",
        ));
    }
    Ok(())
}

fn windows_of(spec: &Option<WindowsSpec>) -> Option<&[usize]> {
    spec.as_ref().map(|spec| spec.windows.as_slice())
}

fn check_windows(field: &'static str, windows: Option<&[usize]>) -> Result<(), ConfigError> {
    if let Some(windows) = windows {
        if windows.is_empty() {
            return Err(tracker_invalid(field, "[]", "at least one window"));
        }
        if windows.contains(&0) {
            return Err(tracker_invalid(field, "0", "all windows greater than 0"));
        }
    }
    Ok(())
}

fn tracker_invalid(field: &'static str, value: &str, expected: &'static str) -> ConfigError {
    ConfigError::Invalid {
        field,
        value: value.into(),
        expected,
    }
}
