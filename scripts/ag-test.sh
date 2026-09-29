#!/usr/bin/env bash
# Tests for tools/ag (the agent launcher) against fake `claude` / `codex`
# binaries on PATH. Needs only bash + coreutils and touches nothing outside
# its temp ROOT. PASS/FAIL lines, exit 1 on any failure (the style of
# hub-deploy-scripts-test.sh). ag runs with PATH=$FAKE:/usr/bin:/bin, so on
# macOS its `#!/usr/bin/env bash` resolves to the stock /bin/bash 3.2.
set -uo pipefail

# shellcheck disable=SC1007  # intentional: clear CDPATH for this one `cd`
HERE="$(CDPATH= cd -- "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1007  # intentional: clear CDPATH for this one `cd`
REPO="$(CDPATH= cd -- "$HERE/.." && pwd)"
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
expect_rc "which: a value with spaces is never a harness (exact match)" 2 which "claude codex"

# --- ag_root: CDPATH safety and relative symlinks ----------------------------
# dirname of "tools/ag/ag" (bash's $0 when invoked as `bash tools/ag/ag`) is
# the bare relative name "tools/ag" — no leading "./" — which is exactly the
# shape CDPATH searches. Without `CDPATH= cd --`, `cd` echoes the matched
# directory to its own stdout, and that extra line gets captured into
# AG_ROOT, breaking every `. "$AG_ROOT"/lib/*.sh` source and any command that
# needs the sourced functions (list/which/shims/doctor/launch — not `version`,
# which is handled before any lib file is needed).
cdout=$(cd "$REPO" && CDPATH=".:$REPO" PATH="$FAKE:/usr/bin:/bin" bash tools/ag/ag list 2>"$ROOT/err" | tr '\n' ' ' | sed 's/ $//')
if [ "$cdout" = "claude $FAKE/claude codex $FAKE/codex" ]; then
  pass "ag_root: CDPATH does not corrupt AG_ROOT resolution"
else
  fail "ag_root CDPATH: got [$cdout] stderr [$(cat "$ROOT/err")]"
fi
RELDIR="$REPO/.ag-relsym-tmp"
rm -rf "$RELDIR"
mkdir -p "$RELDIR"
ln -s "../tools/ag/ag" "$RELDIR/ag"
relout=$(cd "$RELDIR" && PATH="$FAKE:/usr/bin:/bin" bash ./ag list 2>"$ROOT/err" | tr '\n' ' ' | sed 's/ $//')
if [ "$relout" = "claude $FAKE/claude codex $FAKE/codex" ]; then
  pass "ag_root: follows a relative symlink to the real ag"
else
  fail "ag_root relative symlink: got [$relout] stderr [$(cat "$ROOT/err")]"
fi
rm -rf "$RELDIR"

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

# --- config: inline comments -------------------------------------------------
cfg 'default = claude  # note'
expect "config: inline comment after a value is stripped" "BIN=claude"
cfg 'default = claude' 'yolo = true # on'
expect "config: inline comment on yolo (no space collapses value) is stripped" "BIN=claude ARG=--dangerously-skip-permissions" claude
cfg 'default = claude' 'yolo = true#nospace'
expect "config: a # with no preceding space stays part of the value" "BIN=claude" claude

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
expect_rc "--new-id with --resume exits 2" 2 claude --new-id A -r X
expect "claude: --flag=value form for print/model/effort/name/new-id" \
  "BIN=claude ARG=--model ARG=m ARG=--effort ARG=high ARG=--name ARG=N ARG=--session-id ARG=ID1 ARG=-p ARG=hi" \
  claude --model=m --effort=high --name=N --new-id=ID1 --print=hi
expect "claude: --resume=value form" "BIN=claude ARG=--resume ARG=S1" claude --resume=S1

# --- flag parsing: a value starting with "-" is rejected (except -r's picker) -
expect_rc "a flag missing its value exits 2 (--model has no picker exception)" 2 claude --model
expect_rc "-p with a value that looks like a flag exits 2" 2 claude -p --model
run claude -p --model >/dev/null
if grep -q -- "-p takes a value; to pass the CLI's own flags use --" "$ROOT/err"; then
  pass "-p leading-dash value: exact error message"
else
  fail "-p leading-dash value: message [$(cat "$ROOT/err")]"
fi
expect_rc "--model with a value that looks like a flag exits 2" 2 claude --model -m
expect_rc "--name with a value that looks like a flag exits 2" 2 claude --name --yolo
expect_rc "--new-id with a value that looks like a flag exits 2" 2 claude --new-id -r
expect_rc "--effort with a value that looks like a flag exits 2" 2 claude --effort -c
expect_rc "--effort: invalid characters (space form) exits 2" 2 claude --effort 'high level'
expect_rc "--effort=: invalid characters exits 2" 2 claude --effort=high!
expect "--effort: letters/digits/_/- are accepted" "BIN=claude ARG=--effort ARG=high_2-x" claude --effort high_2-x

# --- flag parsing: -r/--resume with no usable value opens the resume picker --
expect "claude: bare -r opens the resume picker (no id)" "BIN=claude ARG=--resume" claude -r
expect "claude: -r followed by another flag opens the picker" \
  "BIN=claude ARG=--dangerously-skip-permissions ARG=--resume" claude -r --yolo
expect_rc "claude: --continue with a bare -r (picker) exits 2" 2 claude -c -r
expect_rc "claude: --new-id with a bare -r (picker) exits 2" 2 claude --new-id A -r
expect_rc "--continue with --resume exits 2" 2 claude -c -r X

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
expect "codex: bare -r opens the resume picker (no id)" "BIN=codex ARG=resume" -r
expect "codex: -r followed by another flag opens the picker" "BIN=codex ARG=resume ARG=--dangerously-bypass-approvals-and-sandbox" -r --yolo
expect_rc "codex: -p with a bare -r (picker) is a usage error (exit 2)" 2 -p hi -r
run -p hi -r >/dev/null
if grep -q "needs a session id to resume non-interactively" "$ROOT/err"; then
  pass "codex: -p + resume-picker: explains why"
else
  fail "codex: -p + resume-picker message: [$(cat "$ROOT/err")]"
fi

# --- shims ---------------------------------------------------------------------------
BIN="$ROOT/bin"
shim_run() { PATH="$BIN:$FAKE:/usr/bin:/bin" sh -c "$1" 2>"$ROOT/err" | tr '\n' ' ' | sed 's/ $//'; }
cfg 'default = claude' '[alias]' 'cl = claude --yolo' 'cx = codex'
expect_rc "shims: generate" 0 shims
if [ -x "$BIN/cl" ] && [ -x "$BIN/cx" ]; then pass "shims: cl and cx are executable"; else fail "shims: missing cl/cx in $BIN"; fi
if grep -q '^# generated by ag-shims' "$BIN/cl"; then pass "shims: carry the marker"; else fail "shims: no marker in $BIN/cl"; fi
# fleet's pane command, verbatim in shape (tmux.rs pane_command_with), through the shim:
got=$(shim_run "cl --resume 'X' --name 'N' 2>/dev/null || cl --session-id 'X' --name 'N' || cl --name 'N'")
want="BIN=claude ARG=--dangerously-skip-permissions ARG=--name ARG=N ARG=--resume ARG=X"
if [ "$got" = "$want" ]; then pass "shims: fleet pane command runs claude"; else fail "shims: pane command got [$got] want [$want]"; fi
# Second branch of the same fleet pane chain: a `claude` that rejects
# --resume (e.g. an unknown/expired session id) exits 1, so the shell `||`
# falls through to the --session-id form — which ag passes straight through
# unchanged (ag has no "--session-id" flag of its own; it is Claude's own,
# reached here as an unknown/passthrough argument).
cp "$FAKE/claude" "$ROOT/claude.orig"
# shellcheck disable=SC2016  # "$@"/"$a" are literal text of the fake script
printf '#!/bin/sh\nfor a in "$@"; do [ "$a" = --resume ] && exit 1; done\necho "BIN=claude"\nfor a in "$@"; do echo "ARG=$a"; done\n' >"$FAKE/claude"
chmod 755 "$FAKE/claude"
got2=$(shim_run "cl --resume 'X' --name 'N' 2>/dev/null || cl --session-id 'X' --name 'N' || cl --name 'N'")
want2="BIN=claude ARG=--dangerously-skip-permissions ARG=--name ARG=N ARG=--session-id ARG=X"
if [ "$got2" = "$want2" ]; then pass "shims: fleet pane chain falls through to --session-id when --resume fails"; else fail "shims: pane chain 2nd branch got [$got2] want [$want2]"; fi
mv "$ROOT/claude.orig" "$FAKE/claude"; chmod 755 "$FAKE/claude"
got=$(shim_run "cx -p 'a b'")
if [ "$got" = "BIN=codex ARG=exec ARG=a b" ]; then pass "shims: arguments with spaces stay intact"; else fail "shims: cx got [$got]"; fi
printf 'mine\n' >"$BIN/mytool"
cfg '[alias]' 'cl = claude --yolo' 'mytool = claude'
expect_rc "shims: a foreign file blocks its alias (exit 5)" 5 shims
if [ "$(cat "$BIN/mytool")" = mine ]; then pass "shims: foreign file left untouched"; else fail "shims: foreign file was overwritten"; fi
if [ ! -e "$BIN/cx" ]; then pass "shims: stale ag shim removed"; else fail "shims: stale cx still there"; fi
cfg '[alias]' 'claude = codex'
expect_rc "shims: alias named like a harness is refused (exit 5)" 5 shims
if [ ! -e "$BIN/claude" ]; then pass "shims: no shim shadows a harness binary"; else fail "shims: wrote $BIN/claude"; fi
cfg '[alias]' 'Claude = codex'
expect_rc "shims: alias name differing only in case still shadows a harness (exit 5)" 5 shims
if [ ! -e "$BIN/Claude" ]; then pass "shims: no shim for a case-variant harness name"; else fail "shims: wrote $BIN/Claude"; fi
cfg '[alias]' 'evil = claude; rm -rf ~'
expect_rc "shims: an alias value with shell metacharacters is refused (exit 5)" 5 shims
if [ ! -e "$BIN/evil" ]; then pass "shims: no shim for an unsafe value"; else fail "shims: wrote $BIN/evil"; fi
rm -f "$BIN/mytool"

# --- shims: a missing/unreadable config must never delete existing shims ----
cfg 'default = claude' '[alias]' 'cl = claude --yolo'
PATH="$FAKE:/usr/bin:/bin" "$AG" shims >/dev/null 2>&1
mv "$AG_CONFIG" "$AG_CONFIG.bak"
expect_rc "shims: a missing config exits 5" 5 shims
if [ -x "$BIN/cl" ]; then pass "shims: existing shim untouched when config is missing"; else fail "shims: cl deleted despite a missing config"; fi
mv "$AG_CONFIG.bak" "$AG_CONFIG"

# --- doctor / install ----------------------------------------------------------------
doc() { PATH="$BIN:$FAKE:/usr/bin:/bin" "$AG" doctor >"$ROOT/doctor" 2>&1; }
cfg 'default = claude' '[alias]' 'cl = claude --yolo'
PATH="$FAKE:/usr/bin:/bin" "$AG" shims >/dev/null 2>&1
doc; rc=$?
if [ $rc = 0 ]; then pass "doctor: healthy machine exits 0"; else fail "doctor: exit $rc: $(cat "$ROOT/doctor")"; fi
if grep -q '^ok    claude:' "$ROOT/doctor"; then pass "doctor: reports claude"; else fail "doctor: no claude line"; fi

# --- doctor: ag on PATH resolving to (or shadowed away from) this install ---
mkdir -p "$ROOT/shadow"
printf '#!/bin/sh\necho "the silver searcher"\n' >"$ROOT/shadow/ag"; chmod 755 "$ROOT/shadow/ag"
PATH="$ROOT/shadow:$BIN:$FAKE:/usr/bin:/bin" "$AG" doctor >"$ROOT/doctor" 2>&1
if grep -q 'shadowing this install' "$ROOT/doctor"; then pass "doctor: detects a foreign ag shadowing on PATH"; else fail "doctor: shadow check: $(cat "$ROOT/doctor")"; fi
ln -sf "$AG" "$BIN/ag"
PATH="$BIN:$FAKE:/usr/bin:/bin" "$AG" doctor >"$ROOT/doctor" 2>&1
if grep -q '^ok    ag on PATH resolves to this install' "$ROOT/doctor"; then pass "doctor: ag on PATH resolving to this install is ok"; else fail "doctor: shadow-ok check: $(cat "$ROOT/doctor")"; fi
rm -f "$BIN/ag"

# --- doctor: alias fix texts -------------------------------------------------
cfg 'default = claude' '[alias]' 'bad!name = claude'
PATH="$FAKE:/usr/bin:/bin" "$AG" doctor >"$ROOT/doctor" 2>&1
if grep -q 'rename it in \[alias\]' "$ROOT/doctor"; then pass "doctor: invalid alias name gets a rename fix"; else fail "doctor: invalid alias name: $(cat "$ROOT/doctor")"; fi
cfg 'default = nope-harness'
PATH="$FAKE:/usr/bin:/bin" "$AG" doctor >"$ROOT/doctor" 2>&1
if grep -q 'set default to one of' "$ROOT/doctor"; then pass "doctor: unknown default harness names the valid ones"; else fail "doctor: unknown default: $(cat "$ROOT/doctor")"; fi
cfg 'default = claude' '[alias]' 'cl = claude --yolo'
{
  printf '#!/bin/sh\n%s\nexec '"'"'%s/ag'"'"' claude --yolo "$@"\n' '# generated by ag-shims' "$ROOT/moved-away"
} >"$BIN/cl"
chmod 755 "$BIN/cl"
PATH="$FAKE:/usr/bin:/bin" "$AG" doctor >"$ROOT/doctor" 2>&1
if grep -q 'points at a missing ag' "$ROOT/doctor"; then pass "doctor: an alias shim whose baked-in ag path is gone fails with the fix"; else fail "doctor: missing ag path: $(cat "$ROOT/doctor")"; fi
PATH="$FAKE:/usr/bin:/bin" "$AG" shims >/dev/null 2>&1

rm "$BIN/cl"
doc; rc=$?
if [ $rc = 1 ] && grep -q 'fix: ag shims' "$ROOT/doctor"; then pass "doctor: missing shim fails with the fix"; else fail "doctor: missing shim: exit $rc: $(cat "$ROOT/doctor")"; fi
PATH="$FAKE:/usr/bin:/bin" "$AG" shims >/dev/null 2>&1
# Capture first: with pipefail, `doctor | grep -q` would carry doctor's exit 1.
PATH="$FAKE:/usr/bin:/bin" "$AG" doctor >"$ROOT/doctor" 2>&1
if grep -q "is not on PATH" "$ROOT/doctor"; then pass "doctor: flags a bin dir missing from PATH"; else fail "doctor: PATH check: $(cat "$ROOT/doctor")"; fi
cfg 'default = claude'
rm "$FAKE/claude"
doc; rc=$?
if [ $rc = 1 ] && grep -q 'fix: ag install claude' "$ROOT/doctor"; then pass "doctor: default not installed fails with the fix"; else fail "doctor: default missing: exit $rc: $(cat "$ROOT/doctor")"; fi
mkfake claude
expect "install prints the official command" "npm install -g @openai/codex   # or: brew install --cask codex" install codex
expect_rc "install: unknown harness exits 2" 2 install nope

# --- installer -----------------------------------------------------------------------
inst() { # inst HOME ARGS… — run install.sh as a fresh user with HOME=$1
  local h=$1; shift
  env -u AG_CONFIG -u AG_BIN_DIR -u AG_HOME -u AG_CODEX_FALLBACKS HOME="$h" XDG_CONFIG_HOME= \
    PATH="$FAKE:/usr/bin:/bin" bash "$REPO/tools/ag/install.sh" --from "$REPO/tools/ag" "$@" >"$ROOT/inst.log" 2>&1
}
H1="$ROOT/home1"; mkdir -p "$H1"
inst "$H1"; rc=$?
if [ $rc = 0 ]; then pass "install: exits 0"; else fail "install: exit $rc: $(cat "$ROOT/inst.log")"; fi
if [ -L "$H1/.local/bin/ag" ] && [ -x "$H1/.local/share/ag/ag" ]; then pass "install: code in ~/.local/share/ag, link in ~/.local/bin"; else fail "install: layout"; fi
v=$(env -u AG_CONFIG -u AG_BIN_DIR HOME="$H1" PATH="$H1/.local/bin:$FAKE:/usr/bin:/bin" ag version 2>&1)
if [ "$v" = "ag 0.1.0" ]; then pass "install: ag runs through the symlink"; else fail "install: ag version gave [$v]"; fi
if grep -q '^default = claude$' "$H1/.config/ag/config"; then pass "install: starter config defaults to the first installed harness"; else fail "install: config: $(cat "$H1/.config/ag/config")"; fi
echo 'yolo = true' >>"$H1/.config/ag/config"
inst "$H1"
if grep -q '^yolo = true$' "$H1/.config/ag/config"; then pass "install: re-run keeps an existing config"; else fail "install: config overwritten"; fi
v2=$(env -u AG_CONFIG -u AG_BIN_DIR HOME="$H1" PATH="$H1/.local/bin:$FAKE:/usr/bin:/bin" ag version 2>&1)
if [ "$v2" = "ag 0.1.0" ]; then pass "install: re-run over our own symlink still works"; else fail "install: re-run ag version gave [$v2]"; fi
H2="$ROOT/home2"; mkdir -p "$H2"
inst "$H2" --default codex
if grep -q '^default = codex$' "$H2/.config/ag/config"; then pass "install: --default sets the default"; else fail "install: --default ignored"; fi
H3="$ROOT/home3"; mkdir -p "$H3/.local/share/ag"; echo keep >"$H3/.local/share/ag/data"
inst "$H3"; rc=$?
if [ $rc = 5 ] && [ -f "$H3/.local/share/ag/data" ]; then pass "install: refuses to replace a non-ag directory"; else fail "install: non-ag dir: exit $rc"; fi

# --- installer: never replace a foreign ag on PATH (e.g. the Silver Searcher)
H4="$ROOT/home4"; mkdir -p "$H4/.local/bin"
printf 'echo "the silver searcher"\n' >"$H4/.local/bin/ag"; chmod 755 "$H4/.local/bin/ag"
inst "$H4"; rc=$?
if [ $rc = 5 ]; then pass "install: a foreign ag on \$AG_BIN_DIR/ag exits 5"; else fail "install: foreign ag: exit $rc: $(cat "$ROOT/inst.log")"; fi
if [ "$(cat "$H4/.local/bin/ag")" = 'echo "the silver searcher"' ]; then pass "install: the foreign ag file is left byte-for-byte intact"; else fail "install: foreign ag was modified: [$(cat "$H4/.local/bin/ag" 2>/dev/null)]"; fi
if [ -x "$H4/.local/share/ag/ag" ]; then pass "install: the ag tree still gets copied even when the bin symlink is blocked"; else fail "install: tree missing after blocked symlink"; fi

# --- installer: AG_HOME / AG_BIN_DIR must be absolute -----------------------
H6="$ROOT/home6"; mkdir -p "$H6"
env -u AG_CONFIG -u AG_BIN_DIR AG_HOME="relative/ag-home" HOME="$H6" XDG_CONFIG_HOME= \
  PATH="$FAKE:/usr/bin:/bin" bash "$REPO/tools/ag/install.sh" --from "$REPO/tools/ag" >"$ROOT/inst.log" 2>&1
rc=$?
if [ $rc = 2 ]; then pass "install: a relative AG_HOME is rejected (exit 2)"; else fail "install: relative AG_HOME: exit $rc: $(cat "$ROOT/inst.log")"; fi
env -u AG_CONFIG -u AG_HOME AG_BIN_DIR="relative/bin" HOME="$H6" XDG_CONFIG_HOME= \
  PATH="$FAKE:/usr/bin:/bin" bash "$REPO/tools/ag/install.sh" --from "$REPO/tools/ag" >"$ROOT/inst.log" 2>&1
rc=$?
if [ $rc = 2 ]; then pass "install: a relative AG_BIN_DIR is rejected (exit 2)"; else fail "install: relative AG_BIN_DIR: exit $rc: $(cat "$ROOT/inst.log")"; fi

# --- installer: a failing `ag shims` does not abort the install -------------
H5="$ROOT/home5"; mkdir -p "$H5/.local/bin" "$H5/.config/ag"
printf 'echo mine\n' >"$H5/.local/bin/cl"; chmod 755 "$H5/.local/bin/cl"
cat >"$H5/.config/ag/config" <<'CFGEOF'
default = claude
order = claude codex
yolo = false
[alias]
cl = claude --yolo
CFGEOF
inst "$H5"; rc=$?
if [ $rc = 0 ]; then pass "install: a failing ag shims does not abort the installer (still exits 0)"; else fail "install: exit $rc when shims fails: $(cat "$ROOT/inst.log")"; fi
if [ "$(cat "$H5/.local/bin/cl")" = "echo mine" ]; then pass "install: the foreign alias file is left untouched when shims fails"; else fail "install: cl was overwritten"; fi
if grep -q 'ag shims reported a problem' "$ROOT/inst.log"; then pass "install: prints the shims-failure message"; else fail "install: no shims-failure message: $(cat "$ROOT/inst.log")"; fi
if grep -q 'installed ag to' "$ROOT/inst.log"; then pass "install: still installs the tree + symlink despite the shims failure"; else fail "install: no install-succeeded message: $(cat "$ROOT/inst.log")"; fi

# --- installer: --alias --------------------------------------------------------------
H7="$ROOT/home7"; mkdir -p "$H7"
inst "$H7" --alias 'cl=claude --yolo'; rc=$?
if [ $rc = 0 ]; then pass "install --alias: exits 0"; else fail "install --alias: exit $rc: $(cat "$ROOT/inst.log")"; fi
if awk '/^\[alias\]/{s=1;next} /^\[/{s=0} s && /^cl = claude --yolo$/{f=1} END{exit !f}' "$H7/.config/ag/config"; then
  pass "install --alias: cl lands under [alias]"; else fail "install --alias: config: $(cat "$H7/.config/ag/config")"; fi
if grep -q '^# generated by ag-shims' "$H7/.local/bin/cl" 2>/dev/null; then pass "install --alias: cl shim generated"; else fail "install --alias: no cl shim"; fi
sed -i.bak 's/^cl = claude --yolo$/cl = codex/' "$H7/.config/ag/config" && rm -f "$H7/.config/ag/config.bak"
inst "$H7" --alias 'cl=claude --yolo'
if grep -q '^cl = codex$' "$H7/.config/ag/config" && ! grep -q '^cl = claude --yolo$' "$H7/.config/ag/config"; then
  pass "install --alias: a user's existing alias is kept"; else fail "install --alias: user alias overwritten: $(cat "$H7/.config/ag/config")"; fi
H8="$ROOT/home8"; mkdir -p "$H8/.config/ag"; printf 'default = claude\n' >"$H8/.config/ag/config"
inst "$H8" --alias 'cx=codex'
if awk '/^\[alias\]/{s=1;next} s && /^cx = codex$/{f=1} END{exit !f}' "$H8/.config/ag/config"; then
  pass "install --alias: adds an [alias] section when the config has none"; else fail "install --alias: no section: $(cat "$H8/.config/ag/config")"; fi
H9="$ROOT/home9"; mkdir -p "$H9"
inst "$H9" --alias 'evil=claude; rm -r ~'; rc=$?
if [ $rc = 2 ] && [ ! -e "$H9/.local/share/ag" ]; then pass "install --alias: an unsafe value exits 2 before installing"; else fail "install --alias: unsafe value: exit $rc"; fi
inst "$H9" --alias 'noequals'; rc=$?
if [ $rc = 2 ]; then pass "install --alias: NAME=VALUE is required"; else fail "install --alias: missing '=': exit $rc"; fi

# --- summary ----------------------------------------------------------------
echo "ag-test: $PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]
