//! Seam A for Codex: real Codex CLI hook payloads → shared adapter → Inbox.

use std::path::Path;
use tapin::adapters::hooks;
use tapin::event::AgentKind;
use tapin::inbox::Inbox;

fn load(seq: &str, name: &str) -> serde_json::Value {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/codex")
        .join(seq)
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// Replays the first `n` fixtures, one per second from t=0, rendered at `now`.
fn replay(n: usize, now: u64) -> String {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codex/approval-turn");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    let mut inbox = Inbox::default();
    for (t, f) in files.iter().take(n).enumerate() {
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap();
        if let Some(ev) = hooks::parse(&raw, AgentKind::Codex) {
            inbox.apply(ev, t as u64);
        }
    }
    inbox.render_plain(now)
}

#[test]
fn a_codex_turn_goes_working_needs_working_done_gone() {
    insta::assert_snapshot!(replay(3, 3), @"WORKING codex  demo  Bash(touch codex-ok.txt)  2s");
    insta::assert_snapshot!(replay(4, 4), @"NEEDS   codex  demo  Bash(touch codex-ok.txt)  1s");
    insta::assert_snapshot!(replay(5, 5), @"WORKING codex  demo  Bash(touch codex-ok.txt)  1s");
    insta::assert_snapshot!(replay(6, 6), @"DONE    codex  demo  Ran `touch codex-ok.txt` successfully.  1s");
    insta::assert_snapshot!(replay(7, 7), @"");
}

#[test]
fn apply_patch_is_summarized_by_the_files_it_touches() {
    let mut raw = load("approval-turn", "04-PermissionRequest.json");
    raw["tool_name"] = "apply_patch".into();
    raw["tool_input"]["command"] =
        "*** Begin Patch\n*** Update File: src/main.rs\n@@\n-a\n+b\n*** Add File: src/new.rs\n+x\n*** End Patch".into();
    let mut inbox = Inbox::default();
    inbox.apply(hooks::parse(&raw, AgentKind::Codex).unwrap(), 0);
    insta::assert_snapshot!(inbox.render_plain(1), @"NEEDS   codex  demo  apply_patch(src/main.rs, src/new.rs)  1s");
}
