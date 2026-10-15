//! One attached engine and the encoder that tells the page about it. Holds the same attachment the
//! window holds, so the page and the window can never disagree about what the engine is doing.

use crate::desktop::attachment::{Attachment, Screen};
use crate::desktop::link_client::LinkCommand;
use crate::warn;

use super::command::Command;
use super::encode::{Encoder, TickInput};

/// The bridge ticks slower than the window repaints and cannot ask for an early tick, so one tick
/// folds several event budgets. The cap keeps a flooding feed from holding back the page's commands.
const DRAINS_PER_TICK: usize = 8;

pub(crate) struct Session {
    attachment: Attachment,
    encoder: Encoder,
    is_paused: bool,
}

impl Session {
    pub(crate) fn new(attachment: Attachment) -> Self {
        Self {
            attachment,
            encoder: Encoder::new(),
            is_paused: false,
        }
    }

    /// Folds what the engine sent since the last step, up to the per-tick budget. A new engine
    /// session starts the page over, since nothing it holds describes the new run.
    pub(crate) fn step(&mut self) {
        if self.attachment.poll_link() {
            self.encoder.resync();
        }
        self.attachment.drain_lifecycle();
        for _ in 0..DRAINS_PER_TICK {
            if !self.attachment.drain_feed() {
                break;
            }
        }
    }

    pub(crate) fn apply(&mut self, command: Command) {
        match command {
            Command::View(view) => {
                if let Err(error) = view.apply(self.attachment.model_mut()) {
                    warn!("bridge command {view:?} refused: {error}");
                }
            }
            Command::Fit(fit) => self.encoder.fit(fit),
            Command::Assert(state) => self.attachment.send(LinkCommand::Assert(state)),
            Command::NextPeer => self.attachment.send(LinkCommand::NextPeer),
            Command::Resync => self.encoder.resync(),
            Command::Pause => self.is_paused = true,
            Command::Resume => {
                self.is_paused = false;
                self.encoder.resync();
            }
        }
    }

    /// The lines to write now. Nothing until the link has reported once, so the page keeps showing
    /// that no engine is attached. Link and lifecycle lines flow while paused, since the page needs
    /// to know the engine stopped; the model keeps folding either way.
    pub(crate) fn lines(&mut self) -> Vec<String> {
        let Some(status) = self.attachment.link_status() else {
            return Vec::new();
        };
        let mut lines = Vec::new();
        lines.extend(self.encoder.link_line(status));
        lines.extend(
            self.encoder
                .lifecycle_line(status.session, self.attachment.screen()),
        );
        if !self.is_paused && matches!(self.attachment.screen(), Screen::Live) {
            lines.extend(self.encoder.lines(&TickInput {
                model: self.attachment.model(),
                session: status.session,
                feed: self.attachment.feed_status(),
            }));
        }
        lines
    }
}
