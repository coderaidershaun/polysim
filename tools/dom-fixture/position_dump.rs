//! Risk scenes as TEXT: GUI proves nothing on locked machine, so print series for headless check.
//! Split from position_scenes for data/summary separation.

use polysim::desktop::position_chart_model::PositionBucket;
use polysim::ids::FIXED_SCALE;

use crate::position_scenes::position_scenes;

/// Summarise every scene's banked series on stdout: same data lower chart would draw, reviewable.
pub fn dump() {
    for scene in position_scenes() {
        let buckets: Vec<&PositionBucket> = scene.positions.buckets(scene.instrument).collect();
        let Some(first) = buckets.first() else {
            println!("{:<46} EMPTY series", scene.name);
            println!("{:<46}   check: {}", "", scene.check);
            continue;
        };
        let last = buckets.last().expect("a non-empty series to have a last");
        let exposure = extent(&buckets, |bucket| bucket.exposure_quote);
        let pnl = extent(&buckets, |bucket| bucket.pnl_quote);
        println!(
            // Modulo keeps spin readable ONLY when BASE_TS/SPIN == 100_000 — change constants -> arbitrary epoch.
            "{:<46} {:>4} buckets  spins {}..={}  gaps {}  levels {:>3}  exposure {}  pnl {}",
            scene.name,
            buckets.len(),
            first.index % 100_000,
            last.index % 100_000,
            gap_widths(&buckets),
            distinct_levels(&buckets),
            span(exposure),
            span(pnl),
        );
        println!("{:<46}   check: {}", "", scene.check);
    }
}

/// Gap widths in spins: count alone can't distinguish 1-bucket from 60-bucket gaps (different bugs).
fn gap_widths(buckets: &[&PositionBucket]) -> String {
    let widths: Vec<u64> = buckets
        .windows(2)
        .filter_map(|pair| (pair[1].index - pair[0].index).checked_sub(1))
        .filter(|missing| *missing > 0)
        .collect();
    match widths.is_empty() {
        true => "none".to_string(),
        false => format!("{widths:?}"),
    }
}

/// Distinct exposure levels: extremes alone can't see SHAPE (staircase collapsed == correct staircase extremes).
fn distinct_levels(buckets: &[&PositionBucket]) -> usize {
    let mut levels: Vec<i64> = buckets.iter().map(|bucket| bucket.exposure_quote).collect();
    levels.sort_unstable();
    levels.dedup();
    levels.len()
}

fn extent(buckets: &[&PositionBucket], value: fn(&PositionBucket) -> i64) -> (i64, i64) {
    buckets
        .iter()
        .fold((i64::MAX, i64::MIN), |(low, high), bucket| {
            (low.min(value(bucket)), high.max(value(bucket)))
        })
}

/// Quote-mantissa range with marker for zero-crossing (property three scenes demonstrate).
fn span((low, high): (i64, i64)) -> String {
    let crosses = if low < 0 && high > 0 { " CROSSES 0" } else { "" };
    format!(
        "[{:>10.2} .. {:<10.2}]{}",
        low as f64 / FIXED_SCALE as f64,
        high as f64 / FIXED_SCALE as f64,
        crosses
    )
}
