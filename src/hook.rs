//! `tapin hook`: the short-lived process an agent runs on every hook event.
//!
//! Contract: never block or fail the agent. With no daemon, exit 0 at once
//! with empty stdout, which every supported agent treats as "carry on".
//! Permission requests wait for an answer from tapin. Claude shows its own
//! dialog meanwhile; Codex shows its prompt only after the hook returns
//! (openai/codex#39835), so Codex waits are short and only while an inbox is open.

use crate::adapters::hooks;
use crate::event::AgentKind;
use crate::socket::{Reply, Request};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How long a hook may take when the agent isn't waiting on an answer.
const BUDGET: Duration = Duration::from_millis(1000);

/// Ceiling for the wait: Claude Code kills command hooks after 600s by default.
const MAX_APPROVAL_TIMEOUT: Duration = Duration::from_secs(590);

/// Milliseconds after start at which the watchdog exits the process with 0.
static DEADLINE_MS: AtomicU64 = AtomicU64::new(0);

pub fn run(agent: AgentKind, socket: &Path) {
    // Never fail the agent: a panic would exit 101 with a message on stderr,
    // which Gemini CLI treats as "deny" and shows to the user. Exit 0, silently.
    std::panic::set_hook(Box::new(|_| std::process::exit(0)));
    let start = Instant::now();
    set_deadline(BUDGET);
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(20));
        if start.elapsed().as_millis() as u64 >= DEADLINE_MS.load(Ordering::Relaxed) {
            std::process::exit(0);
        }
    });

    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return;
    }
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(&input) else {
        return;
    };
    let waits = hooks::wants_decision(&payload);
    if !socket.parent().is_some_and(crate::socket::is_private_dir) {
        return;
    }
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return;
    };
    // An answer may only come from tapin's own daemon: anything else on the
    // socket (say, an agent's fake daemon that approves everything) is never
    // waited for. The event is still sent, so the request shows in the inbox
    // (e.g. right after an upgrade, until the daemon restarts).
    let waits = waits && daemon_is_genuine(&stream);
    let _ = stream.set_write_timeout(Some(BUDGET));
    // The process walk (tty, agent pid) is only worth it when a session (re)appears.
    let walk = matches!(
        payload.get("hook_event_name").and_then(|e| e.as_str()),
        Some("SessionStart" | "UserPromptSubmit")
    );
    let location = Some(Box::new(crate::location::Location::from_env(walk)));
    let Ok(mut line) = serde_json::to_string(&Request::Event {
        agent,
        payload: payload.clone(),
        location,
    }) else {
        return;
    };
    line.push('\n');
    if stream.write_all(line.as_bytes()).is_err() || !waits {
        return;
    }

    let timeout = approval_timeout(agent);
    set_deadline(start.elapsed() + timeout + Duration::from_millis(500));
    let _ = stream.set_read_timeout(Some(timeout));
    let mut reply = String::new();
    if BufReader::new(stream).read_line(&mut reply).is_err() {
        return; // timed out: Claude's own dialog stays in charge
    }
    let Ok(Reply::Decision { decision }) = serde_json::from_str(&reply) else {
        return; // daemon went away
    };
    let output = hooks::decision_output(decision, &payload);
    if let Some(output) = output {
        print!("{output}");
        let _ = std::io::stdout().flush();
    }
}

fn daemon_is_genuine(stream: &UnixStream) -> bool {
    #[cfg(debug_assertions)]
    if std::env::var_os("TAPIN_TEST_DISTRUST_DAEMON").is_some() {
        return false;
    }
    crate::peer::is_genuine_daemon(stream) && speaks_current_protocol(stream)
}

/// True if the daemon answers `Hello` with this build's protocol. A daemon
/// from before it (v0.1, which took answers from anyone) stays silent, and
/// on macOS it can look like this very binary after an upgrade.
fn speaks_current_protocol(stream: &UnixStream) -> bool {
    let Ok(mut line) = serde_json::to_string(&Request::Hello) else {
        return false;
    };
    line.push('\n');
    if (&*stream).write_all(line.as_bytes()).is_err() {
        return false;
    }
    let _ = stream.set_read_timeout(Some(HELLO_TIMEOUT));
    // Byte by byte, so nothing after the hello line is consumed here.
    let mut reply = Vec::new();
    let mut byte = [0u8; 1];
    while reply.len() < 256 {
        match (&*stream).read(&mut byte) {
            Ok(1) if byte[0] == b'\n' => break,
            Ok(1) => reply.push(byte[0]),
            _ => return false,
        }
    }
    matches!(
        serde_json::from_slice(&reply),
        Ok(Reply::Hello { protocol }) if protocol >= crate::socket::MIN_HOOK_PROTOCOL
    )
}

/// How long a hook waits for the daemon's hello before giving up on it.
const HELLO_TIMEOUT: Duration = Duration::from_millis(300);

fn set_deadline(from_start: Duration) {
    DEADLINE_MS.store(from_start.as_millis() as u64, Ordering::Relaxed);
}

fn approval_timeout(agent: AgentKind) -> Duration {
    let from_config = || {
        let c = crate::config::Config::load();
        if agent.prompt_waits_for_hook() {
            c.codex_approval_timeout_secs
        } else {
            c.approval_timeout_secs
        }
    };
    let var = if agent.prompt_waits_for_hook() {
        "TAPIN_CODEX_APPROVAL_TIMEOUT_SECS"
    } else {
        "TAPIN_APPROVAL_TIMEOUT_SECS"
    };
    std::env::var(var)
        .ok()
        .and_then(|s| s.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(from_config()))
        .min(MAX_APPROVAL_TIMEOUT)
}
