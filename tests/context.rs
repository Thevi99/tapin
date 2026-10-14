//! Context % per session, read from the tail of the agent's own transcript.
//! Record shapes come from real Claude Code and Codex CLI files (values made up).

use std::io::Write;
use tapin::context::{claude_context, codex_context, read, Context};

fn ctx(used: u64, window: Option<u64>) -> Option<Context> {
    Some(Context { used, window })
}
use tapin::event::AgentKind;

fn assistant(model: &str, requested: &str, input: u64, created: u64, read: u64) -> String {
    format!(
        r#"{{"type":"assistant","isSidechain":false,"requestedModel":"{requested}","message":{{"model":"{model}","usage":{{"input_tokens":{input},"cache_creation_input_tokens":{created},"cache_read_input_tokens":{read},"output_tokens":900}}}}}}"#
    )
}

#[test]
fn claude_counts_input_and_both_caches_and_leaves_an_unmarked_window_unknown() {
    let tail = [
        r#"{"type":"user","message":{"content":"hi"}}"#.to_string(),
        assistant("claude-sonnet-5-5", "claude-sonnet-5-5", 2, 30_000, 98_000),
    ]
    .join("\n");
    // Some models get 1M without a [1m] marker (seen: claude-opus-4-8 at 257k),
    // so an unmarked window is not assumed to be 200k.
    assert_eq!(claude_context(&tail), ctx(128_002, None));
}

#[test]
fn claude_with_a_1m_model_uses_the_1m_window() {
    let tail = assistant("claude-opus-5-5", "claude-opus-5-5[1m]", 2, 131_328, 972);
    assert_eq!(claude_context(&tail), ctx(132_302, Some(1_000_000)));
    // Older records carry the suffix on the model itself.
    let older = assistant("claude-opus-5[1m]", "", 0, 500_000, 0);
    assert_eq!(claude_context(&older), ctx(500_000, Some(1_000_000)));
}

#[test]
fn claude_takes_the_last_real_turn_skipping_subagents_and_synthetic_replies() {
    let tail = [
        assistant("claude-sonnet-5-5", "claude-sonnet-5-5", 0, 0, 100_000),
        assistant("claude-sonnet-5-5", "claude-sonnet-5-5", 0, 0, 150_000)
            .replace(r#""isSidechain":false"#, r#""isSidechain":true"#),
        assistant("<synthetic>", "", 0, 0, 0),
        r#"{"type":"user","message":{"content":"x"}}"#.to_string(),
    ]
    .join("\n");
    assert_eq!(claude_context(&tail), ctx(100_000, None));
}

fn token_count(last_total: u64, window: u64) -> String {
    format!(
        r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"total_tokens":63836717}},"last_token_usage":{{"input_tokens":106461,"total_tokens":{last_total}}},"model_context_window":{window}}}}}}}"#
    )
}

#[test]
fn codex_uses_the_last_turn_against_its_reported_window() {
    let tail = [
        token_count(50_000, 258_400),
        token_count(106_480, 258_400),
        r#"{"type":"event_msg","payload":{"type":"token_count","info":null}}"#.to_string(),
    ]
    .join("\n");
    assert_eq!(codex_context(&tail), ctx(106_480, Some(258_400)));
}

#[test]
fn nothing_known_means_no_number() {
    assert_eq!(claude_context(r#"{"type":"user"}"#), None);
    assert_eq!(codex_context(""), None);
    assert_eq!(
        read(AgentKind::Claude, "/nonexistent/x.jsonl".as_ref()),
        None
    );
    assert_eq!(
        read(AgentKind::Gemini, "/nonexistent/x.json".as_ref()),
        None
    );
}

#[test]
fn a_record_far_from_the_end_of_a_big_transcript_is_still_found() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(f, "{}", assistant("claude-sonnet-5-5", "", 0, 0, 20_000)).unwrap();
    // A huge tool result after the last reply (seen: 896 KB).
    let filler = format!(
        r#"{{"type":"user","message":{{"content":"{}"}}}}"#,
        "x".repeat(900_000)
    );
    writeln!(f, "{filler}").unwrap();
    assert_eq!(read(AgentKind::Claude, f.path()), ctx(20_000, None));
}

#[test]
fn hook_sessions_carry_their_transcript_and_rows_show_the_context() {
    use tapin::adapters::claude;
    use tapin::inbox::Inbox;
    let raw: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/claude/basic-turn/02-UserPromptSubmit.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let mut inbox = Inbox::default();
    let ev = claude::parse(&raw).unwrap();
    let id = ev.session_id.clone();
    inbox.apply(ev, 0);
    let wanted = inbox.transcripts();
    assert_eq!(wanted.len(), 1);
    assert_eq!(wanted[0].0, id);
    assert_eq!(wanted[0].1, AgentKind::Claude);
    assert_eq!(
        wanted[0].2,
        std::path::PathBuf::from(raw["transcript_path"].as_str().unwrap())
    );
    assert_eq!(inbox.rows(1)[0].context, None);
    inbox.set_context(&id, ctx(64, Some(100)));
    assert_eq!(inbox.rows(1)[0].context, ctx(64, Some(100)));
}

#[test]
fn a_passive_claude_session_finds_its_transcript_by_folder_or_by_id() {
    use tapin::passive::claude::transcript_for;
    let projects = tempfile::tempdir().unwrap();
    let slugged = projects.path().join("-home-dev-my-app");
    std::fs::create_dir_all(&slugged).unwrap();
    std::fs::write(slugged.join("s1.jsonl"), "").unwrap();
    assert_eq!(
        transcript_for(projects.path(), "/home/dev/my.app", "s1"),
        Some(slugged.join("s1.jsonl"))
    );
    // A folder named some other way is still found by the session id.
    let other = projects.path().join("whatever");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("s2.jsonl"), "").unwrap();
    assert_eq!(
        transcript_for(projects.path(), "/home/dev/x y", "s2"),
        Some(other.join("s2.jsonl"))
    );
    assert_eq!(transcript_for(projects.path(), "/home/dev", "nope"), None);
}

#[path = "common/mod.rs"]
mod common;

#[test]
fn the_daemon_shows_context_for_a_hooked_session() {
    use common::*;
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("t.jsonl");
    std::fs::write(
        &transcript,
        format!("{}\n", assistant("claude-sonnet-5-5", "", 0, 0, 160_000)),
    )
    .unwrap();
    let mut payload: serde_json::Value =
        serde_json::from_str(&fixture("basic-turn", "02-UserPromptSubmit.json")).unwrap();
    payload["transcript_path"] = transcript.to_str().unwrap().into();
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    let hook = spawn_hook(&d.socket, &payload.to_string(), &[]);
    let _ = finish(hook, std::time::Duration::from_secs(3));
    let rows = tui.wait_for(|r| r[0]["context"]["used"] == 160_000);
    assert!(rows[0]["context"]["window"].is_null());
}

#[test]
fn after_a_compact_the_post_compact_size_is_shown_until_the_next_turn() {
    let boundary = r#"{"type":"system","subtype":"compact_boundary","isSidechain":false,"compactMetadata":{"trigger":"manual","preTokens":868730,"postTokens":21743}}"#;
    let tail = [
        assistant("claude-opus-5-5", "claude-opus-5-5[1m]", 0, 0, 868_000),
        boundary.to_string(),
    ]
    .join("\n");
    // The window is still known from the turn before the compact.
    assert_eq!(claude_context(&tail), ctx(21_743, Some(1_000_000)));
    // With no turn before it in the tail, only the size is known.
    assert_eq!(claude_context(boundary), ctx(21_743, None));
    let next = [
        tail,
        assistant("claude-opus-5-5", "claude-opus-5-5[1m]", 0, 0, 30_000),
    ]
    .join("\n");
    assert_eq!(claude_context(&next), ctx(30_000, Some(1_000_000)));
}

#[test]
fn a_record_past_the_first_read_in_a_file_over_the_cap_is_found() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    // 5 MB of older turns that would match, then the newest, then 300 KB after it:
    // the first 256 KB read misses, the second starts mid-line inside the filler.
    let old = assistant("claude-sonnet-5-5", "", 0, 0, 1_000);
    for _ in 0..(5_000_000 / old.len()) {
        writeln!(f, "{old}").unwrap();
    }
    writeln!(f, "{}", assistant("claude-sonnet-5-5", "", 0, 0, 42_000)).unwrap();
    let filler = format!(
        r#"{{"type":"user","message":{{"content":"{}"}}}}"#,
        "x".repeat(300_000)
    );
    writeln!(f, "{filler}").unwrap();
    assert_eq!(read(AgentKind::Claude, f.path()), ctx(42_000, None));
}

#[test]
fn special_files_are_never_opened() {
    // A FIFO would block open() forever and wedge the poll thread.
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("fifo");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let started = std::time::Instant::now();
    assert_eq!(read(AgentKind::Claude, &fifo), None);
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
}

#[test]
fn the_cache_rereads_a_file_only_when_it_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    std::fs::write(&path, format!("{}\n", assistant("m", "", 0, 0, 5_000))).unwrap();
    let sessions = vec![("s".to_string(), AgentKind::Claude, path.clone())];
    let mut cache = tapin::context::Cache::default();
    assert_eq!(cache.read_all(&sessions)[0].1, ctx(5_000, None));
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(f, "{}", assistant("m", "", 0, 0, 7_000)).unwrap();
    assert_eq!(cache.read_all(&sessions)[0].1, ctx(7_000, None));
}

#[test]
fn a_hooked_session_keeps_the_hooks_transcript_over_a_passive_guess() {
    use tapin::adapters::claude;
    use tapin::inbox::Inbox;
    use tapin::passive::Observation;
    let mut raw: serde_json::Value =
        serde_json::from_str(&common::fixture("basic-turn", "02-UserPromptSubmit.json")).unwrap();
    raw["transcript_path"] = "/from/hook.jsonl".into();
    let ev = claude::parse(&raw).unwrap();
    let (id, cwd) = (ev.session_id.clone(), ev.cwd.clone());
    let mut inbox = Inbox::default();
    inbox.apply(ev, 0);
    let obs = Observation {
        session_id: id,
        cwd,
        state: tapin::inbox::State::Working,
        detail: "busy".into(),
        pid: None,
        label: None,
        tty: None,
        transcript: Some("/from/passive.jsonl".into()),
    };
    inbox.observe(AgentKind::Claude, vec![obs], 1);
    assert_eq!(
        inbox.transcripts()[0].2,
        std::path::PathBuf::from("/from/hook.jsonl")
    );
}
