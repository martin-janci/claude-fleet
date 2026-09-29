#!/usr/bin/env bash
# Tests for tools/ag (the agent launcher) against fake `claude` / `codex`
# binaries on PATH. Needs only bash + coreutils and touches nothing outside
# its temp ROOT. PASS/FAIL lines, exit 1 on any failure (the style of
# hub-deploy-scripts-test.sh). ag runs with PATH=$FAKE:/usr/bin:/bin, so on
# macOS its `#!/usr/bin/env bash` resolves to the stock /bin/bash 3.2.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
AG="$REPO/tools/ag/ag"
TMP_BASE="/tmp/claude-$(id -u)"
[ -d "$TMP_BASE" ] || TMP_BASE=/tmp
ROOT="$(mktemp -d "$TMP_BASE/ag-test.XXXXXX")" || { echo "ag-test: mktemp failed" >&2; exit 1; }
echo "ag-test: log root: $ROOT"

PASS=0; FAIL=0
pass() { PASS=$((PASS + 1)); echo "PASS: $*"; }
fail() { FAIL=$((FAIL + 1)); echo "FAIL: $*" >&2; }

FAKE="$ROOT/fakebin"; mkdir -p "$FAKE"
# A fake harness prints BIN=<name>, then ARG=<arg> for each argument.
mkfake() {
  # shellcheck disable=SC2016  # "$@" and $a are literal text of the fake script
  printf '#!/bin/sh\necho "BIN=%s"\nfor a in "$@"; do echo "ARG=$a"; done\n' "$1" >"$FAKE/$1"
  chmod 755 "$FAKE/$1"
}
mkfake claude; mkfake codex

export AG_CONFIG="$ROOT/config" AG_BIN_DIR="$ROOT/bin" AG_CODEX_FALLBACKS=""
unset AG_HARNESS AG_ROOT AG_HOME
cfg() { printf '%s\n' "$@" >"$AG_CONFIG"; }
# run ARGS… — ag with only the fakes and system dirs on PATH; stderr → $ROOT/err.
run() { PATH="$FAKE:/usr/bin:/bin" "$AG" "$@" 2>"$ROOT/err"; }
# expect NAME WANT ARGS… — ag's stdout, newlines folded to spaces, must equal WANT.
expect() {
  local name=$1 want=$2 got; shift 2
  got=$(run "$@" | tr '\n' ' ' | sed 's/ $//')
  if [ "$got" = "$want" ]; then pass "$name"; else fail "$name: got [$got] want [$want] stderr [$(cat "$ROOT/err")]"; fi
}
# expect_rc NAME CODE ARGS… — ag's exit status must equal CODE.
expect_rc() {
  local name=$1 want=$2 rc; shift 2
  run "$@" >/dev/null; rc=$?
  if [ "$rc" = "$want" ]; then pass "$name"; else fail "$name: exit $rc, want $want (stderr: $(cat "$ROOT/err"))"; fi
}

# --- basics -----------------------------------------------------------------
cfg 'default = claude'
expect "version" "ag 0.1.0" version
expect_rc "help exits 0" 0 help
expect "list shows every harness and its path" "claude $FAKE/claude codex $FAKE/codex" list
expect "which claude" "$FAKE/claude" which claude
rm "$FAKE/codex"
expect "list marks a missing harness with -" "claude $FAKE/claude codex -" list
expect_rc "which: missing harness exits 4" 4 which codex
mkdir -p "$ROOT/apps"; printf '#!/bin/sh\necho "BIN=codex"\n' >"$ROOT/apps/codex"; chmod 755 "$ROOT/apps/codex"
AG_CODEX_FALLBACKS="$ROOT/apps/codex" expect "which codex falls back to an app-bundled binary" "$ROOT/apps/codex" which codex
mkfake codex
expect_rc "which: unknown harness exits 2" 2 which nope
expect_rc "which: no argument exits 2" 2 which

# --- launch: harness resolution and config ------------------------------------
cfg '# a comment' '  default =  claude  ' '[alias]' 'default = codex'
expect "config: top-level key, trimmed; section keys do not leak" "BIN=claude"
cfg 'default = codex'
expect "positional harness beats config default" "BIN=claude" claude
export AG_HARNESS=claude
expect "AG_HARNESS beats config default" "BIN=claude"
export AG_HARNESS=nope
expect_rc "AG_HARNESS naming an unknown harness exits 2" 2
unset AG_HARNESS
cfg 'order = codex claude'
rm "$FAKE/codex"
expect "no default: first installed harness in config order" "BIN=claude"
cfg 'default = claude'
rm "$FAKE/claude"
expect_rc "default harness not installed exits 4" 4
if grep -q 'claude.ai/install.sh' "$ROOT/err"; then pass "not installed: prints the install command"; else fail "not installed: no install hint in [$(cat "$ROOT/err")]"; fi
rm -f "$AG_CONFIG"
expect_rc "nothing installed, no config exits 4" 4
mkfake claude; mkfake codex

# --- launch: claude argv -----------------------------------------------------------
cfg 'default = claude'
expect "claude: bare" "BIN=claude" claude
expect "claude: --yolo" "BIN=claude ARG=--dangerously-skip-permissions" claude --yolo
cfg 'default = claude' 'yolo = true'
expect "claude: yolo = true in config" "BIN=claude ARG=--dangerously-skip-permissions" claude
expect "claude: --no-yolo beats config" "BIN=claude" claude --no-yolo
cfg 'default = claude'
expect "claude: canonical flag order" \
  "BIN=claude ARG=--dangerously-skip-permissions ARG=--model ARG=opus ARG=--effort ARG=high ARG=--name ARG=N ARG=--resume ARG=X" \
  claude --resume X --name N --yolo -m opus --effort high
expect "claude: --continue" "BIN=claude ARG=--continue" claude -c
expect "claude: --new-id maps to --session-id" "BIN=claude ARG=--session-id ARG=ID1" claude --new-id ID1
expect "claude: -p keeps the prompt as one argument, last" "BIN=claude ARG=--model ARG=m ARG=-p ARG=hi there" claude -p "hi there" -m m
expect "claude: unknown args pass through" "BIN=claude ARG=--verbose ARG=--session-id ARG=S" claude --verbose --session-id S
expect "claude: everything after -- passes through" "BIN=claude ARG=-c ARG=x" claude -- -c x
expect_rc "a flag missing its value exits 2" 2 claude --resume
expect_rc "--continue with --resume exits 2" 2 claude -c -r X
expect_rc "--new-id with --resume exits 2" 2 claude --new-id A -r X

# --- launch: codex argv ------------------------------------------------------------
cfg 'default = codex'
expect "codex: bare" "BIN=codex"
expect "codex: --yolo" "BIN=codex ARG=--dangerously-bypass-approvals-and-sandbox" --yolo
expect "codex: --continue is resume --last" "BIN=codex ARG=resume ARG=--last" -c
expect "codex: --resume ID" "BIN=codex ARG=resume ARG=S1" -r S1
expect "codex: -p is exec, prompt last" "BIN=codex ARG=exec ARG=hi there" -p "hi there"
expect "codex: -p with --resume is exec resume" "BIN=codex ARG=exec ARG=resume ARG=S1 ARG=hi" -p hi -r S1
expect "codex: -p with --continue is exec resume --last" "BIN=codex ARG=exec ARG=resume ARG=--last ARG=hi" -p hi -c
expect "codex: model and effort" 'BIN=codex ARG=-m ARG=gpt-5.5 ARG=-c ARG=model_reasoning_effort="high"' -m gpt-5.5 --effort high
expect "codex: canonical order" \
  "BIN=codex ARG=resume ARG=S1 ARG=--dangerously-bypass-approvals-and-sandbox ARG=-m ARG=M ARG=--search" \
  --search -r S1 --yolo -m M
expect "codex: --name is ignored" "BIN=codex" --name N
expect_rc "codex: --new-id is unsupported (exit 3)" 3 --new-id ID1

# --- summary ----------------------------------------------------------------
echo "ag-test: $PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]
