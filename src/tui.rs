//! `tapin`: the inbox TUI. A thin view over daemon snapshots.

use crate::inbox::{Row, State};
use crate::socket::{Request, Update};
use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub fn run(socket: &Path, no_notify: bool) -> Result<()> {
    let (config, config_error) = crate::config::Config::load_checked();
    let stream = connect_or_spawn(socket, no_notify)?;
    let mut control = stream.try_clone()?;
    let (tx, rx) = mpsc::channel();
    subscribe(stream, tx)?;

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &rx, &mut control, config.bell, config_error);
    ratatui::restore();
    result
}

fn connect_or_spawn(socket: &Path, no_notify: bool) -> Result<UnixStream> {
    crate::socket::check_path(socket)?;
    if let Ok(s) = UnixStream::connect(socket) {
        return Ok(s);
    }
    Command::new(std::env::current_exe()?)
        .arg("daemon")
        .arg("--socket")
        .arg(socket)
        .args(no_notify.then_some("--no-notify"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0) // outlive this terminal
        .spawn()
        .context("starting tapin daemon")?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(s) = UnixStream::connect(socket) {
            return Ok(s);
        }
        anyhow::ensure!(Instant::now() < deadline, "tapin daemon did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Snapshots from the daemon; `None` means the connection is gone.
type Feed = mpsc::Receiver<Option<Vec<Row>>>;

fn subscribe(mut stream: UnixStream, tx: mpsc::Sender<Option<Vec<Row>>>) -> Result<()> {
    let mut line = serde_json::to_string(&Request::Subscribe)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            if let Ok(Update::Rows { rows }) = serde_json::from_str(&line) {
                if tx.send(Some(rows)).is_err() {
                    return;
                }
            }
        }
        let _ = tx.send(None);
    });
    Ok(())
}

/// What the inbox is showing and where the cursor is. Pure: keys go in,
/// actions come out, so the rules about what a key may do are testable.
#[derive(Debug, Default)]
pub struct App {
    rows: Vec<Row>,
    /// The session the user chose (kept across re-sorts and while it vanishes).
    selected: Option<String>,
    index: Option<usize>,
    overlay: Overlay,
    /// The request under the cursor, and since when; `y`/`n` wait for SETTLE.
    cursor_request: Option<u64>,
    cursor_since: Option<Instant>,
}

/// What a key asks the event loop to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Quit,
    Decide(u64, bool),
    Jump(Box<Row>),
}

impl App {
    /// A new snapshot from the daemon.
    pub fn update(&mut self, rows: Vec<Row>, now: Instant) {
        self.rows = rows;
        self.reindex(now);
    }

    pub fn overlay(&self) -> Overlay {
        self.overlay
    }

    fn reindex(&mut self, now: Instant) {
        self.index = reselect(&self.rows, self.selected.as_deref());
        if let Some(row) = self.index.and_then(|i| self.rows.get(i)) {
            self.selected = Some(row.session_id.clone());
        }
        let under = self.index.and_then(|i| self.rows.get(i)?.request_id);
        if under != self.cursor_request || self.cursor_since.is_none() {
            self.cursor_request = under;
            self.cursor_since = Some(now);
        }
        if self.index.is_none() && self.overlay == Overlay::Details {
            self.overlay = Overlay::None; // nothing left to describe
        }
    }

    fn move_to(&mut self, index: usize, now: Instant) {
        if let Some(row) = self.rows.get(index) {
            self.selected = Some(row.session_id.clone());
            self.reindex(now);
        }
    }

    /// Handles one key: chars as typed, `\n` for Enter, `\x1b` for Esc,
    /// `j`/`k` for the arrows. `width` is the screen width, for what fits.
    pub fn key(&mut self, key: char, width: u16, now: Instant) -> Action {
        let row = self.index.and_then(|i| self.rows.get(i)).cloned();
        if self.overlay != Overlay::None {
            // An overlay covers the inbox: only closing it does anything.
            if matches!(key, 'q' | '\x1b' | '?' | 'd') {
                self.overlay = Overlay::None;
            }
            return Action::None;
        }
        match key {
            'q' => Action::Quit,
            '?' => {
                self.overlay = Overlay::Help;
                Action::None
            }
            'd' if row.is_some() => {
                self.overlay = Overlay::Details;
                Action::None
            }
            '\n' => row.map_or(Action::None, |r| Action::Jump(Box::new(r))),
            'j' if !self.rows.is_empty() => {
                let i = self.index.map_or(0, |i| (i + 1).min(self.rows.len() - 1));
                self.move_to(i, now);
                Action::None
            }
            'k' if !self.rows.is_empty() => {
                let i = self.index.map_or(0, |i| i.saturating_sub(1));
                self.move_to(i, now);
                Action::None
            }
            c @ ('y' | 'n') => {
                let Some(row) = row else { return Action::None };
                // Never approve what the user couldn't see in full, and ignore
                // keys that may have been aimed at a row that just moved.
                let visible = c == 'n' || visible_input(&row, width).is_some();
                let settled = self
                    .cursor_since
                    .is_some_and(|since| now.duration_since(since) >= SETTLE);
                match (row.request_id, visible, settled) {
                    (Some(id), true, true) => Action::Decide(id, c == 'y'),
                    _ => Action::None,
                }
            }
            _ => Action::None,
        }
    }

    pub fn view<'a>(&'a self, connected: bool, status: Option<&'a str>) -> View<'a> {
        View {
            rows: &self.rows,
            selected: self.index,
            connected,
            overlay: self.overlay,
            status,
        }
    }
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    rx: &Feed,
    control: &mut UnixStream,
    bell: bool,
    config_error: Option<String>,
) -> Result<()> {
    let mut app = App::default();
    let mut connected = true;
    let mut status: Option<(String, Instant)> = None;
    // Jumps run off the UI thread: osascript may wait on a macOS permission prompt.
    let (jump_tx, jump_rx) = mpsc::channel::<String>();
    loop {
        while let Ok(update) = rx.try_recv() {
            match update {
                Some(rows) => {
                    if bell && new_requests(&app.rows, &rows) > 0 {
                        use std::io::Write as _;
                        let mut out = std::io::stdout();
                        let _ = out.write_all(b"\x07").and_then(|_| out.flush());
                    }
                    app.update(rows, Instant::now());
                }
                None => connected = false,
            }
        }
        while let Ok(message) = jump_rx.try_recv() {
            status = Some((message, Instant::now()));
        }
        terminal.draw(|f| {
            let shown = status
                .as_ref()
                .filter(|(_, at)| at.elapsed() < STATUS_FOR)
                .map(|(m, _)| m.as_str());
            draw(f, &app.view(connected, shown));
            if let Some(e) = &config_error {
                let area = f.area();
                let line = Rect::new(0, area.height.saturating_sub(1), area.width, 1);
                // TOML errors span lines; the cause is the last one.
                let cause = e.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or(e);
                f.render_widget(
                    Paragraph::new(format!(
                        " config ignored: {} (see `tapin doctor`)",
                        cause.trim()
                    ))
                    .style(Style::new().fg(Color::Red)),
                    line,
                );
            }
        })?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        let ch = match key.code {
            KeyCode::Char(c) => c,
            KeyCode::Enter => '\n',
            KeyCode::Esc => '\x1b',
            KeyCode::Down => 'j',
            KeyCode::Up => 'k',
            _ => continue,
        };
        let width = terminal.size()?.width;
        match app.key(ch, width, Instant::now()) {
            Action::None => {}
            Action::Quit => return Ok(()),
            Action::Decide(id, allow) => decide(control, id, allow),
            Action::Jump(row) => {
                status = Some((
                    format!("Jumping to {} {}…", row.agent.name(), row.project),
                    Instant::now(),
                ));
                let tx = jump_tx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(crate::jump::jump(&row));
                });
            }
        }
    }
}

/// The row to select after an update:
/// - stay on a permission request the user is reading;
/// - otherwise go to the longest-waiting request, so `y` works without hunting;
/// - otherwise follow the same session wherever it moved;
/// - the top row if nothing was chosen yet, and nothing if the chosen session
///   is gone, so the cursor never silently lands on an unrelated session.
///
/// Moving onto a request is safe because its full input is shown under the
/// cursor and `y` is ignored for `SETTLE` after the row changes.
pub fn reselect(rows: &[Row], selected: Option<&str>) -> Option<usize> {
    let current = selected.and_then(|id| rows.iter().position(|r| r.session_id == id));
    if let Some(i) = current {
        if rows[i].state == State::NeedsPermission {
            return Some(i);
        }
    }
    // Rows are sorted most urgent first, so the first request is the oldest one.
    if let Some(i) = rows.iter().position(|r| r.state == State::NeedsPermission) {
        return Some(i);
    }
    match selected {
        Some(_) => current,
        None => (!rows.is_empty()).then_some(0),
    }
}

/// How many requests in `now` weren't in `before` (to ring the bell once each).
pub fn new_requests(before: &[Row], now: &[Row]) -> usize {
    let seen: Vec<u64> = before.iter().filter_map(|r| r.request_id).collect();
    now.iter()
        .filter_map(|r| r.request_id)
        .filter(|id| !seen.contains(id))
        .count()
}

/// How long a status message (e.g. a jump result) stays in the footer.
const STATUS_FOR: Duration = Duration::from_secs(5);

/// How long the row under the cursor must stay put before `y`/`n` count.
const SETTLE: Duration = Duration::from_millis(500);

fn decide(control: &mut UnixStream, request_id: u64, allow: bool) {
    let request = Request::Decide { request_id, allow };
    if let Ok(mut line) = serde_json::to_string(&request) {
        line.push('\n');
        let _ = control.write_all(line.as_bytes());
    }
}

/// What sits on top of the inbox.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Overlay {
    #[default]
    None,
    Details,
    Help,
}

/// Everything one frame shows.
pub struct View<'a> {
    pub rows: &'a [Row],
    pub selected: Option<usize>,
    pub connected: bool,
    pub overlay: Overlay,
    /// A transient message (e.g. the result of a jump) shown in place of the footer.
    pub status: Option<&'a str>,
}

pub fn draw(f: &mut Frame, view: &View) {
    let rows = view.rows;
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(f.area());

    let waiting = rows
        .iter()
        .filter(|r| matches!(r.state, State::NeedsPermission | State::Done))
        .count();
    let status = if view.connected {
        Span::raw(format!(" {} agents · {waiting} waiting on you", rows.len()))
    } else {
        Span::styled(
            " disconnected from daemon · restart tapin",
            Style::new().fg(Color::Red).bold(),
        )
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" tapin ", Style::new().bold().reversed()),
            status,
        ])),
        header,
    );

    if rows.is_empty() {
        f.render_widget(
            Paragraph::new("\n  No agent sessions yet. Run `tapin init`, then start an agent.")
                .dim(),
            body,
        );
    } else {
        let items: Vec<ListItem> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| row_item(r, Some(i) == view.selected, body.width))
            .collect();
        let mut state = ListState::default().with_selected(view.selected);
        f.render_stateful_widget(
            List::new(items)
                .highlight_symbol("▶ ")
                .highlight_style(Style::new().bold()),
            body,
            &mut state,
        );
    }

    let footer_text = match view.status {
        Some(message) => Paragraph::new(format!(" {message}")).style(Style::new().fg(Color::Cyan)),
        // Nothing reports through hooks yet: say what installing them adds.
        None if !rows.is_empty() && rows.iter().all(|r| r.passive) => Paragraph::new(
            " passive mode (no hooks) · run `tapin init` to approve here and get live status",
        )
        .style(Style::new().fg(Color::Yellow)),
        None => Paragraph::new(" ⏎ jump · y/n answer · d details · ? help · q quit").dim(),
    };
    f.render_widget(footer_text, footer);

    match view.overlay {
        Overlay::None => {}
        Overlay::Help => draw_overlay(f, body, " keys ", help_lines()),
        Overlay::Details => {
            if let Some(row) = view.selected.and_then(|i| rows.get(i)) {
                draw_overlay(f, body, " details ", details_lines(row));
            }
        }
    }
}

fn draw_overlay(f: &mut Frame, area: Rect, title: &str, lines: Vec<Line<'static>>) {
    let area = area.inner(ratatui::layout::Margin::new(2, 1));
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(ratatui::widgets::Wrap { trim: false })
            .block(
                ratatui::widgets::Block::bordered()
                    .title(title.to_string())
                    .title_bottom(" Esc close "),
            ),
        area,
    );
}

fn help_lines() -> Vec<Line<'static>> {
    let keys = [
        ("Enter", "jump to the selected agent's pane or window"),
        ("y", "approve the selected permission request"),
        ("n", "deny it"),
        ("d", "details: full message, input, where it runs"),
        ("j/k ↑/↓", "move"),
        ("?", "this help"),
        ("Esc", "close this view"),
        ("q", "quit (the daemon keeps running)"),
    ];
    keys.iter()
        .map(|(k, what)| {
            Line::from(vec![
                Span::styled(format!(" {k:<9}"), Style::new().bold()),
                Span::raw(what.to_string()),
            ])
        })
        .collect()
}

fn details_lines(r: &Row) -> Vec<Line<'static>> {
    let field = |name: &str, value: String| {
        Line::from(vec![
            Span::styled(format!(" {name:<9}"), Style::new().bold()),
            Span::raw(value),
        ])
    };
    let loc = &r.location;
    let place = match (&loc.tmux_pane, &loc.tty, &loc.term_program) {
        (Some(pane), _, _) => format!("tmux pane {pane}"),
        (None, Some(tty), Some(p)) => format!("{p} on {tty}"),
        (None, Some(tty), None) => tty.clone(),
        (None, None, Some(p)) => p.clone(),
        (None, None, None) => "unknown".into(),
    };
    let mut lines = vec![
        field("agent", format!("{} · {}", r.agent.name(), r.project)),
        field(
            "state",
            format!("{} for {}", r.state.label(), age(r.age_secs)),
        ),
        field("cwd", r.cwd.clone()),
        field("runs in", place),
        field("session", r.session_id.clone()),
    ];
    if let Some(model) = &r.model {
        lines.insert(3, field("model", model.clone()));
    }
    if let Some(input) = &r.request_input {
        lines.push(Line::raw(""));
        lines.push(Line::styled(" waiting for approval:", Style::new().bold()));
        lines.extend(input.lines().map(|l| Line::raw(format!("   {l}"))));
    }
    if !r.last_message.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(" last message:", Style::new().bold()));
        lines.extend(r.last_message.lines().map(|l| Line::raw(format!("   {l}"))));
    }
    lines
}

/// Most lines of a pending tool input shown under the selected row.
const INPUT_LINES: usize = 8;

/// Left margin plus the `│ ` gutter in front of each input line.
const INPUT_INDENT: &str = "      │ ";

/// The pending input wrapped to the screen width, or None if it doesn't fit in
/// `INPUT_LINES`. `y` is only offered when the user can see all of it.
fn visible_input(r: &Row, width: u16) -> Option<Vec<String>> {
    // The list's highlight symbol takes 2 more columns.
    let room = (width as usize)
        .saturating_sub(INPUT_INDENT.chars().count() + 2)
        .max(10);
    let input = r.request_input.as_deref().unwrap_or_default();
    let lines: Vec<String> = input.lines().flat_map(|l| wrap(l, room)).collect();
    (lines.len() <= INPUT_LINES).then_some(lines)
}

fn row_item(r: &Row, selected: bool, width: u16) -> ListItem<'static> {
    let (label, color) = match r.state {
        State::NeedsPermission => ("NEEDS", Color::Red),
        State::Done => ("DONE", Color::Green),
        State::Working => ("WORK", Color::Yellow),
        State::Idle => ("IDLE", Color::DarkGray),
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{label:<6}"), Style::new().fg(color).bold()),
        Span::styled(format!("{:<7}", r.agent.name()), Style::new().dim()),
        Span::styled(format!("{:<14} ", r.project), Style::new().bold()),
        Span::raw(r.detail.clone()),
        Span::styled(format!("  {}", age(r.age_secs)), Style::new().dim()),
    ])];
    // Show exactly what `y` would approve, not just the one-line summary.
    if selected && r.state == State::NeedsPermission {
        // Wrap rather than truncate: a cut-off command could hide what `y` approves.
        let shown = visible_input(r, width);
        match &shown {
            Some(input) => {
                for l in input {
                    lines.push(Line::styled(
                        format!("{INPUT_INDENT}{l}"),
                        Style::new().fg(Color::Cyan),
                    ));
                }
            }
            None => lines.push(Line::styled(
                format!("{INPUT_INDENT}too long to show here; review it in the agent's terminal"),
                Style::new().fg(Color::Red),
            )),
        }
        let hint = if r.request_id.is_some() && shown.is_some() {
            "      y approve · n deny"
        } else if r.request_id.is_some() {
            "      n deny (approve in the agent, where you can see all of it)"
        } else {
            if r.passive {
                "      answer in the agent's terminal · run `tapin init` to answer here"
            } else {
                "      answer in the agent's terminal (tapin's hook is no longer waiting)"
            }
        };
        lines.push(Line::styled(hint, Style::new().dim()));
        if r.request_id.is_some() && r.agent.prompt_waits_for_hook() {
            // Its own prompt only appears after tapin answers or gives up.
            lines.push(Line::styled(
                format!(
                    "      {} is paused until you answer here (or it asks in its terminal shortly)",
                    title_case(r.agent.name())
                ),
                Style::new().fg(Color::Yellow),
            ));
        }
    }
    ListItem::new(lines)
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// Splits `line` into chunks of at most `width` characters.
fn wrap(line: &str, width: usize) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars.chunks(width).map(|c| c.iter().collect()).collect()
}

fn age(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        _ => format!("{}h", secs / 3600),
    }
}
