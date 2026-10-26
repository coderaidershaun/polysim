//! The connection strip's words: the peer, connection, phase, run and control chips, and what the
//! run button says.

use crate::link::{RunPhase, RunState};
use crate::workstation::format::MISSING;
use crate::workstation::link_model::{ConnectionState, ControlVerdict, LinkStatus};

use super::tone::Tone;

/// The chips in the order the link bar lays them out.
pub(crate) fn link_chips(status: &LinkStatus) -> Vec<(String, Tone)> {
    let mut chips = vec![(peer_label(status), Tone::Text)];
    let (connection, tone) = connection_label(status.connection);
    chips.push((connection.to_owned(), tone));
    if let Some(phase) = status.phase {
        let (label, tone) = phase_label(phase);
        chips.push((label.to_owned(), tone));
    }
    chips.push((run_label(status), run_tone(status)));
    chips.push((control_label(status), control_tone(status)));
    chips
}

pub(crate) fn button_label(target: RunState) -> &'static str {
    match target {
        RunState::Idle => "STOP",
        RunState::Running => "START",
    }
}

pub(crate) fn is_peer_clickable(status: &LinkStatus) -> bool {
    status.peer_count >= 2
}

fn peer_label(status: &LinkStatus) -> String {
    if status.peer_count < 2 {
        return format!("engine {}", status.peer);
    }
    format!(
        "engine {} [{}/{}]",
        status.peer,
        status.peer_index + 1,
        status.peer_count
    )
}

fn connection_label(connection: ConnectionState) -> (&'static str, Tone) {
    match connection {
        ConnectionState::Connecting => ("CONNECTING", Tone::Text2),
        ConnectionState::Live => ("LIVE", Tone::Positive),
        ConnectionState::Stale => ("STALE", Tone::Stale),
    }
}

fn phase_label(phase: RunPhase) -> (&'static str, Tone) {
    match phase {
        RunPhase::Starting => ("starting", Tone::Text2),
        RunPhase::Ready => ("ready", Tone::Text2),
        RunPhase::Draining => ("draining", Tone::Warning),
        RunPhase::Stopped => ("stopped", Tone::Invalid),
    }
}

fn run_label(status: &LinkStatus) -> String {
    match status.reported_state {
        Some(RunState::Running) => "run RUNNING".to_owned(),
        Some(RunState::Idle) => "run IDLE".to_owned(),
        None => format!("run {MISSING}"),
    }
}

fn run_tone(status: &LinkStatus) -> Tone {
    match status.reported_state {
        Some(RunState::Running) => Tone::Positive,
        Some(RunState::Idle) => Tone::Warning,
        None => Tone::Text2,
    }
}

fn control_label(status: &LinkStatus) -> String {
    match (status.control, status.asserted_state) {
        (ControlVerdict::Lost { holder_epoch }, _) => {
            format!("control LOST to epoch {holder_epoch}")
        }
        (ControlVerdict::NoOpinion, _) | (_, None) => "control none".to_owned(),
        (ControlVerdict::Applied, Some(state)) => format!("control applied {}", state_word(state)),
        (ControlVerdict::Pending, Some(state)) => {
            format!("control asserting {}", state_word(state))
        }
    }
}

fn control_tone(status: &LinkStatus) -> Tone {
    match status.control {
        ControlVerdict::NoOpinion => Tone::Text2,
        ControlVerdict::Pending => Tone::Warning,
        ControlVerdict::Applied => Tone::Positive,
        ControlVerdict::Lost { .. } => Tone::Invalid,
    }
}

fn state_word(state: RunState) -> &'static str {
    match state {
        RunState::Running => "RUNNING",
        RunState::Idle => "IDLE",
    }
}
