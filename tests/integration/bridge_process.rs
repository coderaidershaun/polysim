//! The bridge as a process: spawned with no engine to attach to, its stdout is protocol lines from
//! hello to bye and nothing else.
//!
//! Run: `cargo test --test integration -- --ignored stdout_carries_only_protocol_lines_from_hello_to_bye`

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command as Process, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use serde_json::Value;

/// Generous for a first launch of a freshly built binary and a link worker's first status, and short
/// enough that a hung bridge fails the suite rather than holding it.
const WAIT: Duration = Duration::from_secs(30);

fn parse(line: &str) -> Value {
    assert!(
        line.starts_with(r#"{"kind":""#),
        "every protocol line opens with its kind, got {line}"
    );
    assert!(!line.contains('\n'), "a protocol line is one line");
    serde_json::from_str(line).expect("a protocol line is json")
}

fn count(value: &Value) -> usize {
    value.as_array().map_or(0, Vec::len)
}

/// Kills the bridge if a test fails before it ends, so a panic never leaves one running.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        if self.0.kill().is_ok() {
            self.0.wait().ok();
        }
    }
}

/// Jet reads the bridge's stdout as one protocol line per line. A stray print, a log line or usage
/// text on it would corrupt that stream, and a bridge that skipped its hello or its bye would leave
/// the page unable to tell a restart from a crash.
#[ignore = "spawns the bridge binary, which opens a udp socket — agent-run: cargo test --test integration -- --ignored stdout_carries_only_protocol_lines_from_hello_to_bye"]
#[test]
fn stdout_carries_only_protocol_lines_from_hello_to_bye() {
    let child = Process::new(env!("CARGO_BIN_EXE_polysim-bridge"))
        .args([
            "--strategy",
            "strat-micro-recorder",
            "--link",
            "127.0.0.1:9310",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the bridge binary starts");
    let mut bridge = Running(child);
    let stdout = bridge.0.stdout.take().expect("stdout is piped");
    let (sender, lines) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if sender.send(line).is_err() {
                return;
            }
        }
    });
    let next = || {
        lines
            .recv_timeout(WAIT)
            .expect("the bridge writes a line in time")
            .expect("stdout is utf-8")
    };

    let hello = parse(&next());
    assert_eq!(hello["kind"], "hello");
    assert_eq!(hello["protocol"], 1);
    assert_eq!(hello["bridge"], "polysim-bridge");
    assert_eq!(hello["tickMs"], 16);
    assert!(hello["polysim"].is_string(), "a revision, even if unknown");
    assert_eq!(count(&hello["groupings"]["ticks"]), 7);
    assert_eq!(count(&hello["groupings"]["bps"]), 8);
    assert_eq!(hello["levels"]["min"], 12);
    assert_eq!(hello["levels"]["max"], 30);
    assert_eq!(hello["levels"]["default"], 12);

    let link = parse(&next());
    assert_eq!(link["kind"], "link");
    assert_eq!(link["peer"], "127.0.0.1:9310");
    assert_eq!(link["peerCount"], 1);
    assert_eq!(
        keys(&link),
        [
            "button",
            "chips",
            "connection",
            "kind",
            "peer",
            "peerClickable",
            "peerCount",
            "peerIndex",
            "session"
        ],
        "the link line carries exactly the contracted keys"
    );

    let lifecycle = parse(&next());
    assert_eq!(lifecycle["kind"], "lifecycle");
    assert_eq!(lifecycle["screen"], "starting");
    assert_eq!(
        keys(&lifecycle),
        [
            "detail",
            "detailTone",
            "kind",
            "screen",
            "session",
            "title",
            "titleTone"
        ],
        "the lifecycle line carries exactly the contracted keys"
    );

    let mut stdin = bridge.0.stdin.take().expect("stdin is piped");
    stdin.flush().expect("stdin flushes");
    drop(stdin);
    let mut last = None;
    loop {
        match lines.recv_timeout(WAIT) {
            Ok(line) => last = Some(parse(&line.expect("stdout is utf-8"))),
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => panic!("the bridge did not exit after stdin closed"),
        }
    }
    let bye = last.expect("the bridge says bye");
    assert_eq!(bye["kind"], "bye");
    assert_eq!(bye["reason"], "stdin closed");
    assert!(
        bridge.0.wait().expect("the bridge is reaped").success(),
        "closing stdin is a clean exit"
    );
}

fn keys(line: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = line
        .as_object()
        .expect("a protocol line is an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}
