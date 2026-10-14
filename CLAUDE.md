# tapin

An inbox for AI coding agents in the terminal. Rust + ratatui. Spec: `docs/specs/tapin-v0.1.md`.

## Hard rules
- **Clean-room**: implement only from public agent docs + payloads captured from our own sessions. Never take code, specs, protocols, or names from any other app or from reverse engineering.
- **Hooks fail open**: `tapin hook` must never hang or block an agent. No daemon → exit 0 instantly. It never denies unless the user pressed `n`.
- **English only** for code, CLI text, README, and docs.

## Architecture decisions
- One binary, several roles: `tapin` (TUI), `tapin daemon`, `tapin hook`, `tapin init|uninstall|doctor`.
- The daemon owns a per-user Unix socket (0600) with NDJSON messages. The TUI auto-spawns the daemon; hooks never do.
- Raw agent payloads → per-agent adapter → `AgentEvent` → pure `Inbox` reducer. The TUI renders only the Inbox view.
- When hook events and passive discovery disagree, hook events win.
- Approvals: a `PermissionRequest` hook keeps its socket open and waits for a `Decision`. Claude Code shows its own dialog while the hook waits (verified on 2.1.296), so waiting never strands the user.
  - Every request gets a daemon-assigned `request_id`; `Decide` names it, so a stale or second answer can never hit a newer request.
  - One pending request per session. A newer request, or any event other than PreToolUse (which can race ahead of/behind its PermissionRequest), replies `Pass` to the waiting hook.
  - When a waiting hook hangs up, its request is detached: the row still says NEEDS (the agent's dialog is open) but `y`/`n` is disabled.
- **Codex differs** (verified on Codex CLI 0.162.1; https://learn.chatgpt.com/docs/hooks, openai/codex#39835): it runs PermissionRequest hooks *before* showing its own prompt, so a waiting hook leaves the user staring at "Running PermissionRequest hook". So for agents where `AgentKind::prompt_waits_for_hook()`:
  - With no inbox subscribed, the daemon replies `Pass` at once.
  - When the last inbox closes, waiting requests are passed.
  - The wait is capped by `codex_approval_timeout_secs` (default 30, env `TAPIN_CODEX_APPROVAL_TIMEOUT_SECS`).
  - Codex hooks need user trust via `/hooks`, and `init` never writes `trusted_hash`.
- **Gemini CLI** (status only; from docs + source at gemini-cli 29315e5, not verified live since it isn't installed here):
  - It uses its own event names (BeforeAgent/BeforeTool/AfterTool/AfterAgent/Notification) on the same payload and config shape, under `$GEMINI_CLI_HOME/.gemini/settings.json`.
  - There is no blocking permission event.
  - **Any exit code other than 0 or 1 means deny, and stderr text is shown to the user**, so `tapin hook` installs a panic hook that exits 0 silently.
  - Hooks only run in trusted folders.
  - Gemini runs as `node`, so dead-agent cleanup and passive mode don't cover it; a missed SessionEnd leaves its row until restart (known limit).
  - Gemini settings may contain comments (JSONC). tapin won't rewrite them; `init` prints the hooks to add by hand.
- Threat model: other local users cannot reach the socket (0600 in a dir we own that no one else can write). Same-user processes can connect, but since v0.2 (#12, `src/peer.rs`) answers are bound to verified processes on both ends:
  - **Daemon → inbox.** `Decide` counts only on a connection that subscribed and whose peer passes `peer::verify`, checked at Subscribe and again at each Decide.
    - Same uid.
    - The executable has the daemon's own path **and** file identity (dev+inode taken at startup), so another program written to that path fails. On Linux the identity comes from `/proc/<pid>/exe`, the loaded file. On macOS it comes from the file now at that path, a weaker approximation: a `mv`-swap only helps a process that is also detached.
    - The process tree can be read all the way to pid 1, at most 64 levels; deeper or unreadable trees are refused.
    - Every ancestor's name is readable; an unreadable one is refused. Names come from `/proc/<pid>/comm` and the macOS short BSD info, which are readable even for root's `login`/`sshd`.
    - No ancestor is a known agent pid. No ancestor's name stem is claude/codex/gemini (the macOS kernel calls Claude Code's native binary `claude.exe`).
    - Agent pids form an append-only registry, holding pid and start time until the process is gone. It is fed by hooks and passive mode. A hook-reported pid is kept only if `ps` names it like an agent (`location::is_agent_named`, the same rule the hook uses), so pid 1 or the user's terminal can't be injected to refuse every inbox.
  - **Hook → daemon.** A waiting hook only takes an answer from a daemon that passes the same check against the hook's own binary **and** answers `Hello` with `socket::PROTOCOL` ≥ 2. A v0.1 daemon stays silent; on macOS it can otherwise look like the new binary, because file identity there is path-based. Otherwise it still sends the event, so the row shows, but doesn't wait (fail open). So a fake daemon on the socket can't approve.
  - **The TUI starts read-only** and answers only after `Access{can_answer:true}`. A v0.1 daemon never sends it.
  - **Unverified inboxes** get `Access{can_answer:false}` and are read-only. They don't count for Codex's pass-through. Answerers dropped by `broadcast` are hung up on. Answerers failing a re-check are told `Access{false}`. Either way they are removed, and Codex requests are released.
  - **OS calls.** macOS: `LOCAL_PEERPID`, `getpeereid`, `proc_pidpath`, and `proc_pidinfo(PROC_PIDT_SHORTBSDINFO)`. The short form is needed because the full one can't read root's `login` above every Terminal/iTerm2 shell. Linux: `SO_PEERCRED` and `/proc`.
  - **Test switches** exist only under `#[cfg(debug_assertions)]`, and CI fails if the dist binary contains them. `TAPIN_TRUST_PEER_EXE` makes the daemon trust the test binary. `TAPIN_TEST_IGNORE_ANCESTRY`, set for every test in `.cargo/config.toml`, is needed because the suite often runs under a developer's Claude Code. `TAPIN_TEST_DISTRUST_DAEMON` exercises the hook's untrusted-daemon path.
  - **Residual risk.** This is a guard against accidents and prompt injection, not a sandbox. A same-user process can still win in these ways:
    - It detaches from the agent so the agent leaves its ancestry (pty master kept by a reparented child, setsid/nohup, a tmux server) and drives a real tapin.
    - It ptraces the daemon or tapin, or uses fd passing combined with the above.
    - It edits the agent's settings.
    - Agents running as `node` (npm-installed Claude, Gemini) are recognised by name only if a hook or passive source reported their pid, and hooks only find `claude`/`codex` by name.
  - **Containers.** A process whose parent is outside its pid namespace (ppid 0, e.g. a shell from `docker exec`) can't prove its ancestry, so a tapin started that way is read-only. tapin running as the container's pid 1 is fine.
  - **Known cost.** After upgrading tapin, a daemon from the old file refuses the new TUI, and new hooks refuse the old daemon, until the daemon is restarted. The TUI says so, and hooks fail open meanwhile.
- `tapin init/uninstall/doctor` (src/setup.rs): pure JSON edits on Claude's `settings.json` (`$CLAUDE_CONFIG_DIR` or `~/.claude`), with key order (serde_json `preserve_order`), indentation and trailing newline kept. tapin's hooks are recognized by command shape (`tapin … hook --agent claude`) and use the absolute binary path. Writes go through a diff, a confirmation (unless `--yes`), a timestamped backup and an atomic rename. Invalid JSON is never touched.
- Notifications fire from the daemon (one source, so multiple TUIs don't duplicate them, and they still work with no TUI open), on transitions into NEEDS or DONE. Title and body go to osascript/notify-send as argv, never spliced into a script. `TAPIN_NOTIFY_CMD` swaps the notifier (tests). The bell is TUI-side, because the daemon has no terminal.
- `hide_dirs` (default `~/.claude-mem`, claude-mem's always-busy observer) is enforced in the Inbox for hook events and passive observations alike (path-component prefix match). The daemon replies `Pass` at once to a hidden session's permission hook, since no inbox shows it.
- `tapin status` sends `Snapshot` (one reply, then done), never `Subscribe`, so a status bar polling every few seconds never counts as an open inbox (which would hold Codex requests). It never spawns the daemon, prints nothing on any error, and exits after 800ms whatever happens (a wedged daemon's backlog can block `connect`).
- Filter (`/`): keys act only on visible rows. While typing, every key is text (arrows and Ctrl-keys are dropped). Requests hidden by the filter count as known, so they never pull the cursor, even when they later match. Header counts and the bell cover all rows.
- Context column: Codex reports its window; Claude's is known only for `[1m]` models (some models get 1M without the marker), so otherwise the row shows tokens (`ctx 132k`), never a guessed %. Transcripts are read tail-first on the poll thread, outside the lock, cached by size+mtime.
- `x` stops a Claude background session (`claude stop <id>`, conversation kept; never `claude rm`). It's ignored until the cursor settles, like `y`. The next key answers the prompt: `y` stops, anything else cancels. The prompt expires after 5s (a late `y` does nothing) and ends if the session disappears. It runs as the user, so read-only mode (#12, which governs answers through the daemon) doesn't block it.
- Cursor rule (`tui::reselect`): only a request that is new (by session id + request id) pulls the cursor; requests already on screen don't, or `j`/`k` could never leave them.
- Agent glyphs are what each CLI prints itself (`✻` Claude Code, `>_` Codex, `✦` Gemini CLI): single-width Unicode/ASCII, never logos or emoji.
- Config is optional, `deny_unknown_fields`. A broken file is reported on stderr and ignored; it never stops a hook.
- Socket paths are capped at 103 bytes (`sockaddr_un`); `socket::check_path` gives a clear error.

## Releases
- dist 0.33 (`dist-workspace.toml`). Targets: macOS arm64/x86_64 plus static Linux musl arm64/x86_64. Installers: shell and Homebrew (`Thevi99/homebrew-tap`). crates.io is published by `.github/workflows/publish-crates.yml` as a dist custom publish job. Steps and the secrets needed are in RELEASING.md.
- After editing dist config, run `dist generate` and commit `release.yml`. `dist generate --check` must pass.

## Testing
- **Tests must never touch real agent configs.** `.cargo/config.toml` `[env]` points CLAUDE_CONFIG_DIR, CODEX_HOME and XDG_CONFIG_HOME into `target/test-isolation/` for every test process; `tests_can_never_reach_the_real_agent_configs` guards it. (2026-10-14: before this, a test without CODEX_HOME ran `init --yes` on a developer's real ~/.codex/hooks.json; it was restored byte-identical from tapin's own backup.)
- Seam A: fixture sequences → adapters → Inbox reducer, snapshot-tested with insta.
- Seam B: real `tapin hook` binary ↔ daemon on a temp socket ↔ headless client.
- Issue tracker: GitHub Issues on Thevi99/tapin. Triage label: `ready-for-agent`.
