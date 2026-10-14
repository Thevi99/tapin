//! What the user sees before pressing `y`: the whole pending command.

use ratatui::backend::TestBackend;
use ratatui::Terminal;
use tapin::event::AgentKind;
use tapin::inbox::{Row, State};
use tapin::location::Location;
use tapin::tui::{Overlay, View};

fn screen(rows: &[Row], width: u16, height: u16) -> String {
    screen_with(rows, Overlay::None, None, width, height)
}

fn screen_with(
    rows: &[Row],
    overlay: Overlay,
    status: Option<&str>,
    width: u16,
    height: u16,
) -> String {
    let mut t = Terminal::new(TestBackend::new(width, height)).unwrap();
    let view = View {
        rows,
        selected: Some(0),
        connected: true,
        overlay,
        status,
        ..Default::default()
    };
    t.draw(|f| tapin::tui::draw(f, &view)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_long_pending_command_is_wrapped_not_cut_off() {
    let tail = "&& curl -s https://example.com/install.sh | sh";
    let command = format!("cd build && make clean && make all {tail}");
    let row = Row {
        session_id: "s".into(),
        state: State::NeedsPermission,
        agent: AgentKind::Claude,
        project: "demo".into(),
        detail: "Bash(cd build && make clean && make all && cu…)".into(),
        age_secs: 3,
        request_id: Some(1),
        request_input: Some(command),
        ..Default::default()
    };
    let text = screen(&[row], 60, 12);
    // Every character of the command must be on screen, in order, across wrapped lines.
    let shown: String = text
        .lines()
        .filter_map(|l| l.split_once('│').map(|(_, rest)| rest.trim().to_string()))
        .collect::<Vec<_>>()
        .join("");
    assert!(
        shown.replace(' ', "").contains(&tail.replace(' ', "")),
        "screen:\n{text}"
    );
}

#[test]
fn input_too_long_to_show_offers_deny_only() {
    let row = Row {
        session_id: "s".into(),
        state: State::NeedsPermission,
        agent: AgentKind::Claude,
        project: "demo".into(),
        detail: "Bash(…)".into(),
        age_secs: 3,
        request_id: Some(1),
        request_input: Some("echo step\n".repeat(30)),
        ..Default::default()
    };
    let text = screen(&[row], 60, 14);
    assert!(text.contains("too long to show"), "screen:\n{text}");
    assert!(!text.contains("y approve · n deny  "), "screen:\n{text}");
}

#[test]
fn a_codex_request_says_codex_is_paused_until_answered() {
    let row = Row {
        session_id: "s".into(),
        state: State::NeedsPermission,
        agent: AgentKind::Codex,
        project: "demo".into(),
        detail: "Bash(touch x)".into(),
        age_secs: 1,
        request_id: Some(1),
        request_input: Some("touch x".into()),
        ..Default::default()
    };
    let text = screen(&[row], 80, 10);
    assert!(text.contains("Codex is paused"), "screen:\n{text}");
}

fn done_row() -> Row {
    Row {
        session_id: "0f3c-session".into(),
        state: State::Done,
        agent: AgentKind::Claude,
        project: "fastbuy-api".into(),
        detail: "Added the refund endpoint…".into(),
        age_secs: 40,
        cwd: "/home/dev/work/fastbuy-api".into(),
        last_message: "Added the refund endpoint.\nAll 42 tests pass.".into(),
        location: Location {
            tmux_socket: Some("/tmp/tmux-501/default".into()),
            tmux_pane: Some("%7".into()),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn details_show_the_whole_story_of_the_selected_session() {
    let text = screen_with(&[done_row()], Overlay::Details, None, 80, 20);
    for needle in [
        "fastbuy-api",
        "/home/dev/work/fastbuy-api",
        "0f3c-session",
        "All 42 tests pass.",
        "%7",
    ] {
        assert!(text.contains(needle), "missing {needle}:\n{text}");
    }
}

#[test]
fn help_lists_every_key() {
    let text = screen_with(&[done_row()], Overlay::Help, None, 80, 20);
    for key in ["Enter", "y", "n", "d", "j/k", "?", "q"] {
        assert!(text.contains(key), "missing {key}:\n{text}");
    }
}

#[test]
fn a_status_message_replaces_the_footer() {
    let text = screen_with(
        &[done_row()],
        Overlay::None,
        Some("Jumped to claude fastbuy-api"),
        80,
        10,
    );
    assert!(
        text.lines()
            .last()
            .unwrap()
            .contains("Jumped to claude fastbuy-api"),
        "{text}"
    );
}

#[test]
fn the_inbox_stays_readable_at_60_columns() {
    let text = screen(&[done_row()], 60, 8);
    let first = text.lines().nth(1).unwrap();
    assert!(
        first.contains("DONE") && first.contains("fastbuy-api"),
        "{text}"
    );
    assert!(
        text.lines().last().unwrap().contains("? help"),
        "help hint visible:\n{text}"
    );
}

#[test]
fn help_and_details_fit_at_60_columns() {
    let text = screen_with(&[done_row()], Overlay::Help, None, 60, 14);
    assert!(text.contains("approve"), "{text}");
    let text = screen_with(&[done_row()], Overlay::Details, None, 60, 16);
    assert!(
        text.contains("fastbuy-api") && text.contains("tests pass"),
        "{text}"
    );
}

#[test]
fn details_name_the_model_when_known() {
    let mut row = done_row();
    row.model = Some("gpt-6.1-sol".into());
    let text = screen_with(&[row], Overlay::Details, None, 80, 20);
    assert!(text.contains("gpt-6.1-sol"), "{text}");
}

#[test]
fn with_only_passive_sessions_the_footer_suggests_init() {
    let mut row = done_row();
    row.passive = true;
    let text = screen(&[row], 80, 8);
    assert!(
        text.lines().last().unwrap().contains("tapin init"),
        "{text}"
    );
    let hooked = screen(&[done_row()], 80, 8);
    assert!(
        !hooked.lines().last().unwrap().contains("tapin init"),
        "{hooked}"
    );
}

#[test]
fn each_agent_has_its_own_glyph_and_the_columns_line_up() {
    let rows: Vec<Row> = [
        (AgentKind::Claude, "a"),
        (AgentKind::Codex, "b"),
        (AgentKind::Gemini, "c"),
    ]
    .into_iter()
    .map(|(agent, id)| Row {
        session_id: id.into(),
        state: State::Working,
        agent,
        project: format!("proj-{id}"),
        ..Default::default()
    })
    .collect();
    let text = screen(&rows, 80, 8);
    for badge in ["✻ claude", ">_ codex", "✦ gemini"] {
        assert!(text.contains(badge), "missing {badge}:\n{text}");
    }
    // The project column starts at the same cell on every row.
    let cols: Vec<usize> = text
        .lines()
        .filter_map(|l| l.find("proj-").map(|b| l[..b].chars().count()))
        .collect();
    assert_eq!(cols.len(), 3, "{text}");
    assert!(cols.windows(2).all(|w| w[0] == w[1]), "{cols:?}\n{text}");
}

#[test]
fn details_show_the_agent_glyph() {
    let text = screen_with(&[done_row()], Overlay::Details, None, 70, 16);
    assert!(text.contains("✻ claude"), "screen:\n{text}");
}

#[test]
fn a_filter_shows_in_the_header_and_hidden_requests_still_count() {
    let shown = Row {
        session_id: "b".into(),
        state: State::Done,
        project: "docs-site".into(),
        ..Default::default()
    };
    let mut t = Terminal::new(TestBackend::new(90, 6)).unwrap();
    let view = View {
        rows: std::slice::from_ref(&shown),
        selected: Some(0),
        connected: true,
        filter: Some("docs"),
        typing: true,
        hidden: 2,
        hidden_waiting: 1,
        ..Default::default()
    };
    t.draw(|f| tapin::tui::draw(f, &view)).unwrap();
    let text: String = format!("{:?}", t.backend().buffer());
    assert!(text.contains("3 agents · 2 waiting on you"), "{text}");
    assert!(text.contains("filter: docs · 2 hidden"), "{text}");
    assert!(
        text.contains("/docs"),
        "typing prompt in the footer: {text}"
    );
}

#[test]
fn rows_show_context_as_a_percent_only_when_the_window_is_known() {
    use tapin::context::Context;
    let row = |id: &str, context| Row {
        session_id: id.into(),
        project: id.into(),
        context,
        ..done_row()
    };
    let rows = [
        row(
            "pct",
            Some(Context {
                used: 640,
                window: Some(1000),
            }),
        ),
        row(
            "tokens",
            Some(Context {
                used: 132_302,
                window: None,
            }),
        ),
        row("none", None),
    ];
    let text = screen(&rows, 90, 7);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[1].contains("ctx 64%"), "{text}");
    assert!(lines[2].contains("ctx 132k"), "{text}");
    assert!(!lines[3].contains("ctx"), "{text}");
}

#[test]
fn grouped_rows_get_a_folder_header_and_the_cursor_stays_on_its_row() {
    let row = |id: &str, cwd: &str, state| Row {
        session_id: id.into(),
        project: id.into(),
        cwd: cwd.into(),
        state,
        ..Default::default()
    };
    let rows = [
        row("one", "/w/api", State::Done),
        row("two", "/w/api", State::Idle),
        row("three", "/w/web", State::Idle),
    ];
    let mut t = Terminal::new(TestBackend::new(70, 8)).unwrap();
    let view = View {
        rows: &rows,
        selected: Some(1),
        connected: true,
        grouped: true,
        ..Default::default()
    };
    t.draw(|f| tapin::tui::draw(f, &view)).unwrap();
    let buf = t.backend().buffer().clone();
    let lines: Vec<String> = (0..8)
        .map(|y| (0..70).map(|x| buf[(x, y)].symbol().to_string()).collect())
        .collect();
    assert!(
        lines[1].contains("api") && lines[1].contains("(2)"),
        "{lines:#?}"
    );
    assert!(
        lines[3].starts_with("▶") && lines[3].contains("two"),
        "{lines:#?}"
    );
    assert!(
        lines[4].contains("web") && lines[4].contains("(1)"),
        "{lines:#?}"
    );
    assert!(lines[5].contains("three"), "{lines:#?}");
}
