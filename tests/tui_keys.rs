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

fn with_state(id: &str, state: State) -> Row {
    Row {
        session_id: id.into(),
        state,
        ..Default::default()
    }
}

fn jump_target(action: Action) -> Option<String> {
    match action {
        Action::Jump(r) => Some(r.session_id),
        _ => None,
    }
}

#[test]
fn the_cursor_can_leave_a_pending_request_and_stays_where_it_was_moved() {
    let rows = vec![needs("a", 7), with_state("b", State::Done)];
    let mut app = settled(rows.clone());
    app.key('j', WIDE, Instant::now());
    // The daemon re-sends the same snapshot every second.
    app.update(rows.clone(), Instant::now());
    app.update(rows, Instant::now());
    assert_eq!(
        jump_target(app.key('\n', WIDE, Instant::now())),
        Some("b".into())
    );
}

#[test]
fn a_new_request_still_pulls_the_cursor_to_it() {
    let mut app = settled(vec![with_state("b", State::Working)]);
    app.update(
        vec![needs("a", 8), with_state("b", State::Working)],
        Instant::now(),
    );
    assert_eq!(
        jump_target(app.key('\n', WIDE, Instant::now())),
        Some("a".into())
    );
}

#[test]
fn tab_goes_to_the_next_agent_waiting_on_you_and_wraps() {
    let mut app = settled(vec![
        needs("a", 7),
        with_state("b", State::Done),
        with_state("c", State::Working),
        with_state("d", State::Done),
    ]);
    let mut visited = Vec::new();
    for _ in 0..3 {
        assert_eq!(app.key('\t', WIDE, Instant::now()), Action::None);
        visited.push(jump_target(app.key('\n', WIDE, Instant::now())).unwrap());
    }
    assert_eq!(visited, ["b", "d", "a"]);
}

#[test]
fn tab_with_nobody_waiting_stays_put() {
    let mut app = settled(vec![
        with_state("x", State::Working),
        with_state("y", State::Idle),
    ]);
    app.key('\t', WIDE, Instant::now());
    assert_eq!(
        jump_target(app.key('\n', WIDE, Instant::now())),
        Some("x".into())
    );
}

#[test]
fn y_right_after_tab_lands_on_a_request_is_ignored_until_it_settles() {
    let mut app = settled(vec![with_state("b", State::Done), needs("a", 7)]);
    // The cursor is pulled to "a"; move off it, then Tab back onto it.
    app.key('k', WIDE, Instant::now());
    app.key('\t', WIDE, Instant::now());
    let now = Instant::now();
    assert_eq!(app.key('y', WIDE, now), Action::None);
    assert_eq!(
        app.key('y', WIDE, now + Duration::from_secs(1)),
        Action::Decide(7, true)
    );
}

#[test]
fn a_newer_request_from_the_same_session_pulls_the_cursor_too() {
    let mut app = settled(vec![needs("a", 7), with_state("b", State::Done)]);
    app.key('j', WIDE, Instant::now());
    app.update(
        vec![needs("a", 8), with_state("b", State::Done)],
        Instant::now(),
    );
    assert_eq!(
        jump_target(app.key('\n', WIDE, Instant::now())),
        Some("a".into())
    );
}

fn project(id: &str, state: State, project: &str) -> Row {
    Row {
        project: project.into(),
        ..with_state(id, state)
    }
}

fn typed(app: &mut App, keys: &str) -> Vec<Action> {
    keys.chars()
        .map(|c| app.key(c, WIDE, Instant::now() + Duration::from_secs(5)))
        .collect()
}

#[test]
fn typing_a_filter_never_approves_quits_or_moves() {
    let mut app = settled(vec![needs("a", 7), with_state("b", State::Done)]);
    let actions = typed(&mut app, "/yqnjk\n");
    assert!(actions.iter().all(|a| *a == Action::None), "{actions:?}");
}

#[test]
fn a_filter_shows_only_matching_rows_and_esc_brings_them_back() {
    let mut app = settled(vec![
        project("a", State::Done, "shop-api"),
        project("b", State::Done, "docs-site"),
    ]);
    typed(&mut app, "/DOCS\n");
    assert_eq!(app.visible().len(), 1);
    assert_eq!(
        jump_target(app.key('\n', WIDE, Instant::now())),
        Some("b".into())
    );
    // New snapshots stay filtered.
    app.update(
        vec![
            project("a", State::Done, "shop-api"),
            project("b", State::Done, "docs-site"),
        ],
        Instant::now(),
    );
    assert_eq!(app.visible().len(), 1);
    app.key('\x1b', WIDE, Instant::now());
    assert_eq!(app.visible().len(), 2);
}

#[test]
fn backspace_edits_and_esc_while_typing_cancels() {
    let mut app = settled(vec![
        project("a", State::Done, "shop-api"),
        project("b", State::Done, "docs-site"),
    ]);
    typed(&mut app, "/docx\x08");
    assert_eq!(app.filter(), Some("doc"));
    assert_eq!(app.visible().len(), 1);
    app.key('\x1b', WIDE, Instant::now());
    assert_eq!(app.filter(), None);
    assert_eq!(app.visible().len(), 2);
}

#[test]
fn y_never_answers_a_request_the_filter_hides() {
    let mut app = settled(vec![needs("a", 7), project("b", State::Done, "docs-site")]);
    typed(&mut app, "/docs\n");
    // A new request arrives while filtered out: it must not grab the cursor.
    app.update(
        vec![
            needs("a", 7),
            needs("c", 9),
            project("b", State::Done, "docs-site"),
        ],
        Instant::now(),
    );
    let later = Instant::now() + Duration::from_secs(5);
    assert_eq!(app.key('y', WIDE, later), Action::None);
    assert_eq!(jump_target(app.key('\n', WIDE, later)), Some("b".into()));
}

#[test]
fn y_right_after_a_filter_change_lands_on_a_request_waits_to_settle() {
    let mut app = settled(vec![
        project("b", State::Done, "docs-site"),
        Row {
            project: "api".into(),
            ..needs("a", 9)
        },
    ]);
    app.key('k', WIDE, Instant::now());
    typed(&mut app, "/api\n");
    let now = Instant::now() + Duration::from_secs(5);
    assert_eq!(app.key('y', WIDE, now), Action::None);
    assert_eq!(
        app.key('y', WIDE, now + Duration::from_secs(1)),
        Action::Decide(9, true)
    );
}

#[test]
fn the_view_still_counts_requests_the_filter_hides() {
    let mut app = settled(vec![needs("a", 7), project("b", State::Done, "docs-site")]);
    typed(&mut app, "/docs\n");
    let view = app.view(true, None);
    assert_eq!(view.rows.len(), 1);
    assert_eq!((view.hidden, view.hidden_waiting), (1, 1));
}

#[test]
fn arrow_keys_never_type_into_the_filter() {
    let mut app = settled(vec![project("b", State::Done, "docs-site")]);
    app.key('/', WIDE, Instant::now());
    assert!(app.typing());
    // The event loop drops arrows and Ctrl keys while typing.
    assert!(!tapin::tui::passes_while_typing(
        crossterm::event::KeyCode::Down,
        false
    ));
    assert!(!tapin::tui::passes_while_typing(
        crossterm::event::KeyCode::Char('c'),
        true
    ));
    assert!(tapin::tui::passes_while_typing(
        crossterm::event::KeyCode::Char('c'),
        false
    ));
}

#[test]
fn the_bell_rings_once_per_new_request_even_when_the_filter_hides_it() {
    let rows = vec![needs("a", 7), project("b", State::Done, "docs-site")];
    let mut app = settled(rows.clone());
    typed(&mut app, "/docs\n");
    assert!(!app.rings_for(&rows), "a request already seen, only hidden");
    let more = vec![
        needs("a", 7),
        needs("c", 9),
        project("b", State::Done, "docs-site"),
    ];
    assert!(app.rings_for(&more));
}

fn in_dir(id: &str, state: State, cwd: &str) -> Row {
    Row {
        cwd: cwd.into(),
        ..with_state(id, state)
    }
}

trait WithRequest {
    fn with_request(self, id: u64) -> Row;
}

impl WithRequest for Row {
    fn with_request(self, id: u64) -> Row {
        Row {
            request_id: Some(id),
            request_input: Some("touch x".into()),
            ..self
        }
    }
}

fn ids(app: &App) -> Vec<String> {
    app.visible().iter().map(|r| r.session_id.clone()).collect()
}

#[test]
fn g_groups_rows_by_folder_with_the_most_urgent_group_first() {
    let mut app = settled(vec![
        in_dir("a", State::Done, "/w/api"),
        in_dir("b", State::Working, "/w/web"),
        in_dir("c", State::Idle, "/w/api"),
    ]);
    app.key('g', WIDE, Instant::now());
    assert!(app.grouped());
    assert_eq!(ids(&app), ["a", "c", "b"]);
    // Snapshots keep the grouping, and j walks rows in grouped order.
    app.update(
        vec![
            in_dir("a", State::Done, "/w/api"),
            in_dir("b", State::Working, "/w/web"),
            in_dir("c", State::Idle, "/w/api"),
        ],
        Instant::now(),
    );
    app.key('j', WIDE, Instant::now());
    assert_eq!(
        jump_target(app.key('\n', WIDE, Instant::now())),
        Some("c".into())
    );
    app.key('g', WIDE, Instant::now());
    assert_eq!(ids(&app), ["a", "b", "c"]);
}

#[test]
fn grouped_the_cursor_still_goes_to_the_oldest_new_request() {
    let older = |id: &str, cwd: &str, req: u64, age: u64| {
        Row {
            age_secs: age,
            ..in_dir(id, State::NeedsPermission, cwd)
        }
        .with_request(req)
    };
    // Urgency order: n1 (seen) /api, n2 (new, older) /web, n3 (new) /api.
    let mut app = settled(vec![
        older("n1", "/w/api", 1, 50),
        in_dir("x", State::Idle, "/w/x"),
    ]);
    app.key('g', WIDE, Instant::now());
    app.key('j', WIDE, Instant::now()); // off the request
    app.key('j', WIDE, Instant::now());
    app.update(
        vec![
            older("n1", "/w/api", 1, 50),
            older("n2", "/w/web", 2, 40),
            older("n3", "/w/api", 3, 30),
            in_dir("x", State::Idle, "/w/x"),
        ],
        Instant::now(),
    );
    assert_eq!(
        jump_target(app.key('\n', WIDE, Instant::now())),
        Some("n2".into())
    );
}

#[test]
fn grouping_reorders_the_filtered_rows() {
    let tagged = |id: &str, cwd: &str| Row {
        detail: "deploy".into(),
        ..in_dir(id, State::Done, cwd)
    };
    let mut app = settled(vec![
        tagged("a", "/w/api"),
        tagged("b", "/w/web"),
        in_dir("z", State::Done, "/w/zzz"),
        tagged("c", "/w/api"),
    ]);
    app.key('g', WIDE, Instant::now());
    typed(&mut app, "/deploy\n");
    assert_eq!(ids(&app), ["a", "c", "b"]);
}

#[test]
fn g_keeps_the_same_request_under_the_cursor_so_y_still_answers_it() {
    let mut app = settled(vec![needs("a", 7), in_dir("b", State::Done, "/w/b")]);
    app.key('g', WIDE, Instant::now());
    assert_eq!(app.key('y', WIDE, Instant::now()), Action::Decide(7, true));
}
