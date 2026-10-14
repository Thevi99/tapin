//! Claude Code via `claude agents --json`, which its docs call the supported
//! way to read session state from outside (agent view, research preview).
//! Transcript files are not used: their format is internal to Claude Code.

use super::{run_with_timeout, Observation};
use crate::inbox::State;
use serde::Deserialize;
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
            Some(Observation {
                session_id: e.session_id.or(e.id)?,
                cwd: e.cwd,
                state,
                detail,
                pid: e.pid,
                label: e.name.filter(|n| !n.is_empty()),
                tty: None,
            })
        })
        .collect())
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
    for o in &mut observed {
        o.tty = o
            .pid
            .and_then(|p| ttys.iter().find(|(q, _)| *q == p).map(|(_, t)| t.clone()));
    }
    Some(observed)
}
