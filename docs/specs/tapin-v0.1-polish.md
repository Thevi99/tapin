# tapin v0.1 polish (before the v0.1.0 tag)

Parent: #1. Research notes (sources) are in the issue for this spec.

## Goal

Make the inbox easier to read at a glance and quicker to work through, and fix a cursor bug, before launch.

## Scope

1. **Bug: the cursor can't leave a pending request.** Today every snapshot (sent each second) pulls the cursor back to the first NEEDS row, so `j`/`k` can't reach other rows while any request waits.
   - New rule: the cursor is pulled to a request only when a request (session id + request id) appears that was not in the previous snapshot, or when the selected session is gone.
   - Otherwise it follows the session the user chose.
   - The `SETTLE` guard on `y`/`n` is unchanged.
2. **Per-agent identity.** Each row shows a glyph before the agent name:
   - Claude: `✻` in `#D97757`.
   - Codex: `>_` in bold default foreground.
   - Gemini: `✦` in `#4796E4`.
   - Glyphs are plain Unicode (single-width, not emoji, not logos) or ASCII.
   - The glyph column is 2 cells, so names line up.
   - The details overlay uses the same glyph.
   - The README says tapin is not affiliated with Anthropic, OpenAI or Google.
3. **`Tab`: next agent waiting on you.** It moves the cursor to the next NEEDS or DONE row after the cursor, wrapping. If none exists it does nothing. It is listed in help and the footer.
4. **`tapin status`.** One line for a tmux/shell status bar.
   - Format: `NEEDS 2 · DONE 1`, listing only non-zero counts.
   - It prints nothing when nothing waits or when no daemon runs.
   - It never starts a daemon, always exits 0, and gives up after a short timeout.
   - The README shows the tmux line.

## Out of scope

- `/` search and filter (typed letters conflict with `y`/`n`; it needs its own input mode). Deferred to v0.2.
- Grouping by project, cost/context columns, batch approve, snooze.
- Colour themes or `--ascii` mode.

## Acceptance criteria

- With a request pending, `j` moves to another row and stays there across later snapshots with the same requests. A *new* request still pulls the cursor (existing tests keep passing).
- A rendered row (TestBackend) contains `✻ claude`, `>_ codex` or `✦ gemini` with the agent column aligned.
- `Tab` from any row reaches the next NEEDS/DONE row, wraps, and is a no-op when there is none. It never answers anything.
- `tapin status` against a daemon holding one request prints `NEEDS 1`. With no daemon it prints nothing and exits 0 in under 1s.
- Tests 143+ pass, clippy clean, CI green; a separate reviewer signs off.
