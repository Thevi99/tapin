//! Where an agent session runs, so the TUI can jump to it.
//!
//! Hooks have no tty of their own (Claude Code docs), but they inherit the
//! agent's environment, which names the tmux pane or terminal pane. The tty is
//! found by walking up the parent processes.

use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    /// tmux server socket path (the part of `$TMUX` before the first comma).
    pub tmux_socket: Option<String>,
    /// tmux pane id, e.g. `%7` (`$TMUX_PANE`).
    pub tmux_pane: Option<String>,
    /// tmux server pid (second field of `$TMUX`). Pane ids restart at %0 when
    /// a server restarts on the same socket, so a jump checks this first.
    pub tmux_pid: Option<String>,
    /// Terminal app, from `$TERM_PROGRAM` (e.g. `iTerm.app`, `Apple_Terminal`, `WezTerm`).
    pub term_program: Option<String>,
    /// WezTerm pane id (`$WEZTERM_PANE`).
    pub wezterm_pane: Option<String>,
    /// The agent's controlling terminal, e.g. `/dev/ttys012`.
    pub tty: Option<String>,
    /// The agent process and its start time (epoch seconds), to notice when
    /// it dies or its pid is reused.
    pub agent_pid: Option<u32>,
    pub agent_start: Option<u64>,
}

impl Location {
    /// What the current process (a hook) can tell about where its agent runs.
    /// `walk` looks at parent processes (tty, agent pid), a few `ps` calls.
    pub fn from_env(walk: bool) -> Location {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let tmux_pane = var("TMUX_PANE");
        let found = if walk {
            walk_ancestors()
        } else {
            Ancestry::default()
        };
        Location {
            agent_pid: found.agent_pid,
            agent_start: found.agent_start,
            tmux_socket: var("TMUX").and_then(|t| t.split(',').next().map(String::from)),
            tmux_pid: var("TMUX").and_then(|t| t.split(',').nth(1).map(String::from)),
            term_program: var("TERM_PROGRAM"),
            wezterm_pane: var("WEZTERM_PANE"),
            // Inside tmux the pane is the better key than the tty.
            tty: found.tty.filter(|_| tmux_pane.is_none()),
            tmux_pane,
        }
    }

    /// Keeps what is known, taking anything newer that is set.
    pub fn merge(&mut self, newer: &Location) {
        let take = |old: &mut Option<String>, new: &Option<String>| {
            if new.is_some() {
                old.clone_from(new);
            }
        };
        take(&mut self.tmux_socket, &newer.tmux_socket);
        take(&mut self.tmux_pane, &newer.tmux_pane);
        take(&mut self.tmux_pid, &newer.tmux_pid);
        take(&mut self.term_program, &newer.term_program);
        take(&mut self.wezterm_pane, &newer.wezterm_pane);
        take(&mut self.tty, &newer.tty);
        if newer.agent_pid.is_some() {
            self.agent_pid = newer.agent_pid;
            self.agent_start = newer.agent_start;
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Location::default()
    }
}

/// What walking up the parent processes found.
#[derive(Debug, Default)]
struct Ancestry {
    tty: Option<String>,
    agent_pid: Option<u32>,
    agent_start: Option<u64>,
}

/// Agent executables tapin knows (matched on the basename of `ps -o comm=`,
/// which is the path the process was started with).
const AGENT_NAMES: &[&str] = &["claude", "codex"];

/// Walks up from the hook: the first ancestor with a terminal gives the tty
/// (`ps` prints `??`/`?` for none), the first one named like an agent gives
/// the agent pid and its start time (pids get reused; the pair doesn't).
fn walk_ancestors() -> Ancestry {
    let mut found = Ancestry::default();
    let mut pid = std::os::unix::process::parent_id();
    for _ in 0..8 {
        let Some(out) = ps(&["-o", "tty=,ppid=,comm=", "-p", &pid.to_string()]) else {
            break;
        };
        // "<tty> <ppid> <comm>": comm is a path and may contain spaces.
        let Some((tty, rest)) = out.trim().split_once(char::is_whitespace) else {
            break;
        };
        let Some((ppid, comm)) = rest.trim_start().split_once(char::is_whitespace) else {
            break;
        };
        if found.tty.is_none() && tty != "??" && tty != "?" {
            found.tty = Some(if tty.starts_with("/dev/") {
                tty.to_string()
            } else {
                format!("/dev/{tty}")
            });
        }
        let name = std::path::Path::new(comm.trim())
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if found.agent_pid.is_none() && AGENT_NAMES.contains(&name) {
            found.agent_pid = Some(pid);
            found.agent_start = process_start(pid);
        }
        if found.tty.is_some() && found.agent_pid.is_some() {
            break;
        }
        match ppid.parse::<u32>() {
            Ok(p) if p > 1 => pid = p,
            _ => break,
        }
    }
    found
}

/// True if `pid` is a live process named like an agent, by the same rule the
/// hook's walk uses (basename of `ps -o comm=`), so both sides agree.
pub fn is_agent_named(pid: u32) -> bool {
    if pid <= 1 {
        return false;
    }
    let Some(out) = ps(&["-o", "comm=", "-p", &pid.to_string()]) else {
        return false;
    };
    let name = std::path::Path::new(out.trim())
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    AGENT_NAMES.contains(&name)
}

/// When a process started, in epoch seconds, from `ps -o etime=` (elapsed
/// time is locale- and timezone-free, unlike `lstart`). None if it's gone.
pub fn process_start(pid: u32) -> Option<u64> {
    let elapsed = parse_etime(ps(&["-o", "etime=", "-p", &pid.to_string()])?.trim())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(now.saturating_sub(elapsed))
}

/// True only when the process is certainly gone: `kill(pid, 0)` says no such
/// process, or the pid now belongs to a process that started at another time.
/// Anything uncertain (EPERM, `ps` failing) counts as alive.
pub fn is_gone(pid: u32, recorded_start: Option<u64>) -> bool {
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 only checks that the process exists; nothing is sent.
    if unsafe { libc::kill(raw, 0) } != 0 {
        return std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
    }
    match (recorded_start, process_start(pid)) {
        (Some(then), Some(now)) => then.abs_diff(now) > START_TOLERANCE_SECS,
        _ => false,
    }
}

/// `etime` has one-second resolution and is read at slightly different times.
const START_TOLERANCE_SECS: u64 = 5;

/// `ps` elapsed time `[[dd-]hh:]mm:ss` in seconds.
pub(crate) fn parse_etime(s: &str) -> Option<u64> {
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, s),
    };
    let parts: Vec<u64> = rest
        .split(':')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let secs = match parts.as_slice() {
        [m, s] => m * 60 + s,
        [h, m, s] => h * 3600 + m * 60 + s,
        _ => return None,
    };
    Some(days * 86_400 + secs)
}

fn ps(args: &[&str]) -> Option<String> {
    let out = Command::new("ps").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}
