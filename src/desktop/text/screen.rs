//! What each screen without a live workstation says, and what the execution badge says: starting,
//! draining and stopped are honest, with no fabricated numbers.

use crate::config::ExecutionMode;
use crate::desktop::attachment::Screen;

use super::tone::Tone;

/// Title and detail with the role each wears. A live screen says nothing: the workstation shows.
pub(crate) struct ScreenText<'a> {
    pub title: &'a str,
    pub title_tone: Tone,
    pub detail: &'a str,
    pub detail_tone: Tone,
}

pub(crate) fn screen_text(screen: &Screen) -> ScreenText<'_> {
    match screen {
        Screen::Starting => ScreenText {
            title: "STARTING",
            title_tone: Tone::Text,
            detail: "waiting for engine",
            detail_tone: Tone::Text2,
        },
        Screen::Live => ScreenText {
            title: "",
            title_tone: Tone::Text,
            detail: "",
            detail_tone: Tone::Text2,
        },
        Screen::Draining(reason) => ScreenText {
            title: "DRAINING",
            title_tone: Tone::Warning,
            detail: reason,
            detail_tone: Tone::Text2,
        },
        Screen::Stopped(report) => {
            let (title, title_tone) = if report.graceful {
                ("STOPPED - graceful", Tone::Positive)
            } else {
                ("STOPPED - fatal", Tone::Invalid)
            };
            ScreenText {
                title,
                title_tone,
                detail: report.reason.as_ref(),
                detail_tone: Tone::Text2,
            }
        }
    }
}

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
