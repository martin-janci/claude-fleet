#!/usr/bin/env bash
# deploy/hub/upgrade.sh <version> — upgrade a compose deployment whose image
# tag lives in .env (deploy/hub/behind-proxy/docker-compose.yml):
#
#   pull → backup.sh (online, consistent) → compose stop → FLEET_HUB_TAG=<v>
#   → up -d → the image's own healthcheck → `fleet-hub --version`
#   → fleet_health.version over the public URL, only with a READONLY client
#     token kept beside the compose file → keep 3 pre-upgrade backups.
#
# FLEET_HUB_DIR is where docker-compose.yml, .env, fleet-hub.env, ./data and
# backup.sh live. Runs as root (the docker socket). The rollback is printed
# on every failure after the stop. The tag has NO v: `v0.3.1` is `0.3.1`.
set -euo pipefail

NEW="${1:?usage: upgrade.sh <version>   (the image tag, e.g. 0.3.1 — no v)}"
DIR="${FLEET_HUB_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)}"
ENV_FILE="${FLEET_HUB_ENV_FILE:-$DIR/.env}"
IMAGE="${FLEET_HUB_IMAGE:-ghcr.io/martin-janci/fleet-hub}"
KEEP="${KEEP:-3}"
TOKEN_FILE="${FLEET_HUB_READONLY_TOKEN_FILE:-$DIR/readonly.token}"
HEALTH_TRIES="${HEALTH_TRIES:-60}"

case "$NEW" in v*) echo "upgrade: the image tag has no v prefix (got $NEW; the tag for v0.3.1 is 0.3.1)" >&2; exit 2;; esac
cd "$DIR"
[ -f "$ENV_FILE" ] || { echo "upgrade: no $ENV_FILE — FLEET_HUB_TAG lives there (copy .env.example)" >&2; exit 2; }
[ -f "$DIR/backup.sh" ] || { echo "upgrade: no $DIR/backup.sh beside this script" >&2; exit 2; }
OLD="$(sed -n 's/^FLEET_HUB_TAG=//p' "$ENV_FILE" | head -n1)"
rollback() {
  echo "upgrade: ROLLBACK: set FLEET_HUB_TAG=${OLD:-<previous>} in $ENV_FILE, then: docker compose up -d fleet-hub" >&2
  echo "upgrade: if $NEW migrated the database, first restore the newest backups/pre-$NEW-*.db (docs/hub.md → Backups → Restore drill)" >&2
}
echo "upgrade: ${OLD:-<unset>} -> $NEW in $DIR"

# 1. Pull first: a tag ghcr does not have stops here, with the hub untouched.
docker pull "$IMAGE:$NEW"
# 2. A consistent copy of the database while the hub is still serving.
PREFIX="pre-$NEW" KEEP="$KEEP" FLEET_HUB_DATA="${FLEET_HUB_DATA:-$DIR/data}" FLEET_HUB_BACKUPS="$DIR/backups" bash "$DIR/backup.sh"
# 3. SIGTERM under the compose stop_grace_period (drain + tick shutdown); `stop`, not `down`.
docker compose stop fleet-hub
# 4. Move the pin. `-i.bak` is the spelling GNU and BSD sed both accept.
if grep -q '^FLEET_HUB_TAG=' "$ENV_FILE"; then
  sed -i.bak "s|^FLEET_HUB_TAG=.*|FLEET_HUB_TAG=$NEW|" "$ENV_FILE" && rm -f "$ENV_FILE.bak"
else
  printf 'FLEET_HUB_TAG=%s\n' "$NEW" >>"$ENV_FILE"
fi
# 5. Start on the new tag and wait for the image's own healthcheck.
docker compose up -d fleet-hub || { rollback; exit 1; }
i=0
until docker compose exec -T fleet-hub fleet-hub healthcheck >/dev/null 2>&1; do
  i=$((i + 1))
  if [ "$i" -ge "$HEALTH_TRIES" ]; then
    echo "upgrade: the hub did not pass its healthcheck within $HEALTH_TRIES s (docker compose logs fleet-hub)" >&2
    rollback; exit 1
  fi
  sleep 1
done
# 6. The binary names its version without a credential.
GOT="$(docker compose exec -T fleet-hub fleet-hub --version | tr -d '\r')"
case "$GOT" in
  *" $NEW") echo "upgrade: running $GOT" ;;
  *) echo "upgrade: running '$GOT', expected 'fleet-hub $NEW' (the pin did not take?)" >&2; rollback; exit 1 ;;
esac
# 7. Over the network, with a READONLY client token when one is kept beside
#    the compose file (`fleet-hub pair --mode readonly upgrade-check`) —
#    never the master token. Absent file: skipped, not failed.
if [ -f "$TOKEN_FILE" ]; then
  URL="$(sed -n 's/^FLEET_HUB_PUBLIC_URL=//p' "$DIR/fleet-hub.env" 2>/dev/null | head -n1)"
  if [ -n "$URL" ]; then
    BODY='{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"fleet_health","arguments":{}}}'
    ANSWER="$(curl -sS --max-time 15 -X POST "$URL/mcp/json" \
      -H "Authorization: Bearer $(cat "$TOKEN_FILE")" \
      -H 'Content-Type: application/json' -H 'Accept: application/json' \
      --data "$BODY" || true)"
    case "$ANSWER" in
      *"\\\"version\\\":\\\"$NEW\\\""*|*"\"version\":\"$NEW\""*) echo "upgrade: fleet_health.version = $NEW via $URL" ;;
      *) echo "upgrade: WARNING fleet_health over $URL did not report version $NEW (a cache in front? DNS?): ${ANSWER:0:200}" >&2 ;;
    esac
  fi
fi
echo "upgrade: done ${OLD:-<unset>} -> $NEW; pre-upgrade backups kept: $(ls -1 "$DIR"/backups/pre-*.db 2>/dev/null | wc -l | tr -d ' ')"
