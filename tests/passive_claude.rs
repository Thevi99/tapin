//! Passive mode for Claude Code: `claude agents --json` (the supported way to
//! read session state from outside) → observations → the inbox.

mod common;
use common::*;
use std::path::Path;
use tapin::event::AgentKind;
use tapin::inbox::Inbox;
use tapin::passive::claude::parse_agents;

fn agents_json() -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/passive/claude-agents.json"),
    )
    .unwrap()
}

#[test]
fn agents_json_maps_to_inbox_states() {
    let mut inbox = Inbox::default();
    inbox.observe(AgentKind::Claude, parse_agents(&agents_json()).unwrap(), 0);
    // Rows are labelled by Claude's session name, so sessions in one folder differ.
    insta::assert_snapshot!(inbox.render_plain(1), @"
    NEEDS   claude  fix flaky test  background: blocked  1s
    NEEDS   claude  web  waiting: permission prompt  1s
    WORKING claude  refactor auth  background: working  1s
    WORKING claude  cli  busy  1s
    IDLE    claude  docs  idle  1s
    ");
}

#[test]
fn hooked_sessions_are_not_overwritten_and_vanished_ones_go() {
    let mut inbox = Inbox::default();
    // The "cli" session also reports through hooks: hook events win.
    let start = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude/basic-turn/09-Stop.json"),
    )
    .unwrap();
    let mut raw: serde_json::Value = serde_json::from_str(&start).unwrap();
    raw["session_id"] = "22222222-0000-0000-0000-000000000002".into();
    raw["cwd"] = "/home/dev/projects/cli".into();
    inbox.apply(tapin::adapters::claude::parse(&raw).unwrap(), 0);
    let obs = parse_agents(&agents_json()).unwrap();
    inbox.observe(AgentKind::Claude, obs, 1);
    assert!(
        inbox.render_plain(2).contains("DONE    claude  cli"),
        "{}",
        inbox.render_plain(2)
    );
    // Next poll: only the docs session is still alive.
    let only_docs = parse_agents(&agents_json())
        .unwrap()
        .into_iter()
        .filter(|o| o.cwd.ends_with("docs"))
        .collect();
    inbox.observe(AgentKind::Claude, only_docs, 3);
    insta::assert_snapshot!(inbox.render_plain(4), @"
    DONE    claude  cli  `a.txt` contains \"hello\". The first `ls -la` pri…  4s
    IDLE    claude  docs  idle  3s
    ");
}

#[test]
fn the_daemon_polls_claude_without_any_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("claude");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\ncat '{}'\n",
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/passive/claude-agents.json")
                .display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let d = start_daemon_with(&[("TAPIN_CLAUDE_CMD", &fake)], &[]);
    let mut tui = Client::connect(&d.socket);
    let rows = tui.wait_for(|r| r.as_array().is_some_and(|a| a.len() == 5));
    let web = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["project"] == "web")
        .unwrap();
    assert_eq!(web["state"], "NeedsPermission");
    assert!(
        web["request_id"].is_null(),
        "passive rows can't be answered from tapin"
    );
    assert_eq!(web["passive"], true);
}

#[test]
fn an_ended_session_stays_ended_while_it_is_still_listed() {
    let mut inbox = Inbox::default();
    let obs = parse_agents(&agents_json()).unwrap();
    inbox.observe(AgentKind::Claude, obs.clone(), 0);
    inbox.end("22222222-0000-0000-0000-000000000003", 1);
    // Still listed for a long time (e.g. the reaper ended it): it must not come back.
    let other = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/claude/basic-turn/01-SessionStart.json"),
    )
    .unwrap();
    let other = tapin::adapters::claude::parse(&serde_json::from_str(&other).unwrap()).unwrap();
    for t in [5, 200, 400] {
        inbox.apply(other.clone(), t); // unrelated hook traffic (old tombstones get pruned)
        inbox.observe(AgentKind::Claude, obs.clone(), t);
        assert!(
            !inbox.render_plain(t).contains("docs"),
            "t={t}: {}",
            inbox.render_plain(t)
        );
    }
}

#[test]
fn a_passive_guess_never_changes_a_hooked_sessions_pid() {
    let mut inbox = Inbox::default();
    let raw = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/claude/basic-turn/01-SessionStart.json"),
    )
    .unwrap();
    let mut raw: serde_json::Value = serde_json::from_str(&raw).unwrap();
    raw["session_id"] = "22222222-0000-0000-0000-000000000002".into();
    inbox.apply(tapin::adapters::claude::parse(&raw).unwrap(), 0);
    inbox.observe(AgentKind::Claude, parse_agents(&agents_json()).unwrap(), 1);
    let hooked = "22222222-0000-0000-0000-000000000002";
    assert!(
        inbox.agents().iter().all(|(id, _, _)| id != hooked),
        "the hook reported no pid, so a passive guess must not supply one: {:?}",
        inbox.agents()
    );
}

#[test]
fn passive_claude_rows_can_be_jumped_to_by_tty() {
    use tapin::passive::Observation;
    let mut inbox = Inbox::default();
    let obs = vec![Observation {
        session_id: "s1".into(),
        cwd: "/home/dev".into(),
        state: tapin::inbox::State::Idle,
        detail: "idle".into(),
        pid: Some(4242),
        label: Some("dev-37".into()),
        tty: Some("/dev/ttys004".into()),
        transcript: None,
        background: None,
    }];
    inbox.observe(AgentKind::Claude, obs, 0);
    let rows = inbox.rows(1);
    assert_eq!(rows[0].project, "dev-37");
    assert_eq!(rows[0].location.tty.as_deref(), Some("/dev/ttys004"));
}

#[test]
fn background_sessions_carry_the_id_claude_attach_takes() {
    let obs = parse_agents(&agents_json()).unwrap();
    let bg: Vec<_> = obs.iter().filter(|o| o.background.is_some()).collect();
    assert!(!bg.is_empty());
    let mut inbox = Inbox::default();
    inbox.observe(AgentKind::Claude, obs.clone(), 0);
    let rows = inbox.rows(1);
    let row = rows
        .iter()
        .find(|r| r.session_id == bg[0].session_id)
        .unwrap();
    assert_eq!(row.location.claude_background, bg[0].background);
}

#[test]
fn a_background_session_whose_hooks_reported_first_still_opens_with_attach() {
    let mut inbox = Inbox::default();
    let obs = parse_agents(&agents_json()).unwrap();
    let bg = obs.iter().find(|o| o.background.is_some()).unwrap().clone();
    // Its hook reports first, under the same session id.
    let start = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/claude/basic-turn/01-SessionStart.json"),
    )
    .unwrap();
    let mut raw: serde_json::Value = serde_json::from_str(&start).unwrap();
    raw["session_id"] = bg.session_id.clone().into();
    inbox.apply(tapin::adapters::claude::parse(&raw).unwrap(), 0);
    inbox.observe(AgentKind::Claude, obs, 1);
    let row = inbox.row(&bg.session_id, 2).unwrap();
    assert_eq!(row.location.claude_background, bg.background);
}

#[test]
fn merging_a_location_never_forgets_the_background_id() {
    let mut loc = tapin::location::Location {
        claude_background: Some("bg01".into()),
        ..Default::default()
    };
    loc.merge(&tapin::location::Location::default());
    assert_eq!(loc.claude_background.as_deref(), Some("bg01"));
}
