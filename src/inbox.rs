//! The Inbox: applies agent events and orders sessions by how urgently
//! they need the user.

use crate::event::{AgentEvent, AgentKind, EventKind};
use crate::location::Location;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Declaration order is display order: most urgent first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum State {
    NeedsPermission,
    Done,
    Working,
    #[default]
    Idle,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::NeedsPermission => "NEEDS",
            State::Done => "DONE",
            State::Working => "WORKING",
            State::Idle => "IDLE",
        }
    }
}

#[derive(Debug, Clone)]
struct Session {
    id: String,
    agent: AgentKind,
    cwd: String,
    state: State,
    detail: String,
    since: u64,
    /// Daemon-assigned id of the request tapin can still answer, if any.
    request: Option<u64>,
    /// The full tool input awaiting approval, so the user can see all of it.
    request_input: String,
    /// The agent's last message in full (the row shows one line of it).
    last_message: String,
    location: Location,
    model: Option<String>,
    /// Reported through hooks: passive observations never overwrite it.
    hooked: bool,
    /// The summary of the most recent tool started.
    last_tool: String,
    /// The agent's own name for the session, if a passive source gave one.
    label: Option<String>,
}

/// One inbox line, ready to render.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub session_id: String,
    pub state: State,
    pub agent: AgentKind,
    pub project: String,
    pub detail: String,
    pub age_secs: u64,
    /// Present when `y`/`n` can answer this row; send it back in `Decide`.
    pub request_id: Option<u64>,
    /// Complete input of the pending tool use (may span lines).
    pub request_input: Option<String>,
    pub cwd: String,
    pub last_message: String,
    /// Where the session runs, for jumping to it.
    pub location: Location,
    pub model: Option<String>,
    /// Only seen from outside (no hooks): it can't be answered from tapin.
    pub passive: bool,
}

#[derive(Debug, Default)]
pub struct Inbox {
    sessions: Vec<Session>,
    /// Recently ended sessions, by id → when. Each hook is its own connection,
    /// so a Stop can land after SessionEnd; it must not revive the row.
    ended: Vec<(String, u64)>,
    /// Ids in each passive source's latest snapshot. A tombstone outlives its
    /// timeout while its session is still listed, so it can't come back.
    observed: Vec<(AgentKind, Vec<String>)>,
    /// Folders whose sessions are left out (`hide_dirs`).
    hidden: Vec<PathBuf>,
}

/// How long an ended session's id is remembered.
const TOMBSTONE_SECS: u64 = 120;

impl Inbox {
    /// An inbox that leaves out sessions running in `dirs` or below.
    pub fn hiding(dirs: Vec<PathBuf>) -> Inbox {
        Inbox {
            hidden: dirs,
            ..Inbox::default()
        }
    }

    /// True if a session in `cwd` is left out of this inbox.
    pub fn hides(&self, cwd: &str) -> bool {
        self.hidden.iter().any(|d| Path::new(cwd).starts_with(d))
    }

    /// Applies one event observed at time `at` (seconds).
    pub fn apply(&mut self, ev: AgentEvent, at: u64) {
        // A session that moves into a hidden folder leaves the inbox.
        if self.hides(&ev.cwd) {
            self.sessions.retain(|s| s.id != ev.session_id);
            return;
        }
        self.prune_tombstones(at);
        if matches!(ev.kind, EventKind::Started) {
            self.ended.retain(|(id, _)| *id != ev.session_id);
        } else if self.ended.iter().any(|(id, _)| *id == ev.session_id) {
            return;
        }
        // Hooks race, so the PreToolUse for the very tool awaiting approval can
        // land after its PermissionRequest; ignore it. A *different* tool starting
        // means the request was answered elsewhere (e.g. denied in the terminal).
        if let EventKind::ToolStarted { summary } = &ev.kind {
            let trailing = self.sessions.iter().any(|s| {
                s.id == ev.session_id && s.state == State::NeedsPermission && &s.detail == summary
            });
            if trailing {
                return;
            }
        }
        let input = match &ev.kind {
            EventKind::PermissionRequested { input, .. } => input.clone(),
            _ => String::new(),
        };
        let mut last_message = None;
        let model = ev.model.clone();
        let started_tool = match &ev.kind {
            EventKind::ToolStarted { summary } => Some(summary.clone()),
            _ => None,
        };
        let (state, detail) = match ev.kind {
            EventKind::Ended => {
                self.end(&ev.session_id, at);
                return;
            }
            EventKind::Started => (State::Idle, "started".to_string()),
            EventKind::PromptSubmitted => (State::Working, "thinking".to_string()),
            EventKind::ToolStarted { summary } => (State::Working, summary),
            EventKind::PermissionRequested { summary, .. } => {
                // No tool named (Gemini's notification): keep the tool already shown.
                let summary = if summary.is_empty() {
                    self.sessions
                        .iter()
                        .find(|s| s.id == ev.session_id)
                        .map(|s| s.detail.clone())
                        .unwrap_or_default()
                } else {
                    summary
                };
                (State::NeedsPermission, summary)
            }
            EventKind::ToolFinished => {
                // Back to the tool that ran, not a permission notice shown meanwhile.
                let detail = self
                    .sessions
                    .iter()
                    .find(|s| s.id == ev.session_id)
                    .map(|s| {
                        if s.last_tool.is_empty() {
                            s.detail.clone()
                        } else {
                            s.last_tool.clone()
                        }
                    })
                    .unwrap_or_default();
                (State::Working, detail)
            }
            EventKind::TurnDone { message } => {
                last_message = Some(message.clone());
                (State::Done, one_line(&message))
            }
        };
        let session = match self.sessions.iter_mut().find(|s| s.id == ev.session_id) {
            Some(s) => s,
            None => {
                self.sessions.push(Session {
                    id: ev.session_id,
                    agent: ev.agent,
                    cwd: ev.cwd,
                    state,
                    detail: String::new(),
                    since: at,
                    request: None,
                    request_input: String::new(),
                    last_message: String::new(),
                    location: Location::default(),
                    model: None,
                    hooked: true,
                    last_tool: String::new(),
                    label: None,
                });
                self.sessions.last_mut().unwrap()
            }
        };
        if session.state != state {
            session.since = at;
        }
        session.state = state;
        session.detail = detail;
        session.request = None;
        session.request_input = input;
        if let Some(m) = last_message {
            session.last_message = m;
        }
        if model.is_some() {
            session.model = model;
        }
        session.hooked = true;
        if let Some(tool) = started_tool {
            session.last_tool = tool;
        }
    }

    /// Lets tapin answer the session's current request under `id`.
    pub fn attach_request(&mut self, session_id: &str, id: u64) {
        if let Some(s) = self.session_mut(session_id) {
            if s.state == State::NeedsPermission {
                s.request = Some(id);
            }
        }
    }

    /// tapin can no longer answer request `id` (its hook gave up). The agent's
    /// own dialog may still be open, so the row keeps needing the user.
    pub fn detach_request(&mut self, id: u64) {
        for s in &mut self.sessions {
            if s.request == Some(id) {
                s.request = None;
            }
        }
    }

    /// Merges what a passive source currently sees for one agent kind. Hooked
    /// sessions are left alone; passive-only sessions of that kind that are
    /// no longer seen are dropped.
    pub fn observe(
        &mut self,
        agent: AgentKind,
        observed: Vec<crate::passive::Observation>,
        at: u64,
    ) {
        let mut observed = observed;
        observed.retain(|o| !self.hides(&o.cwd));
        let ids = observed.iter().map(|o| o.session_id.clone()).collect();
        match self.observed.iter_mut().find(|(a, _)| *a == agent) {
            Some(slot) => slot.1 = ids,
            None => self.observed.push((agent, ids)),
        }
        self.prune_tombstones(at);
        for o in &observed {
            if self.ended.iter().any(|(id, _)| *id == o.session_id) {
                continue;
            }
            match self.sessions.iter_mut().find(|s| s.id == o.session_id) {
                // Hooks report their own agent; a passive guess never overrides it.
                Some(s) if s.hooked => {}
                Some(s) => {
                    if s.state != o.state {
                        s.since = at;
                    }
                    s.state = o.state;
                    s.detail.clone_from(&o.detail);
                    s.location.agent_pid = o.pid.or(s.location.agent_pid);
                    if o.tty.is_some() {
                        s.location.tty.clone_from(&o.tty);
                    }
                    if o.label.is_some() {
                        s.label.clone_from(&o.label);
                    }
                }
                None => self.sessions.push(Session {
                    id: o.session_id.clone(),
                    agent,
                    cwd: o.cwd.clone(),
                    state: o.state,
                    detail: o.detail.clone(),
                    since: at,
                    request: None,
                    request_input: String::new(),
                    last_message: String::new(),
                    location: Location {
                        agent_pid: o.pid,
                        tty: o.tty.clone(),
                        ..Location::default()
                    },
                    model: None,
                    hooked: false,
                    last_tool: String::new(),
                    label: o.label.clone(),
                }),
            }
        }
        self.sessions.retain(|s| {
            s.hooked || s.agent != agent || observed.iter().any(|o| o.session_id == s.id)
        });
    }

    /// Forgets ended sessions after a while, unless a passive source still lists them.
    fn prune_tombstones(&mut self, at: u64) {
        let observed = &self.observed;
        self.ended.retain(|(id, t)| {
            at.saturating_sub(*t) < TOMBSTONE_SECS
                || observed.iter().any(|(_, ids)| ids.contains(id))
        });
    }

    /// Removes a session and remembers that it ended.
    pub fn end(&mut self, session_id: &str, at: u64) {
        self.sessions.retain(|s| s.id != session_id);
        self.ended.push((session_id.to_string(), at));
    }

    /// Sessions whose agent process is known: (id, pid, start time).
    pub fn agents(&self) -> Vec<(String, u32, Option<u64>)> {
        self.sessions
            .iter()
            .filter_map(|s| Some((s.id.clone(), s.location.agent_pid?, s.location.agent_start)))
            .collect()
    }

    /// Ends a session only if it still belongs to that agent process (a new
    /// SessionStart may have arrived since the check).
    pub fn end_if_agent(&mut self, session_id: &str, pid: u32, at: u64) -> bool {
        let still = self
            .sessions
            .iter()
            .any(|s| s.id == session_id && s.location.agent_pid == Some(pid));
        if still {
            self.end(session_id, at);
        }
        still
    }

    /// Records where a session runs, keeping known parts the new report lacks.
    pub fn locate(&mut self, session_id: &str, location: &Location) {
        if let Some(s) = self.session_mut(session_id) {
            s.location.merge(location);
        }
    }

    /// A session's current state, if it is in the inbox.
    pub fn state_of(&self, session_id: &str) -> Option<State> {
        Some(self.sessions.iter().find(|s| s.id == session_id)?.state)
    }

    /// One session's row, as `rows` would show it.
    pub fn row(&self, session_id: &str, now: u64) -> Option<Row> {
        self.rows(now)
            .into_iter()
            .find(|r| r.session_id == session_id)
    }

    /// The request tapin can currently answer for this session.
    pub fn request_of(&self, session_id: &str) -> Option<u64> {
        self.sessions.iter().find(|s| s.id == session_id)?.request
    }

    fn session_mut(&mut self, session_id: &str) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.id == session_id)
    }

    /// Marks a session's pending request as settled: the agent carries on working.
    pub fn settle(&mut self, session_id: &str, at: u64) {
        if let Some(s) = self.session_mut(session_id) {
            if s.state != State::Working {
                s.since = at;
            }
            s.state = State::Working;
            s.request = None;
            s.request_input.clear();
        }
    }

    /// Sessions in display order: by urgency, then longest-waiting first.
    pub fn rows(&self, now: u64) -> Vec<Row> {
        let mut sessions: Vec<&Session> = self.sessions.iter().collect();
        sessions.sort_by_key(|s| (s.state, s.since));
        sessions
            .into_iter()
            .map(|s| Row {
                session_id: s.id.clone(),
                state: s.state,
                agent: s.agent,
                project: s
                    .label
                    .clone()
                    .unwrap_or_else(|| project_name(&s.cwd).to_string()),
                detail: s.detail.clone(),
                age_secs: now.saturating_sub(s.since),
                request_id: s.request,
                request_input: (s.state == State::NeedsPermission).then(|| s.request_input.clone()),
                cwd: s.cwd.clone(),
                last_message: s.last_message.clone(),
                location: s.location.clone(),
                model: s.model.clone(),
                passive: !s.hooked,
            })
            .collect()
    }

    /// One line per row, for tests and debugging.
    pub fn render_plain(&self, now: u64) -> String {
        self.rows(now)
            .iter()
            .map(|r| {
                format!(
                    "{:<8}{}  {}  {}  {}s",
                    r.state.label(),
                    r.agent.name(),
                    r.project,
                    r.detail,
                    r.age_secs
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn project_name(cwd: &str) -> &str {
    cwd.rsplit('/').find(|p| !p.is_empty()).unwrap_or(cwd)
}

const SUMMARY_CHARS: usize = 48;

/// First line of `text`, cut to `SUMMARY_CHARS`; an ellipsis marks anything dropped.
pub(crate) fn one_line(text: &str) -> String {
    let text = text.trim();
    let line = text.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= SUMMARY_CHARS {
        let more = if line.len() < text.len() { "…" } else { "" };
        return format!("{line}{more}");
    }
    let cut: String = line.chars().take(SUMMARY_CHARS).collect();
    format!("{cut}…")
}
