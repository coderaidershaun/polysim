//! The eframe application and the station behind it. The station owns what the page shares with the
//! browser: the clock, the relay socket, the attached engine, and the one `pump` that every trigger
//! (a message, a frame, a one-second timer) runs.

use core::time::Duration;
use std::cell::RefCell;
use std::rc::Rc;

use eframe::egui;
use js_sys::Math;
use web_sys::Document;
use web_sys::wasm_bindgen::JsCast;
use web_sys::wasm_bindgen::closure::Closure;

use super::layout;
use super::link_bar::{self, LinkAction};
use super::socket::{Frame, LinkSocket};
use super::theme::{DARK, METRICS, clear_color};
use crate::config::{ConfigError, StrategyId};
use crate::link::LinkHash;
use crate::msg::relay::RelayHello;
use crate::time::{DurationUs, EngineClock, TsUs};
use crate::workstation::attachment::{Attachment, Screen};
use crate::workstation::link_session::{LinkSession, LinkSessionError, LinkTarget};
use crate::workstation::text::{STARTING_SCREEN, ScreenText};
use crate::{error, warn};

const RECONNECT_PAUSE: DurationUs = DurationUs::from_secs(1);

/// Hidden tabs still run this timer, so the subscription and the model stay current while no frame
/// is drawn.
const TICK_MS: i32 = 1_000;

const LIVE_REPAINT: Duration = Duration::from_millis(8);
const STARTING_REPAINT: Duration = Duration::from_millis(250);

/// Floor set by the summary band: below about 1190 points its mid cell is narrower than a BTC mid,
/// so the number would cross a separator. A page that cannot be that big is shrunk instead.
const MIN_WIDTH: f32 = 1_200.0;
const MIN_HEIGHT: f32 = 700.0;
const MIN_ZOOM: f32 = 0.5;

/// Rounding to whole pixels moves the measured size by under a pixel each frame. A change smaller
/// than this is that jitter, and acting on it would resize the page every frame.
const ZOOM_TOLERANCE: f32 = 0.002;

#[derive(thiserror::Error, Debug)]
pub(super) enum StartError {
    #[error("this page has no window")]
    NoWindow,
    #[error("this page has no document")]
    NoDocument,
    #[error("cannot start the one-second link timer: {0}")]
    Timer(String),
}

#[derive(thiserror::Error, Debug)]
enum HelloError {
    #[error("the relay hello names a bad strategy: {0}")]
    Strategy(#[from] ConfigError),
    #[error("the relay hello cannot start a link session: {0}")]
    Session(#[from] LinkSessionError),
}

pub(super) struct Station {
    ctx: egui::Context,
    document: Document,
    clock: EngineClock,
    /// Empty until the relay's hello says which engines there are. The hello stays with the
    /// attachment it built: a reconnect brings the same hello again and keeps the model and the run
    /// assertion, and a different one means the server now names other engines.
    attached: Option<(RelayHello, Attachment)>,
    socket: Option<LinkSocket>,
    connected_ts_us: Option<TsUs>,
    title: Box<str>,
    _tick: Option<Closure<dyn FnMut()>>,
}

impl Station {
    fn new(ctx: egui::Context, document: Document) -> Self {
        Self {
            ctx,
            document,
            clock: EngineClock::start(),
            attached: None,
            socket: None,
            connected_ts_us: None,
            title: Box::default(),
            _tick: None,
        }
    }

    /// Every trigger runs this: a message, a frame, and a timer that still fires in a hidden tab.
    /// A repaint is requested only when the screen has something new, so the slow starting
    /// cadence survives.
    pub(super) fn pump(&mut self, shared: &Rc<RefCell<Self>>) {
        let now = self.clock.now();
        let mut is_changed = false;
        if let Some((_, attachment)) = &mut self.attached {
            is_changed = attachment.refresh(now);
            if let Some(socket) = self.socket.as_ref().filter(|socket| socket.is_open())
                && let Some(datagram) = attachment.subscribe_due(now)
            {
                socket.send(datagram);
            }
        }
        let is_socket_gone = self.socket.as_ref().is_none_or(LinkSocket::is_closed);
        let can_reconnect = self
            .connected_ts_us
            .is_none_or(|at| now.diff(at) >= RECONNECT_PAUSE);
        if is_socket_gone && can_reconnect {
            self.connect(shared, now);
        }
        self.apply_title();
        if is_changed {
            self.ctx.request_repaint();
        }
    }

    pub(super) fn on_frame(&mut self, frame: Frame, shared: &Rc<RefCell<Self>>) {
        let now = self.clock.now();
        match frame {
            Frame::Hello(text) => self.on_hello(&text, now),
            Frame::Datagram(bytes) => {
                if let Some((_, attachment)) = &mut self.attached
                    && attachment.on_datagram(&bytes, now)
                {
                    self.ctx.request_repaint();
                }
            }
        }
        self.pump(shared);
    }

    fn on_hello(&mut self, text: &str, now: TsUs) {
        let hello = match serde_json::from_str::<RelayHello>(text) {
            Ok(hello) => hello,
            Err(error) => {
                error!("the relay hello is not valid json: {error}");
                return;
            }
        };
        match &self.attached {
            Some((held, _)) if *held == hello => {}
            Some(_) => self.start_over(),
            None => self.adopt(hello, now),
        }
    }

    fn adopt(&mut self, hello: RelayHello, now: TsUs) {
        match attach(&hello, now) {
            Ok(attachment) => {
                self.attached = Some((hello, attachment));
                self.ctx.request_repaint();
            }
            Err(error) => error!("cannot attach to the engines the relay named: {error}"),
        }
    }

    /// Back to how the page boots, so the next socket's hello attaches afresh. Keeping the old
    /// session would label one engine with another's address and send START or STOP to it.
    // SMELL: this needs the relay to answer with a hello. A server restarted with fewer engines
    // refuses the page's old engine number before any hello, so the page retries that number until
    // it is reloaded.
    fn start_over(&mut self) {
        warn!("the relay now names other engines, so the workstation starts over on them");
        self.attached = None;
        self.socket = None;
        self.connected_ts_us = None;
        self.ctx.request_repaint();
    }

    // SMELL: after a reconnect to the same engine, the new relay socket reaches it from a new
    // address, but the next subscribe is not due until two seconds after the last one, so the
    // engine keeps sending to the old address meanwhile. Those frames are lost and show as event
    // gaps. Subscribing as soon as the socket opens would close the gap.
    fn connect(&mut self, shared: &Rc<RefCell<Self>>, now: TsUs) {
        self.connected_ts_us = Some(now);
        let peer = self
            .attached
            .as_ref()
            .map_or(0, |(_, attachment)| attachment.active_peer());
        self.socket = match LinkSocket::open(&self.document, peer, shared) {
            Ok(socket) => Some(socket),
            Err(error) => {
                error!("cannot open the relay socket: {error:?}");
                None
            }
        };
    }

    /// `ViewportCommand::Title` does nothing on the web, so the page title is set directly.
    fn apply_title(&mut self) {
        let Some(catalog) = self
            .attached
            .as_ref()
            .and_then(|(_, attachment)| attachment.model().catalog())
        else {
            return;
        };
        if catalog.window_title == self.title {
            return;
        }
        self.title = catalog.window_title.clone();
        self.document.set_title(&self.title);
    }

    fn act_on(&mut self, action: LinkAction, shared: &Rc<RefCell<Self>>) {
        let now = self.clock.now();
        match action {
            LinkAction::None => return,
            LinkAction::Assert(state) => {
                if let Some((_, attachment)) = &mut self.attached {
                    attachment.assert_run_state(state, now);
                }
            }
            LinkAction::NextPeer => self.move_to_next_peer(shared, now),
        }
        self.pump(shared);
        self.ctx.request_repaint();
    }

    /// The new engine has its own relay socket, so the old one is closed and its callbacks are
    /// detached before the new one opens.
    fn move_to_next_peer(&mut self, shared: &Rc<RefCell<Self>>, now: TsUs) {
        let Some((_, attachment)) = &mut self.attached else {
            return;
        };
        if !attachment.attach_to_next_peer(now) {
            return;
        }
        self.socket = None;
        self.connect(shared, now);
    }

    fn paint(&mut self, ui: &mut egui::Ui) -> LinkAction {
        let mut link_action = LinkAction::None;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(DARK.canvas))
            .show(ui, |ui| {
                let (content, bar) = layout::split_link_bar(ui.available_rect_before_wrap());
                ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
                    self.paint_content(ui);
                });
                let status = self
                    .attached
                    .as_ref()
                    .map(|(_, attachment)| attachment.link_status());
                link_action = link_bar::paint(ui, bar, status);
            });
        link_action
    }

    fn paint_content(&mut self, ui: &mut egui::Ui) {
        match &mut self.attached {
            Some((_, attachment)) if matches!(attachment.screen(), Screen::Live) => {
                let feed = attachment.feed_status();
                let (model, monitor_state) = attachment.panels();
                layout::workstation(ui, model, monitor_state, feed);
            }
            _ => paint_screen(ui, &STARTING_SCREEN),
        }
    }

    fn repaint_cadence(&self) -> Duration {
        match self
            .attached
            .as_ref()
            .map(|(_, attachment)| attachment.screen())
        {
            Some(Screen::Live) => LIVE_REPAINT,
            _ => STARTING_REPAINT,
        }
    }
}

/// The relay's hello names the engines, but not a clock or a random number, so both come from
/// here.
fn attach(hello: &RelayHello, now: TsUs) -> Result<Attachment, HelloError> {
    let target = LinkTarget {
        strategy: StrategyId::new(&hello.strategy)?,
        peers: hello.peers.clone(),
        token_hash: LinkHash(hello.token_hash),
        sender_nonce: (Math::random() * (1_u64 << 53) as f64) as u64,
    };
    Ok(Attachment::new(LinkSession::new(target, now)?, now))
}

pub(super) struct WorkstationApp {
    station: Rc<RefCell<Station>>,
}

impl WorkstationApp {
    pub(super) fn start(ctx: egui::Context) -> Result<Self, StartError> {
        let window = web_sys::window().ok_or(StartError::NoWindow)?;
        let document = window.document().ok_or(StartError::NoDocument)?;
        let station = Rc::new(RefCell::new(Station::new(ctx, document)));
        let tick = {
            let station = Rc::clone(&station);
            Closure::<dyn FnMut()>::new(move || {
                if let Ok(mut busy) = station.try_borrow_mut() {
                    busy.pump(&station);
                }
            })
        };
        window
            .set_interval_with_callback_and_timeout_and_arguments_0(
                tick.as_ref().unchecked_ref(),
                TICK_MS,
            )
            .map_err(|error| StartError::Timer(format!("{error:?}")))?;
        station.borrow_mut()._tick = Some(tick);
        station.borrow_mut().pump(&station);
        Ok(Self { station })
    }
}

impl eframe::App for WorkstationApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut station = self.station.borrow_mut();
        station.pump(&self.station);
        fit_zoom(ctx);
        ctx.request_repaint_after(station.repaint_cadence());
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let mut station = self.station.borrow_mut();
        let link_action = station.paint(ui);
        station.act_on(link_action, &self.station);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        clear_color()
    }
}

/// A browser frame cannot enforce a minimum window size, so a page smaller than the layout's floor
/// is zoomed out until the layout fits.
fn fit_zoom(ctx: &egui::Context) {
    let css = ctx.content_rect().size() * ctx.zoom_factor();
    if css.x <= 0.0 || css.y <= 0.0 {
        return;
    }
    let zoom = (css.x / MIN_WIDTH)
        .min(css.y / MIN_HEIGHT)
        .clamp(MIN_ZOOM, 1.0);
    if (zoom - ctx.zoom_factor()).abs() > ZOOM_TOLERANCE {
        ctx.set_zoom_factor(zoom);
    }
}

/// The whole surface while no workstation is live. It is honest, with no made-up numbers.
fn paint_screen(ui: &mut egui::Ui, screen_text: &ScreenText) {
    let full = ui.available_rect_before_wrap();
    ui.painter().rect_filled(full, 0.0, DARK.canvas);
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(full)
            .layout(egui::Layout::top_down(egui::Align::Center)),
        |ui| {
            ui.add_space(full.height() * 0.40);
            ui.label(
                egui::RichText::new(screen_text.title)
                    .color(DARK.tone(screen_text.title_tone))
                    .monospace()
                    .size(22.0),
            );
            ui.add_space(METRICS.space_2);
            ui.label(
                egui::RichText::new(screen_text.detail)
                    .color(DARK.tone(screen_text.detail_tone))
                    .monospace(),
            );
        },
    );
}
