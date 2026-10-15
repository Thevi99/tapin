# Records the README demo: `vhs demo/demo.tape` (needs vhs and tmux; run from the repo root).
# Sessions are scripted by demo/feed.sh and the "agent" screen is demo/agent.sh;
# no real agents are involved. Everything runs on a private tmux server (-L tapindemo).

Output demo/tapin.gif

Set Shell "bash"
Set FontSize 16
Set Width 1100
Set Height 460
Set Padding 16
Set Theme "Catppuccin Mocha"

Hide
Set TypingSpeed 1ms
Type "export PS1='$ ' TAPIN_BIN=$PWD/target/release/tapin TAPIN_CLAUDE_CMD=/nonexistent TAPIN_PS_CMD=/nonexistent; T='tmux -L tapindemo -f /dev/null'; $T kill-server 2>/dev/null; sleep 0.3"
Enter
# The agent's pane lives in session "agents"; the inbox will run in session "inbox".
Type "$T new-session -d -s agents -x 120 -y 24 $PWD/demo/agent.sh && $T set -g status off && $T set -g escape-time 0 && export DEMO_TMUX=$($T display -p '#{socket_path},#{pid},0') DEMO_PANE=$($T display -p -t agents '#{pane_id}')"
Enter
Type "D=$(mktemp -d) && chmod 700 $D && S=$D/s.sock && ($TAPIN_BIN daemon --no-notify --socket $S &) && sleep 0.5 && (demo/feed.sh $S &) && clear"
Enter
Sleep 300ms
Show
Set TypingSpeed 60ms

Type "tapin"
Sleep 400ms
Hide
Set TypingSpeed 1ms
Backspace 5
Type `touch $S.go; $T new-session -s inbox -x 120 -y 24 "$TAPIN_BIN --socket $S"`
Enter
Show
# Sessions appear; one finishes; Claude asks to run a command, then another asks a question.
Sleep 8.5s
# The cursor sits on the request with the full command shown: approve it.
Type "y"
Sleep 3.5s

# Another Claude asks a question: answer it with a number key.
Type "1"
Sleep 2.5s

# Jump straight to the agent that just finished.
Type "k"
Sleep 1s
Enter
Sleep 3.5s
