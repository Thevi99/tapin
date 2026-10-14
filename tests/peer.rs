//! #12: only a real tapin inbox may answer permission requests. The daemon
//! checks who is on the other end of the socket: same user, the tapin binary
//! itself, and not started (directly or not) by an agent.

mod common;
use common::*;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;
use tapin::peer::{self, Peer, Trusted};

/// The installed tapin, as the daemon saw it at startup (path + file identity).
fn tapin_exe() -> Trusted {
    Trusted {
        path: PathBuf::from("/opt/tapin/bin/tapin"),
        id: Some((1, 42)),
    }
}

fn peer(exe: &str, ancestors: &[(u32, &str)]) -> Peer {
    Peer {
        uid: 501,
        exe: Some(PathBuf::from(exe)),
        exe_id: Some((1, 42)),
        // Ancestors are known by process name, which anyone may read.
        ancestors: ancestors
            .iter()
            .map(|(pid, exe)| {
                let name = std::path::Path::new(exe).file_name().unwrap();
                (*pid, Some(name.to_string_lossy().into_owned()))
            })
            .collect(),
        complete: true,
    }
}

#[test]
fn the_tapin_binary_run_by_the_user_may_answer() {
    let p = peer(
        "/opt/tapin/bin/tapin",
        &[
            (900, "/bin/zsh"),
            (800, "/Applications/iTerm.app/Contents/MacOS/iTerm2"),
        ],
    );
    assert!(peer::verify(&p, 501, &[tapin_exe()], &[]).is_ok());
}

#[test]
fn another_program_may_not() {
    for exe in ["/usr/bin/nc", "/usr/bin/python3", "/tmp/tapin"] {
        let p = peer(exe, &[(900, "/bin/zsh")]);
        assert!(peer::verify(&p, 501, &[tapin_exe()], &[]).is_err(), "{exe}");
    }
}

#[test]
fn the_same_file_still_counts_after_linux_marks_it_deleted() {
    // Linux shows "(deleted)" once the running binary's path was replaced.
    let p = peer("/opt/tapin/bin/tapin (deleted)", &[]);
    assert!(peer::verify(&p, 501, &[tapin_exe()], &[]).is_ok());
}

#[test]
fn another_file_at_the_trusted_path_does_not_count() {
    // E.g. an agent overwrote ~/.cargo/bin/tapin with its own program.
    let mut p = peer("/opt/tapin/bin/tapin", &[]);
    p.exe_id = Some((1, 99));
    assert!(peer::verify(&p, 501, &[tapin_exe()], &[]).is_err());
    p.exe_id = None;
    assert!(peer::verify(&p, 501, &[tapin_exe()], &[]).is_err());
}

#[test]
fn an_ancestor_whose_name_cannot_be_read_is_refused() {
    let mut p = peer("/opt/tapin/bin/tapin", &[(900, "/bin/zsh")]);
    p.ancestors.push((800, None));
    assert!(peer::verify(&p, 501, &[tapin_exe()], &[]).is_err());
}

#[test]
fn a_process_tree_too_deep_to_check_is_refused() {
    // Nesting shells past the walk's limit must not hide the agent above.
    let mut p = peer("/opt/tapin/bin/tapin", &[(900, "/bin/zsh")]);
    p.complete = false;
    assert!(peer::verify(&p, 501, &[tapin_exe()], &[]).is_err());
}

#[test]
fn tapin_started_under_an_agent_may_not_answer() {
    // By a known agent pid…
    let p = peer(
        "/opt/tapin/bin/tapin",
        &[(700, "/bin/bash"), (650, "/usr/local/bin/node")],
    );
    assert!(peer::verify(&p, 501, &[tapin_exe()], &[650]).is_err());
    // …or by any process that is an agent binary.
    // The kernel's name for Claude Code's native binary is `claude.exe` (seen
    // on macOS with Claude Code 2.1.296).
    for agent in [
        "/Users/u/.local/bin/claude",
        "/opt/homebrew/bin/codex",
        "claude.exe",
    ] {
        let p = peer("/opt/tapin/bin/tapin", &[(700, "/bin/bash"), (600, agent)]);
        assert!(
            peer::verify(&p, 501, &[tapin_exe()], &[]).is_err(),
            "{agent}"
        );
    }
}

#[test]
fn another_user_may_not() {
    let mut p = peer("/opt/tapin/bin/tapin", &[]);
    p.uid = 502;
    assert!(peer::verify(&p, 501, &[tapin_exe()], &[]).is_err());
}

#[test]
fn the_peer_of_a_socket_is_found_from_the_os() {
    let (a, _b) = UnixStream::pair().unwrap();
    let me = peer::of(&a).expect("peer info");
    assert!(me.complete, "the walk reaches the top of the tree");
    let own = std::fs::metadata(std::env::current_exe().unwrap()).unwrap();
    use std::os::unix::fs::MetadataExt;
    assert_eq!(me.exe_id, Some((own.dev(), own.ino())));
    assert_eq!(me.uid, unsafe { libc::geteuid() });
    let exe = me.exe.expect("exe");
    assert_eq!(
        exe.canonicalize().unwrap(),
        std::env::current_exe().unwrap().canonicalize().unwrap()
    );
    assert_eq!(
        me.ancestors.first().map(|(p, _)| *p),
        Some(std::os::unix::process::parent_id())
    );
    // Every ancestor up to pid 1 has a readable name, including root's
    // `login`/`sshd` above a terminal or SSH shell.
    assert!(
        me.ancestors.iter().all(|(_, n)| n.is_some()),
        "{:?}",
        me.ancestors
    );
    assert_eq!(me.ancestors.last().map(|(p, _)| *p), Some(1));
}

fn permission_payload() -> String {
    fixture("permission-turn", "04-PermissionRequest.json")
}

#[test]
fn a_raw_socket_client_cannot_approve() {
    let d = start_untrusting_daemon();
    let mut client = Client::connect(&d.socket);
    let mut hook = spawn_hook(&d.socket, &permission_payload(), &[]);
    let rows = client.wait_for(|r| pending_request(r).is_some());
    client.decide(pending_request(&rows).unwrap(), true);
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        hook.try_wait().unwrap().is_none(),
        "the hook must still be waiting"
    );
    let _ = hook.kill();
    let _ = hook.wait();
}

#[test]
fn a_decide_without_subscribing_is_ignored_even_from_a_trusted_binary() {
    use std::io::Write;
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    let mut hook = spawn_hook(&d.socket, &permission_payload(), &[]);
    let rows = tui.wait_for(|r| pending_request(r).is_some());
    let id = pending_request(&rows).unwrap();
    let mut other = UnixStream::connect(&d.socket).unwrap();
    writeln!(
        other,
        r#"{{"type":"decide","request_id":{id},"allow":true}}"#
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        hook.try_wait().unwrap().is_none(),
        "the hook must still be waiting"
    );
    let _ = hook.kill();
    let _ = hook.wait();
}

#[test]
fn ppid_parsing_survives_odd_command_names() {
    assert_eq!(peer::parse_ppid("123 (zsh) S 45 123 123 0"), Some(45));
    assert_eq!(peer::parse_ppid("9 (a) b) c) R 77 9 9"), Some(77));
    assert_eq!(peer::parse_ppid("garbage"), None);
}

fn first_message(socket: &std::path::Path) -> serde_json::Value {
    use std::io::{BufRead, BufReader, Write};
    let mut s = UnixStream::connect(socket).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    s.write_all(b"{\"type\":\"subscribe\"}\n").unwrap();
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

#[test]
fn an_inbox_is_told_whether_it_may_answer() {
    let trusted = start_daemon();
    assert_eq!(first_message(&trusted.socket)["can_answer"], true);
    let strict = start_untrusting_daemon();
    let msg = first_message(&strict.socket);
    assert_eq!(
        (msg["type"].as_str(), msg["can_answer"].as_bool()),
        (Some("access"), Some(false))
    );
}

#[test]
fn a_hook_ignores_answers_from_a_fake_daemon() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    // Something that is not tapin listens on the socket and approves everything.
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    std::fs::create_dir(&run).unwrap();
    std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = run.join("tapin.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let hook = spawn_hook(&socket, &permission_payload(), &[]);
    if let Ok((conn, _)) = listener.accept() {
        let mut line = String::new();
        let _ = BufReader::new(&conn).read_line(&mut line);
        let _ = (&conn).write_all(b"{\"type\":\"decision\",\"decision\":\"allow\"}\n");
    }
    let out = finish(hook, Duration::from_secs(5));
    assert!(out.status.success());
    assert!(
        out.stdout.is_empty(),
        "a fake daemon's allow must not reach the agent"
    );
}

#[test]
fn a_client_cannot_mark_an_unrelated_process_as_an_agent() {
    use std::io::Write;
    // An injected agent claims pid 1 is an agent, which would refuse every inbox.
    let d = start_daemon();
    let mut payload: serde_json::Value =
        serde_json::from_str(&fixture("basic-turn", "01-SessionStart.json")).unwrap();
    payload["session_id"] = "poison".into();
    let msg = serde_json::json!({
        "type": "event", "agent": "claude", "payload": payload,
        "location": {"agent_pid": 1}
    });
    let mut s = UnixStream::connect(&d.socket).unwrap();
    writeln!(s, "{msg}").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(first_message(&d.socket)["can_answer"], true);
}

#[test]
fn a_hook_that_cannot_trust_the_daemon_still_reports_the_request() {
    // E.g. right after an upgrade: the row shows (answer in the agent's own
    // dialog) instead of silently missing.
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    let hook = spawn_hook(
        &d.socket,
        &permission_payload(),
        &[("TAPIN_TEST_DISTRUST_DAEMON", "1")],
    );
    let out = finish(hook, Duration::from_secs(3));
    assert!(out.stdout.is_empty());
    tui.wait_for(|r| has_state(r, "NeedsPermission"));
}

/// True if this test process itself runs under an agent (e.g. a developer's
/// Claude Code): then tests with the real ancestry check can't expect an
/// inbox to be trusted, and only run where nothing is above them (CI).
fn under_an_agent() -> bool {
    let (a, _b) = UnixStream::pair().unwrap();
    let me = peer::of(&a).unwrap();
    let mut p = me.clone();
    p.exe_id = Some((1, 42));
    p.exe = Some(PathBuf::from("/opt/tapin/bin/tapin"));
    peer::verify(&p, me.uid, &[tapin_exe()], &[]).is_err()
}

#[test]
fn with_the_real_ancestry_check_a_poisoned_pid_still_refuses_nobody() {
    use std::io::Write;
    if under_an_agent() {
        eprintln!("skipped: the test itself runs under an agent");
        return;
    }
    let d = start_daemon_with_real_ancestry();
    assert_eq!(first_message(&d.socket)["can_answer"], true);
    let mut payload: serde_json::Value =
        serde_json::from_str(&fixture("basic-turn", "01-SessionStart.json")).unwrap();
    payload["session_id"] = "poison".into();
    let msg = serde_json::json!({
        "type": "event", "agent": "claude", "payload": payload,
        "location": {"agent_pid": 1}
    });
    let mut s = UnixStream::connect(&d.socket).unwrap();
    writeln!(s, "{msg}").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(first_message(&d.socket)["can_answer"], true);
}

#[test]
fn a_hook_waits_only_for_a_daemon_that_speaks_the_current_protocol() {
    // An older daemon (v0.1) never answers the hello: the hook must not wait
    // for it, even if its binary looks right (macOS checks by path).
    let d = start_daemon_with(&[], &[]);
    let old = start_daemon_with(&[("TAPIN_TEST_NO_HELLO", std::path::Path::new("1"))], &[]);
    let started = std::time::Instant::now();
    let out = finish(
        spawn_hook(&old.socket, &permission_payload(), &[]),
        Duration::from_secs(5),
    );
    assert!(out.stdout.is_empty());
    assert!(started.elapsed() < Duration::from_secs(2), "it didn't wait");
    // The current daemon still holds the request for an answer.
    let mut hook = spawn_hook(&d.socket, &permission_payload(), &[]);
    std::thread::sleep(Duration::from_millis(600));
    assert!(
        hook.try_wait().unwrap().is_none(),
        "waits on the current daemon"
    );
    let _ = hook.kill();
    let _ = hook.wait();
}
