#!/usr/bin/env bash
# Time the synchronous SessionStart hook (work graph M4.5, decision D5) on a
# REMOTE host, the way Claude Code runs it there, in the four cases the M4
# plan's local table uses, plus the one only a remote host has:
#
#   1  up, answering         the host's real hook URL (the reverse tunnel's
#                            loopback end, or the hub's direct URL)
#   2  down, port refused    a loopback port nothing listens on (= a dead
#                            tunnel)
#   3  host unreachable      a blackhole address (packets dropped; default
#                            192.0.2.1, TEST-NET-1)
#   4  up but not answering  a loopback listener that accepts and never
#                            replies (= a wedged hub)
#   5  hub stopped, tunnel   the real hook URL again, after you stop the hub
#      alive (optional)      while the ssh tunnel stays up (--hub-stopped)
#
# The command timed is byte-for-byte what `hooks_install.rs`
# (`session_start_command_sync`) installs: `curl -sf --connect-timeout 1
# -m 2 -X POST -H @~/.claude/fleet-hook.headers … || true`, with a
# SessionStart body on stdin. Only the URL changes between cases 2–4.
#
# Usage (from your machine; the measurement itself runs on HOST):
#   scripts/measure-session-start.sh --ssh HOST [--runs N] [--url URL]
#       [--pane %N] [--blackhole ADDR] [--hub-stopped]
# Or copy the script to the host and run it there without --ssh.
#
#   --url      the hook URL (default: read from the host's
#              ~/.claude/settings.json SessionStart command)
#   --pane     a tmux pane id to send as X-Fleet-Pane, so the hub builds a
#              real work context for case 1 (use a throwaway session linked
#              to a ticket: the hub treats the POST as that pane's
#              SessionStart). Without it the hub answers 204 with no context,
#              which still times the full round trip through the tunnel.
#   --runs     runs per case (default 10)
#
# Output: one Markdown row per case (min / median / p95 / max, ms, and curl's
# exit code from one untimed probe) to paste into the M4 plan's table. A case
# that did not behave as named (case 1 not answering, case 3 rejected at once
# instead of dropped) prints a warning on stderr. Nothing on the host is
# changed (a temp dir, removed on exit); the token never leaves
# ~/.claude/fleet-hook.headers (curl reads it with -H @file).
# Needs on HOST: bash, curl, and python3 (the case-4 listener and the timer).
set -uo pipefail

RUNS=10
URL=""
PANE=""
BLACKHOLE="192.0.2.1"
HUB_STOPPED=0
SSH_HOST=""

while [ $# -gt 0 ]; do
  case "$1" in
    --ssh) SSH_HOST="${2:?--ssh needs a host}"; shift 2 ;;
    --runs) RUNS="${2:?--runs needs a number}"; shift 2 ;;
    --url) URL="${2:?--url needs a URL}"; shift 2 ;;
    --pane) PANE="${2:?--pane needs a pane id}"; shift 2 ;;
    --blackhole) BLACKHOLE="${2:?--blackhole needs an address}"; shift 2 ;;
    --hub-stopped) HUB_STOPPED=1; shift ;;
    -h|--help) sed -n '2,42p' "$0"; exit 0 ;;
    *) echo "measure-session-start: unknown argument: $1" >&2; exit 2 ;;
  esac
done

case "$RUNS" in
  ''|*[!0-9]*|0) echo "measure-session-start: --runs must be a positive integer" >&2; exit 2 ;;
esac

# Run on the host: ship this script over ssh with the same arguments.
if [ -n "$SSH_HOST" ]; then
  args=(--runs "$RUNS" --blackhole "$BLACKHOLE")
  [ -n "$URL" ] && args+=(--url "$URL")
  [ -n "$PANE" ] && args+=(--pane "$PANE")
  [ "$HUB_STOPPED" = 1 ] && args+=(--hub-stopped)
  # Copied over first, not piped: `-t` (--hub-stopped waits for Enter on the
  # host's terminal) and a script on stdin do not mix.
  rpath=$(ssh "$SSH_HOST" 'mktemp "${TMPDIR:-/tmp}/measure-session-start.XXXXXX"') || exit 1
  ssh "$SSH_HOST" "cat > $(printf '%q' "$rpath")" < "$0" || exit 1
  ssh -t "$SSH_HOST" "bash $(printf '%q ' "$rpath" "${args[@]}"); rm -f $(printf '%q' "$rpath")"
  exit $?
fi

command -v curl >/dev/null || { echo "measure-session-start: curl not found" >&2; exit 1; }
command -v python3 >/dev/null || { echo "measure-session-start: python3 not found" >&2; exit 1; }

HDR="$HOME/.claude/fleet-hook.headers"
if [ -z "$URL" ]; then
  # The installed SessionStart command ends in `--data-binary @- '<url>' || true`.
  URL=$(python3 - "$HOME/.claude/settings.json" <<'PY'
import json, re, sys
try:
    s = json.load(open(sys.argv[1]))
except Exception:
    sys.exit(0)
for group in s.get("hooks", {}).get("SessionStart", []):
    for h in group.get("hooks", []):
        m = re.search(r"--data-binary @- '?([^' ]+/hook)'?", h.get("command", ""))
        if m:
            print(m.group(1)); sys.exit(0)
PY
)
fi
if [ -z "$URL" ]; then
  echo "measure-session-start: no SessionStart hook in ~/.claude/settings.json; pass --url" >&2
  exit 1
fi
SYNC=$(python3 - "$HOME/.claude/settings.json" <<'PY'
import json, sys
try:
    s = json.load(open(sys.argv[1]))
except Exception:
    print("unknown"); sys.exit(0)
cmds = [h.get("command", "") for g in s.get("hooks", {}).get("SessionStart", []) for h in g.get("hooks", [])]
print("sync" if any("--connect-timeout" in c for c in cmds) else ("async" if cmds else "none"))
PY
)

TMP=$(mktemp -d)
LISTENER_PID=""
cleanup() {
  [ -n "$LISTENER_PID" ] && kill "$LISTENER_PID" 2>/dev/null
  rm -rf "$TMP"
}
trap cleanup EXIT

# Cases 2–4 never reach a hub: a throwaway header file keeps the real token
# out of them (and lets them run on a host without one).
DUMMY_HDR="$TMP/dummy.headers"
printf 'Authorization: Bearer measure-session-start\n' > "$DUMMY_HDR"
if [ ! -r "$HDR" ]; then
  echo "measure-session-start: $HDR missing; case 1 and 5 will be refused by the hub (401)" >&2
fi

BODY=$(printf '{"session_id":"measure-session-start-%s","hook_event_name":"SessionStart","source":"startup","cwd":"%s"}' "$$" "$PWD")

# bash 5's EPOCHREALTIME (no process per timestamp); else python3, whose
# start-up is measured once below and subtracted.
if [ -n "${EPOCHREALTIME:-}" ]; then
  now_ms() { local t="${EPOCHREALTIME//[!0-9]/}"; echo $(( t / 1000 )); }
else
  now_ms() { python3 -c 'import time; print(int(time.time()*1000))'; }
fi

# One run of the installed command, with this header file and URL. Prints ms.
one_run() {
  local hdr="$1" url="$2" t0 t1
  t0=$(now_ms)
  printf '%s' "$BODY" | TMUX_PANE="$PANE" bash -c \
    'curl -sf --connect-timeout 1 -m 2 -X POST -H @"$0" -H "X-Fleet-Pane: ${TMUX_PANE:-}" -H '"'"'Content-Type: application/json'"'"' --data-binary @- "$1" || true' \
    "$hdr" "$url" >/dev/null 2>&1
  t1=$(now_ms)
  echo $(( t1 - t0 - OVERHEAD ))
}

t0=$(now_ms); t1=$(now_ms); OVERHEAD=$(( t1 - t0 ))

# min / median / p95 / max of the numbers on stdin.
stats() {
  python3 -c '
import sys, math
xs = sorted(int(x) for x in sys.stdin.read().split())
n = len(xs)
p95 = xs[min(n - 1, math.ceil(0.95 * n) - 1)]
med = xs[n // 2] if n % 2 else (xs[n // 2 - 1] + xs[n // 2]) // 2
print(f"{xs[0]} / {med} / {p95} / {xs[-1]}")
'
}

# curl's exit code for the same request, once and untimed (the installed
# command's `|| true` hides it): 0 answered, 7 refused / no route, 28 timed
# out, 22 an HTTP error (a 401 is a bad or missing token).
probe_rc() {
  local hdr="$1" url="$2"
  printf '%s' "$BODY" | curl -sf --connect-timeout 1 -m 2 -X POST -H @"$hdr" \
    -H "X-Fleet-Pane: ${PANE:-}" -H 'Content-Type: application/json' \
    --data-binary @- "$url" >/dev/null 2>&1
  echo $?
}

measure() {
  local label="$1" hdr="$2" url="$3" i out="" rc
  rc=$(probe_rc "$hdr" "$url")
  for i in $(seq 1 "$RUNS"); do out="$out $(one_run "$hdr" "$url")"; done
  printf '| %s | %s | %s |\n' "$label" "$(echo "$out" | stats)" "$rc"
  LAST_RC="$rc"
}

free_port() {
  python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'
}

# Case 4's listener: accepts (the kernel completes the handshake from the
# backlog) and never answers.
# Started in this shell (never inside `$(…)`, which would keep the pipe open
# and lose the pid); the port is read from a file.
start_silent_listener() {
  local port_file="$TMP/port"
  python3 - "$port_file" >/dev/null 2>&1 <<'PY' &
import socket, sys, time
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", 0))
s.listen(64)
open(sys.argv[1], "w").write(str(s.getsockname()[1]))
conns = []
while True:
    c, _ = s.accept()
    conns.append(c)
PY
  LISTENER_PID=$!
  for _ in $(seq 1 50); do [ -s "$port_file" ] && break; sleep 0.1; done
  SILENT_PORT=$(cat "$port_file" 2>/dev/null)
  [ -n "$SILENT_PORT" ] || { echo "measure-session-start: the silent listener did not start" >&2; exit 1; }
}

REFUSED_PORT=$(free_port)
start_silent_listener

echo "Host: $(hostname) · $(uname -sr) · $(curl --version | head -1 | cut -d' ' -f1-2)"
echo "Hook URL: $URL · installed SessionStart: $SYNC · runs per case: $RUNS${PANE:+ · pane $PANE}"
echo
echo "| Hub (remote host) | min / median / p95 / max, ms | curl exit |"
echo "|---|---|---|"
LAST_RC=""
measure "up, answering${PANE:+ (pane $PANE)}" "$HDR" "$URL"
[ "$LAST_RC" = 0 ] || echo "warning: case 1 did not get an answer (curl exit $LAST_RC): its row is not 'up, answering'" >&2
measure "down, port refused (127.0.0.1:$REFUSED_PORT)" "$DUMMY_HDR" "http://127.0.0.1:$REFUSED_PORT/hook"
measure "host unreachable ($BLACKHOLE)" "$DUMMY_HDR" "http://$BLACKHOLE:8765/hook"
[ "$LAST_RC" = 28 ] || echo "warning: $BLACKHOLE was rejected at once (curl exit $LAST_RC), not dropped: this host has no route to it. Pass --blackhole with an address that routes but never answers (an unused address on the host's LAN)" >&2
measure "up but not answering (silent listener)" "$DUMMY_HDR" "http://127.0.0.1:$SILENT_PORT/hook"

if [ "$HUB_STOPPED" = 1 ]; then
  echo
  echo "Stop the hub now (leave the ssh tunnel up), then press Enter." >&2
  read -r _ </dev/tty
  measure "hub stopped, tunnel alive" "$HDR" "$URL"
fi
