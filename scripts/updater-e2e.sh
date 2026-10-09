#!/usr/bin/env bash
# fleet-updater end to end against a real Docker daemon (update-channel
# design §12, S6's acceptance). Opt-in, like --hub-e2e:
#
#   scripts/updater-e2e.sh            # or: scripts/ci-local.sh --updater-e2e
#
# It starts containers named updater-e2e-* and a registry on 127.0.0.1:5055,
# and removes them when it ends.
#
# A local registry holds four images of a stand-in hub (a shell script that
# answers `serve`, `healthcheck --ready --json` and `backup --json` the way
# fleet-hub does, and refuses a database newer than it knows, like the real
# downgrade guard):
#
#   0.5.3        good, schema 140 — what runs at the start of every scenario
#   0.5.4 good   good, migrates to 141
#   0.5.4 crash  exits on start
#   0.5.4 unready migrates to 141, then never reports ready
#
# A stand-in hub API (python, below) answers /update/check with a decision
# whose channel and manifest are signed by a throwaway minisign key, which
# an `e2e` build of fleet-updater trusts through FLEET_UPDATE_E2E_KEYS; it
# records /update/report. Each scenario runs `fleet-updater once` and checks
# what runs, the database, and the reports:
#
#   good    → 0.5.4 runs, schema 141, reports end in success
#   crash   → 0.5.3 runs again, schema 140, recovered, data_restored false
#   unready → 0.5.3 runs again, the backup is restored (140), the migrated
#             database is kept aside, recovered, data_restored true
#
# Needs docker (a daemon it may start containers on), minisign, python3,
# cargo. The registry image doubles as the stand-in hub's base (busybox sh),
# so the run pulls one image only.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
for c in docker minisign python3 cargo; do
  command -v "$c" >/dev/null || { echo "updater-e2e: $c not found" >&2; exit 1; }
done
docker info >/dev/null 2>&1 || { echo "updater-e2e: no Docker daemon" >&2; exit 1; }

PORT="${UPDATER_E2E_REGISTRY_PORT:-5055}"
HUB_PORT="${UPDATER_E2E_HUB_PORT:-5056}"
REG="localhost:$PORT"
REPO_IMG="$REG/fleet-hub-e2e"
BASE="${UPDATER_E2E_BASE:-registry:2}"
T="$(mktemp -d "${TMPDIR:-/tmp}/updater-e2e.XXXXXX")"
HUB_PID=""

cleanup() {
  docker rm -f updater-e2e-hub updater-e2e-registry >/dev/null 2>&1 || true
  if [ -n "$HUB_PID" ]; then kill "$HUB_PID" 2>/dev/null || true; fi
  # Files the containers wrote are root's; let a container remove them.
  docker run --rm -v "$T:/t" --entrypoint /bin/sh "$BASE" -c 'rm -rf /t/*' >/dev/null 2>&1 || true
  rm -rf "$T"
}
trap cleanup EXIT

say() { echo "updater-e2e: $*"; }
fail() { echo "updater-e2e: FAIL: $*" >&2; exit 1; }

# ── the updater, trusting the throwaway key ──
say "building fleet-updater (e2e)"
# Its own target dir, like hub-e2e's e2e hub: the `e2e` feature set never
# replaces the plain build in target/debug.
TARGET="${CARGO_TARGET_DIR:-$REPO/target}/e2e"
(cd "$REPO" && cargo build -q -p fleet-updater --features e2e --locked --target-dir "$TARGET")
UPDATER="$TARGET/debug/fleet-updater"

minisign -G -W -f -p "$T/e2e.pub" -s "$T/e2e.key" >/dev/null
E2E_KEY="$(sed -n 2p "$T/e2e.pub")"

# ── registry and images ──
docker image inspect "$BASE" >/dev/null 2>&1 || docker pull -q "$BASE" >/dev/null
docker rm -f updater-e2e-registry >/dev/null 2>&1 || true
docker run -d --name updater-e2e-registry -p "127.0.0.1:$PORT:5000" "$BASE" >/dev/null
for _ in $(seq 1 30); do
  python3 -c "import urllib.request; urllib.request.urlopen('http://$REG/v2/')" 2>/dev/null && break
  sleep 1
done

mkdir -p "$T/img"
cat >"$T/img/fleet-hub" <<'SH'
#!/bin/sh
# A stand-in fleet-hub: the three subcommands the updater drives.
D="${FLEET_HUB_DATA_DIR:-/var/lib/fleet-hub}"
case "$1" in
  serve)
    db="$(cat "$D/state.db" 2>/dev/null || echo 0)"
    if [ "$db" -gt "$SCHEMA" ]; then
      echo "database schema $db is newer than this build ($SCHEMA); refusing" >&2; exit 1
    fi
    if [ "$BEHAVIOUR" = crash ]; then echo "crash on start" >&2; exit 1; fi
    echo "$SCHEMA" >"$D/state.db"
    if [ "$BEHAVIOUR" = unready ]; then echo false >/tmp/ready; else echo true >/tmp/ready; fi
    trap 'rm -f /tmp/ready; exit 0' TERM INT
    while :; do sleep 1 & wait $!; done
    ;;
  healthcheck)
    [ "$2" = "--ready" ] || exit 0
    r="$(cat /tmp/ready 2>/dev/null || echo false)"
    printf '{"ready":%s,"live":true,"fresh":true,"hub":{"ready":%s,"pid":1,"version":"%s","commit":"c%s","build_id":"e2e","contract":1,"agent_proto":[1,1],"peer_proto":1,"schema":%s,"started_at":0,"heartbeat_at":0,"checks":{"store":"ok","listener":"ok","first_reconcile":"%s","reconcile_failures":0}}}\n' \
      "$r" "$r" "$VERSION" "$VERSION" "$(cat "$D/state.db")" "$( [ "$r" = true ] && echo ok || echo failed)"
    [ "$r" = true ]
    ;;
  backup)
    mkdir -p "$D/backups"
    f="$D/backups/$3-$(date +%s)-$$.db"
    cp "$D/state.db" "$f"
    printf '{"path":"%s","schema":%s,"bytes":%s}\n' "$f" "$(cat "$f")" "$(wc -c <"$f" | tr -d ' ')"
    ;;
  *) echo "fleet-hub (e2e stand-in) $VERSION"; exit 0 ;;
esac
SH
chmod +x "$T/img/fleet-hub"
cat >"$T/img/Dockerfile" <<EOF
FROM $BASE
COPY fleet-hub /usr/local/bin/fleet-hub
ARG VERSION
ARG SCHEMA
ARG BEHAVIOUR
ENV VERSION=\${VERSION} SCHEMA=\${SCHEMA} BEHAVIOUR=\${BEHAVIOUR} FLEET_HUB_DATA_DIR=/var/lib/fleet-hub
ENTRYPOINT ["/usr/local/bin/fleet-hub"]
CMD ["serve"]
EOF

declare -A DIGEST
build() { # name version schema behaviour
  local tag="$REPO_IMG:$1"
  docker build -q -t "$tag" --build-arg VERSION="$2" --build-arg SCHEMA="$3" --build-arg BEHAVIOUR="$4" "$T/img" >/dev/null
  docker push -q "$tag" >/dev/null
  DIGEST[$1]="$(docker image inspect --format '{{range .RepoDigests}}{{println .}}{{end}}' "$tag" | grep "^$REPO_IMG@" | head -n1 | cut -d@ -f2)"
  [ -n "${DIGEST[$1]}" ] || fail "no digest for $tag"
  # Forget the local copy: the updater must pull it.
  [ "$1" = old ] || docker rmi -f "$tag" >/dev/null
}
say "building the stand-in images"
build old 0.5.3 140 good
build good 0.5.4 141 good
build crash 0.5.4 140 crash
build unready 0.5.4 141 unready

# ── the stand-in hub API ──
cat >"$T/hub.py" <<'PY'
import http.server, json, os, sys
T, PORT, TOKEN = sys.argv[1], int(sys.argv[2]), sys.argv[3]
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        if self.path == "/pair":
            # Outside the token and the Host allowlist, like the hub's: the
            # code is the credential. It works once.
            ok = json.loads(body).get("code") == "E2ECODE" and not os.path.exists(os.path.join(T, "paired"))
            out = json.dumps({"token": TOKEN, "name": "updater", "mode": "updater"} if ok else {"error": "invalid code"}).encode()
            if ok:
                open(os.path.join(T, "paired"), "w").close()
            self.send_response(200 if ok else 403)
            self.send_header("Content-Length", str(len(out)))
            self.end_headers()
            self.wfile.write(out)
            return
        # A public hub refuses a Host outside its allowlist (403), and
        # `fleet-hub:4180` is not on it: the updater must ask under the
        # public URL's host.
        if self.headers.get("Host") != "fleet.e2e.test":
            self.send_response(403); self.end_headers(); return
        if self.headers.get("Authorization") != "Bearer " + TOKEN:
            self.send_response(401); self.end_headers(); return
        if self.path == "/update/check":
            out = open(os.path.join(T, "decision.json"), "rb").read()
        elif self.path == "/update/report":
            with open(os.path.join(T, "reports.jsonl"), "ab") as f:
                f.write(body + b"\n")
            out = b'{"recorded":true}'
        else:
            self.send_response(404); self.end_headers(); return
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)
http.server.HTTPServer(("127.0.0.1", PORT), H).serve_forever()
PY
python3 "$T/hub.py" "$T" "$HUB_PORT" e2e-token &
HUB_PID=$!
for _ in $(seq 1 30); do
  python3 -c "import socket; socket.create_connection(('127.0.0.1', $HUB_PORT), 1)" 2>/dev/null && break
  sleep 0.2
done

# The token the way an operator gets it: `fleet-hub pair --mode updater`
# prints a URL, `fleet-updater pair` redeems it into the state volume.
say "pairing"
FLEET_UPDATER_HUB_URL="http://127.0.0.1:$HUB_PORT" FLEET_UPDATER_STATE_DIR="$T/paired-state" \
  "$UPDATER" pair "https://fleet.e2e.test/pair#E2ECODE" || fail "pair did not redeem the code"
[ "$(cat "$T/paired-state/token")" = e2e-token ] || fail "pair did not keep the token"
[ "$(stat -c %a "$T/paired-state/token")" = 600 ] || fail "the token file is not 0600"
if FLEET_UPDATER_HUB_URL="http://127.0.0.1:$HUB_PORT" FLEET_UPDATER_STATE_DIR="$T/paired-state" \
  "$UPDATER" pair E2ECODE 2>/dev/null; then
  fail "a spent code paired again"
fi

# A signed channel + manifest for `variant` as 0.5.4, and the decision the
# hub would serve for it (mode automatic).
decide() { # variant schema_to
  local d="${DIGEST[$1]}"
  python3 - "$T" "$REPO_IMG" "$d" "$2" <<'PY'
import json, sys, time
T, image, digest, schema_to = sys.argv[1:]
schema_to = int(schema_to)
now = int(time.time())
iso = lambda t: time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(t))
art = {"kind": "oci", "image": image, "digest": digest, "platforms": {}}
manifest = {
  "schema": 1,
  "release": {"version": "0.5.4", "track": "stable", "commit": "c0.5.4", "build_id": "e2e",
              "published_at": iso(now), "assets_base": "https://example.invalid/"},
  "compatibility": {"contract": {"hub_serves": 1, "desktop_accepts": [1, 1]},
                    "agent_proto": {"hub_accepts": [1, 1], "agent_speaks": 1},
                    "store": {"schema_to": schema_to, "opens_down_to": schema_to}, "update_proto": 1},
  "components": {"hub": {"version": "0.5.4", "artifacts": [art]}},
}
open(f"{T}/manifest.json", "w").write(json.dumps(manifest))
PY
  minisign -S -s "$T/e2e.key" -m "$T/manifest.json" </dev/null >/dev/null
  python3 - "$T" <<'PY'
import hashlib, json, sys, time
T = sys.argv[1]
now = int(time.time())
iso = lambda t: time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(t))
m = open(f"{T}/manifest.json", "rb").read()
sha = hashlib.sha256(m).hexdigest()
seq = now  # strictly increasing across scenarios
channel = {"schema": 1, "track": "stable", "sequence": seq, "generated_at": iso(now),
           "expires_at": iso(now + 86400), "current": "0.5.4", "recommended": "0.5.4",
           "releases": [{"version": "0.5.4", "manifest": "https://example.invalid/m.json", "manifest_sha256": sha}]}
open(f"{T}/channel.json", "w").write(json.dumps(channel))
PY
  minisign -S -s "$T/e2e.key" -m "$T/channel.json" </dev/null >/dev/null
  python3 - "$T" <<'PY'
import hashlib, json, sys
T = sys.argv[1]
m = open(f"{T}/manifest.json").read()
c = open(f"{T}/channel.json").read()
art = json.loads(m)["components"]["hub"]["artifacts"][0]
decision = {
  "update_proto": 1, "component": "hub", "status": "update_available", "source": "hub",
  "track": "stable", "mode": "automatic", "installed": "0.5.3",
  "target": {"version": "0.5.4", "mandatory": False, "deadline": None,
             "manifest": {"url": "https://example.invalid/m.json", "sha256": hashlib.sha256(m.encode()).hexdigest()},
             "channel": {"sequence": json.loads(c)["sequence"]}, "artifact": art, "url": None,
             "evidence": {"channel": c, "channel_sig": open(f"{T}/channel.json.minisig").read(),
                          "manifest": m, "manifest_sig": open(f"{T}/manifest.json.minisig").read()}},
  "reason": {"code": "newer_recommended", "text": "0.5.4 is recommended for this hub."},
  "next_check_secs": 3600,
}
open(f"{T}/decision.json", "w").write(json.dumps(decision))
PY
}

image_id() { docker inspect --format '{{.Image}}' updater-e2e-hub; }
running() { [ "$(docker inspect --format '{{.State.Running}}' updater-e2e-hub)" = true ]; }
db() { cat "$T/data/state.db"; }
last_report() { tail -n1 "$T/reports.jsonl"; }

scenario() { # variant schema_to expected-exit
  local v="$1" schema_to="$2" want_rc="$3"
  say "── scenario: $v"
  docker rm -f updater-e2e-hub >/dev/null 2>&1 || true
  docker run --rm -v "$T:/t" --entrypoint /bin/sh "$BASE" -c 'rm -rf /t/data /t/state /t/reports.jsonl' >/dev/null
  mkdir -p "$T/data" "$T/state"
  echo 140 >"$T/data/state.db"
  docker run -d --name updater-e2e-hub --restart unless-stopped \
    -e FLEET_HUB_BIND=0.0.0.0 -v "$T/data:/var/lib/fleet-hub" "$REPO_IMG@${DIGEST[old]}" >/dev/null
  OLD_ID="$(image_id)"
  decide "$v" "$schema_to"
  set +e
  DOCKER_HOST=unix:///var/run/docker.sock \
  FLEET_UPDATE_E2E_KEYS="$E2E_KEY" \
  FLEET_UPDATER_HUB_URL="http://127.0.0.1:$HUB_PORT" \
  FLEET_UPDATER_TOKEN_FILE="$T/paired-state/token" \
  FLEET_HUB_PUBLIC_URL=https://fleet.e2e.test \
  FLEET_UPDATER_HUB_CONTAINER=updater-e2e-hub \
  FLEET_UPDATER_HUB_DATA="$T/data" \
  FLEET_UPDATER_STATE_DIR="$T/state" \
  FLEET_UPDATER_READY_TIMEOUT_SECS=20 \
  FLEET_UPDATER_SOAK_SECS=4 \
    "$UPDATER" once >"$T/$v.log" 2>&1
  local rc=$?
  set -e
  sed 's/^/    /' "$T/$v.log"
  [ "$rc" = "$want_rc" ] || fail "$v: fleet-updater once exited $rc, expected $want_rc"
  running || fail "$v: the hub container is not running"
}

scenario good 141 0
[ "$(image_id)" != "$OLD_ID" ] || fail "good: still the old image"
[ "$(db)" = 141 ] || fail "good: schema $(db), expected 141"
last_report | grep -q '"phase":"success"' || fail "good: last report is not success: $(last_report)"
docker inspect --format '{{range .Config.Env}}{{println .}}{{end}}' updater-e2e-hub | grep -qx 'FLEET_HUB_BIND=0.0.0.0' \
  || fail "good: the container lost its own environment"
[ "$(docker inspect --format '{{.HostConfig.RestartPolicy.Name}}' updater-e2e-hub)" = unless-stopped ] \
  || fail "good: the container lost its restart policy"
ls "$T/data/backups"/pre-0.5.4-*.db >/dev/null || fail "good: no pre-update backup"
say "good: ok"

scenario crash 140 1
[ "$(image_id)" = "$OLD_ID" ] || fail "crash: not back on the old image"
[ "$(db)" = 140 ] || fail "crash: schema $(db), expected 140"
last_report | grep -q '"phase":"recovered"' || fail "crash: last report is not recovered: $(last_report)"
last_report | grep -q '"data_restored":false' || fail "crash: data was restored without need"
ls "$T/state"/failed-0.5.4-*.log >/dev/null || fail "crash: the failed hub's log was not kept"
say "crash: ok"

scenario unready 141 1
[ "$(image_id)" = "$OLD_ID" ] || fail "unready: not back on the old image"
[ "$(db)" = 140 ] || fail "unready: schema $(db), expected the restored 140"
last_report | grep -q '"phase":"recovered"' || fail "unready: last report is not recovered: $(last_report)"
last_report | grep -q '"data_restored":true' || fail "unready: the backup was not restored"
[ "$(cat "$T"/data/failed-0.5.4-*/state.db)" = 141 ] || fail "unready: the migrated database was not kept aside"
say "unready: ok"

say "all scenarios passed"
