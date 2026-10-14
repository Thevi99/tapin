//! How Enter gets the user to a session: a pure plan, run by the TUI.

use tapin::jump::{jump_plan, Here, Jump};
use tapin::location::Location;

fn tmux(pane: &str) -> Location {
    Location {
        tmux_socket: Some("/tmp/tmux-501/default".into()),
        tmux_pane: Some(pane.into()),
        ..Default::default()
    }
}

fn run(j: Jump) -> Vec<Vec<String>> {
    match j {
        Jump::Run(cmds) => cmds,
        Jump::Explain(m) => panic!("expected commands, got: {m}"),
    }
}

fn explain(j: Jump) -> String {
    match j {
        Jump::Explain(m) => m,
        Jump::Run(c) => panic!("expected an explanation, got {c:?}"),
    }
}

#[test]
fn inside_the_same_tmux_server_switch_this_client() {
    let here = Here {
        tmux_socket: Some("/tmp/tmux-501/default".into()),
        ..Default::default()
    };
    assert_eq!(
        run(jump_plan(&tmux("%7"), "/p", &here, None)),
        vec![vec![
            "tmux",
            "-S",
            "/tmp/tmux-501/default",
            "switch-client",
            "-t",
            "%7"
        ]]
    );
}

#[test]
fn from_outside_that_server_switch_its_most_recent_client() {
    let here = Here::default();
    assert_eq!(
        run(jump_plan(&tmux("%7"), "/p", &here, Some("/dev/ttys004"))),
        vec![vec![
            "tmux",
            "-S",
            "/tmp/tmux-501/default",
            "switch-client",
            "-c",
            "/dev/ttys004",
            "-t",
            "%7"
        ]]
    );
}

#[test]
fn with_no_attached_client_say_how_to_attach() {
    let m = explain(jump_plan(&tmux("%7"), "/p", &Here::default(), None));
    assert!(
        m.contains("tmux -S '/tmp/tmux-501/default' attach -t %7"),
        "{m}"
    );
}

#[test]
fn iterm_and_terminal_are_focused_by_tty_with_the_tty_as_an_argument() {
    for (program, app) in [("iTerm.app", "iTerm"), ("Apple_Terminal", "Terminal")] {
        let loc = Location {
            term_program: Some(program.into()),
            tty: Some("/dev/ttys012".into()),
            ..Default::default()
        };
        let cmds = run(jump_plan(
            &loc,
            "/p",
            &Here {
                macos: true,
                ..Default::default()
            },
            None,
        ));
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0][0], "osascript");
        assert!(cmds[0].iter().any(|a| a.contains(app)), "{cmds:?}");
        // Data goes in argv, never spliced into the script.
        assert_eq!(cmds[0].last().unwrap(), "/dev/ttys012");
        assert!(cmds[0].iter().filter(|a| a.contains("ttys012")).count() == 1);
    }
}

#[test]
fn wezterm_activates_its_pane() {
    let loc = Location {
        term_program: Some("WezTerm".into()),
        wezterm_pane: Some("3".into()),
        ..Default::default()
    };
    assert_eq!(
        run(jump_plan(&loc, "/p", &Here::default(), None)),
        vec![vec!["wezterm", "cli", "activate-pane", "--pane-id", "3"]]
    );
}

#[test]
fn anything_else_says_where_the_session_is() {
    let loc = Location {
        tty: Some("/dev/ttys012".into()),
        term_program: Some("ghostty".into()),
        ..Default::default()
    };
    let m = explain(jump_plan(
        &loc,
        "/work/api",
        &Here {
            macos: true,
            ..Default::default()
        },
        None,
    ));
    assert!(m.contains("/dev/ttys012") && m.contains("/work/api"), "{m}");
    let m = explain(jump_plan(
        &Location::default(),
        "/work/api",
        &Here::default(),
        None,
    ));
    assert!(m.contains("/work/api"), "{m}");
}

#[test]
fn osascript_is_not_offered_off_macos() {
    let loc = Location {
        term_program: Some("iTerm.app".into()),
        tty: Some("/dev/pts/3".into()),
        ..Default::default()
    };
    explain(jump_plan(&loc, "/p", &Here::default(), None));
}

#[test]
fn a_tty_that_is_not_a_device_path_is_never_passed_on() {
    let loc = Location {
        term_program: Some("iTerm.app".into()),
        tty: Some("-e".into()),
        ..Default::default()
    };
    explain(jump_plan(
        &loc,
        "/p",
        &Here {
            macos: true,
            ..Default::default()
        },
        None,
    ));
}
