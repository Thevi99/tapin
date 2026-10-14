//! Passive mode for Codex CLI: interactive `codex` processes (with a tty)
//! matched by cwd to their newest rollout file, whose last turn event gives
//! the state. Rollouts are undocumented, so only a few stable lines are read.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tapin::inbox::State;
use tapin::passive::codex::{observe, parse_ps, Proc};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/passive/codex-sessions")
            .join(name),
    )
    .unwrap()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// A CODEX_HOME/sessions tree holding `files` (name → contents), all fresh.
fn sessions(files: &[(&str, String)]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let day = dir.path().join("2026/10/14");
    std::fs::create_dir_all(&day).unwrap();
    for (name, body) in files {
        std::fs::write(day.join(name), body).unwrap();
    }
    let root = dir.path().to_path_buf();
    (dir, root)
}

fn proc(cwd: &str, started_secs_ago: u64) -> Proc {
    Proc {
        pid: 4242,
        tty: "/dev/ttys003".into(),
        started: now() - started_secs_ago,
        cwd: cwd.into(),
    }
}

#[test]
fn a_finished_turn_is_done_with_its_last_message() {
    let (_d, root) = sessions(&[("rollout-a.jsonl", fixture("done.jsonl"))]);
    let obs = observe(&[proc("/home/dev/projects/demo", 600)], &root);
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0].state, State::Done);
    assert!(obs[0].detail.contains("codex-ok.txt"), "{}", obs[0].detail);
    assert_eq!(obs[0].pid, Some(4242));
    assert!(!obs[0].session_id.is_empty());
}

#[test]
fn a_running_turn_is_working() {
    let (_d, root) = sessions(&[("rollout-a.jsonl", fixture("working.jsonl"))]);
    assert_eq!(
        observe(&[proc("/home/dev/projects/demo", 600)], &root)[0].state,
        State::Working
    );
}

#[test]
fn a_new_session_without_a_rollout_yet_is_idle() {
    let (_d, root) = sessions(&[]);
    let obs = observe(&[proc("/home/dev/projects/demo", 5)], &root);
    assert_eq!(obs[0].state, State::Idle);
}

#[test]
fn rollouts_from_other_folders_or_before_the_process_started_are_ignored() {
    let (_d, root) = sessions(&[("rollout-a.jsonl", fixture("done.jsonl"))]);
    let other = observe(&[proc("/home/dev/projects/other", 600)], &root);
    assert_eq!(
        other[0].state,
        State::Idle,
        "different cwd: not this session's rollout"
    );
    // A process started after the rollout was last written can't own it.
    let file = root.join("2026/10/14/rollout-a.jsonl");
    let old = SystemTime::now() - Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let later = observe(&[proc("/home/dev/projects/demo", 60)], &root);
    assert_eq!(later[0].state, State::Idle);
}

#[test]
fn only_interactive_codex_processes_count() {
    let ps = "\
  101 ttys003     01:02:03 /opt/homebrew/bin/codex codex
  102 ??          10:00    /Applications/ChatGPT.app/Contents/Resources/codex codex app-server
  103 ttys004        05:12 /usr/local/bin/codex codex app-server --listen stdio
  104 ttys005     1-02:00:00 /usr/local/bin/claude claude
  105 ttys006        00:09 /Users/dev/.local/bin/codex codex resume --last
";
    let found = parse_ps(ps, 100_000);
    let pids: Vec<u32> = found.iter().map(|p| p.pid).collect();
    assert_eq!(pids, vec![101, 105]);
    assert_eq!(found[0].tty, "/dev/ttys003");
    assert_eq!(found[0].started, 100_000 - 3723);
    assert_eq!(found[1].started, 100_000 - 9);
}

#[test]
fn two_codex_sessions_in_one_folder_stay_two_sessions() {
    let first = fixture("done.jsonl");
    let second = first.replace("01a136b9", "02b247ca");
    let (_d, root) = sessions(&[("rollout-a.jsonl", first), ("rollout-b.jsonl", second)]);
    let mut p1 = proc("/home/dev/projects/demo", 600);
    let mut p2 = proc("/home/dev/projects/demo", 300);
    p1.pid = 1;
    p2.pid = 2;
    let obs = observe(&[p1, p2], &root);
    let ids: std::collections::HashSet<_> = obs.iter().map(|o| o.session_id.clone()).collect();
    assert_eq!(ids.len(), 2, "{obs:?}");
}

#[test]
fn flags_before_the_subcommand_do_not_hide_it() {
    let ps = "\
  201 ttys003  00:10 /usr/local/bin/codex codex -c model=o3 exec do the thing
  202 ttys004  00:10 /usr/local/bin/codex codex --model o3 -a on-request
  203 ttys005  00:10 /usr/local/bin/codex codex -m o3 resume --last
";
    let pids: Vec<u32> = parse_ps(ps, 1000).iter().map(|p| p.pid).collect();
    assert_eq!(pids, vec![202, 203]);
}

#[test]
fn passive_mode_writes_nothing() {
    use std::os::unix::fs::PermissionsExt;
    // Everything passive mode reads: a fake `claude agents --json`, a fake `ps`
    // listing this test process as an interactive codex, and a Codex home.
    let world = tempfile::tempdir().unwrap();
    let home = world.path().join("home");
    let codex_home = home.join(".codex");
    std::fs::create_dir_all(codex_home.join("sessions/2026/10/14")).unwrap();
    let rollout = fixture("done.jsonl").replace(
        "/home/dev/projects/demo",
        std::env::current_dir().unwrap().to_str().unwrap(),
    );
    std::fs::write(
        codex_home.join("sessions/2026/10/14/rollout-a.jsonl"),
        rollout,
    )
    .unwrap();
    let bin = world.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let agents =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/passive/claude-agents.json");
    std::fs::write(
        bin.join("claude"),
        format!("#!/bin/sh\ncat '{}'\n", agents.display()),
    )
    .unwrap();
    std::fs::write(
        bin.join("ps"),
        format!(
            "#!/bin/sh\necho '{} ttys999 00:10 /usr/local/bin/codex codex'\n",
            std::process::id()
        ),
    )
    .unwrap();
    for f in ["claude", "ps"] {
        std::fs::set_permissions(bin.join(f), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn snapshot(root: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
        let mut out = Vec::new();
        let mut dirs = vec![root.to_path_buf()];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let m = e.metadata().unwrap();
                if m.is_dir() {
                    dirs.push(e.path());
                }
                out.push((e.path(), m.len(), m.modified().unwrap()));
            }
        }
        out.sort();
        out
    }
    let before = snapshot(&home);

    let runtime = tempfile::tempdir().unwrap();
    let socket = runtime.path().join("run/tapin.sock");
    let mut daemon = std::process::Command::new(env!("CARGO_BIN_EXE_tapin"))
        .args(["daemon", "--no-notify", "--socket"])
        .arg(&socket)
        .env("HOME", &home)
        .env("CODEX_HOME", &codex_home)
        .env("TAPIN_CLAUDE_CMD", bin.join("claude"))
        .env("TAPIN_PS_CMD", bin.join("ps"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(5500)); // a few polls
    let _ = daemon.kill();
    let _ = daemon.wait();

    assert_eq!(snapshot(&home), before, "passive mode must only read");
}
