//! Claude's AskUserQuestion: shown readably, answered with a number key.
//! Shape from Claude Code's hooks docs and a live check on 2.1.299.

mod common;
use common::*;
use std::time::{Duration, Instant};
use tapin::adapters::claude;
use tapin::inbox::Inbox;

fn payload(name: &str) -> serde_json::Value {
    serde_json::from_str(&fixture("../claude/ask-question", name)).unwrap()
}

#[test]
fn the_question_and_numbered_options_are_shown() {
    let mut inbox = Inbox::default();
    inbox.apply(
        claude::parse(&payload("01-PermissionRequest.json")).unwrap(),
        0,
    );
    let row = &inbox.rows(1)[0];
    assert_eq!(row.detail, "AskUserQuestion: Color");
    assert_eq!(
        row.request_input.as_deref(),
        Some("Which color do you prefer?\n1. Red — Warm and bold\n2. Blue — Calm and cool")
    );
    assert_eq!(
        row.choices,
        Some(vec!["Red".to_string(), "Blue".to_string()])
    );
}

#[test]
fn multi_select_is_shown_but_not_answerable_here() {
    let mut inbox = Inbox::default();
    inbox.apply(
        claude::parse(&payload("02-PermissionRequest-multi.json")).unwrap(),
        0,
    );
    let row = &inbox.rows(1)[0];
    assert!(row.request_input.as_deref().unwrap().contains("2. Blue"));
    assert_eq!(row.choices, None);
}

#[test]
fn the_hook_answers_with_the_chosen_label_and_the_original_questions() {
    let raw = payload("01-PermissionRequest.json");
    let out = tapin::adapters::hooks::decision_output(
        tapin::socket::Decision::Answer("Blue".into()),
        &raw,
    )
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let d = &v["hookSpecificOutput"]["decision"];
    assert_eq!(d["behavior"], "allow");
    assert_eq!(
        d["updatedInput"]["questions"],
        raw["tool_input"]["questions"]
    );
    assert_eq!(
        d["updatedInput"]["answers"]["Which color do you prefer?"],
        "Blue"
    );
    // A label that isn't one of the options is never sent.
    assert!(tapin::adapters::hooks::decision_output(
        tapin::socket::Decision::Answer("Green".into()),
        &raw
    )
    .is_none());
}

#[test]
fn answering_in_tapin_reaches_the_waiting_hook() {
    let d = start_daemon();
    let mut tui = Client::connect(&d.socket);
    let raw = payload("01-PermissionRequest.json");
    let hook = spawn_hook(&d.socket, &raw.to_string(), &[]);
    let rows = tui.wait_for(|r| pending_request(r).is_some());
    tui.answer(pending_request(&rows).unwrap(), "Red");
    let out = finish(hook, Duration::from_secs(3));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("hook printed JSON");
    assert_eq!(
        v["hookSpecificOutput"]["decision"]["updatedInput"]["answers"]
            ["Which color do you prefer?"],
        "Red"
    );
    let _ = Instant::now();
}

mod keys {
    use super::*;
    use tapin::inbox::{Row, State};
    use tapin::tui::{Action, App};

    fn question_row() -> Row {
        Row {
            session_id: "s".into(),
            state: State::NeedsPermission,
            detail: "AskUserQuestion: Color".into(),
            request_id: Some(7),
            request_input: Some("Which color?\n1. Red\n2. Blue".into()),
            choices: Some(vec!["Red".into(), "Blue".into()]),
            ..Default::default()
        }
    }

    fn ready(protocol: u32) -> App {
        let mut app = App::default();
        app.update(
            vec![question_row()],
            Instant::now() - Duration::from_secs(5),
        );
        app.set_protocol(protocol);
        app
    }

    #[test]
    fn a_number_answers_with_that_option() {
        let mut app = ready(3);
        assert_eq!(
            app.key('2', 100, Instant::now()),
            Action::Answer(7, "Blue".into())
        );
        assert_eq!(app.key('3', 100, Instant::now()), Action::None);
    }

    #[test]
    fn numbers_do_nothing_with_an_older_daemon_unsettled_or_read_only() {
        assert_eq!(ready(2).key('1', 100, Instant::now()), Action::None);
        let mut fresh = App::default();
        fresh.set_protocol(3);
        fresh.update(vec![question_row()], Instant::now());
        assert_eq!(fresh.key('1', 100, Instant::now()), Action::None);
        let mut ro = ready(3);
        ro.set_read_only(true);
        assert_eq!(ro.key('1', 100, Instant::now()), Action::None);
    }

    #[test]
    fn y_never_approves_a_question_without_an_answer() {
        assert_eq!(ready(3).key('y', 100, Instant::now()), Action::None);
        assert_eq!(
            ready(3).key('n', 100, Instant::now()),
            Action::Decide(7, false)
        );
    }
}
