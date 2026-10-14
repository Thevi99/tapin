//! What each key does, without a terminal: `App::key` is pure.

use std::time::{Duration, Instant};
use tapin::inbox::{Row, State};
use tapin::tui::{Action, App, Overlay};

fn needs(id: &str, request: u64) -> Row {
    Row {
        session_id: id.into(),
        state: State::NeedsPermission,
        request_id: Some(request),
        request_input: Some("touch x".into()),
        ..Default::default()
    }
}

/// An app whose cursor has rested on the first row long enough to answer.
fn settled(rows: Vec<Row>) -> App {
    let mut app = App::default();
    app.update(rows, Instant::now() - Duration::from_secs(5));
    app
}

const WIDE: u16 = 100;

#[test]
fn y_and_n_answer_the_request_under_the_cursor() {
    let mut app = settled(vec![needs("a", 7)]);
    assert_eq!(app.key('y', WIDE, Instant::now()), Action::Decide(7, true));
    assert_eq!(app.key('n', WIDE, Instant::now()), Action::Decide(7, false));
}

#[test]
fn nothing_is_answered_while_an_overlay_covers_the_input() {
    for overlay_key in ['?', 'd'] {
        let mut app = settled(vec![needs("a", 7)]);
        app.key(overlay_key, WIDE, Instant::now());
        assert_ne!(app.overlay(), Overlay::None);
        assert_eq!(
            app.key('y', WIDE, Instant::now()),
            Action::None,
            "after {overlay_key}"
        );
        assert_eq!(app.key('n', WIDE, Instant::now()), Action::None);
    }
}

#[test]
fn q_closes_an_overlay_before_it_quits() {
    let mut app = settled(vec![needs("a", 7)]);
    app.key('?', WIDE, Instant::now());
    assert_eq!(app.key('q', WIDE, Instant::now()), Action::None);
    assert_eq!(app.overlay(), Overlay::None);
    assert_eq!(app.key('q', WIDE, Instant::now()), Action::Quit);
}

#[test]
fn details_close_when_their_session_goes_away() {
    let mut app = settled(vec![needs("a", 7)]);
    app.key('d', WIDE, Instant::now());
    app.update(vec![], Instant::now());
    assert_eq!(app.overlay(), Overlay::None);
    assert_eq!(
        app.key('d', WIDE, Instant::now()),
        Action::None,
        "no rows: no details"
    );
}

#[test]
fn enter_jumps_only_with_a_selection() {
    let mut empty = settled(vec![]);
    assert_eq!(empty.key('\n', WIDE, Instant::now()), Action::None);
    let mut app = settled(vec![needs("a", 7)]);
    assert!(matches!(app.key('\n', WIDE, Instant::now()), Action::Jump(r) if r.session_id == "a"));
}

#[test]
fn a_freshly_moved_cursor_ignores_y() {
    let mut app = App::default();
    app.update(vec![needs("a", 7)], Instant::now());
    assert_eq!(app.key('y', WIDE, Instant::now()), Action::None);
}
