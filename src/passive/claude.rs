//! Claude Code via `claude agents --json`, which its docs call the supported
//! way to read session state from outside (agent view, research preview).
//! Session state never comes from transcript files (their format is internal
//! to Claude Code); they are only located here for the context % column,
//! which shows nothing when it can't read them.

use super::{run_with_timeout, Observation};
use crate::inbox::State;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    kind: String,
    cwd: String,
    session_id: Option<String>,
    id: Option<String>,
    pid: Option<u32>,
    status: Option<String>,
    waiting_for: Option<String>,
    state: Option<String>,
    name: Option<String>,
}

pub fn parse_agents(json: &str) -> anyhow::Result<Vec<Observation>> {
    let entries: Vec<Entry> = serde_json::from_str(json)?;
    Ok(entries
        .into_iter()
        .filter_map(|e| {
            let (state, detail) = if e.kind == "background" {
                match e.state.as_deref()? {
                    "working" => (State::Working, "background: working".to_string()),
                    "blocked" => (State::NeedsPermission, "background: blocked".to_string()),
                    s @ ("done" | "failed") => (State::Done, format!("background: {s}")),
                    _ => return None, // stopped, or unknown
                }
            } else {
                match e.status.as_deref()? {
                    "busy" => (State::Working, "busy".to_string()),
                    "waiting" => (
                        State::NeedsPermission,
                        format!("waiting: {}", e.waiting_for.as_deref().unwrap_or("you")),
                    ),
                    "idle" => (State::Idle, "idle".to_string()),
                    _ => return None,
                }
            };
            let background = (e.kind == "background").then(|| e.id.clone()).flatten();
            Some(Observation {
                background,
                session_id: e.session_id.or(e.id)?,
                cwd: e.cwd,
                state,
                detail,
                pid: e.pid,
                label: e.name.filter(|n| !n.is_empty()),
                tty: None,
                transcript: None,
            })
        })
        .collect())
}

/// A session's transcript: `<projects>/<cwd with every non-alphanumeric
/// character as '-'>/<session>.jsonl`, or wherever `<session>.jsonl` is if
/// Claude named the folder another way.
pub fn transcript_for(projects: &Path, cwd: &str, session_id: &str) -> Option<PathBuf> {
    let file = format!("{session_id}.jsonl");
    let slug: String = cwd
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let direct = projects.join(slug).join(&file);
    if direct.is_file() {
        return Some(direct);
    }
    std::fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|d| d.path().join(&file))
        .find(|p| p.is_file())
}

/// `transcript_for`, remembered across polls: a found path is reused while
/// it exists, and a miss (e.g. a session with no turn yet) is retried at most
/// every 30s, so `projects/` isn't scanned for every session every poll.
fn remembered_transcript(projects: &Path, cwd: &str, session_id: &str) -> Option<PathBuf> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;
    type Seen = HashMap<String, (Option<PathBuf>, Instant)>;
    static SEEN: OnceLock<Mutex<Seen>> = OnceLock::new();
    let mut seen = SEEN.get_or_init(Mutex::default).lock().ok()?;
    match seen.get(session_id) {
        Some((Some(p), _)) if p.is_file() => return Some(p.clone()),
        Some((None, at)) if at.elapsed() < Duration::from_secs(30) => return None,
        _ => {}
    }
    let found = transcript_for(projects, cwd, session_id);
    if seen.len() > 1000 {
        seen.clear(); // sessions come and go; never grow without bound
    }
    seen.insert(session_id.to_string(), (found.clone(), Instant::now()));
    found
}

/// The current sessions, or None if Claude Code isn't available right now
/// (missing, too old for `agents --json`, or slow), in which case nothing
/// should be concluded about which sessions ended.
pub fn poll() -> Option<Vec<Observation>> {
    let program = std::env::var("TAPIN_CLAUDE_CMD").unwrap_or_else(|_| "claude".into());
    let out = run_with_timeout(&program, &["agents", "--json"], Duration::from_secs(5))?;
    let mut observed = parse_agents(&out).ok()?;
    let pids: Vec<u32> = observed.iter().filter_map(|o| o.pid).collect();
    let ttys = super::ttys_of(&pids);
    let projects = crate::setup::claude_dir().map(|d| d.join("projects"));
    for o in &mut observed {
        o.transcript = projects
            .as_deref()
            .and_then(|p| remembered_transcript(p, &o.cwd, &o.session_id));
        o.tty = o
            .pid
            .and_then(|p| ttys.iter().find(|(q, _)| *q == p).map(|(_, t)| t.clone()));
    }
    Some(observed)
}
