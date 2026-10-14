# Changelog

## 0.2.0 - 2026-10-14

### Added
- **`/` filter.** Type to show only matching sessions (project, agent, detail or folder). Enter keeps the filter, Esc clears it. While you type, every key is text, so nothing is approved by accident. The header still counts sessions the filter hides.
- **`g` groups rows by folder**, with the most urgent group first.
- **Context column.** `ctx 64%` turns yellow from 80% and red from 95%. It's read from the agent's own transcript (Claude Code) or rollout (Codex). When the window size isn't known for sure, the row shows the size instead (`ctx 132k`).
- **`hide_dirs` config** leaves sessions in chosen folders out of the inbox. By default it hides claude-mem's background observer.

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
