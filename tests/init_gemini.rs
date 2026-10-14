//! `tapin init` / `uninstall` / `doctor` for Gemini CLI (~/.gemini/settings.json
//! under $GEMINI_CLI_HOME), against a temp home.

use std::path::PathBuf;
use std::process::{Command, Output};

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new(settings: serde_json::Value) -> Env {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("home/.gemini")).unwrap();
        let env = Env { dir };
        std::fs::write(
            env.settings(),
            serde_json::to_string_pretty(&settings).unwrap() + "\n",
        )
        .unwrap();
        env
    }

    fn settings(&self) -> PathBuf {
        self.dir.path().join("home/.gemini/settings.json")
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(self.settings()).unwrap()).unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tapin"))
            .args(args)
            .env("GEMINI_CLI_HOME", self.dir.path().join("home"))
            .env("CLAUDE_CONFIG_DIR", self.dir.path().join("no-claude"))
            .env("CODEX_HOME", self.dir.path().join("no-codex"))
            .env("PATH", self.dir.path().join("empty-path"))
            .env("TAPIN_NO_TTY", "1")
            .output()
            .unwrap()
    }
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

const EVENTS: &[&str] = &[
    "SessionStart",
    "BeforeAgent",
    "BeforeTool",
    "Notification",
    "AfterTool",
    "AfterAgent",
    "SessionEnd",
];

#[test]
fn init_hooks_gemini_and_keeps_its_settings() {
    let env = Env::new(serde_json::json!({"theme": "Dracula", "model": {"name": "gemini-3-pro"}}));
    let o = env.run(&["init", "--yes"]);
    assert!(o.status.success(), "{}", text(&o));
    let s = env.json();
    assert_eq!(s["theme"], "Dracula");
    for ev in EVENTS {
        let cmds: Vec<&str> = s["hooks"][ev]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|g| {
                g["hooks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|h| h["command"].as_str())
            })
            .collect();
        assert!(
            cmds.iter().any(|c| c.ends_with("hook --agent gemini")),
            "{ev}: {cmds:?}"
        );
    }
}

#[test]
fn uninstall_restores_gemini_settings_exactly() {
    let env = Env::new(serde_json::json!({"theme": "Dracula"}));
    let original = std::fs::read_to_string(env.settings()).unwrap();
    assert!(env.run(&["init", "--yes"]).status.success());
    assert!(env.run(&["uninstall", "--yes"]).status.success());
    assert_eq!(std::fs::read_to_string(env.settings()).unwrap(), original);
}

#[test]
fn doctor_explains_gemini_folder_trust_and_disabled_hooks() {
    let env = Env::new(serde_json::json!({"hooksConfig": {"enabled": false}}));
    assert!(env.run(&["init", "--yes"]).status.success());
    let out = text(&env.run(&["doctor"]));
    assert!(
        out.contains("gemini") && out.contains("installed (7/7"),
        "{out}"
    );
    assert!(out.contains("trusted"), "folder trust note: {out}");
    assert!(
        out.contains("hooksConfig.enabled"),
        "disabled hooks warning: {out}"
    );
}

#[test]
fn a_settings_file_with_comments_is_explained_not_called_invalid() {
    let env = Env::new(serde_json::json!({}));
    let commented = "{\n  // my theme\n  \"theme\": \"Dracula\"\n}\n";
    std::fs::write(env.settings(), commented).unwrap();
    let o = env.run(&["init", "--yes"]);
    let out = text(&o);
    assert!(out.contains("comments"), "{out}");
    assert!(
        out.contains("hook --agent gemini"),
        "shows what to add by hand: {out}"
    );
    assert_eq!(
        std::fs::read_to_string(env.settings()).unwrap(),
        commented,
        "untouched"
    );
}
