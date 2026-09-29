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

# --- summary ----------------------------------------------------------------
echo "ag-test: $PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]
