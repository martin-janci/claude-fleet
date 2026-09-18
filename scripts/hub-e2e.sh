#!/usr/bin/env bash
# End-to-end test of a real fleet-hub binary on this machine.
# Isolation: every hub gets its own temp data dir and its own HOME for ssh-key.
# Nothing here provisions a host or edits ~/.claude.json / ~/.ssh.
# Hub A: --local-host true  -> manages this machine's tmux directly.
# Hub B: --local-host false -> must refuse host `local`.
set -uo pipefail

BIN="${BIN:?set BIN to the fleet-hub binary}"
ROOT="$(mktemp -d -p /tmp/claude-1000 hub-e2e.XXXXXX)"
PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); printf 'PASS  %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf 'FAIL  %s\n      %s\n' "$1" "${2:-}"; }
check(){ if eval "$2"; then ok "$1"; else bad "$1" "$3"; fi; }

free_port() { python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()'; }

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
stop_hub() { local pid; pid=$(cat "$ROOT/$1.pid"); kill -TERM "$pid"; wait "$pid"; STOP_RC=$?; }

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
check "state.db is 0600" '[ "$(stat -c %a "$ROOT/a/state.db")" = 600 ]' "$(stat -c %a "$ROOT/a/state.db")"
check "data dir is 0700" '[ "$(stat -c %a "$ROOT/a")" = 700 ]' "$(stat -c %a "$ROOT/a")"
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

echo "== ssh-key in an isolated HOME"
FH="$ROOT/home"; mkdir -p "$FH"
HOME="$FH" "$BIN" ssh-key >"$ROOT/k1" 2>&1
check "ssh-key generates a key" 'grep -q "^ssh-ed25519 " "$ROOT/k1" && [ "$(stat -c %a "$FH/.ssh/id_ed25519")" = 600 ]' "$(cat "$ROOT/k1")"
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
