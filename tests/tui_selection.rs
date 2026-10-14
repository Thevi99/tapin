//! Which row a keypress acts on. `y` approves the selected row, so the
//! selection must never silently jump to a different session.

use tapin::event::AgentKind;
use tapin::inbox::{Row, State};
use tapin::tui::reselect;

fn row(id: &str) -> Row {
    Row {
        session_id: id.into(),
        state: State::Working,
        agent: AgentKind::Claude,
        project: id.into(),
        detail: String::new(),
        age_secs: 0,
        request_id: None,
        request_input: None,
        ..Default::default()
    }
}

#[test]
fn nothing_chosen_yet_selects_the_top_row() {
    assert_eq!(reselect(&[row("a"), row("b")], None, &[]), Some(0));
    assert_eq!(reselect(&[], None, &[]), None);
}

#[test]
fn selection_follows_its_session_when_rows_reorder() {
    assert_eq!(reselect(&[row("a"), row("b")], Some("b"), &[]), Some(1));
    assert_eq!(reselect(&[row("b"), row("a")], Some("b"), &[]), Some(0));
}

#[test]
fn a_vanished_selection_selects_nothing_rather_than_another_session() {
    assert_eq!(reselect(&[row("a"), row("c")], Some("b"), &[]), None);
}

fn needs(id: &str) -> Row {
    Row {
        state: State::NeedsPermission,
        request_id: Some(1),
        ..row(id)
    }
}

#[test]
fn a_new_request_pulls_the_cursor_off_a_row_that_cannot_be_answered() {
    // Cursor was on "b" (busy); a request from "a" arrives at the top.
    assert_eq!(reselect(&[needs("a"), row("b")], Some("b"), &[]), Some(0));
}

#[test]
fn the_cursor_stays_on_the_request_being_read() {
    assert_eq!(reselect(&[needs("a"), needs("b")], Some("b"), &[]), Some(1));
}

#[test]
fn when_the_selected_request_is_gone_the_next_one_is_offered() {
    assert_eq!(reselect(&[needs("a"), row("c")], Some("b"), &[]), Some(0));
}

#[test]
fn a_request_already_on_screen_does_not_pull_the_cursor_back() {
    let known = vec![("a".to_string(), Some(1))];
    assert_eq!(
        reselect(&[needs("a"), row("b")], Some("b"), &known),
        Some(1)
    );
}

#[test]
fn when_the_selected_session_is_gone_a_request_already_shown_is_offered() {
    let known = vec![("a".to_string(), Some(1))];
    assert_eq!(
        reselect(&[needs("a"), row("c")], Some("b"), &known),
        Some(0)
    );
}
