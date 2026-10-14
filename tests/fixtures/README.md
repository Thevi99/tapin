# Claude Code hook fixtures

Payloads captured from real Claude Code sessions, with paths and usernames sanitized.

- `claude/basic-turn/`: one unbroken captured session (prompt, Read, two Bash calls, Stop, SessionEnd).
- `claude/permission-turn/`: spliced. `04-PermissionRequest.json` is a real captured PermissionRequest payload, re-labelled into the `basic-turn` session, with its tool input aligned to the neighbouring PreToolUse and PostToolUse.

Observed behaviour (Claude Code 2.1.296, interactive):
- The permission dialog is shown in the terminal while PermissionRequest hooks run, and whichever answers first wins.
- If the user answers in the terminal, the still-running hook is not killed. The next event for the session (e.g. PostToolUse) is the signal that the request was settled.
- Hook output `{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}` closes the dialog and runs the tool.

# Codex hook fixtures (`codex/`)

- `codex/approval-turn/`: one unbroken captured Codex CLI 0.162.1 session (`-a on-request -s read-only`). It goes prompt, PreToolUse, PermissionRequest (approved in the Codex terminal), PostToolUse, Stop, then SessionEnd. Paths are sanitized.

Observed behaviour, matching openai/codex#39835: Codex runs the PermissionRequest hook *before* it shows its own approval prompt. Concurrent matching hooks are launched in parallel, and any deny wins.

# Passive fixtures (`passive/`)

- `claude-agents.json`: synthetic output of `claude agents --json`, built from the schema in https://code.claude.com/docs/en/agent-view and checked against real 2.1.296 output (field names only; no real content). It covers background sessions in the working, blocked and stopped states, and interactive sessions in the waiting (permission prompt), busy and idle states.
- `codex-sessions/done.jsonl`: one real Codex 0.162.1 rollout (from the `codex/approval-turn` capture), reduced to `session_meta` (id, cwd, version, source) and the `task_started` / `task_complete` turn events. `working.jsonl` is the same file without the final `task_complete`.

# Gemini CLI fixtures (`gemini/`)

- `status-turn/`: **built from the documented payloads** in google-gemini/gemini-cli `docs/hooks/reference.md` (main 29315e5). These are not captures: Gemini CLI wasn't installed on the dev machine. The sequence is SessionStart, BeforeAgent, BeforeTool (`run_shell_command`), Notification (ToolPermission), AfterTool, AfterAgent (`prompt_response`), then SessionEnd.
