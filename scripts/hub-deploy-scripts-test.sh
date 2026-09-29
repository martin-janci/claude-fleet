#!/usr/bin/env bash
# Tests for deploy/hub/backup.sh and deploy/hub/upgrade.sh, against a fake
# `docker` and `curl` on PATH. Needs bash and sqlite3, nothing else, and
# touches nothing outside its temp ROOT. Mirrors scripts/hub-e2e.sh: a
# greppable log root, PASS/FAIL lines, exit 1 on any failure.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
command -v sqlite3 >/dev/null 2>&1 || { echo "hub-deploy-scripts-test: sqlite3 not found" >&2; exit 2; }
TMP_BASE="/tmp/claude-$(id -u)"
[ -d "$TMP_BASE" ] || TMP_BASE=/tmp
ROOT="$(mktemp -d -p "$TMP_BASE" hub-deploy.XXXXXX)" || { echo "hub-deploy-scripts-test: mktemp failed" >&2; exit 1; }
echo "hub-deploy-scripts-test: log root: $ROOT"

PASS=0; FAIL=0
pass() { PASS=$((PASS + 1)); echo "PASS: $*"; }
fail() { FAIL=$((FAIL + 1)); echo "FAIL: $*" >&2; }
check() { local name=$1; shift; if "$@"; then pass "$name"; else fail "$name"; fi; }
count() { ls -1 "$@" 2>/dev/null | wc -l | tr -d ' '; }
# Runs "$@" but kills it after $1 s. `timeout` is GNU coreutils: stock macOS
# has none, and a missing one exits 127, which reads as a failed check. So a
# background watchdog stands in when it is absent. Its stdio is /dev/null, so
# a sleep it leaves behind holds no pipe of ours open.
bounded() {
  local secs=$1; shift
  if command -v timeout >/dev/null 2>&1; then timeout "$secs" "$@"; return; fi
  "$@" &
  local pid=$! rc
  ( sleep "$secs"; kill "$pid" ) >/dev/null 2>&1 &
  local dog=$!
  wait "$pid"; rc=$?
  kill "$dog" 2>/dev/null; wait "$dog" 2>/dev/null
  return "$rc"
}

# --- backup.sh ---------------------------------------------------------------
D="$ROOT/hub"; mkdir -p "$D/data"
sqlite3 "$D/data/state.db" 'CREATE TABLE t(x); INSERT INTO t VALUES (1),(2),(3);'
B="$D/backups"
FLEET_HUB_DATA="$D/data" FLEET_HUB_BACKUPS="$B" KEEP=2 bash "$REPO/deploy/hub/backup.sh" >"$ROOT/backup1.log" 2>&1
check "backup exits 0" test $? = 0
check "backup writes one state-*.db" test "$(count "$B"/state-*.db)" = 1
f="$(ls -1 "$B"/state-*.db)"
check "the copy holds the rows" test "$(sqlite3 "$f" 'SELECT COUNT(*) FROM t')" = 3
check "the copy passes integrity_check" test "$(sqlite3 "$f" 'PRAGMA integrity_check')" = ok
check "the copy is a single file (WAL folded in)" test ! -e "$f-wal"
sleep 1; FLEET_HUB_DATA="$D/data" FLEET_HUB_BACKUPS="$B" KEEP=2 bash "$REPO/deploy/hub/backup.sh" >/dev/null 2>&1
sleep 1; FLEET_HUB_DATA="$D/data" FLEET_HUB_BACKUPS="$B" KEEP=2 bash "$REPO/deploy/hub/backup.sh" >/dev/null 2>&1
check "retention keeps the KEEP=2 newest" test "$(count "$B"/state-*.db)" = 2
check "retention is per prefix" test "$(count "$B"/pre-*.db)" = 0
mode() { stat -c %a "$1" 2>/dev/null || stat -f %Lp "$1"; }
check "the backup dir is 0700" test "$(mode "$B")" = 700
check "every backup is 0600" test -z "$(for g in "$B"/state-*.db; do [ "$(mode "$g")" = 600 ] || echo "$g"; done)"
# A directory an older run (or a person) left 0755 is tightened before the copy.
B2="$ROOT/loose"; mkdir -m 755 "$B2"
FLEET_HUB_DATA="$D/data" FLEET_HUB_BACKUPS="$B2" bash "$REPO/deploy/hub/backup.sh" >/dev/null 2>&1
check "an existing 0755 backup dir is tightened to 0700" test "$(mode "$B2")" = 700
check "…and the copy in it is 0600" test "$(mode "$(ls -1 "$B2"/state-*.db)")" = 600
# A `'`, a `"` and a `\` in the path: the .backup argument is escaped, not cut.
BQ="$ROOT/it's \"q\" \\n dir"
FLEET_HUB_DATA="$D/data" FLEET_HUB_BACKUPS="$BQ" bash "$REPO/deploy/hub/backup.sh" >"$ROOT/backup-quote.log" 2>&1
check "a backup dir with quotes and a backslash works" test $? = 0
check "…and the copy lands in it, whole" test "$(sqlite3 "$(ls -1 "$BQ"/state-*.db)" 'SELECT COUNT(*) FROM t')" = 3
# PREFIX=pre-1.0.0 prunes its own files only, not pre-1.0.0-rc1-*.
BP="$ROOT/prefix"; mkdir -p "$BP"
for s in 20260101-000000 20260102-000000 20260103-000000; do sqlite3 "$BP/pre-1.0.0-rc1-$s.db" 'CREATE TABLE t(x)'; done
for _ in 1 2; do PREFIX=pre-1.0.0 KEEP=1 FLEET_HUB_DATA="$D/data" FLEET_HUB_BACKUPS="$BP" bash "$REPO/deploy/hub/backup.sh" >/dev/null 2>&1; done
check "PREFIX=pre-1.0.0 keeps its KEEP=1 newest" test "$(count "$BP"/pre-1.0.0-2*.db)" = 1
check "…and never prunes pre-1.0.0-rc1-*" test "$(count "$BP"/pre-1.0.0-rc1-*.db)" = 3
FLEET_HUB_DATA="$ROOT/nowhere" bash "$REPO/deploy/hub/backup.sh" >/dev/null 2>&1
check "a missing database exits 2" test $? = 2
printf 'not a database' >"$D/data/state.db"
FLEET_HUB_DATA="$D/data" FLEET_HUB_BACKUPS="$B" bash "$REPO/deploy/hub/backup.sh" >/dev/null 2>&1
check "a corrupt database exits non-zero" test $? != 0
check "…and leaves no half-written copy behind" test "$(count "$B"/state-*.db)" = 2

# --- upgrade.sh: fake docker/curl ------------------------------------------
FAKE="$ROOT/bin"; mkdir -p "$FAKE"
cat >"$FAKE/docker" <<'EOF'
#!/usr/bin/env bash
# Records every call. upgrade.sh speaks `docker compose --env-file <f> <sub>
# …`, so the pair is dropped before dispatching on <sub>, plus a plain
# `docker inspect` / `docker image inspect` for the running image. `pull` of
# a `missing` FLEET_HUB_TAG fails like ghcr does; `ps -q` names container
# c0ffee, which runs image sha256:0ld1d, pulled as ghcr.io/x/fleet-hub@sha256:d1g;
# `exec … healthcheck` is healthy unless FAKE_UNHEALTHY=1; `exec … --version`
# answers whatever tag .env pins.
echo "docker $*" >>"$FAKE_LOG"
case "${1:-}" in
  inspect) echo "sha256:0ld1d"; exit 0 ;;
  image) echo "ghcr.io/x/fleet-hub@sha256:d1g ghcr.io/y/fleet-hub@sha256:d1g"; exit 0 ;;
  compose) ;;
  *) exit 0 ;;
esac
shift
[ "${1:-}" = --env-file ] && shift 2
case "${1:-}" in
  pull) [ "${FLEET_HUB_TAG:-}" = missing ] && { echo "manifest unknown" >&2; exit 1; } ;;
  ps) echo c0ffee ;;
  exec)
    case "${@: -1}" in
      healthcheck) [ "${FAKE_UNHEALTHY:-0}" = 1 ] && exit 1; exit 0 ;;
      --version) echo "fleet-hub $(sed -n 's/^FLEET_HUB_TAG=//p' "$FAKE_DIR/.env")" ;;
    esac ;;
esac
exit 0
EOF
cat >"$FAKE/curl" <<'EOF'
#!/usr/bin/env bash
# upgrade.sh hands the Authorization header over on stdin (`-H @-`).
echo "curl $*" >>"$FAKE_LOG"
case " $* " in *" @- "*) sed 's/^/curl-stdin: /' >>"$FAKE_LOG" ;; esac
tag="$(sed -n 's/^FLEET_HUB_TAG=//p' "$FAKE_DIR/.env")"
printf '{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"{\\"version\\":\\"%s\\",\\"db_ready\\":true}"}]}}' "$tag"
EOF
# sqlite3 as backup.sh runs it: logged (so the backup's place in the order
# is visible), and `.backup` fails on FAKE_BACKUP_FAIL=1.
cat >"$FAKE/sqlite3" <<'EOF'
#!/usr/bin/env bash
echo "sqlite3 $*" >>"$FAKE_LOG"
case "$*" in *".backup "*) [ "${FAKE_BACKUP_FAIL:-0}" = 1 ] && { echo "disk I/O error" >&2; exit 1; } ;; esac
exec "$FAKE_REAL_SQLITE" "$@"
EOF
FAKE_REAL_SQLITE="$(command -v sqlite3)"; export FAKE_REAL_SQLITE
chmod +x "$FAKE/docker" "$FAKE/curl" "$FAKE/sqlite3"

U="$ROOT/up"; mkdir -p "$U/data"
cp "$REPO/deploy/hub/backup.sh" "$REPO/deploy/hub/upgrade.sh" "$U/"
sqlite3 "$U/data/state.db" 'CREATE TABLE t(x); INSERT INTO t VALUES (1);'
printf 'FLEET_HUB_TAG=0.3.0\n' >"$U/.env"
printf 'FLEET_HUB_PUBLIC_URL=https://fleet.example.com\n' >"$U/fleet-hub.env"
printf 'cl_readonly\n' >"$U/readonly.token"
export FAKE_LOG="$ROOT/calls.log" FAKE_DIR="$U"
ord() { grep -n -- "$1" "$FAKE_LOG" | head -n1 | cut -d: -f1; }
# `before A B`: the first call matching A precedes the first matching B. A
# call that never happened fails by name, not as `test`'s "integer expected".
before() {
  local a b; a="$(ord "$1")"; b="$(ord "$2")"
  [ -n "$a" ] || { echo "  no call matching '$1' in $FAKE_LOG" >&2; return 1; }
  [ -n "$b" ] || { echo "  no call matching '$2' in $FAKE_LOG" >&2; return 1; }
  [ "$a" -lt "$b" ]
}

: >"$FAKE_LOG"
PATH="$FAKE:$PATH" FLEET_HUB_DIR="$U" bash "$U/upgrade.sh" 0.3.1 >"$ROOT/upgrade1.log" 2>&1
check "upgrade exits 0" test $? = 0
check ".env now pins 0.3.1" grep -qx 'FLEET_HUB_TAG=0.3.1' "$U/.env"
check "one pre-upgrade backup was taken" test "$(count "$U"/backups/pre-0.3.1-*.db)" = 1
# Only compose calls read .env: `docker inspect` / `docker image inspect`
# (noting the running image for the rollback line) take no env file.
check "every compose call reads .env through --env-file" test -z "$(grep '^docker compose ' "$FAKE_LOG" | grep -v -- "^docker compose --env-file $U/.env ")"
check "pull precedes stop" before ' pull fleet-hub' ' stop fleet-hub'
check "the running image is noted before the stop" before 'docker inspect' ' stop fleet-hub'
check "stop precedes the backup (an exact, offline copy)" before ' stop fleet-hub' '^sqlite3 .*\.backup'
check "the backup precedes up" before '^sqlite3 .*\.backup' ' up -d fleet-hub'
check "the running image is named" grep -q 'runs image sha256:0ld1d (ghcr.io/x/fleet-hub@sha256:d1g)' "$ROOT/upgrade1.log"
check "up precedes the healthcheck" before ' up -d fleet-hub' 'healthcheck'
check "the healthcheck precedes --version" before 'healthcheck' '--version'
check "fleet_health is asked with the readonly token" grep -q '^curl-stdin: Authorization: Bearer cl_readonly' "$FAKE_LOG"
check "…which never reaches curl's command line" test -z "$(grep '^curl .*cl_readonly' "$FAKE_LOG")"
check "…against /mcp/json on the public URL" grep -q 'https://fleet.example.com/mcp/json' "$FAKE_LOG"
check "…and the version is confirmed" grep -q 'fleet_health.version = 0.3.1' "$ROOT/upgrade1.log"

: >"$FAKE_LOG"
PATH="$FAKE:$PATH" FLEET_HUB_DIR="$U" bash "$U/upgrade.sh" missing >"$ROOT/upgrade2.log" 2>&1
check "a tag ghcr does not have fails" test $? != 0
check "…before the pin moved" grep -qx 'FLEET_HUB_TAG=0.3.1' "$U/.env"
check "…and before the hub was stopped" test -z "$(grep ' stop fleet-hub' "$FAKE_LOG")"

PATH="$FAKE:$PATH" FLEET_HUB_DIR="$U" bash "$U/upgrade.sh" v0.3.2 >/dev/null 2>&1
check "a v-prefixed tag is refused with exit 2" test $? = 2

: >"$FAKE_LOG"
HEALTH_TRIES=abc PATH="$FAKE:$PATH" FLEET_HUB_DIR="$U" bounded 20 bash "$U/upgrade.sh" 0.3.2 >"$ROOT/upgrade-badtries.log" 2>&1
check "a non-numeric HEALTH_TRIES is refused with exit 2 (no spin)" test $? = 2
check "…and says so" grep -q "HEALTH_TRIES must be a number, got 'abc'" "$ROOT/upgrade-badtries.log"
KEEP=x PATH="$FAKE:$PATH" FLEET_HUB_DIR="$U" bounded 20 bash "$U/upgrade.sh" 0.3.2 >"$ROOT/upgrade-badkeep.log" 2>&1
check "a non-numeric KEEP is refused with exit 2" test $? = 2
check "…and says so" grep -q "KEEP must be a number, got 'x'" "$ROOT/upgrade-badkeep.log"
check "…both before anything was stopped" test -z "$(grep ' stop fleet-hub' "$FAKE_LOG")"

# A backup that fails after the stop: the old container starts again, the pin
# stays, nothing new is started, and the script exits non-zero.
: >"$FAKE_LOG"
FAKE_BACKUP_FAIL=1 PATH="$FAKE:$PATH" FLEET_HUB_DIR="$U" bash "$U/upgrade.sh" 0.3.2 >"$ROOT/upgrade-nobackup.log" 2>&1
check "a failed backup after the stop exits 1" test $? = 1
check "…the pin did not move" grep -qx 'FLEET_HUB_TAG=0.3.1' "$U/.env"
check "…the old container is started again after the stop" test "$(ord ' stop fleet-hub')" -lt "$(ord ' start fleet-hub')"
check "…and nothing new is brought up" test -z "$(grep ' up -d' "$FAKE_LOG")"
check "…no pre-0.3.2 backup is left" test "$(count "$U"/backups/pre-0.3.2-*.db)" = 0

# A tag set but no state.db where FLEET_HUB_DATA says: refused, hub untouched.
: >"$FAKE_LOG"
FLEET_HUB_DATA="$ROOT/nowhere" PATH="$FAKE:$PATH" FLEET_HUB_DIR="$U" bash "$U/upgrade.sh" 0.3.2 >/dev/null 2>&1
check "a missing state.db is refused with exit 2" test $? = 2
check "…before the hub was stopped" test -z "$(grep ' stop fleet-hub' "$FAKE_LOG")"

# A failure after the stop prints the rollback, naming the image that ran.
: >"$FAKE_LOG"
FAKE_UNHEALTHY=1 HEALTH_TRIES=1 PATH="$FAKE:$PATH" FLEET_HUB_DIR="$U" bash "$U/upgrade.sh" 0.3.2 >"$ROOT/upgrade-unhealthy.log" 2>&1
check "an unhealthy new version exits non-zero" test $? != 0
check "…the rollback names the previous tag" grep -q 'ROLLBACK: set FLEET_HUB_TAG=0.3.1' "$ROOT/upgrade-unhealthy.log"
check "…and the image it ran, to re-point a moved tag" grep -q 'docker tag sha256:0ld1d ghcr.io/x/fleet-hub:0.3.1' "$ROOT/upgrade-unhealthy.log"
printf 'FLEET_HUB_TAG=0.3.1\n' >"$U/.env"

rm -f "$U/readonly.token"; : >"$FAKE_LOG"
PATH="$FAKE:$PATH" FLEET_HUB_DIR="$U" bash "$U/upgrade.sh" 0.3.1 >/dev/null 2>&1
check "without a token file the network check is skipped" test -z "$(grep '^curl' "$FAKE_LOG")"

echo "hub-deploy-scripts-test: $PASS passed, $FAIL failed (logs in $ROOT)"
[ "$FAIL" = 0 ]
