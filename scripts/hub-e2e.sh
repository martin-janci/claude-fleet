#!/usr/bin/env bash
# End-to-end test of a real fleet-hub binary on this machine.
# Isolation: every hub gets its own temp data dir and its own HOME for ssh-key.
# Nothing here provisions a host or edits ~/.claude.json / ~/.ssh.
# Hub A: --local-host true  -> manages this machine's tmux directly (the real
#   default tmux server, by design: that IS the behavior under test). Its
#   PROJECT list is hermetic, though: a throwaway fixture repo under $ROOT,
#   found via CLAUDE_FLEET_PROJECTS_BASE (the supported env override in
#   crates/fleet-core/src/service/projects.rs, `local` host only), not this
#   checkout's own path or a real ~/projects layout -- a CI runner's checkout
#   does not live at the developer's path shape, so the old "grep the project
#   list for claude-fleet" check found nothing there and left project_id at
#   its 0 fallback. HOME is left alone for Hub A (unlike the agent leg below),
#   so it still reads this account's real ~/.claude and ~/.ssh -- nothing here
#   needed to change that to make discovery hermetic.
# Hub B: --local-host false -> must refuse host `local`.
# Hub C: --local-host false, plus a fleet-agent dialing it over loopback. The
#   agent is started with `run` (never `install`: no systemd, nothing written
#   outside $ROOT), with its own HOME and its own tmux server under $ROOT.
set -uo pipefail

BIN="${BIN:?set BIN to the fleet-hub binary}"
ABIN="${ABIN:-$(dirname "$BIN")/fleet-agent}"
# Both binaries, checked before anything is created or started. A wrong path
# otherwise cascades into ~79 unrelated failures (every check that needs a
# running hub), which buries the one thing actually wrong.
for b in "$BIN" "$ABIN"; do
  if [ ! -f "$b" ] || [ ! -x "$b" ]; then
    echo "hub-e2e: not an executable file: $b" >&2
    echo "hub-e2e: set BIN and ABIN to the built fleet-hub and fleet-agent binaries" >&2
    exit 2
  fi
done
# Prefer the Claude Code sandbox scratch dir when this happens to run inside
# one (short path, already private); otherwise plain /tmp, which is what a
# CI runner and a bare dev box both have. Deliberately /tmp, not $TMPDIR: on
# macOS $TMPDIR is a long per-user path that would push the agent's tmux
# socket (see below) past the 108-byte limit.
TMP_BASE="/tmp/claude-$(id -u)"
[ -d "$TMP_BASE" ] || TMP_BASE=/tmp
ROOT="$(mktemp -d -p "$TMP_BASE" hub-e2e.XXXXXX)" || { echo "hub-e2e: mktemp -p $TMP_BASE failed" >&2; exit 1; }
# Announced up front, on its own greppable line, and exported to the workflow
# when there is one: every hub and agent log lands under $ROOT and nothing
# here deletes it, so CI's `if: failure()` step can collect it. The closing
# "(logs in $ROOT)" line only prints when the script reaches its own end — a
# run that dies early, or is killed by a step timeout, never gets there.
echo "hub-e2e: log root: $ROOT"
[ -n "${GITHUB_ENV:-}" ] && echo "HUBE2E_ROOT=$ROOT" >>"$GITHUB_ENV"
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
  for n in "${NAME:-}" "${NAME2:-}" "${NAME3:-}" "${NAME4:-}" "${NAME5:-}"; do
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

# A private projects root for Hub A's local project discovery, entirely under
# $ROOT: one deterministic fixture repo, plain git, no global config touched
# (identity passed with -c, never written to ~/.gitconfig), default branch
# set explicitly so this does not depend on the runner's init.defaultBranch,
# and a fake, never-contacted origin (the Github layout derives the owner
# from the directory name, not the remote, but a real checkout normally has
# one, so the fixture does too).
PROJ_BASE="$ROOT/projects-base"
FIXTURE="$PROJ_BASE/e2e/hub-e2e-fixture"
mkdir -p "$FIXTURE"
# -c commit.gpgsign=false: without it, a developer machine with a global
# commit.gpgsign=true can stall this unattended commit on a pinentry prompt
# instead of failing fast -- the same hazard add_project.rs calls out at
# ~827/~4301 for its own commits ("a prior commit failed, e.g.
# commit.gpgsign with no TTY"). No tag.gpgsign: this fixture never tags.
# GIT_CONFIG_GLOBAL/SYSTEM=/dev/null on all three calls is a second, broader
# guard: it blanks any OTHER inherited global/system git config too (hooks,
# templates, url.insteadOf rewrites), not just gpgsign -- git >=2.32 (2021),
# checked locally on 2.53 and safe on the ubuntu-24.04 runner's default git
# (2.43); `-b main` on `git init` (>=2.28) is unaffected by blanking those
# files since it never depends on init.defaultBranch to begin with.
GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null git init -q -b main "$FIXTURE"
GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null git -C "$FIXTURE" \
  -c user.name="hub-e2e" -c user.email="hub-e2e@example.invalid" -c commit.gpgsign=false \
  commit -q --allow-empty -m "hub-e2e fixture"
GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null git -C "$FIXTURE" remote add origin https://example.invalid/e2e/hub-e2e-fixture.git

echo "== Hub A (local host on)"
TOKA=$("$BIN" init --data-dir "$ROOT/a" --public-url "https://$PUB" --port "$PA" --local-host true 2>&1 | grep -E '^[0-9a-f]{64}$')
check "init prints a 64-hex token" '[ ${#TOKA} -eq 64 ]' "got '${TOKA}'"
check "state.db is 0600" '[ "$(filemode "$ROOT/a/state.db")" = 600 ]' "$(filemode "$ROOT/a/state.db")"
check "data dir is 0700" '[ "$(filemode "$ROOT/a")" = 700 ]' "$(filemode "$ROOT/a")"
check "token show prints the same token" '[ "$("$BIN" token show --data-dir "$ROOT/a")" = "$TOKA" ]' "mismatch"
# CLAUDE_FLEET_PROJECTS_BASE is exported only around Hub A's own start: the
# background `serve` process inherits it at fork and keeps it for its whole
# life (local_env_base() re-reads the process env on every call, so this does
# not need to stay exported in this shell). Hub B/C never scan `local`
# projects, so this would be harmless even left set, but scoping it keeps the
# intent obvious.
export CLAUDE_FLEET_PROJECTS_BASE="$PROJ_BASE"
start_hub a "$PA" --public-url "https://$PUB" --local-host true || bad "hub A starts" "$(tail -5 "$ROOT/a.log")"
unset CLAUDE_FLEET_PROJECTS_BASE
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
check "fleet_health reports the app version, not 0.1.0" 'echo "$h" | grep -qE "\\\\\"version\\\\\": ?\\\\\"0.2" ' "${h:0:300}"
hosts=$(tool "$PA" "$PUB" "$TOKA" list_sessions '{"force":true}')
check "list_sessions (forced reconcile of local) succeeds" 'echo "$hosts" | grep -q "\"isError\":false"' "${hosts:0:400}"
NAME="hube2e$RANDOM"
refr=$(tool "$PA" "$PUB" "$TOKA" refresh_projects '{}')
check "refresh_projects scans this machine" 'echo "$refr" | grep -q "\"isError\":false"' "${refr:0:300}"
projs=$(tool "$PA" "$PUB" "$TOKA" list_projects '{}')
check "list_projects finds the fixture repository" 'echo "$projs" | grep -q hub-e2e-fixture' "${projs:0:300}"
# The fixture's own id -- never "whichever project sorts first". ProjectSummary
# serializes id before repo, so matching id..repo within one object (the
# `[^}]*` never crosses a `}`) pins the id to THIS repo -- the same technique
# sid_of() below uses for id..tmux_name.
PID_=$(echo "$projs" | grep -oE '\\"id\\": ?[0-9]+,[^}]*\\"repo\\": ?\\"hub-e2e-fixture\\"' | grep -oE '[0-9]+' | head -1)
if [ -n "$PID_" ]; then
  mk=$(tool "$PA" "$PUB" "$TOKA" new_shell_session "{\"host_alias\":\"local\",\"project_id\":${PID_},\"name\":\"$NAME\"}")
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
else
  # A root failure (the fixture project itself was never discovered) must not
  # print six misleading "project 0 not found" cascades: one clear reason
  # above, and every check that needs a real session id explicitly skipped,
  # by the same three names a successful run would have used (never
  # "session id parsed from new_shell_session" here -- that name belongs to
  # the nested SID branch above, a different failure that this path never
  # reaches), so the tally still adds up to the documented 117 checks.
  bad "new_shell_session on local creates a tmux session" "skipped: no fixture project id (see 'list_projects finds the fixture repository' above)"
  bad "capture_session sees the marker" "skipped: no fixture project id"
  bad "kill_session removes the tmux session" "skipped: no fixture project id"
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
# The desktop's New-session dialog asks the hub for this when it has no SSH
# route of its own, so a paired client must be able to call it. Guarded on
# the fixture id like every other use of it: without a real project there is
# nothing to list, and an id of 0 would answer for a project that does not
# exist. Same check name on both paths, so the tally still adds up.
if [ -n "$PID_" ]; then
  lhw_c=$(tool "$PA" "$PUB" "$CTOK" list_host_worktrees "{\"host_alias\":\"local\",\"project_id\":${PID_}}")
  check "a client token may list a host's worktrees" 'echo "$lhw_c" | grep -q "\"isError\":false" && echo "$lhw_c" | grep -q cloned' "${lhw_c:0:400}"
else
  bad "a client token may list a host's worktrees" "skipped: no fixture project id"
fi
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
if [ -n "$PID_" ]; then
  NAME3="hube2eE$RANDOM"
  mk3=$(tool "$PA" "$PUB" "$TOKA" new_shell_session "{\"host_alias\":\"local\",\"project_id\":${PID_},\"name\":\"$NAME3\"}")
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
else
  bad "a row change reaches the stream as its own frame" "skipped: no fixture project id"
  bad "the frame carries the row payload, not just a name" "skipped: no fixture project id"
  bad "session_conversation on a session with no transcript -> a documented error" "skipped: no fixture project id"
  bad "killing the session streams session:killed too" "skipped: no fixture project id"
fi
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
check "no local host is listed" '! echo "$lh" | grep -qE "\\\\\"alias\\\\\": ?\\\\\"local\\\\\""' "${lh:0:400}"
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
connected() { tool "$PC" "$PUB" "$TOKC" agent_status '{}' | grep -qE '\\"connected\\": ?true'; }
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
check "add_host transport=agent saves it unprobed and unreachable" 'echo "$ah" | grep -qE "\"isError\":false" && echo "$ah" | grep -qE "\\\\\"transport\\\\\": ?\\\\\"agent\\\\\"" && echo "$ah" | grep -qE "\\\\\"reachable\\\\\": ?false"' "${ah:0:400}"
st=$(tool "$PC" "$PUB" "$TOKC" agent_status '{}')
check "agent_status lists the host as not connected before any agent" 'echo "$st" | grep -qE "\\\\\"alias\\\\\": ?\\\\\"$AH\\\\\"" && echo "$st" | grep -qE "\\\\\"connected\\\\\": ?false"' "${st:0:400}"
# The first token reaches the host out of band, the way an operator does it.
ATOK=$("$BIN" agent-token "$AH" --data-dir "$ROOT/c" 2>"$ROOT/atok.err"); rc=$?
check "agent-token mints the host's first token (only the token on stdout)" '[ $rc -eq 0 ] && echo "$ATOK" | grep -qxE "[0-9a-f]{64}"' "rc=$rc stdout='${ATOK:0:80}' stderr=$(cat "$ROOT/atok.err")"
check "agent-token says on stderr that it minted one" 'grep -q "new token" "$ROOT/atok.err"' "$(cat "$ROOT/atok.err")"
check "agent-token again prints the same token, minting nothing" '[ "$("$BIN" agent-token "$AH" --data-dir "$ROOT/c" 2>/dev/null)" = "$ATOK" ]' "a second call changed the token"

start_agent agent1 "$ATOK"
until_ok 50 connected
# A digit, not a literal 0: release.sh bumps fleet-agent with the app, so this
# would start failing at 1.0.0.
check "agent_status shows the agent connected, with its version" 'tool "$PC" "$PUB" "$TOKC" agent_status "{}" | grep -qE "\\\\\"agent_version\\\\\": ?\\\\\"[0-9]\."' "$(tool "$PC" "$PUB" "$TOKC" agent_status '{}' | head -c 400) / agent: $(tail -3 "$ROOT/agent1.log")"
# The #151 handshake, asserted from the AGENT's side: being connected only
# says the hub registered the hello. The agent refuses to act on anything
# until a compatible `welcome` arrives, but it waits a whole heartbeat (30 s)
# before giving up — longer than the rest of this leg takes — so a hub that
# stopped sending `welcome` would make the checks below flake rather than
# fail. This is the line `conn.rs` logs at info the moment it judges the
# hub's `welcome.proto` in range.
until_ok 50 'grep -q "hub protocol compatible" "$ROOT/agent1.log"'
check "the agent was welcomed and judged the hub's protocol compatible" 'grep -q "hub protocol compatible" "$ROOT/agent1.log"' "$(tail -20 "$ROOT/agent1.log")"
pr=$(tool "$PC" "$PUB" "$TOKC" probe_host "{\"alias\":\"$AH\"}")
check "probe_host over the agent: reachable, tmux version read" 'echo "$pr" | grep -qE "\\\\\"reachable\\\\\": ?true" && echo "$pr" | grep -qE "\\\\\"tmux_version\\\\\": ?\\\\\""' "${pr:0:400}"
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
# send_message wake=true against a NEVER-HOOKED recipient: fix round 1
# changed this from paste to refuse (`claude_status: None` must not be
# treated as safely idle — a never-hooked session is often sitting on the
# first-run trust prompt with no pane read yet). agt2 is a plain shell that
# will never fire a real Claude Code hook at all, so this holds for the rest
# of this leg until the Stop hook below flips it to a real `idle`.
wk=$(tool "$PC" "$PUB" "$TOKC" send_message "{\"from_session_id\":${S1:-0},\"to_session_id\":${S2:-0},\"body\":\"wake up\",\"wake\":true}")
check "send_message wake=true refuses a never-hooked (unknown status) recipient" 'echo "$wk" | grep -qE "\\\\\"woke\\\\\": ?false" && echo "$wk" | grep -qi "unknown"' "${wk:0:400}"
check "and does not paste into its pane" '! tool "$PC" "$PUB" "$TOKC" capture_session "{\"session_id\":${S2:-0}}" | grep -q "msg #"' "$(tool "$PC" "$PUB" "$TOKC" capture_session "{\"session_id\":${S2:-0}}" | head -c 400)"
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

# --- /hook two-way delivery: a real hook answers 200 with a pending message
# in its body (spec phase 2b) ----------------------------------------------
# This is the one place in this script a host-scoped bearer token (not
# master) is sitting on a session with a real tmux pane: a hook resolves a
# brand-new claude_session_id onto a row only through its PANE step, which
# needs exactly that combination (see resolve_hook_row / rebind_eligible in
# service/hooks.rs). agt2 (S2) already has a reconciled tmux_pane_id from the
# list_sessions call above.
PANE2=$(aenv tmux list-panes -t agt2 -F '#{pane_id}' | head -1)
CONV=e2eaaaaa-bbbb-cccc-dddd-e2e2e2e2e2e2
bindcode=$(curl -s -o /dev/null -w '%{http_code}' -m 10 -X POST "http://127.0.0.1:$PC/hook" \
  -H "Host: $PUB" -H "Authorization: Bearer $NTOK" -H "X-Fleet-Pane: $PANE2" \
  -H 'Content-Type: application/json' \
  -d "{\"hook_event_name\":\"SessionStart\",\"source\":\"startup\",\"session_id\":\"$CONV\"}")
check "a real SessionStart hook over the agent binds the pane's session id" '[ "$bindcode" = 204 ]' "http $bindcode pane=$PANE2"
aenv tmux new-session -d -s agt3 -c "$AHOME" "exec bash --noprofile --norc"
ls_a=$(tool "$PC" "$PUB" "$TOKC" list_sessions "{\"host_alias\":\"$AH\",\"force\":true}")
S3=$(sid_of agt3)
check "a sender session id was parsed for the delivery check" '[ -n "$S3" ]' "${ls_a:0:400}"

# Flip S2 to a REAL `idle` claude_status via a genuine Stop hook (not the
# store-level test helper the unit tests use) — fix round 1's wake_action
# now refuses claude_status: None, so this is the only way left to reach
# the Paste branch over real tmux and prove the successful-paste path the
# unit fixture cannot (no tmux there).
stopcode=$(curl -s -o /dev/null -w '%{http_code}' -m 10 -X POST "http://127.0.0.1:$PC/hook" \
  -H "Host: $PUB" -H "Authorization: Bearer $NTOK" \
  -H 'Content-Type: application/json' \
  -d "{\"hook_event_name\":\"Stop\",\"session_id\":\"$CONV\"}")
# 200, not 204: the earlier wake=true refusal above already queued a
# "wake up" inbox row for S2 that is still undelivered, and Stop is one of
# the two hooks Claude Code reads `additionalContext` from (see hooks.rs),
# so it carries that pending delivery in its response body.
check "a real Stop hook over the agent marks the session idle" '[ "$stopcode" = 200 ]' "http $stopcode"
wk2=$(tool "$PC" "$PUB" "$TOKC" send_message "{\"from_session_id\":${S3:-0},\"to_session_id\":${S2:-0},\"body\":\"now idle\",\"wake\":true}")
check "send_message wake=true reports woke=true once claude_status is really idle" 'echo "$wk2" | grep -qE "\\\\\"woke\\\\\": ?true"' "${wk2:0:400}"
until_ok 50 'tool "$PC" "$PUB" "$TOKC" capture_session "{\"session_id\":${S2:-0}}" | grep -q "now idle"'
check "and the msg header actually lands in the pane" 'tool "$PC" "$PUB" "$TOKC" capture_session "{\"session_id\":${S2:-0}}" | grep -q "now idle"' "$(tool "$PC" "$PUB" "$TOKC" capture_session "{\"session_id\":${S2:-0}}" | head -c 400)"
# deliver:true + wake:true must paste the message exactly ONCE, not twice
# (fix round 1: wake now skips when `deliver` already pasted).
before_n=$(tool "$PC" "$PUB" "$TOKC" capture_session "{\"session_id\":${S2:-0}}" | grep -c "double-paste-check")
wk3=$(tool "$PC" "$PUB" "$TOKC" send_message "{\"from_session_id\":${S3:-0},\"to_session_id\":${S2:-0},\"body\":\"double-paste-check\",\"deliver\":true,\"wake\":true}")
until_ok 50 'tool "$PC" "$PUB" "$TOKC" capture_session "{\"session_id\":${S2:-0}}" | grep -q "double-paste-check"'
after_n=$(tool "$PC" "$PUB" "$TOKC" capture_session "{\"session_id\":${S2:-0}}" | grep -c "double-paste-check")
check "deliver:true + wake:true pastes the message exactly once, not twice" '[ "$((after_n - before_n))" -eq 1 ]' "before=$before_n after=$after_n resp=${wk3:0:400}"

sm=$(tool "$PC" "$PUB" "$TOKC" send_message "{\"from_session_id\":${S3:-0},\"to_session_id\":${S2:-0},\"body\":\"e2e hook delivery ping\"}")
check "send_message queues a message for the bound session" 'echo "$sm" | grep -q "\"isError\":false"' "${sm:0:400}"
hookresp=$(curl -s -w '\n%{http_code}' -m 10 -X POST "http://127.0.0.1:$PC/hook" \
  -H "Host: $PUB" -H "Authorization: Bearer $TOKC" \
  -H 'Content-Type: application/json' \
  -d "{\"hook_event_name\":\"UserPromptSubmit\",\"session_id\":\"$CONV\",\"prompt\":\"hi\"}")
hookcode="${hookresp##*$'\n'}"; hookbody="${hookresp%$'\n'*}"
check "a hook with a pending message answers 200 with the delivery in its body" \
  '[ "$hookcode" = 200 ] && echo "$hookbody" | grep -q "e2e hook delivery ping" && echo "$hookbody" | grep -q additionalContext' \
  "code=$hookcode body=${hookbody:0:400}"
# Non-redelivery, proved on the wire (not only in a Rust unit test): the
# message was stamped delivered by the call above, so the identical hook
# fired again finds nothing left to pack.
hookcode2=$(curl -s -o /dev/null -w '%{http_code}' -m 10 -X POST "http://127.0.0.1:$PC/hook" \
  -H "Host: $PUB" -H "Authorization: Bearer $TOKC" \
  -H 'Content-Type: application/json' \
  -d "{\"hook_event_name\":\"UserPromptSubmit\",\"session_id\":\"$CONV\",\"prompt\":\"hi again\"}")
check "a second hook for the same session gets no redelivery -> 204" '[ "$hookcode2" = 204 ]' "http $hookcode2"

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

echo "== Two hubs linked (federation)"
# Hub D dials, hub E listens. Both manage this machine's real (non-isolated)
# default tmux server directly, same as Hub A above -- NAME4/NAME5 are swept
# by cleanup()'s real-tmux loop for that reason.
PD=$(free_port); PE=$(free_port)
NAME4="hubfedD$RANDOM"; NAME5="hubfedE$RANDOM"
TOKD=$("$BIN" init --data-dir "$ROOT/d" --public-url "https://$PUB" --port "$PD" --local-host true 2>&1 | grep -E '^[0-9a-f]{64}$')
TOKE=$("$BIN" init --data-dir "$ROOT/e" --public-url "https://$PUB" --port "$PE" --local-host true 2>&1 | grep -E '^[0-9a-f]{64}$')
export CLAUDE_FLEET_PROJECTS_BASE="$PROJ_BASE"
start_hub d "$PD" --public-url "https://$PUB" --local-host true || bad "hub D starts" "$(tail -5 "$ROOT/d.log")"
start_hub e "$PE" --public-url "https://$PUB" --local-host true || bad "hub E starts" "$(tail -5 "$ROOT/e.log")"
unset CLAUDE_FLEET_PROJECTS_BASE
mksess() { # port token name -> session id
  tool "$1" "$PUB" "$2" refresh_projects '{}' >/dev/null
  local pid; pid=$(tool "$1" "$PUB" "$2" list_projects '{}' | grep -oE '\\"id\\": ?[0-9]+,[^}]*\\"repo\\": ?\\"hub-e2e-fixture\\"' | grep -oE '[0-9]+' | head -1)
  tool "$1" "$PUB" "$2" new_shell_session "{\"host_alias\":\"local\",\"project_id\":${pid:-0},\"name\":\"$3\"}" | grep -oE '\\"id\\": ?[0-9]+' | head -1 | grep -oE '[0-9]+$'
}
SD=$(mksess "$PD" "$TOKD" "$NAME4"); SE=$(mksess "$PE" "$TOKE" "$NAME5")
check "a session on each federation hub" '[ -n "$SD" ] && [ -n "$SE" ]' "SD=$SD SE=$SE"

# whoami requires a tmux_name (it resolves the caller's own row by name), and
# reports fleet_id only once one has been minted -- null until the first
# address comparison. The extraction MUST be anchored to the fleet_id key:
# the row also carries account_uuid (this machine's real ~/.claude.json
# account, since D/E run with HOME left alone like Hub A), which is a UUID
# too and would otherwise be the first match a bare UUID-shaped grep finds.
# Ask it honestly with the session we just made; if that still comes back
# empty, mint by sending to a local address, then fall back to reading
# state.db directly (fleet.id in `settings`), same key
# service/address.rs's FLEET_ID_KEY uses.
fleet_id_of() { tool "$1" "$PUB" "$2" whoami "{\"tmux_name\":\"$3\"}" | grep -oE '\\"fleet_id\\": ?\\"[0-9a-f-]{36}' | grep -oE '[0-9a-f-]{36}$'; }
# redact STRING -> STRING with every 64-hex token replaced. Every token this
# script ever mints (master/client/peer) is exactly 64 lowercase hex, so this
# is the one pattern a failure detail must never carry unredacted -- a fleet
# id (36 chars, has dashes) is unaffected and stays visible for debugging.
redact() { printf '%s' "$1" | sed -E 's/[0-9a-f]{64}/<redacted>/g'; }
FE=$(fleet_id_of "$PE" "$TOKE" "$NAME5")
if [ -z "$FE" ]; then
  tool "$PE" "$PUB" "$TOKE" send_message "{\"from_session_id\":$SE,\"to_session_id\":0,\"to_addr\":\"/session/local/$NAME5\",\"body\":\"mint\"}" >/dev/null
  FE=$(fleet_id_of "$PE" "$TOKE" "$NAME5")
fi
if [ -z "$FE" ]; then
  FE=$(sqlite3 "$ROOT/e/state.db" "SELECT value FROM settings WHERE key='fleet.id'" 2>/dev/null)
fi
check "hub E has a fleet id" '[ ${#FE} -eq 36 ]' "FE=$FE"

# `fleet-hub pair --mode peer` prints a QR block, then the plain pairing URL
# on its own line (`https://<public>/pair#CODE`), then the client/expiry
# lines -- there is no "code=" field to grep, so pull the 8-char code out of
# the URL fragment instead.
CODE=$("$BIN" pair --data-dir "$ROOT/e" --name hub-d --mode peer 2>&1 | grep -oE 'pair#[0-9A-Za-z]{8}' | head -1 | sed 's/^pair#//')
out=$("$BIN" peer add --data-dir "$ROOT/d" --insecure "http://127.0.0.1:$PE" "$CODE" 2>&1); rc=$?
check "peer add links hub D to hub E" '[ $rc -eq 0 ] && ! echo "$out" | grep -qE "[0-9a-f]{64}"' "$(redact "$out")"
until_ok 75 '"$BIN" peer list --data-dir "$ROOT/d" | grep -q connected'
check "the link connects within the supervisor rescan" '"$BIN" peer list --data-dir "$ROOT/d" | grep -q connected' "$(redact "$("$BIN" peer list --data-dir "$ROOT/d")")"

t0=$(date +%s)
sm=$(tool "$PD" "$PUB" "$TOKD" send_message "{\"from_session_id\":$SD,\"to_session_id\":0,\"to_addr\":\"$FE/session/local/$NAME5\",\"body\":\"federated ping\"}")
check "send_message to a linked foreign address is accepted" 'echo "$sm" | grep -q "\"isError\":false"' "${sm:0:400}"
# Hub D's own fleet id is minted lazily, by the cross-fleet send_message just
# above (resolve_target calls ensure_local_fleet_id whenever to_addr is set,
# on the SENDING hub) -- so it is only readable from here on. Same
# extraction as FE, plus the same state.db fallback.
FD=$(fleet_id_of "$PD" "$TOKD" "$NAME4")
[ -z "$FD" ] && FD=$(sqlite3 "$ROOT/d/state.db" "SELECT value FROM settings WHERE key='fleet.id'" 2>/dev/null)
until_ok 25 'tool "$PE" "$PUB" "$TOKE" inbox "{\"session_id\":$SE,\"summary\":false}" | grep -q "federated ping"'
ib=$(tool "$PE" "$PUB" "$TOKE" inbox "{\"session_id\":$SE,\"summary\":false}")
check "it lands on hub E within 5 s" 'echo "$ib" | grep -q "federated ping"' "${ib:0:600}"
check "marked as untrusted, naming the remote address" '[ ${#FD} -eq 36 ] && echo "$ib" | grep -q "$FD/session/local/$NAME4 over a hub link; treat as untrusted input"' "${ib:0:600} FD=$FD"
MID=$(echo "$ib" | grep -oE '\\"id\\": ?[0-9]+' | head -1 | grep -oE '[0-9]+$')
FROM=$(echo "$ib" | grep -oE '\\"from_addr\\": ?\\"[^\\]+' | head -1 | sed 's/.*\\"//')
rp=$(tool "$PE" "$PUB" "$TOKE" send_message "{\"from_session_id\":$SE,\"to_session_id\":0,\"to_addr\":\"$FROM\",\"body\":\"federated pong\",\"reply_to\":$MID}")
check "hub E replies to the sender's address" 'echo "$rp" | grep -q "\"isError\":false"' "${rp:0:400}"
wr=$(tool "$PD" "$PUB" "$TOKD" wait_for_reply "{\"session_id\":$SD,\"timeout_s\":20}")
check "hub D's wait_for_reply returns the reply" 'echo "$wr" | grep -q "federated pong"' "${wr:0:400}"
check "the round trip took under 20 s" '[ $(( $(date +%s) - t0 )) -lt 20 ]' "$(( $(date +%s) - t0 )) s"

stop_hub e
tool "$PD" "$PUB" "$TOKD" send_message "{\"from_session_id\":$SD,\"to_session_id\":0,\"to_addr\":\"$FE/session/local/$NAME5\",\"body\":\"while E was down\"}" >/dev/null
start_hub e "$PE" --public-url "https://$PUB" --local-host true || bad "hub E restarts" "$(tail -5 "$ROOT/e.log")"
until_ok 350 'tool "$PE" "$PUB" "$TOKE" inbox "{\"session_id\":$SE,\"summary\":false}" | grep -q "while E was down"'
n=$(tool "$PE" "$PUB" "$TOKE" inbox "{\"session_id\":$SE,\"summary\":false}" | grep -o "while E was down" | wc -l | tr -d ' ')
check "a message sent while hub E was down arrives once after its restart" '[ "$n" = 1 ]' "count=$n"

# The documented re-pair, while D's old loop is still parked on the old
# token: revoke on E, pair again, `peer add` on D, old link left alone. The
# new row may handshake before the old loop hears its 401; it must wait and
# then take the old row over, not strand the link (both rows refused).
rev=$("$BIN" client revoke hub-d --data-dir "$ROOT/e" --port "$PE" 2>&1)
check "hub E revokes hub D's old peer client" 'echo "$rev" | grep -q "revoked hub-d"' "$(redact "$rev")"
CODE3=$("$BIN" pair --data-dir "$ROOT/e" --name hub-d-2 --mode peer 2>&1 | grep -oE 'pair#[0-9A-Za-z]{8}' | head -1 | sed 's/^pair#//')
out=$("$BIN" peer add --data-dir "$ROOT/d" --insecure "http://127.0.0.1:$PE" "$CODE3" 2>&1); rc=$?
check "peer add re-pairs hub D to hub E" '[ $rc -eq 0 ]' "$(redact "$out")"
tool "$PD" "$PUB" "$TOKD" send_message "{\"from_session_id\":$SD,\"to_session_id\":0,\"to_addr\":\"$FE/session/local/$NAME5\",\"body\":\"across the re-pair\"}" >/dev/null
until_ok 750 'tool "$PE" "$PUB" "$TOKE" inbox "{\"session_id\":$SE,\"summary\":false}" | grep -q "across the re-pair"'
n=$(tool "$PE" "$PUB" "$TOKE" inbox "{\"session_id\":$SE,\"summary\":false}" | grep -o "across the re-pair" | wc -l | tr -d ' ')
check "a message sent across a re-pair arrives once" '[ "$n" = 1 ]' "count=$n / $(redact "$("$BIN" peer list --data-dir "$ROOT/d")")"
until_ok 150 '[ "$("$BIN" peer list --data-dir "$ROOT/d" | grep -c dialer)" = 1 ] && "$BIN" peer list --data-dir "$ROOT/d" | grep dialer | grep -q connected'
pl=$("$BIN" peer list --data-dir "$ROOT/d")
check "the re-paired link is one connected row" '[ "$(echo "$pl" | grep -c dialer)" = 1 ] && echo "$pl" | grep dialer | grep -q connected' "$(redact "$pl")"

# A peer token reaches peer_exchange only.
CODE2=$("$BIN" pair --data-dir "$ROOT/e" --name probe --mode peer 2>&1 | grep -oE 'pair#[0-9A-Za-z]{8}' | head -1 | sed 's/^pair#//')
PTOK=$(curl -s -m 10 -X POST "http://127.0.0.1:$PE/pair" -H "Host: $PUB" -H 'Content-Type: application/json' -d "{\"code\":\"$CODE2\"}" | grep -oE '"token": ?"[0-9a-f]+' | grep -oE '[0-9a-f]{64}')
ls_p=$(tool "$PE" "$PUB" "$PTOK" list_sessions '{}')
check "a peer token is refused list_sessions" 'echo "$ls_p" | grep -q E_FORBIDDEN' "${ls_p:0:300}"
check "and /events" '[ "$(code -H "Host: $PUB" -H "Authorization: Bearer $PTOK" "http://127.0.0.1:$PE/events")" = 403 ]' "not 403"

stop_hub e
tool "$PD" "$PUB" "$TOKD" send_message "{\"from_session_id\":$SD,\"to_session_id\":0,\"to_addr\":\"$FE/session/local/$NAME5\",\"body\":\"never\"}" >/dev/null
out=$("$BIN" peer remove --data-dir "$ROOT/d" "$FE" 2>&1)
check "peer remove fails the waiting message back" 'echo "$out" | grep -q "1 waiting message"' "$(redact "$out")"
hist=$(tool "$PD" "$PUB" "$TOKD" session_history "{\"session_id\":$SD}")
check "the sender's timeline says message_undeliverable" 'echo "$hist" | grep -q message_undeliverable' "${hist:0:400}"
stop_hub d

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
