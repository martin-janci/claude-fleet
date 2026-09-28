# Hub Ops & Accounting Implementation Plan

**Status:** landed (#344). Its migration shipped as **071**, not the 062 below; `upgrade.sh` no longer takes `FLEET_HUB_IMAGE` (the image is the compose file's).

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (or superpowers:executing-plans) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the live hub (`fleet.rlt.sk` 0.3.1 on the NAS) a real backup/upgrade path, a safer ingress, tick-level observability, an event stream that resumes instead of re-listing, a bounded startup, usage-by-day figures that mean what they say, and a reconcile pass that no longer waits for its slowest host.

**Architecture:** Deployment scripts and a "behind an existing proxy" compose variant live under `deploy/hub/` next to the shipped compose (whose `image:` pin `release.sh` / `check-version-consistency.sh` own, so the variant reads its tag from `.env` instead). Hub-side changes stay in `fleet-core` (`mcp/`, `service/`, `store/`) and `fleet-hub/src/config.rs`; every new wire field on `Health` / `DayUsage` is additive with `#[serde(default)]` and re-golden'd. Desktop-side changes are confined to `src-tauri/src/backend/{events,token_store}.rs`, `lib.rs` and `src/lib/hub_connection.ts`.

**Tech Stack:** Rust (axum 0.8, tokio, rusqlite, tracing), bash (POSIX awk in the usage reader), Svelte 5 + Vitest, docker compose.

**Spec:** `docs/ux/2026-09-27-live-instance-analysis/README.md` themes T5/T6 and code-table rows 6, 13, 14, 19, 20, 21, 24; evidence in `hub-ops.md` (F1, F2, F3, F5, F6, F7, F8, F9) and `perf-logs.md` (§1, §2 residual, §3, §4, §5, §6). Line numbers below are from `origin/main @ 7dad1665` (worktree `$W`).

## Global Constraints

Copied from `CLAUDE.md` (Conventions, Build & test, generated files) plus the repo gotchas from the plan brief:

- Backend errors flow as `IpcError` (`ipc_error.rs`) with `E_*` codes; the frontend unwraps a `Result` type (`src/lib/result.ts`).
- Shell-quoting has **one** canonical implementation: `crate::shell::quote` (alias `shq`) in `crates/fleet-core/src/shell.rs`. Every value interpolated into an SSH/bash command string MUST be quoted with it. The former duplicate copies (`shell_quote`/`shell_quote_str`/`shell_escape`) were consolidated — do not reintroduce them.
- SQLite access goes through `Store` behind a `std::sync::Mutex`. Never hold the guard across an `.await`.
- No blocking I/O under `Mutex<PtyState>` and none on a sync Tauri command (a sync command runs on the macOS main thread).
- `docs/control-api-reference.md` is generated from the MCP tool router. After editing any `#[tool(...)]` description or the `generate_handler!` list, regenerate it or CI fails: `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`.
- `src/lib/hub_verdicts.generated.json` and the refusal table in `docs/hub.md` are generated from `src-tauri/src/backend/verdicts.rs`: `REGEN_HUB_VERDICTS=1 cargo test -p claude-fleet --lib verdict_gen`.
- A new wire field on a struct that crosses hub↔desktop needs `#[serde(default)]` (an old hub/desktop otherwise breaks) and fails the hub contract golden test: `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract` (the regen run reports FAILED once; re-run to see green). Bump `CONTRACT_REVISION` ONLY if a call shape changes; nothing in this plan changes one, so it stays at 4 and `MIN_HUB_CONTRACT`/`MAX_HUB_CONTRACT` do not move.
- Any new Tauri command or MCP tool (or edited `#[tool(...)]` description): row in `src-tauri/src/backend/verdicts.rs` + `route`/`refuse_local_only` by command name in `backend/tests_routing.rs`, then the two REGEN steps above. No new command or tool is added by this plan; two descriptions are edited (Tasks 3 and 6). A test (`the_served_definition_budget_stays_bounded`, `crates/fleet-core/src/mcp/tools/tests.rs:3123`) caps the served tool description budget — keep each addition to one slim clause.
- New `work`/`work_link`/`work_admin` actions need an isolation-matrix row — none are added here.
- Hub-routed command args: the whole args struct is serialized; report types need `Deserialize` and no serde defaults except where noted above. `Health` (deserialized by the desktop's `fleet_health` route) gains fields with `#[serde(default)]` — the one sanctioned exception.
- Migrations are `NNN_<topic>.sql` in `crates/fleet-core/migrations/` registered in `MIGRATIONS` in `crates/fleet-core/src/store/schema.rs`; the next free number on main is **061**, which the session-state-machine plan takes for `061_stale_working.sql`; this plan uses **062**.
- Status vocabulary lives in the enums in `service/pane_intel.rs` — untouched here.
- Frontend: Svelte 5 runes stores in `src/lib/*.ts`; tests are Vitest (`npx vitest run <file>`), type-check `npx svelte-check`; `pnpm test`/`pnpm check` do not work on this Mac (use npx).
- Tests: `cargo test -p fleet-core <name>`; the hub e2e is `scripts/hub-e2e.sh` (needs Homebrew bash first on PATH). `cargo` on this Mac is a zsh function pointing at `/Volumes/CargoSD/target`; export `CARGO_TARGET_DIR` yourself when running from a worktree, and run `cargo fmt --all --check` + `cargo clippy --workspace --all-targets -- -D warnings` before every commit.
- The live fleet: hub fleet.rlt.sk 0.3.1 on the NAS (docker compose in `/volume1/docker/fleet-hub`, sudo needed), desktop 0.2.42 in hub-client mode, fleet-agent 0.2.26 on claude-fleet-trn. Nothing in this plan touches the NAS; the operator runbooks are documentation.

---

### Task 1: Backup and upgrade scripts, the `.env`-pinned compose variant, and the operator runbooks (hub-ops F1, F2, F6 runbook, F7, F9; row 13) — effort M

**Files:**
- Create: `deploy/hub/backup.sh`
- Create: `deploy/hub/upgrade.sh`
- Create: `deploy/hub/behind-proxy/docker-compose.yml`
- Create: `deploy/hub/behind-proxy/.env.example`
- Create: `scripts/hub-deploy-scripts-test.sh`
- Modify: `scripts/ci-local.sh` (after `step release_assets_smoke`, currently line ~190)
- Modify: `docs/hub.md` — new `## Backups` after the *Upgrade and rollback* section (which ends at line 275), a `### Upgrade with the script` paragraph inside *Upgrade and rollback* (after the `**Roll back.**` block, line ~250), and a new `## Behind an existing reverse proxy` section before `## Add and provision hosts` (line 277); `### When a host's SSH key changes` + `### Rotate the hub's SSH key` under *Troubleshooting* (line 2207)
- Test: `scripts/hub-deploy-scripts-test.sh` (bash + sqlite3, fake `docker`/`curl` on PATH; no bats in the repo)

**Interfaces:**
- Consumes: `sqlite3` (`.backup`, `PRAGMA integrity_check`), `docker compose {stop,up,exec}`, `docker pull`, the image's `fleet-hub healthcheck` and `fleet-hub --version` (`crates/fleet-hub/Dockerfile:32`), `/mcp/json` `tools/call fleet_health` with a readonly client token.
- Produces: `backups/<PREFIX>-<UTC stamp>.db` files (env: `FLEET_HUB_DATA`, `FLEET_HUB_BACKUPS`, `PREFIX`, `KEEP`, `SQLITE`, `FLEET_HUB_OWNER`); exit 0 verified, 1 integrity failure, 2 bad input. `upgrade.sh <version>` (env: `FLEET_HUB_DIR`, `FLEET_HUB_ENV_FILE`, `FLEET_HUB_IMAGE`, `KEEP`, `FLEET_HUB_READONLY_TOKEN_FILE`, `HEALTH_TRIES`) rewrites `FLEET_HUB_TAG=` in `.env`.

Why a *variant* compose and not `${FLEET_HUB_TAG}` in `deploy/hub/docker-compose.yml`: `scripts/check-version-consistency.sh:244` and `scripts/release.sh:223-245` both require a literal `image: ghcr.io/<owner>/fleet-hub:<tag>` line in that file (`release.sh --list-image-pin`), and `release.sh` rewrites it on every release. The variant is the NAS shape (bind-mounted `./data`, no bundled caddy) and is not a version carrier: its tag comes from `.env` and `compose config` refuses to start without one.

- [ ] **Step 1: Write the failing shell test**

Create `scripts/hub-deploy-scripts-test.sh`:

```bash
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
# Records every call. `pull` of a `:missing` tag fails like ghcr does;
# `compose stop` refuses when no pre-upgrade backup exists yet (that is the
# order under test); `exec … healthcheck` is healthy; `exec … --version`
# answers whatever tag .env pins.
echo "docker $*" >>"$FAKE_LOG"
case "$1 $2" in
  "pull "*) case "$2" in *:missing) echo "manifest unknown" >&2; exit 1;; esac ;;
  "compose stop") ls "$FAKE_DIR"/backups/pre-*.db >/dev/null 2>&1 || { echo "stop before backup" >&2; exit 9; } ;;
  "compose exec")
    case "${@: -1}" in
      healthcheck) exit 0 ;;
      --version) echo "fleet-hub $(sed -n 's/^FLEET_HUB_TAG=//p' "$FAKE_DIR/.env")" ;;
    esac ;;
esac
exit 0
EOF
cat >"$FAKE/curl" <<'EOF'
#!/usr/bin/env bash
echo "curl $*" >>"$FAKE_LOG"
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
check "pull precedes stop" test "$(ord 'docker pull')" -lt "$(ord 'docker compose stop')"
check "stop precedes up" test "$(ord 'docker compose stop')" -lt "$(ord 'docker compose up')"
check "up precedes the healthcheck" test "$(ord 'docker compose up')" -lt "$(ord 'healthcheck')"
check "the healthcheck precedes --version" test "$(ord 'healthcheck')" -lt "$(ord '--version')"
check "fleet_health is asked with the readonly token" grep -q 'Bearer cl_readonly' "$FAKE_LOG"
check "…against /mcp/json on the public URL" grep -q 'https://fleet.example.com/mcp/json' "$FAKE_LOG"
check "…and the version is confirmed" grep -q 'fleet_health.version = 0.3.1' "$ROOT/upgrade1.log"

: >"$FAKE_LOG"
PATH="$FAKE:$PATH" FLEET_HUB_DIR="$U" bash "$U/upgrade.sh" missing >"$ROOT/upgrade2.log" 2>&1
check "a tag ghcr does not have fails" test $? != 0
check "…before the pin moved" grep -qx 'FLEET_HUB_TAG=0.3.1' "$U/.env"
check "…and before the hub was stopped" test -z "$(grep 'compose stop' "$FAKE_LOG")"

PATH="$FAKE:$PATH" FLEET_HUB_DIR="$U" bash "$U/upgrade.sh" v0.3.2 >/dev/null 2>&1
check "a v-prefixed tag is refused with exit 2" test $? = 2

rm -f "$U/readonly.token"; : >"$FAKE_LOG"
PATH="$FAKE:$PATH" FLEET_HUB_DIR="$U" bash "$U/upgrade.sh" 0.3.1 >/dev/null 2>&1
check "without a token file the network check is skipped" test -z "$(grep '^curl' "$FAKE_LOG")"

echo "hub-deploy-scripts-test: $PASS passed, $FAIL failed (logs in $ROOT)"
[ "$FAIL" = 0 ]
```

- [ ] **Step 2: Run it, expect the scripts to be missing**

`bash scripts/hub-deploy-scripts-test.sh` → `bash: …/deploy/hub/backup.sh: No such file or directory`, several `FAIL:` lines, exit 1.

- [ ] **Step 3: Write `deploy/hub/backup.sh`**

```bash
#!/usr/bin/env bash
# deploy/hub/backup.sh — a consistent, online backup of the hub's state.db.
#
# `sqlite3 .backup` uses SQLite's online backup API: one self-contained file
# with the WAL folded in, taken while the hub keeps writing. A `cp` of
# state.db from a running hub is NOT that — it misses every page still in
# state.db-wal (the NAS's backup-* dirs were exactly that: state.db minutes
# older than its -wal), and a `cp` of the triple can be torn mid-write.
#
# Run as root from a DSM Task Scheduler entry (nightly) and from upgrade.sh
# (PREFIX=pre-<version> KEEP=3). Retention is per PREFIX, by count.
# Exit codes: 0 backed up and verified; 1 the copy failed (removed); 2 bad input.
set -euo pipefail

DATA="${FLEET_HUB_DATA:-/volume1/docker/fleet-hub/data}"
OUT_DIR="${FLEET_HUB_BACKUPS:-$(dirname "$DATA")/backups}"
PREFIX="${PREFIX:-state}"
KEEP="${KEEP:-14}"
SQLITE="${SQLITE:-sqlite3}"
OWNER="${FLEET_HUB_OWNER:-1000}"
DB="$DATA/state.db"

command -v "$SQLITE" >/dev/null 2>&1 || { echo "backup: $SQLITE not found" >&2; exit 2; }
[ -f "$DB" ] || { echo "backup: no database at $DB" >&2; exit 2; }
case "$KEEP" in ''|*[!0-9]*) echo "backup: KEEP must be a number, got '$KEEP'" >&2; exit 2;; esac
case "$PREFIX" in *[!A-Za-z0-9._-]*|'') echo "backup: PREFIX must be [A-Za-z0-9._-]+, got '$PREFIX'" >&2; exit 2;; esac

mkdir -p "$OUT_DIR"
OUT="$OUT_DIR/$PREFIX-$(date -u +%Y%m%d-%H%M%S).db"
if ! "$SQLITE" "$DB" ".backup '$OUT'"; then
  echo "backup: .backup of $DB failed; removing $OUT" >&2
  rm -f -- "$OUT"
  exit 1
fi
if ! "$SQLITE" "$OUT" 'PRAGMA integrity_check' | grep -qx ok; then
  echo "backup: integrity_check failed on $OUT; removed" >&2
  rm -f -- "$OUT"
  exit 1
fi
# Retention: the newest $KEEP files of this prefix stay. The names carry no
# whitespace (this script wrote them), so word-splitting `ls -1t` is safe.
n=0
for f in $(ls -1t "$OUT_DIR"/"$PREFIX"-*.db 2>/dev/null); do
  n=$((n + 1))
  if [ "$n" -gt "$KEEP" ]; then rm -f -- "$f"; fi
done
if [ "$(id -u)" = 0 ]; then
  chown -R "$OWNER" "$OUT_DIR"
  chmod 750 "$OUT_DIR"
fi
echo "backup: ok $OUT ($(wc -c <"$OUT" | tr -d ' ') bytes; keeping the newest $KEEP $PREFIX-*.db)"
```

`chmod +x deploy/hub/backup.sh`.

- [ ] **Step 4: Write `deploy/hub/upgrade.sh`**

```bash
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
```

`chmod +x deploy/hub/upgrade.sh`.

- [ ] **Step 5: Run the shell test, expect PASS**

`bash scripts/hub-deploy-scripts-test.sh` → `hub-deploy-scripts-test: 24 passed, 0 failed`, exit 0.

- [ ] **Step 6: The compose variant and its `.env.example`**

Create `deploy/hub/behind-proxy/docker-compose.yml`:

```yaml
# fleet-hub behind a reverse proxy you already run (a NAS with its own
# Caddy, say) — the shape docs/hub.md "Behind an existing reverse proxy"
# describes. Differences from ../docker-compose.yml, all deliberate:
#   * no bundled caddy: yours terminates TLS and forwards to this container;
#   * the image tag comes from .env (FLEET_HUB_TAG), so deploy/hub/upgrade.sh
#     can move it, and `docker compose config` refuses to start without one —
#     this file is never a version carrier the way the shipped compose is;
#   * state.db lives in a bind mount (./data) the host's sqlite3 can reach,
#     which is what deploy/hub/backup.sh needs.
# The https:// public URL in fleet-hub.env is what permits the 0.0.0.0 bind
# without --allow-plaintext; the hub still logs a warning at startup that
# plaintext 4180 is reachable by anything that can route to the container.
services:
  fleet-hub:
    image: ghcr.io/martin-janci/fleet-hub:${FLEET_HUB_TAG:?set FLEET_HUB_TAG in .env (deploy/hub/upgrade.sh <version> writes it)}
    restart: unless-stopped
    # DRAIN_TIMEOUT (10s) + TICK_SHUTDOWN_TIMEOUT (10s) < this grace.
    stop_grace_period: 30s
    env_file: fleet-hub.env
    environment:
      FLEET_HUB_BIND: 0.0.0.0
    ports: ["4180:4180"]
    # Docker's stdout capture of the same lines the hub already rotates in
    # its own log file; uncapped it is the file that fills the disk.
    logging:
      driver: json-file
      options:
        max-size: "10m"
        max-file: "3"
    volumes:
      - ./data:/var/lib/fleet-hub
      - ./ssh:/home/fleet/.ssh
```

Create `deploy/hub/behind-proxy/.env.example`:

```bash
# Copied to .env next to docker-compose.yml. The image tag the compose file
# runs; deploy/hub/upgrade.sh <version> rewrites it. No `v`: v0.3.1 -> 0.3.1.
# Left empty on purpose so a fresh copy fails loudly instead of running an
# unknown version — set it, or run upgrade.sh once.
FLEET_HUB_TAG=
```

Check the variant parses and refuses without a tag: `cd deploy/hub/behind-proxy && touch fleet-hub.env && FLEET_HUB_TAG= docker compose config >/dev/null; echo $?` → non-zero with `set FLEET_HUB_TAG in .env`; `FLEET_HUB_TAG=0.3.1 docker compose config | grep image:` → `image: ghcr.io/martin-janci/fleet-hub:0.3.1`. (Skip when docker is absent locally; the CI check below does not need it.)

Confirm the release scripts are untouched: `scripts/check-version-consistency.sh` still passes (it reads only `deploy/hub/docker-compose.yml`).

- [ ] **Step 7: Wire the test into `scripts/ci-local.sh`**

After `step release_assets_smoke` add:

```bash
# The hub deploy scripts (backup.sh / upgrade.sh) against a fake docker:
# bash + sqlite3 only, a few seconds.
step bash scripts/hub-deploy-scripts-test.sh
```

- [ ] **Step 8: Documentation in `docs/hub.md`**

Inside *Upgrade and rollback*, after the `**Roll back.**` block, add:

```markdown
### Upgrade with the script

`deploy/hub/upgrade.sh <version>` does the sequence above for a deployment
whose tag lives in `.env` (the `deploy/hub/behind-proxy` compose): it pulls
first (a tag ghcr does not have stops it with the hub untouched), takes a
consistent online backup (`backup.sh`, kept as `backups/pre-<version>-*.db`,
newest three), `docker compose stop`s the hub so the 30 s grace applies,
moves `FLEET_HUB_TAG`, starts it, waits for the image's own healthcheck,
and checks `fleet-hub --version`. With a readonly client token in
`readonly.token` beside the compose file (`fleet-hub pair --mode readonly
upgrade-check`) it also asks `fleet_health` over the public URL — never the
master token. On any failure after the stop it prints the rollback.

**Order across the three binaries.** Today (contract 4 on both sides,
proto 1 on both sides) the order is a habit: hub, then desktop, then the
agents. When a release bumps `CONTRACT_REVISION`, upgrade the **hub first,
then the desktop in the same window** — there is no mixed window, the desktop
refuses with `E_HUB_CONTRACT` until it is updated, hooks and the phone keep
working meanwhile. When a release bumps `PROTO_VERSION`, upgrade the **hub
first**; the release holds `MIN_SUPPORTED_PROTO` at the previous value so an
older `fleet-agent` keeps connecting until it is reinstalled. A hub upgrade
never needs the agents restarted.
```

After that section (before `## Add and provision hosts`), add:

```markdown
## Backups

`state.db` carries the master token, every host, every session and the
usage roll-ups. The hub keeps it in WAL mode, so a `cp` of the file from a
running hub is a stale database (pages still in `state.db-wal` are missing)
and a `cp` of the `state.db*` triple can be torn. Use SQLite's online backup
API instead — one self-contained file, no stop:

```bash
sudo deploy/hub/backup.sh          # FLEET_HUB_DATA=./data, keeps 14 dailies in ./backups
```

It runs `.backup`, then `PRAGMA integrity_check` on the copy (a failed check
removes it and exits 1), then prunes to `KEEP` files per `PREFIX`. On a
Synology: Control Panel → Task Scheduler → user `root`, daily 03:30,
`bash /volume1/docker/fleet-hub/backup.sh`; add `backups/` to Hyper Backup
or any off-box target. `upgrade.sh` calls the same script with
`PREFIX=pre-<version> KEEP=3` before it stops the hub.

**Restore drill** (rehearse it once; a backup nobody restored is a hope):

```bash
docker compose stop fleet-hub
mkdir -p data.aside && mv data/state.db data/state.db-wal data/state.db-shm data.aside/ 2>/dev/null
cp backups/state-<stamp>.db data/state.db && chown 1000 data/state.db
docker compose up -d fleet-hub
curl -s https://fleet.example.com/mcp/json -H "Authorization: Bearer <readonly token>" ... # fleet_health.schema_version
```

Restoring a copy taken before a migration onto a newer image re-runs the
migrations (fine). Never restore a *newer* copy onto an *older* image: the
hub refuses a database a newer build has migrated (see *Roll back* above).
Sessions that ran between the copy and the restore are not in it.

## Behind an existing reverse proxy

`deploy/hub/behind-proxy/docker-compose.yml` is the shape for a box that
already runs a reverse proxy (a NAS with its own Caddy): no bundled caddy, the
image tag in `.env` (`FLEET_HUB_TAG=…`, moved by `upgrade.sh`), and
`state.db` in a bind mount `./data` the host's `sqlite3` can back up. Set
`FLEET_HUB_PUBLIC_URL=https://…` in `fleet-hub.env` as usual: it is what
permits the `0.0.0.0` bind without `--allow-plaintext`. Files: `.env`,
`fleet-hub.env`, `docker-compose.yml`, `backup.sh`, `upgrade.sh`, `data/`,
`ssh/`, `backups/`.

**Tidying a hand-upgraded deployment.** A directory upgraded by hand tends to
collect `docker-compose.yml.<version>` copies (the `image:` line was the only
difference), `data.pre-<version>` directory copies and `backup-<version>-<ts>`
`cp` triples. With the script in place: keep the live compose and `.env`;
verify each old copy once (`sqlite3 <copy>/state.db 'PRAGMA integrity_check'`
as root — the triples are only valid as the triple), move the ones that pass
into `backups/legacy/`, delete the rest and the `._docker-compose.yml`
AppleDouble sidecar a Mac copy leaves; `chmod 0640 fleet-hub.env`. Nothing in
the repository does this for you — it is the operator's directory.
```

Under *Troubleshooting*, add:

```markdown
### When a host's SSH key changes

A reinstalled host, or a rotated host key, shows up as `Host key verification
failed` → `reachable: false` → one `E_SSH` row in `GET /reports`. The hub's
`known_hosts` is the bind-mounted `./ssh/known_hosts` (uid 1000, so `sudo` on
a NAS): `ssh-keyscan <host> | sudo tee -a ssh/known_hosts`, remove the stale
line for that host, then `probe_host` with the master token. No restart.

### Rotate the hub's SSH key

`fleet-hub ssh-key` never overwrites an existing pair, so rotation is manual:
`ssh-keygen -t ed25519 -f ssh/id_ed25519.new -N ''`; append
`ssh/id_ed25519.new.pub` to `~/.ssh/authorized_keys` on every host; stop the
hub; `mv` the new pair over `ssh/id_ed25519{,.pub}`; start the hub;
`probe_host` every host; then remove the old public key from each host's
`authorized_keys`. Everything under `./ssh` is uid 1000: `sudo` throughout.
```

- [ ] **Step 9: Commit**

```bash
git add deploy/hub/backup.sh deploy/hub/upgrade.sh deploy/hub/behind-proxy/ scripts/hub-deploy-scripts-test.sh scripts/ci-local.sh docs/hub.md
git commit -m "feat(deploy): hub backup and upgrade scripts, the .env-pinned behind-proxy compose, and the backup/restore runbook"
```

---

### Task 2: Ingress — the proxy-network compose, a startup warning for a routable plaintext bind, and a per-address limiter on failed bearers (hub-ops F3; row 14) — effort S

**Files:**
- Modify: `deploy/hub/behind-proxy/docker-compose.yml` (from Task 1: drop `ports:`, add `networks:`)
- Modify: `crates/fleet-hub/src/config.rs` (add `plaintext_exposure_warning` after `resolve`, line ~437; tests in the `mod tests` at the bottom)
- Modify: `crates/fleet-hub/src/serve.rs:717-723` (log the warning after logging is up)
- Modify: `crates/fleet-core/src/mcp/mod.rs` — `AuthState` (162-173), `authorize` (`Err(status)` arm, 259-263), every `AuthState {` literal (lines 460, 716, 862, 1249, 1311, 1424, 1859), the `#[cfg(test)] mod` list near line 22
- Create: `crates/fleet-core/src/mcp/tests_auth_limit.rs`
- Modify: `docs/hub.md` — *Behind an existing reverse proxy* (Task 1's section) and *Security notes* (line 2107)

**Interfaces:**
- Consumes: `fleet_proto::net::is_loopback_ip`, `TlsMode::terminates_tls` (`config.rs:65`), `RateLimiter::check` (`guard.rs:1027`), `pairing::limiter_key` (`pairing.rs:92`), `axum::extract::ConnectInfo<SocketAddr>` (already in the extensions: `mod.rs:780`).
- Produces: `pub fn plaintext_exposure_warning(r: &Resolved) -> Option<String>`; `pub const AUTH_FAIL_INTERVAL: Duration = 1 s`; `AuthState.rate: Arc<RateLimiter>`; a `429` on a repeated bad bearer from one address inside the interval; `peer=` on the `[mcp] rejected request` line.

- [ ] **Step 1: Failing config tests**

Append to `mod tests` in `crates/fleet-hub/src/config.rs`:

```rust
    /// hub-ops F3: an https:// public URL waives the plaintext refusal
    /// because a proxy terminates TLS *somewhere* in front — it does not
    /// prove that proxy is the only route to the port. Say so at startup.
    #[test]
    fn a_routable_plaintext_bind_behind_an_https_proxy_warns() {
        let mut o = routable();
        o.public_url = Some("https://fleet.example.com".into());
        let r = resolve(&o, &env(&[]), &|_| None).unwrap();
        let w = plaintext_exposure_warning(&r).expect("a warning");
        assert!(w.contains("0.0.0.0:4180"), "{w}");
        assert!(w.contains("https://fleet.example.com"), "{w}");
        assert!(w.contains("behind-proxy"), "names the fix: {w}");
    }

    #[test]
    fn a_waived_routable_bind_warns_naming_the_flag() {
        let mut o = routable();
        o.allow_plaintext = true;
        let r = resolve(&o, &env(&[]), &|_| None).unwrap();
        let w = plaintext_exposure_warning(&r).expect("a warning");
        assert!(w.contains("--allow-plaintext"), "{w}");
    }

    #[test]
    fn loopback_and_hub_terminated_tls_do_not_warn() {
        let r = resolve(&opts(), &env(&[]), &|_| None).unwrap();
        assert_eq!(plaintext_exposure_warning(&r), None, "loopback");
        let mut o = cert_opts();
        o.bind = Some("0.0.0.0".into());
        let r = resolve(&o, &env(&[]), &|_| None).unwrap();
        assert_eq!(plaintext_exposure_warning(&r), None, "the hub terminates TLS itself");
    }
```

- [ ] **Step 2: Run, expect a compile error**

`cargo test -p fleet-hub plaintext` → `error[E0425]: cannot find function plaintext_exposure_warning`.

- [ ] **Step 3: Implement the warning and log it**

In `config.rs`, after `resolve` (before `fn dedup`):

```rust
/// The one thing `resolve` cannot turn into a refusal: a routable bind that
/// serves plaintext because an https:// public URL says a proxy terminates
/// TLS *somewhere* in front, or because `--allow-plaintext` waived the
/// check. Nothing proves that proxy is the only path to the port — on a NAS
/// that publishes `4180:4180` so a separate Caddy container can reach it,
/// every LAN and VPN peer can too (hub-ops F3). `None` on a loopback bind
/// or when the hub terminates TLS itself.
pub fn plaintext_exposure_warning(r: &Resolved) -> Option<String> {
    if fleet_proto::net::is_loopback_ip(&r.bind) || r.tls.terminates_tls() {
        return None;
    }
    let why = if r.allow_plaintext {
        "--allow-plaintext waived the refusal".to_string()
    } else {
        format!(
            "the https:// public URL {} only proves a proxy terminates TLS somewhere in front",
            r.public_url.as_deref().unwrap_or("(none)")
        )
    };
    Some(format!(
        "plaintext http on {}:{} is reachable by anything that can route to this address \
         ({why}); front it with the proxy's own container network \
         (deploy/hub/behind-proxy: networks, no ports) or bind to 127.0.0.1",
        r.bind, r.port
    ))
}
```

In `serve.rs`, right after the `match fleet_core::logging::init_in_with(...)` block (line 723):

```rust
    if let Some(warning) = crate::config::plaintext_exposure_warning(&r) {
        tracing::warn!("{warning}");
    }
```

- [ ] **Step 4: Run, expect PASS**

`cargo test -p fleet-hub plaintext` → 3 new tests pass (plus the existing `allow_plaintext_*` ones).

- [ ] **Step 5: Failing limiter test**

Create `crates/fleet-core/src/mcp/tests_auth_limit.rs`:

```rust
//! A bad bearer is answered once per [`AUTH_FAIL_INTERVAL`] per address; the
//! repeat gets 429 (hub-ops F3: failed bearers on `/mcp` were one warn line
//! each, unthrottled, and the line named no peer).

use super::*;
use std::net::Ipv4Addr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn serve_test_app() -> std::net::SocketAddr {
    let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
    let app = test_app(store, "s3cret", crate::agent::ws::AgentWsState::disabled());
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    addr
}

/// The status line of `POST /mcp` with `bearer`, optionally behind a proxy
/// that appended `forwarded` as the client's address.
async fn status_of(addr: std::net::SocketAddr, bearer: &str, forwarded: Option<&str>) -> String {
    let mut req = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {bearer}\r\n\
         Accept: application/json, text/event-stream\r\nContent-Type: application/json\r\n\
         Content-Length: 2\r\nConnection: close\r\n"
    );
    if let Some(ip) = forwarded {
        req.push_str(&format!("X-Forwarded-For: {ip}\r\n"));
    }
    req.push_str("\r\n{}");
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        s.read_to_end(&mut buf),
    )
    .await;
    String::from_utf8_lossy(&buf)
        .lines()
        .next()
        .unwrap_or("")
        .to_string()
}

#[tokio::test]
async fn a_repeated_bad_bearer_from_one_address_is_throttled() {
    let addr = serve_test_app().await;
    assert!(
        status_of(addr, "wrong", None).await.contains("401"),
        "the first failure is a plain 401"
    );
    assert!(
        status_of(addr, "wrong", None).await.contains("429"),
        "the repeat inside AUTH_FAIL_INTERVAL is throttled"
    );
    assert!(
        status_of(addr, "s3cret", None).await.contains("200"),
        "a valid bearer is never throttled — successes do not touch the bucket"
    );
    assert!(
        status_of(addr, "wrong", Some("203.0.113.9")).await.contains("401"),
        "a different client behind the same loopback proxy has its own bucket"
    );
    tokio::time::sleep(AUTH_FAIL_INTERVAL + std::time::Duration::from_millis(50)).await;
    assert!(
        status_of(addr, "wrong", None).await.contains("401"),
        "the bucket refills after the interval"
    );
}
```

Register it next to `mod tests_token_cache;` (`mod.rs:22`): `#[cfg(test)] mod tests_auth_limit;`.

- [ ] **Step 6: Run, expect a compile error**

`cargo test -p fleet-core tests_auth_limit` → `error[E0425]: cannot find value AUTH_FAIL_INTERVAL`.

- [ ] **Step 7: Implement the limiter**

In `mcp/mod.rs`, next to `AuthState`:

```rust
/// A failed bearer from one address is answered once per this interval; a
/// repeat inside it gets 429 and a `debug` line instead of a `warn`, so a
/// guessing loop costs the hub a hash-map probe and cannot flood the log.
/// Keyed like `/pair` (`pairing::limiter_key`): the peer, or the last
/// `X-Forwarded-For` hop when the peer is a trusted front end. Successful
/// requests never touch the bucket.
pub const AUTH_FAIL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
```

Add to `AuthState`:

```rust
    /// The shared limiter (`McpGuards::rate`), keyed `auth:<address>` for
    /// failed bearers. See [`AUTH_FAIL_INTERVAL`].
    rate: Arc<RateLimiter>,
```

Replace the `Err(status) => { … }` arm of `authorize` (lines 259-263):

```rust
        Err(status) => {
            let peer = pairing::limiter_key(
                request
                    .extensions()
                    .get::<axum::extract::ConnectInfo<SocketAddr>>()
                    .map(|c| c.0.ip()),
                request.headers(),
            );
            if status == StatusCode::UNAUTHORIZED
                && state
                    .rate
                    .check(&format!("auth:{peer}"), AUTH_FAIL_INTERVAL)
                    .is_err()
            {
                tracing::debug!(
                    %peer,
                    path = %request.uri().path(),
                    "[mcp] throttled a repeated bad bearer"
                );
                return Err(StatusCode::TOO_MANY_REQUESTS);
            }
            // The path only: the URI / query can carry the legacy `?token=`.
            tracing::warn!(%status, %peer, path = %request.uri().path(), "[mcp] rejected request");
            return Err(status);
        }
```

`SocketAddr` is already imported in `mod.rs` (used at line 780). Add `rate` to every `AuthState {` literal: production (`mod.rs:716`) gets `rate: Arc::clone(&guards.rate),`; `test_app` (460) and the test literals (862, 1249, 1311, 1424, 1859) get `rate: Arc::new(RateLimiter::new()),`.

- [ ] **Step 8: Run, expect PASS**

`cargo test -p fleet-core tests_auth_limit` → `1 passed`; `cargo test -p fleet-core mcp::` still green (the routing test at 862 sends several bad-bearer requests from loopback — they are spaced by real round trips well under 1 s, so verify: if `mcp_and_hook_routes_serve_behind_shared_auth` now sees a 429, set `AUTH_FAIL_INTERVAL`-sized sleeps between its consecutive bad-bearer requests or give that test's `AuthState` a fresh `RateLimiter` per request block — the test's own comment already does the same for `/pair`).

- [ ] **Step 9: The proxy-network compose and the docs**

In `deploy/hub/behind-proxy/docker-compose.yml` replace `ports: ["4180:4180"]` with:

```yaml
    # Reachable only from your proxy's container network: nothing is published
    # on the host, so LAN and VPN peers cannot reach the plaintext port. The
    # image's HEALTHCHECK probes 127.0.0.1:4180 inside the container and is
    # unaffected. FLEET_HUB_PROXY_NETWORK (in .env) names the network your
    # proxy is on — `docker network ls` shows it (caddy_default for a compose
    # project named caddy).
    networks: [proxy]
```

and append at the end of the file:

```yaml
networks:
  proxy:
    external: true
    name: ${FLEET_HUB_PROXY_NETWORK:-caddy_default}
```

Add to `.env.example`: `FLEET_HUB_PROXY_NETWORK=caddy_default` with the comment `# The docker network your reverse proxy is attached to (docker network ls).`

In `docs/hub.md` *Behind an existing reverse proxy* (Task 1) add:

```markdown
The variant publishes **no** port: the hub joins your proxy's docker network
(`FLEET_HUB_PROXY_NETWORK` in `.env`, `caddy_default` for a compose project
named `caddy`) and the proxy forwards by service name:

```
http://fleet.example.com {
    reverse_proxy fleet-hub:4180 {
        flush_interval -1
    }
}
```

The https:// public URL permits the `0.0.0.0` bind, and the hub logs at
startup that plaintext 4180 is reachable by anything that can route to it —
on the proxy network, that is the proxy. Publishing `4180:4180` on the host
instead makes it the whole LAN and every VPN peer; the warning says so.
```

In *Security notes* add one paragraph:

```markdown
**Failed bearers are throttled per address.** A bad or missing token on any
authenticated route is answered `401` once per second per source address
(the peer, or the last `X-Forwarded-For` hop when the peer is a private or
loopback proxy — the same rule `/pair` uses); a repeat inside that second is
`429`. The `[mcp] rejected request` log line names the address. A valid
token is never throttled: successes do not touch the bucket.
```

- [ ] **Step 10: Commit**

```bash
git add deploy/hub/behind-proxy/ crates/fleet-hub/src/config.rs crates/fleet-hub/src/serve.rs crates/fleet-core/src/mcp/mod.rs crates/fleet-core/src/mcp/tests_auth_limit.rs docs/hub.md
git commit -m "feat(hub): warn on a routable plaintext bind, throttle repeated bad bearers per address, and put the behind-proxy compose on the proxy network"
```

---

### Task 3: Observability — tick stats in `fleet_health.hub`, `/metrics` gauges, log levels, `tunnels_mode`, `peer_links_total`, the logs mount (hub-ops F5, F8; perf-logs §1, §5; rows 21, 24) — effort M

**Files:**
- Modify: `crates/fleet-core/src/service/tick.rs` (add `TickStats`, `ReconcileStats`, `tick_stats()`; record around `reconcile_now` at lines 96-102)
- Modify: `crates/fleet-core/src/service/health.rs` (`Health` 27-75, `health_from_store` 396-430, `health_check` 452-475, tests)
- Modify: `crates/fleet-core/src/store/peer_links.rs` (add `peer_links_total` after `peer_links_down`, line 481)
- Modify: `crates/fleet-core/src/mcp/metrics.rs` (`HubGauges`, `MetricsState`, `expose`, `handle_metrics`) and every `MetricsState {` literal: `mcp/mod.rs:450, 747, 881, 1256, 1320, 1461, 1877`, `mcp/tests_token_cache.rs:84`, `mcp/report_route.rs:117`
- Modify: `crates/fleet-core/src/service/usage.rs:694` (debug → warn), `crates/fleet-core/src/service/playbooks.rs:265-283` (INFO on apply), `crates/fleet-core/src/service/sessions/reconcile.rs:941-944` (INFO on a stuck transition)
- Modify: `crates/fleet-core/src/logging.rs` (a `#[cfg(test)] pub(crate) mod capture`)
- Modify: `crates/fleet-core/src/mcp/tools/fleet.rs:9-16` (`fleet_health` description)
- Modify: `deploy/hub/behind-proxy/docker-compose.yml` (logs mount), `deploy/hub/behind-proxy/.env.example`
- Modify: `docs/hub.md` — `/metrics` section (1273-1296), the `fleet_health` `curl` under *Setup*, *Behind an existing reverse proxy*
- Regen: `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract`; `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`

**Interfaces:**
- Consumes: `service::sessions::reconcile_now` (`reconcile.rs:1843`), `health::summarize` (`health.rs:364`), `guard::LongPollLimiter::active_by_key`, `hub::SETTING_PUBLIC_URL` (`service/hub.rs:10`).
- Produces: `tick::TickStats { begin(now) -> Instant, finish(started, now, Result<bool, String>), uptime_secs(), started_at(), reconcile() -> ReconcileStats }`, `tick::tick_stats() -> Arc<TickStats>`; `health::HubHealth { started_at, uptime_secs, reconcile: ReconcileStats }`; `Health.hub: Option<HubHealth>`, `Health.tunnels_mode: Option<String>` (`"none" | "reverse"`), `Health.peer_links_total: u32`, all `#[serde(default)]`; `Store::peer_links_total() -> Result<u32, IpcError>`; `metrics::HubGauges::read(&Mutex<Store>, &TickStats)`; `/metrics` series `fleet_reconcile_duration_ms`, `fleet_reconcile_failures_total`, `fleet_sessions{status}`, `fleet_hosts_reachable`.

`FLEET_HUB_LOG_DIR` already exists on main (`config.rs:415-419`, documented at `docs/hub.md:1583`); the brief's "make the dir configurable" is therefore compose + docs only.

- [ ] **Step 1: Failing `TickStats` test in `tick.rs`**

Append to `mod tests`:

```rust
    #[test]
    fn tick_stats_track_the_last_pass_and_consecutive_failures() {
        let t = TickStats::new(1_000);
        let s = t.begin(1_010);
        t.finish(s, 1_011, Ok(true));
        let r = t.reconcile();
        assert_eq!(
            (r.last_started_at, r.last_finished_at, r.consecutive_failures),
            (Some(1_010), Some(1_011), 0)
        );
        assert!(r.last_duration_ms.is_some());
        for at in [1_030, 1_050] {
            let s = t.begin(at);
            t.finish(s, at + 1, Err("E_SSH: boom".into()));
        }
        let r = t.reconcile();
        assert_eq!(
            (r.consecutive_failures, r.failures_total, r.last_error.as_deref()),
            (2, 2, Some("E_SSH: boom"))
        );
        let s = t.begin(1_070);
        t.finish(s, 1_071, Ok(false));
        assert_eq!(t.reconcile().consecutive_failures, 2, "a skipped tick is not a pass");
        let s = t.begin(1_090);
        t.finish(s, 1_091, Ok(true));
        assert_eq!(t.reconcile().consecutive_failures, 0, "a good pass clears the streak");
        assert_eq!(t.reconcile().last_error, None);
        assert_eq!(t.reconcile().failures_total, 2, "the total never resets");
        assert_eq!(t.started_at(), 1_000);
        assert!(t.uptime_secs() >= 0);
    }
```

- [ ] **Step 2: Run, expect a compile error**

`cargo test -p fleet-core tick_stats_track` → `error[E0433]: failed to resolve: use of undeclared type TickStats`.

- [ ] **Step 3: Implement `TickStats` and record the pass**

In `tick.rs` (after the `USAGE_POLL_INTERVAL` const):

```rust
/// The last reconcile pass, as `fleet_health.hub.reconcile` reports it
/// (perf-logs §5: tick failures were `warn!` lines and nothing else).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReconcileStats {
    #[serde(default)]
    pub last_started_at: Option<i64>,
    #[serde(default)]
    pub last_finished_at: Option<i64>,
    #[serde(default)]
    pub last_duration_ms: Option<i64>,
    /// Failed passes since the last good one.
    #[serde(default)]
    pub consecutive_failures: u32,
    /// Failed passes since the process started; never resets.
    #[serde(default)]
    pub failures_total: u64,
    #[serde(default)]
    pub last_error: Option<String>,
}

/// What `fleet_health.hub` and the `/metrics` reconcile gauges read: when
/// this process started and how its last reconcile pass went, written by
/// the tick loop. One per process (`tick_stats()`), like `reconcile_gate()`.
pub struct TickStats {
    started: std::time::Instant,
    started_at: i64,
    reconcile: Mutex<ReconcileStats>,
}

impl TickStats {
    pub fn new(now: i64) -> Self {
        Self {
            started: std::time::Instant::now(),
            started_at: now,
            reconcile: Mutex::new(ReconcileStats::default()),
        }
    }

    /// Stamp the start of a pass; the `Instant` goes back into [`finish`](Self::finish).
    pub fn begin(&self, now: i64) -> std::time::Instant {
        if let Ok(mut r) = self.reconcile.lock() {
            r.last_started_at = Some(now);
        }
        std::time::Instant::now()
    }

    /// `Ok(true)`: a pass ran. `Ok(false)`: skipped, another pass was
    /// running — nothing but the start stamp changes. `Err`: the pass failed.
    pub fn finish(&self, started: std::time::Instant, now: i64, outcome: Result<bool, String>) {
        let Ok(mut r) = self.reconcile.lock() else {
            return;
        };
        let duration_ms = started.elapsed().as_millis() as i64;
        match outcome {
            Ok(false) => {}
            Ok(true) => {
                r.last_finished_at = Some(now);
                r.last_duration_ms = Some(duration_ms);
                r.consecutive_failures = 0;
                r.last_error = None;
            }
            Err(e) => {
                r.last_finished_at = Some(now);
                r.last_duration_ms = Some(duration_ms);
                r.consecutive_failures += 1;
                r.failures_total += 1;
                r.last_error = Some(e);
            }
        }
    }

    pub fn uptime_secs(&self) -> i64 {
        self.started.elapsed().as_secs() as i64
    }

    pub fn started_at(&self) -> i64 {
        self.started_at
    }

    pub fn reconcile(&self) -> ReconcileStats {
        self.reconcile
            .lock()
            .map(|r| r.clone())
            .unwrap_or_default()
    }
}

/// The process-wide stats, created on first use. `fleet-hub serve` touches
/// it before the ticks start so `started_at` is the serve start.
pub fn tick_stats() -> Arc<TickStats> {
    static STATS: std::sync::LazyLock<Arc<TickStats>> =
        std::sync::LazyLock::new(|| Arc::new(TickStats::new(unix_now())));
    Arc::clone(&STATS)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
```

In `spawn_reconcile_tick`'s closure replace the `match service::sessions::reconcile_now(store, ssh).await { … }` (lines 96-102) with:

```rust
                let stats = tick_stats();
                let started = stats.begin(unix_now());
                let outcome = service::sessions::reconcile_now(store, ssh).await;
                match &outcome {
                    Ok(true) => {}
                    Ok(false) => {
                        tracing::debug!("a reconcile pass is already running; skipping tick")
                    }
                    Err(e) => tracing::warn!("reconcile tick: reconcile failed: {e}"),
                }
                stats.finish(started, unix_now(), outcome.map_err(|e| e.to_string()));
```

In `crates/fleet-hub/src/serve.rs`, before `spawn_reconcile_tick` (line 853): `let _ = fleet_core::service::tick::tick_stats();` with the comment `// Pin started_at to the serve start, not to the first fleet_health.`

- [ ] **Step 4: Run, expect PASS**

`cargo test -p fleet-core tick_stats_track` → `1 passed`.

- [ ] **Step 5: Failing health test**

Append to `mod tests` in `health.rs`:

```rust
    /// perf-logs §1 / §5, hub-ops F8: `fleet_health` could not say whether
    /// tunnels applied, whether the tick was healthy, or whether "0 links
    /// down" meant "all up" or "none configured".
    #[test]
    fn health_reports_hub_uptime_tunnels_mode_and_peer_links_total() {
        let store = Store::open_in_memory().unwrap();
        let h = health_from_store(&store);
        let hub = h.hub.expect("this process reports itself");
        assert!(hub.uptime_secs >= 0 && hub.started_at > 0);
        assert_eq!(
            h.tunnels_mode.as_deref(),
            Some(TUNNELS_MODE_REVERSE),
            "no public URL: reverse tunnels carry the hooks"
        );
        assert_eq!(h.peer_links_total, 0, "nothing configured is 0 total, not merely 0 down");
        store
            .set_setting(crate::service::hub::SETTING_PUBLIC_URL, "https://fleet.example.com")
            .unwrap();
        assert_eq!(
            health_from_store(&store).tunnels_mode.as_deref(),
            Some(TUNNELS_MODE_NONE),
            "a public hub supervises no tunnel"
        );
        let v = serde_json::to_value(health_from_store(&store)).unwrap();
        assert_eq!(v["tunnels_mode"], "none");
        assert!(v["hub"]["reconcile"].is_object(), "{v}");
    }
```

- [ ] **Step 6: Run, expect a compile error** — `cargo test -p fleet-core health_reports_hub_uptime` → `no field hub on type Health`.

- [ ] **Step 7: Implement the health fields**

In `health.rs` add after the `Health` struct:

```rust
/// `fleet_health.hub`: this process's uptime and its last reconcile pass.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubHealth {
    #[serde(default)]
    pub started_at: i64,
    #[serde(default)]
    pub uptime_secs: i64,
    #[serde(default)]
    pub reconcile: crate::service::tick::ReconcileStats,
}

/// `Health::tunnels_mode` on a hub with a public URL: hooks post to it
/// directly and there is nothing to supervise, so an empty `tunnels` map
/// is "not applicable", not "all down".
pub const TUNNELS_MODE_NONE: &str = "none";
/// `Health::tunnels_mode` without a public URL: reverse SSH tunnels carry
/// the hooks and `tunnels` is their supervisor's view.
pub const TUNNELS_MODE_REVERSE: &str = "reverse";

/// Whether this fleet's hooks ride reverse tunnels, from `hub.public_url`.
pub fn tunnels_mode(s: &Store) -> String {
    let public = s
        .get_setting(crate::service::hub::SETTING_PUBLIC_URL)
        .ok()
        .flatten()
        .is_some_and(|u| !u.trim().is_empty());
    if public { TUNNELS_MODE_NONE } else { TUNNELS_MODE_REVERSE }.to_string()
}

fn hub_health() -> HubHealth {
    let t = crate::service::tick::tick_stats();
    HubHealth {
        started_at: t.started_at(),
        uptime_secs: t.uptime_secs(),
        reconcile: t.reconcile(),
    }
}
```

Add to `Health` (after `trackers`):

```rust
    /// This process's uptime and last reconcile pass (perf-logs §5).
    /// Per-field default: an older hub omits it.
    #[serde(default)]
    pub hub: Option<HubHealth>,
    /// [`TUNNELS_MODE_NONE`] on a public hub, [`TUNNELS_MODE_REVERSE`]
    /// otherwise; `None` from an older hub.
    #[serde(default)]
    pub tunnels_mode: Option<String>,
    /// Configured hub↔hub links (revoked excluded), so `peer_links_down: 0`
    /// can be told from "nothing configured".
    #[serde(default)]
    pub peer_links_total: u32,
```

`health_from_store`: add `hub: Some(hub_health()), tunnels_mode: Some(tunnels_mode(s)), peer_links_total: s.peer_links_total().unwrap_or_default(),`. `health_check`'s poisoned arm: `hub: None, tunnels_mode: None, peer_links_total: 0,`. Every other `Health {` literal in the crate (`grep -rn "Health {" crates src-tauri` — the test literals at `health.rs:806-823`) gets the same three fields.

In `store/peer_links.rs` after `peer_links_down`:

```rust
    /// Links of either role that are configured and not revoked —
    /// `fleet_health.peer_links_total`.
    pub fn peer_links_total(&self) -> Result<u32, IpcError> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM peer_links WHERE revoked_at IS NULL",
            [],
            |r| r.get(0),
        )?;
        Ok(n as u32)
    }
```

- [ ] **Step 8: Run, regen the golden, expect PASS**

`cargo test -p fleet-core health_reports_hub_uptime` → `1 passed`. Then `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract` (reports FAILED once, rewrites `src-tauri/src/backend/hub_contract.golden.json` with `hub`, `tunnels_mode`, `peer_links_total` under `Health` and a new `Health.hub` / `Health.hub.reconcile` row), then `cargo test -p claude-fleet --lib contract` → green. `CONTRACT_REVISION` stays 4.

- [ ] **Step 9: Failing metrics test**

Append to `mod tests` in `metrics.rs`:

```rust
    #[test]
    fn expose_adds_the_hub_gauges() {
        let m = Metrics::new();
        let mut by_status = BTreeMap::new();
        by_status.insert("working".to_string(), 12u32);
        by_status.insert("idle".to_string(), 3u32);
        let hub = HubGauges {
            reconcile_duration_ms: Some(812),
            reconcile_failures_total: 2,
            sessions_by_status: by_status,
            hosts_reachable: 5,
        };
        let out = m.expose(&BTreeMap::new(), &hub);
        for line in [
            "# TYPE fleet_reconcile_duration_ms gauge",
            "fleet_reconcile_duration_ms 812",
            "# TYPE fleet_reconcile_failures_total counter",
            "fleet_reconcile_failures_total 2",
            "# TYPE fleet_sessions gauge",
            "fleet_sessions{status=\"idle\"} 3",
            "fleet_sessions{status=\"working\"} 12",
            "# TYPE fleet_hosts_reachable gauge",
            "fleet_hosts_reachable 5",
        ] {
            assert!(out.contains(line), "missing {line:?} in:\n{out}");
        }
        let none = m.expose(&BTreeMap::new(), &HubGauges::default());
        assert!(
            !none.contains("fleet_reconcile_duration_ms "),
            "no pass yet: HELP/TYPE only, no sample"
        );
    }
```

- [ ] **Step 10: Run, expect a compile error** — `cargo test -p fleet-core expose_adds_the_hub_gauges` → `cannot find struct HubGauges`.

- [ ] **Step 11: Implement the gauges**

In `metrics.rs`:

```rust
/// Process-level gauges beside the per-caller counters (perf-logs §5:
/// `/metrics` could not answer "is reconcile healthy"). Read from the
/// cached rows and the tick stats — no network, one short lock.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HubGauges {
    pub reconcile_duration_ms: Option<i64>,
    pub reconcile_failures_total: u64,
    pub sessions_by_status: BTreeMap<String, u32>,
    pub hosts_reachable: u32,
}

impl HubGauges {
    pub fn read(store: &Mutex<crate::store::Store>, stats: &crate::service::tick::TickStats) -> Self {
        let r = stats.reconcile();
        let (sessions_by_status, hosts_reachable) = match store.lock() {
            Ok(s) => {
                let summary = crate::service::health::summarize(
                    &s.list_all_sessions().unwrap_or_default(),
                    &s.list_hosts().unwrap_or_default(),
                );
                (summary.by_status, summary.hosts_reachable)
            }
            Err(_) => (BTreeMap::new(), 0),
        };
        Self {
            reconcile_duration_ms: r.last_duration_ms,
            reconcile_failures_total: r.failures_total,
            sessions_by_status,
            hosts_reachable,
        }
    }
}
```

Change `expose(&self, streams: &BTreeMap<String, usize>)` to `expose(&self, streams: &BTreeMap<String, usize>, hub: &HubGauges)` and append before `out`:

```rust
        out.push_str("# HELP fleet_reconcile_duration_ms Wall time of the last reconcile pass.\n");
        out.push_str("# TYPE fleet_reconcile_duration_ms gauge\n");
        if let Some(ms) = hub.reconcile_duration_ms {
            out.push_str(&format!("fleet_reconcile_duration_ms {ms}\n"));
        }
        out.push_str("# HELP fleet_reconcile_failures_total Reconcile passes that failed since start.\n");
        out.push_str("# TYPE fleet_reconcile_failures_total counter\n");
        out.push_str(&format!(
            "fleet_reconcile_failures_total {}\n",
            hub.reconcile_failures_total
        ));
        out.push_str("# HELP fleet_sessions Fleet sessions by claude_status (external rows excluded).\n");
        out.push_str("# TYPE fleet_sessions gauge\n");
        for (status, n) in &hub.sessions_by_status {
            out.push_str(&format!("fleet_sessions{{status=\"{}\"}} {n}\n", escape(status)));
        }
        out.push_str("# HELP fleet_hosts_reachable Hosts whose last probe succeeded.\n");
        out.push_str("# TYPE fleet_hosts_reachable gauge\n");
        out.push_str(&format!("fleet_hosts_reachable {}\n", hub.hosts_reachable));
```

`MetricsState` gains `pub store: std::sync::Arc<Mutex<crate::store::Store>>` and `pub stats: std::sync::Arc<crate::service::tick::TickStats>`; `handle_metrics` computes `let hub = HubGauges::read(&state.store, &state.stats);` and passes `&hub` to `expose`. Every `MetricsState {` literal (the nine sites listed under **Files**) adds `store: Arc::clone(&store), stats: crate::service::tick::tick_stats(),` (in `mod.rs:747` the `store` in scope is the writer `Arc` already cloned into `hook_state` — clone it once more before that move).

- [ ] **Step 12: Run, expect PASS** — `cargo test -p fleet-core metrics` → the new test and `metrics_answers_the_master_token_and_refuses_the_rest` (`mod.rs:1720`) pass.

- [ ] **Step 13: Failing log-level tests, with a capture helper**

Add to `crates/fleet-core/src/logging.rs`:

```rust
/// Test-only: run code under a scoped subscriber that writes to a buffer,
/// so a test can assert a log LINE (its level and fields), not only a side
/// effect. Thread-local (`set_default`), so it also covers a
/// `current_thread` `#[tokio::test]`.
#[cfg(test)]
pub(crate) mod capture {
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    pub struct Buf(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buf {
        type Writer = Buf;
        fn make_writer(&'a self) -> Buf {
            self.clone()
        }
    }

    pub struct Captured {
        buf: Buf,
        _guard: tracing::subscriber::DefaultGuard,
    }

    impl Captured {
        pub fn text(&self) -> String {
            String::from_utf8(self.buf.0.lock().unwrap().clone()).unwrap_or_default()
        }
    }

    /// Everything logged at DEBUG and above on this thread until the guard drops.
    pub fn start() -> Captured {
        let buf = Buf::default();
        let sub = tracing_subscriber::fmt()
            .with_writer(buf.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .finish();
        Captured {
            buf,
            _guard: tracing::subscriber::set_default(sub),
        }
    }
}
```

In `service/usage.rs` `mod tests`:

```rust
    /// perf-logs §5: a failed collection was DEBUG, invisible with the
    /// default filter. One WARN per host per pass names the host and code.
    #[tokio::test]
    async fn a_failed_collection_is_a_warn_line_naming_the_host() {
        let (store, _id, _path) = store_with_session("vps");
        let fake = FakeSsh::new();
        fake.on(Match::script_contains("tail -c +"), Reply::fail(1, "ssh: connect to host vps port 22: Connection refused"));
        let log = crate::logging::capture::start();
        assert_eq!(collect_all(&store, &fake, 1_000).await, 0);
        let text = log.text();
        assert!(text.contains("WARN"), "{text}");
        assert!(text.contains("usage collection failed") && text.contains("host=vps"), "{text}");
        assert!(text.contains("E_SHELL"), "{text}");
    }
```

(`store_with_session` seeds `vps` reachable — check its body at `usage.rs:1640-1654`; if it leaves `reachable` false, add `s.set_host_reachable("vps", true)` or the equivalent probe upsert the neighbouring `collect_host_runs_one_quoted_batch_and_accumulates` relies on.)

In `service/playbooks.rs` `mod tests`:

```rust
    /// perf-logs §5: "applied 1 stuck playbook(s)" named no host, session or
    /// kind — the oom loop on 21480 was invisible in the log.
    #[tokio::test]
    async fn an_applied_playbook_logs_host_session_kind_and_action_at_info() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        seed_stuck(&store, "dev-log", "press_enter");
        let exec = FakeExec {
            enters: AtomicUsize::new(0),
            recreates: AtomicUsize::new(0),
            fail: false,
            attached: false,
        };
        let log = crate::logging::capture::start();
        assert_eq!(run_with(&store, &exec, &ALL_ON, now_unix() + 10).await, 1);
        let text = log.text();
        assert!(text.contains("INFO") && text.contains("[playbook] applied"), "{text}");
        for field in ["host=local", "session=dev-log", "kind=press_enter", "action=press_enter"] {
            assert!(text.contains(field), "missing {field} in:\n{text}");
        }
    }
```

In `service/sessions/tests.rs` (next to the ghost/stuck tests), a stuck transition through the real pass, using the `press_enter` fixture the pane classifier already has:

```rust
/// perf-logs §5: a stuck transition was a `session_events` row and nothing
/// in the log. One INFO line names host, session and kind.
#[tokio::test]
async fn a_stuck_transition_is_logged_with_host_session_and_kind() {
    use async_trait::async_trait;
    struct StuckTmux;
    #[async_trait]
    impl TmuxExec for StuckTmux {
        async fn list_sessions(&self) -> Result<Vec<crate::tmux::TmuxSession>, IpcError> {
            Ok(vec![tmux_session("dev-stuck")])
        }
        async fn new_session(&self, _n: &str, _c: &std::path::Path, _p: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn kill_session(&self, _n: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn rename_session(&self, _o: &str, _n: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn restart_session(&self, _n: &str, _p: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn capture_pane(&self, _n: &str) -> Result<String, IpcError> {
            Ok(include_str!("../testdata/pane_intel/permission_bash.txt").to_string())
        }
        async fn capture_pane_scrollback(&self, _n: &str, _l: u32) -> Result<String, IpcError> {
            Ok(String::new())
        }
        async fn list_claude_agents(&self) -> Option<Vec<crate::claude_agents::ClaudeAgentRow>> {
            Some(vec![])
        }
    }
    let store = Mutex::new(Store::open_in_memory().expect("store"));
    let deps = ReconcileDeps::fake(|_| Box::new(StuckTmux), std::time::Duration::from_secs(5));
    // First pass creates the row (no prior → no transition event).
    reconcile_sessions_with(&store, &deps).await.unwrap();
    {
        let s = store.lock().unwrap();
        let row = s.get_session("dev-stuck", "local").unwrap().unwrap();
        assert!(row.stuck_kind.is_some(), "the fixture must classify as stuck: {row:?}");
    }
    // Clear the flag, then let the next pass re-detect it: that is the transition.
    {
        let s = store.lock().unwrap();
        s.conn
            .execute("UPDATE sessions SET stuck_kind = NULL WHERE tmux_name = 'dev-stuck'", [])
            .unwrap();
    }
    let log = crate::logging::capture::start();
    reconcile_sessions_with(&store, &deps).await.unwrap();
    let text = log.text();
    assert!(text.contains("INFO") && text.contains("[reconcile] stuck"), "{text}");
    assert!(text.contains("host=local") && text.contains("session=dev-stuck"), "{text}");
}
```

(If `permission_bash.txt` classifies as `blocked` with no `stuck_kind`, pick the fixture whose expected `stuck_kind` is set in `service/pane_intel.rs`'s fixture table — `grep -n "stuck_kind: Some" crates/fleet-core/src/service/pane_intel.rs` — the test asserts `stuck_kind.is_some()` first so a wrong fixture fails loudly at that line.)

- [ ] **Step 14: Run, expect the three to fail on the missing lines**

`cargo test -p fleet-core a_failed_collection_is_a_warn_line` → assertion `text.contains("WARN")` fails (the line is DEBUG); the playbook and stuck tests fail on their `contains` assertions.

- [ ] **Step 15: Implement the log lines**

`usage.rs:694`: `Err(e) => tracing::warn!(host = %host, code = %e.code, error = %e.message, "usage collection failed (retried next interval)"),`.

`playbooks.rs`, after the `if let Err(e) = &result { … }` block (line 272), add:

```rust
        if let Ok(outcome) = &result {
            tracing::info!(
                host = %p.host_alias,
                session = %p.tmux_name,
                kind = %p.stuck_kind,
                action = p.action.as_str(),
                outcome = ?outcome,
                "[playbook] applied"
            );
        }
```

`reconcile.rs:941-944`: inside `if row.stuck_kind.is_some() && row.stuck_kind != prior.stuck_kind {` add before the `events.push`:

```rust
            tracing::info!(
                host = %host.alias,
                session = %tmux_name,
                kind = row.stuck_kind.as_deref().unwrap_or("?"),
                was = prior.stuck_kind.as_deref().unwrap_or("none"),
                "[reconcile] stuck"
            );
```

(`PressEnterOutcome` needs `Debug` for `?outcome`; derive it if it does not already.)

- [ ] **Step 16: Run, expect PASS** — the three tests pass; `cargo test -p fleet-core` stays green.

- [ ] **Step 17: Description, docs, compose, regen**

`fleet.rs:9-16` — append to the `fleet_health` description, keeping it one clause: `hub: this process's uptime and last reconcile pass; tunnels_mode none|reverse; peer_links_total.` Then `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current` and `cargo test -p fleet-core the_served_definition_budget_stays_bounded`.

`deploy/hub/behind-proxy/docker-compose.yml`: add `FLEET_HUB_LOG_DIR: /var/lib/fleet-hub/logs` under `environment:` and `- ./logs:/var/lib/fleet-hub/logs` under `volumes:`, with the comment `# Hourly files the operator can read without sudo: install -d -o 1000 -g users -m 2750 logs (setgid, so new files inherit the group).`

`docs/hub.md`:
- `/metrics` section: extend the sample and add: "Besides the per-caller counters the exposition carries four process gauges: `fleet_reconcile_duration_ms` (the last pass's wall time), `fleet_reconcile_failures_total`, `fleet_sessions{status="…"}` (by `claude_status`, external rows excluded, the same roll-up `fleet_health.by_status` uses) and `fleet_hosts_reachable`."
- Under *Setup*'s `fleet_health` curl, one paragraph: "`hub` is this process: `started_at`, `uptime_secs` and `reconcile` (`last_started_at`, `last_finished_at`, `last_duration_ms`, `consecutive_failures`, `failures_total`, `last_error`) — alert on `consecutive_failures >= 3`. `tunnels_mode` is `none` on a public hub (hooks post directly; the `tunnels` map is empty because nothing applies) and `reverse` otherwise. `peer_links_total` tells `peer_links_down: 0` from 'no peers configured'."
- *Behind an existing reverse proxy*: "**Logs without sudo.** The variant mounts `./logs` as the hub's log directory (`FLEET_HUB_LOG_DIR`); create it as `install -d -o 1000 -g users -m 2750 logs` so hourly files stay group-readable. Docker's own capture of the same lines is capped at 3 × 10 MB. **Watching it.** Uptime Kuma: an HTTP keyword monitor on `/healthz` (`fleet-hub ok`) and a JSON-query monitor posting `tools/call fleet_health` to `/mcp/json` with a readonly client token (`fleet-hub pair --mode readonly kuma`) — never the master — on `db_ready`, `hosts_reachable`, `tunnels_flapping` and `hub.reconcile.consecutive_failures`."

- [ ] **Step 18: Commit**

```bash
git add crates/fleet-core/src/service/tick.rs crates/fleet-core/src/service/health.rs crates/fleet-core/src/store/peer_links.rs crates/fleet-core/src/mcp/metrics.rs crates/fleet-core/src/mcp/mod.rs crates/fleet-core/src/mcp/tests_token_cache.rs crates/fleet-core/src/mcp/report_route.rs crates/fleet-core/src/service/usage.rs crates/fleet-core/src/service/playbooks.rs crates/fleet-core/src/service/sessions/reconcile.rs crates/fleet-core/src/service/sessions/tests.rs crates/fleet-core/src/logging.rs crates/fleet-core/src/mcp/tools/fleet.rs crates/fleet-hub/src/serve.rs src-tauri/src/backend/hub_contract.golden.json docs/control-api-reference.md deploy/hub/behind-proxy/ docs/hub.md
git commit -m "feat(hub): fleet_health.hub tick stats, /metrics reconcile and fleet gauges, tunnels_mode and peer_links_total, and WARN/INFO lines for usage failures and stuck transitions"
```

---

### Task 4: Event stream resume — the desktop sends `Last-Event-ID`, honours `resumed`, and a resync re-lists projects, worktrees and work (perf-logs §4; row 19) — effort M

The hub side is already on main: `events_route.rs:543` reads `last-event-id` / `?since=`, `row_event` puts `.id(frame_id(generation, seq))` on every row frame (520), and `ready` carries `"resumed"` (682); `mcp/mod.rs:1994/2020` test both answers. Only the desktop never sent the header (`backend/events.rs:844-847`).

**Files:**
- Modify: `crates/fleet-core/src/mcp/wire.rs` (`SseFrame` 26-35, `SseDecoder` 51-140, test helper at 196)
- Modify: `src-tauri/src/backend/events.rs` (`HubEventStream::open` 108-110, `EventBridge` 242-268, `run` 279-345, `pump` 404-427, `deliver` 519-560, `HubResync::resync` 711-786, `HubSse::open` 815-826, `open_stream` 837-850)
- Modify: `src-tauri/src/backend/tests_events.rs` (`ScriptedStream` 150-185, helpers 239-260, the resync expectation at 1675-1700)
- Modify: `src/lib/hub_connection.ts`, `src/lib/hub_connection.test.ts`, `src/App.svelte` (onMount near line 275)
- Modify: `docs/hub.md` *Events* (the `Last-Event-ID` paragraph, line ~1340) and *Point a desktop at the hub*

**Interfaces:**
- Consumes: `/events` frames with `id:`; `ready.resumed`; `HubBackend::list_*` (unchanged).
- Produces: `SseFrame.id: Option<String>`; `HubEventStream::open(&self, last_id: Option<&str>)`; `pub fn events_request(at: &Endpoint, bearer: &str, last_id: Option<&str>) -> String`; `pub const RESYNCED_EVENT: &str = "hub:resynced"` emitted through the sink after every re-list; frontend `setGapHandler(fn)` in `hub_connection.ts`.

Why an event and not a wider resync: `list_projects` answers `ProjectTreeRow` and `list_worktrees` `WorktreeOccupancy`, while `project:updated` / `worktree:updated` carry `ProjectRow` / `WorktreeRow` (`events.rs:640-648`) — re-emitting the list shapes under the row names would corrupt the stores. The window already owns `loadProjects()` (`src/lib/projects.ts:46`, worktrees included in the tree rows) and `loadTrackers()` (`src/lib/trackers.ts:279`, work items and trackers), so the bridge tells it *that* a gap closed and the window refetches with its own loaders. No store shape changes.

- [ ] **Step 1: Failing decoder test**

In `wire.rs` `mod tests`:

```rust
    #[test]
    fn an_id_line_rides_on_its_frame_and_only_that_frame() {
        let mut d = SseDecoder::new();
        let frames = d.feed(
            "event: session:updated\nid: 7-42\ndata: {\"id\":1}\n\nevent: ready\ndata: {}\n\n",
        );
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].id.as_deref(), Some("7-42"));
        assert_eq!(frames[1].id, None, "`ready` carries no id; nothing is inherited");
    }
```

- [ ] **Step 2: Run, expect a compile error** — `cargo test -p fleet-core an_id_line_rides` → `no field id on type SseFrame`.

- [ ] **Step 3: Implement**

`SseFrame` gains `/// The `id:` field, when the server sent one — what a client hands back as `Last-Event-ID`. pub id: Option<String>,`; `SseDecoder` gains `id: Option<String>`; in `line`: replace the `_ => {}` comment/arm with

```rust
            // The spec ignores an id holding U+0000; `retry` stays unused.
            "id" if !value.contains('\0') => self.id = Some(value.to_string()),
            _ => {}
```

and `dispatch` builds `SseFrame { name, data, id: self.id.take() }`. The test helper `frame(name, data)` at `wire.rs:196` gets `id: None`.

- [ ] **Step 4: Run, expect PASS** — `cargo test -p fleet-core wire::` green.

- [ ] **Step 5: Failing bridge tests**

In `tests_events.rs`, extend `ScriptedStream` with `asked: StdMutex<Vec<Option<String>>>` (initialised empty in `new`) and record in `open`: `self.asked.lock().unwrap().push(last_id.map(str::to_string));` plus `fn asked(&self) -> Vec<Option<String>> { self.asked.lock().unwrap().clone() }`. Add helpers next to `frame`:

```rust
/// `frame` with the `id:` line `/events` puts on every row frame.
fn frame_with_id(name: &str, payload: &Value, id: &str) -> String {
    format!("event: {name}\nid: {id}\ndata: {payload}\n\n")
}

/// A `ready` frame saying whether the hub honoured `Last-Event-ID`.
fn ready_resumed(resumed: bool) -> String {
    frame(
        READY_FRAME,
        &json!({
            "version": "0.3.1", "now": 1, "kinds": ["session"],
            "contract": fleet_core::wire_contract::CONTRACT_REVISION,
            "resumed": resumed
        }),
    )
}
```

Tests:

```rust
/// perf-logs §4: the desktop never sent `Last-Event-ID`, so every reconnect
/// re-listed sessions, hosts, tasks and accounts (63 `session:updated`).
#[tokio::test]
async fn a_reconnect_sends_the_last_frame_id_and_a_resumed_ready_skips_the_re_list() {
    let killed = RowChange::SessionKilled(1);
    let cancel = CancellationToken::new();
    let stream = ScriptedStream::new(
        vec![
            Connection::Delivers(vec![
                ready_resumed(false),
                frame_with_id(killed.name(), &killed.payload(), "3-41"),
            ]),
            Connection::Delivers(vec![
                ready_resumed(true),
                frame_with_id(killed.name(), &killed.payload(), "3-42"),
            ]),
            Connection::Delivers(vec![ready_resumed(false)]),
        ],
        cancel.clone(),
    );
    let sink = Arc::new(Recorder::default());
    let resync = Arc::new(CountingResync::default());
    let delay = Arc::new(FakeDelay::default());
    EventBridge::new(stream.clone(), sink, resync.clone(), delay, cancel)
        .run()
        .await;
    assert_eq!(
        stream.asked(),
        vec![None, Some("3-41".to_string()), Some("3-42".to_string())],
        "the first open has nothing to resume from; every reconnect names the last id it applied"
    );
    assert_eq!(
        resync.count(),
        2,
        "the connection the hub resumed needs no re-list; the other two do"
    );
}

#[test]
fn the_events_request_carries_last_event_id_only_when_there_is_one() {
    let at = crate::backend::remote::Endpoint::parse("https://fleet.example.com/events").unwrap();
    let with = crate::backend::events::events_request(&at, "cl_tok", Some("3-41"));
    assert!(with.contains("\r\nLast-Event-ID: 3-41\r\n"), "{with}");
    assert!(with.contains("Authorization: Bearer cl_tok"), "{with}");
    let without = crate::backend::events::events_request(&at, "cl_tok", None);
    assert!(!without.contains("Last-Event-ID"), "{without}");
}

#[tokio::test]
async fn a_resync_ends_by_telling_the_window_to_refetch_projects_and_work() {
    let (resync, seen, _table) = resync_over(&[
        ("list_sessions", "[]"),
        ("list_hosts", "[]"),
        ("list_tasks", "[]"),
        ("list_accounts", "[]"),
    ]);
    resync.resync().await;
    assert_eq!(
        seen.names().last().map(|n| n.to_string()),
        Some(crate::backend::events::RESYNCED_EVENT.to_string()),
        "projects, worktrees and work have list shapes the events cannot carry; \
         the window re-fetches them with its own loaders on this signal"
    );
}
```

Update `a_resync_emits_the_rows_it_re_listed_as_the_events_the_stores_apply` (1675): its expected `seen.names()` ends with `"hub:resynced"`.

- [ ] **Step 6: Run, expect compile errors** — `cargo test -p claude-fleet --lib tests_events` → `open` takes 0 arguments, `events_request` / `RESYNCED_EVENT` not found.

- [ ] **Step 7: Implement the desktop side**

`events.rs`:

```rust
/// What the bridge emits after every re-list. Not a row event: the window
/// re-fetches the stores whose list tools answer a different shape than
/// their events (projects with their worktrees, trackers with their work
/// items) with its own loaders — see `HubResync` for why they are not
/// re-listed here.
pub const RESYNCED_EVENT: &str = "hub:resynced";
```

- `HubEventStream::open(&self, last_id: Option<&str>) -> Result<Box<dyn EventStreamBody>, String>` (doc: "`last_id` is the last frame id this client applied, sent as `Last-Event-ID` so the hub can replay the gap").
- `EventBridge` gains `last_id: Mutex<Option<String>>` (initialised `Mutex::new(None)` in `new`).
- In `run`: `let last = self.last_id.lock().expect("last id").clone(); match self.stream.open(last.as_deref()).await {`.
- In `pump`, the `InRange` arm: keep `self.status.report(HubConnection::Connected);` then

```rust
                            if resumed(&frame.data) {
                                tracing::info!("[hub events] the hub replayed the gap; no re-list");
                            } else {
                                self.resync.resync().await;
                            }
```

with

```rust
/// The `ready` frame's `resumed` field: whether `Last-Event-ID` was honoured.
/// A hub without the field (or a malformed one) is read as `false`, which is
/// today's behaviour: re-list.
fn resumed(ready: &str) -> bool {
    serde_json::from_str::<Value>(ready)
        .ok()
        .and_then(|v| v.get("resumed")?.as_bool())
        .unwrap_or(false)
}
```

- In `pump`'s frame loop, after `match self.deliver(&frame.name, &frame.data)` returns `Delivery::Row`: `if let Some(id) = &frame.id { *self.last_id.lock().expect("last id") = Some(id.clone()); }` (the frame is applied first, then remembered, so a crash between the two re-applies rather than skips).
- `HubResync::resync`: after the accounts block append `self.sink.emit_remote(RESYNCED_EVENT, serde_json::json!({ "projects": true, "work": true }));` and update the struct doc's "deliberately left out" paragraph to say the window re-fetches them on `hub:resynced`.
- `HubSse::open(&self, last_id: Option<&str>)` passes it to `open_stream(&at, &self.cfg.token, last_id)`.
- Factor the request text:

```rust
/// The `GET /events` request head. No `Connection: close` — this request is
/// supposed to stay open. `Cache-Control: no-cache` is what the SSE spec asks
/// a client to send. `Last-Event-ID` is the last row frame this client
/// applied (`/events` replays from it, or says `resumed: false`).
pub fn events_request(at: &Endpoint, bearer: &str, last_id: Option<&str>) -> String {
    let mut request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {bearer}\r\n\
         Accept: text/event-stream\r\nCache-Control: no-cache\r\n",
        at.target(),
        at.authority()
    );
    if let Some(id) = last_id {
        request.push_str(&format!("Last-Event-ID: {id}\r\n"));
    }
    request.push_str("\r\n");
    request
}
```

and have `open_stream(at, bearer, last_id)` use it. Update the doc comment on `RemoteEventSink` ("the only way to obtain one is `known_event_name`") to add "or the bridge's own `RESYNCED_EVENT`". The production `HubSse` is built in `bootstrap/tasks.rs`; its call site does not change (only the trait signature).

- [ ] **Step 8: Run, expect PASS** — `cargo test -p claude-fleet --lib tests_events` green (the real-socket test `the_real_stream_reads_a_chunked_sse_response_from_a_real_socket` at 1425 calls `open()`; give it `open(None)`).

- [ ] **Step 9: Frontend — failing Vitest**

In `src/lib/hub_connection.test.ts` add:

```ts
it('a hub:resynced event runs the gap handler the app installed', async () => {
  const handlers = new Map<string, (e: { payload: unknown }) => void>();
  lis().mockImplementation(async (name: string, cb: (e: { payload: unknown }) => void) => {
    handlers.set(name, cb);
    return () => {};
  });
  inv().mockResolvedValue({ state: 'connected' });
  const gap = vi.fn();
  setGapHandler(gap);
  await startHubConnection();
  handlers.get('hub:resynced')?.({ payload: { projects: true, work: true } });
  expect(gap).toHaveBeenCalledTimes(1);
  setGapHandler(null);
  handlers.get('hub:resynced')?.({ payload: { projects: true, work: true } });
  expect(gap).toHaveBeenCalledTimes(1);
});
```

and import `setGapHandler` from `./hub_connection`. `npx vitest run src/lib/hub_connection.test.ts` → `setGapHandler is not exported`.

- [ ] **Step 10: Implement the frontend**

`src/lib/hub_connection.ts`:

```ts
/** Runs after the backend's event bridge re-listed sessions, hosts, tasks
 * and accounts following a gap it could not replay (`hub:resynced`). The
 * app installs the loaders for the stores whose list tools answer a
 * different shape than their events: projects (with worktrees) and
 * trackers (with work items). */
export type GapHandler = () => void;
let onGap: GapHandler | null = null;
export function setGapHandler(fn: GapHandler | null): void {
  onGap = fn;
}
```

and in `startHubConnection`, after the `hub:connection` listen: `await listen('hub:resynced', () => onGap?.());`.

`src/App.svelte`: import `setGapHandler` from `./lib/hub_connection`; in `onMount` next to `void loadTrackers();` (line 275) add

```ts
    // A hub reconnect the hub could not replay: the backend re-lists rows
    // itself; projects/worktrees and trackers/work have list shapes their
    // events cannot carry, so this window re-fetches them here.
    setGapHandler(() => {
      void loadProjects();
      void loadTrackers();
    });
```

and `setGapHandler(null)` in the returned cleanup.

- [ ] **Step 11: Run, expect PASS** — `npx vitest run src/lib/hub_connection.test.ts` green; `npx svelte-check` clean.

- [ ] **Step 12: Docs** — `docs/hub.md` *Events*: after the `Last-Event-ID` paragraph add "The desktop does this: it sends the last row frame id it applied, re-lists only when `ready` says `resumed: false`, and then also re-fetches projects, worktrees and work in the window." Under *Point a desktop at the hub* → *What is different from standalone*, one sentence: "After a dropped stream the app resumes from the last event it applied when the hub still has it (15 minutes / 512 events); otherwise it re-lists."

- [ ] **Step 13: Commit**

```bash
git add crates/fleet-core/src/mcp/wire.rs src-tauri/src/backend/events.rs src-tauri/src/backend/tests_events.rs src/lib/hub_connection.ts src/lib/hub_connection.test.ts src/App.svelte docs/hub.md
git commit -m "feat(desktop): resume the hub event stream with Last-Event-ID, re-list only on resumed:false, and refetch projects and work after a gap"
```

---

### Task 5: Startup — log the resolve steps and bound the keychain wait (perf-logs §4; row 20) — effort S

`Backend::resolve` lives in `src-tauri/src/backend/mod.rs:233` (not `startup.rs`); `resolve_detail` (242-352) reads the token through `TokenStore::get`, which on macOS is the Security framework call at `token_store.rs:76-84` with no timeout, inside Tauri's `setup` closure (`lib.rs:199`) on the main thread.

**Files:**
- Modify: `src-tauri/src/backend/token_store.rs` (add `BoundedTokenStore`, `KEYCHAIN_WAIT` after `OsTokenStore`)
- Modify: `src-tauri/src/lib.rs:197-200`
- Modify: `src-tauri/src/backend/tests_startup.rs`
- Modify: `docs/hub.md` *Point a desktop at the hub* → *When the configured hub cannot be used* (line 1785)

**Interfaces:**
- Consumes: `TokenStore` (`token_store.rs:17-25`), `Backend::resolve`, `Resolution::unavailable` (mod.rs:192).
- Produces: `pub struct BoundedTokenStore { inner: Arc<dyn TokenStore>, wait: Duration }` implementing `TokenStore`; `pub const KEYCHAIN_WAIT: Duration = 10 s`; INFO lines `startup: resolving backend`, `startup: reading the hub client token`, `startup: hub client token read finished elapsed_ms=…`, `startup: backend resolved`; a WARN and an `Unavailable` resolution when the read does not answer in time.

Deviation from the brief, deliberate: a timed-out keychain read yields `Backend::Unavailable` (the existing "configured hub cannot be used" banner, `hubStatus.unavailable`), **not** local mode. `resolve_detail` is explicit (mod.rs:277-278): once `hub.remote_url` is set every early return is `Unavailable` and never `Local`, because a paired desktop that guesses standalone becomes a second brain reconciling the hub's fleet (`tests_startup.rs:221-229`, `two_brains`). The banner names the reason and says to relaunch after unlocking.

- [ ] **Step 1: Failing tests**

In `tests_startup.rs` add `use std::time::Duration;` and:

```rust
/// A token store whose read blocks — the macOS keychain with a prompt
/// pending (perf-logs §4: 41 s, 2 m 41 s and 11 h 44 m between `starting`
/// and the next log line, the app a dead window meanwhile).
struct StallingTokenStore;

impl TokenStore for StallingTokenStore {
    fn get(&self) -> Result<Option<String>, String> {
        std::thread::sleep(Duration::from_secs(5));
        Ok(Some("cl_late".into()))
    }
    fn set(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn clear(&self) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn a_token_store_that_does_not_answer_in_time_makes_the_hub_unusable_not_standalone() {
    let (_dir, store) = store_with(&[(REMOTE_URL_KEY, "https://fleet.example.com")]);
    let bounded = BoundedTokenStore::new(Arc::new(StallingTokenStore), Duration::from_millis(100));
    let started = std::time::Instant::now();
    let backend = Backend::resolve(&store, &bounded);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the wait is bounded; took {:?}",
        started.elapsed()
    );
    let hub = backend
        .unavailable()
        .expect("a configured hub whose token never came is unusable, never standalone");
    assert!(hub.reason.contains("did not answer"), "{}", hub.reason);
    let recorder = Recorder::default();
    start_background_tasks(&backend, &recorder);
    assert_eq!(recorder.started(), NOTHING, "{}", two_brains("the token store stalled"));
}

#[test]
fn a_prompt_token_store_passes_through_the_bound_unchanged() {
    let (_dir, store) = store_with(&[(REMOTE_URL_KEY, "https://fleet.example.com")]);
    let bounded = BoundedTokenStore::new(
        Arc::new(InMemoryTokenStore::with_token("cl_tok")),
        Duration::from_secs(1),
    );
    assert!(Backend::resolve(&store, &bounded).is_remote());
}
```

- [ ] **Step 2: Run, expect a compile error** — `cargo test -p claude-fleet --lib tests_startup` → `cannot find type BoundedTokenStore`.

- [ ] **Step 3: Implement**

In `token_store.rs` after the `OsTokenStore` impls:

```rust
/// How long the launch waits on the token store before giving up on it.
pub const KEYCHAIN_WAIT: Duration = Duration::from_secs(10);

/// A [`TokenStore`] whose `get` runs on its own thread and answers `Err`
/// past `wait`. The macOS keychain prompts on a locked keychain, and the
/// prompt can sit for hours while `Backend::resolve` blocks Tauri's setup
/// closure on the main thread (perf-logs §4). Past the bound the launch goes
/// on as "configured hub unusable" — the existing banner, with the reason —
/// and never as standalone (a paired app that guesses standalone is the
/// two-brains failure `backend::startup` exists to prevent). The blocked
/// thread finishes on its own whenever the keychain finally answers.
pub struct BoundedTokenStore {
    inner: Arc<dyn TokenStore>,
    wait: Duration,
}

impl BoundedTokenStore {
    pub fn new(inner: Arc<dyn TokenStore>, wait: Duration) -> Self {
        Self { inner, wait }
    }
}

impl TokenStore for BoundedTokenStore {
    fn get(&self) -> Result<Option<String>, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        let inner = Arc::clone(&self.inner);
        let started = std::time::Instant::now();
        tracing::info!("startup: reading the hub client token (a locked keychain prompts here)");
        std::thread::Builder::new()
            .name("hub-token-read".into())
            .spawn(move || {
                let _ = tx.send(inner.get());
            })
            .map_err(|e| format!("cannot start the token read: {e}"))?;
        match rx.recv_timeout(self.wait) {
            Ok(answer) => {
                tracing::info!(
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    found = matches!(answer, Ok(Some(_))),
                    "startup: hub client token read finished"
                );
                answer
            }
            Err(_) => {
                tracing::warn!(
                    waited_secs = self.wait.as_secs(),
                    "startup: the token store did not answer; continuing with the hub marked \
                     unusable (unlock the keychain and relaunch)"
                );
                Err(format!(
                    "the token store did not answer within {} s (a locked keychain?) — unlock it and relaunch",
                    self.wait.as_secs()
                ))
            }
        }
    }

    fn set(&self, token: &str) -> Result<(), String> {
        self.inner.set(token)
    }

    fn clear(&self) -> Result<(), String> {
        self.inner.clear()
    }
}
```

(`Arc` and `Duration` imports as the file already has them for `OsTokenStore`; add `use std::sync::Arc; use std::time::Duration;` if not.)

`lib.rs:197-200`:

```rust
            let tokens: std::sync::Arc<dyn backend::TokenStore> =
                std::sync::Arc::new(OsTokenStore::new(data_dir.clone()));
            tracing::info!("startup: resolving backend (hub.remote_url, then the client token)");
            let resolving = std::time::Instant::now();
            let backend = Backend::resolve(
                &store,
                &backend::token_store::BoundedTokenStore::new(
                    std::sync::Arc::clone(&tokens),
                    backend::token_store::KEYCHAIN_WAIT,
                ),
            );
            tracing::info!(
                elapsed_ms = resolving.elapsed().as_millis() as u64,
                remote = backend.is_remote(),
                unavailable = backend.unavailable().is_some(),
                "startup: backend resolved"
            );
```

(`backend::token_store` must be `pub mod` in `backend/mod.rs`; export `BoundedTokenStore` and `KEYCHAIN_WAIT` next to `TokenStore` there if the module is private.)

- [ ] **Step 4: Run, expect PASS** — `cargo test -p claude-fleet --lib tests_startup` → both new tests pass; the existing `a_configured_hub_whose_token_cannot_be_read_starts_nothing` still passes.

- [ ] **Step 5: Docs** — under *When the configured hub cannot be used* add a bullet: "the client token could not be read in time — the keychain was locked and its prompt did not answer within 10 s. The app logs `startup: resolving backend` … `startup: backend resolved elapsed_ms=…` around this step; unlock the keychain and relaunch."

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/backend/token_store.rs src-tauri/src/backend/mod.rs src-tauri/src/lib.rs src-tauri/src/backend/tests_startup.rs docs/hub.md
git commit -m "fix(startup): log the backend-resolve steps and bound the keychain read to 10 s"
```

---

### Task 6: Usage day attribution — book by the transcript's UTC day, flag first-cursor backfill, label the two populations (perf-logs §6a/§6b; row 6) — effort M

**Files:**
- Create: `crates/fleet-core/migrations/062_usage_daily_backfill.sql`
- Modify: `crates/fleet-core/src/store/schema.rs` (`MIGRATIONS` after the 060 entry at 586; a guard fn near `worktrees_has_host_alias` at 37; tests near 2797)
- Modify: `crates/fleet-core/src/store/rows.rs` (`UsageDelta` 624-636; add `DayDelta`)
- Modify: `crates/fleet-core/src/store/usage.rs` (`apply_usage_body` 214-306, `usage_daily_since` 310-335, tests 377-560)
- Modify: `crates/fleet-core/src/service/usage.rs` (`AWK` 240-311, `FileRead` 389-400, `parse_batch_output` 444-518, `plan_delta` 520-556, `DayUsage` 790-794, `daily_totals` 811-829, `UsageReport` 851-870, `report` 873-916, tests)
- Modify: `crates/fleet-core/src/mcp/tools/fleet.rs:49-55` (`usage_report` description)
- Modify: `docs/hub.md` (the `fleet_health` paragraph from Task 3; a `usage_report` note under *Setup*)
- Regen: `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract`; `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`

**Interfaces:**
- Consumes: transcript lines' top-level `"timestamp":"YYYY-MM-DDTHH:MM:SS.sssZ"`; `ReadMode` (`Cont|New|Shrink`); `day_string` (`usage.rs:774`).
- Produces: AWK `D\t<sid>\t<model>\t<YYYY-MM-DD>\t<in>\t<out>\t<cw>\t<cr>\t<cw5m>` lines; `FileRead.by_day: Vec<DayModelUsage>`; `pub fn day_number(ymd: &str) -> Option<i64>`; `UsageDelta.by_day: Vec<DayDelta { day: i64, totals: UsageTotals, backfill: bool }>`; `usage_daily(day, host_alias, backfill, …)` keyed by all three; `Store::usage_daily_since -> Vec<(i64, String, bool, UsageTotals)>`; `DayUsage.backfill_cost_micros: i64` (`#[serde(default)]`); `UsageReport.by_host_population = "live_rows"`, `UsageReport.by_day_population = "durable"`.

Contract choice (the brief asked to pick): `by_host` / `by_day` keep their names. A serde alias only helps *deserialising* a renamed input; every reader of `usage_report` (the desktop's `Value` route, the phone, the control skill, the generated reference) keys on the current names, so a rename with aliases would still break them. Two additive string fields say what each population is; `DayUsage` gains one additive number. Nothing is renamed, `CONTRACT_REVISION` stays 4.

`backfill` definition: a row is backfill when the read started at byte 0 of a transcript that already existed (`mode` is `new` or `shrink`, so nothing was ever counted from it) **and** the line's day is earlier than the collection day. A brand-new session's first read is today's live usage; a takeover of a week-old transcript books six days of history as backfill and today's slice as live. A `cont` read is live whatever day the line carries.

The usage tests build transcripts inline (`line_with` / `assistant`, `usage.rs:1080-1101`); there is no `service/testdata` usage fixture on main (only `pane_intel`), so the day-attribution tests extend those builders.

- [ ] **Step 1: Failing AWK test**

In `usage.rs` `mod tests`, add a timestamp to the line builder — change `line_with` to take `ts: &str` and emit `"requestId":"req_1","timestamp":"{ts}","type":"assistant","uuid":"u"` at the end of the JSON; `assistant(...)` passes `"2026-01-01T00:00:00.000Z"`, and add:

```rust
    #[allow(clippy::too_many_arguments)]
    fn assistant_at(id: &str, model: &str, ts: &str, i: i64, o: i64, w: i64, r: i64, block: &str) -> String {
        line_with(id, model, ts, i, o, w, 0, r, block)
    }

    /// perf-logs §6a: the reader summed a chunk into one bucket, so a first
    /// read booked a transcript's whole history on the collection day.
    #[test]
    fn awk_splits_usage_by_the_lines_utc_day() {
        let fx = fixture();
        let head = format!(
            "{}\n{}\n{}\n",
            assistant_at("msg_1", "claude-opus-5", "2026-09-18T23:59:59.000Z", 10, 1, 0, 0, "a"),
            assistant_at("msg_2", "claude-opus-5", "2026-09-19T00:00:01.000Z", 20, 2, 0, 0, "b"),
            assistant_at("msg_3", "claude-sonnet-5", "2026-09-19T08:00:00.000Z", 5, 5, 0, 0, "c"),
        );
        append(&fx.file, &head);
        let r = read(run(&fx.home, &cursor(Some(&fx.file), 0, None, None), MAX_CHUNK_BYTES));
        let by_day: Vec<(String, String, i64)> = r
            .by_day
            .iter()
            .map(|d| (d.day.clone().unwrap(), d.model.clone().unwrap(), d.totals.input_tokens))
            .collect();
        assert_eq!(
            by_day,
            vec![
                ("2026-09-18".into(), "claude-opus-5".into(), 10),
                ("2026-09-19".into(), "claude-opus-5".into(), 20),
                ("2026-09-19".into(), "claude-sonnet-5".into(), 5),
            ]
        );
        assert_eq!(
            model_totals(&r, "claude-opus-5"),
            tokens(30, 3, 0, 0),
            "the per-model totals are unchanged"
        );
    }

    #[test]
    fn day_number_inverts_day_string() {
        for n in [0, 19_000, 20_714, 20_716, 25_000] {
            assert_eq!(day_number(&day_string(n)), Some(n), "day {n}");
        }
        assert_eq!(day_number("2026-09-18"), Some(20_714));
        assert_eq!(day_number("?"), None);
        assert_eq!(day_number("2026-13-01"), None);
    }
```

- [ ] **Step 2: Run, expect a compile error** — `cargo test -p fleet-core awk_splits_usage_by` → `no field by_day on type FileRead`, `day_number` not found.

- [ ] **Step 3: Implement the reader, the parser and `day_number`**

`AWK`: in `take()`, after `lastm = m` and the `if (di + dq + dw + dr + d5 == 0) return` line, add the per-day sums (the ERE avoids `{4}` intervals, which BSD awk and mawk do not all support; no single quote anywhere):

```awk
  d = ""
  if (match(line, /"timestamp":"[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]/)) d = substr(line, RSTART + 13, 10)
  if (d == "") d = "?"
  key = m SUBSEP d
  if (!(key in dseen)) { dseen[key] = 1; dorder[++nd] = key; dm[key] = m; dd[key] = d }
  xi[key] += di
  xo[key] += dq
  xw[key] += dw
  xr[key] += dr
  x5[key] += d5
```

(`d`, `key` join `take`'s local list.) In `END`, between the `U` loop and the `E` line:

```awk
  for (j = 1; j <= nd; j++) { key = dorder[j]; printf "D\t%s\t%s\t%s\t%.0f\t%.0f\t%.0f\t%.0f\t%.0f\n", sid, dm[key], dd[key], xi[key], xo[key], xw[key], xr[key], x5[key] }
```

Rust, `service/usage.rs`:

```rust
/// One (model, UTC day) slice of a file's read — the `D` lines.
#[derive(Debug, Clone, PartialEq)]
pub struct DayModelUsage {
    /// `YYYY-MM-DD` from the line's `timestamp`; `None` when the line had none.
    pub day: Option<String>,
    pub model: Option<String>,
    pub totals: UsageTotals,
    pub cache_write_5m_tokens: i64,
}
```

`FileRead` gains `pub by_day: Vec<DayModelUsage>` (initialised `Vec::new()` in `parse_batch_output`'s `F` arm); add the arm

```rust
            ["D", _, model, day, i, o, w, r, w5] => {
                if let Some(read) = pending.get_mut(&sid) {
                    read.by_day.push(DayModelUsage {
                        day: (*day != "?").then(|| day.trim().to_string()),
                        model: clean_model(model),
                        totals: UsageTotals {
                            input_tokens: count(i).unwrap_or(0),
                            output_tokens: count(o).unwrap_or(0),
                            cache_write_tokens: count(w).unwrap_or(0),
                            cache_read_tokens: count(r).unwrap_or(0),
                            cost_micros: 0,
                        },
                        cache_write_5m_tokens: count(w5).unwrap_or(0),
                    });
                }
            }
```

and next to `day_string`:

```rust
/// UTC day number of a `YYYY-MM-DD` (the inverse of [`day_string`]);
/// `None` for anything else. Howard Hinnant's days_from_civil.
pub fn day_number(ymd: &str) -> Option<i64> {
    let mut it = ymd.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    if it.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}
```

- [ ] **Step 4: Run, expect PASS** — `cargo test -p fleet-core awk_ day_number` → both pass and `awk_sums_usage_dedupes_blocks_and_leaves_a_truncated_last_line` still passes.

- [ ] **Step 5: Failing `plan_delta` test**

```rust
    #[test]
    fn plan_delta_attributes_by_transcript_day_and_flags_history_on_a_fresh_cursor() {
        let opus = |day: &str, i: i64| DayModelUsage {
            day: Some(day.into()),
            model: Some("claude-opus-5".into()),
            totals: tokens(i, 0, 0, 0),
            cache_write_5m_tokens: 0,
        };
        let read = FileRead {
            mode: ReadMode::New,
            start: 0,
            chunk: 100,
            source: "x.jsonl".into(),
            by_model: vec![ModelUsage {
                model: Some("claude-opus-5".into()),
                totals: tokens(3, 0, 0, 0),
                cache_write_5m_tokens: 0,
            }],
            by_day: vec![opus("2026-09-18", 2), opus("2026-09-21", 1)],
            consumed: 100,
            last_msg_id: None,
            last_model: None,
            last_msg_usage: None,
        };
        let now = day_number("2026-09-21").unwrap() * SECS_PER_DAY + 3_600;
        let d = plan_delta(&read, &BTreeMap::new(), MAX_CHUNK_BYTES, now);
        assert_eq!(
            d.by_day
                .iter()
                .map(|x| (x.day, x.backfill, x.totals.input_tokens, x.totals.cost_micros))
                .collect::<Vec<_>>(),
            vec![
                (day_number("2026-09-18").unwrap(), true, 2, 10),
                (day_number("2026-09-21").unwrap(), false, 1, 5),
            ],
            "history before the collection day on a fresh cursor is backfill; today's slice is live"
        );
        let mut cont = read.clone();
        cont.mode = ReadMode::Cont;
        cont.start = 50;
        let d = plan_delta(&cont, &BTreeMap::new(), MAX_CHUNK_BYTES, now);
        assert!(
            d.by_day.iter().all(|x| !x.backfill),
            "a continuing cursor is live usage whatever day the line carries"
        );
        let mut undated = read.clone();
        undated.by_day[0].day = None;
        let d = plan_delta(&undated, &BTreeMap::new(), MAX_CHUNK_BYTES, now);
        assert_eq!(d.by_day[0].day, now.div_euclid(SECS_PER_DAY), "a line without a timestamp books to today");
    }
```

(`10` and `5` micro-USD: opus input is $5/MTok → 2 tokens = 10 µ$, 1 token = 5 µ$ — matches `BUILTIN_PRICES` `opus 5/25/10/0.5`.)

- [ ] **Step 6: Run, expect a compile error** — `no field by_day on type UsageDelta`.

- [ ] **Step 7: Implement `DayDelta` and `plan_delta`**

`store/rows.rs`, before `UsageDelta`:

```rust
/// One UTC day's slice of a [`UsageDelta`], priced. `backfill` marks history
/// a fresh cursor read in one go (perf-logs §6a): kept apart in
/// `usage_daily` so a takeover day never reads as a $850 day.
#[derive(Debug, Clone, PartialEq)]
pub struct DayDelta {
    pub day: i64,
    pub totals: UsageTotals,
    pub backfill: bool,
}
```

`UsageDelta` gains `/// Per-day slices of `totals`; empty means "book everything to the day of `now`" (a reader without `D` lines). pub by_day: Vec<DayDelta>,`. Every `UsageDelta {` literal gains `by_day: Vec::new()` (`usage.rs:1995` test; the `store/usage.rs` tests) except `plan_delta`, which builds it:

```rust
    let today = now.div_euclid(SECS_PER_DAY);
    let fresh = read.mode != ReadMode::Cont;
    let mut by_day: BTreeMap<(i64, bool), UsageTotals> = BTreeMap::new();
    for dm in &read.by_day {
        let mut t = dm.totals;
        t.cost_micros = dm
            .model
            .as_deref()
            .and_then(|m| price_for(m, overrides))
            .map(|p| cost_micros(&t, dm.cache_write_5m_tokens, p))
            .unwrap_or(0);
        let day = dm.day.as_deref().and_then(day_number).unwrap_or(today);
        let backfill = fresh && day < today;
        by_day.entry((day, backfill)).or_default().add(&t);
    }
    let by_day = by_day
        .into_iter()
        .map(|((day, backfill), totals)| DayDelta { day, totals, backfill })
        .collect();
```

and `by_day` in the returned struct.

- [ ] **Step 8: Run, expect PASS** — `cargo test -p fleet-core plan_delta_attributes` green.

- [ ] **Step 9: Failing store test and migration test**

Create `crates/fleet-core/migrations/062_usage_daily_backfill.sql`:

```sql
-- 062: usage_daily keyed by (day, host_alias, backfill). A first read of an
-- existing transcript books that transcript's whole history in one pass;
-- those rows are `backfill = 1` and reported apart (perf-logs §6a). Existing
-- rows become live (backfill = 0) rows. Guarded by `usage_daily_has_backfill`
-- in schema.rs, so a re-run cannot collapse backfill rows into live ones.
CREATE TABLE IF NOT EXISTS usage_daily_v2 (
    day INTEGER NOT NULL,
    host_alias TEXT NOT NULL,
    backfill INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cost_micros INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (day, host_alias, backfill)
);
INSERT OR IGNORE INTO usage_daily_v2
    (day, host_alias, backfill, input_tokens, output_tokens, cache_write_tokens, cache_read_tokens, cost_micros)
    SELECT day, host_alias, 0, input_tokens, output_tokens, cache_write_tokens, cache_read_tokens, cost_micros
    FROM usage_daily;
DROP TABLE usage_daily;
ALTER TABLE usage_daily_v2 RENAME TO usage_daily;
```

In `schema.rs` `mod tests`:

```rust
    #[test]
    fn migration_062_rekeys_usage_daily_by_backfill_and_keeps_the_rows() {
        const SEED_AT: i64 = 60;
        let s = store_at_version(SEED_AT);
        s.conn
            .execute_batch(
                "INSERT INTO usage_daily (day, host_alias, input_tokens, output_tokens, \
                 cache_write_tokens, cache_read_tokens, cost_micros) VALUES (20714, 'trn', 1, 2, 3, 4, 5);",
            )
            .unwrap();
        assert!(!usage_daily_has_backfill(&s.conn).unwrap());
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(usage_daily_has_backfill(&s.conn).unwrap());
        let (backfill, cost): (i64, i64) = s
            .conn
            .query_row(
                "SELECT backfill, cost_micros FROM usage_daily WHERE day = 20714 AND host_alias = 'trn'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((backfill, cost), (0, 5), "existing rows are live rows");
        // The new key admits a backfill row beside the live one for the same day.
        s.conn
            .execute(
                "INSERT INTO usage_daily (day, host_alias, backfill, cost_micros) VALUES (20714, 'trn', 1, 7)",
                [],
            )
            .unwrap();
        let n: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM usage_daily WHERE day = 20714", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }
```

In `store/usage.rs` `mod tests`:

```rust
    #[test]
    fn apply_usage_books_each_transcript_day_and_keeps_backfill_apart() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("vps").unwrap();
        s.upsert_session("a", "vps", None, None, 1, 1, "running", None).unwrap();
        let id = s.get_session("a", "vps").unwrap().unwrap().id;
        let t = |cost: i64| UsageTotals {
            input_tokens: 1,
            output_tokens: 1,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            cost_micros: cost,
        };
        let mut all = t(100);
        all.add(&t(7));
        let d = UsageDelta {
            reset: false,
            totals: all,
            model: Some("claude-opus-5".into()),
            offset: 10,
            source: "x.jsonl".into(),
            last_msg_id: None,
            last_msg_usage: None,
            now: 20_716 * 86_400 + 5,
            by_day: vec![
                DayDelta { day: 20_714, totals: t(100), backfill: true },
                DayDelta { day: 20_716, totals: t(7), backfill: false },
            ],
        };
        assert!(s.apply_usage(id, "vps", &d).unwrap());
        let rows: Vec<(i64, bool, i64)> = s
            .usage_daily_since(0, Some("vps"))
            .unwrap()
            .into_iter()
            .map(|(day, _, backfill, t)| (day, backfill, t.cost_micros))
            .collect();
        assert_eq!(rows, vec![(20_714, true, 100), (20_716, false, 7)]);
        let days = crate::service::usage::daily_totals(&s, 0, Some("vps")).unwrap();
        assert_eq!(days[0].day, "2026-09-18");
        assert_eq!(days[0].backfill_cost_micros, 100);
        assert_eq!(days[0].totals.cost_micros, 0, "backfill never inflates the live day");
        assert_eq!(days[1].totals.cost_micros, 7);
        assert_eq!(days[1].backfill_cost_micros, 0);
        // A delta without day slices books to the day of `now`, as before.
        let plain = UsageDelta { by_day: Vec::new(), totals: t(1), offset: 11, ..d.clone() };
        assert!(s.apply_usage(id, "vps", &plain).unwrap());
        let live_today = s
            .usage_daily_since(20_716, Some("vps"))
            .unwrap()
            .into_iter()
            .find(|(_, _, b, _)| !b)
            .unwrap()
            .3
            .cost_micros;
        assert_eq!(live_today, 8);
    }
```

- [ ] **Step 10: Run, expect failures** — `cargo test -p fleet-core migration_062 apply_usage_books` → `usage_daily_has_backfill` not found; `no field by_day`; the tuple pattern `(day, _, backfill, t)` does not match a 3-tuple.

- [ ] **Step 11: Implement the store side**

`schema.rs`: register after the 060 entry

```rust
    // usage_daily keyed by (day, host_alias, backfill): a table rebuild, so
    // guarded — re-running the INSERT…SELECT would collapse backfill rows.
    Migration {
        version: 61,
        sql: include_str!("../../migrations/062_usage_daily_backfill.sql"),
        already_applied: Some(usage_daily_has_backfill),
    },
```

and near `worktrees_has_host_alias`:

```rust
/// `already_applied` guard of migration 062.
fn usage_daily_has_backfill(conn: &Connection) -> rusqlite::Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('usage_daily') WHERE name = 'backfill'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}
```

`store/usage.rs`: factor the upsert into

```rust
    fn add_usage_daily(
        &self,
        day: i64,
        host_alias: &str,
        backfill: bool,
        t: &UsageTotals,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO usage_daily (day, host_alias, backfill, input_tokens, output_tokens, \
             cache_write_tokens, cache_read_tokens, cost_micros) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT(day, host_alias, backfill) DO UPDATE SET \
             input_tokens = input_tokens + excluded.input_tokens, \
             output_tokens = output_tokens + excluded.output_tokens, \
             cache_write_tokens = cache_write_tokens + excluded.cache_write_tokens, \
             cache_read_tokens = cache_read_tokens + excluded.cache_read_tokens, \
             cost_micros = cost_micros + excluded.cost_micros",
            rusqlite::params![
                day,
                host_alias,
                backfill as i64,
                t.input_tokens,
                t.output_tokens,
                t.cache_write_tokens,
                t.cache_read_tokens,
                t.cost_micros
            ],
        )?;
        Ok(())
    }
```

and replace the `if !daily.is_zero() { self.conn.execute(…) }` block in `apply_usage_body` with:

```rust
        // A rewritten file (`reset`) has one growth figure that cannot be
        // split by day; a reader without `D` lines has none — both book to
        // the day of `now`, as before. Otherwise each day's slice goes to
        // its own row, history from a fresh cursor to the backfill row.
        if d.reset || d.by_day.is_empty() {
            if !daily.is_zero() {
                self.add_usage_daily(d.now.div_euclid(86_400), host_alias, false, &daily)?;
            }
        } else {
            for slice in &d.by_day {
                if !slice.totals.is_zero() {
                    self.add_usage_daily(slice.day, host_alias, slice.backfill, &slice.totals)?;
                }
            }
        }
```

`usage_daily_since` selects `backfill` too and returns `Vec<(i64, String, bool, UsageTotals)>` (`r.get::<_, i64>(2)? != 0`), `ORDER BY day, host_alias, backfill`; update its three test call sites (`store/usage.rs:377, 417, 553-560`) to the 4-tuple. Derive `Clone` on `UsageDelta` if it is not already (the test uses `..d.clone()`).

`service/usage.rs`: `DayUsage` gains `/// Cost of history a fresh cursor booked to this day (`backfill = 1` rows) — not in `totals`. #[serde(default)] pub backfill_cost_micros: i64,`; `daily_totals` becomes

```rust
    let mut by: BTreeMap<i64, (UsageTotals, i64)> = BTreeMap::new();
    for (day, _host, backfill, t) in s.usage_daily_since(since_day, host)? {
        let e = by.entry(day).or_default();
        if backfill {
            e.1 += t.cost_micros;
        } else {
            e.0.add(&t);
        }
    }
    Ok(by
        .into_iter()
        .map(|(day, (totals, backfill_cost_micros))| DayUsage {
            day: day_string(day),
            totals,
            backfill_cost_micros,
        })
        .collect())
```

Every other `DayUsage {` literal (`grep -rn "DayUsage {" crates`) gains `backfill_cost_micros: 0`.

- [ ] **Step 12: Run, expect PASS** — `cargo test -p fleet-core usage` and `cargo test -p fleet-core migration_062` green.

- [ ] **Step 13: Label the populations**

`UsageReport` gains, after `note`:

```rust
    /// What `total` and `by_host` count: the session rows that still exist,
    /// each over its whole lifetime — ghosts included, GC'd sessions gone.
    pub by_host_population: &'static str,
    /// What `by_day` counts: the durable daily roll-up, killed sessions
    /// included; `backfill_cost_micros` is history a first read booked.
    pub by_day_population: &'static str,
```

with `pub const POPULATION_LIVE_ROWS: &str = "live_rows"; pub const POPULATION_DURABLE: &str = "durable";` and `report` filling them. Test:

```rust
    #[test]
    fn the_report_names_what_each_population_counts() {
        let s = Store::open_in_memory().unwrap();
        let v = serde_json::to_value(report(&s, None, None, 1_000_000).unwrap()).unwrap();
        assert_eq!(v["by_host_population"], "live_rows");
        assert_eq!(v["by_day_population"], "durable");
    }
```

`fleet.rs:49-55` `usage_report` description: append the clause `by_day carries backfill_cost_micros (history a first read booked) apart from the day's live cost.` Then `REGEN_DOCS=1 cargo test -p fleet-core reference_is_current`, `cargo test -p fleet-core the_served_definition_budget_stays_bounded`, and `REGEN_HUB_CONTRACT=1 cargo test -p claude-fleet --lib contract` (the `DayUsage` row gains `backfill_cost_micros`), then the plain run → green.

- [ ] **Step 14: Docs** — `docs/hub.md`, under the `fleet_health` paragraph (Task 3): "`usage_by_day` books each token to the UTC day of the transcript line that produced it. The first time a transcript is read (a new host, a hub takeover) its history before that day lands in `backfill_cost_micros`, apart from the day's live `cost_micros`, so a takeover never reads as an $850 day. `usage_report` says what each figure counts: `by_host_population: live_rows` (session rows that still exist, over their lifetime — ghosts included) and `by_day_population: durable` (the daily roll-up, killed sessions included); the two need not agree."

- [ ] **Step 15: Commit**

```bash
git add crates/fleet-core/migrations/062_usage_daily_backfill.sql crates/fleet-core/src/store/schema.rs crates/fleet-core/src/store/rows.rs crates/fleet-core/src/store/usage.rs crates/fleet-core/src/service/usage.rs crates/fleet-core/src/mcp/tools/fleet.rs src-tauri/src/backend/hub_contract.golden.json docs/control-api-reference.md docs/hub.md
git commit -m "feat(usage): book tokens to the transcript's UTC day, keep first-read backfill apart, and label the report's two populations"
```

---

### Task 7: Reconcile writes stream per host as each probe completes (perf-logs §3; row 25, first half) — effort S

**Files:**
- Modify: `crates/fleet-core/src/service/sessions/reconcile.rs:1546-1621` (`reconcile_sessions_with` steps 2-3)
- Modify: `crates/fleet-core/src/service/sessions/tests.rs` (next to `fleet_reconcile_completes_when_one_host_never_answers`, line 1991)
- Modify: `docs/hub.md` *Troubleshooting*

**Interfaces:**
- Consumes: `JoinSet::join_next`, `reconcile_write_one_host` (`reconcile.rs:584`), `scripted_deps` / `ScriptedTmux` (`tests.rs:1959-1987`).
- Produces: the same rows and events per host, written as that host's probe completes; a slow host delays only itself.

- [ ] **Step 1: Failing test**

```rust
/// perf-logs §3: the pass joined EVERY probe before writing any host, so one
/// 65 s probe timeout froze the freshness of the whole fleet (63 such
/// timeouts in the log window). A host's rows now land as its probe ends.
#[tokio::test]
async fn a_fast_hosts_rows_land_while_a_slow_host_is_still_being_probed() {
    use std::time::Duration;
    let store = Arc::new(Mutex::new(Store::open_in_memory().expect("store")));
    {
        let s = store.lock().unwrap();
        s.upsert_host("wedged").unwrap();
    }
    let (deps, _probes) = scripted_deps(
        vec![tmux_session("local-live")],
        Duration::from_millis(10),
        Duration::from_millis(600),
    );
    let pass = {
        let store = Arc::clone(&store);
        tokio::spawn(async move { reconcile_sessions_with(&store, &deps).await })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!pass.is_finished(), "the wedged host is still inside its probe budget");
    {
        let s = store.lock().unwrap();
        let local = s.list_sessions_for_host("local").unwrap();
        assert_eq!(
            local.iter().map(|r| r.tmux_name.as_str()).collect::<Vec<_>>(),
            vec!["local-live"],
            "the healthy host's rows are written before the slow host's probe ends"
        );
    }
    pass.await
        .unwrap()
        .expect("the pass completes once the slow host times out");
    let s = store.lock().unwrap();
    let wedged = s
        .list_hosts()
        .unwrap()
        .into_iter()
        .find(|h| h.alias == "wedged")
        .unwrap();
    assert!(!wedged.reachable, "the timed-out host is still marked unreachable");
}
```

- [ ] **Step 2: Run, expect the freshness assertion to fail**

`cargo test -p fleet-core a_fast_hosts_rows_land` → `assertion left == right … left: [] right: ["local-live"]` (no host is written until the 600 ms join completes).

- [ ] **Step 3: Implement**

In `reconcile_sessions_with`, move the `let projects = { … }` block (lines 1596-1599) above the `JoinSet` fan-out, and replace the collect loop plus the write loop (1565-1620) with:

```rust
    // 3. Apply each host's result AS ITS PROBE COMPLETES, taking the store
    //    lock once per host (BE-12), rather than after every host has
    //    joined: `HOST_PROBE_TIMEOUT` (65 s) is far past the 20 s tick, and
    //    one wedged host used to hold every other host's rows back for that
    //    long (perf-logs §3). The write is unchanged — one
    //    `Store::atomically` transaction per host in `reconcile_write_one_host`,
    //    events held until it commits, a failed host rolled back alone.
    while let Some(join) = set.join_next().await {
        let probe = match join {
            Ok(probe) => probe,
            Err(e) => {
                tracing::error!(error = %e, "[reconcile] probe task panicked");
                continue;
            }
        };
        {
            let mut s = lock(store)?;
            if let Err(e) = reconcile_write_one_host(&mut s, &probe, &projects) {
                tracing::error!(
                    host = %probe.host.alias,
                    error = %e,
                    "[reconcile] write failed (rolled back; retried next pass)"
                );
            }
        }
        // The guard is dropped above; give a waiting reader its turn (Task 4):
        // `std::sync::Mutex` is not fair, and re-locking straight away for
        // the next host usually beats a reader already blocked on it.
        tokio::task::yield_now().await;
    }
    Ok(())
```

Update the function's doc comment ("probe every non-hidden host in parallel, then apply each host's result …" → "…apply each host's result as its probe completes, under its own short store-lock window").

- [ ] **Step 4: Run, expect PASS** — `cargo test -p fleet-core a_fast_hosts_rows_land` → `1 passed`; `cargo test -p fleet-core sessions::` green (`fleet_reconcile_completes_when_one_host_never_answers` and the BE-3 `probe_started_at` tests still hold: the per-host write and its timestamps are unchanged).

- [ ] **Step 5: Docs** — `docs/hub.md` *Troubleshooting*, one paragraph: "**One host is slow; everything looks stale.** A reconcile pass probes every host in parallel and writes each host's rows the moment its probe answers, so a host that takes the full 65 s probe budget delays only its own freshness; `fleet_health.hub.reconcile.last_duration_ms` still shows the pass as slow, and the host's `[reconcile] host probe exceeded its wall clock` line names it."

- [ ] **Step 6: Commit**

```bash
git add crates/fleet-core/src/service/sessions/reconcile.rs crates/fleet-core/src/service/sessions/tests.rs docs/hub.md
git commit -m "perf(reconcile): write each host's rows as its probe completes instead of after the slowest host"
```

---

## Self-review

**Spec coverage**

| Finding | Task |
|---|---|
| hub-ops F7 nightly `.backup` + integrity + retention; restore drill | 1 |
| hub-ops F2 compose in git with `.env` tag; `upgrade.sh <ver>` (pull → backup → stop → tag → up → healthz → keep 3); 15-copy cleanup | 1 (variant compose + runbook; the shipped compose keeps its literal pin because `release.sh`/`check-version-consistency.sh` own it) |
| hub-ops F1 upgrade order checklist | 1 (docs) |
| hub-ops F6 `known_hosts` / key-rotation runbook | 1 (docs) |
| hub-ops F9 compose drift (logging caps, AppleDouble, env perms) | 1 (variant carries the caps; runbook) + 3 (logs mount) |
| hub-ops F3 proxy network, no `ports:`; startup WARN; per-IP 401 limiter; peer in the reject line | 2 |
| hub-ops F5 logs without sudo; `fleet_health.hub{uptime, reconcile{…}}`; `/metrics` gauges; Kuma with a readonly token | 3 |
| hub-ops F8 `peer_links_total` | 3 |
| perf-logs §1 `tunnels_mode` | 3 |
| perf-logs §5 usage failures at WARN; stuck/playbook transitions with host+session | 3 |
| perf-logs §4 `Last-Event-ID`, resync only on `resumed:false`, resync covers projects/worktrees/work | 4 |
| perf-logs §4 startup: log the resolve step, bound the keychain wait | 5 |
| perf-logs §6a day attribution + first-cursor backfill | 6 |
| perf-logs §6b `by_host` vs `by_day` labelled | 6 |
| perf-logs §3 stream per-host writes | 7 |
| README rows 6, 13, 14, 19, 20, 21, 24, 25 (first half) | 6, 1, 2, 4, 5, 3, 3(+1 docs), 7 |

**Already on main, no code needed:** `FLEET_HUB_LOG_DIR` (`config.rs:415-419`); the hub's `Last-Event-ID` / `?since=` replay, per-frame `id:` and `ready.resumed` (`events_route.rs:520, 543, 682`; tested at `mcp/mod.rs:1994/2020`).

**Deliberately left out, with the reason**

- *Showing the window before resolve completes* (row 20): `Backend::resolve` runs inside Tauri's `setup` closure (`lib.rs:99-199`) before the event loop, and `FleetBackend` must be managed before any command runs; painting first needs a `Resolving` backend state across all 173 command verdicts (`backend/verdicts.rs`). Task 5 bounds the wait to 10 s and logs every step instead, which is what turns "a dead app" into "a banner in 10 s".
- *Falling back to local mode on a keychain timeout* (brief wording): refused by the two-brains rule (`mod.rs:277-278`, `tests_startup.rs:221`); Task 5 yields `Unavailable` with the reason.
- *`${FLEET_HUB_TAG}` in `deploy/hub/docker-compose.yml`*: `release.sh --list-image-pin` and `check-version-consistency.sh:244` require the literal pin there; the `.env` form lives in the behind-proxy variant.
- *Wiring `hub-deploy-scripts-test.sh` into `.github/workflows/ci.yml`*: the workflow was not read for this plan; it is wired into `ci-local.sh`, and the version-consistency job is its natural home.
- *`fleet-hub backup` subcommand* (F7 optional M) and *`fleet-hub ssh-key --rotate`* (F6 M): not in the seven ordered tasks; the runbooks cover the manual paths.
- *Cloudflare rate rule, Uptime Kuma monitors, the NAS cleanup itself, `chmod` of `settings.json.bak`*: operations on the NAS/Mac, documented, never run by the repo.
- *perf-logs §2 residual (status_change dedupe, hook-recorded transitions, trim to GC)* and the rest of row 25: lifecycle F9/F10, the lifecycle slice's plan.
- *A size cap beside `MAX_LOG_FILES`* (perf §5 P3) and *the idle peer supervisor's slower rescan* (F8 optional): hygiene below the P2 line.
