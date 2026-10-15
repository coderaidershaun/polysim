//! The bridge's I/O: it starts the link client, reads commands from stdin on a thread of its own,
//! and writes the session's lines to stdout once per tick. Nothing else may touch stdout, which the
//! page reads as protocol lines and nothing more.

use std::io::{self, BufRead, BufWriter, ErrorKind, Write};
use std::process::Command as Process;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use crate::desktop::attachment::Attachment;
use crate::desktop::link_client::LinkClient;
use crate::msg::ui::ui_channel;
use crate::warn;

use super::command::Command;
use super::encode::{Encoder, hello_line};
use super::session::Session;
use super::{BridgeConfig, BridgeError};

/// Lines the reader may hold before it waits, so a page that floods commands slows itself instead
/// of growing memory.
const STDIN_CAPACITY: usize = 256;

const STDIN_CLOSED: &str = "stdin closed";
const STDIN_FAILED: &str = "stdin failed";

/// How much of a refused command the log keeps.
const LOGGED_COMMAND_CHARS: usize = 120;

enum StdinEvent {
    Line(String),
    Closed,
    Failed(io::Error),
}

pub(super) fn serve(config: BridgeConfig) -> Result<(), BridgeError> {
    let BridgeConfig { link, tick } = config;
    let (wiring, channels) = ui_channel();
    let (client, feed) = LinkClient::start(link, wiring)?;
    let commands = spawn_stdin_reader();
    let mut session = Session::new(Attachment::new(channels, feed));
    let mut out = BufWriter::new(io::stdout().lock());

    let outcome = write_lines(&mut out, &[hello_line(&revision(), tick)])
        .and_then(|()| pump(&mut session, &commands, tick, &mut out));
    client.shutdown();
    write_lines(&mut out, &[Encoder::bye_line(outcome?)])
}

/// Steps the session and writes its lines once per tick, and applies each command as it arrives,
/// until stdin ends. It waits on stdin rather than sleeping, so a closed stdin ends the bridge at
/// once instead of after the rest of a tick. Returns why it stopped.
fn pump(
    session: &mut Session,
    commands: &Receiver<StdinEvent>,
    tick: Duration,
    out: &mut impl Write,
) -> Result<&'static str, BridgeError> {
    let mut next_tick = Instant::now();
    loop {
        match commands.recv_timeout(next_tick.saturating_duration_since(Instant::now())) {
            Ok(StdinEvent::Line(line)) => apply_line(session, &line),
            Ok(StdinEvent::Closed) | Err(RecvTimeoutError::Disconnected) => {
                return Ok(STDIN_CLOSED);
            }
            Ok(StdinEvent::Failed(error)) => {
                warn!("bridge stdin failed: {error}");
                return Ok(STDIN_FAILED);
            }
            Err(RecvTimeoutError::Timeout) => {
                next_tick = Instant::now() + tick;
                session.step();
                write_lines(out, &session.lines())?;
            }
        }
    }
}

fn apply_line(session: &mut Session, line: &str) {
    match Command::parse(line) {
        Ok(command) => session.apply(command),
        Err(error) => {
            let shown: String = line.trim_end().chars().take(LOGGED_COMMAND_CHARS).collect();
            warn!("bridge command ignored ({error}): {shown}");
        }
    }
}

fn write_lines(out: &mut impl Write, lines: &[String]) -> Result<(), BridgeError> {
    for line in lines {
        out.write_all(line.as_bytes())
            .and_then(|()| out.write_all(b"\n"))
            .map_err(BridgeError::Stdout)?;
    }
    out.flush().map_err(BridgeError::Stdout)
}

/// Reading stdin blocks, so it runs on its own thread and hands lines over a channel. It logs
/// nothing and registers no log lane; the main thread reports what it learns.
fn spawn_stdin_reader() -> Receiver<StdinEvent> {
    let (sender, receiver) = sync_channel(STDIN_CAPACITY);
    thread::Builder::new()
        .name("polysim-bridge-stdin".to_owned())
        .spawn(move || read_stdin(&sender))
        .expect("failed to spawn the bridge stdin thread");
    receiver
}

fn read_stdin(sender: &SyncSender<StdinEvent>) {
    let mut input = io::stdin().lock();
    let mut line = String::new();
    loop {
        line.clear();
        let event = match input.read_line(&mut line) {
            Ok(0) => StdinEvent::Closed,
            Ok(_) => StdinEvent::Line(line.clone()),
            // A line that is not UTF-8 is a bad command, not the end of the conversation.
            Err(error) if error.kind() == ErrorKind::InvalidData => StdinEvent::Line(String::new()),
            Err(error) => StdinEvent::Failed(error),
        };
        let is_last = matches!(event, StdinEvent::Closed | StdinEvent::Failed(_));
        if sender.send(event).is_err() || is_last {
            return;
        }
    }
}

/// The short git revision the bridge was run from, for the page to show. The output is captured and
/// git gets no stdin, so it can neither print on our stdout nor read the page's commands.
fn revision() -> String {
    Process::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}
