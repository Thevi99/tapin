//! Where an agent runs: the hook reads it from the environment it inherits
//! from the agent (hooks have no tty of their own), and rows carry it.

mod common;
use common::*;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

fn hook_with_env(socket: &std::path::Path, payload: &str, env: &[(&str, &str)]) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tapin"))
        .args(["hook", "--agent", "claude", "--socket"])
        .arg(socket)
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("TERM_PROGRAM")
        .env_remove("WEZTERM_PANE")
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    finish(child, Duration::from_secs(3));
}

#[test]
fn a_tmux_pane_reaches_the_row() {
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    hook_with_env(
        &d.socket,
        &fixture("basic-turn", "01-SessionStart.json"),
        &[
            ("TMUX", "/tmp/tmux-501/default,4242,0"),
            ("TMUX_PANE", "%7"),
            ("TERM_PROGRAM", "tmux"),
        ],
    );
    let rows = tui.wait_for(|r| r.as_array().is_some_and(|a| !a.is_empty()));
    let loc = &rows[0]["location"];
    assert_eq!(loc["tmux_socket"], "/tmp/tmux-501/default");
    assert_eq!(loc["tmux_pane"], "%7");
}

#[test]
fn later_events_without_location_keep_the_known_one() {
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    hook_with_env(
        &d.socket,
        &fixture("basic-turn", "01-SessionStart.json"),
        &[("TERM_PROGRAM", "WezTerm"), ("WEZTERM_PANE", "3")],
    );
    // Each hook is its own connection, handled on its own thread: let the
    // first event land before sending the second, or they can race.
    tui.wait_for(|r| {
        r.as_array().is_some_and(|a| {
            a.first()
                .is_some_and(|x| x["location"]["wezterm_pane"] == "3")
        })
    });
    hook_with_env(&d.socket, &fixture("basic-turn", "09-Stop.json"), &[]);
    let rows = tui.wait_for(|r| {
        r.as_array()
            .is_some_and(|a| a.first().is_some_and(|x| x["state"] == "Done"))
    });
    assert_eq!(rows[0]["location"]["wezterm_pane"], "3");
    assert_eq!(rows[0]["location"]["term_program"], "WezTerm");
}
