//! Passive mode: find sessions without hooks, read-only.
//!
//! Each source turns what it can see into `Observation`s; `Inbox::observe`
//! merges them, and hook events always win over observations.

pub mod claude;
pub mod codex;

use crate::inbox::State;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// One session as seen from outside the agent.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub session_id: String,
    pub cwd: String,
    pub state: State,
    pub detail: String,
    /// The agent process, when the source knows it.
    pub pid: Option<u32>,
    /// The agent's own name for the session (e.g. Claude's `ray-37`), shown
    /// in place of the folder name so sessions in one folder can be told apart.
    pub label: Option<String>,
    /// The agent's terminal, so Enter can jump to it.
    pub tty: Option<String>,
    /// The agent's transcript (Claude) or rollout (Codex) file.
    pub transcript: Option<std::path::PathBuf>,
}

/// The controlling tty of each pid (`ps` prints `??`/`?` for none).
pub(crate) fn ttys_of(pids: &[u32]) -> Vec<(u32, String)> {
    if pids.is_empty() {
        return vec![];
    }
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let Some(out) = run_with_timeout(
        "ps",
        &["-o", "pid=,tty=", "-p", &list],
        Duration::from_secs(2),
    ) else {
        return vec![];
    };
    out.lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let pid = f.next()?.parse().ok()?;
            let tty = f.next()?;
            (tty != "??" && tty != "?").then(|| {
                (
                    pid,
                    if tty.starts_with("/dev/") {
                        tty.to_string()
                    } else {
                        format!("/dev/{tty}")
                    },
                )
            })
        })
        .collect()
}

/// Runs a command and returns its stdout, or None if it is missing, fails,
/// or takes longer than `limit` (it is then killed). Stdout is drained on a
/// thread while waiting: a full pipe would otherwise block the command.
pub(crate) fn run_with_timeout(program: &str, args: &[&str], limit: Duration) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        std::io::Read::read_to_string(&mut stdout, &mut out)
            .ok()
            .map(|_| out)
    });
    let started = Instant::now();
    let ok = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Err(_) => break false,
            Ok(None) if started.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let out = reader.join().ok().flatten();
    if ok {
        out
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::run_with_timeout;
    use std::time::Duration;

    #[test]
    fn large_output_is_read_while_the_command_runs() {
        // 1 MB is far beyond a pipe buffer: waiting for exit before reading deadlocks.
        let out = run_with_timeout(
            "sh",
            &["-c", "head -c 1000000 /dev/zero | tr '\\0' x"],
            Duration::from_secs(5),
        );
        assert_eq!(out.map(|o| o.len()), Some(1_000_000));
    }
}
