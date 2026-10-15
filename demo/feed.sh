#!/bin/sh
# Feeds a scripted set of agent sessions to a tapin daemon for the README demo.
# Nothing here touches real agents: payloads go straight to `tapin hook`.
# Usage: demo/feed.sh <socket>
S="$1"
BIN="${TAPIN_BIN:-tapin}"
hook() { # agent payload-json [env assignments...]
  agent="$1"; payload="$2"; shift 2
  printf '%s' "$payload" | env -u TERM_PROGRAM -u TMUX -u TMUX_PANE "$@" "$BIN" hook --agent "$agent" --socket "$S" >/dev/null 2>&1
}
ev() { # agent session cwd event extra-json
  # The shop-api session "runs" in the demo tmux pane, so Enter can jump to it.
  if [ "$2" = "$A" ] && [ -n "$DEMO_TMUX" ]; then
    hook "$1" "{\"session_id\":\"$2\",\"cwd\":\"$3\",\"hook_event_name\":\"$4\"$5}" TMUX="$DEMO_TMUX" TMUX_PANE="$DEMO_PANE"
  else
    hook "$1" "{\"session_id\":\"$2\",\"cwd\":\"$3\",\"hook_event_name\":\"$4\"$5}"
  fi
}
A=11111111-aaaa-4000-8000-000000000001
B=22222222-bbbb-4000-8000-000000000002
C=33333333-cccc-4000-8000-000000000003

# Start only when the recording shows the inbox (the tape touches $S.go).
while [ ! -e "$S.go" ]; do sleep 0.1; done
sleep 1
ev claude $A /home/you/work/shop-api SessionStart
ev codex  $B /home/you/work/landing-page SessionStart
ev claude $C /home/you/work/docs-site SessionStart
ev claude $A /home/you/work/shop-api UserPromptSubmit ',"prompt":"fix the flaky checkout test"'
ev codex  $B /home/you/work/landing-page UserPromptSubmit ',"prompt":"tighten the hero copy"'
ev claude $C /home/you/work/docs-site UserPromptSubmit ',"prompt":"write the API reference"'
ev claude $A /home/you/work/shop-api PreToolUse ',"tool_name":"Edit","tool_input":{"file_path":"src/checkout_test.go"}'
sleep 2
ev codex  $B /home/you/work/landing-page Stop ',"last_assistant_message":"Hero copy is shorter and the CTA now says Start free."'
sleep 2
# Claude asks to run a command: this hook waits for y/n in the inbox.
ev claude $A /home/you/work/shop-api PermissionRequest ',"tool_name":"Bash","tool_input":{"command":"go test ./checkout/... -count=5","description":"Run the checkout tests 5 times"}'
ev claude $A /home/you/work/shop-api PostToolUse ',"tool_name":"Bash","tool_input":{"command":"go test ./checkout/... -count=5"}'
sleep 1
# Another Claude asks a question: this hook waits for a number key.
ev claude $C /home/you/work/docs-site PermissionRequest ',"tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"Where should the new API reference live?","header":"Docs layout","multiSelect":false,"options":[{"label":"One page per endpoint","description":"Easy to link, more files"},{"label":"A single long page","description":"Easy to search, one file"}]}]}'
ev claude $C /home/you/work/docs-site PostToolUse ',"tool_name":"AskUserQuestion","tool_input":{}'
sleep 1
ev claude $A /home/you/work/shop-api Stop ',"last_assistant_message":"Fixed: the test raced on the cart clock. 5/5 runs pass."'
sleep 60
