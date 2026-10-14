//! `tapin status`: one line for a tmux or shell status bar.

mod common;
use common::*;
use std::process::Command;
use std::time::{Duration, Instant};
use tapin::inbox::{Row, State};
use tapin::status::line;

fn with_state(state: State) -> Row {
    Row {
        state,
        ..Default::default()
    }
}

#[test]
fn the_line_counts_what_waits_on_you_and_hides_zeros() {
    let rows = [
        with_state(State::Done),
        with_state(State::NeedsPermission),
        with_state(State::Working),
        with_state(State::Done),
    ];
    assert_eq!(line(&rows), "NEEDS 1 · DONE 2");
    assert_eq!(line(&[with_state(State::Done)]), "DONE 1");
    assert_eq!(line(&[with_state(State::Idle)]), "");
}

fn status(socket: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_tapin"))
        .arg("status")
        .arg("--socket")
        .arg(socket)
        .output()
        .unwrap()
}

#[test]
fn it_reports_a_waiting_request_from_the_daemon() {
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    let mut hook = spawn_hook(
        &d.socket,
        &fixture("permission-turn", "04-PermissionRequest.json"),
        &[],
    );
    tui.wait_for(|r| has_state(r, "NeedsPermission"));
    let out = status(&d.socket);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "NEEDS 1\n");
    let _ = hook.kill();
    let _ = hook.wait();
}

#[test]
fn without_a_daemon_it_prints_nothing_fast_and_starts_none() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("run").join("tapin.sock");
    let started = Instant::now();
    let out = status(&socket);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(out.status.success());
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
    std::thread::sleep(Duration::from_millis(200));
    assert!(!socket.exists(), "status must not start a daemon");
}
