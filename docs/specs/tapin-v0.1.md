# tapin v0.1 — an inbox for your AI coding agents

> Approve, jump, done.

## Problem Statement

I run several AI coding agents at once (Claude Code in three projects, Codex in another, sometimes Gemini) across tmux panes and terminal tabs. Most of the time they work alone, but every few minutes one of them stops and waits for me: it wants permission to run a command, it asked a question, or it finished and needs the next instruction.

I can't tell which one is waiting without alt-tabbing through every pane. Agents sit idle for minutes because I didn't notice, and when I do notice I lose focus hunting for the right window. Existing tools (agtop, agenttop) show what agents are *doing*, sorted by CPU or tokens, but they don't answer the question I actually have: **which agent needs me right now, and how do I answer it fastest?**

## Solution

`tapin` is a single-binary terminal app that I keep open in a small pane. It shows every running agent session as an **inbox sorted by urgency**:

1. **NEEDS YOU**: permission requests and questions, oldest first
2. **DONE**: finished turns waiting for my next instruction
3. **WORKING**: busy agents, with the current tool and context usage
4. **IDLE**: everything else

From the inbox I can press `y`/`n` to approve or deny a permission request without leaving the pane, press Enter to jump straight to that agent's tmux pane or terminal, and get an OS notification when something lands in NEEDS YOU.

Setup is one command (`tapin init`). It shows the exact hook changes it will make to each agent's config and asks before writing anything. `tapin uninstall` reverts them. Without hooks, tapin still runs in a passive, read-only mode that discovers sessions from agent transcript files and processes, so a first-time user sees something useful in under 30 seconds.

## User Stories

### First run and setup
1. As a new user, I want to install tapin with `brew install` or `cargo install`, so that I don't need to build from source.
2. As a new user, I want to run `tapin` with zero setup and immediately see my running agent sessions, so that I can judge whether it's worth configuring.
3. As a new user in passive mode, I want a clear one-line hint that `tapin init` unlocks approvals and live status, so that I know what I'm missing.
4. As a cautious user, I want `tapin init` to show a diff of every config file it will change before writing, so that I trust it with my agent settings.
5. As a cautious user, I want `tapin init` to back up each config file before modifying it, so that I can restore it manually if needed.
6. As a user, I want `tapin init` to detect which agents are installed (Claude Code, Codex, Gemini) and only offer hooks for those, so that it doesn't touch configs I don't use.
7. As a user, I want `tapin init` to be idempotent, so that running it twice doesn't duplicate hooks.
8. As a user, I want `tapin init` to preserve my existing hooks and settings, so that it never clobbers other tools' configuration.
9. As a user, I want `tapin uninstall` to remove exactly the hooks tapin added and nothing else, so that I can leave cleanly.
10. As a user, I want `tapin doctor` to tell me which agents are hooked, whether the daemon socket is reachable, and what's misconfigured, so that I can fix problems without reading source.

### The inbox
11. As a multi-agent user, I want sessions grouped into NEEDS YOU / DONE / WORKING / IDLE, so that the top of the screen is always what needs me.
12. As a user, I want NEEDS YOU sorted oldest-waiting first, so that no agent starves.
13. As a user, I want each row to show the agent kind, the project name (derived from cwd), the status detail and how long it has been in that state, so that I can triage at a glance.
14. As a user, I want permission rows to show the tool and a short summary of its input (e.g. `Bash(rm -rf dist/)`, `Edit(src/main.rs)`), so that I can decide without jumping.
15. As a user, I want question rows to show the question text, so that I know what the agent is asking.
16. As a user, I want DONE rows to show a one-line summary of the agent's last message, so that I remember what it finished.
17. As a user, I want WORKING rows to show the current tool and context-window usage when available, so that I can spot an agent about to run out of context.
18. As a user, I want a header that says how many agents are running and how many need me, so that I can glance at it from across the screen.
19. As a user, I want sessions that end to disappear from the inbox, and sessions whose process died to be marked and then removed, so that the list never fills with ghosts.
20. As a user, I want `j/k` and arrow keys to move the selection, and the selection to follow the same session when the list re-sorts, so that a new event doesn't make me act on the wrong row.
21. As a user, I want `d` to open a details view for the selected session (full tool input, full last message, cwd, model, session id), so that I can inspect before approving.
22. As a user, I want `?` to show all keybindings, so that I don't need the README.
23. As a user, I want the UI to stay readable in a narrow pane (around 60 columns), so that I can keep it docked beside my editor.
24. As a user, I want the UI to work in both light and dark terminal themes, so that it doesn't look broken.

### Acting on agents
25. As a user, I want to press `y` on a permission row to approve that single request, so that the agent continues immediately.
26. As a user, I want to press `n` on a permission row to deny it, so that the agent stops that action and keeps its session.
27. As a user, I want the approval to reach the agent within a second, so that tapin feels faster than switching windows.
28. As a user, I want a request I answered directly in the agent's own terminal to disappear from tapin, so that the two never disagree.
29. As a user, I want Enter to jump to the selected agent's tmux pane (switching session/window as needed), so that I can type a reply.
30. As a user outside tmux, I want Enter to at least bring the right terminal app to the front where the platform allows, and otherwise show the tty and cwd, so that jumping degrades gracefully.
31. As a user, I want an OS notification (macOS Notification Center / Linux notify-send) when a session enters NEEDS YOU, so that I notice even when tapin is hidden.
32. As a user, I want to turn notifications off with a flag or config option, so that tapin isn't noisy.
33. As a user, I want an optional terminal bell on NEEDS YOU, so that I can get alerts over SSH.

### Agent coverage
34. As a Claude Code user, I want full support: live status, current tool, permission approve/deny, questions, done summaries and context usage.
35. As a Codex user, I want live status, done summaries, and approve/deny wherever Codex's public hook interface allows it, so that both of my main agents are in one inbox.
36. As a Gemini CLI user, I want live status (working/done), so that Gemini sessions at least appear in the inbox.
37. As a user of an unsupported agent, I want tapin to ignore it cleanly rather than show garbage, so that the inbox stays trustworthy.

### Reliability and safety
38. As a user, I want my agents to never hang if tapin is not running, crashed, or slow, so that installing tapin can't break my workflow.
39. As a user, I want a pending permission request to fall back to the agent's normal in-terminal prompt if I don't answer in tapin within a configurable timeout, so that nothing is ever stuck.
40. As a user, I want the hook to add almost no latency to normal (non-permission) events, so that my agents don't slow down.
41. As a user, I want tapin's socket to be accessible only by my own user, so that other local users can't approve commands on my behalf.
42. As a user, I want tapin to never send data over the network, so that my code and prompts stay on my machine.
43. As a user, I want to be able to run two tapin windows at once and have both show the same state, so that having an extra pane open doesn't break anything.

### Platform and distribution
44. As a macOS user, I want tapin to work on Apple Silicon and Intel.
45. As a Linux user, I want tapin to work on x86_64 and aarch64, including over SSH.
46. As a user, I want a single static-ish binary with no runtime dependencies, so that installation is trivial.
47. As a user, I want `tapin --version` and release notes on GitHub Releases, so that I know what changed.

### Launch (maintainer)
48. As the maintainer, I want a README with a GIF demo above the fold, a one-line pitch and a three-step quickstart, so that visitors understand it in five seconds.
49. As the maintainer, I want a short "tapin vs agtop/agenttop" section, so that the obvious first question is answered.
50. As the maintainer, I want release automation that builds binaries, publishes to crates.io and updates the Homebrew tap on tag, so that shipping is one command.

## Implementation Decisions

- **Language and UI**: Rust with ratatui and crossterm. Single binary named `tapin`.
- **Clean-room rule**: the repo is implemented only from public agent documentation (Claude Code hooks docs, statusline JSON, transcript JSONL; Codex hooks and session rollout files; Gemini CLI hooks) and payloads captured from the maintainer's own sessions. No code, specs, protocols or naming may be taken from any other application or from reverse engineering.
- **One binary, several roles** (subcommands):
  - `tapin` (default): TUI client, starting the daemon if it isn't already running.
  - `tapin daemon`: the long-running session store that owns the socket.
  - `tapin hook --agent <claude|codex|gemini> --event <name>`: the short-lived process each agent invokes. It reads the agent payload on stdin, forwards it to the daemon, and for permission events waits for a decision.
  - `tapin init`, `tapin uninstall`, `tapin doctor`.
- **Transport**: a Unix domain socket in a per-user runtime directory, with permissions 0600 and a 0700 directory. Messages are newline-delimited JSON. Multiple TUI clients can subscribe to the same daemon.
- **Daemon vs in-TUI store**: there is a separate daemon so that hooks have a stable endpoint even while no TUI is open, and so that multiple TUIs share state. The TUI auto-spawns the daemon. Hooks never spawn it; with no daemon, they no-op instantly.
- **Hook fail-open contract** (the most important invariant):
  - If the socket is missing or connecting fails, the hook exits 0 immediately with output that tells the agent to fall through to its default behaviour.
  - For permission events it waits up to a configurable timeout (default 300s, capped at 590s, under Claude Code's 600s hook limit), then falls through silently. Claude Code shows its own dialog while the hook waits (verified on 2.1.296), so a long wait never strands the user.
  - The hook never returns a deny unless the user explicitly pressed `n`.
- **Core domain model** (the pure, deeply tested module):
  - `AgentKind` is `Claude | Codex | Gemini`.
  - `Session` holds id, agent kind, cwd/project, tty and tmux pane if known, state, state-since timestamp, last tool, last message summary, context usage and pending request.
  - `SessionState` is `NeedsPermission | NeedsAnswer | Done | Working | Idle | Ended`.
  - Each agent adapter normalizes raw agent payloads into a small `AgentEvent` enum. The `Inbox` reducer applies `AgentEvent`s and produces an ordered, grouped view.
  - Adapters are per-agent and stateless, so adding an agent means adding one adapter plus fixtures.
- **Passive mode**: without hooks, the daemon discovers sessions by polling known transcript locations (Claude Code project transcripts, Codex session rollouts) and the process table. It produces the same `AgentEvent`s, but approvals are disabled and state may lag by up to the poll interval. This mode never writes anywhere.
- **Hook source of truth**: when both hook events and passive discovery describe the same session, hook events win.
- **Approvals**:
  - Claude Code uses its documented permission-request hook decision output.
  - Codex is supported only where its public hook interface exposes a blocking approval point. If it doesn't, Codex rows show the request but `y`/`n` is disabled with a hint.
  - Gemini has no approvals in v0.1.
- **Jump**:
  - tmux is resolved from the `TMUX_PANE` the hook process inherits, then `switch-client`/`select-window`/`select-pane`.
  - Outside tmux, macOS uses best-effort terminal activation by tty, and other environments show the tty and cwd.
- **Notifications**: macOS via `osascript` and Linux via `notify-send`, only when available. The terminal bell is optional. Both are configurable.
- **Config**: an optional TOML file in the XDG config directory (notifications, bell, approval timeout, passive poll interval). Every setting has a sane default, and tapin runs with no config file.
- **init/uninstall**:
  - Writes are JSON-merges into each agent's settings file, with entries tagged so uninstall can find exactly tapin's entries.
  - A timestamped backup is written before every change, and the diff is shown before the user confirms.
  - A non-interactive `--yes` flag supports scripts.
- **Distribution**: `cargo-dist` produces macOS (arm64, x86_64) and Linux (x86_64, aarch64) binaries, a Homebrew tap and a shell installer. crates.io publishing happens on tag.
- **Architectural decisions** are recorded in the project `CLAUDE.md` as they are made.

## Testing Decisions

- **Good tests** test external behaviour through the highest seam: feed realistic inputs, assert observable outputs. No tests of private helpers or internal struct layouts.
- **Seam A: Inbox model (pure)**:
  - Ordered sequences of raw agent payloads, captured from real sessions and stored as fixtures per agent, go through adapters into the `Inbox` reducer.
  - Assertions cover grouping, ordering, row summaries and session lifecycle, using snapshot tests (insta).
  - This is where most tests live. Each new agent adapter or bug gets a fixture sequence.
- **Seam B: hook round-trip (integration, cross-process)**:
  - Start the daemon on a temp socket, run the real `tapin hook` binary with a fixture on stdin, answer with a headless test client (approve, deny, no answer), and assert the hook's stdout and exit code byte-for-byte against what the agent expects.
  - Must cover: daemon absent (instant fall-through), daemon killed mid-wait, timeout, answered in the agent instead, two TUIs connected.
- **TUI**: a handful of ratatui `TestBackend` render snapshots (narrow width, each group present, details view). The TUI only renders the `Inbox` view and maps keys to commands, so it stays thin.
- **init/uninstall**: run against temp-dir copies of real settings files that contain other tools' hooks. Assert the merge result, idempotency and exact reversal.
- **Prior art**: none in this repo yet. lazyc (the maintainer's existing Rust/ratatui project) has CI with fmt, clippy and test, which will be reused.
- **CI**: fmt, clippy (deny warnings) and test on macOS and Linux.

## Out of Scope

- Windows support.
- A GUI, menu bar app or notch UI. tapin is terminal-only.
- Sending prompts or free-text replies to agents from tapin. Enter jumps to the pane, and the user types there.
- "Always allow" or rule-based auto-approval. v0.1 does single-request approve/deny only.
- Token cost dashboards, usage history and analytics (that is the existing monitors' territory).
- Agents beyond Claude Code, Codex and Gemini CLI (Cursor, Aider, OpenCode and others) beyond being ignored cleanly.
- Remote or multi-machine aggregation, and any network transport.
- npm/npx distribution.
- Launching or orchestrating agents (that is claude-squad's territory).

## Further Notes

- **Positioning**: "tapin — an inbox for your AI coding agents. Approve, jump, done." The competitors (ldegio/agtop 283★ JS, vicarious11/agenttop Python, MBrassey/agtop Rust) are monitors. tapin's wedge is *acting*: urgency ordering, approvals, jump and notifications.
- **Name check (2026-10-13)**: `tapin` is free on crates.io and Homebrew core, with no significant GitHub collision.
- **Repo**: private until launch day, then made public.
- **Launch plan**:
  - Polish the README GIF first.
  - Launch on a single day: Show HN (Tue–Thu, US morning), r/ClaudeAI, r/commandline, r/rust, X with the GIF, and PRs to awesome-claude-code and awesome-ratatui.
- **v0.2 candidates**: always-allow rules, Codex approvals if they aren't feasible in v0.1, more agents, and replying to questions inline.
