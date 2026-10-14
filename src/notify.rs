//! Desktop notifications, fired by the daemon on state transitions.

use crate::config::{Config, NotifyOn};
use std::process::{Command, Stdio};

pub struct Notifier {
    on: Vec<NotifyOn>,
    /// notify-send bodies may be read as markup by the notification server.
    markup: bool,
    /// Program and leading args; the title and body are appended as two more args.
    command: Option<Vec<String>>,
}

impl Notifier {
    pub fn new(config: &Config, enabled: bool) -> Notifier {
        let command = (enabled && config.notify).then(platform_command).flatten();
        let markup = command.as_ref().is_some_and(|c| c[0] == "notify-send");
        Notifier {
            on: config.notify_on.clone(),
            markup,
            command,
        }
    }

    /// Shows a notification without blocking the caller.
    pub fn send(&self, kind: NotifyOn, title: &str, body: &str) {
        let Some(command) = &self.command else { return };
        if !self.on.contains(&kind) {
            return;
        }
        let (title, body) = if self.markup {
            (escape_markup(title), escape_markup(body))
        } else {
            (title.to_string(), body.to_string())
        };
        let child = Command::new(&command[0])
            .args(&command[1..])
            .args([title, body]) // as arguments, never spliced into a script
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            std::thread::spawn(move || child.wait()); // reap
        }
    }
}

/// `TAPIN_NOTIFY_CMD` (used by tests), else osascript on macOS, else notify-send.
fn platform_command() -> Option<Vec<String>> {
    if let Some(cmd) = std::env::var_os("TAPIN_NOTIFY_CMD") {
        return Some(vec![cmd.to_string_lossy().into_owned()]);
    }
    if cfg!(target_os = "macos") {
        let script = "on run argv\ndisplay notification (item 2 of argv) with title (item 1 of argv)\nend run";
        return Some(vec!["osascript".into(), "-e".into(), script.into()]);
    }
    let found = std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("notify-send").is_file()));
    // `--` so nothing in the text is ever read as an option.
    found.then(|| vec!["notify-send".into(), "--".into()])
}

fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
