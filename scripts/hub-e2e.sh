#!/usr/bin/env bash
# End-to-end test of a real fleet-hub binary on this machine.
# Isolation: every hub gets its own temp data dir and its own HOME for ssh-key.
# Nothing here provisions a host or edits ~/.claude.json / ~/.ssh.
# Hub A: --local-host true  -> manages this machine's tmux directly.
# Hub B: --local-host false -> must refuse host `local`.
# Hub C: --local-host false, plus a fleet-agent dialing it over loopback. The
#   agent is started with `run` (never `install`: no systemd, nothing written
#   outside $ROOT), with its own HOME and its own tmux server under $ROOT.
set -uo pipefail

BIN="${BIN:?set BIN to the fleet-hub binary}"
ABIN="${ABIN:-$(dirname "$BIN")/fleet-agent}"
# Prefer the Claude Code sandbox scratch dir when this happens to run inside
# one (short path, already private); otherwise plain /tmp, which is what a
# CI runner and a bare dev box both have. Deliberately /tmp, not $TMPDIR: on
# macOS $TMPDIR is a long per-user path that would push the agent's tmux
# socket (see below) past the 108-byte limit.
TMP_BASE="/tmp/claude-$(id -u)"
[ -d "$TMP_BASE" ] || TMP_BASE=/tmp
ROOT="$(mktemp -d -p "$TMP_BASE" hub-e2e.XXXXXX)" || { echo "hub-e2e: mktemp -p $TMP_BASE failed" >&2; exit 1; }
# The /events subscriber (below) is a background job with no pid file of its
# own; initialised empty here, before the trap is installed, so `set -u`
# never trips on it and cleanup() can always test it safely.
EV_PID=""
# Whatever happens, leave nothing running: every hub and agent this script
# started records a pid file under $ROOT, and the agent's tmux server lives
# under $ROOT/tmux (a short path: a tmux socket path is capped at 108 bytes).
# `${ROOT:?}` on every path built from it: cleanup must never act on a path
# rooted at an empty/unset $ROOT, however a future edit reorders things.
cleanup() {
  local f pid n
  for f in "${ROOT:?}"/*.pid; do
    [ -e "$f" ] || continue
    pid=$(cat "$f"); kill -TERM "$pid" 2>/dev/null || continue
    until_ok 50 '! kill -0 "$pid" 2>/dev/null' || kill -KILL "$pid" 2>/dev/null
    wait "$pid" 2>/dev/null
  done
  [ -d "${ROOT:?}/tmux" ] && env -u TMUX -u TMUX_PANE TMUX_TMPDIR="${ROOT:?}/tmux" tmux kill-server 2>/dev/null
  # Not pid-filed like the hubs/agent above: kill it directly if a SIGTERM
  # lands between it starting and its own explicit `kill "$EV_PID"`.
  if [ -n "$EV_PID" ]; then
    kill "$EV_PID" 2>/dev/null
    wait "$EV_PID" 2>/dev/null
  fi
  # Hub A manages this machine's own (non-isolated) tmux server directly, so a
  # session it created there can outlive a run that is interrupted before its
  # own kill_session step runs. Sweep this run's session names, if any are
  # still around, on that default server too.
  for n in "${NAME:-}" "${NAME2:-}" "${NAME3:-}"; do
    [ -n "$n" ] && tmux has-session -t "$n" 2>/dev/null && tmux kill-session -t "$n" 2>/dev/null
  done
  return 0
}
trap cleanup EXIT
PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); printf 'PASS  %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf 'FAIL  %s\n      %s\n' "$1" "${2:-}"; }
check(){ if eval "$2"; then ok "$1"; else bad "$1" "$3"; fi; }

free_port() { python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()'; }

# filemode PATH -> octal permission bits, portable across GNU (`stat -c`, the
# CI runner) and BSD (`stat -f`, a macOS dev box) stat.
filemode() { stat -c '%a' "$1" 2>/dev/null || stat -f '%OLp' "$1" 2>/dev/null; }

# rpc PORT HOST TOKEN METHOD PARAMS_JSON -> prints the JSON-RPC body (SSE "data:" line stripped)
rpc() {
  curl -s -m 60 -X POST "http://127.0.0.1:$1/mcp" \
    -H "Host: $2" -H "Authorization: Bearer $3" \
    -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$4\",\"params\":$5}" | sed -n 's/^data: //p'
}
tool() { rpc "$1" "$2" "$3" tools/call "{\"name\":\"$4\",\"arguments\":$5}"; }
code() { curl -s -o /dev/null -w '%{http_code}' -m 10 "$@"; }

start_hub() { # name port extra-args...
  local name=$1 port=$2; shift 2
  "$BIN" serve --data-dir "$ROOT/$name" --port "$port" "$@" >"$ROOT/$name.log" 2>&1 &
  echo $! >"$ROOT/$name.pid"
  for _ in $(seq 50); do
    [ "$(code "http://127.0.0.1:$port/healthz")" = 200 ] && return 0
    sleep 0.2
  done
  return 1
}
stop_hub() { local pid; pid=$(cat "$ROOT/$1.pid"); kill -TERM "$pid"; wait "$pid"; STOP_RC=$?; rm -f "$ROOT/$1.pid"; }
# until TRIES CONDITION: poll every 0.2 s, at most TRIES times.
until_ok() { local n=$1 _; for _ in $(seq "$n"); do eval "$2" && return 0; sleep 0.2; done; return 1; }

echo "== CLI basics"
check "version flag" '"$BIN" --version | grep -q fleet-hub' "$("$BIN" --version 2>&1)"
out=$("$BIN" token --data-dir "$ROOT/missing" show 2>&1); rc=$?
check "token show on a missing database exits 1" '[ $rc -ne 0 ] && echo "$out" | grep -qi "init"' "$out"
check "token show on a missing database creates nothing" '[ ! -e "$ROOT/missing/state.db" ]' "state.db was created"
out=$("$BIN" init --data-dir "$ROOT/plain" --bind 0.0.0.0 2>&1); rc=$?
check "routable bind without https or --allow-plaintext is refused" '[ $rc -ne 0 ] && echo "$out" | grep -q allow-plaintext' "$out"

PA=$(free_port); PB=$(free_port)
PUB=fleet.example.com

echo "== Hub A (local host on)"
TOKA=$("$BIN" init --data-dir "$ROOT/a" --public-url "https://$PUB" --port "$PA" --local-host true 2>&1 | grep -E '^[0-9a-f]{64}$')
check "init prints a 64-hex token" '[ ${#TOKA} -eq 64 ]' "got '${TOKA}'"
check "state.db is 0600" '[ "$(filemode "$ROOT/a/state.db")" = 600 ]' "$(filemode "$ROOT/a/state.db")"
check "data dir is 0700" '[ "$(filemode "$ROOT/a")" = 700 ]' "$(filemode "$ROOT/a")"
check "token show prints the same token" '[ "$("$BIN" token show --data-dir "$ROOT/a")" = "$TOKA" ]' "mismatch"
start_hub a "$PA" --public-url "https://$PUB" --local-host true || bad "hub A starts" "$(tail -5 "$ROOT/a.log")"
check "healthcheck healthy while running" '"$BIN" healthcheck --port "$PA" >/dev/null 2>&1' "$("$BIN" healthcheck --port "$PA" 2>&1)"
check "/healthz needs no token and no allowlisted Host" '[ "$(code "http://127.0.0.1:$PA/healthz" -H "Host: evil.example.com")" = 200 ]' "$(code "http://127.0.0.1:$PA/healthz" -H "Host: evil.example.com")"
check "/healthz answers the liveness body and nothing else" 'curl -s -m 10 "http://127.0.0.1:$PA/healthz" | grep -qx "fleet-hub ok"' "$(curl -s -m 10 "http://127.0.0.1:$PA/healthz")"
check "/healthz refuses a non-GET with 405" '[ "$(code -X POST "http://127.0.0.1:$PA/healthz")" = 405 ]' "$(code -X POST "http://127.0.0.1:$PA/healthz")"
check "no token -> 401" '[ "$(code -X POST "http://127.0.0.1:$PA/mcp" -H "Host: $PUB")" = 401 ]' ""
check "wrong token -> 401" '[ "$(code -X POST "http://127.0.0.1:$PA/mcp" -H "Host: $PUB" -H "Authorization: Bearer nope")" = 401 ]' ""
check "foreign Host -> 403" '[ "$(code -X POST "http://127.0.0.1:$PA/mcp" -H "Host: evil.example.com" -H "Authorization: Bearer $TOKA")" = 403 ]' ""
init=$(rpc "$PA" "$PUB" "$TOKA" initialize '{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}')
check "MCP initialize over the public Host" 'echo "$init" | grep -q serverInfo' "$init"
list=$(rpc "$PA" "$PUB" "$TOKA" tools/list '{}')
check "tools/list returns the fleet tools" 'echo "$list" | grep -q list_sessions' "${list:0:300}"
h=$(tool "$PA" "$PUB" "$TOKA" fleet_health '{}')
check "fleet_health reports the app version, not 0.1.0" 'echo "$h" | grep -q "\\\\\"version\\\\\": \\\\\"0.2" ' "${h:0:300}"
hosts=$(tool "$PA" "$PUB" "$TOKA" list_sessions '{"force":true}')
check "list_sessions (forced reconcile of local) succeeds" 'echo "$hosts" | grep -q "\"isError\":false"' "${hosts:0:400}"
NAME="hube2e$RANDOM"
refr=$(tool "$PA" "$PUB" "$TOKA" refresh_projects '{}')
check "refresh_projects scans this machine" 'echo "$refr" | grep -q "\"isError\":false"' "${refr:0:300}"
projs=$(tool "$PA" "$PUB" "$TOKA" list_projects '{}')
check "list_projects finds this machine's repositories" 'echo "$projs" | grep -q claude-fleet' "${projs:0:300}"
PID_=$(echo "$projs" | grep -oE '\\"id\\": ?[0-9]+' | head -1 | grep -oE '[0-9]+$')
mk=$(tool "$PA" "$PUB" "$TOKA" new_shell_session "{\"host_alias\":\"local\",\"project_id\":${PID_:-0},\"name\":\"$NAME\"}")
check "new_shell_session on local creates a tmux session" 'tmux has-session -t "$NAME" 2>/dev/null' "${mk:0:400}"
SID=$(echo "$mk" | grep -oE '\\"id\\": ?[0-9]+' | head -1 | grep -oE '[0-9]+$')
if [ -n "$SID" ]; then
  tmux send-keys -t "$NAME" "echo hub-e2e-marker" Enter; sleep 1
  cap=$(tool "$PA" "$PUB" "$TOKA" capture_session "{\"session_id\":$SID}")
  check "capture_session sees the marker" 'echo "$cap" | grep -q hub-e2e-marker' "${cap:0:400}"
  k=$(tool "$PA" "$PUB" "$TOKA" kill_session "{\"session_id\":$SID}")
  check "kill_session removes the tmux session" '! tmux has-session -t "$NAME" 2>/dev/null' "${k:0:400}"
else
  bad "session id parsed from new_shell_session" "${mk:0:400}"
fi
check "hook with master token, unknown session -> 204" '[ "$(code -X POST "http://127.0.0.1:$PA/hook" -H "Host: $PUB" -H "Authorization: Bearer $TOKA" -H "Content-Type: application/json" -d "{\"hook_event_name\":\"Stop\",\"session_id\":\"00000000-0000-0000-0000-000000000000\"}")" = 204 ]' ""
check "legacy ?token= on a public hub -> 401" '[ "$(code -X POST "http://127.0.0.1:$PA/hook?token=$TOKA" -H "Host: $PUB" -H "Content-Type: application/json" -d "{}")" = 401 ]' ""

echo "== Client access (pairing, /events, a client token's reach)"
# POST /pair is rate-limited to one attempt per address per ATTEMPT_INTERVAL,
# so every redemption below is spaced by that budget. Kept as one named
# constant so a change to it is a one-line edit here too.
PAIR_INTERVAL=7
redeem() { # code -> the /pair response body
  curl -s -m 10 -X POST "http://127.0.0.1:$PA/pair" \
    -H "Host: $PUB" -H 'Content-Type: application/json' -d "{\"code\":\"$1\"}"
}
pair_code() { # tool-response -> the 8-character code out of the escaped JSON
  echo "$1" | grep -oE '\\"code\\": ?\\"[0-9A-Z]{8}\\"' | grep -oE '[0-9A-Z]{8}' | head -1
}

# A pairing code is minted through the tool, never written straight into the
# store — `fleet-hub pair` drives this same tool.
pc=$(tool "$PA" "$PUB" "$TOKA" pair_client '{"name":"e2e phone","mode":"full"}')
CODE=$(pair_code "$pc")
check "pair_client mints a single-use code" '[ ${#CODE} -eq 8 ]' "${pc:0:400}"
check "the pairing URL carries the code in the FRAGMENT, not the query" 'echo "$pc" | grep -q "/pair#$CODE"' "${pc:0:400}"
pcro=$(tool "$PA" "$PUB" "$TOKA" pair_client '{"name":"e2e kiosk","mode":"readonly"}')
CODE_RO=$(pair_code "$pcro")
check "pair_client mints a readonly code too" '[ ${#CODE_RO} -eq 8 ]' "${pcro:0:400}"
# A client token is master-only to mint.
asclient=$(tool "$PA" "$PUB" "$TOKA" list_clients '{"include_revoked":false}')
check "list_clients shows the pending names only once paired (none yet)" '! echo "$asclient" | grep -q "e2e phone"' "${asclient:0:400}"

body=$(redeem "$CODE")
CTOK=$(echo "$body" | grep -oE '"token":"[0-9a-f]{64}"' | grep -oE '[0-9a-f]{64}')
check "POST /pair exchanges the code for a 64-hex client token" '[ ${#CTOK} -eq 64 ]' "$body"
check "the /pair answer names the client and its mode" 'echo "$body" | grep -q "\"name\":\"e2e phone\"" && echo "$body" | grep -q "\"mode\":\"full\""' "$body"
sleep "$PAIR_INTERVAL"
# One attempt, captured once: the rate limiter spends a budget per request, so
# a second call just to render the failure detail would itself answer 429.
replay=$(code -X POST "http://127.0.0.1:$PA/pair" -H "Host: $PUB" -H "Content-Type: application/json" -d "{\"code\":\"$CODE\"}")
check "the same code fails the second time -> 404" '[ "$replay" = 404 ]' "got $replay"
sleep "$PAIR_INTERVAL"
rbody=$(redeem "$CODE_RO")
RTOK=$(echo "$rbody" | grep -oE '"token":"[0-9a-f]{64}"' | grep -oE '[0-9a-f]{64}')
check "a readonly client pairs the same way" '[ ${#RTOK} -eq 64 ] && echo "$rbody" | grep -q "\"mode\":\"readonly\""' "$rbody"

ls_c=$(tool "$PA" "$PUB" "$CTOK" list_sessions '{}')
check "a client token may list sessions" 'echo "$ls_c" | grep -q "\"isError\":false"' "${ls_c:0:400}"
ls_r=$(tool "$PA" "$PUB" "$RTOK" list_sessions '{}')
check "a readonly client may list sessions" 'echo "$ls_r" | grep -q "\"isError\":false"' "${ls_r:0:400}"
sp_r=$(tool "$PA" "$PUB" "$RTOK" send_prompt '{"session_id":1,"prompt":"hello"}')
check "a readonly client is refused send_prompt" 'echo "$sp_r" | grep -q E_FORBIDDEN' "${sp_r:0:400}"
pv_c=$(tool "$PA" "$PUB" "$CTOK" provision_hosts '{}')
check "a client is refused provision_hosts even in full mode" 'echo "$pv_c" | grep -q E_FORBIDDEN' "${pv_c:0:400}"
pr_c=$(tool "$PA" "$PUB" "$CTOK" pair_client '{"name":"a second phone"}')
check "a client cannot pair another client" 'echo "$pr_c" | grep -q E_FORBIDDEN' "${pr_c:0:400}"

# --- GET /events -------------------------------------------------------------
check "/events needs a token like /mcp" '[ "$(code "http://127.0.0.1:$PA/events" -H "Host: $PUB")" = 401 ]' "$(code "http://127.0.0.1:$PA/events" -H "Host: $PUB")"
SSE="$ROOT/events.sse"
# `-m` only bounds a stream nothing ever closes; every wait below is its own
# bounded poll, and the subscriber is killed as soon as the checks are done.
curl -sN -m 180 "http://127.0.0.1:$PA/events?kinds=session" \
  -H "Host: $PUB" -H "Authorization: Bearer $CTOK" >"$SSE" 2>/dev/null &
EV_PID=$!
await() { for _ in $(seq 120); do grep -q "^event: $1" "$SSE" 2>/dev/null && return 0; sleep 0.25; done; return 1; }
await ready
check "/events opens with a ready frame naming the kinds" 'grep -q "^event: ready" "$SSE" && grep -q "session" "$SSE"' "$(head -5 "$SSE" 2>/dev/null)"
NAME3="hube2eE$RANDOM"
mk3=$(tool "$PA" "$PUB" "$TOKA" new_shell_session "{\"host_alias\":\"local\",\"project_id\":${PID_:-0},\"name\":\"$NAME3\"}")
SID3=$(echo "$mk3" | grep -oE '\\"id\\": ?[0-9]+' | head -1 | grep -oE '[0-9]+$')
await session:created
check "a row change reaches the stream as its own frame" 'grep -q "^event: session:created" "$SSE"' "$(head -20 "$SSE" 2>/dev/null)"
check "the frame carries the row payload, not just a name" 'grep -A1 "^event: session:created" "$SSE" | grep -q "$NAME3"' "$(head -20 "$SSE" 2>/dev/null)"
# A shell session has no Claude transcript, so the tool must answer one of the
# two codes its description documents — never a 500 and never an empty result.
# Which of the two depends on whether reconcile has yet attached a
# claude_session_id to the pane, so both are accepted.
conv=$(tool "$PA" "$PUB" "$CTOK" session_conversation "{\"session_id\":${SID3:-0}}")
check "session_conversation on a session with no transcript -> a documented error" 'echo "$conv" | grep -qE "E_INVALID_STATE|E_NO_TRANSCRIPT"' "session_id=${SID3:-0} ${conv:0:400}"
k3=$(tool "$PA" "$PUB" "$TOKA" kill_session "{\"session_id\":${SID3:-0}}")
# A kill's own reconcile only marks the row `ghost` (that is a session:updated
# frame); the hard delete — and with it session:killed — happens on the NEXT
# pass, so that a session missing from one probe is not deleted on the strength
# of that single probe. Force that pass rather than waiting for the tick.
tool "$PA" "$PUB" "$TOKA" list_sessions '{"force":true}' >/dev/null
await session:killed
check "killing the session streams session:killed too" 'grep -q "^event: session:killed" "$SSE"' "$(tail -20 "$SSE" 2>/dev/null)"
kill "$EV_PID" 2>/dev/null; wait "$EV_PID" 2>/dev/null

# --- the operator's CLI, and revocation ---------------------------------------
tbl=$("$BIN" client list --data-dir "$ROOT/a" --port "$PA" 2>&1)
check "fleet-hub client list shows both paired clients and no digest" 'echo "$tbl" | grep -q "e2e phone" && echo "$tbl" | grep -q "e2e kiosk" && ! echo "$tbl" | grep -qi token' "$tbl"
rev=$("$BIN" client revoke "e2e phone" --data-dir "$ROOT/a" --port "$PA" 2>&1); rc=$?
check "fleet-hub client revoke reports the revoked client" '[ $rc -eq 0 ] && echo "$rev" | grep -q "e2e phone"' "$rev"
check "a revoked client's token is refused -> 401" '[ "$(code -X POST "http://127.0.0.1:$PA/mcp" -H "Host: $PUB" -H "Authorization: Bearer $CTOK")" = 401 ]' "$(code -X POST "http://127.0.0.1:$PA/mcp" -H "Host: $PUB" -H "Authorization: Bearer $CTOK")"
check "the other client still works" '[ "$(code -X POST "http://127.0.0.1:$PA/mcp" -H "Host: $PUB" -H "Authorization: Bearer $RTOK" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\",\"params\":{}}")" = 200 ]' ""
tbl2=$("$BIN" client list --include-revoked --data-dir "$ROOT/a" --port "$PA" 2>&1)
check "a revoked client stays listed for the audit trail" 'echo "$tbl2" | grep -q "e2e phone"' "$tbl2"
check "and is gone from the live list" '! "$BIN" client list --data-dir "$ROOT/a" --port "$PA" 2>&1 | grep -q "e2e phone"' "$("$BIN" client list --data-dir "$ROOT/a" --port "$PA" 2>&1)"

stop_hub a
check "SIGTERM exits 0" '[ "$STOP_RC" = 0 ]' "exit $STOP_RC"
check "log shows the drain" 'grep -q "fleet-hub stopping" "$ROOT/a.log"' "$(tail -5 "$ROOT/a.log")"
check "healthcheck unhealthy after stop" '! "$BIN" healthcheck --port "$PA" >/dev/null 2>&1' "still healthy"

echo "== Hub B (local host off)"
TOKB=$("$BIN" init --data-dir "$ROOT/b" --public-url "https://$PUB" --port "$PB" 2>&1 | grep -E '^[0-9a-f]{64}$')
start_hub b "$PB" --public-url "https://$PUB" || bad "hub B starts" "$(tail -5 "$ROOT/b.log")"
lh=$(tool "$PB" "$PUB" "$TOKB" list_hosts '{}')
check "no local host is listed" '! echo "$lh" | grep -q "\\\\\"alias\\\\\": \\\\\"local\\\\\""' "${lh:0:400}"
NAME2="hube2eB$RANDOM"
r=$(tool "$PB" "$PUB" "$TOKB" new_shell_session "{\"host_alias\":\"local\",\"project_id\":1,\"name\":\"$NAME2\"}")
check "new_shell_session on local is refused with E_NOTFOUND" 'echo "$r" | grep -q E_NOTFOUND && echo "$r" | grep -q "hub.local_host"' "${r:0:400}"
check "and no tmux session was created" '! tmux has-session -t "$NAME2" 2>/dev/null' "session exists"
stop_hub b
check "hub B SIGTERM exits 0" '[ "$STOP_RC" = 0 ]' "exit $STOP_RC"

echo "== Agent leg (a fleet-agent dialing hub C over loopback)"
# The whole agent side runs with this environment: its own HOME, its own tmux
# server, and no $TMUX/$NOTIFY_SOCKET leaking in from whatever runs the script
# (a tmux client with $TMUX set talks to THAT server, not TMUX_TMPDIR's).
AHOME="$ROOT/agent-home"; mkdir -p "$AHOME" "$ROOT/tmux"; chmod 700 "$ROOT/tmux"
AENV=(env -u TMUX -u TMUX_PANE -u NOTIFY_SOCKET HOME="$AHOME" XDG_CONFIG_HOME="$AHOME/.config" TMUX_TMPDIR="$ROOT/tmux")
aenv() { "${AENV[@]}" "$@"; }
start_agent() { # name token: `run`, never `install`; the token goes in on stdin
  # A simple command, not a function, in the background: `env` execs the
  # agent, so $! is the agent itself and SIGTERM reaches it (a backgrounded
  # function would be a subshell, and killing it would orphan the agent).
  "${AENV[@]}" "$ABIN" run --hub "http://127.0.0.1:$PC" --insecure --token-file - <<<"$2" >"$ROOT/$1.log" 2>&1 &
  echo $! >"$ROOT/$1.pid"
}
# SIGTERM, then at most 15 s for it to go (the agent promises 10 s); past
# that it is KILLed, so the wait below cannot hang and the exit code says 137.
stop_agent() { local pid; pid=$(cat "$ROOT/$1.pid"); kill -TERM "$pid"
  until_ok 75 '! kill -0 "$pid" 2>/dev/null' || kill -KILL "$pid" 2>/dev/null
  wait "$pid"; STOP_RC=$?; rm -f "$ROOT/$1.pid"; }
connected() { tool "$PC" "$PUB" "$TOKC" agent_status '{}' | grep -q '\\"connected\\": true'; }
AH=e2eagent
check "fleet-agent binary is there" '[ -x "$ABIN" ] && "$ABIN" --version | grep -q fleet-agent' "ABIN=$ABIN"
out=$(aenv "$ABIN" run --hub http://fleet.example.com --insecure --token-file - <<<"$(printf 'a%.0s' $(seq 64))" 2>&1); rc=$?
check "--insecure is refused for a hub that is not on loopback" '[ $rc -ne 0 ] && echo "$out" | grep -qi loopback' "$out"

PC=$(free_port)
TOKC=$("$BIN" init --data-dir "$ROOT/c" --public-url "https://$PUB" --port "$PC" 2>&1 | grep -E '^[0-9a-f]{64}$')
start_hub c "$PC" --public-url "https://$PUB" || bad "hub C starts" "$(tail -5 "$ROOT/c.log")"
out=$("$BIN" agent-token "$AH" --data-dir "$ROOT/c" 2>&1); rc=$?
check "agent-token for a host that is not registered fails" '[ $rc -ne 0 ] && echo "$out" | grep -q "no host named"' "$out"
ah=$(tool "$PC" "$PUB" "$TOKC" add_host "{\"alias\":\"$AH\",\"ssh_alias\":\"$AH\",\"transport\":\"agent\"}")
check "add_host transport=agent saves it unprobed and unreachable" 'echo "$ah" | grep -q "\"isError\":false" && echo "$ah" | grep -q "\\\\\"transport\\\\\": \\\\\"agent\\\\\"" && echo "$ah" | grep -q "\\\\\"reachable\\\\\": false"' "${ah:0:400}"
st=$(tool "$PC" "$PUB" "$TOKC" agent_status '{}')
check "agent_status lists the host as not connected before any agent" 'echo "$st" | grep -q "\\\\\"alias\\\\\": \\\\\"$AH\\\\\"" && echo "$st" | grep -q "\\\\\"connected\\\\\": false"' "${st:0:400}"
# The first token reaches the host out of band, the way an operator does it.
ATOK=$("$BIN" agent-token "$AH" --data-dir "$ROOT/c" 2>"$ROOT/atok.err"); rc=$?
check "agent-token mints the host's first token (only the token on stdout)" '[ $rc -eq 0 ] && echo "$ATOK" | grep -qxE "[0-9a-f]{64}"' "rc=$rc stdout='${ATOK:0:80}' stderr=$(cat "$ROOT/atok.err")"
check "agent-token says on stderr that it minted one" 'grep -q "new token" "$ROOT/atok.err"' "$(cat "$ROOT/atok.err")"
check "agent-token again prints the same token, minting nothing" '[ "$("$BIN" agent-token "$AH" --data-dir "$ROOT/c" 2>/dev/null)" = "$ATOK" ]' "a second call changed the token"

start_agent agent1 "$ATOK"
until_ok 50 connected
check "agent_status shows the agent connected, with its version" 'tool "$PC" "$PUB" "$TOKC" agent_status "{}" | grep -q "\\\\\"agent_version\\\\\": \\\\\"0\."' "$(tool "$PC" "$PUB" "$TOKC" agent_status '{}' | head -c 400) / agent: $(tail -3 "$ROOT/agent1.log")"
pr=$(tool "$PC" "$PUB" "$TOKC" probe_host "{\"alias\":\"$AH\"}")
check "probe_host over the agent: reachable, tmux version read" 'echo "$pr" | grep -q "\\\\\"reachable\\\\\": true" && echo "$pr" | grep -q "\\\\\"tmux_version\\\\\": \\\\\""' "${pr:0:400}"
# A session on the agent's own tmux server; everything after this goes through
# the agent: the reconcile that finds it, send-keys, capture-pane, kill-session.
aenv tmux new-session -d -s agt1 -c "$AHOME" "echo agent-e2e-marker; exec bash --noprofile --norc"
aenv tmux new-session -d -s agt2 -c "$AHOME" "exec bash --noprofile --norc"
ls_a=$(tool "$PC" "$PUB" "$TOKC" list_sessions "{\"host_alias\":\"$AH\",\"force\":true}")
check "list_sessions finds the host's tmux sessions over the agent" 'echo "$ls_a" | grep -q agt1 && echo "$ls_a" | grep -q agt2' "${ls_a:0:400}"
sid_of() { echo "$ls_a" | grep -oE "\\\\\"id\\\\\":[0-9]+,[^}]*\\\\\"tmux_name\\\\\":\\\\\"$1\\\\\"" | grep -oE '^\\"id\\":[0-9]+' | grep -oE '[0-9]+'; }
S1=$(sid_of agt1); S2=$(sid_of agt2)
check "session ids parsed" '[ -n "$S1" ] && [ -n "$S2" ]' "${ls_a:0:400}"
cap=$(tool "$PC" "$PUB" "$TOKC" capture_session "{\"session_id\":${S1:-0}}")
check "capture_session reads the pane over the agent" 'echo "$cap" | grep -q agent-e2e-marker' "${cap:0:400}"
tool "$PC" "$PUB" "$TOKC" send_prompt "{\"session_id\":${S1:-0},\"prompt\":\"echo sum-\$((40+2))\"}" >/dev/null
until_ok 50 'tool "$PC" "$PUB" "$TOKC" capture_session "{\"session_id\":${S1:-0}}" | grep -q "sum-42"'
check "send_prompt types into the pane over the agent (the shell ran it)" 'tool "$PC" "$PUB" "$TOKC" capture_session "{\"session_id\":${S1:-0}}" | grep -q "sum-42"' "$(tool "$PC" "$PUB" "$TOKC" capture_session "{\"session_id\":${S1:-0}}" | head -c 400)"
k=$(tool "$PC" "$PUB" "$TOKC" kill_session "{\"session_id\":${S1:-0}}")
until_ok 25 '! aenv tmux has-session -t agt1 2>/dev/null'
check "kill_session kills the tmux session over the agent" '! aenv tmux has-session -t agt1 2>/dev/null' "${k:0:400}"

# Rotation: the new token is committed and sent NOTHING over the connection it
# replaces; the next routed call finds the old connection stale and drops it.
NTOK=$("$BIN" agent-token "$AH" --rotate --data-dir "$ROOT/c" 2>/dev/null)
check "agent-token --rotate prints a new token" 'echo "$NTOK" | grep -qxE "[0-9a-f]{64}" && [ "$NTOK" != "$ATOK" ]' "'${NTOK:0:80}'"
c2=$(tool "$PC" "$PUB" "$TOKC" capture_session "{\"session_id\":${S2:-0}}")
check "after a rotation the agent on the old token is cut off -> E_AGENT_OFFLINE" 'echo "$c2" | grep -q E_AGENT_OFFLINE' "${c2:0:400}"
check "and agent_status shows it disconnected" '! connected' "$(tool "$PC" "$PUB" "$TOKC" agent_status '{}' | head -c 400)"
until_ok 50 'grep -q 401 "$ROOT/agent1.log"'
check "the stale agent was refused on redial (401)" 'grep -q 401 "$ROOT/agent1.log"' "$(tail -5 "$ROOT/agent1.log")"
stop_agent agent1
check "the stale agent exits on SIGTERM" '[ "$STOP_RC" = 0 ]' "exit $STOP_RC / $(tail -3 "$ROOT/agent1.log")"
start_agent agent2 "$NTOK"
until_ok 50 connected
check "the agent re-installed with the new token connects again" 'connected' "$(tail -3 "$ROOT/agent2.log")"
c3=$(tool "$PC" "$PUB" "$TOKC" capture_session "{\"session_id\":${S2:-0}}")
check "session commands work again over the new connection" 'echo "$c3" | grep -q "\"isError\":false"' "${c3:0:400}"

# Stopping the agent: it exits cleanly, the hub fails calls at once, and the
# tmux server the agent's commands started outlives it.
stop_agent agent2
check "fleet-agent exits 0 on SIGTERM" '[ "$STOP_RC" = 0 ]' "exit $STOP_RC / $(tail -3 "$ROOT/agent2.log")"
until_ok 50 '! connected'
check "agent_status shows the stopped agent disconnected" '! connected' "$(tool "$PC" "$PUB" "$TOKC" agent_status '{}' | head -c 400)"
t0=$(date +%s%N)
c4=$(tool "$PC" "$PUB" "$TOKC" capture_session "{\"session_id\":${S2:-0}}")
ms=$(( ($(date +%s%N) - t0) / 1000000 ))
check "a call for the host fails with E_AGENT_OFFLINE" 'echo "$c4" | grep -q E_AGENT_OFFLINE' "${c4:0:400}"
check "and fails at once, not after a timeout (<5 s)" '[ "$ms" -lt 5000 ]' "took ${ms} ms"
check "the tmux session survives the agent stopping" 'aenv tmux has-session -t agt2 2>/dev/null' "agt2 is gone"
stop_hub c
check "hub C SIGTERM exits 0" '[ "$STOP_RC" = 0 ]' "exit $STOP_RC"

echo "== ssh-key in an isolated HOME"
FH="$ROOT/home"; mkdir -p "$FH"
HOME="$FH" "$BIN" ssh-key >"$ROOT/k1" 2>&1
check "ssh-key generates a key" 'grep -q "^ssh-ed25519 " "$ROOT/k1" && [ "$(filemode "$FH/.ssh/id_ed25519")" = 600 ]' "$(cat "$ROOT/k1")"
sum=$(sha256sum "$FH/.ssh/id_ed25519" | cut -d" " -f1); rm "$FH/.ssh/id_ed25519.pub"
HOME="$FH" "$BIN" ssh-key >"$ROOT/k2" 2>&1
check "private key only -> public key derived, private key untouched" 'grep -q "^ssh-ed25519 " "$ROOT/k2" && [ "$(sha256sum "$FH/.ssh/id_ed25519" | cut -d" " -f1)" = "$sum" ]' "$(cat "$ROOT/k2")"
mv "$FH/.ssh/id_ed25519" "$ROOT/parked"
HOME="$FH" "$BIN" ssh-key >"$ROOT/k3" 2>"$ROOT/k3.err"
check "public key without its private half is printed with a warning" 'grep -q "^ssh-ed25519 " "$ROOT/k3" && grep -qi "private key" "$ROOT/k3.err" && ! grep -qi "warning" "$ROOT/k3"' "$(cat "$ROOT/k3" "$ROOT/k3.err")"
mv "$ROOT/parked" "$FH/.ssh/id_ed25519"

echo
echo "passed $PASS, failed $FAIL   (logs in $ROOT)"
[ "$FAIL" -eq 0 ]
