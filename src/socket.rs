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
    /// Answer a session's pending permission request.
    Decide { request_id: u64, allow: bool },
}

/// How a pending permission request was settled, as told to the waiting hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Deny,
    /// Settled elsewhere (e.g. in the agent's own dialog): say nothing.
    Pass,
}

/// Daemon → a hook waiting on a permission request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Reply {
    Decision { decision: Decision },
}

/// Daemon → subscriber.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Update {
    Rows { rows: Vec<Row> },
}
