//! How full each agent's context window is, read from the tail of its own
//! transcript (Claude Code) or rollout (Codex CLI). Nothing here writes.

use crate::event::AgentKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// First read from the end of the file, doubled until a record is found.
const FIRST_READ: u64 = 256 * 1024;
/// Never read more than this per file per poll.
const MAX_READ: u64 = 4 * 1024 * 1024;

/// How much of its context window a session uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    /// Tokens in the context as of the last turn.
    pub used: u64,
    /// The window, only when it's known for sure (never guessed).
    pub window: Option<u64>,
}

impl Context {
    /// Percent used, when the window is known.
    pub fn percent(self) -> Option<u8> {
        let w = self.window.filter(|w| *w > 0)?;
        Some((self.used.saturating_mul(100) / w).min(100) as u8)
    }
}

/// The context use for a session, if its transcript says.
pub fn read(agent: AgentKind, path: &Path) -> Option<Context> {
    let (needle, percent): (&str, fn(&str) -> Option<Context>) = match agent {
        AgentKind::Claude => ("\"assistant\"|compact_boundary", claude_context),
        AgentKind::Codex => ("\"token_count\"", codex_context),
        AgentKind::Gemini => return None,
    };
    // Only regular files: opening a FIFO or a tty would block the poll thread.
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut want = FIRST_READ;
    loop {
        let start = len.saturating_sub(want);
        file.seek(SeekFrom::Start(start)).ok()?;
        let mut buf = Vec::new();
        file.by_ref().take(want).read_to_end(&mut buf).ok()?;
        let text = String::from_utf8_lossy(&buf);
        // The first line is cut off unless the read began at the start.
        let whole = if start == 0 {
            &text[..]
        } else {
            text.split_once('\n').map_or("", |(_, rest)| rest)
        };
        // Only parse lines that could be a record (transcripts get big).
        let candidates: String = whole
            .lines()
            .filter(|l| needle.split('|').any(|n| l.contains(n)))
            .collect::<Vec<_>>()
            .join("\n");
        if let Some(p) = percent(&candidates) {
            return Some(p);
        }
        // Each try re-reads the whole tail: simple, and bounded by MAX_READ
        // (a miss costs under 8 MB, only when the file changed).
        if start == 0 || want >= MAX_READ {
            return None;
        }
        want = (want * 2).min(MAX_READ);
    }
}

/// Claude Code: the last main-thread assistant turn's input, cache writes and
/// cache reads. The window is known only for `[1m]` models: some models get 1M
/// without the marker (seen: claude-opus-4-8 at 257k), so 200k is never assumed.
/// After a `/compact`, until the next turn, it's the size the compact left.
pub fn claude_context(lines: &str) -> Option<Context> {
    // Set when the newest record is a compact boundary: its size wins, and the
    // turn before it still tells the window (a known fact, not a guess).
    let mut compacted: Option<u64> = None;
    for line in lines.lines().rev() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v["isSidechain"] == true {
            continue;
        }
        if v["subtype"] == "compact_boundary" && compacted.is_none() {
            if let Some(post) = v["compactMetadata"]["postTokens"].as_u64() {
                compacted = Some(post);
            }
            continue;
        }
        if let Some(turn) = assistant_turn(&v) {
            return Some(Context {
                used: compacted.unwrap_or(turn.used),
                window: turn.window,
            });
        }
    }
    compacted.map(|used| Context { used, window: None })
}

/// One main-thread assistant record's context, if `v` is one.
fn assistant_turn(v: &Value) -> Option<Context> {
    if v["type"] != "assistant" {
        return None;
    }
    let model = v["message"]["model"].as_str().unwrap_or_default();
    if model == "<synthetic>" {
        return None;
    }
    let usage = &v["message"]["usage"];
    let used = [
        "input_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
    ]
    .iter()
    .map(|k| usage[k].as_u64())
    .sum::<Option<u64>>()?;
    let requested = v["requestedModel"].as_str().unwrap_or_default();
    let window = (requested.ends_with("[1m]") || model.ends_with("[1m]")).then_some(1_000_000);
    Some(Context { used, window })
}

/// Codex CLI: the last turn's tokens out of the window Codex reports. (Codex's
/// own "% left" subtracts a baseline first, so the numbers can differ a little.)
pub fn codex_context(lines: &str) -> Option<Context> {
    lines.lines().rev().find_map(|line| {
        let v: Value = serde_json::from_str(line).ok()?;
        let info = &v["payload"]["info"];
        if v["payload"]["type"] != "token_count" || info.is_null() {
            return None;
        }
        let used = info["last_token_usage"]["total_tokens"].as_u64()?;
        let window = info["model_context_window"].as_u64().filter(|w| *w > 0);
        Some(Context { used, window })
    })
}

/// Remembers each file's answer until the file changes, so an idle session's
/// transcript isn't re-read every poll.
#[derive(Default)]
pub struct Cache {
    known: std::collections::HashMap<
        std::path::PathBuf,
        (u64, std::time::SystemTime, Option<Context>),
    >,
}

impl Cache {
    /// The context % for each session, reading only files that changed.
    pub fn read_all(
        &mut self,
        sessions: &[(String, AgentKind, std::path::PathBuf)],
    ) -> Vec<(String, Option<Context>)> {
        self.known
            .retain(|path, _| sessions.iter().any(|(_, _, p)| p == path));
        sessions
            .iter()
            .map(|(id, agent, path)| {
                let stamp = std::fs::metadata(path)
                    .ok()
                    .filter(|m| m.is_file())
                    .and_then(|m| Some((m.len(), m.modified().ok()?)));
                let pct = match (stamp, self.known.get(path)) {
                    (Some((len, at)), Some((l, a, pct))) if *l == len && *a == at => *pct,
                    (Some((len, at)), _) => {
                        let pct = read(*agent, path);
                        self.known.insert(path.clone(), (len, at, pct));
                        pct
                    }
                    (None, _) => None,
                };
                (id.clone(), pct)
            })
            .collect()
    }
}
