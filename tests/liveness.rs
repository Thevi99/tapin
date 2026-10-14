//! Sessions whose agent process died (crash, kill -9, closed terminal) leave
//! the inbox on their own, even though no SessionEnd ever arrives.

mod common;
use common::*;
use std::process::{Command, Stdio};
use std::time::Duration;

#[test]
fn a_killed_agent_leaves_the_inbox() {
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    let dir = tempfile::tempdir().unwrap();
    // A stand-in agent: a shell launched under the name `claude`, running the
    // hook as its child the way Claude Code does, then staying alive.
    let agent_bin = dir.path().join("claude");
    std::os::unix::fs::symlink("/bin/sh", &agent_bin).unwrap();
    let payload = dir.path().join("start.json");
    std::fs::write(&payload, fixture("basic-turn", "01-SessionStart.json")).unwrap();
    let script = format!(
        "'{}' hook --agent claude --socket '{}' < '{}'; sleep 60",
        env!("CARGO_BIN_EXE_tapin"),
        d.socket.display(),
        payload.display()
    );
    let mut agent = Command::new(&agent_bin)
        .args(["-c", &script])
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let rows = tui.wait_for(|r| r.as_array().is_some_and(|a| !a.is_empty()));
    assert_eq!(
        rows[0]["location"]["agent_pid"].as_u64(),
        Some(agent.id() as u64),
        "the hook found its agent: {rows}"
    );
    agent.kill().unwrap();
    agent.wait().unwrap();
    tui.wait_for(|r| r.as_array().is_some_and(|a| a.is_empty()));
    let _ = Duration::ZERO;
}

#[test]
fn liveness_is_judged_by_existence_and_a_locale_free_start_time() {
    use tapin::location::{is_gone, process_start};
    let me = std::process::id();
    let start = process_start(me).expect("own start time");
    assert!(!is_gone(me, Some(start)));
    assert!(!is_gone(me, Some(start + 2)), "small clock jitter is fine");
    assert!(
        is_gone(me, Some(start + 3600)),
        "a different process now has this pid"
    );
    assert!(!is_gone(1, None), "EPERM still means alive");
    assert!(is_gone(999_999, None));
}
