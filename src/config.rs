//! Optional `~/.config/tapin/config.toml` (or `$XDG_CONFIG_HOME/tapin/`).
//! Every setting has a default; tapin runs fine without the file.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyOn {
    /// A session is waiting on a permission answer.
    Needs,
    /// A session finished its turn.
    Done,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Desktop notifications (macOS Notification Center, Linux notify-send).
    pub notify: bool,
    /// Which transitions notify.
    pub notify_on: Vec<NotifyOn>,
    /// Ring the terminal bell in the TUI when a new request arrives (works over SSH).
    pub bell: bool,
    /// How long a permission hook waits for an answer from tapin.
    pub approval_timeout_secs: u64,
    /// The same for Codex, which shows no prompt of its own while the hook waits.
    pub codex_approval_timeout_secs: u64,
    /// How often passive mode looks for sessions without hooks.
    pub passive_poll_secs: u64,
    /// Sessions running in these folders (or below) are left out of the inbox.
    /// `~` means your home. By default: claude-mem's background observer.
    pub hide_dirs: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            notify: true,
            notify_on: vec![NotifyOn::Needs, NotifyOn::Done],
            bell: false,
            approval_timeout_secs: 300,
            codex_approval_timeout_secs: 30,
            passive_poll_secs: 2,
            hide_dirs: vec!["~/.claude-mem".to_string()],
        }
    }
}

impl Config {
    pub fn parse(text: &str) -> Result<Config> {
        Ok(toml::from_str(text)?)
    }

    /// `hide_dirs` as absolute paths: `~` and `~/…` mean your home; empty,
    /// relative and `~user` entries are ignored. A folder reached through a
    /// symlink is listed by its real path too, since agents report real cwds
    /// (resolved once at daemon start, so a folder created later matches only as written).
    pub fn hidden_dirs(&self) -> Vec<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut dirs = Vec::new();
        for d in &self.hide_dirs {
            let path = match (d.as_str(), &home) {
                ("~", Some(home)) => home.clone(),
                (d, Some(home)) if d.starts_with("~/") => home.join(&d[2..]),
                (d, _) => PathBuf::from(d),
            };
            if !path.is_absolute() {
                continue;
            }
            if let Ok(real) = path.canonicalize() {
                if real != path {
                    dirs.push(real);
                }
            }
            dirs.push(path);
        }
        dirs
    }

    pub fn path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("tapin").join("config.toml"))
    }

    /// The config file if there is one. A broken file never stops the inbox or
    /// a hook: it falls back to defaults, and the error is returned so the TUI
    /// and `doctor` can show it instead of failing silently.
    pub fn load_checked() -> (Config, Option<String>) {
        let Some(path) = Config::path() else {
            return (Config::default(), None);
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return (Config::default(), None);
        };
        match Config::parse(&text).with_context(|| format!("{} is ignored", path.display())) {
            Ok(c) => (c, None),
            Err(e) => (Config::default(), Some(format!("{e:#}"))),
        }
    }

    pub fn load() -> Config {
        Config::load_checked().0
    }
}
