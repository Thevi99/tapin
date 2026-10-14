//! `tapin init` / `uninstall` / `doctor` against a temp Claude config dir.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new(fixture: Option<&str>) -> Env {
        let dir = tempfile::tempdir().unwrap();
        if let Some(f) = fixture {
            let src = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/settings")
                .join(f);
            std::fs::copy(src, dir.path().join("settings.json")).unwrap();
        }
        Env { dir }
    }

    fn settings(&self) -> PathBuf {
        self.dir.path().join("settings.json")
    }

    fn read(&self) -> String {
        std::fs::read_to_string(self.settings()).unwrap()
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.read()).unwrap()
    }

    /// Runs `tapin <args>` with `stdin` piped in (no TTY).
    fn run(&self, args: &[&str], stdin: &str) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tapin"))
            .args(args)
            .env("CLAUDE_CONFIG_DIR", self.dir.path())
            // Never let a test find the developer's real Codex install.
            .env("CODEX_HOME", self.dir.path().join("no-codex"))
            .env("PATH", self.dir.path().join("empty-path"))
            .env("NO_COLOR", "1")
            .env("TAPIN_NO_TTY", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn backups(&self) -> Vec<PathBuf> {
        std::fs::read_dir(self.dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains("tapin-backup")
            })
            .collect()
    }
}

const EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PostToolUseFailure",
    "Stop",
    "SessionEnd",
];

fn tapin_commands(settings: &serde_json::Value, event: &str) -> Vec<String> {
    settings["hooks"][event]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|g| g["hooks"].as_array().cloned().unwrap_or_default())
        .filter_map(|h| h["command"].as_str().map(String::from))
        .filter(|c| c.contains("hook --agent claude"))
        .collect()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn init_hooks_every_event_into_a_fresh_config() {
    let env = Env::new(None);
    let out = env.run(&["init", "--yes"], "");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s = env.json();
    for ev in EVENTS {
        let cmds = tapin_commands(&s, ev);
        assert_eq!(cmds.len(), 1, "{ev}: {cmds:?}");
        assert!(
            cmds[0].contains(env!("CARGO_BIN_EXE_tapin")),
            "absolute path: {}",
            cmds[0]
        );
    }
}

#[test]
fn init_keeps_everything_else_untouched() {
    let env = Env::new(Some("with-other-hooks.json"));
    let before = env.json();
    assert!(env.run(&["init", "--yes"], "").status.success());
    let after = env.json();
    for key in ["model", "permissions", "statusLine"] {
        assert_eq!(after[key], before[key], "{key}");
    }
    let other = &after["hooks"]["PermissionRequest"][0];
    assert_eq!(
        other, &before["hooks"]["PermissionRequest"][0],
        "other tool's hook kept first"
    );
    assert_eq!(after["hooks"]["Stop"][0], before["hooks"]["Stop"][0]);
    // Key order of the original file is preserved.
    let text = env.read();
    let pos = |k: &str| text.find(&format!("\"{k}\"")).unwrap();
    assert!(
        pos("model") < pos("permissions")
            && pos("permissions") < pos("hooks")
            && pos("hooks") < pos("statusLine")
    );
}

#[test]
fn init_backs_up_first_and_is_idempotent() {
    let env = Env::new(Some("with-other-hooks.json"));
    let original = env.read();
    assert!(env.run(&["init", "--yes"], "").status.success());
    let backups = env.backups();
    assert_eq!(backups.len(), 1);
    assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), original);

    let installed = env.read();
    let again = env.run(&["init", "--yes"], "");
    assert!(again.status.success());
    assert!(stdout(&again).contains("already"), "{}", stdout(&again));
    assert_eq!(env.read(), installed);
    assert_eq!(env.backups().len(), 1, "no backup when nothing changes");
}

#[test]
fn without_a_terminal_init_shows_the_diff_but_needs_yes() {
    // e.g. `curl ... | sh`: stdin is the script, so it must not be read as an answer.
    let env = Env::new(Some("with-other-hooks.json"));
    let original = env.read();
    let out = env.run(&["init"], "y\n");
    assert!(!out.status.success());
    assert!(stdout(&out).contains("+") && stdout(&out).contains("hook --agent claude"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--yes"));
    assert_eq!(env.read(), original);
    assert!(env.backups().is_empty());
}

#[test]
fn uninstall_restores_the_original_exactly() {
    let env = Env::new(Some("with-other-hooks.json"));
    let original = env.read();
    assert!(env.run(&["init", "--yes"], "").status.success());
    assert!(env.run(&["uninstall", "--yes"], "").status.success());
    assert_eq!(env.read(), original);
}

#[test]
fn uninstall_after_init_on_a_fresh_config_leaves_no_hooks_key() {
    let env = Env::new(None);
    assert!(env.run(&["init", "--yes"], "").status.success());
    assert!(env.run(&["uninstall", "--yes"], "").status.success());
    assert!(env.json().get("hooks").is_none(), "{}", env.read());
}

#[test]
fn doctor_reports_hooks_and_warns_about_things_that_hide_approvals() {
    let env = Env::new(Some("with-other-hooks.json"));
    let before = stdout(&env.run(&["doctor"], ""));
    assert!(before.contains("not installed"), "{before}");
    assert!(env.run(&["init", "--yes"], "").status.success());

    // Things that make approvals silently never reach tapin.
    let mut s = env.json();
    s["permissions"]["defaultMode"] = "bypassPermissions".into();
    s["permissions"]["allow"] = serde_json::json!(["Bash", "Write"]);
    std::fs::write(env.settings(), serde_json::to_string_pretty(&s).unwrap()).unwrap();

    let after = stdout(&env.run(&["doctor"], ""));
    assert!(after.contains("installed (8/8"), "{after}");
    assert!(
        after.contains("other-notifier"),
        "names the competing PermissionRequest hook:\n{after}"
    );
    assert!(after.contains("bypassPermissions"), "{after}");
    assert!(
        after.contains("Bash") && after.contains("Write"),
        "names the broad allow rules:\n{after}"
    );
}

#[test]
fn init_never_touches_a_settings_file_it_cannot_parse() {
    let env = Env::new(None);
    std::fs::write(env.settings(), "{ \"model\": \"opus\", oops }").unwrap();
    let out = env.run(&["init", "--yes"], "");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not valid JSON"));
    assert_eq!(env.read(), "{ \"model\": \"opus\", oops }");
    assert!(env.backups().is_empty());
}

#[test]
fn doctor_flags_hooks_pointing_at_a_tapin_that_moved() {
    let env = Env::new(None);
    let stale = serde_json::json!({"hooks": {"Stop": [{"hooks": [
        {"type": "command", "command": "\"/old/place/tapin\" hook --agent claude"}
    ]}]}});
    std::fs::write(
        env.settings(),
        serde_json::to_string_pretty(&stale).unwrap(),
    )
    .unwrap();
    let out = stdout(&env.run(&["doctor"], ""));
    assert!(out.contains("/old/place/tapin"), "{out}");
    assert!(out.contains("run `tapin init`"), "{out}");
}

#[test]
fn a_symlinked_settings_file_stays_a_symlink() {
    let env = Env::new(None);
    let dotfiles = env.dir.path().join("dotfiles");
    std::fs::create_dir(&dotfiles).unwrap();
    let real = dotfiles.join("settings.json");
    std::fs::write(&real, "{\n  \"model\": \"opus\"\n}\n").unwrap();
    std::os::unix::fs::symlink(&real, env.settings()).unwrap();
    assert!(env.run(&["init", "--yes"], "").status.success());
    assert!(std::fs::symlink_metadata(env.settings())
        .unwrap()
        .file_type()
        .is_symlink());
    let target: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&real).unwrap()).unwrap();
    assert_eq!(tapin_commands(&target, "Stop").len(), 1);
    // Backups stay in the config dir, not in the (possibly git-tracked) dotfiles dir.
    assert_eq!(env.backups().len(), 1);
    assert_eq!(std::fs::read_dir(&dotfiles).unwrap().count(), 1);
}

#[test]
fn a_broken_symlink_is_reported_not_replaced() {
    let env = Env::new(None);
    std::os::unix::fs::symlink(env.dir.path().join("gone.json"), env.settings()).unwrap();
    let out = env.run(&["init", "--yes"], "");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("broken symlink"));
    assert!(std::fs::symlink_metadata(env.settings())
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn a_world_readable_file_keeps_its_mode() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new(Some("with-other-hooks.json"));
    std::fs::set_permissions(env.settings(), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(env.run(&["init", "--yes"], "").status.success());
    let mode = std::fs::metadata(env.settings())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o644);
}

#[test]
fn hook_commands_are_shell_safe_and_recognised_for_odd_paths() {
    use tapin::setup::{hook_command, is_tapin_hook};
    for path in [
        "/opt/my tools/tapin",
        "/home/a$(touch x)/tapin",
        "/srv/it's/tapin",
        "/srv/q\"t/tapin",
    ] {
        let cmd = hook_command(Path::new(path), tapin::event::AgentKind::Claude);
        assert!(is_tapin_hook(&cmd), "{cmd}");
        // Through a real shell, the program part must expand to the path verbatim.
        let program = cmd.strip_suffix(" hook --agent claude").unwrap();
        let printed = Command::new("sh")
            .arg("-c")
            .arg(format!("printf %s {program}"))
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&printed.stdout), path, "{cmd}");
    }
}

#[test]
fn tab_indented_files_keep_their_tabs_and_round_trip() {
    let env = Env::new(None);
    let original = "{\n\t\"model\": \"opus\",\n\t\"permissions\": {\n\t\t\"allow\": []\n\t}\n}\n";
    std::fs::write(env.settings(), original).unwrap();
    assert!(env.run(&["init", "--yes"], "").status.success());
    assert!(
        env.read().lines().skip(1).all(|l| !l.starts_with(' ')),
        "{}",
        env.read()
    );
    assert!(env.run(&["uninstall", "--yes"], "").status.success());
    assert_eq!(env.read(), original);
}

#[test]
fn the_diff_shows_reformatting_too() {
    let env = Env::new(None);
    std::fs::write(env.settings(), "{\"model\":\"opus\"}").unwrap();
    let out = stdout(&env.run(&["init"], ""));
    assert!(
        out.contains("-{\"model\":\"opus\"}"),
        "minified line shown as removed:\n{out}"
    );
}

#[test]
fn backups_are_private_and_never_overwrite_each_other() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new(Some("with-other-hooks.json"));
    std::fs::set_permissions(env.settings(), std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(env.run(&["init", "--yes"], "").status.success());
    assert!(env.run(&["uninstall", "--yes"], "").status.success());
    let backups = env.backups();
    assert_eq!(
        backups.len(),
        2,
        "init and uninstall each back up: {backups:?}"
    );
    for b in backups {
        let mode = std::fs::metadata(&b).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{}", b.display());
    }
    let mode = std::fs::metadata(env.settings())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn init_repoints_hooks_at_a_tapin_that_moved() {
    let env = Env::new(None);
    let stale = serde_json::json!({"hooks": {"Stop": [{"hooks": [
        {"type": "command", "command": "\"/old/place/tapin\" hook --agent claude"}
    ]}]}});
    std::fs::write(
        env.settings(),
        serde_json::to_string_pretty(&stale).unwrap(),
    )
    .unwrap();
    assert!(env.run(&["init", "--yes"], "").status.success());
    let stop = tapin_commands(&env.json(), "Stop");
    assert_eq!(stop.len(), 1, "{stop:?}");
    assert!(!stop[0].contains("/old/place"), "{stop:?}");
}

#[test]
fn uninstall_leaves_neighbours_in_a_shared_group_and_lookalikes_alone() {
    let env = Env::new(None);
    let mixed = serde_json::json!({"hooks": {"Stop": [{"hooks": [
        {"type": "command", "command": "\"/usr/local/bin/tapin\" hook --agent claude"},
        {"type": "command", "command": "say done"},
        {"type": "command", "command": "/home/tapinder/bin/notify hook --agent claude"}
    ]}]}});
    std::fs::write(
        env.settings(),
        serde_json::to_string_pretty(&mixed).unwrap(),
    )
    .unwrap();
    assert!(env.run(&["uninstall", "--yes"], "").status.success());
    let left: Vec<String> = env.json()["hooks"]["Stop"][0]["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["command"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        left,
        vec!["say done", "/home/tapinder/bin/notify hook --agent claude"]
    );
}

#[test]
fn init_says_so_when_claude_code_is_not_installed() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-claude-here");
    let out = Command::new(env!("CARGO_BIN_EXE_tapin"))
        .args(["init", "--yes"])
        .env("CLAUDE_CONFIG_DIR", &missing)
        .env("CODEX_HOME", dir.path().join("no-codex"))
        .env("PATH", dir.path())
        .env("TAPIN_NO_TTY", "1")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("Claude Code not found"));
    assert!(
        !missing.exists(),
        "must not create a config for an agent that isn't there"
    );
}

mod common;

#[test]
fn doctor_reports_whether_the_daemon_is_reachable() {
    let env = Env::new(None);
    let d = common::start_daemon();
    let out = Command::new(env!("CARGO_BIN_EXE_tapin"))
        .args(["doctor", "--socket"])
        .arg(&d.socket)
        .env("CLAUDE_CONFIG_DIR", env.dir.path())
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("daemon    running"));
}

#[test]
fn doctor_shows_a_broken_config_instead_of_hiding_it() {
    let env = Env::new(None);
    let config_home = env.dir.path().join("cfg");
    std::fs::create_dir_all(config_home.join("tapin")).unwrap();
    std::fs::write(config_home.join("tapin/config.toml"), "notfy = false\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_tapin"))
        .arg("doctor")
        .env("CLAUDE_CONFIG_DIR", env.dir.path())
        .env("XDG_CONFIG_HOME", &config_home)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("config") && text.contains("notfy"), "{text}");
}

#[test]
fn tests_can_never_reach_the_real_agent_configs() {
    // .cargo/config.toml points these at target/ for every test process.
    let home = std::env::var("HOME").unwrap();
    for var in [
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
        "XDG_CONFIG_HOME",
        "GEMINI_CLI_HOME",
    ] {
        let v = std::env::var(var).unwrap_or_else(|_| panic!("{var} must be set for tests"));
        assert!(v.contains("target/test-isolation"), "{var}={v}");
        assert!(!v.starts_with(&format!("{home}/.")), "{var}={v}");
    }
}
