//! The eframe application: the 0.35 `logic`/`ui` split. `logic` drains lifecycle, arbitrates the
//! shared close path (Escape key, toolbar button, window close all route through
//! [`DesktopApp::request_close`]), and schedules repaints; `ui` paints the phase's screen. Never
//! blocks — and never waits on an engine: the workstation is its own process, so closing it stops
//! nothing.

use std::time::{Duration, Instant};

use eframe::egui;

use crate::msg::ui::{UiChannels, UiLifecycle};
use crate::runtime::ExitReport;

use super::dom_view::FeedStatus;
use super::layout::ShellAction;
use super::link_bar::{self, LinkAction};
use super::link_client::{LinkCommand, LinkFeed};
use super::link_model::{ConnectionState, LinkStatus};
use super::model::UiModel;
use super::monitor::MonitorUiState;
use super::monitor_model::SystemNote;
use super::theme::{self, DARK};
use super::{layout, lifecycle};

/// Events drained from the ring per frame before yielding. Post-warmup a spin banks a burst — one
/// quote plus ~60 feature events per recorded instrument, atop the continuous trade prints — so once
/// a second the lane briefly holds a few hundred events. The budget deliberately caps the per-frame
/// drain rather than clearing the whole burst in one go: [`DesktopApp::drain_feed`] carries the
/// remainder over and asks for an immediate repaint, so it empties within a frame or two at the 8 ms
/// live cadence while no single frame's fold ever runs unbounded.
const EVENT_BUDGET: usize = 256;

/// How long the window stays hidden before it is destroyed: several 60 Hz display cycles, so the
/// pending AppKit flush (see [`DesktopApp::advance_close_handshake`]) has run, yet imperceptible —
/// the window is already invisible.
const HIDE_BEFORE_CLOSE: Duration = Duration::from_millis(120);

/// The window-teardown walk: visible, hidden-and-waiting, or final close issued.
enum CloseHandshake {
    Open,
    Hidden(Instant),
    CloseSent,
}

/// The UI-side lifecycle, driven by drained [`UiLifecycle`] plus the local close handshake.
/// `Starting` is the window's own opening phase — it means "no engine has reported in yet", which
/// the workstation decides for itself rather than being told.
enum AppPhase {
    Starting,
    Live,
    Draining(Box<str>),
    Stopped(ExitReport),
}

pub(crate) struct DesktopApp {
    channels: UiChannels,
    link: LinkFeed,
    link_status: Option<LinkStatus>,
    phase: AppPhase,
    model: UiModel,
    monitor_state: MonitorUiState,
    title_applied: bool,
    close_wanted: bool,
    handshake: CloseHandshake,
}

impl DesktopApp {
    pub(crate) fn new(
        creation: &eframe::CreationContext<'_>,
        channels: UiChannels,
        link: LinkFeed,
    ) -> Self {
        theme::install_style(&creation.egui_ctx);
        Self {
            channels,
            link,
            link_status: None,
            phase: AppPhase::Starting,
            model: UiModel::new(),
            monitor_state: MonitorUiState::new(),
            title_applied: false,
            close_wanted: false,
            handshake: CloseHandshake::Open,
        }
    }

    fn poll_link(&mut self) {
        let Some(status) = self.link.poll() else {
            return;
        };
        let is_new_engine = self
            .link_status
            .is_some_and(|held| held.session != status.session);
        self.link_status = Some(status);
        if !is_new_engine {
            return;
        }
        while self.channels.books.pop().is_ok() {}
        while self.channels.events.pop().is_ok() {}
        self.phase = AppPhase::Starting;
        self.model = UiModel::new();
        self.monitor_state = MonitorUiState::new();
        self.title_applied = false;
    }

    fn drain_lifecycle(&mut self) {
        while let Ok(message) = self.channels.lifecycle.try_recv() {
            self.apply_lifecycle(message);
        }
    }

    fn apply_lifecycle(&mut self, message: UiLifecycle) {
        self.model.note_lifecycle(system_note(&message));
        match message {
            UiLifecycle::Ready(catalog) => {
                if matches!(self.phase, AppPhase::Starting) {
                    self.phase = AppPhase::Live;
                }
                self.model.set_catalog(catalog);
            }
            UiLifecycle::Draining { reason } => {
                if !matches!(self.phase, AppPhase::Stopped(_)) {
                    self.phase = AppPhase::Draining(reason);
                }
            }
            UiLifecycle::Stopped(report) => self.phase = AppPhase::Stopped(report),
        }
    }

    fn drain_feed(&mut self, ctx: &egui::Context) {
        while let Ok(snapshot) = self.channels.books.pop() {
            self.model.apply_book(snapshot);
        }
        for _ in 0..EVENT_BUDGET {
            match self.channels.events.pop() {
                Ok(event) => self.model.apply_event(event),
                Err(_) => return,
            }
        }
        if !self.channels.events.is_empty() {
            ctx.request_repaint();
        }
    }

    /// What the shell should assume about the feed before the ladder consults the book itself. A
    /// draining or stopped engine sends nothing more, which reads the same as a dropped link.
    fn feed_status(&self) -> FeedStatus {
        let is_attached = !matches!(self.phase, AppPhase::Draining(_) | AppPhase::Stopped(_))
            && self
                .link_status
                .is_some_and(|status| status.connection == ConnectionState::Live);
        if is_attached { FeedStatus::Live } else { FeedStatus::Disconnected }
    }

    fn request_close(&mut self) {
        self.close_wanted = true;
    }

    fn apply_title(&mut self, ctx: &egui::Context) {
        if self.title_applied {
            return;
        }
        if let Some(catalog) = self.model.catalog() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(
                catalog.window_title.to_string(),
            ));
            self.title_applied = true;
        }
    }

    fn handle_escape(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.request_close();
        }
    }

    fn handle_close_request(&mut self, ctx: &egui::Context) {
        if matches!(self.handshake, CloseHandshake::CloseSent) {
            return;
        }
        if ctx.input(|input| input.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.request_close();
        }
        self.advance_close_handshake(ctx);
    }

    /// AppKit quirk (macOS 15, observed 2026-07-24): once the window has been key, its TouchBar
    /// finder holds a KVO observation on the view's responder chain and invalidates it inside a
    /// later display-cycle flush. eframe's run-and-return teardown destroys the window mid-loop, so
    /// that flush runs against a dead view, throws `cannot remove an observer … nextResponder`, and
    /// AppKit escalates to SIGTRAP ("quit unexpectedly"). Hiding the window first lets the flush
    /// complete against a live view; only after [`HIDE_BEFORE_CLOSE`] is the real close issued.
    fn advance_close_handshake(&mut self, ctx: &egui::Context) {
        if !self.close_wanted {
            return;
        }
        match self.handshake {
            CloseHandshake::Open => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                self.handshake = CloseHandshake::Hidden(Instant::now());
                ctx.request_repaint();
            }
            CloseHandshake::Hidden(at) if at.elapsed() >= HIDE_BEFORE_CLOSE => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                self.handshake = CloseHandshake::CloseSent;
            }
            CloseHandshake::Hidden(_) => ctx.request_repaint(),
            CloseHandshake::CloseSent => {}
        }
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

impl eframe::App for DesktopApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_link();
        self.drain_lifecycle();
        self.drain_feed(ctx);
        self.apply_title(ctx);
        self.handle_escape(ctx);
        self.handle_close_request(ctx);

        let cadence = match self.phase {
            AppPhase::Live | AppPhase::Draining(_) => Duration::from_millis(8),
            AppPhase::Starting | AppPhase::Stopped(_) => Duration::from_millis(250),
        };
        ctx.request_repaint_after(cadence);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let mut shell_action = ShellAction::None;
        let mut link_action = LinkAction::None;
        let feed = self.feed_status();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(DARK.canvas))
            .show(ui, |ui| {
                let (content, bar) = layout::split_link_bar(ui.available_rect_before_wrap());
                ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
                    match &self.phase {
                        AppPhase::Starting => lifecycle::starting(ui),
                        AppPhase::Live => {
                            shell_action = layout::workstation(
                                ui,
                                &mut self.model,
                                &mut self.monitor_state,
                                feed,
                            )
                        }
                        AppPhase::Draining(reason) => lifecycle::draining(ui, reason),
                        AppPhase::Stopped(report) => lifecycle::stopped(ui, report),
                    }
                });
                link_action = link_bar::paint(ui, bar, self.link_status.as_ref());
            });
        if shell_action == ShellAction::CloseRequested {
            self.request_close();
        }
        match link_action {
            LinkAction::None => {}
            LinkAction::Assert(state) => self.link.send(LinkCommand::Assert(state)),
            LinkAction::NextPeer => self.link.send(LinkCommand::NextPeer),
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::clear_color()
    }
}
