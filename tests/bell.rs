//! The terminal bell rings when a new request appears, not on every redraw.

use tapin::event::AgentKind;
use tapin::inbox::{Row, State};
use tapin::tui::new_requests;

fn row(id: &str, request: Option<u64>) -> Row {
    Row {
        session_id: id.into(),
        state: if request.is_some() {
            State::NeedsPermission
        } else {
            State::Working
        },
        agent: AgentKind::Claude,
        project: id.into(),
        detail: String::new(),
        age_secs: 0,
        request_id: request,
        request_input: None,
        ..Default::default()
    }
}

#[test]
fn a_request_that_was_not_there_before_counts() {
    assert_eq!(new_requests(&[row("a", None)], &[row("a", Some(1))]), 1);
}

#[test]
fn the_same_request_on_the_next_snapshot_does_not() {
    assert_eq!(new_requests(&[row("a", Some(1))], &[row("a", Some(1))]), 0);
}
