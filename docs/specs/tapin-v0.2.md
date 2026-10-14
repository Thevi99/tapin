# tapin v0.2

Parent: #1. Research is recorded on each ticket.

## Goal

Make a busy inbox (10+ sessions) easy to search and scan, show how full each agent's context is, and close the "any same-user process can approve" gap (#12).

## Scope

### 1. `/` search and filter
- `/` opens a filter prompt in the footer.
  - While typing, every printable key (including `y`, `n`, `q`, `j`, `k`) goes into the query. Nothing is approved, quit or moved.
  - Backspace deletes.
  - Enter keeps the filter and returns to normal keys.
  - Esc clears the filter and returns to normal keys.
- Matching is a case-insensitive substring match on project label, agent name, detail and cwd.
- With a filter active, the header shows `filter: <q>` and how many rows are hidden. The "waiting on you" count still counts **all** rows, so a hidden request is never silently missed.
- Cursor rules (`reselect`, SETTLE, Tab) apply to the visible rows only.
- Esc in normal mode with a filter active clears it.

### 2. `g`: group by folder
- `g` toggles grouping. Rows are grouped by cwd. Each group gets a header line `▸ <folder name>  (<n>)`.
- Groups are ordered by their most urgent row (same order as rows today), then by folder name.
- Within a group, rows keep today's order.
- Headers are not selectable: `j`/`k`/Tab skip them.
- Grouping is remembered for the TUI session only (no config).

### 3. Context % column
- Each row shows how full the agent's context window is, e.g. `ctx 64%`, dim. It turns yellow at 80% or more and red at 95% or more.
- Nothing is shown when unknown.
- **Claude**:
  - Source: the transcript at the hook's `transcript_path`, or for passive rows `<claude config dir>/projects/<cwd with / and . → ->/<sessionId>.jsonl`.
  - Use the last `type=="assistant"` line, skipping `isSidechain==true` and `message.model=="<synthetic>"`.
  - `used = input_tokens + cache_creation_input_tokens + cache_read_input_tokens`.
  - Window: 1,000,000 if `requestedModel` or `message.model` ends with `[1m]`, else 200,000. This matches Claude Code's statusline docs.
- **Codex**:
  - Source: the rollout file passive mode already tracks.
  - Use the last `event_msg` / `token_count` with non-null `info`: `info.last_token_usage.total_tokens / info.model_context_window`.
- Gemini: not shown (no verified source).
- Reading:
  - Tail only. Start with the last 256 KB and double up to 4 MB.
  - Pre-filter lines on raw bytes before JSON parsing.
  - Cache by (path, size, mtime).
  - Runs on the daemon's passive poll thread, never under the inbox lock while doing file I/O.
- Cost is **out of scope**:
  - Live Claude sessions have no reported cost.
  - Codex reports none.
  - A price table would be wrong on subscription plans.

### 4. Approvals only from a real tapin TUI (#12)
- The daemon accepts `Decide` only on a connection that previously `Subscribe`d **and** whose peer passed `verify_peer`:
  - the peer uid equals ours;
  - the peer's executable path (canonicalized, Linux " (deleted)" suffix stripped) equals the daemon's own `current_exe` at startup;
  - no ancestor of the peer pid (walking ppid to 1) is a known agent pid (from hook locations and passive observations).
- The check runs at Subscribe and again at each Decide.
- An unverified subscriber still gets snapshots (read-only), but does not count as an inbox that can answer. This affects the Codex "nobody to answer" rule. Its TUI shows "read-only: approvals are disabled here" instead of `y/n`.
- Platform calls via `libc`:
  - Linux: `SO_PEERCRED`, `/proc/<pid>/exe`, `/proc/<pid>/stat`.
  - macOS: `LOCAL_PEERPID`, `proc_pidpath`, `proc_pidinfo(PROC_PIDTBSDINFO)`.
- Test seam: in debug builds only (`cfg(debug_assertions)`), `TAPIN_TRUST_PEER_EXE=<path>` lets the daemon treat that executable as tapin, so the headless test client can approve. Release builds ignore it.
- Residual risk, documented in CLAUDE.md and the README: a same-user agent that detaches (double-fork/`setsid`) and drives a real `tapin` inside a pty can still approve, as can one that ptraces the daemon or edits settings. This closes the cheap `nc -U`/script self-approval.

## Out of scope
- Cost column, price tables, the statusline integration.
- Folding/collapsing groups, persistent UI prefs.
- Regex or fuzzy search.
- Gemini context %.

## Acceptance criteria
- Filter:
  - Typing `/yes` then Enter never produces a Decide or Quit.
  - Rows are filtered.
  - The header counts all waiting rows.
  - Esc restores everything.
  - y/n/Tab act only on visible rows.
- Group: headers render, ordering follows urgency, `j`/`k`/Tab never land on a header, Enter jumps to the right session.
- Context:
  - Fixture transcripts give the exact expected % for Claude (200k and 1M) and Codex.
  - The 896 KB-from-end case is found.
  - Unknown shows nothing.
  - No file I/O under the inbox lock.
- #12:
  - A raw socket client that Subscribes and Decides leaves the request pending.
  - The TUI (or the debug seam) still approves.
  - The `verify_peer` unit tests cover exe mismatch, the deleted suffix, an agent ancestor, and a clean chain.
  - CLAUDE.md and the README threat model are updated.
- All tests pass, clippy is clean, CI is green, and a separate review (plus Codex for #12) signs off.
