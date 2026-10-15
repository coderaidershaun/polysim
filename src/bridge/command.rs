//! What the page can ask of the bridge, one JSON object per stdin line. The wire shapes stay private
//! here, so no domain type has to learn about serde.

use serde::Deserialize;

use crate::desktop::chart_view::ChartMode;
use crate::desktop::dom_view::{DomGrouping, DomUnit};
use crate::desktop::model::UiModel;
use crate::desktop::position_chart_view::RiskSeries;
use crate::desktop::text::grouping_table;
use crate::ids::InstrumentId;
use crate::link::RunState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    View(ViewCommand),
    Fit(PageFit),
    Assert(RunState),
    NextPeer,
    Resync,
    Pause,
    Resume,
}

/// A choice about what the operator looks at. It never reaches the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewCommand {
    Select(InstrumentId),
    ChartMode(ChartMode),
    DomUnit(DomUnit),
    DomGrouping(DomGrouping),
    DomLevels(usize),
    RiskSeries(RiskSeries),
}

/// What the page has room for, which the bridge sizes its answers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageFit {
    DomRows(usize),
    AxisTicks { mid: usize, risk: usize },
}

#[derive(thiserror::Error, Debug)]
pub enum CommandError {
    #[error("command is not a protocol json object")]
    Malformed {
        #[source]
        source: serde_json::Error,
    },
    #[error("dom grouping {grouping:?} is not one the ladder offers")]
    UnofferedGrouping { grouping: DomGrouping },
    #[error("instrument {instrument} is not in the engine's catalog")]
    UnknownInstrument { instrument: u16 },
}

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Wire {
    Select {
        instrument: u16,
    },
    ChartMode {
        mode: WireMode,
    },
    DomUnit {
        unit: WireUnit,
    },
    DomGrouping {
        #[serde(flatten)]
        grouping: WireGrouping,
    },
    DomLevels {
        levels: usize,
    },
    DomFit {
        rows: usize,
    },
    ChartTicks {
        mid: usize,
        risk: usize,
    },
    RiskSeries {
        series: WireSeries,
    },
    Assert {
        state: WireState,
    },
    NextPeer,
    Resync,
    Pause,
    Resume,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum WireMode {
    Line,
    Candles,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum WireUnit {
    Ticks,
    Bps,
}

#[derive(Deserialize)]
#[serde(tag = "unit", rename_all = "lowercase")]
enum WireGrouping {
    Ticks {
        #[serde(rename = "perBucket")]
        per_bucket: i64,
    },
    Bps {
        numerator: i64,
        denominator: i64,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum WireSeries {
    Exposure,
    Pnl,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum WireState {
    Running,
    Idle,
}

impl Command {
    /// Reads one stdin line.
    ///
    /// # Errors
    /// [`CommandError::Malformed`] when the line is not a JSON object naming a command this
    /// bridge knows, or a field is missing or of the wrong kind.
    pub fn parse(line: &str) -> Result<Self, CommandError> {
        let wire: Wire =
            serde_json::from_str(line).map_err(|source| CommandError::Malformed { source })?;
        Ok(match wire {
            Wire::Select { instrument } => {
                Self::View(ViewCommand::Select(InstrumentId(instrument)))
            }
            Wire::ChartMode { mode } => Self::View(ViewCommand::ChartMode(match mode {
                WireMode::Line => ChartMode::Line,
                WireMode::Candles => ChartMode::Candles,
            })),
            Wire::DomUnit { unit } => Self::View(ViewCommand::DomUnit(match unit {
                WireUnit::Ticks => DomUnit::Ticks,
                WireUnit::Bps => DomUnit::Bps,
            })),
            Wire::DomGrouping { grouping } => {
                Self::View(ViewCommand::DomGrouping(match grouping {
                    WireGrouping::Ticks { per_bucket } => DomGrouping::Ticks { per_bucket },
                    WireGrouping::Bps {
                        numerator,
                        denominator,
                    } => DomGrouping::Bps {
                        numerator,
                        denominator,
                    },
                }))
            }
            Wire::DomLevels { levels } => Self::View(ViewCommand::DomLevels(levels)),
            Wire::DomFit { rows } => Self::Fit(PageFit::DomRows(rows)),
            Wire::ChartTicks { mid, risk } => Self::Fit(PageFit::AxisTicks { mid, risk }),
            Wire::RiskSeries { series } => Self::View(ViewCommand::RiskSeries(match series {
                WireSeries::Exposure => RiskSeries::Exposure,
                WireSeries::Pnl => RiskSeries::Pnl,
            })),
            Wire::Assert { state } => Self::Assert(match state {
                WireState::Running => RunState::Running,
                WireState::Idle => RunState::Idle,
            }),
            Wire::NextPeer => Self::NextPeer,
            Wire::Resync => Self::Resync,
            Wire::Pause => Self::Pause,
            Wire::Resume => Self::Resume,
        })
    }
}

impl ViewCommand {
    /// Applies the choice to the model the next frame is read from.
    ///
    /// # Errors
    /// [`CommandError::UnknownInstrument`] when a catalog is held and lacks the instrument, and
    /// [`CommandError::UnofferedGrouping`] when the ladder's dropdown for that unit lacks the
    /// grouping. Nothing changes then.
    pub fn apply(self, model: &mut UiModel) -> Result<(), CommandError> {
        match self {
            Self::Select(instrument) => {
                // No catalog yet means the page is replaying its choice after a restart, so the
                // selection is taken now and the catalog is trusted to hold it when it lands.
                let is_known = model
                    .catalog()
                    .is_none_or(|catalog| catalog.instrument(instrument).is_some());
                if !is_known {
                    return Err(CommandError::UnknownInstrument {
                        instrument: instrument.0,
                    });
                }
                model.select(instrument);
            }
            Self::ChartMode(mode) => model.set_chart_mode(mode),
            Self::DomUnit(unit) => model.set_dom_unit(unit),
            Self::DomGrouping(grouping) => {
                let is_offered = grouping_table(grouping.unit())
                    .iter()
                    .any(|(candidate, _)| *candidate == grouping);
                if !is_offered {
                    return Err(CommandError::UnofferedGrouping { grouping });
                }
                model.set_dom_grouping(grouping);
            }
            Self::DomLevels(levels) => model.set_dom_levels(levels),
            Self::RiskSeries(series) => model.set_risk_series(series),
        }
        Ok(())
    }
}
