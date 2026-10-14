//! `tapin init`, `uninstall` and `doctor` for each agent's hook config file
//! (Claude Code's settings.json, Codex CLI's hooks.json: same JSON schema).
//!
//! The JSON edits are pure functions over `serde_json::Value` (key order is
//! preserved); `run_*` add the file IO, diff, confirmation and backup.

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use std::io::{BufRead, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::event::AgentKind;

/// Claude Code hook events tapin listens to.
pub const CLAUDE_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PostToolUseFailure",
    "Stop",
    "SessionEnd",
];

/// Codex CLI hook events tapin listens to (Codex has no PostToolUseFailure).
pub const CODEX_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "Stop",
    "SessionEnd",
];

/// Gemini CLI hook events tapin listens to (its own names; docs/hooks/reference.md).
pub const GEMINI_EVENTS: &[&str] = &[
    "SessionStart",
    "BeforeAgent",
    "BeforeTool",
    "Notification",
    "AfterTool",
    "AfterAgent",
    "SessionEnd",
];

/// Events that are about one tool use, so they accept a tool matcher.
const TOOL_EVENTS: &[&str] = &[
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PostToolUseFailure",
];

/// One agent's hook config file, and how tapin hooks into it.
pub struct Target {
    pub agent: AgentKind,
    /// Human name, e.g. "Claude Code".
    pub title: &'static str,
    pub path: PathBuf,
    pub events: &'static [&'static str],
    /// Matcher for tool events. Claude's docs use "*"; for Codex tapin omits
    /// it, which its docs define as matching every occurrence.
    pub tool_matcher: Option<&'static str>,
    /// The agent looks installed: its config dir exists or it is on PATH.
    pub detected: bool,
    /// Where detection looked, for the "not found" message.
    pub looked_in: String,
}

/// Every agent tapin can hook into, detected or not.
pub fn targets() -> Result<Vec<Target>> {
    let home = || -> Result<PathBuf> {
        Ok(PathBuf::from(
            std::env::var_os("HOME").context("HOME is not set")?,
        ))
    };
    let claude_path = claude_settings_path()?;
    let codex_dir = match std::env::var_os("CODEX_HOME") {
        Some(d) => PathBuf::from(d),
        None => home()?.join(".codex"),
    };
    // Gemini CLI uses $GEMINI_CLI_HOME in place of the home dir (utils/paths.ts).
    let gemini_home = match std::env::var_os("GEMINI_CLI_HOME") {
        Some(d) => PathBuf::from(d),
        None => home()?,
    };
    let make = |agent, title, path: PathBuf, events, tool_matcher, binary: &str| {
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        Target {
            agent,
            title,
            detected: dir.exists() || find_on_path(binary).is_some(),
            looked_in: format!("no {} and no `{binary}` on PATH", dir.display()),
            path,
            events,
            tool_matcher,
        }
    };
    Ok(vec![
        make(
            AgentKind::Claude,
            "Claude Code",
            claude_path,
            CLAUDE_EVENTS,
            Some("*"),
            "claude",
        ),
        make(
            AgentKind::Codex,
            "Codex CLI",
            codex_dir.join("hooks.json"),
            CODEX_EVENTS,
            None,
            "codex",
        ),
        make(
            AgentKind::Gemini,
            "Gemini CLI",
            gemini_home.join(".gemini").join("settings.json"),
            GEMINI_EVENTS,
            None,
            "gemini",
        ),
    ])
}

/// Bare allow rules for these tools are harmless: they never need approval anyway.
const READ_ONLY_TOOLS: &[&str] = &["Read", "Glob", "Grep", "LS", "WebSearch", "TodoWrite"];

/// `$CLAUDE_CONFIG_DIR/settings.json`, else `~/.claude/settings.json`.
pub fn claude_settings_path() -> Result<PathBuf> {
    let dir = match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(d) => PathBuf::from(d),
        None => PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".claude"),
    };
    Ok(dir.join("settings.json"))
}

/// The hook command, with tapin's absolute path: agents launched from an IDE or
/// GUI may not have `~/.cargo/bin` on PATH.
pub fn hook_command(exe: &Path, agent: AgentKind) -> String {
    // Single quotes: the shell expands nothing inside them; ' itself is spliced as '\''.
    let quoted = exe.display().to_string().replace('\'', "'\\''");
    format!("'{quoted}' hook --agent {}", agent.name())
}

/// True for a hook command that `install` wrote, wherever that tapin lives:
/// the program is a file named `tapin` and the arguments are tapin's.
pub fn is_tapin_hook(command: &str) -> bool {
    let command = command.trim();
    let program = first_shell_word(command);
    Path::new(&program)
        .file_name()
        .is_some_and(|n| n == "tapin")
        && [AgentKind::Claude, AgentKind::Codex, AgentKind::Gemini]
            .iter()
            .any(|a| command.ends_with(&format!(" hook --agent {}", a.name())))
}

/// The first word of a shell command line, with quoting and escapes removed
/// (enough of POSIX sh for hook commands: '…', "…", and backslash escapes).
fn first_shell_word(command: &str) -> String {
    let mut word = String::new();
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' if !word.is_empty() => break,
            ' ' | '\t' => {}
            '\'' => word.extend(chars.by_ref().take_while(|&c| c != '\'')),
            '"' => {
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => word.extend(chars.next()),
                        c => word.push(c),
                    }
                }
            }
            '\\' => word.extend(chars.next()),
            c => word.push(c),
        }
    }
    word
}

/// The path hooks should run. Prefers the `tapin` found on PATH when it is this
/// same binary, so hooks survive upgrades that move the real file (e.g. a
/// Homebrew Cellar path that changes on every `brew upgrade`).
pub fn stable_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("locating the tapin binary")?;
    let real = std::fs::canonicalize(&exe).unwrap_or(exe.clone());
    let on_path = all_on_path("tapin")
        .into_iter()
        .find(|p| std::fs::canonicalize(p).ok() == Some(real.clone()));
    Ok(on_path.unwrap_or(exe))
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    all_on_path(name).into_iter().next()
}

fn all_on_path(name: &str) -> Vec<PathBuf> {
    let Some(paths) = std::env::var_os("PATH") else {
        return vec![];
    };
    std::env::split_paths(&paths)
        .map(|d| d.join(name))
        .filter(|p| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
        .collect()
}

fn commands(group: &Value) -> impl Iterator<Item = &str> {
    group["hooks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| h["command"].as_str())
}

/// Adds tapin's hook to every event that lacks one. Returns whether anything changed.
pub fn install(settings: &mut Value, command: &str, target: &Target) -> Result<bool> {
    let root = settings
        .as_object_mut()
        .context("settings.json is not a JSON object")?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .context("\"hooks\" in settings.json is not an object")?;
    let mut changed = false;
    for &event in target.events {
        let groups = hooks
            .entry(event)
            .or_insert_with(|| Value::Array(vec![]))
            .as_array_mut()
            .with_context(|| format!("hooks.{event} is not an array"))?;
        let mut found = false;
        for hook in groups
            .iter_mut()
            .filter_map(|g| g.get_mut("hooks").and_then(Value::as_array_mut))
            .flatten()
        {
            if hook["command"].as_str().is_some_and(is_tapin_hook) {
                found = true;
                // Re-point a hook left behind by a tapin that moved.
                if hook["command"] != command {
                    hook["command"] = command.into();
                    changed = true;
                }
            }
        }
        if found {
            continue;
        }
        let hook = json!({ "type": "command", "command": command });
        groups.push(match target.tool_matcher {
            Some(m) if TOOL_EVENTS.contains(&event) => json!({ "matcher": m, "hooks": [hook] }),
            _ => json!({ "hooks": [hook] }),
        });
        changed = true;
    }
    Ok(changed)
}

/// Removes exactly tapin's hooks, plus any group, event list or `hooks` object
/// that only existed to hold them. Returns whether anything changed.
pub fn uninstall(settings: &mut Value) -> bool {
    let Some(root) = settings.as_object_mut() else {
        return false;
    };
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return false;
    };
    let mut removed = 0;
    let mut emptied_events = Vec::new();
    for (event, groups) in hooks.iter_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        let groups_before = groups.len();
        groups.retain_mut(|group| {
            let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let n = list.len();
            list.retain(|h| !h["command"].as_str().is_some_and(is_tapin_hook));
            removed += n - list.len();
            // Drop a group only if removing tapin's hook is what emptied it.
            list.len() == n || !list.is_empty()
        });
        if groups.is_empty() && groups_before > 0 {
            emptied_events.push(event.clone());
        }
    }
    for event in emptied_events {
        hooks.remove(&event);
    }
    if removed > 0 && hooks.is_empty() {
        root.remove("hooks");
    }
    removed > 0
}

/// What `doctor` found in a settings file.
#[derive(Debug, Default)]
pub struct Diagnosis {
    pub missing_events: Vec<&'static str>,
    pub other_permission_hooks: Vec<String>,
    pub bypass_mode: bool,
    pub broad_allow: Vec<String>,
    /// Gemini's `hooksConfig.enabled` is false: no hook runs at all.
    pub hooks_disabled: bool,
    /// tapin hook commands that run some other tapin than `command` (e.g. it moved).
    pub stale_hooks: Vec<String>,
}

/// Inspects `settings`; `command` is the hook command this tapin would install.
pub fn diagnose(settings: &Value, command: &str, events: &'static [&'static str]) -> Diagnosis {
    let hooks = &settings["hooks"];
    let missing_events = events
        .iter()
        .copied()
        .filter(|e| {
            !hooks[*e]
                .as_array()
                .is_some_and(|gs| gs.iter().any(|g| commands(g).any(is_tapin_hook)))
        })
        .collect();
    let other_permission_hooks = hooks["PermissionRequest"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|g| commands(g).map(String::from).collect::<Vec<_>>())
        .filter(|c| !is_tapin_hook(c))
        .collect();
    let perms = &settings["permissions"];
    let broad_allow = perms["allow"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|r| {
            let tool = r.strip_suffix("(*)").unwrap_or(r);
            !tool.contains('(') && !READ_ONLY_TOOLS.contains(&tool)
        })
        .map(String::from)
        .collect();
    let mut stale_hooks: Vec<String> = hooks
        .as_object()
        .into_iter()
        .flat_map(|m| m.values())
        .filter_map(Value::as_array)
        .flatten()
        .flat_map(|g| commands(g).map(String::from).collect::<Vec<_>>())
        .filter(|c| is_tapin_hook(c) && c != command)
        .collect();
    stale_hooks.sort();
    stale_hooks.dedup();
    Diagnosis {
        hooks_disabled: settings["hooksConfig"]["enabled"] == false,
        stale_hooks,
        missing_events,
        other_permission_hooks,
        bypass_mode: perms["defaultMode"] == "bypassPermissions",
        broad_allow,
    }
}

/// Line or block comments outside strings (JSONC), which serde_json rejects.
fn has_comments(text: &str) -> bool {
    let mut in_string = false;
    let mut escaped = false;
    let mut prev = ' ';
    for c in text.chars() {
        if in_string {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_string = false,
                _ => {}
            }
        } else if c == '"' {
            in_string = true;
        } else if prev == '/' && (c == '/' || c == '*') {
            return true;
        }
        prev = c;
    }
    false
}

/// A settings file as found on disk, kept so writes can match its style.
struct SettingsFile {
    /// Where the bytes live: a symlinked settings.json is resolved, so writes
    /// update the target (e.g. a dotfiles repo) instead of replacing the link.
    path: PathBuf,
    /// Where backups go: beside the settings path the agent uses, not beside a
    /// symlink's target, which may be a git-tracked dotfiles repo.
    backup_dir: PathBuf,
    original: Option<String>,
    value: Value,
}

impl SettingsFile {
    fn load(link: &Path) -> Result<SettingsFile> {
        let is_link = std::fs::symlink_metadata(link).is_ok_and(|m| m.file_type().is_symlink());
        let path = &match std::fs::canonicalize(link) {
            Ok(real) => real,
            Err(_) if is_link => anyhow::bail!(
                "{} is a broken symlink (to {}); fix or remove it first",
                link.display(),
                std::fs::read_link(link)
                    .map(|t| t.display().to_string())
                    .unwrap_or_default()
            ),
            Err(_) => link.to_path_buf(),
        };
        let backup_dir = link
            .parent()
            .context("settings path has no parent")?
            .to_path_buf();
        let original = match std::fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let value = match &original {
            Some(text) if !text.trim().is_empty() => {
                serde_json::from_str(text).with_context(|| {
                    if has_comments(text) {
                        format!(
                            "{} has comments; tapin only edits plain JSON, so it won't touch it",
                            path.display()
                        )
                    } else {
                        format!("{} is not valid JSON; not touching it", path.display())
                    }
                })?
            }
            _ => Value::Object(Map::new()),
        };
        Ok(SettingsFile {
            backup_dir,
            path: path.to_path_buf(),
            original,
            value,
        })
    }

    /// The value serialized in the original file's indentation and trailing newline.
    fn render(&self, value: &Value) -> Result<String> {
        let original = self.original.as_deref().unwrap_or("");
        // Reuse the file's own indent unit (spaces or tabs), from its first indented line.
        let indent = original
            .lines()
            .skip(1)
            .map(|l| &l[..l.len() - l.trim_start().len()])
            .find(|ws| !ws.is_empty())
            .unwrap_or("  ")
            .to_string();
        let mut out = Vec::new();
        let fmt = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
        let mut ser = serde_json::Serializer::with_formatter(&mut out, fmt);
        serde::Serialize::serialize(value, &mut ser)?;
        let mut text = String::from_utf8(out)?;
        if self.original.is_none() || original.ends_with('\n') {
            text.push('\n');
        }
        Ok(text)
    }

    /// Backs up the old file (0600, never overwriting an older backup), then
    /// atomically replaces it, keeping its permissions. Aborts if the file
    /// changed since it was read (e.g. Claude Code saved a setting meanwhile).
    fn write(&self, text: &str) -> Result<()> {
        let dir = self.path.parent().context("settings path has no parent")?;
        std::fs::create_dir_all(dir)?;
        let current = std::fs::read_to_string(&self.path).ok();
        anyhow::ensure!(
            current == self.original,
            "{} changed while you were deciding; nothing was written, run the command again",
            self.path.display()
        );
        let mode = std::fs::metadata(&self.path)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0o600);
        if let Some(original) = &self.original {
            let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
            let name = self
                .path
                .file_name()
                .context("settings path has no file name")?;
            let backup = self
                .backup_dir
                .join(format!("{}.tapin-backup-{stamp}", name.to_string_lossy()));
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&backup)
                .with_context(|| format!("creating backup {}", backup.display()))?;
            f.write_all(original.as_bytes())?;
            println!("Backed up the old file to {}", backup.display());
        }
        let tmp = self.path.with_extension("json.tapin-tmp");
        let _ = std::fs::remove_file(&tmp); // left over from an interrupted run
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&tmp)?;
        // create_new's mode is masked by umask; set it exactly.
        f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// Shows the change, asks unless `yes`, then writes. Returns whether it wrote.
fn apply(file: &SettingsFile, updated: &Value, yes: bool) -> Result<bool> {
    // Diff against the bytes on disk, so any reformatting is visible too.
    let before = file.original.clone().unwrap_or_default();
    let after = file.render(updated)?;
    let shown = file.path.display().to_string();
    let diff = similar::TextDiff::from_lines(&before, &after);
    print!(
        "{}",
        diff.unified_diff().context_radius(2).header(&shown, &shown)
    );
    if !yes && !confirm(&format!("\nApply these changes to {shown}? [y/N] "))? {
        println!("Nothing changed.");
        return Ok(false);
    }
    file.write(&after)?;
    Ok(true)
}

/// Asks on the terminal itself, never stdin: under `curl ... | sh` stdin is the
/// script. With no terminal, refuses and points at `--yes`.
fn confirm(question: &str) -> Result<bool> {
    let tty = if std::env::var_os("TAPIN_NO_TTY").is_some() {
        None
    } else {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .ok()
    };
    let Some(mut tty) = tty else {
        anyhow::bail!("no terminal to confirm on; re-run with --yes to apply the change above");
    };
    tty.write_all(question.as_bytes())?;
    let mut answer = String::new();
    std::io::BufReader::new(tty).read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// Runs `step` for each agent; one agent's broken file never stops the others,
/// but any failure is reported and fails the command at the end.
fn for_each_target(mut step: impl FnMut(&Target) -> Result<()>) -> Result<()> {
    let mut failed = Vec::new();
    for target in targets()? {
        if let Err(e) = step(&target) {
            eprintln!("{}: {e:#}", target.title);
            failed.push(target.title);
        }
    }
    anyhow::ensure!(failed.is_empty(), "not done for: {}", failed.join(", "));
    Ok(())
}

pub fn run_init(yes: bool) -> Result<()> {
    let exe = stable_exe()?;
    let result = for_each_target(|target| {
        if !target.detected {
            println!(
                "{} not found ({}); skipping.",
                target.title, target.looked_in
            );
            return Ok(());
        }
        println!("== {} ({})", target.title, target.path.display());
        let file = match SettingsFile::load(&target.path) {
            Ok(f) => f,
            Err(e) => {
                // Commented settings (Gemini allows them): show what to add by hand.
                if format!("{e:#}").contains("has comments") {
                    let mut hooks = Value::Object(Map::new());
                    install(&mut hooks, &hook_command(&exe, target.agent), target)?;
                    println!(
                        "Add these hooks to {} yourself:\n{}",
                        target.path.display(),
                        serde_json::to_string_pretty(&hooks)?
                    );
                }
                return Err(e);
            }
        };
        let mut updated = file.value.clone();
        if !install(&mut updated, &hook_command(&exe, target.agent), target)? {
            println!("tapin hooks are already installed.");
        } else if apply(&file, &updated, yes)? {
            println!("Installed. New {} sessions report to tapin.", target.title);
            if target.agent == AgentKind::Codex {
                println!("{CODEX_TRUST}");
            }
        }
        Ok(())
    });
    println!("Run `tapin` to open the inbox.");
    result
}

/// Codex skips new or changed hooks until the user reviews them. tapin never
/// writes trust itself: that would bypass Codex's own safety check.
const CODEX_TRUST: &str = "Codex runs new hooks only after you trust them: start `codex` and use /hooks to review and trust tapin's.";

pub fn run_uninstall(yes: bool) -> Result<()> {
    for_each_target(|target| {
        if !target.path.exists() {
            return Ok(());
        }
        let file = SettingsFile::load(&target.path)?;
        let mut updated = file.value.clone();
        if !uninstall(&mut updated) {
            println!("No tapin hooks in {}.", target.path.display());
            return Ok(());
        }
        println!("== {} ({})", target.title, target.path.display());
        if apply(&file, &updated, yes)? {
            println!("Removed tapin's hooks.");
        }
        Ok(())
    })
}

pub fn run_doctor(socket: &Path) -> Result<()> {
    let exe_path = stable_exe()?;
    println!("tapin doctor");
    println!("  binary    {}", exe_path.display());
    let (_, config_error) = crate::config::Config::load_checked();
    match (&config_error, crate::config::Config::path()) {
        (Some(e), _) => println!("  config    ERROR: {e}"),
        (None, Some(p)) if p.exists() => println!("  config    {}", p.display()),
        _ => println!("  config    none (defaults)"),
    }
    let running = std::os::unix::net::UnixStream::connect(socket).is_ok();
    println!(
        "  daemon    {} ({})",
        if running {
            "running"
        } else {
            "not running, starts when you open `tapin`"
        },
        socket.display()
    );

    for target in targets()? {
        println!("\n{} ({})", target.title, target.agent.name());
        if !target.detected {
            println!("  not found ({})", target.looked_in);
            continue;
        }
        let file = match SettingsFile::load(&target.path) {
            Ok(f) => f,
            Err(e) => {
                println!("  ERROR     {e:#}");
                continue;
            }
        };
        let d = diagnose(
            &file.value,
            &hook_command(&exe_path, target.agent),
            target.events,
        );
        println!("  settings  {}", target.path.display());
        let total = target.events.len();
        match d.missing_events.len() {
            0 => println!("  hooks     installed ({total}/{total} events)"),
            n if n == total => println!("  hooks     not installed: run `tapin init`"),
            _ => println!(
                "  hooks     partly installed, missing {}: run `tapin init`",
                d.missing_events.join(", ")
            ),
        }
        let mut warnings = Vec::new();
        if target.agent == AgentKind::Codex && d.missing_events.len() < total {
            warnings.push(CODEX_TRUST.to_string());
        }
        if target.agent == AgentKind::Gemini {
            warnings.push(
                "Gemini runs hooks only in trusted folders (folders are untrusted by default); trust your project folders in Gemini"
                    .to_string(),
            );
            if d.hooks_disabled {
                warnings.push(
                    "hooksConfig.enabled is false in Gemini's settings: no hooks run".to_string(),
                );
            }
        }
        for stale in &d.stale_hooks {
            warnings.push(format!(
                "A hook runs a different tapin: {stale}\n    run `tapin init` to point it here"
            ));
        }
        for other in &d.other_permission_hooks {
            warnings.push(format!(
                "Another PermissionRequest hook may answer before tapin does: {other}"
            ));
        }
        if d.bypass_mode {
            warnings.push(
                "permissions.defaultMode is bypassPermissions: Claude never asks, so no approvals reach tapin"
                    .to_string(),
            );
        }
        if !d.broad_allow.is_empty() {
            warnings.push(format!(
                "Allow rules skip approval entirely for: {}",
                d.broad_allow.join(", ")
            ));
        }
        if !warnings.is_empty() {
            println!("  Why approvals might not show up:");
            for w in warnings {
                println!("  ! {w}");
            }
        }
    }
    Ok(())
}
