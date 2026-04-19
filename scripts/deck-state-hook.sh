#!/bin/sh
# deck Claude Code state hook.
#
# Installed by `deck hooks install` into ~/.claude/hooks/deck-state.sh
# and registered in ~/.claude/settings.json for every stock Claude
# Code hook event. Reads the hook payload JSON on stdin and atomically
# writes a per-Claude-session state file that deck polls.
#
# State file path: $DECK_STATE_DIR or ${XDG_CONFIG_HOME:-$HOME/.config}/deck/state/<session_id>.json
#
# Requires jq. On any error (missing jq, malformed payload, unwritable
# dir) the hook silently exits 0 — never block Claude Code.

set -eu

# deck's status model:
# - working: Claude is actively processing (sending tokens, running tools)
# - waiting: Claude wants the user's attention — Stop (turn ended,
#   awaiting reply) and Notification (permission prompt, idle reminder)
#   both qualify. deck downgrades acknowledged waitings to Ready in
#   the UI, so this isn't permanently noisy.
# - idle: Claude is alive but hasn't done anything yet. Promoted to
#   Ready by deck when matched to a tmux session.
map_event_to_status() {
  case "$1" in
    UserPromptSubmit|PreToolUse|PostToolUse|SubagentStop|PreCompact)
      echo working ;;
    Stop|Notification)
      echo waiting ;;
    SessionStart)
      echo idle ;;
    *)
      echo "" ;;
  esac
}

STATE_DIR="${DECK_STATE_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/deck/state}"
mkdir -p "$STATE_DIR" 2>/dev/null || exit 0

command -v jq >/dev/null 2>&1 || exit 0

input=$(cat)
[ -n "$input" ] || exit 0

event=$(printf '%s' "$input" | jq -r '.hook_event_name // empty' 2>/dev/null || true)
session_id=$(printf '%s' "$input" | jq -r '.session_id // empty' 2>/dev/null || true)
cwd=$(printf '%s' "$input" | jq -r '.cwd // ""' 2>/dev/null || true)

[ -n "$session_id" ] || exit 0
[ -n "$event" ] || exit 0

# SessionEnd deletes the state file so deck stops attributing this
# Claude session to a tmux pane. Other events write a fresh file.
if [ "$event" = "SessionEnd" ]; then
  rm -f "$STATE_DIR/$session_id.json"
  exit 0
fi

status=$(map_event_to_status "$event")
[ -n "$status" ] || exit 0

ts_ms=$(($(date +%s) * 1000))
pid=$PPID
tmux_pane="${TMUX_PANE:-}"

tmp="$STATE_DIR/$session_id.json.tmp.$$"
if ! jq -n \
    --arg session_id "$session_id" \
    --arg status "$status" \
    --arg event "$event" \
    --arg cwd "$cwd" \
    --argjson pid "$pid" \
    --arg tmux_pane "$tmux_pane" \
    --argjson ts_ms "$ts_ms" \
    '{session_id: $session_id, status: $status, event: $event, cwd: $cwd, pid: $pid, tmux_pane: $tmux_pane, ts_ms: $ts_ms}' \
    > "$tmp" 2>/dev/null; then
  rm -f "$tmp" 2>/dev/null || true
  exit 0
fi

mv -f "$tmp" "$STATE_DIR/$session_id.json" 2>/dev/null || rm -f "$tmp" 2>/dev/null || true
exit 0
