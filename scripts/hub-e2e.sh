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
    code "http://127.0.0.1:$port/mcp" -X POST >/dev/null 2>&1 && [ "$(code "http://127.0.0.1:$port/mcp" -X POST)" != 000 ] && return 0
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

echo
echo "passed $PASS, failed $FAIL   (logs in $ROOT)"
[ "$FAIL" -eq 0 ]
