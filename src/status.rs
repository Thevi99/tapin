//! `tapin status`: one line for a tmux or shell status bar.

use crate::inbox::{Row, State};
use crate::socket::{Request, Update};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// How long a status bar may wait on the daemon before showing nothing.
const TIMEOUT: Duration = Duration::from_millis(500);
/// When `tapin status` exits regardless.
const DEADLINE: Duration = Duration::from_millis(800);

/// `NEEDS 1 · DONE 2`, leaving out zero counts; empty when nothing waits.
pub fn line(rows: &[Row]) -> String {
    let count = |state| rows.iter().filter(|r| r.state == state).count();
    [
        ("NEEDS", count(State::NeedsPermission)),
        ("DONE", count(State::Done)),
    ]
    .into_iter()
    .filter(|(_, n)| *n > 0)
    .map(|(label, n)| format!("{label} {n}"))
    .collect::<Vec<_>>()
    .join(" · ")
}

/// Prints the line if a daemon answers. A status bar runs this every few
/// seconds, so it never starts a daemon and never prints an error.
pub fn run(socket: &Path) {
    // One deadline for everything, `connect` included: a wedged daemon's
    // backlog can block it, and a status bar must never pile up processes.
    std::thread::spawn(|| {
        std::thread::sleep(DEADLINE);
        std::process::exit(0);
    });
    if let Some(rows) = snapshot(socket) {
        let line = line(&rows);
        if !line.is_empty() {
            println!("{line}");
        }
    }
}

fn snapshot(socket: &Path) -> Option<Vec<Row>> {
    let mut stream = UnixStream::connect(socket).ok()?;
    stream.set_read_timeout(Some(TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(TIMEOUT)).ok()?;
    let mut request = serde_json::to_string(&Request::Snapshot).ok()?;
    request.push('\n');
    stream.write_all(request.as_bytes()).ok()?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).ok()?;
    match serde_json::from_str(&reply).ok()? {
        Update::Rows { rows } => Some(rows),
    }
}
