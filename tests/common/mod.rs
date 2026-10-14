//! Shared helpers for cross-process tests: a daemon on a temp socket and hook runners.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub fn fixture(seq: &str, name: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/claude")
        .join(seq)
        .join(name);
    std::fs::read_to_string(p).unwrap()
}

pub struct Daemon {
    pub child: Child,
    pub socket: PathBuf,
    _dir: tempfile::TempDir,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A daemon that trusts this test binary as if it were tapin's TUI, so the
/// headless `Client` can answer requests (debug builds only; see #12).
pub fn start_daemon() -> Daemon {
    start_daemon_with(&[], &[])
}

/// A daemon that trusts only the real tapin binary, as in production.
pub fn start_untrusting_daemon() -> Daemon {
    start_daemon_inner(&[], &[], false)
}

/// A daemon that trusts this test binary but checks the real process tree
/// (the suite otherwise turns that off: see `.cargo/config.toml`).
pub fn start_daemon_with_real_ancestry() -> Daemon {
    REAL_ANCESTRY.with(|r| r.set(true));
    let d = start_daemon_inner(&[], &[], true);
    REAL_ANCESTRY.with(|r| r.set(false));
    d
}

thread_local! {
    static REAL_ANCESTRY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A daemon with extra environment variables and arguments.
pub fn start_daemon_with(envs: &[(&str, &Path)], args: &[&str]) -> Daemon {
    start_daemon_inner(envs, args, true)
}

fn start_daemon_inner(envs: &[(&str, &Path)], args: &[&str], trust_tests: bool) -> Daemon {
    let dir = tempfile::tempdir().unwrap();
    let trusted = if trust_tests {
        std::env::current_exe().unwrap()
    } else {
        std::path::PathBuf::new()
    };
    let socket = dir.path().join("run").join("tapin.sock");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tapin"));
    cmd.args(["daemon", "--socket"])
        .arg(&socket)
        .args(args)
        // Never pop real notifications from tests.
        .env("TAPIN_NOTIFY_CMD", "true")
        .env("XDG_CONFIG_HOME", dir.path().join("no-config"))
        .env("TAPIN_TRUST_PEER_EXE", &trusted)
        .envs(envs.iter().map(|(k, v)| (*k, *v)));
    if REAL_ANCESTRY.with(|r| r.get()) {
        cmd.env_remove("TAPIN_TEST_IGNORE_ANCESTRY");
    }
    let child = cmd.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while UnixStream::connect(&socket).is_err() {
        assert!(Instant::now() < deadline, "daemon never listened");
        std::thread::sleep(Duration::from_millis(20));
    }
    Daemon {
        child,
        socket,
        _dir: dir,
    }
}

/// Starts `tapin hook --agent claude` with `payload` on stdin, without waiting for it.
pub fn spawn_hook(socket: &Path, payload: &str, envs: &[(&str, &str)]) -> Child {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tapin"))
        .args(["hook", "--agent", "claude", "--socket"])
        .arg(socket)
        .envs(envs.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    child
}

/// Waits up to `limit` for a child to exit; kills it and panics otherwise.
pub fn finish(mut child: Child, limit: Duration) -> std::process::Output {
    let deadline = Instant::now() + limit;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("process still running after {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().unwrap()
}

/// A TUI-like client: subscribes and can send decisions.
pub struct Client {
    lines: std::io::Lines<BufReader<UnixStream>>,
    writer: UnixStream,
}

impl Client {
    pub fn connect(socket: &Path) -> Client {
        let s = UnixStream::connect(socket).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut writer = s.try_clone().unwrap();
        writer.write_all(b"{\"type\":\"subscribe\"}\n").unwrap();
        Client {
            lines: BufReader::new(s).lines(),
            writer,
        }
    }

    /// Returns the first snapshot whose rows satisfy `pred`.
    pub fn wait_for(&mut self, pred: impl Fn(&serde_json::Value) -> bool) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(Instant::now() < deadline, "no matching snapshot within 10s");
            let line = self
                .lines
                .next()
                .expect("daemon closed")
                .expect("read timed out");
            let msg: serde_json::Value = serde_json::from_str(&line).unwrap();
            if msg["type"] != "rows" {
                continue; // e.g. the one-time "access" message
            }
            if pred(&msg["rows"]) {
                return msg["rows"].clone();
            }
        }
    }

    /// Answers a question request with one of its option labels.
    pub fn answer(&mut self, request_id: u64, label: &str) {
        let msg = serde_json::json!({"type": "decide", "request_id": request_id, "allow": true, "answer": label});
        self.writer
            .write_all(format!("{msg}\n").as_bytes())
            .unwrap();
    }

    pub fn decide(&mut self, request_id: u64, allow: bool) {
        let msg = serde_json::json!({"type": "decide", "request_id": request_id, "allow": allow});
        self.writer
            .write_all(format!("{msg}\n").as_bytes())
            .unwrap();
    }
}

pub fn has_state(rows: &serde_json::Value, state: &str) -> bool {
    rows.as_array()
        .is_some_and(|a| a.iter().any(|r| r["state"] == state))
}

/// The request id of the first row that has a pending request.
pub fn pending_request(rows: &serde_json::Value) -> Option<u64> {
    rows.as_array()?
        .iter()
        .find_map(|r| r["request_id"].as_u64())
}
