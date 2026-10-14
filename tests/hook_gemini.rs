//! Gemini CLI treats any exit code but 0/1 as "deny" and shows stderr to the
//! user, and it has no hook that can answer a permission prompt.

mod common;
use common::*;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn gemini_fixture(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/gemini/status-turn")
            .join(name),
    )
    .unwrap()
}

fn run_gemini_hook(socket: &std::path::Path, payload: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tapin"))
        .args(["hook", "--agent", "gemini", "--socket"])
        .arg(socket)
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
    finish(child, Duration::from_secs(3))
}

#[test]
fn the_gemini_hook_is_silent_and_exits_0_with_or_without_a_daemon() {
    let dir = tempfile::tempdir().unwrap();
    for payload in [
        gemini_fixture("03-BeforeTool.json"),
        "not json at all".to_string(),
    ] {
        let out = run_gemini_hook(&dir.path().join("none.sock"), &payload);
        assert_eq!(out.status.code(), Some(0));
        assert!(out.stdout.is_empty() && out.stderr.is_empty());
    }
    let d = start_daemon();
    let out = run_gemini_hook(&d.socket, &gemini_fixture("03-BeforeTool.json"));
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
}

#[test]
fn a_gemini_permission_notice_shows_needs_but_cannot_be_answered() {
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    let started = Instant::now();
    let out = run_gemini_hook(&d.socket, &gemini_fixture("04-Notification.json"));
    assert!(
        started.elapsed() < Duration::from_millis(800),
        "did not wait"
    );
    assert_eq!(out.status.code(), Some(0));
    let rows = tui.wait_for(|r| has_state(r, "NeedsPermission"));
    assert!(rows[0]["request_id"].is_null(), "no y/n for Gemini: {rows}");
    assert!(
        rows[0]["detail"].as_str().unwrap().contains("ls -la"),
        "{rows}"
    );
}
