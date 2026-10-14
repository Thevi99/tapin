//! Codex CLI without hooks: interactive `codex` processes (with a tty), each
//! matched by cwd to the newest rollout written since it started. Rollouts
//! are undocumented, so only `session_meta` (id, cwd) and the turn events
//! `task_started` / `task_complete` / `turn_aborted` are read.

use super::{run_with_timeout, Observation};
use crate::inbox::State;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// An interactive Codex process.
#[derive(Debug, Clone, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub tty: String,
    /// Start time, seconds since the epoch.
    pub started: u64,
    pub cwd: String,
}

/// Subcommands that are not a user's interactive session.
const NOT_INTERACTIVE: &[&str] = &[
    "app-server",
    "exec",
    "sandbox",
    "mcp-server",
    "mcp",
    "login",
];

/// Parses `ps -axo pid=,tty=,etime=,comm=,args=` into interactive Codex
/// processes (cwd left empty). `now` is the current time in epoch seconds.
pub fn parse_ps(text: &str, now: u64) -> Vec<Proc> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.split_whitespace();
            let pid: u32 = f.next()?.parse().ok()?;
            let tty = f.next()?;
            let etime = crate::location::parse_etime(f.next()?)?;
            let comm = f.next()?;
            let args: Vec<&str> = f.collect();
            let name = Path::new(comm).file_name()?.to_str()?;
            let subcommand = subcommand(args.get(1..).unwrap_or_default());
            if name != "codex" || tty == "??" || tty == "?" || NOT_INTERACTIVE.contains(&subcommand)
            {
                return None;
            }
            Some(Proc {
                pid,
                tty: if tty.starts_with("/dev/") {
                    tty.into()
                } else {
                    format!("/dev/{tty}")
                },
                started: now.saturating_sub(etime),
                cwd: String::new(),
            })
        })
        .collect()
}

/// Codex flags that take a value, so the value isn't mistaken for the subcommand.
const VALUE_FLAGS: &[&str] = &[
    "-c",
    "--config",
    "-m",
    "--model",
    "-p",
    "--profile",
    "-C",
    "--cd",
    "-s",
    "--sandbox",
    "-a",
    "--ask-for-approval",
    "-i",
    "--image",
    "--enable",
    "--disable",
];

/// The first positional argument (the subcommand, if any).
fn subcommand<'a>(args: &[&'a str]) -> &'a str {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if VALUE_FLAGS.contains(a) {
            it.next();
        } else if !a.starts_with('-') {
            return a;
        }
    }
    ""
}

struct Rollout {
    id: String,
    cwd: String,
    modified: u64,
    path: PathBuf,
}

/// What each process's session looks like right now.
pub fn observe(procs: &[Proc], sessions_root: &Path) -> Vec<Observation> {
    let oldest = procs.iter().map(|p| p.started).min().unwrap_or(0);
    let rollouts = rollouts_since(sessions_root, oldest.saturating_sub(2));
    // Each rollout belongs to at most one process. Newest processes choose
    // first, so with two sessions in one folder each keeps its own file.
    let mut order: Vec<&Proc> = procs.iter().collect();
    order.sort_by_key(|p| std::cmp::Reverse(p.started));
    let mut taken: Vec<&Path> = Vec::new();
    let mut chosen: Vec<(u32, Option<&Rollout>)> = Vec::new();
    for p in order {
        let own = rollouts
            .iter()
            .filter(|r| r.cwd == p.cwd && r.modified + 2 >= p.started)
            .filter(|r| !taken.contains(&r.path.as_path()))
            .max_by_key(|r| r.modified);
        if let Some(r) = own {
            taken.push(&r.path);
        }
        chosen.push((p.pid, own));
    }
    procs
        .iter()
        .map(|p| {
            let own = chosen
                .iter()
                .find(|(pid, _)| *pid == p.pid)
                .and_then(|(_, r)| *r);
            let (session_id, state, detail) = match own {
                Some(r) => {
                    let (state, detail) = last_turn(&r.path);
                    (r.id.clone(), state, detail)
                }
                // Started, but no prompt yet: Codex writes the rollout lazily.
                None => (
                    format!("codex-pid-{}", p.pid),
                    State::Idle,
                    "started".into(),
                ),
            };
            Observation {
                session_id,
                cwd: p.cwd.clone(),
                state,
                detail,
                pid: Some(p.pid),
                label: None,
                tty: Some(p.tty.clone()),
                transcript: own.map(|r| r.path.clone()),
                background: None,
            }
        })
        .collect()
}

/// Rollout files modified at or after `since` (epoch seconds), with their
/// session id and cwd from the first line.
fn rollouts_since(root: &Path, since: u64) -> Vec<Rollout> {
    let mut found = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                dirs.push(path);
                continue;
            }
            let modified = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            if modified < since || path.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            if let Some((id, cwd)) = cached_session_meta(&path, modified) {
                found.push(Rollout {
                    id,
                    cwd,
                    modified,
                    path,
                });
            }
        }
    }
    found
}

/// `session_meta` never changes once written, so each file is parsed once
/// (per modification time) rather than on every poll.
fn cached_session_meta(path: &Path, modified: u64) -> Option<(String, String)> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    type Cache = HashMap<PathBuf, (u64, Option<(String, String)>)>;
    static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
    let mut guard = CACHE.lock().ok()?;
    let cache = guard.get_or_insert_with(HashMap::new);
    if let Some((at, meta)) = cache.get(path) {
        if *at == modified {
            return meta.clone();
        }
    }
    let meta = session_meta(path);
    cache.insert(path.to_path_buf(), (modified, meta.clone()));
    meta
}

fn session_meta(path: &Path) -> Option<(String, String)> {
    let mut first = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(std::fs::File::open(path).ok()?),
        &mut first,
    )
    .ok()?;
    let line: serde_json::Value = serde_json::from_str(&first).ok()?;
    if line["type"] != "session_meta" {
        return None;
    }
    let p = &line["payload"];
    let id = p["id"].as_str().or(p["session_id"].as_str())?;
    Some((id.to_string(), p["cwd"].as_str()?.to_string()))
}

/// The state implied by the last turn event in the file's tail.
fn last_turn(path: &Path) -> (State, String) {
    const TAIL: u64 = 256 * 1024;
    let mut text = String::new();
    if let Ok(mut f) = std::fs::File::open(path) {
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let _ = f.seek(SeekFrom::Start(len.saturating_sub(TAIL)));
        let mut bytes = Vec::new();
        let _ = f.read_to_end(&mut bytes);
        text = String::from_utf8_lossy(&bytes).into_owned();
    }
    for line in text.lines().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v["type"] != "event_msg" {
            continue;
        }
        match v["payload"]["type"].as_str() {
            Some("task_started") => return (State::Working, "working".into()),
            Some("task_complete") => {
                let msg = v["payload"]["last_agent_message"]
                    .as_str()
                    .unwrap_or("done");
                return (State::Done, crate::inbox::one_line(msg));
            }
            Some("turn_aborted") => return (State::Idle, "interrupted".into()),
            _ => {}
        }
    }
    (State::Idle, "idle".into())
}

/// The process's working directory (`lsof` on macOS, `/proc` on Linux).
fn cwd_of(pid: u32) -> Option<String> {
    if let Ok(link) = std::fs::read_link(format!("/proc/{pid}/cwd")) {
        return Some(link.display().to_string());
    }
    let out = run_with_timeout(
        "lsof",
        &["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"],
        Duration::from_secs(2),
    )?;
    out.lines()
        .find_map(|l| l.strip_prefix('n'))
        .map(String::from)
}

/// Current Codex sessions, or None if processes can't be listed right now.
pub fn poll() -> Option<Vec<Observation>> {
    let ps = std::env::var("TAPIN_PS_CMD").unwrap_or_else(|_| "ps".into());
    let text = run_with_timeout(
        &ps,
        &["-axo", "pid=,tty=,etime=,comm=,args="],
        Duration::from_secs(5),
    )?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let mut procs = parse_ps(&text, now);
    procs.retain_mut(|p| match cwd_of(p.pid) {
        Some(cwd) => {
            p.cwd = cwd;
            true
        }
        None => false,
    });
    let home = match std::env::var_os("CODEX_HOME") {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".codex"),
    };
    Some(observe(&procs, &home.join("sessions")))
}
