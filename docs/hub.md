# fleet-hub — the headless daemon

## What it is

`fleet-hub` is the same fleet — hosts, sessions, the Control API, the asset
catalog — running as a headless daemon instead of inside the Tauri desktop
app. One hub serves one fleet; run it once, on any always-on machine
(a small VPS, a home server), and point every host and every MCP client at
it. The desktop app becomes optional: install it only where you want the
terminal UI, and let it talk to the same `state.db` or run alongside without
touching it (see *Coexistence with the desktop* below).

## Setup with Docker (recommended)

```bash
mkdir -p ~/fleet-hub && cd ~/fleet-hub
curl -O https://raw.githubusercontent.com/martin-janci/claude-fleet/main/deploy/hub/docker-compose.yml
curl -O https://raw.githubusercontent.com/martin-janci/claude-fleet/main/deploy/hub/Caddyfile
curl -O https://raw.githubusercontent.com/martin-janci/claude-fleet/main/deploy/hub/fleet-hub.env.example
cp fleet-hub.env.example fleet-hub.env
# edit fleet-hub.env: set FLEET_HUB_DOMAIN and FLEET_HUB_PUBLIC_URL to your
# own domain (both point DNS at this machine; Caddy gets a cert automatically)
```

**The image.** The compose file pins one release —
`ghcr.io/martin-janci/fleet-hub:0.2.41`-shaped, not `:latest`. `docker compose
pull` then fetches exactly that version, so the hub you are running is the one
you can name, and going back to it is editing one line. Upgrading is a
deliberate act: see *Upgrade and rollback* below.

Note the missing `v`: the git tag is `v0.2.41`, the image tag published for it
is `0.2.41`. Every released version has one. There is no image for an
arbitrary commit: a `sha-<commit>` tag is published too, but only ever
alongside a release, because only a `v*` tag builds an image at all (see the
next paragraph) — so it is a second name for the version tag, not a way to
pull an unreleased commit. `latest` exists as well and is a convenience for
"whatever the newest stable release is" — fine for a throwaway trial, wrong
for anything you will have to roll back, because the tag moves under you on
the next release and `docker compose pull` then silently changes your
deployment. A release candidate (`0.3.0-rc.1`) never moves `latest`.

Only a pushed `v*` tag publishes an image at all: `hub-image.yml`'s jobs are
gated on the ref being a tag, and on the tag matching the version in the tree
it points at, so nothing built from unreleased code can reach this namespace.
[The package page](https://github.com/martin-janci/claude-fleet/pkgs/container/fleet-hub)
lists what exists. Each release also records the exact image digest its tag
points at, on the release page — pin that instead of the version tag if you
want an image that cannot be re-pointed even in principle:

```yaml
image: ghcr.io/martin-janci/fleet-hub@sha256:<digest from the release page>
```

If you cannot pull the package for any reason, build the image locally from a
checkout of the repository instead and point `image:` in `docker-compose.yml`
at it:

**Platforms.** `linux/amd64` and `linux/arm64`. **arm64 is best-effort
until it has a track record**: `hub-image.yml` builds each platform on its
own native runner (no QEMU) and, if the `arm64` leg fails, still publishes
`amd64` alone under the same tags rather than blocking the image on it —
so a given `X.Y.Z`/`latest` may, on such a run, carry only an amd64
manifest, and `docker pull --platform linux/arm64` (or any arm64 host
pulling by tag) then fails outright rather than silently getting an amd64
image. **The run itself still shows green** when this happens — an
amd64-only publish is a successful run, not a failed one, since amd64
publishing must keep working regardless of arm64 — but it is not silent:
the run carries a `::warning::` annotation and a job-summary note saying
arm64 failed and the manifest is amd64-only. Check the `hub-image`
workflow's own run history and summaries (the release page says
`linux/amd64 only` for such a version), or `docker buildx imagetools inspect
ghcr.io/martin-janci/fleet-hub:<version>`, if that matters to you.

```bash
docker build -f crates/fleet-hub/Dockerfile -t fleet-hub:local .
# docker-compose.yml:  image: fleet-hub:local
```

The container runs as uid 1000 (its `fleet` user), and `./ssh` is
bind-mounted as that user's `~/.ssh`, so create the directory owned by uid
1000 before the first container touches it:

```bash
mkdir -p ssh && sudo chown -R 1000:1000 ssh && sudo chmod 700 ssh
```

Mint the master token and the hub's SSH key before starting the daemon
properly (`docker compose run --rm` runs a one-off container against the
same named volumes and bind mounts the long-running services will use):

```bash
docker compose run --rm fleet-hub init          # prints the master token — save it
docker compose run --rm fleet-hub ssh-key       # prints the hub's SSH public key
```

`ssh-key` prints `~/.ssh/id_ed25519.pub`. When only the private key
`~/.ssh/id_ed25519` exists (for example one you copied into `./ssh`), it
derives the public half with `ssh-keygen -y` and saves it next to it; it
generates a new key only when neither file exists, and never overwrites an
existing private key.

Add the printed public key to `~/.ssh/authorized_keys` on every host you want
the hub to manage. Then create `./ssh/config` (bind-mounted at
`/home/fleet/.ssh/config` in the container) with one `Host` block per
machine, the same shape as `~/.ssh/config` for the desktop app. `./ssh` is
now owned by uid 1000, so write into it with `sudo`:

```bash
sudo tee ssh/config >/dev/null <<'CONFIG'
Host devbox
    HostName 10.0.0.12
    User martin
CONFIG
```

The hub connects with `BatchMode=yes`, so it cannot answer an unknown-host
prompt: every host's key must already be in `./ssh/known_hosts`, or the host
is recorded unreachable. Scan each host at the same `HostName` (and, if its
block sets one, `Port` — pass it as `-p <port>`) as in `./ssh/config`:

```bash
ssh-keyscan -H 10.0.0.12 | sudo tee -a ssh/known_hosts >/dev/null
# repeat for every host
```

Both files must be owned by uid 1000 and private to it:

```bash
sudo chown 1000:1000 ssh/known_hosts ssh/config && sudo chmod 600 ssh/known_hosts ssh/config
```

Bring the daemon up and verify it answers:

```bash
docker compose up -d
curl -s https://fleet.example.com/mcp \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"fleet_health","arguments":{}}}'
```

A healthy hub answers with `db_ready: true` and the running version.

`hub` is this process: `started_at`, `uptime_secs` and `reconcile`
(`last_started_at`, `last_finished_at`, `last_duration_ms`, `last_ok_at`
(when the last clean pass finished; a later failure leaves it set),
`consecutive_failures`, `failures_total`, `last_error`) — alert on
`consecutive_failures >= 3`. `tunnels_mode` is `none` on a public hub (hooks
post directly; the `tunnels` map is empty because nothing applies) and
`reverse` otherwise. `peer_links_total` tells `peer_links_down: 0` from "no
peers configured".

`usage_by_day` books each token to the UTC day of the transcript line that
produced it. The first time a transcript is read (a new host, a hub takeover)
its history before that day lands in `backfill_cost_micros`, apart from the
day's live `cost_micros`, so a takeover never reads as an $850 day — however
many passes a large transcript takes, and likewise when a transcript is
rewritten. Yesterday's lines stay live for two collection intervals after
midnight UTC, so a session that started just before midnight is not history.
The split applies from the upgrade that introduced it on: rows booked before
it (migration 071) all became live rows, so a takeover spike that is already
in the roll-up stays one. `usage_daily` is kept for as long as its host is
configured (removing the host deletes its rows) and has no retention window:
it is at most two rows (live and backfill) per host per day, a few KB a year.
`usage_report` says what each figure counts: `by_host_population: live_rows`
(session rows that still exist, over their lifetime — ghosts included) and
`by_day_population: durable` (the daily roll-up, killed sessions included);
the two need not agree.

The image carries a Docker `HEALTHCHECK` that runs `fleet-hub healthcheck`
every 30 s, so `docker compose ps` shows the hub as `healthy` (or
`unhealthy`) in its STATUS column. The check sends one `GET /healthz` to
`127.0.0.1` on `FLEET_HUB_PORT` (default `4180`) and passes only when the
answer is an HTTP status line with the body `fleet-hub ok` — so an unrelated
process holding the port no longer reads as a healthy hub. It reads
`FLEET_HUB_TLS` the same way, so when the hub terminates TLS itself the probe
speaks TLS too (certificate verification off: the certificate names a public
domain, and this is a liveness check to `127.0.0.1` carrying no credential).
`fleet-hub healthcheck --tls off|cert` overrides that explicitly. Neither the
port nor the TLS mode is read from `state.db`: the probe runs beside a live
`serve` and never opens the database.

`/healthz` and `/pair` are the only routes outside the bearer token and the
`Host` allowlist (`/pair` is guarded by its single-use code instead — see
[control-api.md](control-api.md)). `/healthz` is there because it reveals
nothing: it never opens `state.db` and
never names a version, a host, a session or a setting — the fixed body only
means "this process is accepting HTTP". Every other route stays behind the
token. Probes therefore leave no rejected-request lines in the log.

A hub that can no longer serve exits rather than lingering as a process that
answers nothing: when the control API stops on its own (its listener failed,
or the server task ended or panicked), or when a panic leaves the store's lock
poisoned so every write would fail, `serve` logs why, shuts down as it does on
SIGTERM, and exits with status 1, so the restart policy (systemd
`Restart=on-failure`, compose `restart: unless-stopped`) starts a fresh one.
Panics are written to the file log and the error reports with their location
and a backtrace, not only to stderr. SIGHUP is logged and ignored; stop the hub
with SIGTERM (or Ctrl-C), and the stop line names the signal.

The check does not open `state.db` and does not read the stored `mcp.port`:
if you run the hub on another port, set it with `FLEET_HUB_PORT`, not only
`--port`.

## Upgrade and rollback

The compose file pins one version, so nothing changes under you: an upgrade is
a line you edit, and a rollback is the same line edited back. Both are two
commands and a check.

**What am I running?** `/healthz` deliberately will not tell you — it answers a
fixed `fleet-hub ok` and names no version, which is why it can sit outside the
bearer token and the `Host` allowlist. Ask the binary instead, on the hub box,
with no credential:

```bash
docker compose exec fleet-hub fleet-hub --version    # fleet-hub 0.2.41
docker compose images fleet-hub                      # the tag and image id in use
```

Over the network, and only with the master token, `fleet_health` reports the
same string in its `version` field (see the `curl` under *Setup* above). To see
which exact image — not just which version — is running:

```bash
docker inspect --format '{{index .RepoDigests 0}}' "$(docker compose ps -q fleet-hub)"
```

**Is there a newer one?** `docker compose exec fleet-hub fleet-hub update check`
reads the signed release channel and names the version (and image digest) this
hub should run; see `docs/updates.md` → *Operating it*. It only reports: the
upgrade is still the edit below.

**Back up `state.db` first.** It lives in the `hub-data` volume at
`/var/lib/fleet-hub/state.db` and carries the master token, every host, every
session and the asset catalog. Copy it with the container stopped, so you are
not copying a database mid-write:

```bash
cd ~/fleet-hub
docker compose stop fleet-hub
docker compose cp fleet-hub:/var/lib/fleet-hub/state.db "state.db.$(date +%F).bak"
```

**Upgrade.**

```bash
# 1. read the new version's release notes, then point the pin at it
#    (docker-compose.yml:  image: ghcr.io/martin-janci/fleet-hub:0.3.0)
$EDITOR docker-compose.yml

# 2. fetch exactly that version and restart on it
docker compose pull fleet-hub
docker compose up -d fleet-hub

# 3. check it came up on the version you asked for
docker compose exec fleet-hub fleet-hub --version
docker compose ps           # STATUS should reach `healthy` within a minute
```

Step 3 is not ceremony: `up -d` recreates the container only if something
actually changed, so a pin you forgot to edit produces a completely silent
no-op.

**Codex on upgraded hosts.** The release with per-host harnesses keeps
Codex on for every host fleet has already synced Codex assets to, and their
next sync adds Codex subagents (`~/.codex/agents/`). Turn Codex `off` in Host
detail to retire it on a host instead; see *Harnesses per host* in
`docs/concepts.md`. A Codex MCP merge re-serializes `~/.codex/config.toml`
and drops its comments, as before.

**Codex skills move to `~/.agents/skills`.** Codex only reads user skills
from `~/.agents/skills`, so a release with this change renders Codex skills
there, and the first sync after upgrading moves every Codex skill fleet put
in `~/.codex/skills` (backing the old copy up, never touching Codex's
`.system` or skills you added yourself). A host whose `~/.agents/skills`
(or `~/.agents`, or `~/.codex/skills`) is a symlink shows those Codex
actions as `blocked` until the link is replaced with a real directory. A
link the other way — a Claude skill such as `~/.claude/skills/<name>`
pointing into `~/.agents/skills` — is not detected yet, and both harnesses
would then write one file: don't link Claude skills into
`~/.agents/skills`. See *Codex skills live in `~/.agents/skills`* in
`docs/concepts.md`.

Refreshing the compose file itself (`curl -O …/deploy/hub/docker-compose.yml`)
also upgrades you, because the copy on `main` carries the pin from the newest
stable release. Diff it against yours before overwriting: it is the file your
local edits live in.

One window to know about. That pin is written in the release commit, which
lands on `main` at the same moment the image build *starts* — so for the
length of two container builds, and permanently if that build fails, the pin
on `main` can name a tag ghcr does not have yet. A fresh install caught in it
gets `manifest unknown` from `docker compose pull`. Nothing in CI can catch
this (`check-version-consistency.sh` compares the pin with the repo's own
version, not with the registry); what does is
`scripts/check-release-drift.sh`, which asks ghcr daily whether the pin
resolves and opens an issue when it does not. If you hit it, pin the previous
version — [the package
page](https://github.com/martin-janci/claude-fleet/pkgs/container/fleet-hub)
lists what actually exists — and try again later.

**Roll back.** Put the old version back in `image:` and repeat the same two
commands:

```bash
$EDITOR docker-compose.yml   # image: …/fleet-hub:0.2.41
docker compose pull fleet-hub
docker compose up -d fleet-hub
docker compose exec fleet-hub fleet-hub --version
```

The one thing that can stop a rollback is the **database schema**. The hub
migrates `state.db` forward on startup and refuses to open a database that a
newer release has already migrated, rather than running against a shape it does
not understand:

```
this database is at schema version <newer>, but this build of claude-fleet
only knows up to <older>: it was last opened by a newer release. It is not
corrupt; do not delete it. Run that release (or a newer one) again, or restore
the copy of state.db you backed up before upgrading.
```

That message is the whole rollback procedure for a version that migrated:
go forward again, or restore the backup you took above — with the hub stopped,
`docker compose cp state.db.<date>.bak
fleet-hub:/var/lib/fleet-hub/state.db`, then start the older version. Sessions
that ran while the newer release was up are in the newer database, not in the
restored one. Rolling back between two versions that share a schema needs
none of this and loses nothing.

**If a pull fails.** `manifest unknown` means the tag does not exist — check
the spelling and, above all, that you did not write the `v`: the image tag for
`v0.3.0` is `0.3.0`. `no matching manifest for linux/arm64` means that
version's arm64 leg failed and the tag carries an amd64-only manifest (see
*Platforms* above); pin the previous version, or a later one, instead.

### Upgrade with the script

`deploy/hub/upgrade.sh <version>` does the sequence above for a deployment
whose tag lives in `.env` (the `deploy/hub/behind-proxy` compose): it pulls
first (a tag ghcr does not have stops it with the hub untouched), notes the
image the hub runs now (its id and digest), `docker compose stop`s the hub so
the 30 s grace applies, and only then takes the backup (`backup.sh`, kept as
`backups/pre-<version>-*.db`, the newest three *of that version*) — a copy
of the stopped hub, so no write between the backup and the upgrade is lost
by a rollback. It then moves `FLEET_HUB_TAG`, starts the hub, waits for the
image's own healthcheck, and checks `fleet-hub --version`. With a readonly
client token in `readonly.token` beside the compose file it also asks `fleet_health` over the public
URL — never the master token, and passed to `curl` on stdin, not its command
line. To make that token, mint a code with `fleet-hub pair --mode readonly
--name upgrade-check`, redeem it with `curl -s -X POST <public-url>/pair -H
'Content-Type: application/json' -d '{"code":"<code>"}'` and save the
answer's `token` field in `readonly.token` (`pair` prints a pairing code, never
a token). On any failure after the stop it prints the rollback, naming the image
that ran before: a tag can be re-pushed, so if `<old tag>` no longer points
at it, the printed `docker tag <image id> <repo>:<old tag>` puts it back
before the `up -d`. If the backup itself fails, the script starts the old
container again and exits 1 without touching the pin — it never upgrades
without a backup. `KEEP` and `HEALTH_TRIES` (seconds to wait for the
healthcheck, default 60) must be numbers; anything else exits 2 up front.

The image pulled is the compose file's own `image:` line at the new tag, and
every `docker compose` call reads `FLEET_HUB_ENV_FILE` (default `.env`
beside the compose file) through `--env-file`. The tag must match
`[0-9A-Za-z._-]+`. On a fresh copy of `.env.example` (`FLEET_HUB_TAG=`
empty) there is nothing to stop and no `state.db` to back up: the script
says so and skips those steps, so the same command is also the first
install. With a tag set, a missing `state.db` (or no `sqlite3`) stops the
upgrade before the hub is touched (point `FLEET_HUB_DATA` at the right
directory) — it never migrates without a backup. An upgrade never prunes an older
version's `pre-<version>-*.db` (see *Backups*).

**Order across the three binaries.** Today (the hub serves contract 16, the desktop accepts 16,
proto 1 on both sides; `fleet-hub compat` prints a build's windows) the order is a habit: hub, then desktop, then the
agents. When a release bumps `CONTRACT_REVISION`, upgrade the **hub first,
then the desktop in the same window** — there is no mixed window, the desktop
refuses with `E_HUB_CONTRACT` until it is updated, hooks and the phone keep
working meanwhile. When a release bumps `PROTO_VERSION`, upgrade the **hub
first**; the release holds `MIN_SUPPORTED_PROTO` at the previous value so an
older `fleet-agent` keeps connecting until it is reinstalled. A hub upgrade
never needs the agents restarted.

**Upgrading to multi-user M1 is three steps, not two.** That release takes
`CONTRACT_REVISION` to 8, so the first two are the rule above — hub, then the
desktop, in the same window. The third is **a full re-provisioning of every
host**: `provision_hosts` from a client, or `fleet-hub provision --host <alias>`
per host, and *not* `--content-only`, which by contract never rewrites
`~/.claude.json`. Until a host is re-provisioned its agents send no
`X-Fleet-Pane` header, prove no pane, and are refused every call that needs the
proof, including the read of their own session. That is fail-closed, and nothing
surfaces it: the provisioning fingerprint does not cover the MCP entry, so the
host does not report stale and the UI shows nothing wrong (*The pane header*
under *Add and provision hosts*). Re-provision, then restart Claude Code on each
host. And before you pair a colleague's first device, read *Who owns a session*
→ *Privacy, precisely*: in a deployment where the company's admin is also the
hub's operator, privacy holds against colleagues and against anyone whose
authority comes only through the application, and not against the person who
runs the machine.

### Automatic updates with fleet-updater

The `auto-update` compose profile adds `fleet-updater`, which does the same
sequence on its own when the hub's update policy says so (by default only
for a version an operator pins with `update_admin`): pull by the signed
digest, back up, swap the container, check that the new build comes up
ready as the right build and stays so, and otherwise go back to the
previous one — restoring the pre-update backup when the new build migrated
the database. Setup, what it decides, what a rollback loses, and the
systemd-timer alternative to mounting the Docker socket in a long-running
container: `docs/updates.md` → *fleet-updater*.

## Backups

`state.db` carries the master token, every host, every session and the
usage roll-ups. The hub keeps it in WAL mode, so a `cp` of the file from a
running hub is a stale database (pages still in `state.db-wal` are missing)
and a `cp` of the `state.db*` triple can be torn. Use SQLite's online backup
API instead — one self-contained file, no stop:

```bash
sudo FLEET_HUB_DATA=/volume1/docker/fleet-hub/data deploy/hub/backup.sh
# keeps 14 dailies in <FLEET_HUB_DATA>/../backups (FLEET_HUB_BACKUPS overrides)
```

`FLEET_HUB_DATA` defaults to `/volume1/docker/fleet-hub/data` (the Synology
layout below); point it at the compose directory's `data/` anywhere else.

It runs `.backup`, then `PRAGMA integrity_check` on the copy (a failed check
removes it and exits 1; a run killed mid-copy removes its `.part`), then
prunes to the newest `KEEP` files of its own `PREFIX` — other prefixes are
never touched (`PREFIX=pre-1.0.0` prunes `pre-1.0.0-<stamp>.db`, never
`pre-1.0.0-rc1-*.db`). A backup holds everything `state.db` does, the master
token included, so every copy is written `0600` into a `0700` directory
(an existing looser one is tightened first), whoever runs it; as root it
also `chown`s the directory to `FLEET_HUB_OWNER` (default `1000`). On a
Synology: Control Panel → Task Scheduler → user `root`, daily 03:30,
`bash /volume1/docker/fleet-hub/backup.sh`; add `backups/` to Hyper Backup
or any off-box target. `upgrade.sh` calls the same script with
`PREFIX=pre-<version> KEEP=3` right after it stops the hub, so each version
keeps its own three and older versions' `pre-*` files stay until you delete
them.

**From the binary, with no `sqlite3` on the host.** `fleet-hub backup` takes
the same kind of copy from inside the container. It opens `state.db`
read-only and never migrates it, copies it with `VACUUM INTO` while the hub
keeps serving, and checks the copy with `PRAGMA integrity_check` before
renaming it out of `.part`. It never overwrites an existing file. It does
not prune: retention stays with `backup.sh`.

```bash
docker compose exec fleet-hub fleet-hub backup --prefix pre-0.3.4 --json
# {"path":"/var/lib/fleet-hub/backups/pre-0.3.4-20260930-101200.db","schema":73,"bytes":524288}
```

The default path is `<data dir>/backups/<prefix>-<UTC stamp>.db`, the same
name `backup.sh` uses. `--to <path>` names the file instead. This is the
backup `fleet-updater` takes before it replaces the hub (update design
`docs/superpowers/specs/2026-09-28-update-channel-design.md` §8).

**Readiness, beside liveness.** The image's `HEALTHCHECK` (`fleet-hub
healthcheck`) says only that `/healthz` answers, and `/healthz` names no
version. `fleet-hub healthcheck --ready --json` also reads the readiness
file that `serve` rewrites every 5 s at `<data dir>/run/ready.json`. It exits
0 only when all three hold:

- the hub is **live**;
- the file is **fresh**, meaning its heartbeat is at most 20 s old and its
  pid is running;
- the hub is **ready**, meaning the store is migrated, the listener is bound
  and the first reconcile pass has finished.

It prints the build the process runs (`version`, `commit`, `build_id`,
`contract`, `agent_proto`, `peer_proto`, `schema`). None of it goes on the
network: run it with `docker compose exec`. A hub that stops removes the
file, and one that was killed leaves a stale file, which reads as not ready.

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

The variant publishes **no** port: the hub joins your proxy's docker network
(`FLEET_HUB_PROXY_NETWORK` in `.env`, `caddy_default` for a compose project
named `caddy`) and the proxy forwards by service name:

```
fleet.example.com {
    reverse_proxy fleet-hub:4180 {
        flush_interval -1
    }
}
```

The bare site address lets your Caddy obtain the certificate and terminate
TLS for the `https://` public URL; an `http://` prefix would switch that off
and serve the hub in plaintext. The https:// public URL permits the
`0.0.0.0` bind, and the hub logs at startup that plaintext 4180 is reachable
by anything that can route to it —
on the proxy network, that is the proxy. Publishing `4180:4180` on the host
instead makes it the whole LAN and every VPN peer; the warning says so.

**Logs without sudo.** The variant mounts `./logs` as the hub's log directory
(`FLEET_HUB_LOG_DIR`); create it as `install -d -o 1000 -g users -m 2750 logs`
so hourly files stay group-readable. Docker's own capture of the same lines
is capped at 3 × 10 MB. **Watching it.** Uptime Kuma: an HTTP keyword monitor
on `/healthz` (`fleet-hub ok`) and a JSON-query monitor posting `tools/call
fleet_health` to `/mcp/json` with a readonly client token (`fleet-hub pair
--mode readonly kuma`) — never the master — on `db_ready`, `hosts_reachable`,
`tunnels_flapping` and `hub.reconcile.consecutive_failures`.

**Tidying a hand-upgraded deployment.** A directory upgraded by hand tends to
collect `docker-compose.yml.<version>` copies (the `image:` line was the only
difference), `data.pre-<version>` directory copies and `backup-<version>-<ts>`
`cp` triples. With the script in place: keep the live compose and `.env`;
verify each old copy once (`sqlite3 <copy>/state.db 'PRAGMA integrity_check'`
as root — the triples are only valid as the triple), move the ones that pass
into `backups/legacy/`, delete the rest and the `._docker-compose.yml`
AppleDouble sidecar a Mac copy leaves; `chmod 0640 fleet-hub.env`. Nothing in
the repository does this for you — it is the operator's directory.

## Add and provision hosts

Connect any MCP client to `https://fleet.example.com/mcp` with the master
token. For Claude Code:

```bash
claude mcp add --transport http claude-fleet https://fleet.example.com/mcp \
  --header "Authorization: Bearer <token>"
```

Then, from that client:

1. `discover_hosts` — reads the `Host` blocks from the mounted
   `./ssh/config` and proposes hosts to add.
2. `add_host` — adds each one you want managed.
3. `provision_hosts` — installs the skills, the `mcpServers.claude-fleet`
   entry, the Claude Code hooks, and a per-host token on every host (see
   `control-api.md` → *Provisioning hosts* for the full step list).

On a hub with a public URL, provisioning writes `https://<domain>/hook` and
`https://<domain>/mcp` directly into each host's hook block and MCP entry —
no reverse SSH tunnel is started, because every host can already reach the
hub's public address.

Re-running `provision_hosts` (for example after changing the public URL) is
safe: it replaces only fleet's own hook entries — an `http` hook to
`<scheme>://<authority>/hook` with a Bearer header and a 5 s timeout — and
keeps every other hook already in the host's `~/.claude/settings.json`
untouched. The previous file is saved as `settings.json.fleet-bak` first.

**After provisioning, restart Claude Code on each host** to pick up the new
MCP server entry (the skill files and hooks are picked up live).

**Stale content, and `fleet-hub provision`.** Every provisioning records a
fingerprint of what it shipped (both skills, the managed CLAUDE.md block and
the hook shape); `list_hosts` reports `provision_stale: true` for a host whose
fingerprint is not this build's. A minute after start the hub refreshes every
reachable stale host *content only* — skills, the CLAUDE.md block and hooks,
with the host's existing token; no new token, no `~/.claude.json` rewrite, no
Claude restart. By hand: `fleet-hub provision [--host <alias>]
[--content-only]` (the `provision_hosts {host, content_only}` tool).

**The pane header, and why `--content-only` cannot add it.** Provisioning also
writes `"X-Fleet-Pane": "${TMUX_PANE:-}"` into each host's
`mcpServers.claude-fleet` headers, beside the per-host token. That header is how
an agent proves which session it is sitting in, and without it an agent is
refused every call that needs the proof — the read of its own private row
included (*Who owns a session* → *Two people on one host*). The fingerprint
above does **not** cover `~/.claude.json`, so a host provisioned before
multi-user M1 reports no staleness and nothing in the UI tells you: it simply
proves nothing. Only a FULL provisioning writes the header — `provision_hosts`,
or `fleet-hub provision --host <alias>` — because `--content-only` by contract
never rewrites `~/.claude.json`. Re-provision every host after upgrading to that
release, then restart Claude Code on each one so its agents pick the new entry
up; see *Order across the three binaries* under *Upgrade and rollback*.

**Upgrade heads-up (the `ag` launcher).** The fingerprint also covers fleet's
`ag` launcher, so upgrading to the build that ships it makes every provisioned
host stale: within about a minute of start the unattended content refresh
installs `ag` (`~/.local/share/ag`, `~/.local/bin/ag`) and, where the host has
no `cl` command, a `cl` shim (`claude --yolo`) on every reachable host. To opt
out, set `provision.install_ag=false` right after upgrading.

**Who owns what on a host.**

| Path on host | Owner | Written by |
|---|---|---|
| `~/.claude/skills/claude-fleet-control/` | fleet (carries `.fleet-managed`) | `provision_hosts` (overwrites) |
| `~/.claude/skills/fleet-friendly-name/` | fleet (carries `.fleet-managed`) | `provision_hosts` (overwrites) |
| `~/.claude/CLAUDE.md` between the sentinels | fleet | the rest is the user's |
| `~/.claude/settings.json` → the 9 `FLEET_HOOK_EVENTS` entries | fleet | sibling hooks are kept |
| `~/.claude/fleet-hook.headers` | fleet (secret, 0600) | `provision_hook` |
| `~/.claude.json` → `mcpServers.claude-fleet` | fleet (secret) | sibling keys are kept |
| `~/.tmux.conf` `set -g set-clipboard on` line | fleet (append-only) | `provision_tmux_clipboard` |
| every other skill, hook, plugin, `~/.claude/projects` | the user / dotfiles | never touched |

If `~/.claude/skills` is inside a git work tree (a dotfiles checkout) that
tracks files in either fleet dir, provisioning **refuses** that host
(`E_INVALID`, `details.git_toplevel`) rather than overwrite committed files:
untrack the two fleet dirs there (`git rm -r --cached`, then `.gitignore`
them), or set `provision.force_git_tree = true` to write anyway. A checkout
that leaves them untracked or ignored is provisioned as usual.

**What a host needs for prompt delivery.** A prompt rides to the pane as
`base64 -d` piped into `tmux load-buffer -`, so each managed host needs
`base64(1)` with `-d` (GNU coreutils and the BSD/macOS build both have it) and
**tmux 3.0 or newer** (`load-buffer -` reads from stdin only from 3.0). A host
missing either one fails every send with `E_TMUX` carrying the shell's own
complaint — `base64: illegal option` or `load-buffer: invalid option`.

## A host that cannot be reached

The hub normally reaches every host over SSH. A laptop behind a home router,
a machine on a corporate network or anything on mobile tethering has no
address the hub can dial. For those hosts, run **`fleet-agent`** on the host
instead: it dials the hub (`wss://<hub>/agent`), keeps that one connection
open, and runs what the hub sends. The host needs no listening port, no
public address, no tunnel and no key in `authorized_keys`. It does need to
reach the hub, so an agent host needs a hub with a public URL (`--public-url`):
provisioning writes that URL into the host's hooks and MCP entry, and starts
no reverse tunnel for it.

> **Read this first. Installing the agent gives the hub full control of that
> user's account on that machine.** The hub can run any command as that user
> and write any file that user can write. Uploads are deliberately not
> confined to any directory. That is exactly what the hub can already do to
> an SSH host through its key; the agent does not make it less. Install it
> only for a hub you trust as much as you trust that account.

### Set it up

**A host the hub already reaches over SSH** needs none of the steps below:
`install_agent { alias: "laptop" }` (Orbit Fleet 4.9) runs them as one job.
The host downloads the release for its platform and checks it against
`SHA256SUMS`, installs `~/.local/bin/fleet-agent`, takes its token on stdin
into `fleet-agent install --user` (no systemd: `fleet-agent run` under
`nohup`, which does not survive a reboot), and the host moves onto the agent
once it says hello; no hello within 120 s puts it back on SSH.
`agent_installs` shows each job's step and outcome. A paired desktop (a
trusted full device) starts the same job from Host detail: on an SSH host the
health checklist's fleet-agent row reads *not installed* with an *Install
<version>* button, and the job's step shows beside the Hex field until the
host is on its agent (the `install_agent` / `agent_installs` commands route
to the hub; a standalone desktop accepts no agents and says *not needed*).
Nothing installs without that click. `FLEET_AGENT_DIST` on the
hub replaces the GitHub release URL (a mirror). The steps below are for a
host the hub cannot reach.

1. **Register the host as an agent host.** From an MCP client holding the
   master token:
   `add_host { alias: "laptop", ssh_alias: "laptop", transport: "agent" }`.
   `ssh_alias` is still required. Nothing dials it, but it is recorded and
   routing matches on it, so **use the alias itself** — and in any case give
   every host a *distinct* `ssh_alias`. Sharing one never misroutes a command:
   an `ssh_alias` resolves only when exactly one host in the fleet claims it,
   so two hosts claiming the same one — whatever their transports — route to
   neither, and a command for an SSH host never runs on an agent's machine.
   What it costs is reachability. An agent host whose `ssh_alias` another row
   also claims is probed over SSH instead of through its agent, so the probe
   fails and the host is stamped unreachable even while its agent is connected
   and answering everything else. Placeholder values like `none` or `unused`
   are what make this likely. Editing either host row fixes it. An agent host
   is saved **without** an SSH probe and shows as unreachable until its
   agent connects. An existing SSH host becomes an agent host when it is
   re-added with `transport: "agent"`. Re-adding it with `transport: "ssh"`
   moves it back, which cuts its agent off.
2. **Get the host's token, on the hub:**
   ```bash
   fleet-hub agent-token laptop            # mints one on first use; prints only the token
   docker compose run --rm fleet-hub agent-token laptop   # the same, under Docker
   ```
   This is the only way an agent host's token reaches the host. The hub never
   sends a token over the agent connection (see *Rotating* below). Treat the
   output like a password.
3. **Get the binary, on the host.** Each release attaches
   `fleet-agent-<version>-<target>.tar.gz` for `x86_64-unknown-linux-gnu` and
   `aarch64-unknown-linux-gnu` (binary + a `README.txt` pointing back here,
   plus the repository's own root `LICENSE` when one exists — this
   repository does not have one yet, so today's tarballs carry no LICENSE
   file rather than a fabricated one), plus one `SHA256SUMS` covering all
   four tarballs in the release —
   [github.com/martin-janci/claude-fleet/releases](https://github.com/martin-janci/claude-fleet/releases):
   ```bash
   v=0.3.0   # the release you're installing; target: x86_64- or aarch64-unknown-linux-gnu
   curl -LO https://github.com/martin-janci/claude-fleet/releases/download/v$v/fleet-agent-$v-x86_64-unknown-linux-gnu.tar.gz
   curl -LO https://github.com/martin-janci/claude-fleet/releases/download/v$v/SHA256SUMS
   sha256sum -c SHA256SUMS --ignore-missing
   tar xzf fleet-agent-$v-x86_64-unknown-linux-gnu.tar.gz
   sudo install -m 0755 fleet-agent-$v-x86_64-unknown-linux-gnu/fleet-agent /usr/local/bin/fleet-agent
   ```
   These are built on `ubuntu-22.04` runners and need **glibc 2.35 or
   newer** on the host — Ubuntu 22.04+, Debian 12+ and equivalents; not
   Debian 11. No matching release yet, or an older host? Build from a
   checkout instead: `cargo build -p fleet-agent --release --locked` needs
   neither the Tauri system libraries nor `fleet-core` (see
   `crates/fleet-agent/Cargo.toml`).
4. **Install the agent, on the host.** Paste the token into stdin rather
   than the command line. `--token` also works, but it shows up in the
   process list and in your shell history.
   ```bash
   # a system unit (the default), run as the user whose sessions it drives:
   sudo fleet-agent install --hub https://fleet.example.com --token-file -
   # or a user unit, no root needed:
   fleet-agent install --user --hub https://fleet.example.com --token-file -
   ```
   **No systemd on this host (or not Linux at all)?** `install` checks for
   it first — `/run/systemd/system` and `systemctl` on `$PATH` — and refuses
   cleanly, before writing anything, naming what it checked. Run the agent
   under your own supervisor instead: `fleet-agent run --hub <url>
   --token-file -` (or `--config <path>` once `install` has written one
   somewhere systemd could).
   **Careful with stdin under `sudo`.** `sudo` reads its password from the
   terminal, so a token pasted while it is still asking goes to the password
   prompt, not to `--token-file -`. A pipe into `ssh -tt host sudo
   fleet-agent install … --token-file -` does the same: with `-tt` the
   remote side is a terminal, and sudo's prompt reads the token from it. A
   paste into a terminal is also echoed on screen. When either matters, pass
   a file instead: on the hub, `fleet-hub agent-token laptop > laptop.token`,
   copy it to the host over a channel you trust, run
   `--token-file laptop.token`, then
   `shred -u laptop.token`.
   `install` writes the config (hub URL and token, mode `0600`, created that
   way) and a systemd unit, then runs `systemctl daemon-reload`, `enable` and
   `restart`, printing what it wrote. It never writes the token into the
   unit.
   - **System unit.** The files are
     `/etc/systemd/system/fleet-agent.service` and
     `/etc/fleet-agent/config.json`. The config belongs to the run-as user,
     in a directory that user can reach. The unit runs as `--run-as <user>`,
     which defaults to the user who ran `sudo`. It never runs as root.
   - **User unit** (`--user`). The files are under
     `$XDG_CONFIG_HOME/systemd/user` and `$XDG_CONFIG_HOME/fleet-agent`. It
     stops when you log out unless lingering is on:
     `loginctl enable-linger <user>`.
   - **Other flags.** `--config <path>` puts the config somewhere else, and
     `--no-start` writes the files but leaves systemd alone.
   - **A hub with a private CA.** Pass `--ca-file <bundle.pem>`. Otherwise
     the agent trusts the host's own CA bundle (`$SSL_CERT_FILE`, or the
     usual `/etc/ssl` / `/etc/pki` locations).
   - **`--insecure`** accepts a plain `http://`/`ws://` hub, and **only** on
     loopback (`localhost`, `127.0.0.0/8`, `::1`). It exists for a test on
     one machine. Anywhere else it is refused, because the token would cross
     the network in clear.
   - **`"report_errors": true`** (the default, no install flag) sends this
     agent's own error-level log events to the hub's error channel — see
     *Error reports*. Set it to `false` in the written config to turn it off.
5. **Check it,** on the host with `fleet-agent status [--user]`, or from any
   client with `agent_status`. `fleet-agent status` exits `0` when the
   service is running and connected, and `3` otherwise. It reads the
   agent's own report through `systemctl show`; the agent keeps no state
   file.
6. **Provision it:** `probe_host { alias: "laptop" }` (or wait for the next
   reconcile pass), then `provision_hosts`. Provisioning runs over the
   agent, exactly as it would over SSH.

(No systemd? See the no-systemd note under step 4 — `fleet-agent run` is
the same loop, just in the foreground, under whatever supervises it instead.)

### What the agent does, and does not do

- **Commands.** It runs each command as `bash -c <command>` in the user's
  home directory, which is the same model as an ssh remote command. Its
  environment is the service's, not a login shell's. Fleet's own commands
  start a login shell themselves where they need one. A child's stdin is
  `/dev/null`.
- **Reconnecting.** It reconnects after any drop, waiting up to 60 s with
  jitter. A connection the hub has accepted resets that wait. It answers the
  hub's heartbeat, which the hub sends every 30 s. It gives up on a hub that
  has been silent for three heartbeats. The hub drops an agent that misses
  two.
- **Stopping.** Stopping or restarting the service (SIGTERM, or Ctrl-C under
  `run`) kills every command still running and refuses new ones, then exits
  within 10 s. The unit uses `KillMode=process`, so systemd itself stops
  only the agent: the tmux servers the agent started, and with them your
  Claude sessions, survive a restart or an upgrade.
- **No terminal.** The desktop's terminal view attaches over SSH, and an
  agent host has none. Sessions, prompts, captures, provisioning and
  everything else in the MCP API work. The interactive terminal does not.
- **Offline.** A call for an agent host with no agent connected fails at
  once with `E_AGENT_OFFLINE`; it never waits out a timeout.
- **Updates.** `install --auto-update` adds a timer that runs `fleet-agent
  update`: the hub decides (`update.agent.mode`, pins), the agent installs
  the signed release and goes back to the previous one when it does not
  reconnect. See `docs/updates.md` → *The agent updates itself*.

### Protocol version negotiation

The hub and `fleet-agent` are separately-released binaries, so a frame kind
one side adds can reach a peer that predates it. The wire carries a small,
explicit version to keep that safe:

- The agent's `hello` carries `proto`, its build's protocol version. The
  hub's `welcome` — the first frame it sends back, right after accepting the
  `hello` — carries the hub's own, so each side learns the other's.
- Each side accepts a range, `MIN_SUPPORTED_PROTO..=PROTO_VERSION`, compiled
  into that binary. A number outside the other side's range is refused with
  a WebSocket close naming both versions and which one to update — never a
  bare disconnect. The hub logs `refused a hello: protocol version` at warn
  and never registers the connection; the agent logs `protocol version
  refused` at **error** (louder than an ordinary reconnect, which logs at
  warn) and does not tight-loop over it: it keeps trying, at the slowest
  backoff interval, quietly, until whichever side is behind is upgraded —
  no restart needed on the host once that happens.
- Past that handshake, a frame whose `kind` neither side recognises is
  **not** fatal: it is logged once per kind, at warn, and skipped, so a
  hub ahead of an agent (or the reverse) can add a frame kind the older side
  simply never acts on. A `kind` a side DOES recognise, but cannot parse the
  rest of, is still corruption and still ends the connection — evolution is
  forgiven, damage is not.
- **Which order to upgrade in, today.** `PROTO_VERSION` is 2 (chunked
  payloads: `upload_chunk` / `result_chunk`, 1 MiB each) and
  `MIN_SUPPORTED_PROTO` stays 1, so a proto-2 hub keeps every proto-1 agent
  working — it simply sends that agent whole frames, as before. Upgrade the
  hub first; a proto-2 agent dialling a proto-1 hub is refused as "update the
  hub" and waits at the slowest backoff until it is. The rule for whoever
  bumps `PROTO_VERSION` next (enforced by a
  doc comment on `MIN_SUPPORTED_PROTO` in `fleet-proto`, not by this doc):
  hold `MIN_SUPPORTED_PROTO` at the version BEFORE the bump for at least one
  release. Only under that rule is either order actually safe — the hub
  first (an agent within the still-wide window keeps working unchanged), or
  an agent first (it waits, quietly, at the slowest backoff interval — it
  retries every 30-60 s — until the hub catches up, then reconnects with no
  further action). If a hub is ever bumped WITHOUT holding the floor down, it
  refuses every older agent the moment it restarts; that is a mistake in the
  release, not something an operator can route around by choosing an order.
- **What a pre-versioning agent looks like, if one is ever run against this
  hub.** An agent built before `proto` existed sends a `hello` with no
  `proto` field, which this hub reads as `proto: 0` — below
  `MIN_SUPPORTED_PROTO` (1) today, always. The hub refuses it at the
  WebSocket layer with a close naming both versions and closes with
  `refused a hello: protocol version` in its own log; the agent, being a
  pre-versioning build, has no special handling for this — from the agent's
  side it looks like an ordinary rejected connection, so it just reconnects
  at ITS ordinary (pre-versioning) backoff, indefinitely, `refused a hello:
  protocol version` repeating in the HUB's log every time it tries. The
  operator's fix is the same either way: install a proto-1 (or later)
  `fleet-agent`.
- **What a version-refused CURRENT agent looks like.** Unlike a
  pre-versioning agent, a proto-1-or-later `fleet-agent` that gets refused —
  by the hub (its hello was out of range) or because it decided the hub's own
  `welcome` was out of ITS range, or because the hub never sent one at all
  within one heartbeat (30 s) of connecting — logs the reason at **error**,
  once per attempt, and backs off at the slowest interval (it retries every
  30-60 s, not the normal 1-2-4-8...-60 s growth) instead of hammering a hub
  that has already said no. It keeps trying — a hub upgrade heals it without
  touching the host — just quietly.

### Rotating, narrowing or removing an agent host's token

The agent authenticates with the host's per-host token. Any of these cuts a
**connected** agent off: rotating the token, setting its mode to `readonly`,
removing the host, or moving it back to the SSH transport. The next call
routed to that host drops the connection before anything is sent over it,
and the hub drops it on its next heartbeat (within 30 s) even if nothing is
routed to it.

- **Rotating.** Run `fleet-hub agent-token laptop --rotate` on the hub, then
  re-run `fleet-agent install … --token-file -` on the host. That rewrites
  the agent's own config. Then run `provision_hosts` (without `rotate`) to
  rewrite the host's hooks over the re-authenticated agent. Until then, the
  host's hooks still carry the old token and get `401`, and the agent is
  offline.
- **Why not in-band.** A new token is **never** sent over the agent
  connection, because that connection authenticated with the token being
  replaced. A rotation is how you answer a stolen token, and the connected
  agent may be the thief. So `provision_hosts { rotate: true }` on an agent
  host saves the new token, sends the host nothing, and reports
  `E_AGENT_REINSTALL` with the steps above.
- **A `readonly` token is refused at `/agent`** (`403`). An agent receives
  every command the hub runs on its host, which is more than "readonly"
  promises. **Rotating does not fix this: the new token keeps the old mode.**
  Set the host's token mode to `full` instead:

  ```bash
  fleet-hub host-token-mode laptop full     # …and `readonly` to narrow it again
  ```

  That touches only the mode, not the token, so nothing has to be
  re-installed on the host. A refused agent is retrying with a backoff
  capped at a minute, so it reconnects by itself; restarting it only hurries
  that along. The desktop's `set_host_token_mode` command is `local_only`: it
  changes the mode in the desktop's *own* store, not the hub's, so it only
  does something when the desktop is running its own embedded control API
  (standalone, or as its own agent-accepting server) — never against a hub it
  is paired to as a client. On a hub, always use `fleet-hub host-token-mode`
  above. A token that `fleet-hub agent-token` mints for a host that had none
  is `full`, and the command warns on stderr when a token is not.

### Limits, and what is still open

- **Connection limits.** `/agent` accepts a connection only from a host on
  the agent transport (`403` otherwise). Every provisioned host holds a
  token for its hooks, SSH hosts included, and theirs are refused. It
  accepts at most **2** connections per host and **64** across the hub,
  counted before the upgrade (`429` beyond that). A write that takes longer
  than 5 minutes ends the connection, and a replaced connection is torn
  down at once.
- **Memory is bounded, not small.** One message can be up to about 267 MiB,
  because "Move to host…" carries a transcript of up to 200 MiB. The
  WebSocket library reserves that much as soon as it reads a frame's
  header, so the worst case is still 64 × ~267 MiB of reservable memory.
  Splitting large transfers into small frames is the durable fix, and it is
  not built.
- **Most answers get the full budget.** The hub decodes each answer against
  the largest output any in-flight request allows. Routine commands ask for
  no cap (only the transcript read needs the full size, but nothing tells
  them apart). So in practice a connected agent may send a frame up to the
  full ceiling whenever any routine call is in flight. This is documented,
  not fixed.
- **Other open items.**
  - An upload waiting in the agent's queue still completes after its
    connection has gone.
  - An upload rewrites the file in place, so a process that already had the
    old file open can read the new contents. That is the same as SSH's
    `cat >`.
  - `install` does not check who can write to the directory holding the
    `fleet-agent` binary. Put it somewhere only root (or the run-as user)
    can write.
  - A command's output is cut at 200 MiB, and the caller is not told: the
    agent flags the cut, but the hub's command interface has nowhere to
    carry the flag, so a cut answer looks complete.

## Demo rows, for setting a client up

A client paired to a hub that has never run a session shows an empty list, and
an empty list is indistinguishable from a broken pairing: no sessions, no
hosts, no error. Somebody setting the app up for the first time — or a script
doing it unattended — cannot tell "it works and there is nothing here" from "it
does not work".

```bash
fleet-hub demo-seed              # two fake hosts, two projects, six sessions
fleet-hub demo-seed --hosts 4
fleet-hub demo-seed --clear      # remove them again
```

Every row is named `demo-…`, and that prefix is the whole mechanism: `--clear`
removes exactly the rows `demo-seed` wrote and nothing else. There is no
`is_demo` column and deliberately so — a migration to support a development
convenience would put the concept in every production database for good.

**It refuses a hub that already has rows of its own.** Seeding a live fleet
would mix invented sessions into a list an operator makes decisions from, with
only the names telling them apart. `--force` is there for somebody who
genuinely wants both. Re-seeding a hub that holds *only* demo rows is the
ordinary case and needs no flag.

The fake fleet is arranged to be worth looking at rather than merely non-empty:
one host reachable and one not, and one session in each of `working`, `blocked`
and `completed` — `blocked` being what a client's "needs attention" filter
keeps. A client that groups, filters or draws reachability wrongly shows it
here, instead of the first time something actually goes down.

## Pair a phone

A *client* is a device that drives the fleet without being a fleet host: a
phone, a tablet, a browser on a laptop that is not provisioned. It gets its
own token — not the master one — which you can see, name and revoke.

On the hub, with the daemon running:

```bash
fleet-hub pair --name phone
```

That prints a QR code, the URL under it, and how long the code is good for:

```
█▀▀▀▀▀█ ▀▄█▀▄ █▀▀▀▀▀█
…
https://fleet.example.com/pair#ABCDEFGH

client:  phone (full)
expires: in 600 s — the code works once, and a hub restart voids it
Scan it with the claude-fleet app on the device you are pairing.
```

Scan it with the device's camera. The page it lands on says what to do and
nothing else — no JavaScript, no auto-redeem. The app on the device posts the
code to the hub's `/pair` once and gets a token of its own back.

What travels in that QR is a **pairing code**, not a token: eight characters,
single-use, and it lives in the URL *fragment*, which a browser never sends —
so no proxy, access log or scroll-back of your terminal ever holds a
credential. Codes live in the hub's memory only, so restarting the daemon
voids every outstanding one. Mint a new one and walk back to the phone.

Four options:

```bash
fleet-hub pair --name kiosk --mode readonly   # observe only; the default is full
fleet-hub pair --name phone --ttl 120         # seconds the code stays valid (30–3600)
fleet-hub pair --name mac-desktop --trusted   # its prompts reach agents unmarked (see *Clients*)
fleet-hub pair --name ada-laptop --person ada # whose device it is (see *Clients*)
```

**`--person` is optional, and its default is you.** Omitted, the device is
paired to this hub's own owner — so pairing your own second phone needs
nothing new. Name a person and the device is theirs instead: it sees their
sessions and the ones they have been shared, and no others. The person is
created on first use, so handing a laptop to a new colleague is one command.
`--person` is not for `--mode peer` or `--mode updater`: neither is anybody's
device.

Pairing needs a **running** hub (`fleet-hub serve`): the code only means
something inside the process that will redeem it. `fleet-hub pair` reads the
master token out of the data dir, resolves the port and the TLS mode the same
way `serve` does (`--port`/`--tls`, `FLEET_HUB_PORT`/`FLEET_HUB_TLS`, the
stored setting, then the default) and calls the hub's own `/mcp` on loopback —
so run it on the hub's machine, as the user the daemon runs as. When the hub
terminates TLS itself (`--tls cert`), `pair` and `client list|revoke` speak
TLS too, with certificate verification off — but unlike the healthcheck
probe, this connection carries the master token. What makes that safe is the
address: it is hardcoded loopback (`pair.rs`:
`SocketAddr::from(([127, 0, 0, 1], port))` — only the port is configurable),
so the token never leaves the machine. The residual exposure — a local
process squatting the port while the hub is down — is the same one the
already-documented plaintext path carries. See *Single binary with its own
certificate* below.

**Changing the public URL needs a restart.** The `hub` field in the `/pair`
response — the base URL the freshly paired device will talk to — is a snapshot
taken when the server started, while the URL inside the QR is read fresh on
every mint. So after changing `hub.public_url` (a `--public-url` run, or the
stored setting) on a *running* hub, a phone can be sent to the new address by
the QR and then handed the old one to talk to. Restart `fleet-hub serve`
before pairing anything, and the two agree again.

## Clients

```bash
fleet-hub client list
fleet-hub client list --include-revoked
fleet-hub client revoke phone
fleet-hub client trust mac-desktop
fleet-hub client untrust mac-desktop
fleet-hub client bind contractor-phone 2
fleet-hub client unbind contractor-phone
fleet-hub client bind-person ada-laptop ada
fleet-hub client unbind-person ada-laptop
fleet-hub person list                       # which person each `person <id>` is
fleet-hub client grant mac-desktop assets
fleet-hub client ungrant mac-desktop assets
```

`client list` prints one line per client, newest first:

```
NAME              MODE      PERSON    ORG    TRUSTED            ASSETS             CREATED            LAST SEEN          REVOKED
mac-desktop       full      person 1  -      2026-09-21 10:02Z  2026-09-21 10:02Z  2026-09-21 10:01Z  2026-09-21 10:05Z  -
ada-laptop        full      person 3  -      -                  -                  2026-10-02 11:30Z  2026-10-02 12:04Z  -
contractor-phone  full      person 4  org 2  -                  -                  2026-09-27 08:00Z  2026-09-27 09:12Z  -
kiosk             readonly  -         -      -                  -                  2026-09-17 09:12Z  -                  -
```

PERSON is whose device it is, printed as an id (`person 3`) because that is what
the device row carries; `fleet-hub person list` names the ids. A dash means
nobody — a device `client unbind-person` cut loose — and such a device sees no
private session at all. See *Who owns a session*.

The token itself is never shown again: only its SHA-256 is stored, and the
plaintext exists in the one `/pair` response that minted it. Lost it? Revoke
the client and pair again under the same name.

What a client may do:

- **`full`** — whole-fleet *session* control: list, spawn, steer, kill, read
  transcripts, follow the event stream. The same reach a `full` per-host
  token has.
- **`readonly`** — the observing tools only (`list_*`, `capture_session`,
  `session_transcript`, `session_conversation`, `session_history`, `repo_*`,
  `wait_for_*`, …). Anything that sends, kills, deletes or writes answers
  `E_FORBIDDEN`.
- **The composer's chip row is shared.** `quick_replies` is one tool that both
  reads and replaces the fleet's quick replies — the prompt presets the
  desktop composer and the phone both draw above their text box — so it is
  classified as a write: a `full` client may call it and a `readonly` one is
  not shown it (a readonly device draws no chip row to begin with). The list
  itself is fleet state in the hub's database, not a device preference, so a
  chip written on the laptop is on the phone and the other way round — its
  order and each chip's `auto_send` (a tap sends at once instead of only
  filling the box) included. Only a person's token replaces it: a per-host
  token and the operator may read the list but get `E_FORBIDDEN` on `set`,
  since an auto-send chip is a prompt one tap away. A `set` may name the list
  it last read as `expected`; if another device saved in between, it answers
  `E_CONFLICT` instead of overwriting that edit.
- **Neither mode reaches fleet admin.** `provision_hosts`, `add_host`,
  `remove_host`, `hide_host`, `apply_sync`, `set_secret`, `set_host_layers`,
  `set_host_harnesses`, `pair_client`, `revoke_client`, `set_client_trust`
  and `list_clients` are master-token only, so a paired phone can neither
  re-provision the fleet nor pair a second device nor revoke (or trust) your
  own client — nor even enumerate the other devices you have paired.
- **Except what you grant: the asset catalog.** `fleet-hub client grant
  <name> assets` lets that one client manage the asset catalog from its
  Assets tab — configure and load the checkout, create / edit / delete
  assets and their resources, lint, commit and push, Sync (plan and
  apply), Secrets and layers — through the hub's `catalog_admin` tool.
  Everything else above stays the master's. Only a `full` client bound to no
  org can hold it (a Sync writes to every host, across orgs); `fleet-hub
  client ungrant <name> assets` takes it back, and `client list` shows it in
  the ASSETS column. The hub reads the grant on every call, so both take
  effect from the client's next call, and neither needs a running hub. Grant
  it to the desktop whose keyboard is yours, like `trust` — never to a token
  an agent holds: a Sync writes files and plugins to every host. A per-host
  token can never use `catalog_admin`, and is not shown it. A grant names
  one catalog: `--catalog <name>` grants an org's catalog instead of the
  personal one (`client ungrant <name> assets --catalog <name>` takes it
  back), and the client may then touch only the catalogs it holds — admit
  hosts to them, load them or list their layers, and apply a Sync plan only
  when it holds every catalog that plan writes from. The fleet-wide actions
  (`plan_sync`, `apply_sync`, `inventory`, secrets, `resolve_preview` and
  the rest) also need the personal grant: an org-only grant covers that
  catalog's config, load, layers and admissions, nothing more.
  `list_catalogs` is open to the master or a person's own unbound full
  device, never an org-bound client; only the master token may
  `add_catalog` or `remove_catalog` (a removal drops every other client's
  grant on that catalog, and only the master could add it back). `client
  list`'s ASSETS column is the personal grant; `catalog list` shows every
  grant.
- A prompt typed on a phone reaches an agent **marked** as untrusted input,
  naming the client it came from, unless you have **trusted** that client.
  `raw: true` is the master token's alone.
- **Trusted.** `fleet-hub pair --trusted`, or `fleet-hub client trust <name>`
  later (`set_client_trust` over the API), says the device's words are your
  own: everything it sends with `send_prompt`, `broadcast_prompt` or
  `send_message` is delivered *without* the marker line, exactly as the
  master's `raw: true` is, so the receiving agent is not told to distrust
  what you typed. The grant is per client, revocable with `client untrust`,
  shown in `client list`, and takes effect on the client's next call; the
  session timeline still records the call as `client:<name>`. Trust a device
  whose keyboard is yours — the desktop app paired to this hub, your phone —
  never a token an agent holds: what makes an agent's output safe to relay
  is precisely the marker. A fresh pairing is untrusted, and a hub older than
  this option keeps marking everything, which is the safe direction.
  Since declarative pages P6 trust also lets the device **change the
  fleet's settings** (`set_setting`, and applying or rejecting proposals):
  an untrusted device reads them and can only propose. See *Proposed
  settings and their history* below.
- **Whose device it is** (multi-user M1). A paired device belongs to one
  **person**, and a person's sessions are private to them: the device sees
  their sessions and the ones they have been shared, and no others.
  `fleet-hub pair --person <person>` says whose it is at pairing;
  `fleet-hub client bind-person <name> <person>` hands an already-paired
  device over afterwards, and `fleet-hub client unbind-person <name>` takes
  it back — the device stays paired (only `client revoke` ends that) but
  belongs to nobody and then sees no private session at all. The person is
  created the first time you name them, so a new colleague's laptop is one
  command; `fleet-hub person list` is what maps the `person <id>` in the table
  above back to a name, and `person rename` / `person disable` are the other
  two things you can do to one. Both `bind-person` and `unbind-person` write
  `state.db` directly, so neither needs a running hub, and both take effect
  from that device's next request (an open event stream ends at its next
  beat). Leaving `--person` out pairs the device to **this hub's own owner**,
  which is what makes pairing your own second phone need nothing new; a peer
  hub link and an updater token are nobody's device and are refused either
  way. Only the hub owner's own device reaches the fleet's settings — see *On
  a paired device* under *Configuration*.
- **Sharing a session, and the sessions nobody owns** (multi-user M1). A
  session a person starts is private to them; they share it with one
  colleague at a time, at `watch` (read it) or `drive` (also prompt it),
  through `session_share` / `session_unshare` / `session_narrow` and
  `session_access` on their own device. A grant only ever moves **downward**
  — revoke it, or narrow `drive` to `watch`; nothing widens one, so widening
  is an explicit revoke and a fresh share — only the owner makes one, a
  grantee cannot share on, and **sharing never gives a terminal**: the
  terminal is the desktop's own SSH into the host, which no revoke of ours
  could reach. There is no team recipient in M1 (that needs memberships, and
  arrives in M2) and no `own` level: `own` is the set of operations only the
  owner may perform — killing, restarting, renaming, moving, forking,
  re-tagging, re-sharing — and no grant reaches it.

  A session fleet did **not** start — one a reconcile pass found in a tmux
  server somebody started by hand — belongs to nobody and is `unclaimed`.
  Such a row leaks nothing: a caller who is not entitled to it is told a
  per-host COUNT and no more, and on a hub with one person that count is the
  only thing that changes about the rows they could already see. On a hub with
  more people the count goes to whoever administers the host — the hub's
  owner, or the admins of the company that owns the hub — and to an org's
  admins for its hosts when the owner switched that on (*Companies: members
  and roles*). Claiming one
  needs proof that the claimant is IN the session's pane — never host access
  on its own, and never org membership: the agent inside the session calls
  `session_claim` and its request's `X-Fleet-Pane` header has to name that
  session's active pane, so a per-host token that merely happens to be on the
  same machine is refused. On the hub machine:

  ```bash
  fleet-hub session unclaimed                 # per-host counts
  fleet-hub session unclaimed --host mefistos # that host's rows, with ids
  fleet-hub session claim 42 --person ada     # give one to a person
  ```

  Both write and read `state.db` directly, like `client bind-person`: the
  master token cannot reach `session_claim` over the API at all (it has no
  pane to prove), and on a hub with more than one person this listing is the
  only way a human sees those rows. A claim is addressed by **row id**, never
  by tmux name — a name is reused by the next session on that host — it is
  refused on a session that already belongs to someone (ownership is never
  transferred; its owner shares it instead), and it is recorded on that
  session's own timeline without announcing the row to every connected
  client.
- **Bound to an org** (work graph M14.1b). `fleet-hub pair --name <name>
  --org <org id>`, or `fleet-hub client bind <name> <org id>` later
  (`work_admin { action: "assign_client", name, org_id }`; no `org_id` and
  `fleet-hub client unbind <name>` unbind), restricts a client to one
  organisation: it reads that org's work **and sessions** only — the Work
  view (`work { tree | task | session_tasks | review | rules | rule_preview
  | views }`), tickets, Today, conversations, `list_sessions`, every
  session-addressed tool, `fleet_health`'s trackers, spend and counts, and
  every `/events` frame. An org-bound client is still somebody's *device*, so
  the person fence applies inside the org as well: its `fleet_health` session
  counts, per-host spend and `hosts[]` cover the sessions that device's person
  owns or was granted, not every session in the org. Whether it also sees *unassigned* work and sessions
  (no org) is the org's switch `bound_sees_unassigned` (decision D31): on by
  default, as a host sees them; the master turns it off with `fleet-hub org
  set <id> --bound-sees-unassigned off` (`work_admin { action: "update_org",
  org_id, bound_sees_unassigned: false }`; on a standalone desktop, Settings →
  Organisations → *Bound devices see unassigned*), and `org list`
  marks such an org *bound devices: own org only*. The org's bound clients then see
  only rows assigned to it. Another org's session or task answers exactly as
  one that does not exist, whatever `isolate_sessions` says (a bound client
  asked to be restricted, so the session fence is always on for it), and
  `work` frames are not sent to it at all (it re-reads through `work { … }`).
  It cannot start or resume a session in another org's project or host, read
  what moving a task between orgs would change (`org_impact`) or move one
  (`assign_org`), write placement rules (`rule_save` / `rule_delete`), or
  change trust or reopened work; it may decide links and place tasks it
  sees, and keep its own org's saved views (M14.1c). The binding and the switch take effect from its
  next request (re-binding invalidates the token cache; an open event
  stream re-reads its scope and ends at its next beat when re-bound).
  Deleting the org leaves the client bound to an org that no longer
  exists — it then reads nothing of any org and no unassigned data either,
  never every org (fail closed). A peer hub link is never bound. Use it for
  a device that belongs to one company's work, such as a contractor's
  second phone.

**Work on a phone** (the work graph, M8). A client token is served `work`
and — `full` only — `work_link`, and never `work_admin`, so tracker
administration stays on the desktop. With it the phone shows each session's
ticket and groups by it, confirms or rejects a suggestion, starts and resumes
work from a ticket, and (M8.6) reads *Today* (`work today`), a ticket's
acceptance criteria (`work card`) and past work (`resume_plan`), asks a
session for a handover note (`work_link handover`, `full`), and labels and
filters by organisation (`work orgs`). A client token is never org-scoped
(the org fence of M5 is for per-host tokens, the ones agents hold), so the
org chips on a phone are a way of reading the fleet, not a fence. Every one of these is gated on the hub's own
`tools/list`, action by action, so an older hub simply shows less.

`revoke` takes effect on the client's very next request — the auth layer only
resolves live rows — and an open event stream ends within one heartbeat
(15 s). The row is kept, revoked, for the audit trail, and the name becomes
free to pair again:

```
revoked phone (paired 2026-09-17 09:12Z); its next request is refused and the name is free again
```

## File downloads

A session on a host you cannot reach makes a file you want — a PDF, a
CSV, an image, a build. Have it **sent to your devices**: the hub copies
the file off the host and keeps it, and the phone's *Files* tab and the
desktop's *⤓ Downloads* (footer) list it for you to save, share or open.

- **Claude sends it.** Ask the session ("send me the report"): its Claude
  calls the control API's `send_file { session_id, path }` with its own
  session id (`whoami`). A host's token may only send from its own host.
- **You pick it.** In the desktop's Files tab, open the file and press
  *Send to downloads*.

The copy runs in the background in 8 MiB pieces over the same SSH (or
agent) link the hub already uses; the row shows *copying…* until it is
ready, and a toast says so. Folders are refused: zip them first.

| setting | default | |
|---|---|---|
| `downloads.max_file_mb` | 100 | largest file one send may copy |
| `downloads.max_total_mb` | 2048 | everything kept together; a new file pushes out the oldest |
| `downloads.keep_secs` | 7 days | then the GC sweep removes it (`0` keeps it until removed) |

The copies live in `<data dir>/downloads/` (mode 0700), named by row id.
A copy that was in flight when the hub stopped is marked failed on the
next start. The bytes are served at `GET /downloads/<id>` behind the
same bearer token as `/mcp`; a client bound to an org sees only its org's
files, and a host's token cannot fetch them. Design:
`docs/superpowers/specs/2026-10-03-file-downloads-design.md`.
## Who owns a session

Every session has an owner, and a session fleet started is private to that
person. This is the model behind the `--person` flags in *Pair a phone* and the
sharing bullets in *Clients*: who a person is, what privacy covers, and where
it stops.

### People and devices

A **person** is a row in the hub's database — a name, and nothing else. There
is no registration, no password and no account service: a fresh hub mints its
own owner on first run, and anyone added later is created the first time you
name them. A **device** is a paired client (*Pair a phone*), and it belongs to
exactly one person. That is what makes a session's owner unambiguous.

```bash
fleet-hub pair --name ada-laptop --person ada   # a colleague's first device
fleet-hub pair --name my-phone                  # no --person: your own
fleet-hub client bind-person ada-phone ada      # hand an existing device over
fleet-hub client unbind-person ada-phone        # take it back; still paired
fleet-hub person list [--json]                  # everyone, with their ids
fleet-hub person rename ada --to ada.lovelace   # or --display-name "Ada L."
fleet-hub person disable ada --force            # end their reach (see below)
fleet-hub session unclaimed [--host <alias>]    # the sessions nobody owns
fleet-hub session claim 42 --person ada         # give one to a person
```

`person list` is the only thing that maps an id to a human: `client list`
names a device's person as `person 3`, because a device row carries an id, and
this is what says who 3 is. It prints people and nothing else — the id, the
name, the display name, which row is this hub's own owner, when each was
created, and when a disabled one was disabled. It never prints a person's
sessions, nor a count of them: there is no admin view of somebody else's work
on a hub, and this listing is not a way around that. Disabled people are in
it on purpose — a grant and a session still point at them, and a row the
listing hid would be one you could not act on.

`person rename` changes the text and nothing else. Every grant is addressed to
the person's **id**, so a rename cannot hand somebody's share to a different
human, and the owner flag does not travel with the name either — rename this
hub's placeholder `owner` to your own name whenever you like. A name only a
*disabled* person holds is free to take, so a departed colleague never blocks a
new one; a name a **live** person holds is refused.

A person's reach is the devices bound to them. **Disabling a person ends both
halves of it in one transaction:** every device of theirs is revoked, and every
live grant *to* them — every share anybody made them — is revoked with it, so a
device minted for them later cannot restore access you believed you had removed.
Grants they *made* are untouched, because a grant belongs to the session's owner
and nothing here gives anyone authority over somebody else's share. And **their
own sessions do not move**: those rows stay private and stay theirs, unreadable
by anyone else. That is the whole answer to "a colleague has left" — their
access ends, their work is not re-attributed, and no admin inherits it. The
hub's own owner cannot be disabled: every gate keys on that row.

`fleet-hub person disable <name>` is how you do it, and it does both halves in
that one transaction. It refuses until you add `--force`, and the refusal is
the confirmation: it prints how many devices and how many grants would go, and
writes nothing. With `--force` it says what it actually revoked, counted
afterwards, so you can see both halves happened:

```
disabled ada (person 3): 2 device(s) revoked, 1 grant(s) to them revoked
```

What it does **not** do, plainly: it does not re-attribute their sessions —
those rows stay theirs and stay private, readable by nobody, and no command
here moves them; it does not touch the grants *they* made, which belong to each
session's own owner; it cannot disable this hub's owner, because every access
gate keys on that row (rename the owner instead, or revoke their devices one at
a time with `client revoke`); and **there is no re-enable in this release** —
`person disable` has no inverse, and a device cannot be bound to a disabled
person, so plan on adding a fresh person if someone comes back. No tool over
the control API disables anybody either: this is a command on the hub machine,
run as the person at its console, like `client bind-person` and `session
claim`. It needs no running hub, and a running one honours it from its next
request.

### Companies: members and roles

A person can be a **member** of an organisation (org administration phase D,
`docs/superpowers/plans/2026-10-06-org-administration-phase-d.md`), with one of
three roles:

| Role | What they get |
|---|---|
| **admin** | Everything the hub's owner can do to that org in Settings → Organisations — its settings, colour, isolation, auto-tidy, Jev consent, its own settings, its spend, its members, its members' devices (pair, trust, revoke) and its catalogs' grants — and nothing outside it. |
| **member** | The org's work, and what is shared with the org. |
| **viewer** | The org's work view and overview, read-only. |

A member's **devices follow their memberships**: a colleague's phone is fenced
to the org they are in (to one of them, when they are in several — `bind_device`
picks which), and a viewer's phone is read-only whatever it was paired as. The
hub's owner and a person in no org keep whatever their device was bound to, so
an upgrade changes nobody. A person taken out of their last org keeps their
row and their sessions, but their devices then read nothing of any org — a
departed colleague does not fall back to the whole fleet's work.

Some things stay the **hub owner's** whoever administers an org, because each
decides which company something belongs to: the orgs' rules, which org a
tracker belongs to, whether bound devices see unassigned work, people's names,
disabling a person, and the two switches below. **Who administers a host** is
the hub's owner — or, when a company owns the hub (`org own-hub`), that
company's admins: they route hosts into orgs and see how many sessions on each
host nobody has claimed. An org's own admins see that count for its hosts only
when the hub's owner turns it on (`org unclaimed-count`). Registering,
removing and provisioning hosts stay the operator's, as before.

```bash
fleet-hub org member add 1 jane --role admin    # a new name becomes a person
fleet-hub org member list 1
fleet-hub org member grants 1 bob [--narrow | --revoke]
fleet-hub org member rm 1 bob                   # also revokes what was shared with
                                                # bob on the org's sessions
fleet-hub org own-hub 1 | --none                # the company that owns this hub
fleet-hub org unclaimed-count 1 on|off          # its admins see unclaimed counts
```

These write `state.db` directly, like `person` and `client bind-person`; a
running hub honours them from its next request. From a device, the org page's
**Members** section does the same (`org_admin`).

**Sharing with a team** is a grant to an org: the owner of a session shares it
with an org they are a member of (`session_share { org }`, or *an org* in the
Share sheet), and it reaches the org's members and admins who are in it at that
moment. Somebody who joins later gets nothing from it until the owner shares
again — changing a membership never widens a grant — and a viewer never
receives one. Taking a member out of the org revokes what was shared with them
on its sessions (`--keep-grants` keeps it); an admin can also lower or revoke
those grants while they stay. Downward only: no admin can widen a grant, add a
recipient or redirect one, and **no admin reads a member's private session**.

### Two people on one host

Give each person their own unix account on a shared host, and add each account
as its own fleet host alias — `box-ada`, `box-martin` — with its own per-host
token and its own `~/.claude`. That is the deployment rule, and it costs no
configuration: the unix accounts keep the work apart on disk, and the aliases
keep the tokens apart in fleet.

**On a host where two people share one unix account, fleet-level privacy is
cosmetic.** The person with the account reads the other's transcripts under
`~/.claude/projects`, attaches to the other's tmux session and reads the pane.
Fleet does not claim otherwise and no setting changes it. **And the pane proof
is exactly as strong as that rule.** Every Claude on a host authenticates with
that host's one token, so the token cannot say which session the caller is
sitting in; the pane does. Each request carries `X-Fleet-Pane`, and a host token
reaches the unclaimed rows of its own host plus the one row whose active pane
that header names — nothing else, and in particular not another person's private
session on the same host. But any process that can run `tmux list-panes` on a
host can enumerate every pane id on it, so presenting one proves host access,
not pane occupancy: the proof works because a separate unix account cannot read
another account's tmux socket, not because a pane id is a secret. One account
and one alias for two people makes the proof guessable; an account and an alias
each leaves nothing on that token's host to guess at.

Two mechanics follow from how the proof is resolved:

- It is the session's **active** pane, as the last reconcile pass saw it. A
  claim run from a non-active pane of a split window is refused with
  `E_INVALID_STATE` naming that rule rather than `E_NOTFOUND` — run it from the
  session's active pane. (`session_claim` also answers `E_FORBIDDEN` for
  another session's pane and `E_EXISTS` for a row that already has an owner:
  ownership is never transferred, its owner shares it instead.)
- Nothing about a proof is stored anywhere. It is read off the connection and
  re-resolved on every single request, and it lapses on its own the moment
  reconcile writes a different pane onto the row.

The header itself is written by provisioning, into the host's
`mcpServers.claude-fleet` entry. A host provisioned before multi-user M1 sends
none, proves nothing, and is refused every call that needs the proof — see *The
pane header* under *Add and provision hosts*, and re-provision after upgrading.

### What a watcher actually sees

A `watch` grant is a read. The recipient's sidebar shows the row, and the
conversation panel reads its turns, tool calls, timeline and activity. The pane
arrives as a **snapshot on a poll** (`capture_session`), so a watcher is always
slightly behind the owner's own terminal and never sees keystrokes land. There
is no terminal, and withholding one is not what sharing does: the desktop's
terminal is its own SSH connection to the host with no hub in the path, so a
share could not confer one and a revoke could not take one away. If a watcher
independently has SSH to that host they can attach to the tmux session
themselves — that is their access to your machine, which sharing neither created
nor claims to revoke. A `drive` grant adds prompting on top of the same view.
Everything only the owner may do — killing, restarting, renaming, moving,
forking, re-tagging and re-sharing — is reached by no grant at all.

A share and a revoke both reach an open client as a `grant:changed` frame, so
the buttons and the pane follow within the same beat (*Sharing, revoking, and
the fifteen-second bound*). What a client computes from the row and its own
grants is a display, never a permission: the hub judges every request itself,
whatever the client decided to draw.

### Privacy, precisely

**There is no admin override.** Not audited, not break-glass, not one session at
a time. A private session is readable by its owner and by whoever the owner
shared it with, and by nobody else; the application has no path around that, and
`fleet-hub` has no subcommand that prints another person's transcript.

That has to be said precisely, because "admin" names two different people and
only one of them is fenced:

| Role | Fenced? |
|---|---|
| **Org admin** — authority over an organisation's settings and membership (*Companies: members and roles*) | **Yes.** There is no path to a member's session content: they see the org's spend and members, never a member's private session. |
| **Hub operator** — holds the master token and `state.db` | **No, and by design.** They pair a device as any person, read the database, and reach the hosts. |

The operator is unfenced on purpose rather than by omission. `fleet-hub pair
--person <name>` mints a code for any person and `POST /pair` ties redemption to
nobody, so whoever holds the master token can hold a device that is a
colleague's; they also hold `state.db` and a shell on the hub's machine.
Fencing that inside the application would be theatre.

**The consequence, stated plainly: in a deployment where the company's admin IS
the hub operator — the normal shape for a small company — privacy holds against
colleagues and against anyone whose authority comes only through the
application, and not against the person who runs the machine.** Give the master
token to the people you would give the database to, and nobody else.

**Outside the application fleet promises nothing, and must not pretend to.** The
hub's operator reads the hub's database; a host's unix owner reads that host's
transcripts; the terminal attaches over SSH outside the hub entirely, as does
the pane's file drop. Fleet's privacy is about what the application shows, not
about what the machine's owner can do. These are two protections with two
owners: fleet keeps the application boundary, and your machine policy — who
holds which unix account, who has SSH where — keeps the rest. A company that
needs an investigative route has one there: the host, its unix account, the
transcripts on its own disk, and its AI provider's audit trail.

This release shares with a **person**, and only a person. There is no team
recipient and no "the whole org may see it" visibility — both need memberships,
which arrive later — and **no admin has any authority over a grant at all**:
revoking or narrowing a departed colleague's grants comes with those
memberships, which are what say who departed from what. Until then a departed
person's grants simply stand, harmless for as long as no device of theirs
answers.

### Recovering administrative access

Pairing is the only way a device comes into being, and the database keeps only a
token's SHA-256 — so a lost device has nothing to restore. Revoke it and pair a
replacement onto the same person:

```bash
fleet-hub client revoke ada-phone
fleet-hub pair --name ada-phone-2 --person ada
```

The person row, their sessions and their grants are untouched: nothing was
derived from the device that is gone.

**An owner who has lost every paired device recovers through the hub machine,
and there is no path that avoids it.** On that machine, as the user the daemon
runs as:

```bash
# the master token, if you no longer have it (reads state.db, mints nothing)
docker compose exec fleet-hub fleet-hub token show
#   bare binary, as the daemon's user:
#   sudo -u fleet env FLEET_HUB_DATA_DIR=/var/lib/fleet-hub fleet-hub token show

# then pair a replacement device onto yourself (--person omitted: your own)
docker compose exec fleet-hub fleet-hub pair --name new-laptop
```

If the master token itself is lost or was exposed, `fleet-hub token regenerate`
mints a fresh one — the running daemon keeps accepting the old one until it is
restarted, so restart the hub and reconfigure every MCP client afterwards.
Pairing needs a running hub; `client revoke`, `client bind-person` and
`session claim` do not.

So recovery needs shell on the hub's machine, as the user the daemon runs as —
or a `state.db` backup (*Backups*) and a machine to restore it onto. Keep one of
the two: a hub whose only administrative path was a device you have lost cannot
be recovered from a phone.

## Link two hubs

Two fleets can message each other's sessions by address:
`<fleet>/session/<host>/<name>`. One hub **dials** (it needs a route to the
other), the other **listens**; messages flow both ways over the dialer's
connection, with about one round-trip of latency.

1. On the hub that will listen: `fleet-hub pair --mode peer --name <label>`.
2. On the hub that will dial: `fleet-hub peer add https://<other-hub> <code>`.
   `fleet-hub peer list` shows the link `connected` within a few seconds.
   From a desktop paired to that hub, Settings → Federation → Link a hub does
   the same (a trusted full device), and lists each link's state, latency and
   messages carried.

Pair only a hub you trust: the **first** peer token to claim a given (never
linked) fleet id gets that link, and no other token can claim the same fleet
afterward.

To **re-pair** a link you already have (a lost or leaked token, a rebuilt
hub), keep the old link — its waiting messages are what a re-pair keeps:

1. On the listening hub: `fleet-hub client revoke <old peer client>`, then
   `fleet-hub pair --mode peer --name <label>` for a new code. (Without the
   revoke, the new code's first exchange is refused: `fleet <id> is already
   linked to another peer token; revoke that client first (…), then mint a
   new pairing code`.)
2. On the dialing hub: `fleet-hub peer add https://<other-hub> <new code>`.
   Do **not** `peer remove` the old link: that fails its waiting messages
   back to their senders.

The order matters. That refusal is final for the token it was given: the
dialing hub's new link stops as `refused` and is never retried, so revoking
the old client afterwards revives nothing. After a re-pair in the wrong
order, revoke the old client, then mint a **new** pairing code and `peer add`
it again; the refused row on the dialing hub can be `peer remove`d by its ID
(it holds no messages), and the client the refused code minted on the
listening hub is revoked like any other (`fleet-hub client revoke <label>`).

The new link's first exchange reaches the other hub, but the old link may not
have noticed its revoked token yet (it can be parked in a long-poll for up to
25 s, or backing off). Until it does, `peer list` shows the new link
`retrying` with `fleet <id> is still linked (link <N>); waiting for it to
stop`, and it carries no messages either way. Once the old link's next
exchange is refused, the new credentials move onto the old link — same link
id, the new row disappears — and the waiting messages go out and come in over
them, once each. This normally takes seconds; both links back off up to a
minute between tries, so it can take two.

The same wait is what keeps a newly paired hub from claiming a fleet you
already talk to: a new link whose handshake names the fleet of a link that is
still `connected` or `retrying` never takes it over, delivers nothing and
accepts nothing; it waits, and `fleet_health` counts it as a link down. If you
did not mean a re-pair, remove the waiting row by its ID (see below) — it holds
no messages. Only if the old link can no longer reach the other hub at all
(the hub moved to a new URL) will it never hear the refusal; then `peer
remove` the old link to let the new one take the fleet, knowing its waiting
messages fail back to their senders. A peer that answers with a malformed
fleet id is `incompatible`; one that answers with this hub's own fleet id is
`refused`.

What a linked hub can do: deliver messages into your sessions' inboxes,
marked as untrusted input, and receive your sessions' messages to it. What it
cannot do: call any other tool, read `/events`, type into a pane (a message
from another fleet wakes an idle session with a fixed one-line nudge only,
never its text), or forward your messages to a third fleet.

Remove a link on either side with `fleet-hub peer remove <fleet-id>` or
`fleet-hub peer remove <id>` (the `ID` column of `peer list`); messages still
waiting on it fail back to their senders as `message_undeliverable`, as does
any message a peer has not taken within 7 days. A link that is refused (a
revoked token, a fleet-id mismatch) or incompatible (a hub without
`peer_exchange`) stops retrying; pair again to restore it — waiting messages
are kept for the week. A link that stopped **before** its first handshake
completed (its fleet reads `(handshake pending)` — a self-link, a fleet
already linked the other way, a token the other hub refused) has no fleet id
to remove it by and holds no messages: remove it by its ID, which also clears
the `fleet_health` line it keeps raised.

A message's `kind` crossing a link is a short lowercase token — 1 to 32
bytes of `[a-z0-9_-]` (the default, `message`, always passes) — and never
`question`: a message from another fleet cannot hold a session's `Stop`
hook. Sender and recipient addresses are at most 256 bytes. The sender
checks all of this before queuing, so a message the other hub would refuse
comes back as an immediate error, never as a `message_undeliverable` a week
later.

If you put a reverse proxy in front of a **listening** hub, it must allow a
request of at least 35 s: the dialer's idle exchange long-polls up to 25 s,
and a proxy timeout shorter than that (plus margin) drops the connection
mid-poll and the link sits `retrying`. It must also allow request bodies of
at least 1 MiB (nginx's default `client_max_body_size` is exactly 1 MiB): one
exchange carries up to 512 KiB of messages each way, encoded.

Limits: plain `http://` peers are allowed only on loopback (`--insecure`);
at most 50 messages and 512 KiB of them per exchange in each direction (a
single larger message still goes, alone), and 32 KiB per message; an unread message
from another fleet whose recipient session is later deleted is not reported
back to the sending fleet.

**What ends a link versus what just interrupts it.** The dialer treats HTTP
401, or a structured tool refusal (`E_FORBIDDEN`/`E_UNAUTHORIZED` with a
code), a `fleet_id` mismatch, a rejected `proto`, or a peer that does not
know `peer_exchange` as terminal — the link goes `refused` or `incompatible`
and stops on its own. A bare HTTP 403 with none of that — the shape a proxy
in front of the peer sends, not the peer hub's own answer — is treated like
a dropped connection instead and retried with backoff.

`fleet-hub client revoke <name>` on the listener's peer token is not the same
as `fleet-hub peer remove`: it acts at once (a call the listener already had
parked returns immediately, and a further send to that fleet is refused)
but leaves the link's pending rows exactly where `peer remove` would have
failed them — attached, waiting for a re-pair inside the retention window.

A reply only threads onto a message the two ends actually exchanged: onto
one your own hub sent across this same link, or one the peer sent that
named you. An id that only looks right — a purely local thread, another
fleet's traffic, or a message this link never carried — is refused
(`E_INVALID`) rather than silently accepted.

## Trackers

A hub can read tickets from **Jira Cloud, GitHub Issues, Asana, Linear and
Jira Data Center**, read-only: sessions then show their ticket's title and
status, ⌘K lists *My work* (and a current sprint / cycle where one exists),
and work can be started from a ticket. Every provider behaves the same in
⌘K, on the chips, in start and resume and in detection; with trackers of two
or more kinds, a small provider badge tells them apart. Nothing needs a
tracker — keys in branch names group sessions without one — and nothing
waits on it: with a tracker down or its token expired, everything answers
from the cache.

Trackers are fleet administration, so they are configured **on the hub**
(`work_admin` is master-only; a paired desktop shows them read-only and says
so). From the hub machine — paste any ticket or issue URL; the provider and
site are inferred from it:

```sh
fleet-hub tracker add https://acme.atlassian.net/browse/ABC-123   # Jira Cloud
fleet-hub tracker set-credential 1 --email you@acme.com < jira-token.txt
fleet-hub tracker test 1          # probe: account, key prefixes, sprints, views
fleet-hub tracker list
fleet-hub tracker status          # each tracker's last sync pass, and retention
fleet-hub work usage --days 30    # how the work graph is used, counts only
fleet-hub tracker remove 1        # its items stay, marked unavailable
fleet-hub tracker section-map 2 --set 'ideas=todo'   # Asana: set sections' categories and confirm the map
```

A token is read from **stdin**, from an environment variable of that command
(`--from-env JIRA_TOKEN`), or not read at all: `--ref env:NAME` or `--ref
file:/run/secrets/jira` stores a *reference* the hub resolves each time it
syncs. It is never an argument, so it never lands in `ps` or shell history.
Without `--email` the token is the whole credential (Asana, Linear, Data
Center). With Docker, put the token in a secret and point the tracker at it:

```yaml
services:
  fleet-hub:
    secrets: [jira]
secrets:
  jira:
    file: ./jira-token.txt     # mounted at /run/secrets/jira
```

```sh
docker compose exec fleet-hub fleet-hub tracker set-credential 1 \
  --email you@acme.com --ref file:/run/secrets/jira
```

### Per provider

| Provider | Add | Credential | What fleet reads |
|---|---|---|---|
| **Jira Cloud** | `https://<name>.atlassian.net` or any ticket URL | email + API token (id.atlassian.com → Security → API tokens; they expire within a year) | `search/jql` views *My work*, *Current sprint* (projects with sprints), *Recent*, favourite filters; status category + resolution; epics by `hierarchyLevel` |
| **GitHub Issues** | `https://github.com/<owner>` or any issue URL, **`--via-cli <host>`** (`--repo owner/repo` to narrow) | **none in fleet**: `gh` on that host, with its own `gh auth login` | GraphQL through `gh api`: `assignee:@me` issues in the owner's repositories; `OPEN` → to do (in progress when GitHub links a branch or a closing PR), `CLOSED` → done / not planned / duplicate by `stateReason`; sub-issues' parent; a transferred issue keeps its links |
| **Asana** | `https://app.asana.com[/<workspace gid>]` or any task URL | personal access token | *My tasks*, one view per project your tasks sit in (the **events API**, a sync token per project; an expired token lists the project whole once), *Recent* where search exists (Premium); `completed` → done, otherwise the task's section through the section map |
| **Linear** | `https://linear.app/<workspace>` or any issue URL | personal API key | issues assigned to you: *My issues*, *Current cycle* (teams with cycles), *Recent*; team keys are the key prefixes; `state.type` → status (canceled → not planned); a team move keeps the link |
| **Jira Data Center** | `--provider jira_dc https://jira.corp.example[/jira]` | personal access token | API v2 (`/search` by `startAt`), the Epic Link field, the same views and filters as Cloud |

- **GitHub reads through `gh`**, run over SSH on the host you name
  (`transport = via_cli:<host>`). Fleet never reads, stores or sends a GitHub
  token: the host's `gh` login is used, and `set-credential` refuses a
  GitHub tracker. A host without `gh`, or with `gh` logged out, makes the
  tracker `unreachable` with the fix in its error.
- **Asana has no human keys.** A task's reference is its URL (both forms,
  `/0/<project>/<task>` and `/1/<workspace>/project/<p>/task/<t>`); the UI
  shows a short `Asana …123456`. A task in two projects is listed under both.
  Which **sections** mean *in progress* is inferred on the first test from
  their names (progress / doing / review → in progress, done / shipped →
  done) and shown in Settings → Trackers with a Confirm button; once confirmed,
  your map wins (`work_admin update` with `settings.section_map`, or
  `fleet-hub tracker section-map <id> --set 'name=category'`). The names the
  rule cannot classify stay *to do*; with the `status_map` decision feature
  on (off by default, [`decisions.md`](decisions.md#status_map--asana-section-proposals-j3))
  the hub can propose a category for them, which you apply the same way.
- **Linear vs Jira keys:** `ENG-123` belongs to the tracker whose probed
  prefixes (Jira projects, Linear team keys) include `ENG`. A prefix two
  trackers claim is never bound automatically.

### GitHub Enterprise Server

An enterprise instance is a GitHub tracker with a `hostname`: paste an issue
URL on it and say it is GitHub (its host cannot be told from the URL), or
give the host with `--hostname`, which implies `--provider github`:

```sh
fleet-hub tracker add https://ghe.corp.example/acme --via-cli devbox \
  --hostname ghe.corp.example:8443
fleet-hub tracker test 4
```

`gh` on that host must be logged in to the instance (`gh auth login
--hostname ghe.corp.example:8443`); fleet runs `gh api --hostname <it>
graphql` there, the hostname `shell::quote`d, and nothing but the instance's
`https://<host>/api/graphql` is ever asked for. The hostname is admin-set
(`work_admin` is master-only) and fenced by name: a DNS name of two or more
labels with an optional port — no scheme, path, userinfo, IP literal,
`localhost`, or github.com lookalike. Fleet itself never connects to it
(`gh` on the host does, through that host's resolver), so there is no
resolve-then-refuse step as for Data Center: the fence is the name. The
site's host is the hostname's; its keys are `host/owner/repo#n`, so the same
repository name on github.com and on the instance is never the same work, and
only a configured instance's URLs and `host/owner/repo#n` references are
recognised.

### Sync metrics

`fleet-hub tracker status` (`work_admin { action: status }`, master-only, and
Settings → Trackers on the machine that syncs) shows each tracker's last pass:
its duration, the items the tracker listed or fetched, the items that
changed, the event frames the pass emitted, and the error it ended with
(redacted, one line). They are kept in memory only and start empty after a
restart.
Since M12.3 the tool answers `{ trackers, retention }`, and the command
also prints each retention table's rows, its dry-run count and the last
sweep (see *Work retention*).

### Reaching a tracker from a host: `via_host`

A tracker only one machine can reach (a VPN, an internal network), or one
whose requests should leave from a particular host, is read with `curl` on
that host: `fleet-hub tracker add <url> --via-host <host>`. The token goes to
the host **on stdin** into a private temp directory (`umask 077`, on
`$XDG_RUNTIME_DIR` when there is one); the header file is unlinked as soon
as the script holds it open, before `curl -q` reads it through
`-H @/dev/fd/3`, so it is a file only for a moment, in no argv on the host
(`ps` shows `/dev/fd/3` and file names), no environment variable and no
log. The directory goes on exit, and one a killed shell left behind is
swept by the next request after ten minutes. Requests are https only,
never follow a redirect, and still go only to that tracker's own host.
`curl` 7.55 or newer is needed on the host.

### Jira Data Center

The site is whatever an admin enters, so it is fenced harder than the others:
https only, one exact host (no subdomain, no port, no userinfo, an optional
context path), and **before connecting the hub resolves the name and refuses
a loopback, link-local (169.254.0.0/16, cloud metadata) or unspecified
address**, then connects to the address it checked. A site that really is on
such an address needs `settings.allow_private_network: true`. An internal CA
goes into `settings.extra_ca` (PEM), trusted besides the system store. A site
only a VPN host can reach uses `--via-host` instead (that host's trust store).

```sh
fleet-hub tracker add --provider jira_dc https://jira.corp.example/jira
fleet-hub tracker set-credential 3 < pat.txt
fleet-hub tracker test 3
```

### What to know

- **Sites are fenced** per provider — `*.atlassian.net`, `api.github.com`
  or the enterprise instance's `/api/graphql` (through `gh`), `app.asana.com`, `api.linear.app`, the one Data Center host
  — and redirects are never followed: a tracker's URL is where the hub sends
  a credential from its own network position.
- **States.** An expired or refused credential sets `auth_failed` and polling
  stops until you set a new one and `test` it; a CAPTCHA (`captcha`) needs
  one browser login to the site. `rate_limited` (429, `Retry-After`, Linear's
  complexity limit, GitHub's spent quota) and `unreachable` retry on their
  own.
- **Sync** runs every `work.sync_interval_secs` (default 300; `0` turns it
  off, read at start): each view from its watermark with a 2-minute overlap
  (Asana projects: from their sync token), every linked ticket by id, and
  references typed before the tracker was connected (which then bind to their
  tickets on their own). A ticket that vanishes is marked *unavailable* —
  deleted or no longer visible, a tracker cannot say which — never deleted,
  and its links stay.
- **Secrets** never leave `tracker_secrets`: no answer, event, log line,
  diagnostics bundle or error report carries a token (a row shows only
  `…abcd`), and `last_error` is redacted before it is stored — Atlassian,
  GitHub (`ghp_`, `gho_`, `github_pat_` …), Linear (`lin_api_`) and Asana
  token shapes included.
- **Isolation:** a per-host token (an in-session Claude) sees only tickets
  linked to sessions on its own host, inside its host's organisation (see
  *Organisations and isolation* below), whatever the provider, and never
  receives `work:*` frames on `/events`. Master and paired clients see all.
- **Migrating from the desktop:** a copied `state.db` carries the desktop's
  trackers and a stored token. Re-enter the token on the hub (or rotate it
  and use a `--ref`) rather than keep one that lived on another machine.

## Organisations and isolation

**From a paired desktop.** Settings → Company (Organisations, Devices,
People) changes this hub's orgs, paired devices and people through the
`org_admin` tool: the owner's own device bound to no org reads them, and a
**trusted** `full` one changes them (`fleet-hub client trust <name>`). It
pairs a phone with a one-time code and QR (`pair_device`, a person's device
only — peer links and updater tokens stay `fleet-hub pair --mode`), and
never revokes, untrusts, binds or hands over the device it is used from.
`fleet-hub org|client|person` stay the operator's side on this machine.

Organisations are optional. With none, the desktop's scope selector offers
the GitHub owners of the live sessions (only when there are two or more) and
nothing is fenced. Name an org to merge or split owners, to attach a
tracker, or to make it a **boundary** for the hosts you put in it.

An org is two things at once:

- **A view** for people. The master and every unbound paired client read
  every org (a client bound to an org reads that org only — *Clients* above);
  the sidebar's selector (⌘⇧O / Ctrl+Shift+O) only narrows what is shown,
  and a session waiting on you in another scope still says so ("2 need you
  in Personal →").
- **A boundary** for per-host tokens. The Claude on a host in org A reads
  only org A's work and unassigned work; the Claude on a host in no org
  reads unassigned work only. That covers work items and tickets, trackers,
  work links (live and past), the work journal and every text built from it
  (`work { context }`, resume and start briefs, the SessionStart context),
  and a session row's `work` / `work_suggested` / `work_rejected` in any
  answer or `/events` frame. An item, link or tracker id outside the
  boundary answers exactly as an id that does not exist; a key or URL
  outside it answers as a key nothing is linked to on that host.

Which org a session is in: the most specific matching rule — a path prefix,
then `owner/repo`, then `owner`, then a host-only rule — else its host's
org. Rules are text (a project row that is re-created keeps its org);
`local` (an adopted folder's placeholder owner) is never an owner. A
ticket's org is its tracker's. A link's is its ticket's, else its session's.

```sh
fleet-hub org add "Company A" --color '#e11d48'
fleet-hub org rule add 1 --owner acme                  # acme/*
fleet-hub org rule add 1 --owner acme-labs --repo api  # one repo of another owner
fleet-hub org rule add 1 --path /home/me/work/acme     # by where the worktree lives
fleet-hub org assign-host hetzner-a 1                  # the boundary for that host's token
fleet-hub org assign-tracker 2 1                       # its tickets are Company A's
fleet-hub org set 1 --isolate-sessions on              # see below
fleet-hub org set 1 --bound-sees-unassigned off        # its bound phones: its own only (D31)
fleet-hub org list
```

Only the master can change any of it (`work_admin`); a paired desktop shows
it read-only, and a host can never move itself into another org.

What else to know:

- **Linking across orgs is refused for everyone**, the master included,
  unless `force_cross_org: true`: it is a data-integrity rule that stops
  Company B's ticket from being attached to a Company A session by mistake
  (the desktop explains it and offers "Link anyway"). A `move_session` to a
  host whose org the session's live links are not in is refused the same
  way, before anything is copied; `force_cross_org: true` carries the links
  as they are and the report's `warnings` names each crossing (the Transfer
  sheet offers "Move anyway"). Detection never
  guesses across orgs, and the sync never binds (nor fetches) a bare key
  for another org's session.
- **Sessions are not fenced by default.** `isolate_sessions` (per org, off
  by default) also hides that org's sessions from every other org's hosts —
  `list_sessions`, `whoami`, `peer_status`, `related_sessions`,
  `session_history`, the repo reads, `send_message`, `broadcast_prompt` and
  `session:*` frames — and its own hosts then see only its sessions and
  unassigned ones. A host always sees its own sessions. It can break a
  controller that dispatches across companies, which is why it is yours to
  turn on.
- **With isolation off, a session's own fields are not work data — but
  multi-user M1 narrowed who reaches the row at all.** This bullet used to
  say a session's name, branch, worktree and last prompt stay readable by
  other orgs' hosts, so a branch named after a ticket showed its key. That
  was the rule while a session belonged to the fleet. Now it belongs to a
  PERSON: a `private` row is readable by its owner and whoever they shared it
  with, and a per-host token is a machine that owns nothing, so it no longer
  reads another host's sessions whatever `isolate_sessions` says — it reaches
  its own host's rows, plus the `unclaimed` ones there. `isolate_sessions`
  therefore no longer has to be turned on to keep a session NAME from
  another company's hosts; it still governs the org dimension, which is a
  different question from ownership and is composed with it. See *Who owns a
  session* above.
- **A host in no org sees only unassigned work.** Assign every host of a
  company before connecting a second company's tracker: a bare key linked
  on an unassigned host's session belongs to no org, so ANY org's tracker
  may bind it (fetching the key with that org's credentials), which is why
  hosts are assigned first.

## Tidy-up and auto-tidy

The hub (or a standalone desktop) plans tidy-up on every GC sweep
(`gc.sweep_interval_secs`) and on request (`work { action: "tidy" }`, the
desktop's "Tidy up · n"). Suggestions only, by default. The settings, all
under Settings → Work → Lifecycle or `set_fleet_setting`:

| Setting | Default | Meaning |
|---|---|---|
| `work.tidy_done_days` | `2` | a linked ticket must have been done this many days (from the tracker transition) |
| `work.tidy_idle_hours` | `4` | a session must have been idle this long before any reason suggests it |
| `work.tidy_idle_unlinked_days` | `7` | a session with no work linked is suggested (`idle_unlinked`) after this many days idle and unprompted (1–90) |
| `work.auto_tidy` | `false` | the sweep acts on the allowed reasons by itself |
| `work.auto_tidy_reasons` | `done_idle,pr_merged_idle` | comma list of `done_idle`, `pr_merged_idle`, `not_planned` |

With `work.auto_tidy` on, the sweep **safe-kills** (or, for a session with
no worktree fleet can inspect, archives) the candidates whose primary reason
is allowed — never a plain kill, so a duplicate worktree and a lost session
stay suggestions. Every action is written to the session's timeline
(`gc_tidied`, or `gc_failed`) and to its work journal (`tidy`). The
protections apply to auto-tidy exactly as to a person's confirm: working,
blocked, stuck or dialog-waiting sessions, sessions linked to in-progress
work, the controller and the operator, anything prompted or attached to in
the last hour, and background agents with open tasks are never touched. The
idle killer (`gc.enabled`, `gc.*_idle_secs`) is separate and unchanged.

**Idle, no work linked** (`idle_unlinked`, work graph M11.3). A work
session with its own worktree and no live or suggested link, idle and
unprompted (no prompt, attach or finished turn) for
`work.tidy_idle_unlinked_days`, is also suggested. It only ever suggests:
auto-tidy never acts on it, whatever `work.auto_tidy`, the org override or
the reason list say (decision D19; the setting cannot name it). Its kill is
refused unless the worktree inspects clean and pushed — with no work linked,
fleet does not guess what uncommitted work is for, so it is not safe-killed
either — and only while the fresh plan still names it. **Keep** (`tidy_apply`
item `{ action: "keep", days }`, 1–90, default 7) holds any live session out
of tidy-up per session; it is a `tidy_kept` timeline event, no column. A
per-host token keeps only its own host's and org's sessions.

**Per organisation.** An org can override `work.auto_tidy` for its own
sessions: `fleet-hub org set 1 --auto-tidy on|off|inherit` (or `work_admin
{ update_org, org_id, auto_tidy }`); `inherit` (the default) follows the
fleet-wide setting. The allowed reasons and thresholds stay fleet-wide. A
per-host token sees and applies only its own host's candidates of its org.

## Language census (Jev evaluation)

Before any decision model is tried in fleet (the Jev evaluation,
`docs/superpowers/specs/2026-09-27-jev-language-census-design.md`), the
census measures which languages the texts such a model would see are
written in, per organisation. It is local and read-only: it opens
`state.db` directly (no running hub needed), never writes it, sends
nothing anywhere, and prints counts only — a count from 1 to 4 shows as
`<5`.

```bash
fleet-hub census languages                      # last 90 days, every org
fleet-hub census languages --days 30 --org 2 --json
fleet-hub census languages --db ~/path/to/desktop/state.db   # a desktop's store
```

What it reads, per org:

| Source | What | Stands for |
|---|---|---|
| `prompt` | each conversation's first prompt (the 200 characters the hook keeps) | the input of a work-link decision |
| `title:<provider>`, `description:<provider>` | work items' titles and cached descriptions | the candidates of that decision |
| `journal:<kind>` | journal notes, summaries and handovers written by a person or Claude, never fleet's own rows | the language of Claude's replies |
| pairs | confirmed links: the prompt that opened the conversation × the item's title | how often the match is across languages |

Prompts fleet typed itself — a ticket start or resume, a handover or
safe-kill request, a quick-reply chip, anything `[claude-fleet`-marked —
and prompts Claude Code submitted itself (a `<task-notification>`, a
slash command's echo) are counted as `fleet-typed` and left out; a
person's words after a `<system-reminder>` head are read without it.
fleet no longer stores such a prompt as a conversation's first, so the
person's next prompt is. Each text is read as one of `en
sk cs de pl hu other mixed unknown`, with Slovak and Czech written without
diacritics flagged, and how much of it is code (`none`, `low`, `high`).
The output lists what it cannot count (later prompts, Claude's replies as
such, commit subjects) instead of guessing.

**Checking the detector on your own texts.** `--export-sample N --out
FILE` writes N distinct first prompts, spread over the window, with the
detector's guess, to a new file created `0600` — the one path that writes
text, so keep the file on the hub machine. Correct each `expect`, set
`checked` to `true`, then `--labels FILE` prints how often the detector was
right and which languages it confused. The same `--labels` runs the
fixtures in `crates/fleet-core/src/service/testdata/nl/`.

The detector's language models add about 45 MB to `fleet-hub` (cargo
feature `nl-detect` of `fleet-core`, which only the hub turns on; the
desktop app is built without it).

## Decisions (Jev) — experimental, off

The hub is the one place a decision model may be called from (a desktop
paired to it, a phone and an agent host never call out). Everything is
**off by default**: the kill switch `decide.jev.enabled`, each feature's
mode (`decide.jev.status_map`, `decide.jev.work_link`: `off | shadow |
assist`), each org's consent, and the global `decide.jev.unassigned` for
rows with no org. With the defaults no request can leave the hub. The
first use case is `status_map` (J3): after a clean sync, at most daily, an
Asana tracker's section names the keyword rule could not classify are put
to the model; in `assist` its answers are **proposals** a person applies
(`fleet-hub tracker section-map`), never written by themselves. The full
guide — what is sent, what is recorded (never raw text), the fallbacks,
retention, how to turn it off — is [`decisions.md`](decisions.md).

```bash
fleet-hub decide set-key < jev-key.txt          # or --from-env NAME / --ref file:/run/secrets/jev
fleet-hub org set 2 --jev on                    # this org consents
fleet-hub decide enable                         # the kill switch on (disable: every call stops)
fleet-hub decide mode status_map shadow         # off | shadow | assist, per feature
fleet-hub decide unassigned on                  # rows with no org may be sent too
fleet-hub decide set decide.jev.daily_token_budget 500000   # any other decide.* setting
fleet-hub decide status                         # read-only: flag, modes, consent, breaker, spend
fleet-hub decide runs --feature work_link --limit 20
fleet-hub decide proposals [--tracker 3] [--json] [--db FILE]   # status_map: section → category (confidence)
fleet-hub tracker section-map 3 --set 'ideas=todo' --set 'parked=done'   # a person applies them
fleet-hub decide proposals apply 812 [--as in_progress]   # or one at a time, by its run
fleet-hub decide proposals reject 814                    # "not this": stays unmapped
```

`enable`, `disable`, `mode`, `unassigned` and `set` change the `decide.*`
settings over the running hub's `set_setting` (loopback, master token),
like `org set`: the hub checks the value and audits the change. `set-key`
and `clear-key` write `state.db` directly (the key is read like
`fleet-hub tracker set-credential`'s secret: stdin, `--from-env` or
`--ref`, never argv); `status`, `runs` and `proposals` open it
read-only and print ids, words and numbers — never the key; the section
names `proposals` shows come from the trackers' stored config, never from
the record. `tracker section-map` is a `work_admin update` over loopback
(master token) that confirms the section map with your entries on top;
`proposals apply <run>` is the same call for one proposal (`not_planned`
applies as done; `--as` corrects it) and marks its run confirmed or
corrected. `proposals reject <run>` writes only the run's follow-up
(`rejected`) in `state.db`, like `set-key`: the section stays unmapped and
the answer is not proposed again until a new one exists (another input,
question version or model). A paired desktop refuses both (tracker
administration); a standalone desktop decides them in Settings → Trackers.

The offline `work_link` benchmark (test map card J1, phase 0) measures a
provider against the links people confirmed, before anything is turned on:

```bash
fleet-hub decide bench work-link                       # test split, none + bm25, read-only, offline
fleet-hub decide bench work-link --split all --json
fleet-hub decide bench work-link --export-unlinked 150 --out h.jsonl   # D39: a person labels it
fleet-hub decide bench work-link --labels h.jsonl      # adds the labels as dataset H
fleet-hub decide bench work-link --split all --provider bm25 --provider jev --max-calls 200
fleet-hub decide bench work-link --split all --provider bm25 --provider jev --shape choice+noul
fleet-hub decide bench work-link --split all --provider bm25 --provider jev \
    --provider haiku --haiku-host gpu1 [--haiku-model haiku] [--haiku-timeout 120]   # D33 baseline
```

Both reports end in the card's acceptance lines (PASS / FAIL / NOT JUDGED
against the thresholds the test map registered) and carry Jev's
calibration (ECE, Brier). The `status_map` benchmark (card J3) reads
labeled Asana sections — the owner's file, or the built-in synthetic set
(LLM-written, not yet spot-checked) — and opens no database unless
`--provider jev`:

```bash
fleet-hub decide bench status-map --fixture                      # todo + rule, offline
fleet-hub decide bench status-map --labels sections.jsonl --provider rule --provider jev
fleet-hub decide bench status-map --labels sections.jsonl --provider jev --provider haiku --haiku-host gpu1
fleet-hub decide bench status-map --labels sections.jsonl --provider jev --provider haiku --haiku-host gpu1 \
    --split dev --question q.json   # try a reworded question on the dev boards
```

Without `--provider jev` or `--provider haiku` neither sends anything
(`work-link` opens the database read-only). `--provider haiku` (decision
D33) asks the same question — the same redacted state and options — of
`claude -p --model haiku` (no tools, no MCP, no hooks, no transcript) on
the host `--haiku-host` names, which is required: each case leaves the hub
over SSH for that host — the prompt on stdin, never in argv — and reaches
Anthropic through its Claude account, a note on stderr says so before the
first call, and nothing is recorded in `decision_runs`. It never crosses
the org boundary: the host's org is read from the database, and a case of
any other org (a case with no org counts as one, unless the host has none
either) is skipped as `other_org` — so `--fixture`, which has no org,
needs a host with no org. One call at a time, `--haiku-timeout` each, `--max-calls`
at most; with it the haiku lines of the acceptance are judged. With it, each case goes through the envelope's gate
(so only orgs that consented — or, for a row with no org,
`decide.jev.unassigned` — with the flag on and `decide.jev.work_link` /
`decide.jev.status_map` at `shadow` or `assist`) and every call is recorded
in `decision_runs` (subject `bench`), which is why that run opens the
database for writing. They print counts, rates, thresholds, latency and
cost — never a prompt, a title or a section name; `--export-unlinked`
writes the one file that holds text (`0600`,
never over an existing file). What it measures and approximates:
[`decisions.md`](decisions.md) → *Benchmarking work_link* and
*Benchmarking status_map*; the order to run it all in is *How to run
phase 0* there.

## `/mcp/json` — the same tools, a body a proxy can compress

`POST /mcp` answers `text/event-stream`: the JSON-RPC reply arrives on a
`data:` line. That framing is load-bearing for the long polls
(`wait_for_session`, `run_prompt`), whose 15-second keep-alives are what hold
a reverse tunnel open — but it also means the answer is never compressed,
because Caddy's `encode` matcher and Cloudflare both skip
`text/event-stream`, correctly: compressing a stream would buffer it.

`POST /mcp/json` is the same tool surface, behind the same bearer token, with
the framing taken off. It answers `application/json`, so a reverse proxy
compresses it like any other body:

```bash
curl -s --compressed https://fleet.example.com/mcp/json \
  -H "Authorization: Bearer <token>" \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"fleet_health","arguments":{}}}'
```

(The `Accept` header still offers both types — rmcp requires the pair on
either mount.)

Both mounts read a request body of at most ~23.4 MiB and answer `413` past
it (a declared `Content-Length` over the cap is refused before a byte is
read). The one tool call that comes near it is `work_link { attach }`, a task
attachment as base64 at its 16 MiB ceiling (`store::ATTACHMENT_WIRE_BYTES`;
`work.attachment_max_mb` defaults to 10); the desktop reads a hub answer up
to the same size, for `work { attachment }`. The cap is what stops one
oversized POST from any valid token buffering until the hub runs out of
memory.

What it is worth, measured on a 44-session fleet: `list_sessions`
`{summary:false}` is 51 968 B unframed and 7 767 B gzipped, `list_projects`
7 660 → 1 308 B, `list_hosts` 1 707 → 475 B. A phone's cold start is those
three calls: **61 335 B today, about 9 550 B over `/mcp/json` behind a proxy
that compresses** — and one fewer full JSON re-parse on the device, since the
body is no longer a JSON document inside a JSON string.

Use `/mcp` for anything long-polling; use `/mcp/json` for ordinary calls from
a client on a metered or slow link. Existing clients need no change:
`/mcp` is untouched, and the wire-contract revision does not move for a new
path. `deploy/hub/Caddyfile` ships with `encode zstd gzip`; a bare-binary
deployment with no reverse proxy in front gets the unframed body but no
compression.

### Asking for fewer columns

Compression makes the same answer cheaper; these two parameters make the
answer smaller, and they stack with it.

`list_sessions { view: "phone" }` returns each row projected to the
columns the phone app reads — the session list's `id`, `tmux_name`,
`friendly_name`, `last_prompt`, `host_alias`, `project_id`, `status`,
`kind`, `claude_status`, `stuck_kind`, `current_activity`, `context_pct`,
`last_activity_at`, `ci_status`, `pending_input`, `needs_attention`, and the
session card's `is_controller`, `tags`, `turn_seq`, `safe_kill_state`,
`started_at`, `last_turn_at`, `last_stop_at`, `usage_cost_micros`,
`usage_model`, plus the work graph's `work` (the row's primary work link)
and the PR fact, CI check count and Switch account's `pr_url`,
`pr_evidence` and `claude_profile`. The first cut — the list's sixteen alone — measured
**46 990 → 16 733 B of JSON (−64 %), 7 712 → 3 023 B gzipped** on the same
44-session fleet; the card's nine are short scalars and do not change that
picture. The first cut left the card's columns out, and a phone that
switched to it would have read `is_controller` as `false` and offered to
kill its own controller — the list is not the only screen a row feeds. The view is
*named*, not a caller-supplied field list, so the hub keeps the definition of
what a pager row is and can widen it without an app release; an unknown name
is refused with `E_INVALID` rather than quietly answering full rows. It
exists because neither shape fitted: the default summary row is 9 951 B but
drops `friendly_name`, `current_activity` and `last_activity_at`, which are
three of the columns a pager uses. `pending_input` is in the view for the
same reason: it carries the dialog a blocked session is waiting on and its
options, and without it the view is a list a phone can read but not act on —
answering that dialog is the one thing a pager exists for. `needs_attention`
is there for the mirror of that reason: projected away, the view would hand a
phone the columns to re-derive the answer instead of the answer. Its reasons,
most urgent first (`service/attention.rs`): `waiting`, `stuck`, `host_down`
(the session's host was pinged and did not answer), `account_limit` (its
account's 5-hour or weekly window is used up and the session is not
working), `no_credentials` (its account's login has expired or its
token was rejected, and the session is not working), `stop_failed`,
`failed`, `context_full` (at or past `health.context_red_pct`),
`stale_working` (a `working` row demoted after `reconcile.stale_working_secs`
with no activity; it lifts on the next hook, when its terminal is opened,
when the row works again, or after `reconcile.stale_working_ttl_secs`),
`ci_failing`, `probably_waiting` (contract 15: Jev read a silent turn's end
as a question; a proposal, not a fact) and `lifecycle`. The three after `stuck` are decided from what
the hub's event bus follows of hosts and account usage, not from the row, so
every site that stamps `needs_attention` (`list_sessions`, `/events`, Today,
the org counts, the work view) agrees. Beside the reason, `state` (contract
11) is the attention state it puts the session in: `action_required`,
`failed`, `blocked` (the three the Needs you badge counts), `proposed`
(`probably_waiting`, contract 15: shown apart from Needs you, never counted)
or `paused` (`lifecycle`). A mission waiting on a person is the model's
other class (contract 15): its row's `waiting_on` (`question`,
`sign_grant` or `confirm`, with `since` and `open_cards`), and Today's
`missions`; the badge counts it. `tags`
is there because the phone's tag editor starts from them and
`set_session_tags` replaces the whole list: without them a phone that added
one tag deleted the rest. `work` (the primary work link: key, title) is the
row's key chip and what the list groups by work on.

`list_projects { has_sessions: true }` keeps only the projects that hold a
live session — the rows a client needs to turn a session's `project_id` into
a heading. That fleet's hub listed **78 projects (6 581 B) to name the 8
(835 B)** its sessions actually carried: −87 %, and the one call whose cost
grows with the operator's history rather than with the fleet.

Both are opt-in and additive: omit them and the bytes are what they were, so
the wire-contract revision does not move.

## `/metrics` — what each caller costs this hub

```bash
curl -s https://fleet.example.com/metrics -H "Authorization: Bearer <master token>"
```

```
fleet_tool_calls_total{caller="client:phone"} 412
fleet_tool_errors_total{caller="client:phone"} 3
fleet_event_streams_open{caller="client:phone"} 1
fleet_reconcile_duration_ms 812
fleet_reconcile_failures_total 0
fleet_sessions{status="working"} 12
fleet_hosts_reachable 5
```

Besides the per-caller counters the exposition carries four process gauges:
`fleet_reconcile_duration_ms` (the last pass's wall time),
`fleet_reconcile_failures_total`, `fleet_sessions{status="…"}` (by
`claude_status`, external rows excluded, the same roll-up
`fleet_health.by_status` uses) and `fleet_hosts_reachable`. They are two SQL
counts on the hub's read pool, so a scrape never waits on the writer.

Prometheus text format, **master token only** — a per-host token and a paired
phone are both callers this reports on, and letting one read the others'
figures would make a read-only device a traffic monitor for the operator's own
work. A non-master token gets 403 with that sentence, not a 404: the route
exists and the token is the problem.

The dimension is the caller label, the same key the rate limiter and the
stream cap use, so a number here lines up with a refusal in the log.
Deliberately **no session id, no prompt, no project path and no tool name**: a
metrics endpoint is scraped on a timer and kept for months, and a series
labelled with a session id is an activity log of the operator's work with a
retention policy nobody chose.

## Events

A client that has listed what it needs does not have to poll for changes:

```
GET /events            Authorization: Bearer <token>
GET /events?kinds=session,host
```

is a server-sent-event stream of every row change the hub makes — the same
events the desktop UI repaints from. Each frame is named after the change
(`session:created`, `session:updated`, `session:killed`, `host:probed`,
`task:updated`, …) and carries the same JSON payload the desktop receives;
the stream opens with a `ready` frame naming the kinds it will carry, and
sends a comment line every 15 s so a phone's NAT, a tunnel or a proxy in
between keeps the connection open. `?kinds=` filters on the part of the name
before the `:`. An unrecognised kind (`sessions` for `session`, say) is
dropped from the filter and logged as a warning by the hub, and it is missing
from the `ready` frame's `kinds` — which is how you spot the typo instead of
watching a stream that never says anything.

A session row's `pending_input` (carried on `session:updated`, migration 040)
is the permission/question dialog a blocked pane is showing —
`{kind, question, options[{n,label,selected}]}`, or null when the pane shows
none — so a client can turn the numbered choices into buttons instead of
typing them. A multi-select question adds `multi: true` and `checked: true` on
each ticked option (both absent otherwise): there a digit toggles a box, and
`send_prompt { keys: "Tab" }` keeps the ticks and moves on to the next
question or to the "Review your answers" step.

`session:created` and `session:updated` frames also carry `needs_attention`
(`{reason, since}`, absent when the session needs nobody) — the same answer
`list_sessions` stamps on each row, from the one rule in
`service::attention`. A client that listed once and then follows the stream
keeps the hub's answer instead of losing it at the row's first change and
falling back to a rule of its own.

The `ready` frame also carries `contract`, the wire-contract revision of the
row shapes and tool results this hub sends (`fleet_core::wire_contract`,
starting at `1`). It moves when a client's assumptions about the wire would
break — a field removed or renamed, a new enum variant a client may decode as
closed, or a new tool the desktop routes to; a purely additive field does not
move it — and a
hub built before this field existed sends nothing, which a client reads as
revision `0`. See *Version skew* below for what a client does with it.

Each row frame carries an `id:` of the form `<generation>-<seq>`, where `seq`
counts the frames **this connection** was served. A client that reconnects
sends the last one back as `Last-Event-ID` (or as `?since=<id>`, for the
proxies that strip the header), and the `ready` frame answers `"resumed"`:
`true` if the hub honoured the id, `false` if it did not. `false` means
re-list; it is never a reason to assume continuity.

**Today the answer is always `false`.** A reconnecting client re-lists
instead of being handed the events it missed. The replay history exists and
the hub's own internal readers use it, but it is not served to a `GET
/events` caller: a frame out of the history describes the fleet as it WAS,
and judging it under the reconnecting caller's present permissions needs an
identity for its subject that the row's id alone does not give — `sessions`
ids are reused, so an id kept past a row's death resolves to whoever holds
it next. Refusing the replay is the one answer that needs no such identity.
Giving sessions a hub-minted birth marker would let the resume come back;
until that is decided, every reconnect costs a re-list, and the shape on the
wire (`resumed: false`) is the one clients have always handled.

The desktop does this: it sends the last row frame id it applied, re-lists
when `ready` says `resumed: false`, and then also re-fetches projects,
worktrees and work in the window.

`?fields=id,claude_status,…` keeps only those keys in each frame's payload.
There is no fixed vocabulary — a field is whatever the row type serialises,
and that differs per event — so every name is accepted and the `ready` frame
echoes back the list it honoured, which is where a typo shows up. A session
row is about 1.2 KB on the wire and a phone draws perhaps a third of it.

The stream sits behind the same bearer token as `/mcp` (a change stream names
sessions, hosts, projects and prompts), and a caller may hold eight of them at
once. A subscriber that falls far enough behind gets one `lagged` frame and
the stream closes — reconnect and re-list rather than assume continuity.

### What a stream carries, and whose

A stream shows you exactly what the tools show you: every `session:*` frame
is judged, frame by frame, against the same answer `list_sessions` gives that
caller. So a second person's device following `/events` sees the sessions they
own, the ones shared with them, and nothing else — the stream is not a way
round the gate, and there is no caller for whom the judgement is skipped
except the hub's own internal readers.

The same goes for the frames that name a session without being one:
`task:updated` (which carries a task's `prompt`, `result` and `error`) is
judged by the sessions at its two ends, `move:progress` by the session being
moved, and `session:killed` by the facts it carries — that frame fires after
the row is deleted, so it brings its own `host_alias`, `visibility` and
`owner_person_id` along, as additional keys next to the `id` every client has
always read. Every other kind (`host:*`, `project:*`, `worktree:*`,
`account*`, `asset_inventory:*`, `catalog:*`, `sync:*`, and `start:progress`,
which carries only the opaque `start_token` the starting client minted and a
step name) carries no session content and is not narrowed per person; `work:*`, `settings:*`, `update:*` and
`grant:changed` never reach a per-host token or a client bound to an org at
all.

**No frame out of the replay history is served to you.** A `Last-Event-ID`
is answered `resumed: false` and your client re-lists — see *Events* above
for why. A frame in the history describes the fleet as it was, and there is
no sound way to judge it under your present permissions, so none is sent.

**A session that leaves your view is announced.** Your own permissions
changing ends the stream (see *Sharing, revoking, and the fifteen-second
bound* below), but a session can leave your view without anything about YOU
changing: its host moves to another org, it is claimed by somebody, or its
visibility changes. When that happens on a session you have already been
served a frame about, the stream sends one `session:killed` with that `id`
and stops mentioning it — so your client removes the row instead of
displaying it for ever. It is the same frame a real kill sends, on purpose:
"the session is still running, you have just lost access" is not something
the hub tells you. A session you were never served a frame about is never
announced either, so the frame is not a way to learn that a row exists.

**The frame `id:` counts what you were served, and nothing else.** The `seq`
half is this connection's own counter: a frame the fence dropped takes no
number, so the ids you receive run 1, 2, 3 whatever else the fleet is doing.
It used to be the hub's global counter, which let a client that may see one
session out of fifty measure the fleet's aggregate frame rate from the gaps
between its ids. That was the price of one shared replay history keyed on
that counter; with the history served to nobody, the price is not paid any
more. For the same reason, the `lagged` frame tells you that you fell behind
and no longer tells you by how many frames — the count is the same
fleet-wide number.

### Sharing, revoking, and the fifteen-second bound

`grant:changed` — `{session_id, person_id, level}`, with `level: null` for a
revoke — is how a client keeps its own set of shares current. Sharing a
session emits two frames: the `session:updated` that carries the row (which
is how a new recipient's client learns the row exists at all) and this one.
Both reach the person the grant names and the session's owner, and nobody
else.

**A change to what you may see ENDS your open streams rather than widening or
narrowing them in place.** When a session is shared with you, or a share of
yours is revoked, or your device is re-bound to another person or another
org, every stream that device has open is closed; the client reconnects, is
told `"resumed": false`, and re-lists. That is deliberate: re-listing repairs
the client's whole picture, where a frame would only have corrected one row of
it.

**The bound is the keep-alive beat: at most 15 s.** Each beat re-reads the
caller's scope and ends the stream if it moved. The hub also keeps two
in-process counters (one for org moves, one for grant changes) and re-reads
immediately when either has moved, so in practice the stream drops on the very
next frame — but those counters are process-local, so a change made by another
process is noticed on the beat and not before. Treat 15 s as the guarantee and
the rest as an optimisation.

Revoking a DEVICE and revoking a SHARE are separate mechanisms, deliberately:
`fleet-hub client revoke` stops the token (and the stream notices on the same
beat, through a different check), while revoking a share leaves the device
paired and only narrows what it may see. Neither is expressed in terms of the
other.

### A revoked share, precisely

"B loses access" has three bounds, and a long poll is the awkward one: it is
the only request that outlives its own authorisation — the gate ran once, at
the top, and the call then sat for up to ten minutes.

1. **The next request is refused.** Nothing is cached between calls.
2. **An open `/events` stream drops within 15 s**, the keep-alive beat above.
3. **A long poll already in flight is re-checked** on every wake — twice a
   second — and once more immediately before it answers. So the wait a
   grantee started before the revoke ends with `E_NOTFOUND` (the row is no
   longer theirs to see) or `E_FORBIDDEN` (still visible, no longer theirs
   to drive), and not with the payload. This covers `wait_for_session`,
   `wait_for_reply`, `wait_for_task` and `run_prompt`'s wait.
   `add_project`, the fifth long poll, waits on a clone and names no
   session, so no share governs it.

**What is NOT recalled, and will not be:**

- **A prompt `run_prompt` has already delivered.** Its order is deliver,
  wait, read the transcript. A revoke that lands during the wait withholds
  the transcript — the reply is content the caller may no longer have — but
  the keystrokes are already in the owner's pane, and there is no un-typing
  them. Treat `run_prompt` from a shared session as something that has
  happened the moment it returns anything at all, including a refusal.
- **An attached terminal.** The PTY is the desktop's own, outside the
  hub's request path entirely; detaching is the operator's act.
- **A payload already on the wire.** A refusal cannot overtake bytes that
  have left.

One more thing worth knowing if you are watching rate limits: a revoked
device keeps its long-poll SLOTS (`MAX_LONG_POLLS_PER_CALLER`) until its
parked waits age out, up to 660 s, because the permit bucket is keyed on the
device name. Every wait behind those slots now refuses, so nothing is served
through them — the device is only rate-limiting itself.

## Error reports

The hub is also the one place its participants' errors are collected. The
desktop paired to this hub, every `fleet-agent`, and the hub itself send
their **error-level** log events here; the hub keeps a bounded, redacted
table of them, and:

```bash
fleet-hub reports                      # the newest 100
fleet-hub reports --since 2h --origin host:build-box
fleet-hub reports --limit 500 --json   # with each report's context
```

```
RECEIVED           ORIGIN            LEVEL  COMPONENT                 CODE     MESSAGE
2026-09-21 10:41Z  host:build-box    error  fleet_agent::conn         -        dial https://fleet.example.com: connection refused
2026-09-21 10:40Z  client:mac-desk   error  frontend:unhandled        -        TypeError: Cannot read properties of undefined  [trunc]
2026-09-21 10:38Z  hub               error  fleet_core::ssh           E_SSH    ssh mefistos: Host key verification failed
```

Every accepted report is also one `warn` line in the hub's own log, under
the target `fleet_core::report`, so `journalctl -u fleet-hub | grep report`
works too. `--origin` takes any client name, percent-encoded on the way out,
so an origin with spaces or punctuation (a client name, a host alias) filters
correctly.

**Who sends what.** Only `error`-level `tracing` events, never warnings
(the reconcile tick warns per unreachable host per pass). Each sender keeps
a queue of 256 and drops the oldest, counted, when it is full; nothing ever
waits on the hub. The desktop sends every 5 s (or at 20 queued) to
`POST /report`, splitting a drain so a single POST body never crosses 64 KB;
it stops for the run when the hub answers `404` (it predates this route) or
refuses the client (`401`/`403`), and **discards** — one `warn`, never the
body — a batch the hub refuses with any other `4xx` (`400` from validation,
`413` from the hub's own body-size limit) rather than retrying it forever,
which would wedge every report behind it. `CLAUDE_FLEET_HUB_REPORTS=0` in the
desktop's environment turns it off. An agent sends up to 16 per heartbeat in
a `report` frame, including errors from *before* it managed to connect —
which is the case the channel exists for; `"report_errors": false` in its
config turns it off. The hub's own errors join the table on the reconcile
tick under origin `hub`, and a caller holding the *master* token that posts
to `POST /report` is stored under origin `master`. (A standalone desktop —
one that owns its own fleet rather than pairing to a hub — reports nowhere,
but it runs the same tick, so its own error-level events land in its own
local `error_reports` table under origin `hub`, under the same two bounds
below; nothing reads them yet beyond the database.)

**Bounds.** `reports.max_rows` (default 5000) newest rows are kept, pruned
on every insert; rows older than `reports.max_age_secs` (default 604800,
seven days; `0` never) are swept on the reconcile tick. An origin may store
60 reports a minute — an empty batch, one that reports only drops, counts as
one of them — and a batch that would cross that is refused whole with `429`. A message is at most 2048 characters, a context 4 KB, a body 64 KB.

**Privacy.** Every string is run through the same redaction the log gets
(bearer tokens, `?token=` values, 64-hex strings) before it is stored. No
prompt, transcript or pane text has a path here: `tracing` never logs
bodies, and a frontend report carries the toast's message and, for a crash,
a stack. `GET /reports` is master-token only, since the rows hold every
client's messages.

**For a phone or any client:** `POST /report` with the client's own bearer
token (`readonly` included), body

```json
{ "reports": [ { "at": 1790000000, "level": "error", "component": "screen:sessions",
                 "code": "E_PARSE", "message": "…", "context": { "…": "…" } } ],
  "dropped": 0 }
```

at most 50 reports per call; `204` stored, `400` malformed or a level other
than `error`/`warn`, `413` over 64 KB, `429` over budget (nothing stored).
The origin is taken from the token, never from the body: `client:<name>` for
a paired client, `host:<alias>` for an agent, `master` for the master token.

## Bare binary

Prefer running without Docker, or need it as a system service:

```bash
v=0.3.0   # the release you're installing; target: x86_64- or aarch64-unknown-linux-gnu
curl -LO https://github.com/martin-janci/claude-fleet/releases/download/v$v/fleet-hub-$v-x86_64-unknown-linux-gnu.tar.gz
curl -LO https://github.com/martin-janci/claude-fleet/releases/download/v$v/SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing
tar xzf fleet-hub-$v-x86_64-unknown-linux-gnu.tar.gz
sudo install -m 0755 fleet-hub-$v-x86_64-unknown-linux-gnu/fleet-hub /usr/local/bin/fleet-hub
```

Like `fleet-agent`'s binaries (see *Get the binary* above), these are built
on `ubuntu-22.04` runners and need **glibc 2.35 or newer** on the host. No
matching release, or an older host? Build from a checkout instead:

```bash
cargo build -p fleet-hub --release
sudo cp target/release/fleet-hub /usr/local/bin/fleet-hub
```

Create the `fleet` user and the data directory the unit uses, then
initialise the hub as that user, against that directory:

```bash
sudo useradd --system --create-home --shell /usr/sbin/nologin fleet   # if it doesn't exist yet
sudo install -d -o fleet -g fleet -m 700 /var/lib/fleet-hub
sudo -u fleet env FLEET_HUB_DATA_DIR=/var/lib/fleet-hub fleet-hub init --public-url https://fleet.example.com
```

Create `/etc/fleet-hub.env` (same keys as `deploy/hub/fleet-hub.env.example`,
plus whatever else you want set — see *Configuration* below), then install
the unit:

```bash
sudo cp deploy/hub/fleet-hub.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now fleet-hub
```

To have it update itself (S9: a pin, a required update or, under
`update.hub.mode = automatic`, every release), pair an updater and enable
`deploy/hub/fleet-hub-update.timer`; see `docs/updates.md` → *A hub without
Docker updates itself*.

To print the master token again later, use the same user and data dir:

```bash
sudo -u fleet env FLEET_HUB_DATA_DIR=/var/lib/fleet-hub fleet-hub token show
```

`init` and `serve` open `<data-dir>/state.db`, creating it when missing.
`token show`, `token regenerate`, `agent-token` and `host-token-mode` never
create one: pointed at the wrong data dir (or run as a user who cannot see
it) they exit 1 with
`no hub database at <data-dir>/state.db; run fleet-hub init first (or pass
--data-dir)` instead of minting a token nothing uses.

Put it behind your own TLS-terminating proxy (the same role Caddy plays in
the Docker setup) and set `FLEET_HUB_PUBLIC_URL` — or let the hub terminate
TLS itself with `--tls cert`, which needs no proxy at all (see *Single binary
with its own certificate* below). Or skip the public URL
entirely and bind loopback, reaching it over Tailscale or an SSH tunnel of
your own: with no public URL configured, the hub behaves exactly like the
desktop app — it binds `127.0.0.1` and opens a reverse SSH tunnel to every
provisioned remote SSH host. If you instead bind a non-loopback address with no
public URL (for example the machine's Tailscale address,
`--bind 100.64.0.1`), pass `--allow-plaintext` (or set
`FLEET_HUB_ALLOW_PLAINTEXT=1`): the hub refuses any non-loopback bind that
is not fronted by an `https://` public URL unless plaintext is explicitly
allowed.

## Single binary with its own certificate

Caddy (or any other TLS-terminating proxy) is still the documented default —
it renews certificates for you and the compose file wires it up. But the hub
can also terminate TLS itself, which is what you want when a second container
or a second daemon is one thing too many: one binary, one port, reachable
from a phone.

`--tls cert` serves an HTTPS listener from a certificate and key you supply:

```bash
fleet-hub serve \
  --bind 0.0.0.0 --port 443 \
  --public-url https://fleet.example.com \
  --tls cert \
  --tls-cert /etc/fleet-hub/tls/fullchain.pem \
  --tls-key  /etc/fleet-hub/tls/privkey.pem
```

- `--tls-cert` is a PEM **chain**, leaf certificate first, issuers after it
  (certbot's `fullchain.pem`, or the `.crt` bundle your CA hands you).
- `--tls-key` is the matching PEM private key (PKCS#8, PKCS#1 or SEC1),
  readable by the user the hub runs as and by nobody else.
- The two are loaded and checked **before** the listener is bound. A missing
  file, a file with no `CERTIFICATE` block, or a key that does not match the
  certificate exits 1 naming the file. The hub never falls back to plaintext
  on a port a client expects to be encrypted.
- With TLS on, a non-loopback bind no longer needs `--allow-plaintext`:
  terminating TLS *is* the protection that refusal asks for.
- Nothing renews the certificate for you. Point the flags at the files your
  renewal tool writes (certbot, your CA's client, a mounted secret) and
  restart the hub after each renewal — the PEM pair is read once at startup.
- The hub warns (it does not refuse) when the key file is readable by group
  or others; `chmod 600` it.
- `--tls cert` requires `--public-url` to be an `https://` address. Hooks post
  to the public URL and a paired client is sent back to it, so a hub that
  terminates TLS and advertises `http://` — or advertises nothing, which
  falls back to `http://127.0.0.1:<port>` — would point every one of them at
  a port that will not answer plaintext.

Settings are persisted before the certificate is loaded, the same ordering the
bind failure already has. So a run refused for a bad `--tls-cert`/`--tls-key`
path has already stored `hub.tls=cert` and those paths: the next bare
`fleet-hub serve` fails the same way until you correct the paths (or pass
`--tls off`, which stores `off` again).

Binding port 443 as an unprivileged user needs a capability: uncomment the
`AmbientCapabilities=CAP_NET_BIND_SERVICE` lines in
`deploy/hub/fleet-hub.service` (they are off by default — with a proxy in
front the hub binds a high port and needs nothing). Otherwise bind a high
port and forward to it.

### `--tls auto` (ACME) is not built

`--tls auto` — a certificate the hub obtains and renews itself over ACME — is
a recognised value, but it is **not available in this build** and exits 1
saying so. The implementation would be `rustls-acme`, which reaches the ACME
directory through `async-web-client` and so depends unconditionally on
`webpki-roots`, published under `CDLA-Permissive-2.0`. That licence is not in
this repository's `deny.toml` allowlist, so the crate is not in the tree at
all. Until that allowlist decision is made, use `--tls cert` with a renewal
tool, or keep a proxy in front.

## Configuration

`fleet-hub --help`, `fleet-hub serve --help` and `fleet-hub init --help` all
list these flags (every one is `global`, so it works both before and after a
subcommand — `fleet-hub token show --data-dir D` and
`fleet-hub token --data-dir D show` are equivalent). Precedence is
**flag > env > `hub.*` setting in state.db > default**.

| Flag | Env | Setting | Default |
|---|---|---|---|
| `--data-dir` | `FLEET_HUB_DATA_DIR` | — | the hub's own platform data dir: `~/.local/share/fleet-hub` on Linux, `~/Library/Application Support/sk.rlt.fleet-hub` on macOS, `/var/lib/fleet-hub` in the Docker image |
| `--bind` | `FLEET_HUB_BIND` | `hub.bind` | `127.0.0.1` |
| `--port` | `FLEET_HUB_PORT` | `mcp.port` | `4180` |
| `--public-url` | `FLEET_HUB_PUBLIC_URL` | `hub.public_url` | unset (loopback + reverse tunnels) |
| `--allowed-host` (repeatable) | `FLEET_HUB_ALLOWED_HOSTS` (comma-separated) | `hub.allowed_hosts` | none — the public URL's own host is always accepted in addition to this list |
| `--local-host true\|false` | `FLEET_HUB_LOCAL_HOST` | `hub.local_host` | `false` |
| `--operator-host <alias>` | `FLEET_HUB_OPERATOR_HOST` | `operator.host` | `local` |
| `--allow-plaintext` | `FLEET_HUB_ALLOW_PLAINTEXT` (`1`/`true` or `0`/`false`) | `hub.allow_plaintext` | off |
| `--log-dir` | `FLEET_HUB_LOG_DIR` | — | `<data-dir>/logs` |
| `--tls off\|auto\|cert` | `FLEET_HUB_TLS` | `hub.tls` | `off` (`auto` is refused — see above) |
| `--tls-cert` | `FLEET_HUB_TLS_CERT` | `hub.tls_cert` | unset (required by `--tls cert`) |
| `--tls-key` | `FLEET_HUB_TLS_KEY` | `hub.tls_key` | unset (required by `--tls cert`) |
| — | — | `reports.max_rows` | `5000` |
| — | — | `reports.max_age_secs` | `604800` |
| — | — | `work.retention.journal_days` | `365` |
| — | — | `work.retention.tracker_items_days` | `180` |
| — | — | `work.retention.timeline_work_events_days` | `180` |
| — | — | `work.recent_days` | `14` |

The `reports.*` and `work.*` settings have no flag: set them over the API
with `set_setting` (master token; `get_settings` reads them all). It
reaches only the settings registry, never the `hub.*` and `mcp.*` values
in this table.

**Proposed settings and their history** (declarative pages P5). An agent
that should not change a setting on its own proposes it instead:
`set_setting { key, value, propose: true, why }` stores a proposal and
writes nothing. On the hub machine, the operator reviews them:

```bash
fleet-hub settings proposals            # key: now → proposed (who), and why
fleet-hub settings apply 4 7            # apply by id
fleet-hub settings reject 5             # reject by id
fleet-hub settings history work.recent_days [--limit N] [--json]
```

**Guides a session proposes** (declarative pages, layout `guide`). A
Claude session on a host — with the `fleet-guides` skill from
`catalog-seed/` in your asset catalog — writes a step-by-step guide for
Settings → Guides and proposes it with its own token (`guide { propose }`);
nothing is shown until a person approves it. On the hub machine:

```bash
fleet-hub guides list [--json]          # live guides, and proposals: who, why
fleet-hub guides show 3                 # a proposal's steps, and the settings it lets a person change
fleet-hub guides approve 3              # on the pages (a trusted device may approve too)
fleet-hub guides reject 4
fleet-hub guides remove guide.cleanup   # a live guide off the pages
```

See `docs/pages.md` → *Guides*.

**On a paired device** (declarative pages P6). The desktop paired with this
hub shows these settings in its own Settings pages, read and written
through the hub: `get_settings` and `set_setting` answer the master and the
hub OWNER's own paired device — a client bound to no org whose person is
this hub's owner (multi-user M1); never a per-host token, an org-bound
client, or a colleague's device paired to the same hub. A device of either
mode reads them; a `full` device proposes; a device you **trust**
(`fleet-hub client trust <name>`) changes them and decides proposals,
recorded in the history as `person (client <name>)`. The desktop's review
uses `setting_proposals`, `setting_history` and `decide_setting_proposals`,
served to the owner's own paired device and not to the master; a phone can
read the page specs with `list_pages`. A page's data items (usage,
retention) and page actions stay on a standalone desktop: a paired desktop
shows the settings only.

These read and write `state.db` directly, as the person at the console:
an applied proposal is recorded with actor `person` and its id. Every write
of a registered setting is kept in that history, whoever made it (5,000
rows at most). A change that needs confirming (`gc.enabled`,
`work.auto_tidy`, `decide.jev.*`) is never proposed: set it with
`set_setting` yourself.

**Work retention** (work graph M12.3). The GC tick deletes a row only when
it is ended or done, older than its window, and nothing live points at it.
`0` keeps a table forever.

- `journal_days`: work memory (the journal behind resume and the handover
  brief). Kept regardless of age: an open conversation's rows, a live-linked
  session's, and those of work that is not done or still has a live link.
  Also kept: an undelivered handover, and one addressed to a live session.
  Replaces `work.journal_days`. While this key is unset, an old `0` still
  keeps forever and an old window longer than 365 still stands.
- `tracker_items_days`: cached tickets in `done`. Kept while any link, live
  or ended, names one, and while it is the parent of a kept ticket.
- `timeline_work_events_days`: handover, nudge, tidy and withdrawn-suggestion events. The newest
  of each kind per session stays.

At most 2,000 rows per table per tick, 200 per store lock.
`work_admin { action: status }` (master) shows row counts, a dry-run count
and the last sweep; `work_admin { action: sweep_now }` runs one sweep.

`--allow-plaintext` permits a non-loopback bind that is not fronted by an
`https://` public URL — one with an `http://` public URL or with none at all
(a private network such as Tailscale, or a container-internal hop). Without
it, a routable bind (anything but loopback) is refused at startup unless the
public URL is `https://`: `refusing to serve plaintext http on <bind>: use an
https:// public URL, terminate TLS in the hub itself with --tls cert, bind to
127.0.0.1 behind a TLS proxy, or pass --allow-plaintext`. The compose setup
does not need it: the hub binds `0.0.0.0` on the compose network with the
`https://` public URL Caddy serves. Nor does `--tls cert` (see *Single binary
with its own certificate* above) — the hub is then the thing terminating TLS.

Like the other values, the allowance is saved (`hub.allow_plaintext`), so a
later bare `fleet-hub serve` keeps it. The flag can only turn it on; to turn
a saved allowance off, set `FLEET_HUB_ALLOW_PLAINTEXT=0` on a run that
succeeds and so saves it — for example
`FLEET_HUB_ALLOW_PLAINTEXT=0 fleet-hub init --bind 127.0.0.1` (a run that is
refused saves nothing). Any other value of `FLEET_HUB_ALLOW_PLAINTEXT` is an
error.

`fleet-hub serve` logs to stderr and, once the data dir is writable, also to
`<log-dir>` (or wherever `--log-dir`/`FLEET_HUB_LOG_DIR` points).

`--local-host` (default `false`, unlike the desktop where it is implicitly
`true`) controls whether the hub's own machine is itself a managed fleet
host: with it off, reconcile never creates or probes a `local` host, and a
single-host refresh of `local` returns `E_NOTFOUND`. With it off, any tool
or command naming host `local` returns `E_NOTFOUND` too, so nothing runs on
the hub's machine as a fleet host.

`--operator-host` names the fleet host the UX agent's operator session
(`ensure_operator` / `operator_status`, the desktop's agent panel) is homed
on. It defaults to `local`, which on a hub with `--local-host false` is a
host the fleet does not have: `operator_status` then answers `no_host`, the
panel says there is nowhere to start the agent, and `serve` logs a warning
at startup. Point it at any host in the fleet (`--operator-host mefistos`)
and the next press of the agent button creates the session there — its
`~/.claude-fleet/operator/.mcp.json` is handed the hub's public URL, the
same address every provisioned host uses. Only the alias's syntax is checked
at startup; whether the fleet has that host is answered live, since hosts
come and go while the hub runs. The setting is saved like the others, so a
later bare `serve` keeps it. Changing it does not move a running operator:
kill the old `fleet-operator` session first, then press the button again.

The configured host is the operator's *home*, not its only place. When the
home is missing or its last probe failed, `ensure_operator` starts the agent
on the first host that can take it (`pick_operator_home`): `local`, then any
other reachable, visible host in no org with `claude` seen on it, the
provisioned ones first, by alias. A fallback is held to more than the home
is because nobody chose it — the operator's token is fleet-wide and does not
belong on an org's machine. When the agent already runs and its host stops
answering (or is removed or hidden), `operator_status` answers `host_down`
with the session and a `fallback`; the desktop panel and the phone's
**Agent** button then start it on that fallback without asking, record it
there, and revoke the stranded session's token. Its conversation so far stays
on the dead host, and once that host is back the old `fleet-operator` there
is an ordinary session to kill. The agent does not move home again on its
own when the home recovers — that would end a conversation that is working;
kill it and press the button to bring it back. Only a fleet with no host
left to take it answers `no_host` (the home is not in the fleet) or
`host_down` with no fallback (it is there, and down), and `ensure_operator`
refuses with `E_NOTFOUND` / `E_HOST_OFFLINE` before minting anything.

The birth also answers Claude Code's workspace trust dialog for the operator
directory. On a fresh host Claude Code stops at "Is this a project you
created or one you trust?" before it reads `CLAUDE.md`, and the fleet would
report the operator as `stuck_kind: trust_prompt` for good. The directory
holds nothing but the two files the fleet just wrote, so the answer is known,
and it is recorded the way Claude Code records the user's own: in the host's
`~/.claude.json`, `projects["<absolute operator dir>"].hasTrustDialogAccepted`
is set to `true`, and the project-scoped `.mcp.json` approval
(`enabledMcpjsonServers`) lists `claude-fleet` — only that server, so nothing
else is enabled on the user's behalf. The write is a read-merge-write like
the `mcpServers` entry provisioning puts in the same file: every other key
and project survives, a `.fleet-bak` copy is kept, the file is renamed into
place rather than truncated, and a file that is not the JSON object Claude
Code writes is refused (`E_PROVISION`) before anything is written — that
refusal ends the birth with no token committed and no session started, and
the next press retries.

## Asset catalog

The asset catalog — skills, agents, hooks, MCP servers and plugin refs that
Sync installs on hosts — is a git checkout on the hub's machine. The desktop
sets it in its Assets tab; on a hub, set it with `fleet-hub catalog`. A
running hub is not needed, and does not need a restart: it picks the change
up at its next catalog call (on a paired client, Assets → Refresh).
`catalog set` configures the personal catalog. An org can have its own:
`catalog add <name> <path> --org <org> [--remote <url>]` records and loads
it (on an existing name it re-points it). Which hosts take what: a host
bound to an org receives that org's catalog plus the `shared` assets of the
personal catalog; a host with no org receives all of personal plus every org
catalog it admits (`catalog admit <host> <catalog>`, `catalog unadmit`).
`catalog list` shows each catalog's owner, load state, HEAD, admissions and
grants — read-only: it parses each checkout where it is and never clones,
pulls or records a load (`reload` does); `catalog reload --catalog <name>`
re-reads one. `catalog remove <name>` forgets an org catalog — config only,
the checkout stays — along with its layer assignments, admissions and
grants, and withdraws the open changeset cards that name it (over MCP,
`remove_catalog` is the master token's alone, like `add_catalog`).

A catalog whose checkout cannot be loaded is shown as a problem (`catalog
list`) while the others load; it is retried at its next `reload`. Sync never
removes what a catalog installed because that catalog went away — not
loaded, failed, unadmitted, the host changed org, or removed — nor an asset
whose own file in a loaded catalog does not parse (or whose kind's directory
cannot be read): it reports those assets as `Noop` "kept, not removed" and
leaves them to you. Changeset cards never propose a removal either: a kept
copy stays until you sync a plan that removes it.

**Changeset cards.** After each asset scan the hub proposes cards from what
the hosts have: a Bootstrap card that adopts what is already installed into
the catalogs as layers (grouped by which hosts have each asset; personal
assets an org-bound host already has are proposed `shared`), New-on-host
cards, Drift cards (take the host's copy, or restore the catalog's) and
Rollout cards (sync a layer to its hosts). Nothing is applied until someone
applies a card through the MCP tool `changesets` (`list`, `propose`,
`apply`, `undo`, `dismiss`, `reject_item`). Applying commits only the files
it wrote, once in each catalog it changes, and commits nothing if any step
fails; if something else changed the checkout meanwhile, it leaves that
catalog alone and the card says "manual cleanup needed". `undo` reverts the
latest applied card in each catalog and puts back that catalog's host layer
assignments as they were before the apply, without touching hosts. Undo
does not un-hide what the card hid, and a layer the card's Rollout already
synced stays rolled out. Undo itself leaves every host alone, but the copies
that Rollout installed or adopted keep fleet's manifest entries while the
catalog no longer has those assets, so the next ordinary sync would remove
them (with a backup) — even copies a host had before fleet adopted them.
The undo's answer warns, naming each such asset and its hosts; to keep a
copy, put the asset back in the catalog before syncing. Undo replaces the
catalog's whole set of host layer assignments, so a layer change made there
after the apply is lost too. A card never overwrites or removes on a host,
except a Drift card's restore, which a person picks for one asset on one
host and harness and which backs up what it replaces. `catalog.auto`
(Settings → Automation → Assets, on by default) hides fleet's and Claude's
internal assets, prepares the cards after each scan, and — once a Rollout
card for a layer has been applied — installs that layer's assets a host
is missing, and adopts identical copies, by itself, skipping a host whose
rollout a person rejected. It never changes a copy a host already has,
even one that is behind the catalog: updating a drifted copy is a Drift or
Rollout card, or a sync a person runs. Off, cards come only from `propose` and nothing syncs by itself.
`catalog.auto_push` (off by default) pushes the catalog right after a card
commits or is undone. `changesets` is for the master token or a person's
own full device bound to no org (never a per-host token); listing needs no
more. Proposing needs the personal grant; applying, undoing, dismissing or
rejecting needs a grant on every catalog the card (for an apply, the items
it runs) names (`fleet-hub client grant <name> assets [--catalog NAME]`),
plus the personal grant for a Rollout, a restore or a hide; a Rollout or
restore also asks for the same confirmation as `apply_sync`. A catalog with uncommitted or
untracked files refuses an apply or undo until they are committed or moved
away.

`catalog list` and the MCP `list_catalogs` action show every catalog's path
and remote across orgs, so only the master token or a person's own unbound
full device may call them — an org-bound client is refused.

**Upgrade note.** An asset's `scope` defaults to `private`: once a host is
bound to an org, it stops receiving creates and updates for assets that are
still `private` — only `shared` ones sync to it. To share an asset, add
`scope: shared` to its `asset.yaml`. Copies already installed on an org-bound
host from before this change are kept, never removed; Sync reports them
(`Blocked` on a layered host, `Noop` on an unlayered one) instead of
uninstalling anything.

```bash
# Docker: keep the checkout on the data volume so it survives the container.
docker compose exec fleet-hub fleet-hub catalog set /var/lib/fleet-hub/agent-assets \
  --remote git@github.com:you/agent-assets.git
docker compose exec fleet-hub fleet-hub catalog show
docker compose exec fleet-hub fleet-hub catalog reload --pull   # after a push to the remote
```

- `set <path> [--remote <url>]` records the path and loads it. A path with
  no checkout is cloned from `--remote`, with this machine's git
  credentials: for an SSH remote, allow the key `fleet-hub ssh-key` prints
  to read the repository.
- `add <name> <path> [--remote <url>] --org <org>` records an org catalog
  and loads it; `list`, `remove <name>`, `admit <host> <catalog>`,
  `unadmit <host> <catalog>` manage the set; `reload --catalog <name>`
  reloads one.
- `reload [--pull]` re-reads the checkout (optionally `git pull --ff-only`
  first). Nothing pulls on its own.
- `show` prints the path, remote and last loaded commit.
- The checkout, and every directory above it, must be readable by the user
  the hub runs as (`fleet` in the image and in `fleet-hub.service`). A path
  set by another user — `sudo fleet-hub catalog set ~/agent-assets` — is one
  the running hub cannot read: every paired client's Assets tab then fails
  with `E_IO: catalog checkout <path>: Permission denied`. Run `catalog set`
  as the hub's user (`sudo -u fleet fleet-hub …`, or `docker compose exec`,
  which already is) with a path under its data directory.

The hub also loads the configured catalog when it starts. A desktop whose
`state.db` was copied over (*Migrating from the desktop*) brings its
catalog path with it; if that path is not on the hub's machine, `set` it
again.

A paired desktop sees the catalog read-only, unless you grant it the
catalog (`fleet-hub client grant <name> assets`, see *Clients*): then its
Assets tab manages this checkout — edits, commits, pushes, Sync, Secrets —
as the desktop app manages its own, and can also set the path itself (a
path on the hub's machine). Without a grant, editing is `git` in the
checkout, followed by `reload`.

## Migrating from the desktop

1. Quit the desktop app.
2. Copy its `state.db` into the hub's data dir:
   - macOS source: `~/Library/Application Support/sk.rlt.claude-fleet/state.db`
   - Linux source: `~/.local/share/claude-fleet/state.db`
3. Run `fleet-hub init --data-dir <hub-data-dir> --regenerate-token` (plus
   your `--public-url`), and reconfigure every MCP client with the new
   token. The desktop's master token must not become the public hub's: it
   has lived on the desktop machine, in its MCP clients' configs and, on
   hosts provisioned by older releases, in `curl … /hook?token=` command
   hooks.
4. Start the hub.
5. Run `provision_hosts` again — the hub's URLs (and likely its port) differ
   from the desktop's loopback ones, so every host needs its hook block and
   `mcpServers` entry rewritten. Re-provisioning also removes any old
   `curl … /hook?token=` command hooks, which carry the desktop's token; a
   hub with a public URL refuses that `?token=` form outright, so such a
   host reports no hooks until it is re-provisioned.

If the desktop had a Jira tracker, its token came along in `state.db`:
set it again on the hub (`fleet-hub tracker set-credential`), ideally a
rotated one, and see *Trackers* above.

The desktop's `state.db` carries a `local` host row for the machine it ran
on. Since the hub defaults `hub.local_host` to `false`, that copied `local`
row is hidden and marked unreachable automatically on first start — not
deleted, just no longer listed, counted, probed or polled for usage. The
sessions that were live on it are ghosted with `lost_reason =
local_disabled` on every start (nothing probes `local` on such a hub, so
they would otherwise stay live and refuse every action); they are ghosted
on the start that finds them and reaped on the next
(`retire_local_sessions`), and each reconcile pass reaps that `local` the
same way. A host you hide yourself is different: Hide is reversible, so its
sessions are only frozen at their last-known state, and Unhide finds them
again. `refresh_projects` has no
local projects directory to scan there and returns the stored list, after
folding duplicate worktree rows; `forget_project {project_id}` (master) drops
a row the scan can never revisit. And the
new-session, add-project and background-session dialogs start on the first
pickable host instead of `local`.

### Retire a renamed alias (`local` → `mac`)

A store copied from the desktop keeps its old machine under `local` while
the same machine was re-added under a new alias; worktrees, dismissals and
usage stay stranded on the hidden row. Fold it in one transaction:

1. Back up first, as root on the NAS:
   `sqlite3 /volume1/docker/fleet-hub/data/state.db ".backup /volume1/docker/fleet-hub/backup-$(date +%F).db"`
2. With the hub running: `fleet-hub host merge local mac` (or the
   `merge_host {from: "local", into: "mac"}` tool with the master token).
   If `mcp.confirm_destructive` is on, approve the request on the desktop
   or pass `confirm_nonce` from the `E_CONFIRM_REQUIRED` reply.
3. Verify: `list_hosts` no longer lists `local`; `list_worktrees
   {host_alias: "mac"}` shows the moved rows; `usage_report {host_alias:
   "mac"}`'s `by_day` includes the old days.

What moves: `worktrees` (a name `mac` already has keeps `mac`'s),
`worktree_parent_fingerprints`, `dismissed_agents`, `host_layers`,
`catalog_secrets_host`, `usage_daily` (summed per day). What is dropped: a
`local` session whose `claude_session_id` or `tmux_name` already exists
under `mac`. What is deleted: the `local` host row and its token.

The hub's default data dir is separate from the desktop's on every
platform, so a hub and a desktop app on the same machine never share a
database by accident; migration is always an explicit copy of `state.db`,
as above:

- Linux: `~/.local/share/fleet-hub` (hub) vs `~/.local/share/claude-fleet` (desktop)
- macOS: `~/Library/Application Support/sk.rlt.fleet-hub` (hub) vs `~/Library/Application Support/sk.rlt.claude-fleet` (desktop)

## Coexistence with the desktop

A host provisioned by `fleet-hub` reports its Claude Code hooks to the hub
only (the hook block is rewritten, not duplicated). A desktop app can still
see that host's sessions through its own reconcile pass, but it loses the
hook-driven signals for that host: real-time `idle`/`working` status,
`turn_seq` updates, task completion, and `safe_kill_session` finalization —
unless the desktop itself is paired to the hub as a client (see *Point a
desktop at the hub* below), where it follows the hub's own event stream
instead of reconciling independently.

Run `provision_hosts` from only one of the two — the hub or the desktop —
for a given host. Running it from both leaves the host's hook block pointed
at whichever one provisioned it last.

## Point a desktop at the hub

On Windows this is the recommended setup: Windows' `ssh` cannot multiplex,
so a standalone Windows desktop pays a fresh SSH connection per command. See
[windows.md](windows.md).

Settings → **Hub & sync**. On the hub, mint a code and paste it:

```bash
fleet-hub pair --name laptop     # prints a code; it dies on first use
```

The desktop pairs as an ordinary client — the hub cannot tell it from a phone
and should not. It stores the client token in the OS keychain (macOS),
Windows Credential Manager, or an owner-only 0600 file (Linux), never in
`state.db` and never in a log
line. Off macOS that file is *not* an OS secret store: anything running as
your user can read it, so treat that machine's account as holding a fleet
credential. Which fleet the app is a window onto is decided **once, at startup**, so
pairing and Disconnect both take effect at the next launch; Settings says so
rather than looking like nothing happened.

Plain `http://` to anything but loopback is refused unless you say otherwise:
the client token is a credential for the whole fleet, and it would cross the
network in the clear on every call, forever. The pairing dialog names the risk
and offers to do it anyway (mirroring the hub's own `--allow-plaintext`), and
an opted-in plaintext hub keeps saying so beside its badge on every launch.
The opt-in is saved as `hub.client_plaintext_token`, deliberately **not** the
daemon's `hub.allow_plaintext`: that one says a `fleet-hub` may serve a
routable bind in the clear, this one says this app may send its own
credential that way, and a `state.db` copied from one machine to the other
must not answer a question nobody asked it.

"Loopback" here means `localhost` itself, `127.0.0.0/8` or `::1` — the same
rule the agent's `--insecure` uses. A name *under* `localhost`
(`hub.localhost`) does not count: RFC 6761 only says a resolver should keep
that subtree on the machine, and one that does not would let a DNS answer
choose where the token goes. Such a URL needs the opt-in like any other.

**Upgrading.** Both of those are new, and both surface as a desktop that
resolves to *cannot be used* on its first launch after the upgrade, with the
reason in the red banner. A desktop that had opted into plaintext must opt in
again under the new key name, and a hub reached at `http://<name>.localhost`
now needs the opt-in as well — or, better, be reached at `http://127.0.0.1`.

**Disconnect does not revoke.** It forgets the URL and the token on that
machine. The client stays in the hub's list and its token stays valid there
until you revoke it (`fleet-hub client revoke <name>`) — a paired client is
refused `revoke_client` by design, so the app could not do it even if it
tried. For a lost laptop, revoke on the hub.

### When the configured hub cannot be used

If a hub is configured but a launch cannot use it, the desktop **owns
nothing** until that is fixed. It does not fall back to standalone. The
causes are:

- no client token is stored;
- the token cannot be read, for example because the macOS keychain was locked
  at launch or its prompt was denied;
- the client token could not be read in time — the keychain was locked and
  its prompt did not answer within 10 s. The app logs `startup: resolving
  backend` … `startup: backend resolved elapsed_ms=…` around this step;
  unlock the keychain and relaunch;
- the URL is plain `http://` to a host that is not loopback, and
  `hub.client_plaintext_token` is not set;
- `hub.remote_url` does not parse;
- the settings cannot be read, but a client token is stored, which proves the
  app was paired.

In that state it runs no reconcile tick, no account-usage poll, no embedded
control API and no event stream. Every fleet command is refused with
`E_HUB_UNAVAILABLE` and the reason. A red banner at the top of the window
names the hub and the reason, with a button to Settings → Hub & sync. There you can
pair again, or Disconnect to go back to standalone. Either takes effect at the
next launch.

Falling back to standalone would be the dangerous choice. You pointed the app
at a hub, so the fleet is the hub's. An app that quietly started reconciling
it again would be a second brain for the same hosts, and that is the failure
this mode exists to prevent. With no `hub.remote_url` at all, the app is
standalone exactly as before.

**When the hub does not answer at launch.** The splash chases the hub, and
after 6 s reads Signal lost with three choices: Retry, Hub settings…, and
Open offline. Open offline lists this computer's own tmux sessions and
nothing of the hub's fleet (`offline_local_sessions`, which reads this
machine's tmux server and writes nothing), with Copy attach command and Open
in VS Code for each. It is not standalone: no reconcile, no host the hub
manages, no `state.db` row. When the hub answers, the offline view steps aside
for the app.

### What is different from standalone

- **Live, from the hub.** The desktop follows the hub's `GET /events` and
  re-emits every change as the same frontend event a local change would have
  produced, so the window updates itself. When that stream drops it reconnects
  with backoff, re-lists sessions, hosts, tasks and accounts once, and shows a
  banner — "what you see may be out of date", the attempt number and the
  reason — until it is back. After a dropped stream the app resumes from the
  last event it applied when the hub still has it (the last 512 events, roughly thirteen minutes of a busy fleet);
  otherwise it re-lists. A stream that goes silent (not even the hub's
  15-second keep-alive) for about 40 seconds is treated as dead, which is what
  a laptop that slept and woke on another network looks like.
- **The fleet is the hub's.** No reconcile tick, no account-usage poll and no
  embedded control API in the desktop; two brains for one fleet is the failure
  this mode exists to prevent. The footer's database and schema are the hub's
  too, and the version line names both sides — `app 0.4.5 · hub 0.4.6 · db:
  ok · schema 90`, where everything after `app …` is the hub's. Settings →
  Hub repeats the two in words. They are allowed to differ: the hub and its
  clients are released separately, and what decides whether they can talk is
  the wire contract (an unacceptable one gets the banner above), not matching
  version numbers.
- **Projects are added through the hub.** "＋ Add project…" clones or
  creates the repository on the host you pick, with that host's `git` and
  `gh`; the new row arrives like any other change. Cancel stops the desktop
  waiting, not the run on the host. The *Existing folder* source is absent
  here, because it would mean a folder on the hub's machine, and so is the
  "into …" destination preview: the project roots are the hub's settings.
- **Its errors reach the hub.** Error-level events and frontend crashes are
  queued and posted to the hub's `/report` every few seconds — see *Error
  reports*; `CLAUDE_FLEET_HUB_REPORTS=0` turns it off.
- **A prompt sent from the desktop reaches the agent marked *untrusted*,**
  exactly as one typed on a phone does, unless the hub's operator has
  **trusted** this client (`fleet-hub pair --trusted` when pairing it, or
  `fleet-hub client trust <name>` afterwards — see *Clients*). `apply_marker`
  refuses `raw=true` to any non-master caller and a paired client is never
  the master; a trusted client is delivered unmarked without asking for
  `raw`. For the desktop you type on yourself, trusting it is the intended
  setting; the marker exists for text an agent produced.
- **Destructive confirmations are answered on the hub.** With
  `mcp.confirm_destructive` on, `kill_session`, `delete_worktree`,
  `move_session` and `cancel_task` come back `E_CONFIRM_REQUIRED`, and the
  request waits in the hub's queue. The desktop's dialog and Control's
  cards list and answer that queue (`mcp_confirms`, `answer_mcp_confirm`)
  and follow it live through `confirm:changed`; the approved change then
  arrives over the hub's event stream like any other. A hub older than
  these tools has no queue to show: the dialog stays empty, as before.
- **Fleet administration is refused.** A client is not the fleet's
  administrator, so those controls are disabled in the interface with the
  reason rather than failing at the click. *What a hub client refuses* below
  lists exactly which ones.
- **The terminal attaches, the same as standalone.** The PTY is a local
  `ssh <host>` / `tmux attach` from this machine, built from the alias and
  tmux name of the selected session; it reads no `state.db` and the hub is
  not in that path, so being a paired client changes nothing about it. The
  hub still streams no pane — it does not need to. Two things follow. A host
  this machine has no `Host` block for fails in `ssh`, with ssh's own message,
  in the pane: the fleet's aliases come from the *hub's* `ssh/config`, and
  where the two disagree that is what you see. And a session on an
  **agent host** is not offered an attach at all — nothing anywhere can dial
  such a host, which is the whole reason it dials the hub instead — so the tab
  says that rather than showing a command that cannot work. Dropping files on
  the pane (`upload_to_session`) follows the same rule, for the same reason.
- **The asset catalog** is the hub's. The Assets tab asks the hub's
  `catalog_admin` once when it opens. For a client granted the catalog
  (`fleet-hub client grant <name> assets`, see *Clients*) it is the full
  panel onto the hub's checkout: set it up, edit assets, lint, commit, push,
  Sync, Secrets and Import from host, exactly as standalone — only *Open in
  session* stays disabled. Import reads any host over SSH (`import_assets {
  host_alias }`), so a hub imports from the machines it manages. A resource
  file you add is read on this machine and its bytes sent to the hub. For
  any other client the hub answers `E_FORBIDDEN`
  and the tab is a read-only overview: the hub's catalog through
  `list_assets` (each asset's per-host state, unmanaged assets, problems;
  the desktop sends `all_catalogs`, which the hub honours for an unbound
  full client and answers personal-only for anyone else, as it does
  without the flag) and a Scan hosts button through `scan_assets`, with the grant command to
  ask the operator for. The catalog itself can also be set on the hub's
  machine with `fleet-hub catalog set` (see *Asset catalog*).
- **The setup checklist** is about the machine that owns the fleet, so it
  shows the reason instead of its panel.
- **A revoked or rotated token** comes back `E_UNAUTHORIZED` on every call;
  the error says to pair again in Settings → Hub & sync.

### What a hub client refuses

What a hub client cannot do from here, and what to do instead: every command
that is local-only outright, plus `repair_session`, which routes for one
argument shape and refuses for the other. Generated from the same table the
backend enforces from (`src-tauri/src/backend/verdicts.rs`) — that file also
has the full verdict for every command, including the ones that route
normally or run the same in both modes, which this table leaves out because
they tell an operator nothing they came to docs to learn. Regenerate with:

```text
REGEN_HUB_VERDICTS=1 cargo fleet-test -- verdict_gen
```

<!-- BEGIN GENERATED: hub-client verdicts -->
<!-- Regenerate with: REGEN_HUB_VERDICTS=1 cargo fleet-test -- verdict_gen -->

Every command below refuses in hub client mode; the full table, with the commands that route to a hub tool, is `src-tauri/src/backend/verdicts.rs`.

| Command | What to do instead |
| --- | --- |
| `account_spend` | this app collects no usage while a hub owns the fleet, so its store has no spend per account; read usage on the hub |
| `account_usage_history` | this app does not poll account usage while a hub owns the fleet, so it keeps no history; read usage on the hub |
| `add_host` | registering a host is fleet administration, which the hub reserves for its own operator — add it there with `fleet-hub` |
| `add_tracker` | trackers and their credentials are fleet administration: the hub's work_admin is master-only, and a paired client is never the fleet's administrator; configure them on the hub with `fleet-hub tracker add\|set-credential\|test` |
| `catalog_spawn_author_session` | an author session is a Claude session started in the catalog's checkout on the machine that owns it, and the hub has no tool that starts one; edit the assets from this panel, or start a session in the checkout on the hub's machine |
| `check_host` | the health checklist reads a host's settings over this app's own SSH; repair a host's hooks from the hub with `fleet-hub provision --host <alias>` |
| `check_local_prereqs` | the onboarding checklist is about running a fleet from this machine, which the hub is doing instead |
| `decide_status_map_proposal` | the decision model's Asana section proposals are tracker administration: applying one writes the tracker's section map through the hub's work_admin, master-only, and a paired client is never the fleet's administrator; decide them on the hub with `fleet-hub decide proposals apply\|reject` |
| `discard_host_setup` | the add-host wizard adds a host of this machine's ~/.ssh/config and checks it over this app's own SSH; the hub adds hosts with `add_host` and installs fleet-agent with `install_agent` |
| `discard_kill_session` | the hub exposes no tool that discards a worktree and kills in one step; use safe_kill_session, or do it from the hub |
| `discover_hosts` | it reads this machine's ~/.ssh/config, not the hub's — register hosts on the hub itself with `fleet-hub` or a standalone app |
| `dismiss_agent_session` | use Kill instead: the hub's kill_session removes an inactive agent from the list exactly as this would. It is not routed here because the two differ on a WORKING agent, which this refuses and kill_session stops |
| `draft_commit_message` | the hub exposes no draft tool — a commit message is drafted where the commit is made: in the session, or from a standalone app |
| `fetch_page_source` | a page's data sources read this fleet's store, which the hub owns; read the same numbers on the hub with usage_report |
| `flow_back` | a flow administers the fleet this app owns, and the hub owns it; connect a tracker on the hub with fleet-hub tracker add <ticket-url> |
| `flow_cancel` | a flow administers the fleet this app owns, and the hub owns it; connect a tracker on the hub with fleet-hub tracker add <ticket-url> |
| `flow_start` | a flow administers the fleet this app owns, and the hub owns it; connect a tracker on the hub with fleet-hub tracker add <ticket-url> |
| `flow_submit` | a flow administers the fleet this app owns, and the hub owns it; connect a tracker on the hub with fleet-hub tracker add <ticket-url> |
| `hide_host` | hiding a host is fleet administration, which the hub reserves for its own operator — hide it there with `fleet-hub` |
| `inspect_safe_kill` | it inspects the worktree over this machine's SSH connection and the hub exposes no tool for it; retire the session from the hub |
| `install_fleet_hook` | the hook it installs points at this app's control API, which is not running; install it from the hub |
| `list_host_setups` | the add-host wizard adds a host of this machine's ~/.ssh/config and checks it over this app's own SSH; the hub adds hosts with `add_host` and installs fleet-agent with `install_agent` |
| `list_host_tokens` | these are this app's own per-host tokens, not the hub's; list them on the hub |
| `mcp_configure` | starting a second control API against a fleet the hub already owns is the failure remote mode exists to prevent; configure the hub's |
| `mcp_status` | this app runs no embedded control API while a hub owns the fleet; the hub is the control API |
| `merge_host` | merging one host's rows into another is fleet administration, which the hub reserves for its own operator — run it there with `fleet-hub host merge <from> <into>` |
| `probe_ssh_alias` | it SSHes from this machine to preview a host for the Add-host dialog; the hub is the one that must be able to reach it |
| `propose_host_placement` | the decision model and the account usage are the hub's while it owns the fleet; pick the host as usual |
| `provision_hosts` | it rewrites every host's hook block to report to this app; provision from the hub with `fleet-hub provision [--host <alias>] [--content-only]` |
| `purge_project` | it deletes Claude Code state on every host over this machine's SSH connections and the hub exposes no tool for it; purge from the hub |
| `record_host_placement` | the decision model's runs are recorded on the hub that owns the fleet; nothing to record here |
| `refresh_account_usage` | it reads the account's usage over this machine's SSH connection to the host; refresh it on the hub |
| `remove_host` | removing a host is fleet administration, which the hub reserves for its own operator — remove it there with `fleet-hub` |
| `remove_tracker` | trackers and their credentials are fleet administration: the hub's work_admin is master-only, and a paired client is never the fleet's administrator; configure them on the hub with `fleet-hub tracker add\|set-credential\|test` |
| `repair_session` | Refuses when explicit: false, the automatic pre-attach check (otherwise routes to `repair_session`): the hub's repair_session always runs the EXPLICIT repair, which may unregister a stale worktree entry, adopt a moved checkout and recreate a branch — this app will not turn an automatic pre-attach check into that; repair explicitly, or from the hub |
| `repair_workspaces_now` | repairing every workspace or restoring every host's lost sessions at once is the fleet's own pass: on a paired client, open the host in Hosts and use its Repair or Restore, which route to the hub |
| `repo_checkout` | the hub exposes no git-write tool — a remote client must not stage or commit under a running agent; do it in the session, or from a standalone app |
| `repo_checkout_commit` | the hub exposes no git-write tool — a remote client must not stage or commit under a running agent; do it in the session, or from a standalone app |
| `repo_commit_create` | the hub exposes no git-write tool — a remote client must not stage or commit under a running agent; do it in the session, or from a standalone app |
| `repo_create_branch` | the hub exposes no git-write tool — a remote client must not stage or commit under a running agent; do it in the session, or from a standalone app |
| `repo_delete_branch` | the hub exposes no git-write tool — a remote client must not stage or commit under a running agent; do it in the session, or from a standalone app |
| `repo_delete_merged_branches` | the hub exposes no git-write tool — a remote client must not stage or commit under a running agent; do it in the session, or from a standalone app |
| `repo_fetch` | the hub exposes no git-write tool — a remote client must not stage or commit under a running agent; do it in the session, or from a standalone app |
| `repo_pull` | the hub exposes no git-write tool — a remote client must not stage or commit under a running agent; do it in the session, or from a standalone app |
| `repo_push` | the hub exposes no git-write tool — a remote client must not stage or commit under a running agent; do it in the session, or from a standalone app |
| `repo_stage` | the hub exposes no git-write tool — a remote client must not stage or commit under a running agent; do it in the session, or from a standalone app |
| `repo_unstage` | the hub exposes no git-write tool — a remote client must not stage or commit under a running agent; do it in the session, or from a standalone app |
| `restore_all_lost_sessions` | repairing every workspace or restoring every host's lost sessions at once is the fleet's own pass: on a paired client, open the host in Hosts and use its Repair or Restore, which route to the hub |
| `rotate_host_token` | it re-provisions the host to report to this app; rotate the token on the hub |
| `run_host_setup_check` | the add-host wizard adds a host of this machine's ~/.ssh/config and checks it over this app's own SSH; the hub adds hosts with `add_host` and installs fleet-agent with `install_agent` |
| `save_host_setup` | the add-host wizard adds a host of this machine's ~/.ssh/config and checks it over this app's own SSH; the hub adds hosts with `add_host` and installs fleet-agent with `install_agent` |
| `set_account_nickname` | the nickname lives in the hub's database and there is no tool to set it; rename the account on the hub |
| `set_host_token_mode` | these are this app's own per-host tokens, not the hub's; change the mode on the hub |
| `set_tracker_credential` | trackers and their credentials are fleet administration: the hub's work_admin is master-only, and a paired client is never the fleet's administrator; configure them on the hub with `fleet-hub tracker add\|set-credential\|test` |
| `status_map_proposals` | the decision model's Asana section proposals are tracker administration: applying one writes the tracker's section map through the hub's work_admin, master-only, and a paired client is never the fleet's administrator; decide them on the hub with `fleet-hub decide proposals apply\|reject` |
| `test_tracker` | trackers and their credentials are fleet administration: the hub's work_admin is master-only, and a paired client is never the fleet's administrator; configure them on the hub with `fleet-hub tracker add\|set-credential\|test` |
| `tracker_sync_metrics` | trackers and their credentials are fleet administration: the hub's work_admin is master-only, and a paired client is never the fleet's administrator; configure them on the hub with `fleet-hub tracker add\|set-credential\|test` |
| `tunnel_status` | the tunnels belong to the process that owns the fleet; check them on the hub |
| `update_tracker` | trackers and their credentials are fleet administration: the hub's work_admin is master-only, and a paired client is never the fleet's administrator; configure them on the hub with `fleet-hub tracker add\|set-credential\|test` |
| `work_retention_status` | work retention is the hub's own sweep of its store: its status and sweep_now are the hub's work_admin, master-only, and a paired client is never the fleet's administrator; set the windows with set_setting and read the status on the hub |
| `work_retention_sweep` | work retention is the hub's own sweep of its store: its status and sweep_now are the hub's work_admin, master-only, and a paired client is never the fleet's administrator; set the windows with set_setting and read the status on the hub |
<!-- END GENERATED: hub-client verdicts -->

### Version skew

Every `/events` hello frame carries the hub's wire-contract revision (see
*Events* above). The desktop only trusts a hub whose revision falls inside
the range this build understands (`MIN_HUB_CONTRACT..=MAX_HUB_CONTRACT`,
`src-tauri/src/backend/contract.rs`). Outside it, the banner says which side
is behind and what to do — the hub is too old (update the hub) or this app
predates the hub (update this app) — and, for as long as that connection
lasts, the event bridge applies no row event from it and calls no resync (the
backfill a fresh connection would otherwise do): stale-but-honest beats
fresh-but-wrong for anything driven by the live stream. It keeps retrying on
the same backoff rather than hammering a hub it cannot use, and re-checks the
revision on every reconnect, so an upgrade on either side is picked up on
its own without restarting the app.

Every **call** to that hub is refused too, for as long as its revision is the
last thing this window learned: the routed reads (`list_sessions`,
`list_hosts`, `session_conversations`, the repo reads, the health check, the
focus-refresh path) and the routed mutations alike, since a mutation's answer
is a row the window merges like any other. They answer `E_HUB_CONTRACT` with
the banner's own sentence — which side is behind and what to do — and nothing
is sent, so there is nothing to read with a renamed field silently defaulted.

"The last thing this window learned" is exactly that, and not "how the
connection is doing right now". `GET /events` and `POST /mcp` are separate
sockets: a hub whose event stream is down, or behind a flapping proxy, can
still answer calls, and its row shapes have not changed because a socket
dropped. So only a hello frame moves the verdict — a desktop that has not
finished a handshake yet (`connecting`) calls, because nothing has been
learned; a stream that ended, or that answered anything other than 200,
changes nothing either way for calls; and the first `ready` frame that
classifies the hub back in range opens all of it again, with no restart.

One narrower thing does short-circuit a call, and only to save it waiting:
after **two consecutive failures to even CONNECT** to the hub — no socket at
all, not a hub that answered a 503 or closed the stream — a call is refused
immediately with `E_HUB_UNREACHABLE` instead of spending its own bound
discovering the same thing. The event bridge's reconnect is already probing;
the user's click need not probe again. Anything that got an answer out of the
hub leaves calls alone, because `GET /events` and `POST /mcp` are separate
sockets and a hub whose stream is unhappy can still serve every call.

**Upgrade a desktop and its hub together.** Both ends require the current
revision: a new desktop refuses an older hub (any revision below
`MIN_HUB_CONTRACT`, today 13: "update the hub"), and an older desktop refuses
a newer hub ("update this app"). There is no mixed window in which the two
work together. Revisions 2 and 3 are why `move_session`'s preview and wait
are guarded below: revision 2 is the release where `move_session` answers a
tagged result (`kind: moved | preview`) and honours `dry_run`, and revision 3
adds a `when` argument (`now` | `idle` | `cancel`): `idle` waits for the
source to go idle before moving, `cancel` ends a pending wait instead of
moving anything.

Two things wait for more than the absence of a skew: a Transfer preview
(`move_session` with `dry_run: true`) and a call whose `when` is not `now`. A
hub from before revision 3 ignores both `dry_run` and `when`, so it performs a
real move regardless of what either one asked for. For `dry_run` and `when:
idle` that only means the desktop refuses to ask something it could not trust
the answer to; for `when: cancel` it is the reason the guard exists at all —
an old hub sees an ordinary move request and moves the session, so cancelling
a wait would perform the very move it was meant to stop. Either way the
desktop refuses with `E_HUB_CONTRACT` until the current connection's
`ready` frame has been judged in range — not merely "no mismatch recorded
yet", which is also what a desktop still connecting sees. A dropped event
stream withdraws that judgement until the next `ready` frame: the hub that
answers the reconnect may be an older build. These calls become available
once the desktop has confirmed the hub's version; a plain move (`when: now`,
not a dry run) is not held back by this.

### Parity or refusal

A desktop mutation is routed to the hub **only where the desktop's arguments
map one-to-one onto the tool's parameters**, checked field by field. Where
they do not, the command *refuses* instead of routing.

`new_session` set the rule, and used to be its example: `NewSessionArgs`
carried `kind`, `start_command` and `friendly_name` that the tool's
`NewSessionParams` carried none of, and a shell session was a different tool
entirely, so routing it would have **succeeded** while silently dropping the
label the user typed. A refusal is visible; a dropped field is not. The gap
is closed — the tool's params grew the three fields (optional; absent is
today's MCP behaviour) — so a hub client can create a session, including a
shell session with a start command and a label, the same way a standalone
desktop does.

`repair_session` is still partly refused, for the same shape of reason: the
tool always runs the *explicit* repair (which may unregister a stale worktree
entry, adopt a moved checkout and recreate a branch), and the desktop's
automatic pre-attach check has no counterpart. Only `explicit: true` — the
Repair workspace button — routes; the automatic check stays local-only rather
than silently becoming a destructive explicit repair.

Do not "fix" a refusal like this by wiring a lossy mapping. If a tool grows
the missing parameters, route it then.

### Known limitations

- **The terminal, for an agent host only.** A session on an agent host cannot
  be attached from anywhere; see *The terminal attaches, the same as
  standalone* under *What is different from standalone* above.
- **Projects and worktrees are not re-listed on reconnect**, because their
  list tools answer a different shape from their events. They refresh when
  the window regains focus.
- **Partly accepted live.** A macOS desktop has run in this mode against a
  remote hub since 2026-09-25: launch, the keychain token, reconnect after a
  lost network or a hub restart, and gap replay are observed. Steering,
  un-pairing and the dialogs listed in [hub-acceptance.md](hub-acceptance.md)
  are not yet. The macOS keychain arm of `token_store.rs` is tested against
  the real login keychain (`token_store::keychain_tests`). Report anything
  that does not match this page.

### Going back

Disconnect in Settings and restart. Disconnect is also offered while the
configured hub cannot be used (see above), so a half-finished pairing can
always be cleared. The desktop's own `state.db` is untouched
throughout — pointing it at a hub is a view change, not a data move — so it
resumes managing whatever it managed before. Nothing migrates in either
direction; see *Migrating from the desktop* above for moving a database
deliberately.

## Security notes

- **Bearer tokens.** Same model as the desktop's Control API: a master token
  for the hub itself, a per-host token for every provisioned host (see
  `control-api.md` → *Per-host tokens*). Every request needs
  `Authorization: Bearer <token>`.
- **Client tokens.** A paired device holds a third kind of token: named,
  revocable, `full` or `readonly`, never the master and never fleet admin.
  Only its SHA-256 is stored. A client may additionally be *trusted*
  (`client_tokens.trusted_at`), which drops the untrusted-content marker
  from what it sends and nothing else — it widens what an agent will believe,
  not what the token may call, so grant it to a device you type on and not
  to a token an agent holds. What crosses the room in the QR is a
  single-use, minutes-long pairing *code* in a URL fragment — not a token —
  and `POST /pair`, the one unauthenticated route besides `/healthz`, is
  rate-limited to one attempt per address every six seconds (an IPv6
  address counts as its /64) and to thirty attempts a minute across the
  whole hub, so minting addresses buys no extra guesses. See *Pair a phone* and
  *Clients* above.
- **Failed bearers are logged once per address.** A bad or missing token on
  any authenticated route is answered `401`, every time — a client reads
  `401` as "pair again", never as a busy hub. The `[mcp] rejected request`
  warn line, which names the address, is written once per second per source
  address (the peer, or the last `X-Forwarded-For` hop when the peer is a
  private or loopback proxy — the same rule `/pair` uses); repeats inside
  that second are logged at debug. Successes do not touch the bucket.
- **Slow and surplus connections are closed.** A connection must deliver
  each request's head (request line and headers) within 30 s of the hub
  starting to wait for it, so a peer that connects and sends nothing, drips
  a header byte at a time, or leaves a kept-alive connection idle is closed
  without a token ever being checked. At most 512 connections are served at
  once; one past that is closed on accept (logged at warn, at most once a
  minute), so a flood fills that cap before it can exhaust the process's
  file descriptors. Long-lived streams (`/events`, `/agent`, the `/mcp` long
  polls) are unaffected once their request is in. The hub speaks HTTP/1.1
  only.
- **Peer tokens.** A linked hub holds a fourth kind of token, mode `peer`: it
  reaches the `peer_exchange` tool only — every other tool answers
  `E_FORBIDDEN` and `/events` answers `403` — and it is never trusted; there
  is no `--trusted` for a peer link, and a host token can never hold the
  `peer` mode. A message that arrives over a link is stored marked as
  untrusted input and is never typed into a pane: the only thing it can do
  to a pane is wake an idle session with a fixed one-line nudge, never the
  remote text itself. See *Link two hubs* above.
- **`pair_client --person` is an operator capability, and the privacy model
  does not fence the holder of the master token.** A session started through
  fleet is private to its owner, and that holds against an *organisation*
  admin: there is no override, audited or not. It does not hold against
  whoever runs the hub. The master token mints a single-use pairing code for
  any named person (`fleet-hub pair --person <name>`, or the `pair_client`
  tool), `POST /pair` ties redemption to nobody, and the operator can
  therefore redeem a code themselves and hold a `full` client token whose
  person is a colleague's — which sees that colleague's private sessions.
  Nothing is being hidden by saying so: an operator already holds the
  database and a shell on the hub. What the hub records is the `pair_client`
  audit line, not a per-session access record. Give the master token to the
  people you would give the database to, and nobody else. The full statement
  — which admin is fenced, which is not, and what it means when they are the
  same person — is *Who owns a session* → *Privacy, precisely*.
- **A peer link is an operator-to-operator channel, and it is NOT fenced by
  person.** On a hub with several people (*Who owns a session*), a message
  arriving over a link resolves its recipient by fleet address —
  `<fleet>/<host>/<session>` — and lands in that session's inbox, with a
  timeline row and, when the sender asked to wake it, the fixed nudge into
  its pane, whoever owns the session. There is deliberately no person check
  on that path: a peer token names no person, and the operator who ran
  `fleet-hub pair --mode peer` is the one the privacy model puts out of scope
  — they hold the hub's master token and its database already. What this
  means in practice: **do not link a hub to one whose operator you would not
  give a device on your own hub.** `fleet-hub peer remove <name>` ends it,
  and an addressed session that does not exist answers
  `E_PARTICIPANT_UNKNOWN`, so a link can also probe which session names
  exist on this hub.
- **A peer's own words cannot forge the marker that quotes them.** If a
  message body from another fleet happens to contain a line matching
  fleet's own untrusted-content marker, that line is neutralised (prefixed
  `> `, and every `[claude-fleet` in the body defused to `(claude-fleet`,
  whatever invisible character sits in front of it) before it is ever
  stored — a peer cannot close the marked block early and have the rest of
  its text read back as fleet's own.
- **Trust in a link is decided once, at pairing, by identity — not by a
  fleet-id allowlist.** The pairing code itself is the credential: only
  someone who can already run commands on the other hub can mint one, and
  the first exchange pins whichever `fleet_id` that hub answers with for
  the life of the link. There is nothing to pre-register, because a fleet
  id is not secret and requiring one in advance would mean the listener's
  operator already had to know the dialer's id before pairing — the code
  is what lets them skip that.
- **A reverse proxy in front of the hub must APPEND to `X-Forwarded-For`.**
  That per-address budget keys on the request's TCP peer, except when the peer
  is a loopback or private address — the compose topology, where the peer is
  Caddy — in which case it believes the **last** parseable hop in
  `X-Forwarded-For`, i.e. the address that proxy saw. A proxy that *replaces*
  the header appends the real client and is correct; one that forwards a
  client-supplied header verbatim, or sets the header from a client-controlled
  value, would let a caller choose its own bucket — spend someone else's
  budget, or dodge its own. Caddy's `reverse_proxy` appends by default
  (`deploy/hub/Caddyfile` relies on it); if you front the hub with something
  else, check that it does too.
- **TLS.** Two ways, and the hub defaults to neither doing it itself: put TLS
  in front of it, or let it terminate TLS with `--tls cert`. The Docker setup
  takes the first road with Caddy (automatic certificates via its domain,
  `deploy/hub/Caddyfile`); the bare-binary setup either needs your own proxy,
  or runs `--tls cert` with a certificate and key you supply and renew (see
  *Single binary with its own certificate* above) — or binds loopback and is
  reached over Tailscale/SSH, with no public URL at all. Whichever you pick,
  a routable bind serving plaintext is refused unless you pass
  `--allow-plaintext`.
- **Per-host tokens and the org boundary.** A per-host token's scope is
  computed from its host's org on every call (and on every `/events`
  frame after an org change), in one place (`Caller::org_scope`); the
  work service layer filters with it, and every tool answer a per-host
  token receives passes one more redaction of session rows' work fields.
  An isolation matrix test runs every `work` / `work_link` / `work_admin`
  action for master, clients and hosts in two orgs and in none. See
  *Organisations and isolation*.
- **`state.db` permissions.** Written `0600` on the hub's machine, same as
  the desktop — the file is created owner-only before SQLite opens it, so
  the WAL sidecars `state.db-wal` and `state.db-shm` (which hold every
  recent commit, tokens included, while the daemon runs) inherit `0600`
  too; a leftover sidecar is tightened on the next open.
- **`mcp.confirm_destructive`.** This desktop setting gates destructive
  tools (`broadcast_prompt`, `kill_session`, `delete_worktree`, …) behind a
  UI confirmation dialog. On a hub the request waits for the owner's paired
  device instead (`mcp_confirms`, `answer_mcp_confirm`) and expires after
  ten minutes unanswered; leave the setting off (its default) unless one of
  your devices will answer. A `state.db` copied from a desktop can carry it
  switched on; `fleet-hub serve` logs a warning at startup when it is.
- **The operator's starts and kills.** The UX agent's operator session
  must have its session starts and restarts (`new_session`,
  `new_shell_session`, `new_bg_session`, `spawn_review`, `dispatch_task` with
  `new_worker`, `restore_host_sessions` other than a `dry_run`,
  `recreate_session`, `restart_session`, `work_link` `start` / `resume`)
  and kills approved by a person, whatever
  `mcp.confirm_destructive` says (work graph M9.7, decision D12). On a hub
  the call waits in the hub's queue until the owner approves it from a
  paired device (redesign step 9.2: a card in Control's transcript, or the
  dialog); the operator can neither list nor answer that queue.
- **Rotating tokens.** `fleet-hub token regenerate` mints a fresh master
  token — reconfigure every client afterward. For host tokens, call
  `provision_hosts { rotate: true }` (from any client), which re-provisions
  every SSH host with a new per-host token. An **agent** host's token is
  rotated out of band instead (`fleet-hub agent-token <host> --rotate`, then
  re-install the agent). See *A host that cannot be reached*. Rotating a
  `readonly` token keeps it `readonly`; `fleet-hub host-token-mode <host>
  full` is what widens it again.

## Troubleshooting

- **`403` from the Host check** — the request's `Host` header isn't on the
  hub's allowlist (which always includes the public URL's own host). Add
  extra names with `--allowed-host`, or — if a proxy in front of the hub
  rewrites `Host` to something else — keep the Caddyfile's
  `header_up Host 127.0.0.1:4180` rewrite, which the loopback allowlist
  entry always accepts.
- **An agent that will not connect.** Run `fleet-agent status` on the host,
  or `journalctl -u fleet-agent`:
  - `hub refused: 401` means the token is wrong, rotated or revoked
    (re-install with `fleet-hub agent-token <host>`);
  - `403 … readonly` means the token mode is not `full`, and rotating will
    not fix it — run `fleet-hub host-token-mode <host> full`;
  - `403 … not an agent host` means the host is not on the agent transport;
  - `429` means the host already holds two connections, or the hub holds
    64 in all;
  - `invalid peer certificate` means pass `--ca-file`;
  - `refusing the plain hub` means use `https://`.
- **`401`** — wrong or missing token. Confirm the client sends
  `Authorization: Bearer <token>` with the exact current token
  (`fleet-hub token show`).
- **`refusing to serve plaintext http` at startup** — a routable `--bind`
  without an `https://` public URL (an `http://` one, or none at all). Use
  `https://` in front of a TLS proxy, bind `127.0.0.1`, or pass
  `--allow-plaintext` for a private-network or container-internal
  plaintext hop.
- **`no hub is answering on 127.0.0.1:<port> — start fleet-hub serve first`**
  from `pair` / `client list` / `client revoke` — these three drive the
  *running* hub, not the database. Start the daemon, and point the command at
  the same data dir and port it runs with (`--data-dir`, `--port`, or the
  `FLEET_HUB_*` env the unit sets).
- **The phone says the pairing code is invalid** — a code is single-use, it
  expires (10 minutes by default), and a hub restart voids every outstanding
  one. Mint a fresh one with `fleet-hub pair`. A `429` instead means the
  address has spent its attempt budget (one every six seconds), or the hub as a
  whole has had thirty attempts this minute; retry after `Retry-After`.
- **`fleet-hub pair` refuses with `E_EXISTS`** — a live client already holds
  that name. `fleet-hub client revoke <name>` first, or pair under another
  name; a revoked row does not block the name.
- **`could not bind`** — another process already holds the configured
  `--bind`/`--port`. Pick a different port, or find and stop what's using
  it.
- **A host shows `skipped: unreachable` from `provision_hosts`** — the
  hub's SSH key (`fleet-hub ssh-key`) is not in that host's
  `~/.ssh/authorized_keys`, or `./ssh/config` (bare binary: `~/.ssh/config`
  as the `fleet` user) is missing or unreadable by uid 1000. In Docker,
  confirm `./ssh/config` is owned by uid 1000 (`sudo chown 1000:1000
  ./ssh/config`) and has the right `Host` alias for the target.
- **`Host key verification failed` in the hub's log, or a host recorded
  unreachable right after `add_host`** — the host's key is missing from
  `./ssh/known_hosts` (bare binary: `~fleet/.ssh/known_hosts`). The hub
  connects with `BatchMode=yes` and cannot accept a new key interactively:
  run `ssh-keyscan -H <HostName>` (with `-p <Port>` when the host's block
  sets one) into that file, then restore its ownership (uid 1000) and mode
  `0600`.
- **Hooks never arrive (status stays stale, `safe_kill_session` never
  finalizes)** — the host cannot reach the hub's public URL. From the host
  itself, run `curl -sI https://fleet.example.com/mcp` and confirm it
  connects; check DNS, firewalls, and that Caddy has a valid certificate.
- **`fleet-hub reports` is empty** — the hub predates the route (the desktop
  logs `no /report route` once), the desktop was started with
  `CLAUDE_FLEET_HUB_REPORTS=0`, the agent's config has
  `report_errors: false`, or the sender's `RUST_LOG` silences `error`.

**One host is slow; everything looks stale.** A reconcile pass probes every
host in parallel and writes each host's rows the moment its probe answers, so
a host that takes the full 65 s probe budget delays only its own freshness;
`fleet_health.hub.reconcile.last_duration_ms` still shows the pass as slow,
and the host's `[reconcile] host probe exceeded its wall clock` line names it.

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
