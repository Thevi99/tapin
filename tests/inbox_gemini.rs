//! Gemini CLI (status only): its own hook event names → the shared states.
//! Fixtures follow the documented payloads (Gemini wasn't installed to capture).

use std::path::Path;
use tapin::adapters::hooks;
use tapin::event::AgentKind;
use tapin::inbox::Inbox;

fn replay(n: usize, now: u64) -> String {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gemini/status-turn");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    let mut inbox = Inbox::default();
    for (t, f) in files.iter().take(n).enumerate() {
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap();
        if let Some(ev) = hooks::parse(&raw, AgentKind::Gemini) {
            inbox.apply(ev, t as u64);
        }
    }
    inbox.render_plain(now)
}

#[test]
fn a_gemini_turn_maps_onto_the_inbox() {
    insta::assert_snapshot!(replay(1, 1), @"IDLE    gemini  demo  started  1s");
    insta::assert_snapshot!(replay(2, 2), @"WORKING gemini  demo  thinking  1s");
    insta::assert_snapshot!(replay(3, 3), @"WORKING gemini  demo  run_shell_command(ls -la)  2s");
    insta::assert_snapshot!(replay(4, 4), @"NEEDS   gemini  demo  Gemini wants to run: ls -la  1s");
    insta::assert_snapshot!(replay(5, 5), @"WORKING gemini  demo  run_shell_command(ls -la)  1s");
    insta::assert_snapshot!(replay(6, 6), @"DONE    gemini  demo  The folder has one file, a.txt.  1s");
    insta::assert_snapshot!(replay(7, 7), @"");
}

#[test]
fn gemini_never_waits_on_tapin_for_a_decision() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/gemini/status-turn/04-Notification.json");
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    assert!(!hooks::wants_decision(&raw));
}
