//! `hide_dirs`: sessions running under these folders never reach the inbox
//! (by default claude-mem's background observer, which is always busy).

mod common;
use common::*;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tapin::adapters::claude;
use tapin::config::Config;
use tapin::event::AgentKind;
use tapin::inbox::Inbox;
use tapin::passive::claude::parse_agents;

fn permission_payload() -> String {
    fixture("permission-turn", "04-PermissionRequest.json")
}

#[test]
fn claude_mem_observer_is_hidden_by_default_and_tilde_expands() {
    let home = std::env::var("HOME").unwrap();
    let c = Config::parse("").unwrap();
    assert_eq!(
        c.hidden_dirs(),
        vec![PathBuf::from(home).join(".claude-mem")]
    );
    assert!(Config::parse("hide_dirs = []\n")
        .unwrap()
        .hidden_dirs()
        .is_empty());
}

#[test]
fn passive_sessions_in_a_hidden_folder_stay_out_but_siblings_dont() {
    let mut inbox = Inbox::hiding(vec![PathBuf::from("/home/dev/projects/web")]);
    inbox.observe(AgentKind::Claude, parse_agents(&agents_json()).unwrap(), 0);
    let rows = inbox.render_plain(1);
    assert!(!rows.contains(" web "), "{rows}");
    // A sibling whose name merely starts the same is not inside the folder.
    let mut inbox = Inbox::hiding(vec![PathBuf::from("/home/dev/projects/w")]);
    inbox.observe(AgentKind::Claude, parse_agents(&agents_json()).unwrap(), 0);
    assert!(inbox.render_plain(1).contains(" web "));
}

#[test]
fn hook_events_from_a_hidden_folder_are_dropped() {
    let mut inbox = Inbox::hiding(vec![PathBuf::from("/home/dev/projects")]);
    let raw: serde_json::Value = serde_json::from_str(&permission_payload()).unwrap();
    inbox.apply(claude::parse(&raw).unwrap(), 0);
    assert_eq!(inbox.render_plain(1), "");
    assert!(inbox.hides("/home/dev/projects/demo"));
}

#[test]
fn a_request_from_a_hidden_folder_is_passed_back_at_once() {
    let cfg = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cfg.path().join("tapin")).unwrap();
    std::fs::write(
        cfg.path().join("tapin/config.toml"),
        "hide_dirs = [\"/home/dev/projects/demo\"]\n",
    )
    .unwrap();
    let d = start_daemon_with(&[("XDG_CONFIG_HOME", cfg.path())], &[]);
    // An open inbox would normally hold the hook until someone answers.
    let mut tui = Client::connect(&d.socket);
    let hook = spawn_hook(&d.socket, &permission_payload(), &[]);
    let out = finish(hook, Duration::from_secs(3));
    assert!(out.status.success());
    assert!(
        out.stdout.is_empty(),
        "no decision: the agent asks in its own terminal"
    );
    // Snapshots keep coming every second; none of them may show the session.
    let until = std::time::Instant::now() + Duration::from_millis(1500);
    tui.wait_for(|rows| {
        assert_eq!(rows.as_array().map(Vec::len), Some(0), "{rows}");
        std::time::Instant::now() > until
    });
}

fn agents_json() -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/passive/claude-agents.json"),
    )
    .unwrap()
}

#[test]
fn only_plain_tilde_expands_and_empty_or_relative_entries_are_ignored() {
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    let c =
        Config::parse("hide_dirs = [\"~\", \"~bob/x\", \"\", \"rel/dir\", \"/abs\"]\n").unwrap();
    let dirs = c.hidden_dirs();
    assert!(dirs.contains(&home), "{dirs:?}");
    assert!(dirs.contains(&PathBuf::from("/abs")), "{dirs:?}");
    assert!(!dirs.iter().any(|d| d.ends_with("bob/x")), "{dirs:?}");
    assert!(dirs.iter().all(|d| d.is_absolute()), "{dirs:?}");
}

#[test]
fn a_symlinked_hidden_folder_also_matches_its_real_path() {
    let real = tempfile::tempdir().unwrap();
    let link_parent = tempfile::tempdir().unwrap();
    let link = link_parent.path().join("link");
    std::os::unix::fs::symlink(real.path(), &link).unwrap();
    let c = Config::parse(&format!("hide_dirs = [{:?}]\n", link.to_str().unwrap())).unwrap();
    let inbox = Inbox::hiding(c.hidden_dirs());
    let real = real.path().canonicalize().unwrap();
    assert!(inbox.hides(real.join("sub").to_str().unwrap()));
}

#[test]
fn a_session_that_moves_into_a_hidden_folder_leaves_the_inbox() {
    let mut inbox = Inbox::hiding(vec![PathBuf::from("/home/dev/hidden")]);
    let mut raw: serde_json::Value = serde_json::from_str(&permission_payload()).unwrap();
    inbox.apply(claude::parse(&raw).unwrap(), 0);
    assert!(inbox.render_plain(1).contains("NEEDS"));
    raw["cwd"] = "/home/dev/hidden/sub".into();
    inbox.apply(claude::parse(&raw).unwrap(), 1);
    assert_eq!(inbox.render_plain(2), "");
}
