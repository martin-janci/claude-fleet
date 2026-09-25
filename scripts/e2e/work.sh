# shellcheck shell=bash
# The work graph, end to end (work graph M10.2). Sourced by scripts/hub-e2e.sh,
# whose helpers it uses (tool, check, ok, bad, until_ok, start_hub, stop_hub,
# free_port, code) and whose $ROOT, $BIN, $PUB and $PROJ_BASE it reads.
#
# Hub W: --local-host true, on this machine's tmux like hub A (run after hub A
# and the federation hubs have stopped, so no two hubs manage `local` at once),
# with three test doubles and nothing else faked:
#   * a loopback fake Jira Cloud (scripts/e2e/fake_jira.py), reached through
#     FLEET_E2E_TRACKER_URL -- honoured only by a debug fleet-hub built with
#     `--features e2e`; any other build refuses to start with it set, and then
#     this whole leg is SKIPPED (loudly), not failed;
#   * a fake Claude REPL (scripts/e2e/fake_claude.py) as `cl` first on the
#     hub's PATH, which the pane command prefers to `claude`;
#   * the hooks Claude Code would post (/hook), posted here instead, so each
#     step is deterministic.
# The tidy step backdates two timestamps in the hub's own state.db (idle for
# hours, done for days) instead of waiting for them: the only place the leg
# reaches under the API.

WNAMES=()   # tmux sessions hub W created, swept by work_cleanup

work_cleanup() {
  local n
  for n in "${WNAMES[@]}"; do
    [ -n "$n" ] && tmux has-session -t "=$n" 2>/dev/null && tmux kill-session -t "=$n" 2>/dev/null
  done
  return 0
}

# jt EXPR < tool-output: the tool's JSON result as `t`, EXPR evaluated in
# Python; dicts/lists print as JSON, None as nothing. A non-JSON result (an
# error's text) prints nothing.
jt() {
  python3 -c '
import json, sys
try:
    r = json.loads(sys.stdin.read())
    t = json.loads(r["result"]["content"][0]["text"])
except Exception:
    sys.exit(0)
try:
    v = eval(sys.argv[1], {"t": t})
except Exception:
    sys.exit(0)
if isinstance(v, (dict, list)):
    print(json.dumps(v))
elif v is not None:
    print(v)' "$1"
}
# jobj k=v ... -> a JSON object of strings.
jobj() { python3 -c 'import json,sys; print(json.dumps(dict(a.split("=",1) for a in sys.argv[1:])))' "$@"; }
isok() { echo "$1" | grep -q '"isError":false'; }

wtool() { tool "$PW" "$PUB" "$TOKW" "$@"; }
# hook JSON -> the hub's answer body, then a last line `HTTP <status>`.
hook() {
  curl -s -m 30 -w '\nHTTP %{http_code}' -X POST "http://127.0.0.1:$PW/hook" -H "Host: $PUB" \
    -H "Authorization: Bearer $TOKW" -H 'Content-Type: application/json' -d "$1"
}
prompt_hook() { hook "$(jobj hook_event_name=UserPromptSubmit session_id="$1" prompt="$2")"; }
stop_hook()   { hook "$(jobj hook_event_name=Stop session_id="$1" last_assistant_message="${2:-}")"; }
# srow ID EXPR: EXPR over session ID's row (`t`), from list_sessions.
srow() {
  wtool list_sessions '{"summary":false}' | jt "next((s for s in (t if isinstance(t, list) else t.get(\"sessions\", [])) if s[\"id\"] == $1), {})$2"
}
history_has() { wtool session_history "{\"session_id\":$1}" | grep -q "$2"; }
fake() { curl -s -m 10 -X POST "http://127.0.0.1:$FJP/_fake/issue" -H 'Content-Type: application/json' -d "$1"; }
db() { python3 -c 'import sqlite3,sys; c=sqlite3.connect(sys.argv[1], timeout=10); c.execute(sys.argv[2]); c.commit()' "$ROOT/w/state.db" "$1"; }
fixture_repo() { # dir
  mkdir -p "$1"
  GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null git init -q -b main "$1"
  GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null git -C "$1" \
    -c user.name="hub-e2e" -c user.email="hub-e2e@example.invalid" -c commit.gpgsign=false \
    commit -q --allow-empty -m "hub-e2e work fixture"
  GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null git -C "$1" remote add origin "https://example.invalid/e2e/$(basename "$1").git"
}
start_w() {
  CLAUDE_FLEET_PROJECTS_BASE="$PROJ_BASE" FLEET_E2E_TRACKER_URL="http://127.0.0.1:$FJP" \
    PATH="$ROOT/fakebin:$PATH" start_hub w "$PW" --public-url "https://$PUB" --local-host true
}
# The sync tick's first pass runs as the hub starts: a restart is "sync now".
restart_w() { stop_hub w; start_w; }

work_leg() {
  echo "== Work graph (fake tracker, fake Claude)"
  # --- the doubles -----------------------------------------------------------
  python3 "$E2E_DIR/fake_jira.py" "$ROOT/fj.port" 2>"$ROOT/fakejira.log" &
  echo $! >"$ROOT/fakejira.pid"
  until_ok 50 '[ -s "$ROOT/fj.port" ]' || { bad "the fake tracker starts" "$(tail -5 "$ROOT/fakejira.log")"; return; }
  FJP=$(cat "$ROOT/fj.port")
  mkdir -p "$ROOT/fakebin"
  cp "$E2E_DIR/fake_claude.py" "$ROOT/fakebin/cl" && chmod +x "$ROOT/fakebin/cl"
  W1="$PROJ_BASE/e2e/work-one"; W2="$PROJ_BASE/e2e/work-two"
  fixture_repo "$W1"; fixture_repo "$W2"

  PW=$(free_port)
  TOKW=$("$BIN" init --data-dir "$ROOT/w" --public-url "https://$PUB" --port "$PW" --local-host true 2>&1 | grep -E '^[0-9a-f]{64}$')
  if ! start_w; then
    if grep -q "has no end-to-end fake tracker" "$ROOT/w.log"; then
      echo "SKIP  work graph leg: $BIN was built without --features e2e (cargo build -p fleet-hub --features e2e)"
      WORK_SKIPPED=1
      return
    fi
    bad "hub W starts with the fake tracker" "$(tail -5 "$ROOT/w.log")"
    return
  fi
  ok "hub W starts with the fake tracker"

  work_connect || return
  work_start
  work_detect
  work_multi_start
  work_handover
  work_tidy
  work_orgs
  stop_hub w
  work_cleanup
}

# 1. work_admin connect; sync; work { tickets }.
work_connect() {
  local a b t r
  a=$(wtool work_admin '{"action":"add","provider":"jira","site_url":"https://acme.atlassian.net","name":"Acme"}')
  TA=$(echo "$a" | jt 't["id"]')
  b=$(wtool work_admin '{"action":"add","provider":"jira","site_url":"https://beta.atlassian.net","name":"Beta"}')
  TB=$(echo "$b" | jt 't["id"]')
  check "work_admin add connects two Jira Cloud trackers" '[ -n "$TA" ] && [ -n "$TB" ]' "${a:0:300} / ${b:0:300}"
  [ -n "$TA" ] && [ -n "$TB" ] || return 1
  for t in "$TA" "$TB"; do
    wtool work_admin "{\"action\":\"set_credential\",\"tracker_id\":$t,\"auth_kind\":\"basic\",\"username\":\"e2e@example.invalid\",\"secret\":\"e2e-token-not-real\"}" >/dev/null
    r=$(wtool work_admin "{\"action\":\"test\",\"tracker_id\":$t}")
    check "work_admin test probes tracker $t through the fake" '[ "$(echo "$r" | jt "t[\"ok\"]")" = True ]' "${r:0:400}"
  done
  r=$(curl -s -m 10 "http://127.0.0.1:$FJP/_fake/requests")
  check "the fake saw both sites, authenticated" 'echo "$r" | grep -q "acme.atlassian.net" && echo "$r" | grep -q "beta.atlassian.net"' "${r:0:300}"
  restart_w || { bad "hub W restarts for its first sync" "$(tail -5 "$ROOT/w.log")"; return 1; }
  until_ok 100 'wtool work "{\"action\":\"tickets\"}" | grep -q "ABC-1"'
  r=$(wtool work '{"action":"tickets"}')
  check "work { tickets } lists the synced tickets of both trackers" 'echo "$r" | grep -q "ABC-1" && echo "$r" | grep -q "XYZ-1"' "${r:0:400}"
  check "…mine and not done (not someone else's)" '! echo "$r" | grep -q "ABC-5"' "${r:0:400}"
}

# 2. work_link start on a real tmux host: the branch, the brief, the link.
work_start() {
  local p out prompt
  wtool refresh_projects '{}' >/dev/null
  p=$(wtool list_projects '{}')
  P1=$(echo "$p" | jt 'next(x["id"] for x in (t if isinstance(t, list) else t["projects"]) if x["repo"] == "work-one")')
  P2=$(echo "$p" | jt 'next(x["id"] for x in (t if isinstance(t, list) else t["projects"]) if x["repo"] == "work-two")')
  check "list_projects finds both work fixtures" '[ -n "$P1" ] && [ -n "$P2" ]' "${p:0:400}"
  [ -n "$P1" ] || return
  out=$(wtool work_link "{\"action\":\"start\",\"key\":\"ABC-1\",\"project_id\":$P1,\"host_alias\":\"local\",\"with_brief\":true}")
  S1=$(echo "$out" | jt 't["id"]'); C1=$(echo "$out" | jt 't["claude_session_id"]'); N1=$(echo "$out" | jt 't["tmux_name"]')
  WNAMES+=("$N1")
  check "work_link start opens a tmux session on local" '[ -n "$N1" ] && tmux has-session -t "=$N1" 2>/dev/null' "${out:0:400}"
  check "…on a worktree branch named {key}-{slug}" 'git -C "$W1" branch --list "abc-1-fix-the-login-redirect" | grep -q abc-1' "$(git -C "$W1" branch --list 2>&1)"
  check "…with the ticket confirmed as its work" '[ "$(srow "$S1" ".get(\"work\", {}).get(\"key\")")" = ABC-1 ] && [ "$(srow "$S1" ".get(\"work\", {}).get(\"state\")")" = confirmed ]' "$(srow "$S1" "")"
  # The fake shows the REPL cue, so fleet types the start prompt.
  until_ok 150 'tmux capture-pane -p -t "$N1" 2>/dev/null | grep -q "Start on ABC-1"'
  check "the start prompt is typed into the pane" 'tmux capture-pane -p -t "$N1" | grep -q "Start on ABC-1"' "$(tmux capture-pane -p -t "$N1" 2>&1 | tail -5)"
  prompt=$(prompt_hook "$C1" "Start on ABC-1: the ticket's context is in your fleet brief.")
  check "the prompt hook hands the brief to the session" 'echo "$prompt" | grep -q "Fix the login redirect"' "${prompt:0:400}"
  until_ok 100 'history_has "$S1" handover_started'
  check "…and the start is recorded as acknowledged" 'history_has "$S1" handover_started' "$(wtool session_history "{\"session_id\":$S1}" | head -c 400)"
}

# 3. Detection: a prompt naming a key yields a suggestion; reject it (final);
# a second key's suggestion, confirmed, becomes the session's work.
work_detect() {
  local out l
  [ -n "${P2:-}" ] || { bad "detection" "skipped: no fixture project"; return; }
  out=$(wtool new_session "{\"host_alias\":\"local\",\"project_id\":$P2,\"name\":\"e2ew-detect-$RANDOM\"}")
  S3=$(echo "$out" | jt 't["id"]'); C3=$(echo "$out" | jt 't["claude_session_id"]')
  WNAMES+=("$(echo "$out" | jt 't["tmux_name"]')")
  check "new_session starts a plain session to detect on" '[ -n "$S3" ] && [ -n "$C3" ]' "${out:0:400}"
  prompt_hook "$C3" "Please pick up XYZ-2 today" >/dev/null
  until_ok 50 '[ "$(srow "$S3" ".get(\"work_suggested\") or {}")" != "{}" ]'
  check "a prompt naming XYZ-2 yields work_suggested" '[ "$(srow "$S3" "[\"work_suggested\"][\"key\"]")" = XYZ-2 ]' "$(srow "$S3" "")"
  l=$(srow "$S3" '["work_suggested"]["link_id"]')
  out=$(wtool work_link "{\"action\":\"reject\",\"session_id\":$S3,\"link_id\":${l:-0}}")
  check "rejecting it clears the suggestion and records the rejection" '[ "$(srow "$S3" ".get(\"work_suggested\") or {}")" = "{}" ] && srow "$S3" "[\"work_rejected\"]" | grep -q XYZ-2' "${out:0:300} / $(srow "$S3" "")"
  prompt_hook "$C3" "Actually ABC-2 first, then maybe XYZ-2" >/dev/null
  until_ok 50 '[ "$(srow "$S3" ".get(\"work_suggested\", {}).get(\"key\")")" = ABC-2 ]'
  check "a second key is suggested; the rejected one is not proposed again" '[ "$(srow "$S3" "[\"work_suggested\"][\"key\"]")" = ABC-2 ] && [ "$(srow "$S3" "[\"work_suggested\"][\"suggestions\"]")" = 1 ]' "$(srow "$S3" "")"
  l=$(srow "$S3" '["work_suggested"]["link_id"]')
  out=$(wtool work_link "{\"action\":\"confirm\",\"session_id\":$S3,\"link_id\":${l:-0}}")
  check "confirming it makes ABC-2 the session's work" '[ "$(srow "$S3" "[\"work\"][\"key\"]")" = ABC-2 ]' "${out:0:300} / $(srow "$S3" "")"
}

# 4. Multi-start: two projects, one failure itemised; the duplicate guard.
work_multi_start() {
  local out again
  [ -n "${P2:-}" ] || { bad "multi-start" "skipped: no fixture project"; return; }
  out=$(wtool work_link "{\"action\":\"start\",\"key\":\"ABC-3\",\"project_ids\":[$P1,$P2,987654],\"host_alias\":\"local\"}")
  MS=$(echo "$out" | jt '[s["id"] for s in t["started"]]')
  for n in $(echo "$out" | jt '" ".join(s["tmux_name"] for s in t["started"])'); do WNAMES+=("$n"); done
  check "multi-start starts ABC-3 in both projects" '[ "$(echo "$out" | jt "len(t[\"started\"])")" = 2 ]' "${out:0:600}"
  check "…and itemises the project that does not exist" '[ "$(echo "$out" | jt "len(t[\"failed\"])")" = 1 ] && echo "$out" | grep -q 987654' "${out:0:600}"
  again=$(wtool work_link "{\"action\":\"start\",\"key\":\"ABC-3\",\"project_ids\":[$P1],\"host_alias\":\"local\"}")
  check "starting ABC-3 in the same project again is refused by the duplicate guard" '[ "$(echo "$again" | jt "len(t[\"started\"])")" = 0 ]' "${again:0:600}"
}

# 5. An agent-written handover: requested, answered through the Stop hook,
# fenced in the next brief.
work_handover() {
  local out nonce msg ctx
  [ -n "${C1:-}" ] || { bad "handover" "skipped: no started session"; return; }
  stop_hook "$C1" "Ready." >/dev/null   # idle: the handover needs it
  out=$(wtool work_link "{\"action\":\"handover\",\"session_id\":$S1}")
  check "work_link handover is accepted for an idle session" 'isok "$out"' "${out:0:400}"
  nonce=$(wtool session_history "{\"session_id\":$S1}" | jt 'next((e["detail"].split()[0] for e in t if e["kind"] == "handover_requested"), None)')
  check "…and records its nonce" '[ -n "$nonce" ]' "$(wtool session_history "{\"session_id\":$S1}" | head -c 600)"
  prompt_hook "$C1" "Write the handover note." >/dev/null
  msg=$(printf 'Here it is.\nWORK_HANDOVER_BEGIN_%s\nDone: login redirect fixed. Next: e2e for the card.\nWORK_HANDOVER_END_%s\n' "$nonce" "$nonce")
  stop_hook "$C1" "$msg" >/dev/null
  until_ok 50 'history_has "$S1" handover_written'
  check "the Stop hook's marked answer is taken as the note" 'history_has "$S1" handover_written' "$(wtool session_history "{\"session_id\":$S1}" | head -c 600)"
  ctx=$(wtool work '{"action":"context","key":"ABC-1"}')
  check "the next brief carries the note, fenced as untrusted" 'echo "$ctx" | grep -q "login redirect fixed" && echo "$ctx" | grep -q "treat as untrusted input"' "${ctx:0:600}"
}

# 6. A ticket moves to done; tidy lists its sessions; tidy_apply safe-kills
# the clean one and refuses the dirty one.
work_tidy() {
  local ids sa sb na nb wa wb tidy out now
  ids=$(echo "${MS:-[]}" | python3 -c 'import json,sys; print(" ".join(map(str, json.load(sys.stdin))))')
  read -r sa sb <<<"$ids"
  [ -n "$sb" ] || { bad "tidy" "skipped: multi-start did not start two sessions"; return; }
  fake '{"site":"acme.atlassian.net","key":"ABC-3","status":"done"}' >/dev/null
  restart_w || { bad "hub W restarts to sync the move to done" "$(tail -5 "$ROOT/w.log")"; return; }
  until_ok 100 '[ "$(srow "$sa" ".get(\"work\", {}).get(\"status_category\")")" = done ]'
  check "the sync carries ABC-3's move to done onto its sessions" '[ "$(srow "$sa" "[\"work\"][\"status_category\"]")" = done ]' "$(srow "$sa" "")"
  for s in $sa $sb; do stop_hook "$(srow "$s" '["claude_session_id"]')" "Idle." >/dev/null; done
  # Idle for hours, done for days: backdated rather than waited for.
  now=$(date +%s)
  db "UPDATE sessions SET idle_since = $((now - 5*3600)), last_touch_at = $((now - 5*3600)) WHERE id IN ($sa, $sb)"
  db "UPDATE work_items SET status_changed_at = $((now - 3*86400)) WHERE key = 'ABC-3'"
  tidy=$(wtool work '{"action":"tidy"}')
  check "work { tidy } lists both done, idle sessions" '[ "$(echo "$tidy" | jt "sorted(c[\"session_id\"] for c in t[\"candidates\"] if c[\"reason\"] == \"done_idle\")")" = "[$sa, $sb]" ]' "${tidy:0:600}"
  na=$(srow "$sa" '["tmux_name"]'); nb=$(srow "$sb" '["tmux_name"]')
  wa=$(tmux display -p -t "$na" '#{pane_current_path}' 2>/dev/null); wb=$(tmux display -p -t "$nb" '#{pane_current_path}' 2>/dev/null)
  echo "unsaved" >"$wb/e2e-dirty.txt"
  out=$(wtool work_link "{\"action\":\"tidy_apply\",\"items\":[{\"session_id\":$sa,\"action\":\"safe_kill\"},{\"session_id\":$sb,\"action\":\"safe_kill\"}]}")
  check "tidy_apply asks both sessions to safe-remove" '[ "$(echo "$out" | jt "[r[\"outcome\"] for r in t[\"results\"]]")" = "[\"safe_kill_requested\", \"safe_kill_requested\"]" ]' "${out:0:600}"
  # The fake answers SAFE_REMOVE_READY in the pane; the Stop hook makes the
  # hub read it.
  until_ok 100 '[ "$(tmux capture-pane -p -t "$na" -S -200 2>/dev/null | grep -c SAFE_REMOVE_READY)" -ge 2 ]'
  until_ok 100 '[ "$(tmux capture-pane -p -t "$nb" -S -200 2>/dev/null | grep -c SAFE_REMOVE_READY)" -ge 2 ]'
  for s in $sa $sb; do stop_hook "$(srow "$s" '["claude_session_id"]')" "Done." >/dev/null; done
  until_ok 100 '! tmux has-session -t "=$na" 2>/dev/null'
  check "the clean session is safe-killed and its worktree removed" '! tmux has-session -t "=$na" 2>/dev/null && [ ! -d "$wa" ]' "wt=$wa / $(wtool session_history "{\"session_id\":$sa}" | head -c 400)"
  until_ok 100 '[ "$(srow "$sb" "[\"safe_kill_state\"]")" = failed ]'
  check "the dirty one is refused: kept, worktree and its file intact" '[ "$(srow "$sb" "[\"safe_kill_state\"]")" = failed ] && tmux has-session -t "=$nb" 2>/dev/null && [ -f "$wb/e2e-dirty.txt" ]' "$(srow "$sb" "")"
}

# 7. The org boundary over the wire: a per-host token on host B sees none of
# org A's work, in tool answers or on /events.
work_orgs() {
  local oa ob hb tk ls sse ready
  oa=$(wtool work_admin '{"action":"add_org","name":"Acme"}' | jt 't["id"]')
  ob=$(wtool work_admin '{"action":"add_org","name":"Beta"}' | jt 't["id"]')
  check "work_admin add_org makes two orgs" '[ -n "$oa" ] && [ -n "$ob" ]' "oa=$oa ob=$ob"
  [ -n "$oa" ] && [ -n "$ob" ] || return
  wtool work_admin "{\"action\":\"assign_tracker\",\"tracker_id\":$TA,\"org_id\":$oa}" >/dev/null
  wtool work_admin "{\"action\":\"assign_tracker\",\"tracker_id\":$TB,\"org_id\":$ob}" >/dev/null
  wtool work_admin "{\"action\":\"assign_host\",\"host_alias\":\"local\",\"org_id\":$oa}" >/dev/null
  wtool add_host '{"alias":"e2ehostb","ssh_alias":"e2ehostb","transport":"agent"}' >/dev/null
  wtool work_admin "{\"action\":\"assign_host\",\"host_alias\":\"e2ehostb\",\"org_id\":$ob}" >/dev/null
  hb=$("$BIN" agent-token e2ehostb --data-dir "$ROOT/w" 2>/dev/null)
  check "a per-host token for host B" 'echo "$hb" | grep -qxE "[0-9a-f]{64}"' "${hb:0:80}"
  tk=$(tool "$PW" "$PUB" "$hb" work '{"action":"tickets"}')
  check "host B's tickets hold none of org A's" 'isok "$tk" && ! echo "$tk" | grep -q "ABC-"' "${tk:0:400}"
  ls=$(tool "$PW" "$PUB" "$hb" list_sessions '{"summary":false}')
  # Session rows stay visible across orgs (D7, off); their work does not.
  check "host B's session rows carry none of org A's work" 'isok "$ls" && [ "$(echo "$ls" | jt "sum(1 for r in t if r.get(\"work\") or r.get(\"work_suggested\") or r.get(\"work_rejected\"))")" = 0 ]' "${ls:0:400}"
  check "…while the master token's rows still carry it" '[ "$(wtool list_sessions "{\"summary\":false}" | jt "sum(1 for r in t if r.get(\"work\"))")" -ge 1 ]' ""
  sse="$ROOT/work-events-hostb.sse"
  curl -sN -m 20 "http://127.0.0.1:$PW/events?kinds=session,work" -H "Host: $PUB" -H "Authorization: Bearer $hb" >"$sse" 2>/dev/null &
  local ev=$!
  until_ok 50 'grep -q "^event: ready" "$sse"'
  # A change on an org-A session while host B listens.
  prompt_hook "$C3" "Still on ABC-2" >/dev/null
  until_ok 50 'grep -q "^event: session" "$sse"'
  kill "$ev" 2>/dev/null; wait "$ev" 2>/dev/null
  ready=$(grep -A1 "^event: ready" "$sse" | tail -1)
  check "host B's /events drops the work kind" '! echo "$ready" | grep -q "\"work\""' "$ready"
  check "…and its session frames carry none of org A's work" 'grep -q "^event: session" "$sse" && [ "$(sed -n "s/^data: //p" "$sse" | python3 -c "import json,sys; print(sum(1 for l in sys.stdin if (lambda d: isinstance(d, dict) and (d.get(\"work\") or d.get(\"work_suggested\") or d.get(\"work_rejected\")))(json.loads(l))))")" = 0 ]' "$(head -c 600 "$sse")"
}
