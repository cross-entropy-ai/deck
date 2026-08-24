#!/bin/sh
# deck agent-state hook - managed by deck; reinstalling overwrites this file.
# Add your own hooks beside it instead of editing it.
# DECK_HOOK_VERSION=1
#
# Reports the owning agent's lifecycle state onto the tmux pane it runs in
# (pane user options @deck_agent_state / @deck_agent_session), where deck's
# Agents sidebar reads it out of the list-panes call it already makes.
# Without deck running, the writes are inert pane options that vanish with
# the pane. This script must never affect the agent: it prints nothing
# (some hook events inject stdout into the prompt or show it to the user)
# and always exits 0.

state="${1:-}"

# Swallow stdin unconditionally so the agent never sees EPIPE.
input=$(cat 2>/dev/null || :)

# Not inside tmux (a plain ssh login), or explicitly disabled: do nothing.
[ -n "${TMUX:-}" ] || exit 0
[ -n "${TMUX_PANE:-}" ] || exit 0
[ "${DECK_HOOKS:-1}" = "1" ] || exit 0
command -v tmux >/dev/null 2>&1 || exit 0

# Subagent events describe a child, not this pane's agent.
case "$input" in *'"agent_id"'*) exit 0 ;; esac

now=$(date +%s 2>/dev/null) || now=0

case "$state" in
  working|blocked|idle)
    tmux set-option -p -t "$TMUX_PANE" @deck_agent_state "$state@$now" >/dev/null 2>&1 || :
    ;;
  session)
    # SessionStart: a fresh session's turn state is unknown, so drop any
    # state a previous (possibly killed) agent left on this pane before
    # recording identity. Also prove the hook actually runs (Codex only
    # executes hooks the user has trusted).
    tmux set-option -p -u -t "$TMUX_PANE" @deck_agent_state >/dev/null 2>&1 || :
    sid=$(printf '%s' "$input" | sed -n 's/.*"session_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
    if [ -n "$sid" ]; then
      tmux set-option -p -t "$TMUX_PANE" @deck_agent_session "$sid" >/dev/null 2>&1 || :
    fi
    tmux set-option -p -t "$TMUX_PANE" @deck_hook_alive "1" >/dev/null 2>&1 || :
    ;;
  clear)
    tmux set-option -p -u -t "$TMUX_PANE" @deck_agent_state >/dev/null 2>&1 || :
    tmux set-option -p -u -t "$TMUX_PANE" @deck_agent_session >/dev/null 2>&1 || :
    ;;
esac
exit 0
