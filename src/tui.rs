//! `tapin`: the inbox TUI. A thin view over daemon snapshots.

use crate::event::AgentKind;
use crate::inbox::{Row, State};
use crate::socket::{Request, Update};
use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
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

/// Messages from the daemon; `None` means the connection is gone.
type Feed = mpsc::Receiver<Option<Update>>;

fn subscribe(mut stream: UnixStream, tx: mpsc::Sender<Option<Update>>) -> Result<()> {
    let mut line = serde_json::to_string(&Request::Subscribe)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            if let Ok(update) = serde_json::from_str::<Update>(&line) {
                if tx.send(Some(update)).is_err() {
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
    /// Every row from the latest snapshot.
    all: Vec<Row>,
    /// The rows on screen: `all` narrowed by the filter. Keys act only on these.
    rows: Vec<Row>,
    /// The `/` filter, if any, and whether the user is still typing it.
    filter: Option<String>,
    typing: bool,
    /// `g`: rows from the same folder sit together.
    grouped: bool,
    /// The daemon won't take answers from this inbox (#12).
    read_only: bool,
    /// `x` pressed on a background session: (attach id, name, when), awaiting `y`.
    confirm_stop: Option<(String, String, Instant)>,
    /// The session the user chose (kept across re-sorts and while it vanishes).
    selected: Option<String>,
    index: Option<usize>,
    overlay: Overlay,
    /// The request under the cursor, and since when; `y`/`n` wait for SETTLE.
    cursor_request: Option<u64>,
    cursor_since: Option<Instant>,
    /// Requests already on screen before the latest snapshot, by session and
    /// request id: only one that is new may pull the cursor.
    known_needs: Vec<Need>,
}

/// What a key asks the event loop to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Quit,
    Decide(u64, bool),
    Jump(Box<Row>),
    /// Stop this Claude background session (`claude stop <id>`).
    Stop(String),
}

impl App {
    /// A new snapshot from the daemon.
    pub fn update(&mut self, rows: Vec<Row>, now: Instant) {
        // Requests hidden by the filter count as known: they never pull the
        // cursor, and stay known so they don't pull it once the filter is gone.
        self.known_needs = needs_of(&self.all);
        self.all = rows;
        // Nothing left to stop if that session is gone.
        if let Some((id, _, _)) = &self.confirm_stop {
            let present = self
                .all
                .iter()
                .any(|r| r.location.claude_background.as_deref() == Some(id.as_str()));
            if !present {
                self.confirm_stop = None;
            }
        }
        self.refilter(now);
    }

    /// The rows on screen.
    pub fn visible(&self) -> &[Row] {
        &self.rows
    }

    /// True if `rows` brings a request this inbox hasn't seen, hidden or not.
    pub fn rings_for(&self, rows: &[Row]) -> bool {
        new_requests(&self.all, rows) > 0
    }

    /// The daemon said this inbox may not answer requests.
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    /// The session `x` asked to stop, while waiting for `y`.
    pub fn confirming_stop(&self) -> Option<&str> {
        self.confirm_stop.as_ref().map(|(_, name, _)| name.as_str())
    }

    /// True when rows are grouped by folder (`g`).
    pub fn grouped(&self) -> bool {
        self.grouped
    }

    /// True while the user is typing the `/` filter.
    pub fn typing(&self) -> bool {
        self.typing
    }

    /// The `/` filter, if one is set.
    pub fn filter(&self) -> Option<&str> {
        self.filter.as_deref()
    }

    fn refilter(&mut self, now: Instant) {
        let query = self.filter.as_deref().unwrap_or_default().to_lowercase();
        self.rows = self
            .all
            .iter()
            .filter(|r| query.is_empty() || matches_filter(r, &query))
            .cloned()
            .collect();
        if self.grouped {
            self.rows = group_by_folder(std::mem::take(&mut self.rows));
        }
        self.reindex(now);
    }

    /// A key while typing the filter: text only, never an action.
    fn filter_key(&mut self, key: char, now: Instant) -> Action {
        match key {
            '\n' => self.typing = false,
            '\x1b' => {
                self.typing = false;
                self.filter = None;
            }
            '\x08' => {
                if let Some(f) = &mut self.filter {
                    f.pop();
                }
            }
            c if !c.is_control() => self.filter.get_or_insert_with(String::new).push(c),
            _ => {}
        }
        if self.filter.as_deref() == Some("") && !self.typing {
            self.filter = None;
        }
        self.filter_changed(now);
        Action::None
    }

    /// The user changed the filter: rows appearing don't count as new requests,
    /// and if their row is now hidden the cursor starts over on what's shown.
    fn filter_changed(&mut self, now: Instant) {
        self.known_needs = needs_of(&self.all);
        let query = self.filter.as_deref().unwrap_or_default().to_lowercase();
        let hidden = |id: &str| {
            self.all
                .iter()
                .find(|r| r.session_id == id)
                .is_some_and(|r| !query.is_empty() && !matches_filter(r, &query))
        };
        if self.selected.as_deref().is_some_and(hidden) {
            self.selected = None;
        }
        self.refilter(now);
    }

    pub fn overlay(&self) -> Overlay {
        self.overlay
    }

    fn reindex(&mut self, now: Instant) {
        self.index = reselect(&self.rows, self.selected.as_deref(), &self.known_needs);
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
            // The user moved on purpose: no request already shown may pull them back.
            self.known_needs = needs_of(&self.rows);
            self.reindex(now);
        }
    }

    /// Handles one key: chars as typed, `\n` for Enter, `\x1b` for Esc,
    /// `j`/`k` for the arrows. `width` is the screen width, for what fits.
    pub fn key(&mut self, key: char, width: u16, now: Instant) -> Action {
        // A pending stop takes the next key: `y` stops, anything else cancels.
        // A prompt left open expires, and the late key does nothing: it may
        // have been meant for something else (like approving a request).
        if let Some((id, _, at)) = self.confirm_stop.take() {
            let fresh = now.duration_since(at) < STOP_PROMPT_FOR;
            return if key == 'y' && fresh {
                Action::Stop(id)
            } else {
                Action::None
            };
        }
        if self.typing {
            return self.filter_key(key, now);
        }
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
            'x' => {
                // Like `y`: ignored right after the cursor moved under the user.
                let settled = self
                    .cursor_since
                    .is_some_and(|since| now.duration_since(since) >= SETTLE);
                let target = row.as_ref().filter(|_| settled).and_then(|r| {
                    let id = r.location.claude_background.clone()?;
                    Some((id, r.project.clone(), now))
                });
                self.confirm_stop = target;
                Action::None
            }
            'g' => {
                self.grouped = !self.grouped;
                self.refilter(now);
                Action::None
            }
            '/' => {
                self.typing = true;
                self.filter.get_or_insert_with(String::new);
                Action::None
            }
            '\x1b' if self.filter.is_some() => {
                self.filter = None;
                self.filter_changed(now);
                Action::None
            }
            '?' => {
                self.overlay = Overlay::Help;
                Action::None
            }
            'd' if row.is_some() => {
                self.overlay = Overlay::Details;
                Action::None
            }
            '\n' => row.map_or(Action::None, |r| Action::Jump(Box::new(r))),
            '\t' => {
                // The next session waiting on the user, after the cursor, wrapping.
                let n = self.rows.len();
                let start = self.index.map_or(0, |i| i + 1);
                let next = (0..n)
                    .map(|k| (start + k) % n)
                    .find(|&i| matches!(self.rows[i].state, State::NeedsPermission | State::Done));
                if let Some(i) = next {
                    self.move_to(i, now);
                }
                Action::None
            }
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
            'y' | 'n' if self.read_only => Action::None,
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
            filter: self.filter.as_deref(),
            typing: self.typing,
            grouped: self.grouped,
            read_only: self.read_only,
            hidden: self.all.len() - self.rows.len(),
            hidden_waiting: waiting(&self.all) - waiting(&self.rows),
            hooks: &[],
            // Shown only while it can still be answered.
            confirm_stop: self
                .confirm_stop
                .as_ref()
                .filter(|(_, _, at)| at.elapsed() < STOP_PROMPT_FOR)
                .map(|(id, name, _)| (id.as_str(), name.as_str())),
        }
    }
}

/// Rows waiting on the user: a request or a finished turn.
fn waiting(rows: &[Row]) -> usize {
    rows.iter()
        .filter(|r| matches!(r.state, State::NeedsPermission | State::Done))
        .count()
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    rx: &Feed,
    control: &mut UnixStream,
    bell: bool,
    config_error: Option<String>,
) -> Result<()> {
    let hooks = crate::setup::agents_with_hooks();
    let mut app = App::default();
    // Answers only once the daemon says this inbox may answer: a daemon from
    // before that check (v0.1) never says so, and must be restarted.
    app.set_read_only(true);
    let mut connected = true;
    let mut status: Option<(String, Instant)> = None;
    // Jumps run off the UI thread: osascript may wait on a macOS permission prompt.
    let (jump_tx, jump_rx) = mpsc::channel::<String>();
    loop {
        while let Ok(update) = rx.try_recv() {
            match update {
                Some(Update::Access { can_answer }) => app.set_read_only(!can_answer),
                Some(Update::Rows { rows }) => {
                    if bell && app.rings_for(&rows) {
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
            draw(
                f,
                &View {
                    hooks: &hooks,
                    ..app.view(connected, shown)
                },
            );
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
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if app.typing() && !passes_while_typing(key.code, ctrl) {
            continue;
        }
        let ch = match key.code {
            KeyCode::Char(c) => c,
            KeyCode::Enter => '\n',
            KeyCode::Tab => '\t',
            KeyCode::Backspace => '\x08',
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
            Action::Stop(id) => {
                status = Some((format!("Stopping {id}…"), Instant::now()));
                let tx = jump_tx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(crate::jump::stop(&id));
                });
            }
        }
    }
}

/// While typing the filter, only text, Backspace, Enter and Esc reach it:
/// arrows would arrive as `j`/`k` and Ctrl-keys as plain letters.
pub fn passes_while_typing(code: KeyCode, ctrl: bool) -> bool {
    !ctrl
        && matches!(
            code,
            KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Enter | KeyCode::Esc
        )
}

/// The row to select after an update:
/// - stay on a permission request the user is reading;
/// - go to a session that newly needs a permission (not in `known_needs`), so
///   `y` works without hunting; requests already on screen don't pull the
///   cursor, or the user could never move off them;
/// - otherwise follow the same session wherever it moved;
/// - if the chosen session is gone, offer the oldest request, else nothing, so
///   the cursor never silently lands on an unrelated session;
/// - the top row (or the oldest request) if nothing was chosen yet.
///
/// Moving onto a request is safe because its full input is shown under the
/// cursor and `y` is ignored for `SETTLE` after the row changes.
pub fn reselect(rows: &[Row], selected: Option<&str>, known_needs: &[Need]) -> Option<usize> {
    let current = selected.and_then(|id| rows.iter().position(|r| r.session_id == id));
    if let Some(i) = current {
        if rows[i].state == State::NeedsPermission {
            return Some(i);
        }
    }
    // The oldest request by its own age: grouping can reorder rows.
    let needs = |r: &Row| r.state == State::NeedsPermission;
    if let Some(i) = oldest(rows, |r| needs(r) && !known_needs.contains(&need(r))) {
        return Some(i);
    }
    match (selected, current) {
        (_, Some(i)) => Some(i),
        (Some(_), None) => oldest(rows, needs),
        (None, None) => oldest(rows, needs).or((!rows.is_empty()).then_some(0)),
    }
}

/// The longest-waiting row matching `pred` (the first one on a tie).
fn oldest(rows: &[Row], pred: impl Fn(&Row) -> bool) -> Option<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, r)| pred(r))
        .rev()
        .max_by_key(|(_, r)| r.age_secs)
        .map(|(i, _)| i)
}

/// A permission wait as the cursor rule sees it: the session and its request
/// (none for a request tapin can't answer, e.g. one seen passively).
pub type Need = (String, Option<u64>);

fn need(r: &Row) -> Need {
    (r.session_id.clone(), r.request_id)
}

/// The last part of a folder path, for a group header.
fn folder_name(cwd: &str) -> &str {
    match cwd.trim_end_matches('/').rsplit('/').next() {
        Some(name) if !name.is_empty() => name,
        _ if cwd.is_empty() => "(unknown folder)",
        _ => "/",
    }
}

/// Rows from one folder together, groups in order of their most urgent row
/// (rows arrive most urgent first), each group keeping that order.
fn group_by_folder(rows: Vec<Row>) -> Vec<Row> {
    let mut order: Vec<String> = Vec::new();
    for r in &rows {
        let key = group_key(&r.cwd);
        if !order.iter().any(|k| k == key) {
            order.push(key.to_string());
        }
    }
    let mut grouped = rows;
    grouped.sort_by_key(|r| order.iter().position(|k| k == group_key(&r.cwd)));
    grouped
}

/// The folder a row is grouped under (`/w/api/` and `/w/api` are one).
fn group_key(cwd: &str) -> &str {
    match cwd.trim_end_matches('/') {
        "" if cwd.starts_with('/') => "/",
        key => key,
    }
}

/// A group header's name: the folder's last part, or its last two when
/// another group shown has the same last part (`/a/api` vs `/b/api`).
fn folder_label(rows: &[Row], key: &str) -> String {
    let name = folder_name(key);
    let clash = rows.iter().any(|r| {
        let other = group_key(&r.cwd);
        other != key && folder_name(other) == name
    });
    match key.rsplit('/').nth(1).filter(|p| clash && !p.is_empty()) {
        Some(parent) => format!("{parent}/{name}"),
        None => name.to_string(),
    }
}

/// Case-insensitive substring match; `query` is already lowercase.
fn matches_filter(r: &Row, query: &str) -> bool {
    [
        r.project.as_str(),
        r.agent.name(),
        r.detail.as_str(),
        r.cwd.as_str(),
    ]
    .iter()
    .any(|field| field.to_lowercase().contains(query))
}

/// The permission waits in `rows`.
fn needs_of(rows: &[Row]) -> Vec<Need> {
    rows.iter()
        .filter(|r| r.state == State::NeedsPermission)
        .map(need)
        .collect()
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

/// How long a stop prompt waits for its `y`.
const STOP_PROMPT_FOR: Duration = Duration::from_secs(5);

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
#[derive(Default)]
pub struct View<'a> {
    pub rows: &'a [Row],
    pub selected: Option<usize>,
    pub connected: bool,
    pub overlay: Overlay,
    /// A transient message (e.g. the result of a jump) shown in place of the footer.
    pub status: Option<&'a str>,
    /// The `/` filter, and whether it's still being typed.
    pub filter: Option<&'a str>,
    pub typing: bool,
    /// Rows are grouped by folder: draw a header before each group.
    pub grouped: bool,
    /// The daemon won't take answers from this inbox.
    pub read_only: bool,
    /// Agents whose settings already run tapin's hooks (sessions started
    /// before `tapin init` still show passively until restarted).
    pub hooks: &'a [AgentKind],
    /// The background session `x` asked to stop (id, name), awaiting `y`.
    pub confirm_stop: Option<(&'a str, &'a str)>,
    /// Rows the filter hides, and how many of those wait on the user.
    pub hidden: usize,
    pub hidden_waiting: usize,
}

pub fn draw(f: &mut Frame, view: &View) {
    let rows = view.rows;
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(f.area());

    // Counts cover hidden rows too, so a filter never hides that something waits.
    let waiting = waiting(rows) + view.hidden_waiting;
    let total = rows.len() + view.hidden;
    let status = if view.connected {
        Span::raw(format!(" {total} agents · {waiting} waiting on you"))
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
            match view.filter.filter(|f| !f.is_empty()) {
                Some(f) => Span::styled(
                    format!(" · filter: {f} · {} hidden", view.hidden),
                    Style::new().fg(Color::Cyan),
                ),
                None => Span::raw(""),
            },
        ])),
        header,
    );

    if rows.is_empty() && view.hidden > 0 {
        f.render_widget(
            Paragraph::new("\n  No session matches the filter. Esc clears it.").dim(),
            body,
        );
    } else if rows.is_empty() {
        f.render_widget(
            Paragraph::new("\n  No agent sessions yet. Run `tapin init`, then start an agent.")
                .dim(),
            body,
        );
    } else {
        let mut items: Vec<ListItem> = Vec::new();
        // Headers are drawn only: the selection maps rows to list positions.
        let mut selected_item = None;
        for (i, r) in rows.iter().enumerate() {
            let key = group_key(&r.cwd);
            let new_group = i == 0 || group_key(&rows[i - 1].cwd) != key;
            if view.grouped && new_group {
                let n = rows[i..]
                    .iter()
                    .take_while(|o| group_key(&o.cwd) == key)
                    .count();
                items.push(ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("▸ {}", folder_label(rows, key)),
                        Style::new().bold(),
                    ),
                    Span::styled(format!("  ({n})"), Style::new().dim()),
                ])));
            }
            if Some(i) == view.selected {
                selected_item = Some(items.len());
            }
            let hooked = view.hooks.contains(&r.agent);
            items.push(row_item(r, Some(i) == view.selected, body.width, hooked));
        }
        let mut state = ListState::default().with_selected(selected_item);
        f.render_stateful_widget(
            List::new(items)
                .highlight_symbol("▶ ")
                .highlight_style(Style::new().bold()),
            body,
            &mut state,
        );
    }

    let footer_text = match view.status {
        _ if view.confirm_stop.is_some() => {
            let (id, name) = view.confirm_stop.unwrap_or_default();
            Paragraph::new(format!(
                " Stop {name} ({id})? Its conversation is kept (claude attach reopens it). \
                 y stops · any other key cancels"
            ))
        }
        .style(Style::new().fg(Color::Yellow).bold()),
        _ if view.typing => Paragraph::new(format!(
            " /{}▏  ⏎ keep · esc clear",
            view.filter.unwrap_or_default()
        ))
        .style(Style::new().fg(Color::Cyan)),
        Some(message) => Paragraph::new(format!(" {message}")).style(Style::new().fg(Color::Cyan)),
        None if view.read_only => Paragraph::new(
            " read-only: the daemon won't take answers here (started under an agent, \
             or a daemon from another tapin: restart it with pkill -f \"tapin daemon\")",
        )
        .style(Style::new().fg(Color::Yellow)),
        // Nothing reports through hooks yet: say what installing them adds.
        None if !rows.is_empty() && rows.iter().all(|r| r.passive) && view.hooks.is_empty() => {
            Paragraph::new(
                " passive mode (no hooks) · run `tapin init` to approve here and get live status",
            )
            .style(Style::new().fg(Color::Yellow))
        }
        None => Paragraph::new(" ⏎ jump · ⇥ next · / filter · y/n answer · ? help · q quit").dim(),
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
        (
            "Tab",
            "next agent waiting on you (a request or a finished turn)",
        ),
        ("y", "approve the selected permission request"),
        ("n", "deny it"),
        ("d", "details: full message, input, where it runs"),
        ("/", "filter by project, agent, detail or folder"),
        ("g", "group rows by folder (again to ungroup)"),
        (
            "x",
            "stop a Claude background session (asks first; claude rm deletes for good)",
        ),
        ("j/k ↑/↓", "move"),
        ("?", "this help"),
        ("Esc", "close this view, or clear the filter"),
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
        field(
            "agent",
            format!("{} {} · {}", glyph(r.agent), r.agent.name(), r.project),
        ),
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

/// Each agent's mark, as the agent's own CLI prints it: Claude Code's `✻`,
/// Codex's `>_` prompt, Gemini CLI's `✦`. Plain single-width text, not logos.
pub fn glyph(agent: AgentKind) -> &'static str {
    match agent {
        AgentKind::Claude => "✻",
        AgentKind::Codex => ">_",
        AgentKind::Gemini => "✦",
    }
}

fn glyph_style(agent: AgentKind) -> Style {
    match agent {
        AgentKind::Claude => Style::new().fg(Color::Rgb(0xD9, 0x77, 0x57)),
        AgentKind::Codex => Style::new().bold(),
        AgentKind::Gemini => Style::new().fg(Color::Rgb(0x47, 0x96, 0xE4)),
    }
}

/// `ctx 64%` when the window is known (dim, yellow from 80%, red from 95%),
/// else the size, `ctx 132k`; nothing when unknown.
fn context_span(context: Option<crate::context::Context>) -> Span<'static> {
    let Some(c) = context else {
        return Span::raw("");
    };
    match c.percent() {
        Some(p) => {
            let style = match p {
                95.. => Style::new().fg(Color::Red),
                80.. => Style::new().fg(Color::Yellow),
                _ => Style::new().dim(),
            };
            Span::styled(format!("  ctx {p}%"), style)
        }
        None if c.used >= 1000 => {
            Span::styled(format!("  ctx {}k", c.used / 1000), Style::new().dim())
        }
        None => Span::raw(""),
    }
}

/// `hooked`: the row's agent already runs tapin's hooks, so a passive row is
/// an older session, not a missing `tapin init`.
fn row_item(r: &Row, selected: bool, width: u16, hooked: bool) -> ListItem<'static> {
    let (label, color) = match r.state {
        State::NeedsPermission => ("NEEDS", Color::Red),
        State::Done => ("DONE", Color::Green),
        State::Working => ("WORK", Color::Yellow),
        State::Idle => ("IDLE", Color::DarkGray),
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{label:<6}"), Style::new().fg(color).bold()),
        Span::styled(format!("{:>2} ", glyph(r.agent)), glyph_style(r.agent)),
        Span::styled(format!("{:<7}", r.agent.name()), Style::new().dim()),
        Span::styled(format!("{:<14} ", r.project), Style::new().bold()),
        Span::raw(r.detail.clone()),
        Span::styled(format!("  {}", age(r.age_secs)), Style::new().dim()),
        context_span(r.context),
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
        let background = r.location.claude_background.as_deref();
        let hint: String = if r.request_id.is_some() && shown.is_some() {
            "      y approve · n deny".into()
        } else if r.request_id.is_some() {
            "      n deny (approve in the agent, where you can see all of it)".into()
        } else if let Some(id) = background {
            format!(
                "      a Claude background session · Enter opens it (claude attach {id}) · x stops it"
            )
        } else if r.passive && hooked {
            "      answer in the agent's terminal · this session started before tapin's \
             hooks were installed; new ones can be answered here"
                .into()
        } else if r.passive {
            "      answer in the agent's terminal · run `tapin init` to answer here".into()
        } else {
            "      answer in the agent's terminal (tapin's hook is no longer waiting)".into()
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
