# Changelog

## 0.2.1 - 2026-10-15

### Added
- **Claude background sessions open with Enter.** Sessions from `claude agents` have no terminal of their own. In tmux, Enter opens a new window in the session's folder running `claude attach <id>`; elsewhere, tapin shows that command. The row says so instead of "answer in the agent's terminal".
- **`x` stops a Claude background session** (`claude stop <id>`; its conversation is kept and `claude attach` reopens it). It asks first: `y` stops, any other key cancels, and the prompt expires after 5 seconds. To delete a session for good, use `claude rm <id>` yourself.
- Outside tmux on macOS, Enter on a background session opens a new iTerm2 or Terminal window running `claude attach <id>`.
- **Answer Claude's questions from tapin.** When Claude Code asks with its question tool, the row shows the question and numbered options. For a single question with one answer, press `1`-`N` to answer (the same guards as `y` apply). Other questions are shown, and you answer them in Claude's window. This relies on behaviour verified on Claude Code 2.1.299 that isn't documented for permission hooks yet.

### Fixed
- `y` (from any inbox, including 0.2.0) never approves a question without an answer: the hook leaves Claude's own question up instead.

### Upgrading
- Restart the daemon once (`pkill -f "tapin daemon"`). Answering questions needs the new daemon. Until then, the number keys do nothing.

## 0.2.0 - 2026-10-14

### Added
- **`/` filter.** Type to show only matching sessions (project, agent, detail or folder). Enter keeps the filter, Esc clears it. While you type, every key is text, so nothing is approved by accident. The header still counts sessions the filter hides.
- **`g` groups rows by folder**, with the most urgent group first.
- **Context column.** `ctx 64%` turns yellow from 80% and red from 95%. It's read from the agent's own transcript (Claude Code) or rollout (Codex). When the window size isn't known for sure, the row shows the size instead (`ctx 132k`).
- **`hide_dirs` config** leaves sessions in chosen folders out of the inbox. By default it hides claude-mem's background observer.

### Fixed
- A session that started before `tapin init` no longer tells you to run `tapin init` (again). It now says the session predates the hooks and that new sessions can be answered from tapin.

### Security
- **Only a real tapin inbox can approve** ([#12](https://github.com/Thevi99/tapin/issues/12)).
  - The daemon checks the process at the other end of the socket. It must be your user, running the same tapin file, with no agent anywhere above it. So an agent can't approve its own request with `nc`, a script, or a tapin it launched.
  - Hooks check the daemon the same way, so a fake daemon can't approve either.
  - An inbox that fails the check is read-only and says so.
  - This guards against accidents and prompt injection, not a determined process running as your user. See the README.

### Upgrading
- After upgrading, quit any open tapin inbox and restart the daemon once: `pkill -f "tapin daemon"`, then run `tapin` again. Until you do, the new inbox stays read-only against the old daemon. New hooks also won't wait on the old daemon (they check its protocol), so agents ask in their own terminal meanwhile.

## 0.1.0 - 2026-10-14

First release.
- An inbox for Claude Code, Codex CLI and Gemini CLI, sorted by what needs you.
- `y`/`n` answers permission requests, Enter jumps to the agent's pane or tab, and `Tab` moves to the next agent waiting on you.
- Desktop notifications.
- `tapin status` for a tmux status bar.
- Zero-setup passive mode.
- `tapin init` / `uninstall` / `doctor`.
