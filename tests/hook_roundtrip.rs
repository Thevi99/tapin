//! Seam B: the real `tapin` binary as hook and daemon, talking over a temp socket.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn fixture(name: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/claude/basic-turn")
        .join(name);
    std::fs::read_to_string(p).unwrap()
}

/// Runs `tapin hook --agent claude` the way Claude Code would: payload on stdin.
fn run_hook(socket: &Path, payload: &str) -> (std::process::Output, Duration) {
    let start = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tapin"))
        .args(["hook", "--agent", "claude", "--socket"])
        .arg(socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (out, start.elapsed())
}

#[test]
fn hook_without_daemon_exits_cleanly_and_fast() {
    let dir = tempfile::tempdir().unwrap();
    let (out, took) = run_hook(
        &dir.path().join("none.sock"),
        &fixture("03-PreToolUse.json"),
    );
    assert!(out.status.success());
    assert!(
        out.stdout.is_empty(),
        "empty stdout lets Claude continue normally"
    );
    assert!(took < Duration::from_millis(500), "took {took:?}");
}

struct Daemon {
    child: Child,
    socket: PathBuf,
    _dir: tempfile::TempDir,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_daemon() -> Daemon {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("run").join("tapin.sock");
    let child = Command::new(env!("CARGO_BIN_EXE_tapin"))
        .args(["daemon", "--socket"])
        .arg(&socket)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while UnixStream::connect(&socket).is_err() {
        assert!(Instant::now() < deadline, "daemon never listened");
        std::thread::sleep(Duration::from_millis(20));
    }
    Daemon {
        child,
        socket,
        _dir: dir,
    }
}

/// Subscribes like a TUI and returns the first snapshot whose rows satisfy `pred`.
fn wait_for_rows(socket: &Path, pred: impl Fn(&serde_json::Value) -> bool) -> serde_json::Value {
    let mut s = UnixStream::connect(socket).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    s.write_all(b"{\"type\":\"subscribe\"}\n").unwrap();
    let mut lines = BufReader::new(s).lines();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert!(Instant::now() < deadline, "no matching snapshot within 10s");
        let line = lines
            .next()
            .expect("daemon closed")
            .expect("no matching snapshot in time");
        let msg: serde_json::Value = serde_json::from_str(&line).unwrap();
        if pred(&msg["rows"]) {
            return msg["rows"].clone();
        }
    }
}

#[test]
fn hook_event_reaches_subscribers_through_the_daemon() {
    let d = start_daemon();
    let (out, _) = run_hook(&d.socket, &fixture("05-PreToolUse.json"));
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    let rows = wait_for_rows(&d.socket, |r| r.as_array().is_some_and(|a| !a.is_empty()));
    assert_eq!(rows[0]["state"], "Working");
    assert_eq!(rows[0]["project"], "demo");
    assert_eq!(rows[0]["detail"], "Bash(ls -la)");
}

#[test]
fn socket_is_private_to_the_user() {
    use std::os::unix::fs::PermissionsExt;
    let d = start_daemon();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&d.socket), 0o600);
    assert_eq!(mode(d.socket.parent().unwrap()), 0o700);
}

fn hook_cmd(args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_tapin"));
    c.arg("hook")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c
}

#[test]
fn hook_with_unknown_arguments_never_blocks_the_agent() {
    // Exit code 2 would make Claude Code block the tool call.
    let mut child = hook_cmd(&["--agent", "claude", "--event", "PreToolUse", "--bogus"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
}

#[test]
fn hook_gives_up_quickly_when_stdin_never_closes() {
    let dir = tempfile::tempdir().unwrap();
    let start = Instant::now();
    let mut child = hook_cmd(&["--agent", "claude", "--socket"])
        .arg(dir.path().join("x.sock"))
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let _held_open = child.stdin.take();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if start.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            panic!("hook hung with open stdin");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success());
}

#[test]
fn hook_refuses_a_socket_dir_other_users_can_write() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    std::fs::create_dir(&run).unwrap();
    std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o777)).unwrap();
    let socket = run.join("tapin.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let (out, _) = run_hook(&socket, &fixture("03-PreToolUse.json"));
    assert!(out.status.success());
    assert!(
        listener.accept().is_err(),
        "hook connected to an untrusted socket"
    );
}

#[test]
fn daemon_refuses_a_socket_dir_other_users_can_write() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    std::fs::create_dir(&run).unwrap();
    std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o777)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tapin"))
        .args(["daemon", "--socket"])
        .arg(run.join("tapin.sock"))
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("daemon started on an unsafe dir");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(!status.success());
    let mode = std::fs::metadata(&run).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o777, "daemon must not chmod a dir it didn't create");
}

#[test]
fn a_stalled_subscriber_does_not_delay_others() {
    let d = start_daemon();
    // Subscriber that never reads.
    let mut stalled = UnixStream::connect(&d.socket).unwrap();
    stalled.write_all(b"{\"type\":\"subscribe\"}\n").unwrap();
    // Many sessions make each snapshot large enough to fill its socket buffer.
    let template: serde_json::Value = serde_json::from_str(&fixture("05-PreToolUse.json")).unwrap();
    let start = Instant::now();
    for i in 0..300 {
        let mut ev = template.clone();
        ev["session_id"] = format!("s{i}").into();
        let mut line =
            serde_json::json!({"type":"event","agent":"claude","payload":ev}).to_string();
        line.push('\n');
        // Bursting 300 connects can overflow the listen backlog; retry like a slow client.
        let mut s = loop {
            match UnixStream::connect(&d.socket) {
                Ok(s) => break s,
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        };
        s.set_write_timeout(Some(Duration::from_secs(1))).unwrap();
        s.write_all(line.as_bytes()).unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "daemon stalled after {i} events"
        );
    }
    let rows = wait_for_rows(&d.socket, |r| r.as_array().is_some_and(|a| a.len() == 300));
    assert_eq!(rows.as_array().unwrap().len(), 300);
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "took {:?}",
        start.elapsed()
    );
}

#[test]
fn too_long_socket_path_is_reported_clearly() {
    let dir = tempfile::tempdir().unwrap();
    let long = dir.path().join("a".repeat(120)).join("tapin.sock");
    for args in [vec!["daemon"], vec![]] {
        let out = Command::new(env!("CARGO_BIN_EXE_tapin"))
            .args(&args)
            .arg("--socket")
            .arg(&long)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(!out.status.success(), "{args:?} should fail");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("too long"), "{args:?}: {err}");
    }
}
