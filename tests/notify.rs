//! Desktop notifications fire from the daemon when a session starts needing
//! the user, once per transition, and can be turned off.

mod common;
use common::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A fake notifier that appends its arguments, one call per line.
fn recorder(dir: &Path) -> (PathBuf, PathBuf) {
    let log = dir.join("notifications.log");
    let script = dir.join("notify.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s|' \"$@\" >> '{}'\necho >> '{}'\n",
            log.display(),
            log.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script, log)
}

fn lines(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

fn wait_lines(log: &Path, n: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(3);
    while lines(log).len() < n && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    lines(log)
}

fn send(d: &Daemon, seq: &str, name: &str) {
    finish(
        spawn_hook(&d.socket, &fixture(seq, name), &[]),
        Duration::from_secs(3),
    );
}

#[test]
fn needing_approval_and_finishing_each_notify_once() {
    let dir = tempfile::tempdir().unwrap();
    let (script, log) = recorder(dir.path());
    let d = start_daemon_with(&[("TAPIN_NOTIFY_CMD", &script)], &[]);
    send(&d, "basic-turn", "02-UserPromptSubmit.json");
    send(&d, "basic-turn", "05-PreToolUse.json");
    assert!(wait_lines(&log, 1).is_empty(), "working is not news");

    let hook = spawn_hook(
        &d.socket,
        &fixture("permission-turn", "04-PermissionRequest.json"),
        &[],
    );
    let got = wait_lines(&log, 1);
    assert_eq!(got.len(), 1);
    assert!(
        got[0].contains("demo") && got[0].contains("touch approved.txt"),
        "{got:?}"
    );

    send(&d, "permission-turn", "05-PostToolUse.json");
    finish(hook, Duration::from_secs(3));
    send(&d, "basic-turn", "09-Stop.json");
    send(&d, "basic-turn", "09-Stop.json"); // same state again: no repeat
    let got = wait_lines(&log, 3);
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(
        got[1].contains("demo") && got[1].contains("hello"),
        "done summary: {got:?}"
    );
}

#[test]
fn notifications_can_be_turned_off_by_flag() {
    let dir = tempfile::tempdir().unwrap();
    let (script, log) = recorder(dir.path());
    let d = start_daemon_with(&[("TAPIN_NOTIFY_CMD", &script)], &["--no-notify"]);
    send(&d, "basic-turn", "09-Stop.json");
    std::thread::sleep(Duration::from_millis(500));
    assert!(lines(&log).is_empty());
}

#[test]
fn notifications_can_be_turned_off_or_narrowed_in_config() {
    let dir = tempfile::tempdir().unwrap();
    let (script, log) = recorder(dir.path());
    let config_home = dir.path().join("config");
    std::fs::create_dir_all(config_home.join("tapin")).unwrap();
    std::fs::write(
        config_home.join("tapin/config.toml"),
        "notify_on = [\"needs\"]\n",
    )
    .unwrap();
    let d = start_daemon_with(
        &[
            ("TAPIN_NOTIFY_CMD", &script),
            ("XDG_CONFIG_HOME", &config_home),
        ],
        &[],
    );
    send(&d, "basic-turn", "09-Stop.json");
    std::thread::sleep(Duration::from_millis(500));
    assert!(lines(&log).is_empty(), "done is off: {:?}", lines(&log));
    let hook = spawn_hook(
        &d.socket,
        &fixture("permission-turn", "04-PermissionRequest.json"),
        &[],
    );
    assert_eq!(wait_lines(&log, 1).len(), 1);
    let _ = finish_or_kill(hook);
}

fn finish_or_kill(mut c: std::process::Child) -> Option<std::process::ExitStatus> {
    let _ = c.kill();
    c.wait().ok()
}

#[test]
fn a_new_request_right_after_another_still_notifies() {
    let dir = tempfile::tempdir().unwrap();
    let (script, log) = recorder(dir.path());
    let d = start_daemon_with(&[("TAPIN_NOTIFY_CMD", &script)], &[]);
    let first = spawn_hook(
        &d.socket,
        &fixture("permission-turn", "04-PermissionRequest.json"),
        &[],
    );
    assert_eq!(wait_lines(&log, 1).len(), 1);
    // Denied in the terminal, and Claude immediately asks for something else.
    let second = spawn_hook(
        &d.socket,
        &fixture("permission-turn", "04-PermissionRequest.json"),
        &[],
    );
    assert_eq!(wait_lines(&log, 2).len(), 2);
    let _ = finish_or_kill(first);
    let _ = finish_or_kill(second);
}

#[test]
fn the_hook_waits_as_long_as_the_config_says() {
    let dir = tempfile::tempdir().unwrap();
    let config_home = dir.path().join("config");
    std::fs::create_dir_all(config_home.join("tapin")).unwrap();
    std::fs::write(
        config_home.join("tapin/config.toml"),
        "approval_timeout_secs = 1\n",
    )
    .unwrap();
    let d = start_daemon();
    let started = Instant::now();
    let hook = spawn_hook(
        &d.socket,
        &fixture("permission-turn", "04-PermissionRequest.json"),
        &[("XDG_CONFIG_HOME", config_home.to_str().unwrap())],
    );
    let out = finish(hook, Duration::from_secs(5));
    assert!(out.stdout.is_empty());
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}
