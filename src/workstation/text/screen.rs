//! The words on the starting screen and on the execution badge. The starting screen shows no
//! numbers, because there are none yet.

use crate::config::ExecutionMode;

use super::tone::Tone;

/// Title and detail with the role each wears.
pub(crate) struct ScreenText {
    pub title: &'static str,
    pub title_tone: Tone,
    pub detail: &'static str,
    pub detail_tone: Tone,
}

/// The only screen that is not the live workstation: no engine has announced its catalog yet.
pub(crate) const STARTING_SCREEN: ScreenText = ScreenText {
    title: "STARTING",
    title_tone: Tone::Text,
    detail: "waiting for engine",
    detail_tone: Tone::Text2,
};

/// The badge word and its role. No catalog yet means nothing has claimed to be armed, which reads
/// the same as `off`.
pub(crate) fn execution_badge(mode: Option<ExecutionMode>) -> (&'static str, Tone) {
    let tone = match mode {
        None | Some(ExecutionMode::Off) => Tone::Text,
        Some(ExecutionMode::Sim) => Tone::Warning,
        Some(ExecutionMode::Live) => Tone::Negative,
    };
    (ExecutionMode::badge(mode), tone)
}
