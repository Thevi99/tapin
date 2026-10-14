//! Where the daemon listens, and the newline-delimited JSON spoken over it.

use crate::event::AgentKind;
use crate::inbox::Row;
use serde::{Deserialize, Serialize};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// `$XDG_RUNTIME_DIR/tapin/tapin.sock`, else a per-user dir under the temp dir.
pub fn default_path() -> PathBuf {
    let dir = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) => PathBuf::from(d).join("tapin"),
        None => {
            let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
            std::env::temp_dir().join(format!("tapin-{user}"))
        }
    };
    dir.join("tapin.sock")
}

/// Unix socket paths are capped by `sockaddr_un` (104 bytes on macOS, 108 on Linux).
const MAX_PATH_BYTES: usize = 103;

/// Rejects paths the OS can't bind, with an error that says how to fix it.
pub fn check_path(path: &Path) -> anyhow::Result<()> {
    let len = path.as_os_str().len();
    anyhow::ensure!(
        len <= MAX_PATH_BYTES,
        "socket path is too long ({len} bytes, max {MAX_PATH_BYTES}): {}\n\
         pass a shorter one with --socket",
        path.display()
    );
    Ok(())
}

/// True if `dir` is a real directory owned by us that no one else can write to.
/// Anything else could let another local user plant or swap the socket.
pub fn is_private_dir(dir: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(dir) else {
        return false;
    };
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    meta.is_dir() && meta.uid() == uid && meta.mode() & 0o022 == 0
}

/// Client → daemon.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Request {
    /// A raw hook payload, forwarded untouched by `tapin hook`.
    Event {
        agent: AgentKind,
        payload: serde_json::Value,
        /// Where the agent runs, as seen from the hook's environment.
        #[serde(default)]
        location: Option<Box<crate::location::Location>>,
    },
    /// Stream inbox snapshots to this connection.
    Subscribe,
    /// Asks the daemon which protocol it speaks; a waiting hook sends this
    /// first, so it never waits on a daemon too old to check who answers.
    Hello,
    /// Reply with one inbox snapshot and nothing more (status bars). Unlike
    /// Subscribe it doesn't count as an open inbox someone could answer from.
    Snapshot,
    /// Answer a session's pending permission request.
    Decide {
        request_id: u64,
        allow: bool,
        /// For a question: the chosen option's label (protocol 3).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answer: Option<String>,
    },
}

/// How a pending permission request was settled, as told to the waiting hook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Deny,
    /// Settled elsewhere (e.g. in the agent's own dialog): say nothing.
    Pass,
    /// Answer an AskUserQuestion with this option label.
    Answer(String),
}

/// Daemon → a hook waiting on a permission request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Reply {
    Decision {
        decision: Decision,
    },
    /// The answer to `Hello`.
    Hello {
        protocol: u32,
    },
}

/// The protocol this build speaks. 2: answers only from verified inboxes
/// (#12). 3: `Decide` may carry an answer to a question.
pub const PROTOCOL: u32 = 3;

/// The protocol hooks require before waiting on a daemon.
pub const MIN_HOOK_PROTOCOL: u32 = 2;

fn protocol_2() -> u32 {
    2
}

/// Daemon → subscriber.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Update {
    Rows {
        rows: Vec<Row>,
    },
    /// Sent once after Subscribe: whether this inbox may answer requests (#12).
    Access {
        can_answer: bool,
        /// The daemon's protocol; daemons before it sent this are 2.
        #[serde(default = "protocol_2")]
        protocol: u32,
    },
}
