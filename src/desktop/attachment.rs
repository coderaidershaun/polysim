//! One attached trading engine as the workstation holds it, toolkit-free: link status, screen,
//! model, and the rule that a new session starts them over. The window and the bridge each hold one,
//! so the two cannot disagree about what an attached engine is doing.

use crate::msg::ui::{UiChannels, UiLifecycle};
use crate::runtime::ExitReport;

use super::dom_view::FeedStatus;
use super::link_client::{LinkCommand, LinkFeed};
use super::link_model::{ConnectionState, LinkStatus};
use super::model::UiModel;
use super::monitor_model::SystemNote;

/// Events folded per call. Once a second the engine sends a burst of a few hundred events, a quote
/// and about 60 features per instrument on top of the steady trade prints. Capping each call keeps
/// one fold short; [`Attachment::drain_feed`] reports what remains, and the next call takes it.
const EVENT_BUDGET: usize = 256;

/// The screen the workstation shows, set by the engine's [`UiLifecycle`] messages. `Starting` means
/// no engine has reported in yet, which the workstation decides for itself rather than being told.
pub(crate) enum Screen {
    Starting,
    Live,
    Draining(Box<str>),
    Stopped(ExitReport),
}

pub(crate) struct Attachment {
    channels: UiChannels,
    link: LinkFeed,
    link_status: Option<LinkStatus>,
    screen: Screen,
    model: UiModel,
}

impl Attachment {
    pub(crate) fn new(channels: UiChannels, link: LinkFeed) -> Self {
        Self {
            channels,
            link,
            link_status: None,
            screen: Screen::Starting,
            model: UiModel::new(),
        }
    }

    /// Takes the newest link status. Returns true when it names a different engine session than
    /// the one held, in which case everything this attachment holds has already started over.
    pub(crate) fn poll_link(&mut self) -> bool {
        let Some(status) = self.link.poll() else {
            return false;
        };
        let is_new_engine = self
            .link_status
            .is_some_and(|held| held.session != status.session);
        self.link_status = Some(status);
        if !is_new_engine {
            return false;
        }
        while self.channels.books.pop().is_ok() {}
        while self.channels.events.pop().is_ok() {}
        self.screen = Screen::Starting;
        self.model = UiModel::new();
        true
    }

    pub(crate) fn drain_lifecycle(&mut self) {
        while let Ok(message) = self.channels.lifecycle.try_recv() {
            self.apply_lifecycle(message);
        }
    }

    fn apply_lifecycle(&mut self, message: UiLifecycle) {
        self.model.note_lifecycle(system_note(&message));
        match message {
            UiLifecycle::Ready(catalog) => {
                if matches!(self.screen, Screen::Starting) {
                    self.screen = Screen::Live;
                }
                self.model.set_catalog(catalog);
            }
            UiLifecycle::Draining { reason } => {
                if !matches!(self.screen, Screen::Stopped(_)) {
                    self.screen = Screen::Draining(reason);
                }
            }
            UiLifecycle::Stopped(report) => self.screen = Screen::Stopped(report),
        }
    }

    /// Folds what the rings hold into the model. Returns true while events remain for the next call.
    pub(crate) fn drain_feed(&mut self) -> bool {
        while let Ok(snapshot) = self.channels.books.pop() {
            self.model.apply_book(snapshot);
        }
        for _ in 0..EVENT_BUDGET {
            match self.channels.events.pop() {
                Ok(event) => self.model.apply_event(event),
                Err(_) => return false,
            }
        }
        !self.channels.events.is_empty()
    }

    /// What the link says about the feed, before the ladder checks how fresh the book is. A
    /// draining or stopped engine sends nothing more, which reads the same as a dropped link.
    pub(crate) fn feed_status(&self) -> FeedStatus {
        let is_attached = !matches!(self.screen, Screen::Draining(_) | Screen::Stopped(_))
            && self
                .link_status
                .is_some_and(|status| status.connection == ConnectionState::Live);
        if is_attached { FeedStatus::Live } else { FeedStatus::Disconnected }
    }

    pub(crate) fn send(&self, command: LinkCommand) {
        self.link.send(command);
    }

    pub(crate) fn model(&self) -> &UiModel {
        &self.model
    }

    pub(crate) fn model_mut(&mut self) -> &mut UiModel {
        &mut self.model
    }

    pub(crate) fn screen(&self) -> &Screen {
        &self.screen
    }

    pub(crate) fn link_status(&self) -> Option<&LinkStatus> {
        self.link_status.as_ref()
    }
}

fn system_note(message: &UiLifecycle) -> SystemNote {
    match message {
        UiLifecycle::Ready(_) => SystemNote::Ready,
        UiLifecycle::Draining { reason } => SystemNote::Draining {
            reason: reason.clone(),
        },
        UiLifecycle::Stopped(report) => SystemNote::Stopped {
            graceful: report.graceful,
            reason: report.reason.clone(),
        },
    }
}
