//! Seam A: real Claude Code hook payloads → adapter → Inbox view.

use std::path::Path;
use tapin::adapters::claude;
use tapin::inbox::Inbox;

/// Replays the first `n` fixtures of a captured sequence, one per second
/// starting at t=0, and renders the inbox as seen at `now`.
fn replay(dir: &str, n: usize, now: u64) -> String {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/claude")
        .join(dir);
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    let mut inbox = Inbox::default();
    for (t, f) in files.iter().take(n).enumerate() {
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap();
        if let Some(ev) = claude::parse(&raw) {
            inbox.apply(ev, t as u64);
        }
    }
    inbox.render_plain(now)
}

#[test]
fn new_session_is_idle_under_its_project_name() {
    insta::assert_snapshot!(replay("basic-turn", 1, 5), @"IDLE    claude  demo  started  5s");
}

#[test]
fn running_tool_shows_working_with_tool_summary() {
    // The prompt (t=1) starts WORKING; fixture 5 (t=4) is Bash `ls -la`.
    // Age counts time in the state, not time on the current tool.
    insta::assert_snapshot!(replay("basic-turn", 5, 6), @"WORKING claude  demo  Bash(ls -la)  5s");
}

#[test]
fn finished_turn_shows_done_with_truncated_last_message() {
    // fixture 9 is Stop at t=8; summaries are cut to 48 chars plus an ellipsis
    insta::assert_snapshot!(replay("basic-turn", 9, 10), @r#"DONE    claude  demo  `a.txt` contains "hello". The first `ls -la` pri…  2s"#);
}

#[test]
fn ended_session_leaves_the_inbox() {
    insta::assert_snapshot!(replay("basic-turn", 10, 12), @"");
}

#[test]
fn submitted_prompt_shows_working_before_any_tool() {
    // fixture 2 is UserPromptSubmit at t=1
    insta::assert_snapshot!(replay("basic-turn", 2, 3), @"WORKING claude  demo  thinking  2s");
}

fn permission_as_session(id: &str, project: &str) -> tapin::event::AgentEvent {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/claude/permission-turn/04-PermissionRequest.json");
    let mut raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    raw["session_id"] = id.into();
    raw["cwd"] = format!("/home/dev/projects/{project}").into();
    claude::parse(&raw).unwrap()
}

/// Loads one fixture, re-labelled as a different session in a different project.
fn as_session(fixture: &str, id: &str, project: &str) -> tapin::event::AgentEvent {
    as_session_from("basic-turn", fixture, id, project)
}

fn as_session_from(seq: &str, fixture: &str, id: &str, project: &str) -> tapin::event::AgentEvent {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/claude")
        .join(seq)
        .join(fixture);
    let mut raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    raw["session_id"] = id.into();
    raw["cwd"] = format!("/home/dev/projects/{project}").into();
    claude::parse(&raw).unwrap()
}

#[test]
fn sessions_are_ordered_needs_then_done_then_working_then_idle_oldest_first() {
    let mut inbox = Inbox::default();
    inbox.apply(as_session("01-SessionStart.json", "a", "idle-app"), 0);
    inbox.apply(as_session("02-UserPromptSubmit.json", "b", "busy-api"), 1);
    inbox.apply(as_session("09-Stop.json", "c", "late-done"), 3);
    inbox.apply(as_session("09-Stop.json", "d", "early-done"), 2);
    inbox.apply(permission_as_session("e", "asks-perm"), 4);
    insta::assert_snapshot!(inbox.render_plain(10), @r#"
    NEEDS   claude  asks-perm  Bash(touch approved.txt)  6s
    DONE    claude  early-done  `a.txt` contains "hello". The first `ls -la` pri…  8s
    DONE    claude  late-done  `a.txt` contains "hello". The first `ls -la` pri…  7s
    WORKING claude  busy-api  thinking  9s
    IDLE    claude  idle-app  started  10s
    "#);
}

#[test]
fn multi_line_tool_input_is_summarized_on_one_line() {
    let mut raw: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/claude/basic-turn/05-PreToolUse.json"),
        )
        .unwrap(),
    )
    .unwrap();
    raw["tool_input"]["command"] = "cargo build\ncargo test".into();
    let mut inbox = Inbox::default();
    inbox.apply(claude::parse(&raw).unwrap(), 0);
    insta::assert_snapshot!(inbox.render_plain(1), @"WORKING claude  demo  Bash(cargo build…)  1s");
}

#[test]
fn permission_request_needs_you_with_tool_summary() {
    // fixture 4 is PermissionRequest at t=3
    insta::assert_snapshot!(replay("permission-turn", 4, 5), @"NEEDS   claude  demo  Bash(touch approved.txt)  2s");
}

#[test]
fn request_settled_elsewhere_returns_to_working() {
    // fixture 5 (t=4): the tool ran, so the request was answered in the terminal
    insta::assert_snapshot!(replay("permission-turn", 5, 6), @"WORKING claude  demo  Bash(touch approved.txt)  2s");
}

#[test]
fn a_late_pre_tool_use_does_not_hide_a_pending_request() {
    // Hook processes race: PreToolUse for the same tool can land after PermissionRequest.
    let mut inbox = Inbox::default();
    inbox.apply(permission_as_session("x", "racy"), 1);
    inbox.apply(
        as_session_from("permission-turn", "03-PreToolUse.json", "x", "racy"),
        2,
    );
    insta::assert_snapshot!(inbox.render_plain(3), @"NEEDS   claude  racy  Bash(touch approved.txt)  2s");
}

#[test]
fn a_different_tool_starting_means_the_request_was_answered_elsewhere() {
    // Denied in the agent's own dialog: no PostToolUse, the next tool just starts.
    let mut inbox = Inbox::default();
    inbox.apply(permission_as_session("x", "moved-on"), 1);
    // The trailing PreToolUse for the requested tool is ignored...
    inbox.apply(
        as_session_from("permission-turn", "03-PreToolUse.json", "x", "moved-on"),
        2,
    );
    let mut raw: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/claude/basic-turn/05-PreToolUse.json"),
        )
        .unwrap(),
    )
    .unwrap();
    raw["session_id"] = "x".into();
    raw["cwd"] = "/home/dev/projects/moved-on".into();
    raw["tool_input"]["command"] = "git status".into();
    // ...but a different tool starting settles the request.
    inbox.apply(claude::parse(&raw).unwrap(), 3);
    insta::assert_snapshot!(inbox.render_plain(4), @"WORKING claude  moved-on  Bash(git status)  1s");
}

#[test]
fn a_late_event_after_session_end_does_not_bring_the_row_back() {
    let mut inbox = Inbox::default();
    inbox.apply(as_session("01-SessionStart.json", "x", "gone"), 0);
    inbox.apply(as_session("10-SessionEnd.json", "x", "gone"), 1);
    // Each hook is its own connection, so a Stop can arrive after SessionEnd.
    inbox.apply(as_session("09-Stop.json", "x", "gone"), 2);
    insta::assert_snapshot!(inbox.render_plain(3), @"");
    // A new start (e.g. a resume) is real again.
    inbox.apply(as_session("01-SessionStart.json", "x", "gone"), 4);
    insta::assert_snapshot!(inbox.render_plain(5), @"IDLE    claude  gone  started  1s");
}
