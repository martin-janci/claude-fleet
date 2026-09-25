#!/usr/bin/env bash
# A scripted stand-in for Claude Code, for scripts/hub-e2e.sh's work-graph leg
# (work graph M10.2). The script puts it on the e2e hub's PATH as `claude`;
# the hub's own pane command (`cl --resume ID || cl --session-id ID || cl`,
# with `cl` falling back to `claude --dangerously-skip-permissions`) starts
# it in a real tmux pane. It is NOT a model: it only does what the hub can
# observe of one, over the same wire a real Claude Code uses.
#
#   * `--resume <id>` fails (no such conversation), so the pane command
#     reaches `--session-id <id>`: the id fleet minted for the row. Anything
#     else (`claude agents --json`, a bare `cl`) exits 1.
#   * On start it POSTs the SessionStart hook, prints the REPL chrome fleet
#     waits for (`? for shortcuts`), then reads prompts from the pane.
#   * Each prompt (the lines of one paste, gathered until 0.5 s of quiet)
#     is POSTed as UserPromptSubmit, then answered with a Stop hook whose
#     `last_assistant_message` is the reply. A prompt carrying a hand-off
#     request's `WORK_HANDOVER_BEGIN_<nonce>` is answered with a hand-off
#     between that request's two marker lines (M9.3); anything else gets a
#     one-line acknowledgement.
#   * Every hook's HTTP status and response body is appended to
#     $FAKE_CLAUDE_LOG_DIR/<id>/hooks.log, every prompt to prompts.log, so
#     the script can assert what fleet handed Claude (the queued brief rides
#     the first prompt's UserPromptSubmit response).
#
# Environment (inherited from the hub through its tmux server):
#   FAKE_CLAUDE_HUB      http://127.0.0.1:<port> of the hub
#   FAKE_CLAUDE_HOST     the Host header the hub allowlists
#   FAKE_CLAUDE_TOKEN    the bearer for POST /hook
#   FAKE_CLAUDE_LOG_DIR  where the per-session logs go
set -u

case " $* " in
  *" agents "* | *" --resume "*) exit 1 ;;
esac
sid=""; name=""
while [ $# -gt 0 ]; do
  case "$1" in
    --session-id) sid="${2:-}"; shift ;;
    --name) name="${2:-}"; shift ;;
  esac
  shift
done
[ -n "$sid" ] || exit 1

log="${FAKE_CLAUDE_LOG_DIR:?}/$sid"
mkdir -p "$log"
jstr() { python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))'; }
# hook EVENT EXTRA-JSON-FIELDS
hook() {
  {
    echo "--- $1"
    curl -s -m 20 -w '\nhttp=%{http_code}\n' -X POST "${FAKE_CLAUDE_HUB:?}/hook" \
      -H "Host: ${FAKE_CLAUDE_HOST:?}" -H "Authorization: Bearer ${FAKE_CLAUDE_TOKEN:?}" \
      -H "X-Fleet-Pane: ${TMUX_PANE:-}" -H 'Content-Type: application/json' \
      -d "{\"hook_event_name\":\"$1\",\"session_id\":\"$sid\",\"cwd\":$(printf '%s' "$PWD" | jstr)$2}"
  } >>"$log/hooks.log" 2>&1
}

hook SessionStart ',"source":"startup"'
echo "fake claude (hub-e2e) ${name} ${sid}"
echo "? for shortcuts"
while IFS= read -r line; do
  prompt="$line"
  while IFS= read -r -t 0.5 more; do prompt+=$'\n'"$more"; done
  printf '%s\n=====\n' "$prompt" >>"$log/prompts.log"
  hook UserPromptSubmit ",\"prompt\":$(printf '%s' "$prompt" | jstr)"
  nonce=$(printf '%s' "$prompt" | grep -oE 'WORK_HANDOVER_BEGIN_[0-9a-f]+' | head -1 | sed 's/^WORK_HANDOVER_BEGIN_//')
  if [ -n "$nonce" ]; then
    reply="Here is the hand-off.
WORK_HANDOVER_BEGIN_$nonce
E2E-HANDOVER-NOTE: the redirect fix is on the branch; left: the tests.
WORK_HANDOVER_END_$nonce
Anything else?"
  else
    reply="fake claude read your prompt ($(printf '%s' "$prompt" | wc -c | tr -d ' ') bytes)"
  fi
  hook Stop ",\"last_assistant_message\":$(printf '%s' "$reply" | jstr)"
  printf '%s\n? for shortcuts\n' "$reply"
done
