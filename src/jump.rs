//! Enter on a row: bring the user to that agent's pane or window.
//!
//! `jump_plan` is pure (what to run, or what to tell the user); `jump` gathers
//! the live facts it needs (tmux clients, whether the pane still exists) and
//! runs the plan. All data goes to commands as argv, never spliced into scripts.

use crate::inbox::Row;
use crate::location::Location;
use std::process::Command;

/// What the TUI knows about its own surroundings.
#[derive(Debug, Clone, Default)]
pub struct Here {
    /// The tmux server the TUI itself runs in, if any.
    pub tmux_socket: Option<String>,
    /// osascript is available.
    pub macos: bool,
    /// The terminal app the TUI runs in (`$TERM_PROGRAM`).
    pub term_program: Option<String>,
}

impl Here {
    pub fn current() -> Here {
        Here {
            tmux_socket: std::env::var("TMUX")
                .ok()
                .and_then(|t| t.split(',').next().map(String::from))
                .filter(|s| !s.is_empty()),
            macos: cfg!(target_os = "macos"),
            term_program: std::env::var("TERM_PROGRAM").ok().filter(|s| !s.is_empty()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Jump {
    /// Commands to run in order.
    Run(Vec<Vec<String>>),
    /// Nothing to run; tell the user this instead.
    Explain(String),
}

/// A background session id safe to hand to `claude` and a shell: letters,
/// digits, `-` and `_`, not empty and not option-like.
fn plain_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The command that stops a Claude background session (its conversation is
/// kept; `claude attach` reopens it), if the id is plain.
pub fn stop_plan(id: &str) -> Option<Vec<String>> {
    plain_id(id).then(|| vec!["claude".into(), "stop".into(), id.to_string()])
}

/// Runs `stop_plan` and says what happened.
pub fn stop(id: &str) -> String {
    let Some(cmd) = stop_plan(id) else {
        return "That session id looks wrong; stop it from `claude agents`.".into();
    };
    match Command::new(&cmd[0]).args(&cmd[1..]).output() {
        Ok(o) if o.status.success() => format!("Stopped {id}. `claude attach {id}` reopens it."),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            let out = String::from_utf8_lossy(&o.stdout);
            let why = [err.trim(), out.trim()]
                .into_iter()
                .find(|s| !s.is_empty())
                .map(String::from)
                .unwrap_or_else(|| o.status.to_string());
            format!("Couldn't stop {id}: {why}")
        }
        Err(e) => format!("Couldn't run claude: {e}"),
    }
}

/// What to tell the user when a jump command failed.
pub fn failure_message(stderr: &str) -> String {
    let err = stderr.trim();
    // macOS refuses Apple events until the user allows them (errAEEventNotPermitted).
    if err.contains("-1743") {
        return "macOS blocked tapin from controlling the terminal: allow it in System \
                Settings > Privacy & Security > Automation, then press Enter again."
            .into();
    }
    // osascript prefixes errors with "<from>:<to>: execution error: ".
    let err = err.split("execution error: ").last().unwrap_or(err);
    format!("Couldn't jump: {err}")
}

/// Open a new iTerm2 window running `claude attach <argv 1>`. No folder is
/// passed: `claude attach` opens the session wherever it runs. The id is
/// checked to be plain (letters, digits, -, _) before it gets here.
const ITERM_ATTACH_SCRIPT: &str = r#"on run argv
  tell application "iTerm"
    activate
    set w to (create window with default profile)
    tell current session of w to write text "claude attach " & (item 1 of argv)
  end tell
  return "ok"
end run"#;

/// The same in Terminal.app.
const TERMINAL_ATTACH_SCRIPT: &str = r#"on run argv
  tell application "Terminal"
    activate
    do script "claude attach " & (item 1 of argv)
  end tell
  return "ok"
end run"#;

/// Focus an iTerm2 session by its tty (argv 1).
const ITERM_SCRIPT: &str = r#"on run argv
  set target to item 1 of argv
  tell application "iTerm"
    repeat with w in windows
      repeat with t in tabs of w
        repeat with s in sessions of t
          if tty of s is target then
            select w
            tell t to select
            tell s to select
            activate
            return "ok"
          end if
        end repeat
      end repeat
    end repeat
  end tell
  error "no iTerm session on " & target
end run"#;

/// Focus a Terminal.app tab by its tty (argv 1).
const TERMINAL_SCRIPT: &str = r#"on run argv
  set target to item 1 of argv
  tell application "Terminal"
    repeat with w in windows
      repeat with t in tabs of w
        if tty of t is target then
          set selected of t to true
          set index of w to 1
          activate
          return "ok"
        end if
      end repeat
    end repeat
  end tell
  error "no Terminal tab on " & target
end run"#;

/// `client`: the most recently active client of the session's tmux server,
/// used when the TUI is not inside that server.
pub fn jump_plan(loc: &Location, cwd: &str, here: &Here, client: Option<&str>) -> Jump {
    let cmd = |parts: &[&str]| parts.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    // A Claude background session has no terminal: open it with
    // `claude attach`, in a new window of the tmux the TUI runs in.
    if let Some(id) = &loc.claude_background {
        let plain = plain_id(id);
        if !plain {
            return Jump::Explain(
                "It's a Claude background session: open it from `claude agents`.".into(),
            );
        }
        return match &here.tmux_socket {
            Some(sock) => {
                let mut run = cmd(&["tmux", "-S", sock, "new-window"]);
                // tmux expands #{…} and #(…) in -c: only a plain absolute path goes.
                if cwd.starts_with('/') && !cwd.contains('#') {
                    run.extend(cmd(&["-c", cwd]));
                }
                run.extend(cmd(&["claude", "attach", id]));
                Jump::Run(vec![run])
            }
            // On macOS, a new window of the terminal the TUI runs in.
            None => match (here.macos, here.term_program.as_deref()) {
                (true, Some("iTerm.app")) => {
                    Jump::Run(vec![cmd(&["osascript", "-e", ITERM_ATTACH_SCRIPT, id])])
                }
                (true, Some("Apple_Terminal")) => {
                    Jump::Run(vec![cmd(&["osascript", "-e", TERMINAL_ATTACH_SCRIPT, id])])
                }
                _ => Jump::Explain(format!(
                    "It's a Claude background session. Open it with: claude attach {id}"
                )),
            },
        };
    }
    if let (Some(sock), Some(pane)) = (&loc.tmux_socket, &loc.tmux_pane) {
        if here.tmux_socket.as_deref() == Some(sock.as_str()) {
            return Jump::Run(vec![cmd(&[
                "tmux",
                "-S",
                sock,
                "switch-client",
                "-t",
                pane,
            ])]);
        }
        if let Some(client) = client {
            return Jump::Run(vec![cmd(&[
                "tmux",
                "-S",
                sock,
                "switch-client",
                "-c",
                client,
                "-t",
                pane,
            ])]);
        }
        return Jump::Explain(format!(
            "No tmux client is attached to that session. Attach with: tmux -S {} attach -t {pane}",
            shell_quote(sock)
        ));
    }
    let program = loc.term_program.as_deref().unwrap_or_default();
    // Only a device path is handed to osascript (never something option-like).
    let tty = loc.tty.as_ref().filter(|t| t.starts_with("/dev/"));
    if let (true, Some(tty)) = (here.macos, tty) {
        let script = match program {
            "iTerm.app" => Some(ITERM_SCRIPT),
            "Apple_Terminal" => Some(TERMINAL_SCRIPT),
            _ => None,
        };
        if let Some(script) = script {
            return Jump::Run(vec![cmd(&["osascript", "-e", script, tty])]);
        }
    }
    if let ("WezTerm", Some(pane)) = (program, &loc.wezterm_pane) {
        return Jump::Run(vec![cmd(&[
            "wezterm",
            "cli",
            "activate-pane",
            "--pane-id",
            pane,
        ])]);
    }
    Jump::Explain(match &loc.tty {
        Some(tty) => format!("Can't switch to it from here. It runs on {tty} in {cwd}"),
        None => format!("Can't switch to it from here. It runs in {cwd}"),
    })
}

/// Runs the jump for a row and returns a one-line status for the TUI.
pub fn jump(row: &Row) -> String {
    let here = Here::current();
    let loc = &row.location;
    let mut client = None;
    if let (Some(sock), Some(pane)) = (&loc.tmux_socket, &loc.tmux_pane) {
        // `display-message -t` succeeds for missing panes, so list them instead.
        let panes = output(&["tmux", "-S", sock, "list-panes", "-a", "-F", "#{pane_id}"]);
        if !panes.is_some_and(|p| p.lines().any(|l| l == pane)) {
            return format!("That tmux pane ({pane}) is gone.");
        }
        // Same pane id on a restarted server would be someone else's pane.
        if let Some(pid) = &loc.tmux_pid {
            let now = output(&["tmux", "-S", sock, "display-message", "-p", "#{pid}"]);
            if now.as_deref().map(str::trim) != Some(pid.as_str()) {
                return "That tmux server restarted since the session started; its pane is gone."
                    .into();
            }
        }
        if here.tmux_socket.as_deref() != Some(sock.as_str()) {
            client = output(&[
                "tmux",
                "-S",
                sock,
                "list-clients",
                "-F",
                "#{client_activity} #{client_tty}",
            ])
            .and_then(|out| {
                out.lines()
                    .filter_map(|l| l.split_once(' '))
                    .max_by_key(|(at, _)| at.parse::<u64>().unwrap_or(0))
                    .map(|(_, tty)| tty.to_string())
            });
        }
    }
    match jump_plan(loc, &row.cwd, &here, client.as_deref()) {
        Jump::Explain(message) => message,
        Jump::Run(cmds) => {
            for c in &cmds {
                match Command::new(&c[0]).args(&c[1..]).output() {
                    Ok(o) if o.status.success() => {}
                    Ok(o) => {
                        return failure_message(&String::from_utf8_lossy(&o.stderr));
                    }
                    Err(e) => return format!("Couldn't run {}: {e}", c[0]),
                }
            }
            format!("Jumped to {} {}", row.agent.name(), row.project)
        }
    }
}

/// POSIX single-quoting, for commands shown to the user to paste.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn output(cmd: &[&str]) -> Option<String> {
    let o = Command::new(cmd[0]).args(&cmd[1..]).output().ok()?;
    o.status
        .success()
        .then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}
