//! Seam B for approvals: a real PermissionRequest hook waits while a TUI-like
//! client approves or denies through the daemon.

mod common;
use common::*;
use std::time::Duration;

fn permission_payload() -> String {
    fixture("permission-turn", "04-PermissionRequest.json")
}

/// The hook must still be blocked waiting for an answer.
fn assert_waiting(hook: &mut std::process::Child) {
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        hook.try_wait().unwrap().is_none(),
        "hook did not wait for a decision"
    );
}

fn decision_of(out: &std::process::Output) -> serde_json::Value {
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("hook printed JSON");
    assert_eq!(
        v["hookSpecificOutput"]["hookEventName"],
        "PermissionRequest"
    );
    v["hookSpecificOutput"]["decision"].clone()
}

#[test]
fn approving_in_tapin_allows_the_tool() {
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    let hook = spawn_hook(&d.socket, &permission_payload(), &[]);
    let rows = tui.wait_for(|r| pending_request(r).is_some());
    tui.decide(pending_request(&rows).unwrap(), true);
    let out = finish(hook, Duration::from_secs(3));
    assert!(out.status.success());
    assert_eq!(decision_of(&out)["behavior"], "allow");
    tui.wait_for(|r| !has_state(r, "NeedsPermission"));
}

#[test]
fn denying_in_tapin_denies_the_tool() {
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    let hook = spawn_hook(&d.socket, &permission_payload(), &[]);
    let rows = tui.wait_for(|r| pending_request(r).is_some());
    tui.decide(pending_request(&rows).unwrap(), false);
    let out = finish(hook, Duration::from_secs(3));
    assert!(out.status.success());
    let decision = decision_of(&out);
    assert_eq!(decision["behavior"], "deny");
    assert!(decision["message"]
        .as_str()
        .is_some_and(|m| m.contains("tapin")));
}

#[test]
fn answering_in_the_terminal_releases_the_waiting_hook() {
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    let mut hook = spawn_hook(&d.socket, &permission_payload(), &[]);
    tui.wait_for(|r| has_state(r, "NeedsPermission"));
    assert_waiting(&mut hook);
    // The user answered in Claude's own dialog, so the tool ran.
    let post = spawn_hook(
        &d.socket,
        &fixture("permission-turn", "05-PostToolUse.json"),
        &[],
    );
    finish(post, Duration::from_secs(3));
    let out = finish(hook, Duration::from_secs(3));
    assert!(out.status.success());
    assert!(out.stdout.is_empty(), "no decision: Claude already has one");
    tui.wait_for(|r| has_state(r, "Working"));
}

#[test]
fn unanswered_request_times_out_without_a_decision() {
    let d = start_daemon();
    let mut hook = spawn_hook(
        &d.socket,
        &permission_payload(),
        &[("TAPIN_APPROVAL_TIMEOUT_SECS", "1")],
    );
    assert_waiting(&mut hook);
    let out = finish(hook, Duration::from_secs(4));
    assert!(out.status.success());
    assert!(out.stdout.is_empty(), "timeout must never deny");
}

#[test]
fn daemon_dying_mid_wait_releases_the_hook() {
    let mut d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    let mut hook = spawn_hook(&d.socket, &permission_payload(), &[]);
    tui.wait_for(|r| has_state(r, "NeedsPermission"));
    assert_waiting(&mut hook);
    d.child.kill().unwrap();
    let out = finish(hook, Duration::from_secs(3));
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}

#[test]
fn with_two_tuis_the_first_answer_wins() {
    let d = start_daemon();
    let mut a = Client::connect(&d.socket);
    let mut b = Client::connect(&d.socket);
    let hook = spawn_hook(&d.socket, &permission_payload(), &[]);
    let id = pending_request(&a.wait_for(|r| pending_request(r).is_some())).unwrap();
    b.wait_for(|r| pending_request(r) == Some(id));
    a.decide(id, true);
    let out = finish(hook, Duration::from_secs(3));
    assert_eq!(decision_of(&out)["behavior"], "allow");
    // B's late answer to the same request is a no-op.
    b.decide(id, false);
    b.wait_for(|r| !has_state(r, "NeedsPermission"));
}

#[test]
fn an_answer_to_an_old_request_never_reaches_a_newer_one() {
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    let first = spawn_hook(&d.socket, &permission_payload(), &[]);
    let old = pending_request(&tui.wait_for(|r| pending_request(r).is_some())).unwrap();
    // A second request from the same session replaces the first before the user answers.
    let mut second = spawn_hook(&d.socket, &permission_payload(), &[]);
    tui.wait_for(|r| pending_request(r).is_some_and(|id| id != old));
    let out = finish(first, Duration::from_secs(3));
    assert!(
        out.stdout.is_empty(),
        "replaced request falls back to Claude's dialog"
    );
    tui.decide(old, true);
    assert_waiting(&mut second);
    let _ = second.kill();
}

#[test]
fn a_late_pre_tool_use_does_not_release_the_waiting_hook() {
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    let mut hook = spawn_hook(&d.socket, &permission_payload(), &[]);
    tui.wait_for(|r| pending_request(r).is_some());
    let pre = spawn_hook(
        &d.socket,
        &fixture("permission-turn", "03-PreToolUse.json"),
        &[],
    );
    finish(pre, Duration::from_secs(3));
    assert_waiting(&mut hook);
    let _ = hook.kill();
}

#[test]
fn answering_after_the_hook_gave_up_changes_nothing() {
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    let hook = spawn_hook(
        &d.socket,
        &permission_payload(),
        &[("TAPIN_APPROVAL_TIMEOUT_SECS", "1")],
    );
    let id = pending_request(&tui.wait_for(|r| pending_request(r).is_some())).unwrap();
    finish(hook, Duration::from_secs(4));
    // Claude's own dialog is still open, but tapin can no longer answer it.
    let rows = tui.wait_for(|r| has_state(r, "NeedsPermission") && pending_request(r).is_none());
    tui.decide(id, true);
    std::thread::sleep(Duration::from_millis(300));
    assert!(has_state(&rows, "NeedsPermission"));
    tui.wait_for(|r| has_state(r, "NeedsPermission"));
}

#[test]
fn an_absurd_timeout_setting_is_capped_not_fatal() {
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    let mut hook = spawn_hook(
        &d.socket,
        &permission_payload(),
        &[("TAPIN_APPROVAL_TIMEOUT_SECS", "18446744073709551615")],
    );
    tui.wait_for(|r| pending_request(r).is_some());
    assert_waiting(&mut hook);
    let _ = hook.kill();
}

#[test]
fn a_different_tool_starting_releases_the_waiting_hook() {
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    let hook = spawn_hook(&d.socket, &permission_payload(), &[]);
    tui.wait_for(|r| pending_request(r).is_some());
    let mut next: serde_json::Value =
        serde_json::from_str(&fixture("permission-turn", "03-PreToolUse.json")).unwrap();
    next["tool_input"]["command"] = "git status".into();
    finish(
        spawn_hook(&d.socket, &next.to_string(), &[]),
        Duration::from_secs(3),
    );
    let out = finish(hook, Duration::from_secs(3));
    assert!(out.stdout.is_empty());
    tui.wait_for(|r| pending_request(r).is_none() && has_state(r, "Working"));
}
