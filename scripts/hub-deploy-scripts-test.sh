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
# Records every call. upgrade.sh speaks only `docker compose --env-file <f>
# <sub> …`, so the pair is dropped before dispatching on <sub>. `pull` of a
# `missing` FLEET_HUB_TAG fails like ghcr does; `stop` refuses when no
# pre-upgrade backup exists yet (that is the order under test); `exec …
# healthcheck` is healthy; `exec … --version` answers whatever tag .env pins.
echo "docker $*" >>"$FAKE_LOG"
[ "${1:-}" = compose ] || exit 0
shift
[ "${1:-}" = --env-file ] && shift 2
case "${1:-}" in
  pull) [ "${FLEET_HUB_TAG:-}" = missing ] && { echo "manifest unknown" >&2; exit 1; } ;;
  stop) ls "$FAKE_DIR"/backups/pre-*.db >/dev/null 2>&1 || { echo "stop before backup" >&2; exit 9; } ;;
  exec)
    case "${@: -1}" in
      healthcheck) exit 0 ;;
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
chmod +x "$FAKE/docker" "$FAKE/curl"

U="$ROOT/up"; mkdir -p "$U/data"
cp "$REPO/deploy/hub/backup.sh" "$REPO/deploy/hub/upgrade.sh" "$U/"
sqlite3 "$U/data/state.db" 'CREATE TABLE t(x); INSERT INTO t VALUES (1);'
printf 'FLEET_HUB_TAG=0.3.0\n' >"$U/.env"
printf 'FLEET_HUB_PUBLIC_URL=https://fleet.example.com\n' >"$U/fleet-hub.env"
printf 'cl_readonly\n' >"$U/readonly.token"
export FAKE_LOG="$ROOT/calls.log" FAKE_DIR="$U"
ord() { grep -n -- "$1" "$FAKE_LOG" | head -n1 | cut -d: -f1; }

: >"$FAKE_LOG"
PATH="$FAKE:$PATH" FLEET_HUB_DIR="$U" bash "$U/upgrade.sh" 0.3.1 >"$ROOT/upgrade1.log" 2>&1
check "upgrade exits 0" test $? = 0
check ".env now pins 0.3.1" grep -qx 'FLEET_HUB_TAG=0.3.1' "$U/.env"
check "one pre-upgrade backup was taken" test "$(count "$U"/backups/pre-0.3.1-*.db)" = 1
check "every compose call reads .env through --env-file" test -z "$(grep '^docker ' "$FAKE_LOG" | grep -v -- "^docker compose --env-file $U/.env ")"
check "pull precedes stop" test "$(ord ' pull fleet-hub')" -lt "$(ord ' stop fleet-hub')"
check "stop precedes up" test "$(ord ' stop fleet-hub')" -lt "$(ord ' up -d fleet-hub')"
check "up precedes the healthcheck" test "$(ord ' up -d fleet-hub')" -lt "$(ord 'healthcheck')"
check "the healthcheck precedes --version" test "$(ord 'healthcheck')" -lt "$(ord '--version')"
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

rm -f "$U/readonly.token"; : >"$FAKE_LOG"
PATH="$FAKE:$PATH" FLEET_HUB_DIR="$U" bash "$U/upgrade.sh" 0.3.1 >/dev/null 2>&1
check "without a token file the network check is skipped" test -z "$(grep '^curl' "$FAKE_LOG")"

echo "hub-deploy-scripts-test: $PASS passed, $FAIL failed (logs in $ROOT)"
[ "$FAIL" = 0 ]
