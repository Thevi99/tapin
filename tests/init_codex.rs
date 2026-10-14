//! `tapin init` / `uninstall` / `doctor` for Codex CLI, against a temp CODEX_HOME.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    /// A Codex home holding `hooks.json` with another tool's hooks, and a config.toml.
    fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("codex");
        std::fs::create_dir(&home).unwrap();
        let other = serde_json::json!({"hooks": {"PermissionRequest": [{"hooks": [
            {"type": "command", "command": "other-island --source codex", "timeout": 7200}
        ]}]}});
        std::fs::write(
            home.join("hooks.json"),
            serde_json::to_string_pretty(&other).unwrap() + "\n",
        )
        .unwrap();
        std::fs::write(home.join("config.toml"), "model = \"gpt\"\n").unwrap();
        Env { dir }
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("codex")
    }

    fn hooks(&self) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(self.home().join("hooks.json")).unwrap())
            .unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tapin"))
            .args(args)
            .env("CODEX_HOME", self.home())
            .env("CLAUDE_CONFIG_DIR", self.dir.path().join("no-claude"))
            .env("PATH", self.dir.path().join("empty-path"))
            .env("TAPIN_NO_TTY", "1")
            .output()
            .unwrap()
    }
}

fn tapin_commands(hooks: &serde_json::Value, event: &str) -> Vec<String> {
    hooks["hooks"][event]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|g| g["hooks"].as_array().cloned().unwrap_or_default())
        .filter_map(|h| h["command"].as_str().map(String::from))
        .filter(|c| c.ends_with("hook --agent codex"))
        .collect()
}

fn out(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn init_hooks_codex_and_keeps_other_hooks() {
    let env = Env::new();
    let before = env.hooks();
    let o = env.run(&["init", "--yes"]);
    assert!(o.status.success(), "{}", out(&o));
    let after = env.hooks();
    for ev in [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PermissionRequest",
        "PostToolUse",
        "Stop",
        "SessionEnd",
    ] {
        assert_eq!(tapin_commands(&after, ev).len(), 1, "{ev}");
    }
    assert_eq!(
        after["hooks"]["PermissionRequest"][0],
        before["hooks"]["PermissionRequest"][0]
    );
    // Codex matchers are regexes; tapin's groups match everything by omitting one.
    assert!(after["hooks"]["PreToolUse"][0].get("matcher").is_none());
}

#[test]
fn init_asks_the_user_to_trust_hooks_in_codex_and_never_trusts_them_itself() {
    let env = Env::new();
    let o = env.run(&["init", "--yes"]);
    assert!(out(&o).contains("/hooks"), "{}", out(&o));
    let config = std::fs::read_to_string(env.home().join("config.toml")).unwrap();
    assert_eq!(
        config, "model = \"gpt\"\n",
        "config.toml (trust state) untouched"
    );
}

#[test]
fn uninstall_restores_codex_hooks_exactly() {
    let env = Env::new();
    let original = std::fs::read_to_string(env.home().join("hooks.json")).unwrap();
    assert!(env.run(&["init", "--yes"]).status.success());
    assert!(env.run(&["uninstall", "--yes"]).status.success());
    assert_eq!(
        std::fs::read_to_string(env.home().join("hooks.json")).unwrap(),
        original
    );
}

#[test]
fn doctor_covers_codex() {
    let env = Env::new();
    assert!(env.run(&["init", "--yes"]).status.success());
    let o = out(&env.run(&["doctor"]));
    assert!(o.contains("codex") && o.contains("installed (7/7"), "{o}");
    assert!(o.contains("/hooks"), "trust reminder: {o}");
    assert!(
        o.contains("other-island"),
        "competing PermissionRequest hook: {o}"
    );
}

#[test]
fn agents_that_are_not_installed_are_skipped() {
    let env = Env::new();
    std::fs::remove_dir_all(env.home()).unwrap();
    let o = env.run(&["init", "--yes"]);
    assert!(out(&o).contains("not found"), "{}", out(&o));
    assert!(!env.home().exists());
    assert!(!Path::new(&env.dir.path().join("no-claude")).exists());
}

#[test]
fn a_broken_claude_settings_file_does_not_stop_codex_setup() {
    let env = Env::new();
    let claude = env.dir.path().join("claude");
    std::fs::create_dir(&claude).unwrap();
    std::fs::write(claude.join("settings.json"), "{ broken").unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_tapin"))
        .args(["init", "--yes"])
        .env("CODEX_HOME", env.home())
        .env("CLAUDE_CONFIG_DIR", &claude)
        .env("PATH", env.dir.path().join("empty-path"))
        .env("TAPIN_NO_TTY", "1")
        .output()
        .unwrap();
    let text = out(&o);
    assert!(text.contains("not valid JSON"), "{text}");
    assert_eq!(
        tapin_commands(&env.hooks(), "Stop").len(),
        1,
        "Codex still set up: {text}"
    );
    assert!(
        !o.status.success(),
        "the Claude failure is still reported in the exit code"
    );
    assert_eq!(
        std::fs::read_to_string(claude.join("settings.json")).unwrap(),
        "{ broken"
    );
}
