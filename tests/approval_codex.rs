//! Seam B for Codex. Codex runs PermissionRequest hooks *before* it shows its
//! own prompt (openai/codex#39835), so waiting is only worth it when someone
//! can answer in tapin, and only briefly.

mod common;
use common::*;
use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn codex_payload() -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/codex/approval-turn/04-PermissionRequest.json");
    std::fs::read_to_string(p).unwrap()
}

fn spawn_codex_hook(socket: &std::path::Path, envs: &[(&str, &str)]) -> Child {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tapin"))
        .args(["hook", "--agent", "codex", "--socket"])
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
        .write_all(codex_payload().as_bytes())
        .unwrap();
    child
}

#[test]
fn with_no_tapin_open_codex_is_not_kept_waiting() {
    let d = start_daemon();
    let started = Instant::now();
    let out = finish(spawn_codex_hook(&d.socket, &[]), Duration::from_secs(3));
    assert!(out.status.success());
    assert!(
        out.stdout.is_empty(),
        "no decision: Codex shows its own prompt"
    );
    assert!(
        started.elapsed() < Duration::from_millis(800),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn with_tapin_open_y_approves_the_codex_request() {
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    tui.wait_for(|_| true);
    let hook = spawn_codex_hook(&d.socket, &[]);
    let rows = tui.wait_for(|r| pending_request(r).is_some());
    assert_eq!(rows[0]["agent"], "codex");
    tui.decide(pending_request(&rows).unwrap(), true);
    let out = finish(hook, Duration::from_secs(3));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["hookSpecificOutput"]["decision"]["behavior"], "allow");
}

#[test]
fn codex_waits_only_briefly_by_default() {
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    tui.wait_for(|_| true);
    let config_home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(config_home.path().join("tapin")).unwrap();
    std::fs::write(
        config_home.path().join("tapin/config.toml"),
        "codex_approval_timeout_secs = 1\n",
    )
    .unwrap();
    let started = Instant::now();
    let hook = spawn_codex_hook(
        &d.socket,
        &[("XDG_CONFIG_HOME", config_home.path().to_str().unwrap())],
    );
    let out = finish(hook, Duration::from_secs(5));
    assert!(out.stdout.is_empty(), "timeout never decides");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

fn assert_waiting(hook: &mut Child) {
    std::thread::sleep(Duration::from_millis(300));
    assert!(hook.try_wait().unwrap().is_none(), "hook did not wait");
}

#[test]
fn an_inbox_that_was_closed_does_not_count_as_watching() {
    let d = start_daemon();
    {
        let mut tui = Client::connect(&d.socket);
        tui.wait_for(|_| true);
    } // the TUI quits
    std::thread::sleep(Duration::from_millis(200));
    let started = Instant::now();
    let out = finish(spawn_codex_hook(&d.socket, &[]), Duration::from_secs(3));
    assert!(out.stdout.is_empty());
    assert!(
        started.elapsed() < Duration::from_millis(800),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn closing_the_last_inbox_releases_a_waiting_codex_request() {
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    tui.wait_for(|_| true);
    let mut hook = spawn_codex_hook(&d.socket, &[]);
    tui.wait_for(|r| pending_request(r).is_some());
    assert_waiting(&mut hook);
    drop(tui);
    let started = Instant::now();
    let out = finish(hook, Duration::from_secs(3));
    assert!(out.stdout.is_empty(), "Codex asks in its own terminal");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn n_denies_the_codex_request() {
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    tui.wait_for(|_| true);
    let hook = spawn_codex_hook(&d.socket, &[]);
    let rows = tui.wait_for(|r| pending_request(r).is_some());
    tui.decide(pending_request(&rows).unwrap(), false);
    let out = finish(hook, Duration::from_secs(3));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["hookSpecificOutput"]["decision"]["behavior"], "deny");
}

#[test]
fn codex_has_its_own_timeout_override() {
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    tui.wait_for(|_| true);
    let started = Instant::now();
    let hook = spawn_codex_hook(&d.socket, &[("TAPIN_CODEX_APPROVAL_TIMEOUT_SECS", "1")]);
    let out = finish(hook, Duration::from_secs(5));
    assert!(out.stdout.is_empty());
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}
