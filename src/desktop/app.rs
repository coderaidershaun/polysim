//! The eframe application: the 0.35 `logic`/`ui` split. `logic` drains lifecycle, arbitrates the
//! shared close path (Escape key, toolbar button, window close all route through
//! [`DesktopApp::request_close`]), and schedules repaints; `ui` paints the current screen. Never
//! blocks — and never waits on an engine: the workstation is its own process, so closing it stops
//! nothing.

use std::time::{Duration, Instant};

use eframe::egui;

use crate::msg::ui::UiChannels;

use super::attachment::{Attachment, Screen};
use super::layout::ShellAction;
use super::link_bar::{self, LinkAction};
use super::link_client::{LinkCommand, LinkFeed};
use super::monitor::MonitorUiState;
use super::text;
use super::theme::{self, DARK};
use super::{layout, lifecycle};

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

pub(crate) struct DesktopApp {
    attachment: Attachment,
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
            attachment: Attachment::new(channels, link),
            monitor_state: MonitorUiState::new(),
            title_applied: false,
            close_wanted: false,
            handshake: CloseHandshake::Open,
        }
    }

    fn poll_link(&mut self) {
        if self.attachment.poll_link() {
            self.monitor_state = MonitorUiState::new();
            self.title_applied = false;
        }
    }

    fn request_close(&mut self) {
        self.close_wanted = true;
    }

    fn apply_title(&mut self, ctx: &egui::Context) {
        if self.title_applied {
            return;
        }
        if let Some(catalog) = self.attachment.model().catalog() {
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

impl eframe::App for DesktopApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_link();
        self.attachment.drain_lifecycle();
        if self.attachment.drain_feed() {
            ctx.request_repaint();
        }
        self.apply_title(ctx);
        self.handle_escape(ctx);
        self.handle_close_request(ctx);

        let cadence = match self.attachment.screen() {
            Screen::Live | Screen::Draining(_) => Duration::from_millis(8),
            Screen::Starting | Screen::Stopped(_) => Duration::from_millis(250),
        };
        ctx.request_repaint_after(cadence);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let mut shell_action = ShellAction::None;
        let mut link_action = LinkAction::None;
        let feed = self.attachment.feed_status();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(DARK.canvas))
            .show(ui, |ui| {
                let (content, bar) = layout::split_link_bar(ui.available_rect_before_wrap());
                ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
                    match self.attachment.screen() {
                        Screen::Live => {
                            shell_action = layout::workstation(
                                ui,
                                self.attachment.model_mut(),
                                &mut self.monitor_state,
                                feed,
                            )
                        }
                        screen => lifecycle::paint(ui, &text::screen_text(screen)),
                    }
                });
                link_action = link_bar::paint(ui, bar, self.attachment.link_status());
            });
        if shell_action == ShellAction::CloseRequested {
            self.request_close();
        }
        match link_action {
            LinkAction::None => {}
            LinkAction::Assert(state) => self.attachment.send(LinkCommand::Assert(state)),
            LinkAction::NextPeer => self.attachment.send(LinkCommand::NextPeer),
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::clear_color()
    }
}
