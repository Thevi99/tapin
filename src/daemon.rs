//! `tapin daemon`: owns the socket and the Inbox, fans snapshots out to TUIs.

use crate::adapters;
use crate::config::{Config, NotifyOn};
use crate::event::{AgentKind, EventKind};
use crate::inbox::{Inbox, State as SessionState};
use crate::notify::Notifier;
use crate::socket::{self, Decision, Reply, Request, Update};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Default)]
struct Shared {
    inbox: Inbox,
    /// Open inboxes, by connection id; removed as soon as the connection closes.
    subscribers: Vec<(u64, UnixStream)>,
    /// The open inboxes that passed the peer check (#12) and so may answer.
    answerers: Vec<u64>,
    /// Every agent pid hooks or passive mode reported, while it lives. Only
    /// ever added to: a spoofed pid can't make the real one forgotten.
    agent_pids: Vec<(u32, Option<u64>)>,
    next_subscriber: u64,
    /// Hooks blocked on a permission answer, by request id.
    pending: HashMap<u64, Pending>,
    next_request: u64,
}

struct Pending {
    session_id: String,
    agent: AgentKind,
    hook: UnixStream,
}

impl Shared {
    /// Releases the session's waiting hook, if any, without an answer.
    fn pass(&mut self, session_id: &str) {
        let ids: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, p)| p.session_id == session_id)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if let Some(p) = self.pending.remove(&id) {
                reply(p.hook, Decision::Pass);
            }
        }
    }
}

type State = Arc<Mutex<Shared>>;

pub fn run(socket: &Path, notify: bool) -> Result<()> {
    let Some(listener) = bind(socket)? else {
        return Ok(()); // another daemon already serves this socket
    };
    let config = Config::load();
    let state: State = Arc::new(Mutex::new(Shared {
        inbox: Inbox::hiding(config.hidden_dirs()),
        ..Shared::default()
    }));
    let notifier = Arc::new(Notifier::new(&config, notify));

    let poller = state.clone();
    let every = Duration::from_secs(config.passive_poll_secs.max(1));
    std::thread::spawn(move || {
        let mut contexts = crate::context::Cache::default();
        loop {
            let claude = crate::passive::claude::poll();
            let codex = crate::passive::codex::poll();
            let known: Vec<u32> = {
                let shared = poller.lock().unwrap();
                shared.agent_pids.iter().map(|(p, _)| *p).collect()
            };
            let passive_pids: Vec<(u32, Option<u64>)> = claude
                .iter()
                .chain(codex.iter())
                .flatten()
                .filter_map(|o| o.pid)
                .filter(|pid| !known.contains(pid))
                .map(|pid| (pid, crate::location::process_start(pid)))
                .collect();
            let mut shared = poller.lock().unwrap();
            // Pids seen by passive mode come from the OS: remember them too,
            // with start times (read above, outside the lock) so a reused pid
            // is never taken for the agent.
            for (pid, start) in &passive_pids {
                remember_agent(&mut shared, *pid, *start);
            }
            if let Some(observed) = claude {
                shared.inbox.observe(AgentKind::Claude, observed, now());
            }
            if let Some(observed) = codex {
                shared.inbox.observe(AgentKind::Codex, observed, now());
            }
            let transcripts = shared.inbox.transcripts();
            drop(shared);
            // File reads happen outside the lock, so hooks never wait on them.
            let read = contexts.read_all(&transcripts);
            let mut shared = poller.lock().unwrap();
            for (id, pct) in read {
                shared.inbox.set_context(&id, pct);
            }
            // The 1s ticker broadcasts the result; no extra broadcast per poll.
            drop(shared);
            std::thread::sleep(every);
        }
    });

    let watcher = state.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(LIVENESS_EVERY);
        reap_dead_agents(&watcher);
    });

    let ticker = state.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        broadcast(&mut ticker.lock().unwrap());
    });

    // Fixed at startup: the binary this daemon runs as (see `peer`).
    let trusted = Arc::new(crate::peer::trusted_binaries());
    for conn in listener.incoming().flatten() {
        let state = state.clone();
        let notifier = notifier.clone();
        let trusted = trusted.clone();
        std::thread::spawn(move || handle(conn, &state, &notifier, &trusted));
    }
    Ok(())
}

/// Binds a 0600 socket inside a private dir, creating the dir (0700) if needed.
/// Returns None if a live daemon already owns the socket.
fn bind(socket: &Path) -> Result<Option<UnixListener>> {
    socket::check_path(socket)?;
    let dir = socket.parent().context("socket path has no parent dir")?;
    if !dir.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    anyhow::ensure!(
        socket::is_private_dir(dir),
        "refusing to listen in {}: it must be owned by you and not writable by others",
        dir.display()
    );
    if socket.exists() {
        if UnixStream::connect(socket).is_ok() {
            return Ok(None);
        }
        std::fs::remove_file(socket).context("removing stale socket")?;
    }
    // Create the socket as 0600 from the start, so no one can connect before the chmod.
    // SAFETY: umask has no preconditions; the previous mask is restored right after.
    let old = unsafe { libc::umask(0o177) };
    let bound = UnixListener::bind(socket);
    unsafe { libc::umask(old) };
    let listener = bound.context("binding socket")?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    Ok(Some(listener))
}

/// True if the process at the other end of `conn` may answer requests (#12).
fn may_answer(conn: &UnixStream, state: &State, trusted: &[crate::peer::Trusted]) -> bool {
    let Some(peer) = crate::peer::checked(conn) else {
        return false;
    };
    let agent_pids: Vec<u32> = {
        let shared = state.lock().unwrap();
        let known = shared.agent_pids.iter().map(|(pid, _)| *pid);
        let current = shared.inbox.agents().into_iter().map(|(_, pid, _)| pid);
        known.chain(current).filter(|pid| *pid > 1).collect()
    };
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    crate::peer::verify(&peer, uid, trusted, &agent_pids).is_ok()
}

fn handle(conn: UnixStream, state: &State, notifier: &Notifier, trusted: &[crate::peer::Trusted]) {
    let Ok(writer) = conn.try_clone() else { return };
    // The request this connection's hook is waiting on, if it is one.
    let mut waiting_on: Option<u64> = None;
    // The subscription this connection opened, if it is an inbox.
    let mut subscribed_as: Option<u64> = None;
    for line in BufReader::new(conn).lines() {
        let Ok(line) = line else { break };
        match serde_json::from_str(&line) {
            Ok(Request::Event {
                agent,
                payload,
                mut location,
            }) => {
                // A reported agent pid must be a live process named like an
                // agent: anything else (pid 1, the user's terminal or tmux)
                // could be used to refuse every inbox. (Not "the sender's
                // ancestor": a hook has usually exited by the time we look.)
                if let Some(loc) = location.as_deref_mut() {
                    if loc
                        .agent_pid
                        .is_some_and(|pid| !crate::location::is_agent_named(pid))
                    {
                        loc.agent_pid = None;
                        loc.agent_start = None;
                    }
                }
                let event = adapters::hooks::parse(&payload, agent);
                if let Some(event) = event {
                    let mut shared = state.lock().unwrap();
                    let session_id = event.session_id.clone();
                    // A hidden session is never shown, so nobody could answer it here.
                    if shared.inbox.hides(&event.cwd) {
                        let was_shown = shared.inbox.state_of(&session_id).is_some();
                        shared.inbox.apply(event, now());
                        if was_shown {
                            shared.pass(&session_id);
                            broadcast(&mut shared);
                        }
                        if adapters::hooks::wants_decision(&payload) {
                            if let Ok(hook) = writer.try_clone() {
                                reply(hook, Decision::Pass);
                            }
                        }
                        continue;
                    }
                    let is_request = matches!(event.kind, EventKind::PermissionRequested { .. });
                    // Only a hook that waits for the answer can be answered (Gemini's
                    // permission notice is observability only).
                    let answerable = is_request && adapters::hooks::wants_decision(&payload);
                    // The Inbox decides whether this event settles the open request
                    // (it ignores a PreToolUse trailing its own PermissionRequest).
                    let before = shared.inbox.request_of(&session_id);
                    let was = shared.inbox.state_of(&session_id);
                    shared.inbox.apply(event, now());
                    if let Some(location) = &location {
                        shared.inbox.locate(&session_id, location);
                        if let Some(pid) = location.agent_pid {
                            remember_agent(&mut shared, pid, location.agent_start);
                        }
                    }
                    // A request replacing another (NEEDS → NEEDS) is news too.
                    let was = if is_request { None } else { was };
                    announce(notifier, &shared.inbox, &session_id, was);
                    if before.is_some() && shared.inbox.request_of(&session_id) != before {
                        shared.pass(&session_id);
                    }
                    // An agent that holds its own prompt until the hook returns must not
                    // wait on a tapin nobody is looking at: let it ask in its terminal.
                    let nobody_to_answer = shared.answerers.is_empty();
                    if answerable && agent.prompt_waits_for_hook() && nobody_to_answer {
                        if let Ok(hook) = writer.try_clone() {
                            reply(hook, Decision::Pass);
                        }
                    } else if answerable {
                        if let Ok(hook) = writer.try_clone() {
                            shared.next_request += 1;
                            let id = shared.next_request;
                            let session_id = session_id.clone();
                            shared.pending.insert(
                                id,
                                Pending {
                                    session_id: session_id.clone(),
                                    agent,
                                    hook,
                                },
                            );
                            shared.inbox.attach_request(&session_id, id);
                            waiting_on = Some(id);
                        }
                    }
                    broadcast(&mut shared);
                }
            }
            Ok(Request::Subscribe) if subscribed_as.is_none() => {
                let Ok(mut sub) = writer.try_clone() else {
                    return;
                };
                let _ = sub.set_write_timeout(Some(Duration::from_millis(200)));
                // Checked before taking the lock: it reads the process table.
                let can_answer = may_answer(&writer, state, trusted);
                if let Ok(mut line) = serde_json::to_string(&Update::Access { can_answer }) {
                    line.push('\n');
                    let _ = sub.write_all(line.as_bytes());
                }
                let mut shared = state.lock().unwrap();
                shared.next_subscriber += 1;
                let id = shared.next_subscriber;
                shared.subscribers.push((id, sub));
                if can_answer {
                    shared.answerers.push(id);
                }
                subscribed_as = Some(id);
                broadcast(&mut shared);
            }
            Ok(Request::Subscribe) => {}
            Ok(Request::Hello) => {
                #[cfg(debug_assertions)]
                if std::env::var_os("TAPIN_TEST_NO_HELLO").is_some() {
                    continue; // plays a daemon from before the handshake
                }
                let hello = Reply::Hello {
                    protocol: socket::PROTOCOL,
                };
                if let Ok(mut line) = serde_json::to_string(&hello) {
                    line.push('\n');
                    let _ = (&writer).write_all(line.as_bytes());
                }
            }
            Ok(Request::Snapshot) => {
                let rows = state.lock().unwrap().inbox.rows(now());
                if let Ok(mut line) = serde_json::to_string(&Update::Rows { rows }) {
                    line.push('\n');
                    let _ = writer.set_write_timeout(Some(Duration::from_millis(200)));
                    let _ = (&writer).write_all(line.as_bytes());
                }
            }
            Ok(Request::Decide { request_id, allow }) => {
                // Only an inbox that passed the peer check, checked again now
                // (the process table may have changed since it subscribed).
                let approved_inbox =
                    subscribed_as.is_some_and(|id| state.lock().unwrap().answerers.contains(&id));
                if !approved_inbox {
                    continue;
                }
                if !may_answer(&writer, state, trusted) {
                    // No longer trusted: it stops counting as an inbox that
                    // can answer, and is told so.
                    let mut shared = state.lock().unwrap();
                    if let Some(id) = subscribed_as {
                        shared.answerers.retain(|a| *a != id);
                    }
                    if shared.answerers.is_empty() {
                        release_unwatched(&mut shared);
                    }
                    drop(shared);
                    let access = Update::Access { can_answer: false };
                    if let Ok(mut line) = serde_json::to_string(&access) {
                        line.push('\n');
                        let _ = (&writer).write_all(line.as_bytes());
                    }
                    continue;
                }
                let mut shared = state.lock().unwrap();
                // Answers name the exact request: a stale or second answer finds nothing.
                if let Some(p) = shared.pending.remove(&request_id) {
                    let decision = if allow {
                        Decision::Allow
                    } else {
                        Decision::Deny
                    };
                    if reply(p.hook, decision) {
                        shared.inbox.settle(&p.session_id, now());
                    } else {
                        shared.inbox.detach_request(request_id);
                    }
                    broadcast(&mut shared);
                }
            }
            Err(_) => {}
        }
    }
    // An inbox that closes stops counting as someone who can answer.
    if let Some(id) = subscribed_as {
        let mut shared = state.lock().unwrap();
        shared.subscribers.retain(|(s, _)| *s != id);
        shared.answerers.retain(|s| *s != id);
        if shared.answerers.is_empty() {
            release_unwatched(&mut shared);
        }
    }
    // A hook that hangs up (timeout, killed) can no longer be answered.
    if let Some(id) = waiting_on {
        let mut shared = state.lock().unwrap();
        if shared.pending.remove(&id).is_some() {
            shared.inbox.detach_request(id);
            broadcast(&mut shared);
        }
    }
}

/// How often agent processes are checked for being alive.
const LIVENESS_EVERY: Duration = Duration::from_secs(2);

/// Ends sessions whose agent process is gone (or whose pid now belongs to a
/// different process, judged by start time). Checks run outside the lock.
/// Adds an agent process to the append-only registry (until it's gone).
fn remember_agent(shared: &mut Shared, pid: u32, start: Option<u64>) {
    if pid > 1 && !shared.agent_pids.iter().any(|(p, _)| *p == pid) {
        shared.agent_pids.push((pid, start));
    }
}

fn reap_dead_agents(state: &State) {
    state
        .lock()
        .unwrap()
        .agent_pids
        .retain(|(pid, start)| !crate::location::is_gone(*pid, *start));
    let agents = state.lock().unwrap().inbox.agents();
    let dead: Vec<(String, u32)> = agents
        .into_iter()
        .filter(|(_, pid, start)| crate::location::is_gone(*pid, *start))
        .map(|(id, pid, _)| (id, pid))
        .collect();
    if dead.is_empty() {
        return;
    }
    let mut shared = state.lock().unwrap();
    for (id, pid) in dead {
        if shared.inbox.end_if_agent(&id, pid, now()) {
            shared.pass(&id);
        }
    }
    broadcast(&mut shared);
}

/// With no inbox open, nobody can answer: release requests from agents that
/// hold their own prompt until the hook returns, so they ask in their terminal.
fn release_unwatched(shared: &mut Shared) {
    let ids: Vec<u64> = shared
        .pending
        .iter()
        .filter(|(_, p)| p.agent.prompt_waits_for_hook())
        .map(|(id, _)| *id)
        .collect();
    for id in ids {
        if let Some(p) = shared.pending.remove(&id) {
            reply(p.hook, Decision::Pass);
            shared.inbox.detach_request(id);
        }
    }
}

/// Notifies when a session has just started needing the user.
fn announce(notifier: &Notifier, inbox: &Inbox, session_id: &str, was: Option<SessionState>) {
    let Some(row) = inbox.row(session_id, now()) else {
        return;
    };
    if Some(row.state) == was {
        return;
    }
    let title = format!("tapin · {} {}", row.agent.name(), row.project);
    match row.state {
        SessionState::NeedsPermission => notifier.send(
            NotifyOn::Needs,
            &title,
            &format!("Needs approval: {}", row.detail),
        ),
        SessionState::Done => {
            notifier.send(NotifyOn::Done, &title, &format!("Done: {}", row.detail))
        }
        _ => {}
    }
}

/// Tells a waiting hook how its request was settled. False if it is gone.
fn reply(mut hook: UnixStream, decision: Decision) -> bool {
    let _ = hook.set_write_timeout(Some(Duration::from_millis(200)));
    let Ok(mut line) = serde_json::to_string(&Reply::Decision { decision }) else {
        return false;
    };
    line.push('\n');
    hook.write_all(line.as_bytes()).is_ok()
}

/// Sends the current rows to every subscriber, dropping the ones that are gone.
fn broadcast(shared: &mut Shared) {
    let update = Update::Rows {
        rows: shared.inbox.rows(now()),
    };
    let Ok(mut line) = serde_json::to_string(&update) else {
        return;
    };
    line.push('\n');
    let mut dropped = Vec::new();
    shared.subscribers.retain_mut(|(id, s)| {
        let ok = s.write_all(line.as_bytes()).is_ok();
        if !ok {
            // Hang up, so the inbox shows "disconnected" instead of looking
            // like it can still answer.
            let _ = s.shutdown(std::net::Shutdown::Both);
            dropped.push(*id);
        }
        ok
    });
    // A dropped inbox (gone, or too slow to read) can't answer any more.
    if !dropped.is_empty() {
        let had = !shared.answerers.is_empty();
        shared.answerers.retain(|a| !dropped.contains(a));
        if had && shared.answerers.is_empty() {
            release_unwatched(shared);
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
