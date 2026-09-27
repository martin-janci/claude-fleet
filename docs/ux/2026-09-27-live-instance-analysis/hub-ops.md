# Hub daemon / deployment / operations review — fleet.rlt.sk (2026-09-27)

Read-only review of the live `fleet-hub` 0.3.1 on `nas` (`/volume1/docker/fleet-hub`),
the desktop 0.2.42 paired to it, and the `fleet-agent` 0.2.26 on `claude-fleet-trn`.
Sources: repo worktree (branch is stale at 0.2.39; changelog/tag facts were read from
`origin/main` and the `v0.2.26`…`v0.3.1` tags with `git show`, no checkout),
`ssh nas` read-only listing (no sudo, no docker, nothing under `./ssh`, `./data`,
`fleet-hub.env`), and unauthenticated `curl https://fleet.rlt.sk/{healthz,health,version,metrics}`.

Severity: P0 = fix now, P1 = this week, P2 = next maintenance window, P3 = hygiene.

---

## F1 — Version triangle: desktop 0.2.42 ↔ hub 0.3.1 ↔ agent 0.2.26

**Severity:** P2 (nothing is broken today; the exposure is procedural)

**Evidence**
- Contract constants per tag (`git show <tag>:…`):

  | tag | `CONTRACT_REVISION` (fleet-core `wire_contract.rs`) | desktop `MIN..=MAX_HUB_CONTRACT` (`src-tauri/src/backend/contract.rs`) | `PROTO_VERSION` / `MIN_SUPPORTED_PROTO` (`crates/fleet-proto/src/lib.rs`) |
  |---|---|---|---|
  | v0.2.26 | 1 | 0..=1 | 1 / 1 |
  | v0.2.42 | 4 | 4..=4 | 1 / 1 |
  | v0.3.0  | 4 | 4..=4 | 1 / 1 |
  | v0.3.1  | 4 | 4..=4 | 1 / 1 |

- `git diff v0.2.42 v0.3.1 -- crates/fleet-core/src/wire_contract.rs src-tauri/src/backend/hub_contract.golden.json src-tauri/src/backend/contract.rs docs/control-api-reference.md crates/fleet-proto` touches only `crates/fleet-proto/Cargo.toml` (version bump). The tool list (`### ` headings of the reference) and the golden row shapes are byte-identical between the desktop's build and the hub's build.
- `origin/main:CHANGELOG.md` 0.3.0: "adds migrations 059 and 060", read-only connection pool, auth-epoch cache, `BEGIN IMMEDIATE`, one-transaction-per-host reconcile writes, `list_sessions` refreshes in the background. 0.3.1: tracker sync fix, release pipeline, no migrations. Neither release changes a wire shape.
- Changelog 0.2.28…0.2.41 wire-relevant items: contract revision 2 (tagged `move_session` result + `dry_run`), revision 3 (`when` argument), revision 4 (`ConvItem` kinds `bash`/`harness`, `session_activity` as a hub tool) — all already in 0.2.42; `AgentFrame::Report` (error batches from agents, "agent: report error-level events to the hub on the heartbeat", "batch report frames by bytes"), `POST /report` / `GET /reports`, `/metrics`, `/mcp/json`, peer links, `get_settings`/`set_setting` (0.2.42), "store: refuse a database a newer build has migrated" (0.2.42).
- `docs/hub.md` "Upgrade a desktop and its hub together from contract revision 3": "There is no mixed window in which the two work together" when the revision moves. Today both sides sit at 4, so the desktop's `ContractFit` is `InRange`.
- The `schema_version` vs `LATEST_SCHEMA_VERSION` assertion is a unit test only (`crates/fleet-core/src/service/health.rs:498`), not a runtime gate: a 0.2.42 desktop is not refused by a schema-60 hub. The 0.2.42 "refuse a database a newer build has migrated" guard only bites if a schema-60 `state.db` is ever opened by the 0.2.42 desktop in *local* mode.
- Agent: `fleet-proto` has had `PROTO_VERSION = MIN_SUPPORTED_PROTO = 1` and the `welcome` frame since v0.2.26, so the 0.2.26 agent's `hello{proto:1}` is inside the hub's `1..=1` window (`judge_proto`), and the hub does not gate on `agent_version` (it only records it, `crates/fleet-core/src/agent/registry.rs:238`). The agent has been connected since 2026-09-26 20:24 UTC, which confirms it.

**Root cause / assessment**
- The 0.2.42 desktop can see *every* hub tool and field a 0.3.1 hub serves. What it lacks is not on the wire: it is the 0.3.x *desktop-side* fixes (none are wire-visible) and, more importantly, it is one revision bump away from an `E_HUB_CONTRACT` banner with no mixed window — the next release that bumps `CONTRACT_REVISION` will refuse until both sides move.
- The 0.2.26 agent is 16 releases behind but protocol-compatible. What the hub does not get from it: `Report` frames (its error-level events never reach `GET /reports` / `fleet-hub reports`), the byte-bounded report batching, and the "offline check bounds the budget" fix. Unknown frame kinds are tolerated on both sides by design (`docs/hub.md` "Protocol version negotiation"), so the old agent is silent, not broken.

**Recommended upgrade order (today, both windows are single-version, so order is about habit, not necessity)**
1. Take a real backup of `state.db` (see F7) — the hub is already at 0.3.1, no hub step this round.
2. Desktop → 0.3.1. Contract stays 4..=4 → no banner. Do this before any release that bumps the revision lands, and from then on treat desktop+hub as one unit: when the changelog says the revision moved, upgrade **hub first, then the desktop within the same window** (the desktop shows "update this app" until it moves; the hub-first order keeps the phone client and hooks alive throughout).
3. `fleet-agent` on `claude-fleet-trn` → 0.3.1 (`fleet-agent install` re-run with the same token; sessions survive, the agent only proxies). At proto 1 either order works; the documented safe order for a future proto bump is **hub first** while `MIN_SUPPORTED_PROTO` is held at the previous version.
4. Verify: `fleet_health.version`, `GET /healthz` = `fleet-hub ok`, `agent_status` shows `agent_version 0.3.1`, desktop banner absent.

**Effort:** S (operational) — plus S in the repo for a `docs/hub.md` "Upgrade checklist" that states the order in one place (today it is split between §Protocol version negotiation, §Upgrade a desktop and its hub together, and the 0.3.0 changelog note).

---

## F2 — NAS deploy hygiene: 15 compose copies, 4 `data.pre-*` dirs, 2 `backup-*` dirs

**Severity:** P3 (clutter, not risk) — but the *procedure* it documents is P1 (see F7)

**Evidence** (`ssh nas ls -la /volume1/docker/fleet-hub`, `du -sh`, `diff`)
- `docker-compose.yml.0.2.26 … .0.3.0` (15 files, 1636 B each): `diff` between any two = the `image:` tag line only. The live `docker-compose.yml` differs from `.0.3.0` by the tag only.
- `data.pre-0.2.38`, `data.pre-0.2.40`, `data.pre-0.2.41`, `data.pre-0.2.42`: uid 1000, mode 0700 → unreadable to the operator account (uid 1027 `mjanci`), `du` reports 0. `data`, `data.pre-0.2.41` and `data.pre-0.2.42` share the identical mtime `2026-09-25 20:24:19.426` — they are `cp -a` snapshots taken after 0.2.40.
- `backup-0.2.42-20260926-175756/` and `backup-0.3.0-20260926-222357/`: 7.7 M each, contents `state.db` (3,584,000 B) + `state.db-shm` (32 KiB) + `state.db-wal` (4,383,712 B). In both, `state.db` mtime is 1.5–5 min *older* than the `-wal`/`-shm` mtimes (17:56:05 vs 17:57:53; 22:18:27 vs 22:23:46) → copied with `cp -p` from a database whose WAL had not been checkpointed into the main file, i.e. either still open or not closed cleanly. Each backup is only valid as the *triple*.
- `._docker-compose.yml` (163 B, uid 501): an AppleDouble sidecar from a Mac `scp`/Finder copy.
- The procedure changed mid-stream: `data.pre-<ver>` (whole-dir copies, ≤0.2.42) → `backup-<ver>-<ts>/state.db*` (from 0.3.0), matching the 0.3.0 changelog's "Back up `state.db`, together with any `state.db-wal` / `state.db-shm`".
- Volume: `/volume1` 36 T, 57 % used — space is not the issue.
- Upstream `deploy/hub/docker-compose.yml` vs the NAS file: the NAS copy intentionally drops the bundled caddy and uses a `./data` bind mount (documented in its header), but it also **drops the upstream `logging: json-file max-size 10m / max-file 3` block** (see F5) and the commented resource limits.

**Root cause:** every upgrade is done by hand (`cp docker-compose.yml docker-compose.yml.<old>`; `sed` the tag; copy data) with no script, no retention rule, and the compose file is not under version control.

**Proposed fix**
1. Put the NAS compose under git (dotfiles, next to the caddy/cloudflared ones) with the image tag in a `.env` (`FLEET_HUB_TAG=0.3.1`, `image: ghcr.io/martin-janci/fleet-hub:${FLEET_HUB_TAG}`) and delete the 15 `.yml.<ver>` copies and `._docker-compose.yml`.
2. Move the two `backup-*` triples and the four `data.pre-*` dirs into `backups/pre-upgrade/` after verifying each restores (`sqlite3 <copy>/state.db 'PRAGMA integrity_check'` as root); then keep only the newest 3.
3. `scripts/hub-upgrade.sh <ver>` in the repo (`deploy/hub/upgrade.sh`), run on the NAS as root:
   ```
   set -euo pipefail; cd /volume1/docker/fleet-hub
   NEW=$1; KEEP=${KEEP:-3}
   docker pull ghcr.io/martin-janci/fleet-hub:$NEW                     # 1. pull first (no downtime yet)
   mkdir -p backups && sqlite3 data/state.db ".backup 'backups/pre-$NEW-$(date +%Y%m%d-%H%M%S).db'"   # 2. online, consistent
   docker compose stop fleet-hub                                        # 3. SIGTERM, 30 s grace (F4)
   sed -i "s/^FLEET_HUB_TAG=.*/FLEET_HUB_TAG=$NEW/" .env && docker compose up -d
   for i in $(seq 1 30); do curl -sf http://127.0.0.1:4180/healthz | grep -q 'fleet-hub ok' && break; sleep 1; done
   ls -1t backups/pre-*.db | tail -n +$((KEEP+1)) | xargs -r rm --     # 4. keep N
   ```
   Rollback = `FLEET_HUB_TAG=<old>` + restore the `pre-<ver>.db` (only needed when the new version migrated; 0.3.1 → 0.3.0 would need it, 0.3.1 → 0.3.1 would not).
4. Add the upstream `logging:` block back into the NAS compose.

**Effort:** S (cleanup) + S (script) + S (docs: a "Deploying on a NAS behind an existing reverse proxy" subsection in `docs/hub.md` that the NAS compose header currently carries alone).

---

## F3 — Ingress and LAN exposure: `0.0.0.0:4180` plaintext on the host network

**Severity:** P1

**Evidence**
- `ssh nas 'cat /proc/net/tcp /proc/net/tcp6'` → `00000000:1054` and `…:1054` in state `0A` (LISTEN): port 4180 (0x1054) is bound on all v4 and v6 addresses of the NAS.
- NAS compose: `ports: ["4180:4180"]`, `FLEET_HUB_BIND: 0.0.0.0`, and its header explains: "The https:// public URL is what allows the 0.0.0.0 bind without --allow-plaintext". Code agrees: `crates/fleet-hub/src/config.rs:386-411` — `tls_in_front = public_url starts with https://`; the plaintext refusal applies only when `!is_loopback(bind) && !tls_in_front && !allow_plaintext`. So the hub *trusts* that the only path to 4180 is through the TLS terminator; nothing enforces it.
- `/volume1/docker/caddy/Caddyfile:43-52`: `http://fleet.rlt.sk { reverse_proxy 192.168.13.113:4180 { flush_interval -1 } }` — Caddy (a container) reaches the hub through the host's LAN IP, which is why the port is published at all.
- Host allowlist: `allowed_hosts` defaults to the public URL host (`config.rs` test `allowed_hosts_default_to_the_public_url_host`); `auth::check_request` checks the `Host` header against it. This is the DNS-rebinding defence (`auth.rs:748` test comment), **not** a LAN barrier: a LAN client sends `-H 'Host: fleet.rlt.sk'` and is through to the bearer check.
- Auth failure handling (`crates/fleet-core/src/mcp/mod.rs:218-233`): a bad/missing bearer on any route → one `warn` line `[mcp] rejected request` with status and path, then 401. **No** rate limit, lockout or backoff on failed bearer attempts. The `RateLimiter` (`guard.rs:951-963`, one-slot bucket per key) is applied only to `POST /pair` (per source IP, ~10/min, keyed by `X-Forwarded-For` which the Caddyfile comment protects) and to the messaging tools (`tools/messaging.rs:158`). `GET /events` has a per-caller stream cap (`events_route.rs:557`).
- Cloudflare: `curl https://fleet.rlt.sk/health` → `401` with `via: 1.1 Caddy`, `server: cloudflare`, no `cf-access-*` headers → there is **no Cloudflare Access policy** in front of `/mcp`, `/pair`, `/events`, `/hook`; the internet reaches the hub's bearer check directly. `/healthz` is intentionally unauthenticated and reveals only `fleet-hub ok` (`docs/hub.md` §healthcheck).
- Legacy `/hook?token=` query path (`mod.rs:218-229`) is disabled because the allowlist is non-empty on a public hub — good.

**Root cause:** the deployment publishes the plaintext port on the host so that a *separate* Caddy container can reach it; the hub's "https public URL permits the bind" rule was written for the upstream compose where the hub is only reachable on the compose network.

**Risk:** every device on the LAN (and every WireGuard peer, since wg-easy is on the same NAS) can (a) sniff master / per-host / client bearer tokens if any client is ever pointed at `http://192.168.13.113:4180` (today nothing is — all provisioned hooks and the desktop use `https://fleet.rlt.sk`), and (b) hit `/mcp` with a guessed bearer at full speed with only a warn line per attempt. With 256-bit random tokens (b) is not a practical break-in, but it is a log-flood and it hides in `docker logs`.

**Proposed fix (pick one)**
1. **Preferred:** put `fleet-hub` on Caddy's docker network and stop publishing the port: in the fleet-hub compose `networks: [caddy_net]` (external), remove `ports:`, keep `FLEET_HUB_BIND: 0.0.0.0` (needed inside the bridge), Caddyfile `reverse_proxy fleet-hub:4180`. The image `HEALTHCHECK` still probes `127.0.0.1:4180` inside the container, unaffected.
2. Or bind the publication to loopback (`ports: ["127.0.0.1:4180:4180"]`) and run Caddy with `network_mode: host` — larger blast radius on the Caddy side.
3. In the hub: `config.rs` should at least `warn!` at startup when `bind` is routable, `tls` is `off`, and the reason for allowing plaintext is only the https public URL ("plaintext on <bind>:<port> is reachable by anything that can route to it; front it with your proxy's network, not a published port"). Effort S, one line + test.
4. Add a Cloudflare rate-limiting rule (e.g. >30 responses with status 401 from one IP in 1 min → challenge/block for 10 min) for `fleet.rlt.sk/*`. Hooks and the desktop never produce 401s, so no false positives. Alternatively a small in-hub per-IP limiter on 401s (reuse `RateLimiter`, key `auth:<ip>`, only counting failures) — effort S, and it also protects a bare-binary deployment.
5. Observability of failures: the warn line lacks the source address; add `peer = %addr` (already available on `/pair`) so a burst is attributable.

**Effort:** S (compose/Caddy change) + S (hub warn + 401 limiter) + S (Cloudflare rule).

---

## F4 — Shutdown / drain vs `stop_grace_period`

**Severity:** P3 (numbers check out; one documented edge)

**Evidence** (`crates/fleet-hub/src/serve.rs:853-902`)
- Order on SIGTERM/SIGINT (`wait_for_signal`): `ticks_cancel.cancel()` → `shutdown.cancel()` → `timeout(DRAIN_TIMEOUT=10s, serve_task)` → `await_ticks(…, TICK_SHUTDOWN_TIMEOUT=10s)` → `tunnels.stop_all()` → `ssh.shutdown_all()`. Worst case ≈ 20 s + master teardown; `stop_grace_period: 30s` in the NAS compose matches the upstream compose and its comment.
- Ticks observe cancellation **only between passes** (`service/tick.rs:22-41`, issue #144): a pass in flight when SIGTERM arrives runs to completion, bounded by the 10 s tick timeout after which SSH masters are torn down under it (warn `reconcile/usage ticks did not finish…`).
- Store: WAL + `synchronous=NORMAL` since 0.2.40 ("store: open the file store in WAL with synchronous=NORMAL"); 0.3.0: every host's reconcile writes commit in **one** transaction, transactions `BEGIN IMMEDIATE`, and "a transaction that SQLite aborts midway no longer half-commits".
- Ghosting (`store/sessions.rs:332,404`) sets `status='ghost', lost_at, lost_reason` only from a *successful* probe of a reachable host whose tmux no longer lists the session; a probe that fails because the SSH master vanished marks the host unreachable and leaves rows alone (reboot-survival design, migration 036).
- Reconcile interval default 20 s (`reconcile.rs:36`), lost-row TTL 14 d (`reconcile.rs:56`).

**Assessment**
- A SIGKILL at 30 s cannot corrupt the store: SQLite WAL survives process death (only OS/power loss can lose the last `NORMAL`-synced transactions, and even then without corruption). A torn pass loses at most that host's single uncommitted transaction.
- A mid-tick kill cannot mark sessions lost: the ghost write needs a completed probe. The realistic failure is the opposite — hosts flip to `reachable=false` for one tick after restart, which `hosts_reachable` shows for ≤20 s.
- One edge: Synology Container Manager's UI "Stop" button issues `docker stop` with the container's configured grace (honoured for compose projects), but the *package* stop on DSM upgrade/reboot stops containers in parallel with a global timeout; if the NAS reboots while a slow pass is running, the pass is cut at the daemon's limit, still safe by the reasoning above.

**Proposed fix:** none required. Optional: log the elapsed drain/tick wait at info on every shutdown so `fleet-hub reports`/logs show whether the 10+10 s budget is ever approached (effort S), and have the NAS upgrade script (F2) use `docker compose stop` (not `down`) so the grace period is applied before `up -d`.

---

## F5 — Observability: logs behind `sudo`, no tick stats, `/metrics` too thin

**Severity:** P2

**Evidence**
- Hub file logging is on: `serve()` calls `fleet_core::logging::init_in_with(&r.log_dir, true)`; `log_dir` defaults to `<data-dir>/logs` (`config.rs:415-419`, `FLEET_HUB_LOG_DIR` overrides) → on the NAS that is `./data/logs`, i.e. `/volume1/docker/fleet-hub/data/logs/fleet-hub.YYYY-MM-DD-HH.log`, hourly, `MAX_LOG_FILES = 72` (`logging.rs:46-49`). **But** `data/` is uid 1000 mode 0700 (`stat`), and `ls data/logs` → `Permission denied` for the operator account; so the only way to read hub logs today is `sudo` (docker logs or the file).
- The NAS compose has no `logging:` block → Docker's json-file driver with Synology defaults (no `max-size`) captures the same lines to stdout unbounded; upstream compose caps it at 3×10 M with an explicit comment that this is "the file that fills the disk".
- Endpoints: `/healthz` = liveness only (`fleet-hub ok`, no version by design). `fleet_health` (bearer) = `version, db_ready, schema_version, hosts_reachable/total, sessions_total, by_status, ghosts, context_red, stuck, usage_by_host, tunnels, tunnels_flapping, usage_by_day, peer_links_down` (+ the 0.2.42 tracker roll-up) — **no** `uptime`, `started_at`, last reconcile pass time/duration/outcome, consecutive tick failures, usage-tick / tracker-sync last run, WAL size, or DB file size. `/metrics` (master only) exposes exactly three series: `fleet_tool_calls_total`, `fleet_tool_errors_total`, `fleet_event_streams_open` per caller (`mcp/metrics.rs`).
- Tick failures are only `warn!` lines (`tick.rs:96,123,182,187`) — invisible unless someone reads the log.
- Error reports: the hub drains its own `ERROR` ring into the `reports` table (0.2.3x "hub: age-sweep error reports and drain the hub's own ring on the tick"); readable via `GET /reports` (bearer) or `fleet-hub reports` (CLI → `docker compose exec` → sudo on the NAS).
- Uptime Kuma already runs on the NAS (`/volume1/docker/kuma`).
- The 0.2.26 agent on `trn` sends no `Report` frames, so that host's errors never reach the table (F1).

**Root cause:** the hub was designed for a VPS with `journalctl`; on a NAS with a bind-mounted 0700 data dir and no shell as `fleet`, everything the operator needs is behind `sudo`.

**Proposed fix**
1. **Logs readable without sudo (S):** mount logs separately, `./logs:/var/lib/fleet-hub/logs` with `FLEET_HUB_LOG_DIR=/var/lib/fleet-hub/logs`, create `./logs` as `1000:users` mode `2750` (setgid so hourly files inherit the group) — the hub creates files 0644 after the umask fix (`serve.rs:680-686` pattern). Add the json-file caps.
2. **Health fields (M, fleet-core `service/health.rs` + `mcp/tools`):** `hub: { started_at, uptime_secs, reconcile: { interval_secs, last_started_at, last_duration_ms, last_ok, last_error, consecutive_failures }, usage_tick: {…}, tracker_sync: {…}, db: { path_bytes, wal_bytes, last_checkpoint_at } }`. Needs a small shared `TickStats` (Arc<Mutex<…>>) written by `run_cancellable_tick` and read by `fleet_health`. Wire additive field with `#[serde(default)]` so older desktops ignore it (memory: a new field without `serde(default)` is a shipped outage).
3. **`/metrics` gauges (S once 2 exists):** `fleet_uptime_seconds`, `fleet_reconcile_last_duration_seconds`, `fleet_reconcile_failures_total`, `fleet_hosts_reachable`, `fleet_sessions{claude_status=…}`, `fleet_db_wal_bytes`. Still master-only per the design note in `docs/hub.md`.
4. **NAS operator kit (S):** Uptime Kuma HTTP monitor on `https://fleet.rlt.sk/healthz` (keyword `fleet-hub ok`) plus a JSON-query monitor on `fleet_health` using a **readonly client token** (`fleet-hub pair --mode readonly kuma`), never the master; alert on `db_ready=false`, `hosts_reachable < 5`, `tunnels_flapping > 0`, and (after 2) `reconcile.consecutive_failures >= 3`.
5. Tick failures should also produce one ERROR-level event after N consecutive failures so they land in `reports` (S).

**Effort:** S+S+M+S+S.

---

## F6 — SSH from inside the container: masters, `known_hosts`, key rotation, the `local` host

**Severity:** P2 (runbook gap) / P3 (`local` cruft)

**Evidence**
- ControlMaster sockets: `ssh.rs:210-224` → `cache_dir()/cm-<host>.sock`, `cache_dir()` = `$HOME/.cache/claude-fleet` (`ssh.rs:1663-1668`) → `/home/fleet/.cache/claude-fleet` inside the container — the **writable layer**, not the `./ssh` bind mount. Options per call: `ControlMaster=auto ControlPath=… ControlPersist=10m BatchMode=yes` (`ssh.rs:233-239`). Consequence: masters die with the container (no stale-socket problem across restarts) and are recreated on the first call after `up`; after `ssh.shutdown_all()` on SIGTERM nothing lingers.
- `known_hosts`: no `StrictHostKeyChecking`/`UserKnownHostsFile` override in `ssh.rs` → ssh's default `ask`, which under `BatchMode=yes` fails; `docs/hub.md` §Setup: "every host's key must already be in `./ssh/known_hosts`, or the host is recorded unreachable" and shows the `ssh-keyscan … | sudo tee -a ssh/known_hosts` step. A host reinstall / host-key rotation on `mac|mefistos|htz|oci` therefore shows up as `Host key verification failed` → `reachable=false` → one `E_SSH` row in `reports` (the example line in `docs/hub.md` §Error reports is exactly this) — a clear signal, but the fix needs `sudo` on the NAS.
- Hub key rotation: `fleet-hub ssh-key` "never overwrites an existing private key" and generates only when neither file exists (`serve.rs:326-343`, `KeyAction`); there is no `--rotate`. Rotation is manual: generate a new pair into `./ssh`, append the new `.pub` to `authorized_keys` on 5 hosts, swap files, restart the hub, remove the old key from the hosts.
- `local` host: row exists in the hub's `state.db` (copied from the desktop), `hidden=true`, `reachable=false`, `last_pinged 2026-09-20`. `serve()` calls `fleet_core::service::hub::disable_local_host()` when `local_host=false` (the NAS default), so any tool naming `local` is refused (`hub.rs:181-212`); `reconcile_all` probes only `!h.hidden` hosts (`service/sessions/reconcile.rs:1428`) so `local` is never SSH'd. The account-usage tick passes `list_hosts()` **unfiltered** (`tick.rs:178-199`) and relies on `poll_due_accounts`/`usage.rs:678` consulting `local_host_enabled()` — it is skipped, but by a different gate than reconcile uses. 0.2.42 "hub: self-heal leftovers of a disabled local host" cleans some leftovers, yet the brief's 6 `local` ghost rows (3 of them duplicates of `mac` rows, still `claude_status=working`) are still there — they are what `include_lost=true` shows and they will only age out at the 14-day lost TTL.

**Proposed fix**
1. Runbook in `docs/hub.md` (S): "When a host's SSH key changes" (keyscan → `sudo tee -a`, then `probe_host`) and "Rotate the hub's SSH key" (the 5-step manual above), noting the NAS needs `sudo` for both because `./ssh` is uid 1000.
2. `fleet-hub ssh-key --rotate` (M): writes `id_ed25519.new`, prints the pub key, and `--commit` swaps once `probe_host` succeeds on every non-hidden host with the new key (`-i` override); refuses to commit while any host still fails.
3. Make the hidden filter one function used by *every* host loop (reconcile, usage poll, tunnels, tracker `via_host`) — `store::hosts::active_hosts()` (S), so `local`/hidden handling cannot diverge again.
4. Delete the `local` host row and its 6 ghost rows on the hub (after F7's backup): either `fleet-hub`-side `host remove local --with-sessions` (does not exist today; the MCP `remove_host` path is the desktop's) or a one-off SQL as root with the container stopped. The duplicate `bg:*` rows under `local` vs `mac` are a rename artefact worth a regression test in `store/hosts.rs` (rename should re-key sessions, not clone them) — outside this lens, flagged for the sessions reviewer.

**Effort:** S / M / S / S.

---

## F7 — Backups: only manual pre-upgrade copies, taken as `cp` of an open WAL database

**Severity:** P1

**Evidence**
- Nothing scheduled: `/etc/cron.d` holds only Synology's own tasks; `crontab` is not on PATH; `ls /usr/syno/etc/synoschedule.d` shows only `root` (contents not readable). No Hyper Backup evidence for `/volume1/docker/fleet-hub` is visible from the shell (cannot be excluded without DSM access).
- The two `backup-*` triples were copied while the WAL was live (F2 evidence: `state.db` mtime precedes `-wal`/`-shm` by minutes; a cleanly closed WAL database has no `-wal` file at all, since SQLite checkpoints and deletes it on the last close). A `cp` of `state.db` alone from such a set is a *stale* database (up to 4.3 MB of pages live only in the WAL); a `cp` of the triple while the hub writes can be torn.
- `sqlite3` exists on the NAS (`/usr/bin/sqlite3`), `python3` too. `docker`/`docker compose` are root-only (socket `root:docker`, `mjanci` is not in `docker`).
- The 0.3.0 changelog's own instruction is "Back up `state.db`, together with any `state.db-wal` / `state.db-shm`" — correct but only if the hub is stopped first; `docs/hub.md` has no backup section at all (`grep -n backup docs/hub.md` → 0 lines outside the peer/tracker text).

**Root cause:** no backup design for the hub's single-file store; the desktop app never needed one because Time Machine covered `~/Library/Application Support`.

**Proposed fix**
1. **Nightly online backup (S):** DSM Task Scheduler → user `root` → daily 03:30:
   ```
   set -e; D=/volume1/docker/fleet-hub; mkdir -p $D/backups
   sqlite3 $D/data/state.db ".backup '$D/backups/state-$(date +%F).db'"      # consistent, no stop, WAL-aware
   sqlite3 $D/backups/state-$(date +%F).db 'PRAGMA integrity_check' | grep -qx ok
   find $D/backups -name 'state-*.db' -mtime +14 -delete
   chown -R 1000:users $D/backups && chmod 750 $D/backups
   ```
   `.backup` uses the online backup API and yields a single self-contained file; 14 dailies at ~4–8 MB is trivial. Add the folder to Hyper Backup / an off-NAS target (the Mac already syncs dotfiles; `backups/` can ride the same route).
2. **Pre-upgrade backup** = the same `.backup` line inside the upgrade script (F2), so the hub need not be stopped to get a consistent copy.
3. **Restore drill** documented in `docs/hub.md` §Backups: stop → move `data/state.db*` aside → copy the chosen `.db` to `data/state.db` (owner 1000) → `up -d` → check `fleet_health.schema_version`; note that restoring a pre-migration file onto a newer image re-runs migrations (fine) and that the desktop's "refuse a database a newer build has migrated" guard means never restore a *newer* file onto an *older* image.
4. Optional, in the hub (M): `fleet-hub backup <path>` subcommand using rusqlite's `Backup` API, so bare-binary deployments get the same without `sqlite3` installed, and the docs can say one thing for both.

**Effort:** S + S + S (+ M optional).

---

## F8 — Federation: no peers configured; the dialer supervisor still ticks

**Severity:** P3

**Evidence:** `fleet_health.peer_links_down = 0`, `peer_links` empty. `serve()` always spawns `spawn_peer_supervisor` (`serve.rs:836-842`); the loop (`service/peer/supervisor.rs`) wakes every `RESCAN = 5 s`, takes the **writer** store mutex (`lock(&ctx.store)`), runs `live_dialer_links()`, reconciles an empty map, sleeps. No sockets, no dials. On a spinning-disk NAS the cost is one short mutex hold + one indexed SELECT every 5 s — measurable only in that it competes with the writer during a heavy reconcile.

**Proposed fix:** none required. If the read pool from 0.3.0 is reachable there, route `live_dialer_links()` through it so the idle supervisor never touches the writer mutex (S); or back the rescan off to 30 s while the last listing was empty and snap back to 5 s after a `peer add` (S). `peer_links_down` being 0 with no rows is honest, but `fleet_health` could say `peer_links_total: 0` so an operator can tell "nothing configured" from "all up" (S, additive field with `serde(default)`).

---

## F9 — Compose drift from upstream and small hygiene items

**Severity:** P3

- Missing `logging:` caps (F5) and the commented `deploy.resources` block — with `restart: unless-stopped`, an OOM on a 0.3.x-sized reconcile of 63 rows would just cycle; measure with `docker stats` and set a `memory:` limit with headroom.
- `._docker-compose.yml` AppleDouble file → delete; `fleet-hub.env` is mode `0777` (rwxrwxrwx+ in the listing) — it holds `FLEET_HUB_DOMAIN`/`PUBLIC_URL` only per `fleet-hub.env.example`, but tighten to 0640 root:users anyway. (Contents not read.)
- The NAS compose header is the only place the "existing Caddy on the host, publish 4180" topology is written down; move that paragraph into `docs/hub.md` with the F3 fix so the next NAS follows the safer shape.
- `cloudflared` config on the NAS does not name `fleet` (grep of `routes.md`/compose, secrets excluded) → the tunnel forwards a wildcard to Caddy; fine, but it means removing `http://fleet.rlt.sk` from the Caddyfile is the only off switch — document it.

**Effort:** S.

---

## Prioritised list

| # | Finding | Sev | Effort | Action |
|---|---|---|---|---|
| 1 | F7 no real backup; existing copies are live-WAL `cp`s | P1 | S | DSM root task: nightly `sqlite3 .backup` + integrity check + 14-day retention; same line in the upgrade script; restore drill in `docs/hub.md` |
| 2 | F3 plaintext `0.0.0.0:4180` on LAN/WG, no 401 throttling, no Cloudflare Access | P1 | S+S | Put fleet-hub on Caddy's docker network and drop `ports:`; hub startup warn for routable plaintext bind; per-IP 401 limiter or Cloudflare rate rule; add peer addr to the reject warn |
| 3 | F5 logs behind sudo, unbounded docker json log, no tick stats | P2 | S+M | `./logs` mount (`2750`, group users) + json-file caps; `hub{uptime,reconcile,…}` in `fleet_health` + `/metrics` gauges; Kuma monitors with a readonly client token |
| 4 | F1 version triangle: procedural exposure to the next contract bump; old agent sends no error reports | P2 | S | Upgrade desktop then agent to 0.3.1 now; write the one-page upgrade checklist |
| 5 | F6 `known_hosts`/key-rotation runbook; hidden-host filter duplicated; `local` cruft | P2/P3 | S/M/S | Runbook; `ssh-key --rotate`; single `active_hosts()`; purge `local` rows after backup |
| 6 | F2 15 compose copies, 4 `data.pre-*`, ad-hoc procedure | P3 | S | Compose in git with `.env` tag; `upgrade.sh <ver>` (pull → `.backup` → stop → tag → up → healthz → keep 3) |
| 7 | F9 compose drift (logging, limits), AppleDouble file, env perms | P3 | S | Align with upstream; delete; chmod |
| 8 | F4 shutdown budget | P3 | – | Numbers verified (10 s + 10 s < 30 s grace); WAL + one-tx-per-host make a mid-tick kill safe; no sessions are ghosted by a torn pass |
| 9 | F8 idle peer supervisor | P3 | S | Optional: read pool / slower rescan when empty; `peer_links_total` |

## Recommended upgrade order

**This round (contract 4 on both sides, proto 1 on both sides — order is a habit, not a constraint):**
1. `sqlite3 data/state.db ".backup …"` on the NAS (root) — establishes the F7 job at the same time.
2. Hub is already 0.3.1; nothing to do. (If it were not: pull → backup → `compose stop` → tag → `up -d` → `/healthz` → `fleet_health.version`.)
3. Desktop `mac-desktop` → 0.3.1 (no migration risk in hub-client mode; the local `state.db` snapshot from 2026-09-25 is untouched by a hub-mode launch).
4. `fleet-agent` on `claude-fleet-trn` → 0.3.1 (`fleet-agent install` with the existing token; the 30 tmux sessions are unaffected, the agent is a proxy). Confirm `agent_status.agent_version`.
5. Verify with `fleet_health` (version 0.3.1, `db_ready`, `hosts_reachable 5/6` → 5/5 once `local` is purged) and the absence of a red banner on the desktop.

**Standing rule for future releases** (from `contract.rs`, `fleet-proto/src/lib.rs`, `docs/hub.md`): when a release bumps `CONTRACT_REVISION`, upgrade **hub first, then the desktop in the same window** — there is no mixed window, the desktop refuses with `E_HUB_CONTRACT` until it is updated, hooks and the phone keep working meanwhile. When a release bumps `PROTO_VERSION`, upgrade **hub first**; the release must hold `MIN_SUPPORTED_PROTO` at the previous value so the 0.2.x-style agent keeps connecting until it is reinstalled. A hub upgrade never needs the agents restarted; an agent that is refused backs off to 30–60 s retries and heals when the hub catches up.
