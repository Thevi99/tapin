# Answer Claude's questions from tapin

Parent: #1.

## Goal

When Claude Code asks a question with its AskUserQuestion tool, show the question and options in the inbox and let the user answer with a number key. Today the row says "too long to show".

## Evidence

- Docs: https://code.claude.com/docs/en/hooks.
  - AskUserQuestion's input is `questions[]`, each with `question`, `header`, `options[{label, description}]` and `multiSelect`.
  - Answers go in `updatedInput: {questions, answers: {<question text>: <label>}}`. The docs describe this for PreToolUse.
- Verified live on Claude Code 2.1.299 (2026-10-14): a **PermissionRequest** hook returning `{"behavior":"allow","updatedInput":{"questions":…,"answers":{…}}}` answers the question. Claude logged "User answered Claude's questions: → Blue · Allowed by PermissionRequest hook". This is not documented for PermissionRequest, so it may change.

## Scope

- **Display.** For an AskUserQuestion request, the input shown is readable text: each question, then `1. label — description` lines. The summary is `AskUserQuestion: <header>`.
- **Answering.** It is offered only for exactly one question with `multiSelect` false. Keys `1`–`N` choose an option. The same guards as `y` apply: the cursor has settled (SETTLE), the whole question is visible, the inbox isn't read-only, and the daemon speaks protocol ≥ 3. The footer hint reads `1-N answer · n deny`.
- **Other shapes.** Multiple questions or multi-select are shown read-only, with "answer in Claude's window".
- **Wire format.**
  - `Decide` gains an optional `answer` label.
  - The daemon replies to the hook with `Decision::Answer(label)`.
  - The hook builds `updatedInput` from its own payload's `questions`, plus `answers: {question: label}`. The label must be one of that question's options, otherwise the hook passes.
- **Protocol.** `socket::PROTOCOL` becomes 3. `Access` carries `protocol`, which defaults to 2 for older daemons. The TUI enables number keys only when protocol ≥ 3, because an older daemon would ignore `answer` and send a bare allow. Hooks keep accepting protocol ≥ 2.

## Out of scope

- Multi-select, multiple questions, free-text "Other" answers and notes.
- ExitPlanMode (plan approval).
- PreToolUse-based answering.

## Acceptance criteria

- A captured AskUserQuestion PermissionRequest renders its question and numbered options. It never says "too long to show" for a normal question.
- With protocol 3 and a settled cursor, pressing `2` sends `Decide{answer:"<label 2>"}`.
  - The hook prints `allow` with `updatedInput.questions` (unchanged) and `answers` mapping the question text to that label.
  - A label not among the options makes the hook pass.
- Digits do nothing on other rows, while the cursor is unsettled, when read-only, with protocol 2, or for multi-select or multi-question requests.
- The live check against real Claude Code answers the question.
