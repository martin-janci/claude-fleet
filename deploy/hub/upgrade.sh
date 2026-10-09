#!/usr/bin/env bash
# deploy/hub/upgrade.sh <version> — upgrade a compose deployment whose image
# tag lives in .env (deploy/hub/behind-proxy/docker-compose.yml):
#
#   pull → note the running image → compose stop → backup.sh (an exact copy
#   of the stopped hub's state) → FLEET_HUB_TAG=<v> → up -d → the image's own
#   healthcheck → `fleet-hub --version`
#   → fleet_health.version over the public URL, only with a READONLY client
#     token kept beside the compose file → keep 3 backups per version.
#
# FLEET_HUB_DIR is where docker-compose.yml, .env, fleet-hub.env, ./data and
# backup.sh live. Runs as root (the docker socket). The rollback is printed
# on every failure after the stop, naming the image that ran before (a tag
# can move; its image id cannot). A backup that fails after the stop starts
# the old container again and exits 1: never an upgrade without a backup.
# The tag has NO v: `v0.3.1` is `0.3.1`.
# The image is the compose file's own `image:` line; every compose call reads
# FLEET_HUB_ENV_FILE (default .env) through --env-file.
set -euo pipefail

NEW="${1:?usage: upgrade.sh <version>   (the image tag, e.g. 0.3.1 — no v)}"
DIR="${FLEET_HUB_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)}"
ENV_FILE="${FLEET_HUB_ENV_FILE:-$DIR/.env}"
case "$ENV_FILE" in /*) ;; *) ENV_FILE="$PWD/$ENV_FILE" ;; esac
KEEP="${KEEP:-3}"
TOKEN_FILE="${FLEET_HUB_READONLY_TOKEN_FILE:-$DIR/readonly.token}"
HEALTH_TRIES="${HEALTH_TRIES:-60}"
DATA="${FLEET_HUB_DATA:-$DIR/data}"

case "$NEW" in v*) echo "upgrade: the image tag has no v prefix (got $NEW; the tag for v0.3.1 is 0.3.1)" >&2; exit 2;; esac
# The tag goes into a sed replacement and .env: nothing but a docker tag's characters.
case "$NEW" in *[!0-9A-Za-z._-]*) echo "upgrade: the tag must be [0-9A-Za-z._-]+, got '$NEW'" >&2; exit 2;; esac
# Both are checked here, before anything is stopped: a non-number would spin
# the health loop forever, or fail backup.sh with the hub already down.
case "$KEEP" in ''|*[!0-9]*) echo "upgrade: KEEP must be a number, got '$KEEP'" >&2; exit 2;; esac
case "$HEALTH_TRIES" in ''|*[!0-9]*) echo "upgrade: HEALTH_TRIES must be a number, got '$HEALTH_TRIES'" >&2; exit 2;; esac
cd "$DIR"
[ -f "$ENV_FILE" ] || { echo "upgrade: no $ENV_FILE — FLEET_HUB_TAG lives there (copy .env.example)" >&2; exit 2; }
[ -f "$DIR/backup.sh" ] || { echo "upgrade: no $DIR/backup.sh beside this script" >&2; exit 2; }
OLD="$(sed -n 's/^FLEET_HUB_TAG=//p' "$ENV_FILE" | head -n1)"
dc() { docker compose --env-file "$ENV_FILE" "$@"; }
STOPPED=0
BACKED_UP=0
PREV_ID=""
PREV_DIGEST=""
PREV_REPO=""
rollback() {
  if [ -z "$OLD" ]; then
    echo "upgrade: no previous FLEET_HUB_TAG to roll back to; see docker compose logs fleet-hub and rerun upgrade.sh" >&2
    return
  fi
  echo "upgrade: ROLLBACK: set FLEET_HUB_TAG=$OLD in $ENV_FILE, then: docker compose --env-file $ENV_FILE up -d fleet-hub" >&2
  if [ -n "$PREV_ID" ]; then
    echo "upgrade: $OLD was image $PREV_ID${PREV_DIGEST:+ ($PREV_DIGEST)}; if the tag has moved since, re-point it first: docker tag $PREV_ID ${PREV_REPO:-<image>}:$OLD" >&2
  fi
  if [ "$BACKED_UP" = 1 ]; then
    echo "upgrade: if $NEW migrated the database, first restore the newest backups/pre-$NEW-*.db (docs/hub.md → Backups → Restore drill)" >&2
  fi
}
# Every exit after the stop that is not success — an explicit `exit 1` below
# or any `set -e` failure — prints the rollback exactly once.
on_exit() {
  local rc=$?
  if [ "$rc" -ne 0 ] && [ "$STOPPED" = 1 ]; then rollback; fi
}
trap on_exit EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
echo "upgrade: ${OLD:-<unset>} -> $NEW in $DIR"

# 1. Pull first: a tag ghcr does not have stops here, with the hub untouched.
#    The shell's FLEET_HUB_TAG outranks the env file, so this pulls exactly
#    what `up` will run.
FLEET_HUB_TAG="$NEW" dc pull fleet-hub
# 2. Before the stop, preflight what the backup needs — a missing state.db
#    or sqlite3 stops here with the hub untouched, rather than migrating with
#    no backup. Skipped only on a first install (no FLEET_HUB_TAG, no
#    state.db): there is nothing to back up.
TAKE_BACKUP=1
if [ -z "$OLD" ] && [ ! -f "$DATA/state.db" ]; then
  echo "upgrade: first install (no FLEET_HUB_TAG, no $DATA/state.db) — no backup to take"
  TAKE_BACKUP=0
elif [ ! -f "$DATA/state.db" ]; then
  echo "upgrade: no database at $DATA/state.db (set FLEET_HUB_DATA) — not upgrading without a backup" >&2
  exit 2
elif ! command -v "${SQLITE:-sqlite3}" >/dev/null 2>&1; then
  echo "upgrade: ${SQLITE:-sqlite3} not found — backup.sh needs it; not upgrading without a backup" >&2
  exit 2
fi
# 3. Note the image the hub runs now, by id and digest: the rollback names it,
#    since the old tag may have been re-pushed by the time anyone rolls back.
#    Best effort — a hub that is not running has none.
if [ -n "$OLD" ]; then
  CID="$(dc ps -q fleet-hub 2>/dev/null | head -n1 || true)"
  if [ -n "$CID" ]; then
    PREV_ID="$(docker inspect --format '{{.Image}}' "$CID" 2>/dev/null || true)"
  fi
  if [ -n "$PREV_ID" ]; then
    PREV_DIGEST="$(docker image inspect --format '{{join .RepoDigests " "}}' "$PREV_ID" 2>/dev/null | cut -d' ' -f1 || true)"
    PREV_REPO="${PREV_DIGEST%@*}"
    echo "upgrade: $OLD runs image $PREV_ID${PREV_DIGEST:+ ($PREV_DIGEST)}"
  fi
fi
# 4. SIGTERM under the compose stop_grace_period (drain + tick shutdown); `stop`, not `down`.
#    An empty FLEET_HUB_TAG (a fresh .env.example) means nothing of this
#    compose is running, and compose would refuse to interpolate the image.
if [ -n "$OLD" ]; then
  dc stop fleet-hub
else
  echo "upgrade: FLEET_HUB_TAG is empty in $ENV_FILE — first install, nothing to stop"
fi
STOPPED=1
# 5. The backup, of a hub that writes nothing now: every write up to the stop
#    is in it, none after it can be lost by a restore. A failed backup starts
#    the old container again (`start`: the very image it ran) and stops here.
if [ "$TAKE_BACKUP" = 1 ]; then
  if ! PREFIX="pre-$NEW" KEEP="$KEEP" FLEET_HUB_DATA="$DATA" FLEET_HUB_BACKUPS="$DIR/backups" bash "$DIR/backup.sh"; then
    echo "upgrade: the pre-upgrade backup failed — not upgrading${OLD:+; starting $OLD again}" >&2
    if [ -n "$OLD" ] && dc start fleet-hub; then STOPPED=0; fi
    exit 1
  fi
  BACKED_UP=1
fi
# 6. Move the pin. `-i.bak` is the spelling GNU and BSD sed both accept.
if grep -q '^FLEET_HUB_TAG=' "$ENV_FILE"; then
  sed -i.bak "s|^FLEET_HUB_TAG=.*|FLEET_HUB_TAG=$NEW|" "$ENV_FILE" && rm -f "$ENV_FILE.bak"
else
  printf 'FLEET_HUB_TAG=%s\n' "$NEW" >>"$ENV_FILE"
fi
# 7. Start on the new tag and wait for the image's own healthcheck.
dc up -d fleet-hub
i=0
until dc exec -T fleet-hub fleet-hub healthcheck >/dev/null 2>&1; do
  i=$((i + 1))
  if [ "$i" -ge "$HEALTH_TRIES" ]; then
    echo "upgrade: the hub did not pass its healthcheck within $HEALTH_TRIES s (docker compose logs fleet-hub)" >&2
    exit 1
  fi
  sleep 1
done
# 8. The binary names its version without a credential.
GOT="$(dc exec -T fleet-hub fleet-hub --version | tr -d '\r')"
case "$GOT" in
  *" $NEW") echo "upgrade: running $GOT" ;;
  *) echo "upgrade: running '$GOT', expected 'fleet-hub $NEW' (the pin did not take?)" >&2; exit 1 ;;
esac
# 9. Over the network, with a READONLY client token when one is kept beside
#    the compose file (mint a code with `fleet-hub pair --mode readonly
#    --name upgrade-check`, redeem it with `POST /pair {"code":…}` and
#    keep the answer's `token`) —
#    never the master token. Absent file: skipped, not failed. The header
#    reaches curl on stdin (`-H @-`, curl >= 7.55), never on its command
#    line where `ps` would show the token; printf is a builtin.
if [ -f "$TOKEN_FILE" ]; then
  URL="$(sed -n 's/^FLEET_HUB_PUBLIC_URL=//p' "$DIR/fleet-hub.env" 2>/dev/null | head -n1)"
  if [ -n "$URL" ]; then
    BODY='{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"fleet_health","arguments":{}}}'
    ANSWER="$(printf 'Authorization: Bearer %s\n' "$(tr -d '\r\n' <"$TOKEN_FILE")" \
      | curl -sS --max-time 15 -X POST "$URL/mcp/json" -H @- \
      -H 'Content-Type: application/json' -H 'Accept: application/json' \
      --data "$BODY" || true)"
    case "$ANSWER" in
      *"\\\"version\\\":\\\"$NEW\\\""*|*"\"version\":\"$NEW\""*) echo "upgrade: fleet_health.version = $NEW via $URL" ;;
      *) echo "upgrade: WARNING fleet_health over $URL did not report version $NEW (a cache in front? DNS?): ${ANSWER:0:200}" >&2 ;;
    esac
  fi
fi
echo "upgrade: done ${OLD:-<unset>} -> $NEW; pre-upgrade backups kept: $(ls -1 "$DIR"/backups/pre-*.db 2>/dev/null | wc -l | tr -d ' ')"
