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

**The image.** The compose file pulls `ghcr.io/martin-janci/fleet-hub:latest`.
A new ghcr package starts out private, and no `latest` tag exists until the
first `v*` release tag is pushed (a manual run of the `hub-image.yml`
workflow publishes only a `sha-<commit>` tag). Until then — or if you cannot
pull the package — build the image locally from a checkout of the repository
and point `image:` in `docker-compose.yml` at it:

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

## Bare binary

Prefer running without Docker, or need it as a system service:

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

To print the master token again later, use the same user and data dir:

```bash
sudo -u fleet env FLEET_HUB_DATA_DIR=/var/lib/fleet-hub fleet-hub token show
```

Every `fleet-hub` subcommand opens `<data-dir>/state.db`, creating it when
missing: `token show` without the right data dir (or as the wrong user)
creates a separate, empty database with its own fresh token instead of
printing the running hub's.

Put it behind your own TLS-terminating proxy (the same role Caddy plays in
the Docker setup) and set `FLEET_HUB_PUBLIC_URL`. Or skip the public URL
entirely and bind loopback, reaching it over Tailscale or an SSH tunnel of
your own: with no public URL configured, the hub behaves exactly like the
desktop app — it binds `127.0.0.1` and opens a reverse SSH tunnel to every
provisioned remote host. If you instead bind a non-loopback address with no
public URL (for example the machine's Tailscale address,
`--bind 100.64.0.1`), pass `--allow-plaintext` (or set
`FLEET_HUB_ALLOW_PLAINTEXT=1`): the hub refuses any non-loopback bind that
is not fronted by an `https://` public URL unless plaintext is explicitly
allowed.

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
| `--allow-plaintext` | `FLEET_HUB_ALLOW_PLAINTEXT` | — | off |
| `--log-dir` | `FLEET_HUB_LOG_DIR` | — | `<data-dir>/logs` |

`--allow-plaintext` permits a non-loopback bind that is not fronted by an
`https://` public URL — one with an `http://` public URL or with none at all
(a private network such as Tailscale, or a container-internal hop). Without
it, a routable bind (anything but loopback) is refused at startup unless the
public URL is `https://`: `refusing to serve plaintext http on <bind>: use an
https:// public URL, bind to 127.0.0.1 behind a TLS proxy, or pass
--allow-plaintext`. The compose setup does not need it: the hub binds
`0.0.0.0` on the compose network with the `https://` public URL Caddy
serves.

`fleet-hub serve` logs to stderr and, once the data dir is writable, also to
`<log-dir>` (or wherever `--log-dir`/`FLEET_HUB_LOG_DIR` points).

`--local-host` (default `false`, unlike the desktop where it is implicitly
`true`) controls whether the hub's own machine is itself a managed fleet
host: with it off, reconcile never creates or probes a `local` host, and a
single-host refresh of `local` returns `E_NOTFOUND`. With it off, any tool
or command naming host `local` returns `E_NOTFOUND` too, so nothing runs on
the hub's machine as a fleet host.

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

The desktop's `state.db` carries a `local` host row for the machine it ran
on. Since the hub defaults `hub.local_host` to `false`, that copied `local`
row is hidden and marked unreachable automatically on first start — not
deleted, just no longer listed, counted, probed or polled for usage.

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
until the host becomes a client of the hub itself (a future sub-project).

Run `provision_hosts` from only one of the two — the hub or the desktop —
for a given host. Running it from both leaves the host's hook block pointed
at whichever one provisioned it last.

## Security notes

- **Bearer tokens.** Same model as the desktop's Control API: a master token
  for the hub itself, a per-host token for every provisioned host (see
  `control-api.md` → *Per-host tokens*). Every request needs
  `Authorization: Bearer <token>`.
- **TLS.** The hub itself speaks plain HTTP; put TLS in front of it. The
  Docker setup does this with Caddy (automatic certificates via its domain,
  `deploy/hub/Caddyfile`); the bare-binary setup needs your own proxy (or a
  loopback bind reached over Tailscale/SSH, with no public URL at all).
- **`state.db` permissions.** Written `0600` on the hub's machine, same as
  the desktop.
- **`mcp.confirm_destructive`.** This desktop setting gates destructive
  tools (`broadcast_prompt`, `kill_session`, `delete_worktree`, …) behind a
  UI confirmation dialog. A hub has no UI to show that dialog to — leave the
  setting off (its default) on a hub; if it is on, a request needing
  confirmation is refused (`E_CONFIRM_REQUIRED`) with no way to approve it,
  and the hub logs a warning naming the tool and nonce. A `state.db` copied
  from a desktop can carry it switched on; `fleet-hub serve` logs a warning
  at startup when it is.
- **Rotating tokens.** `fleet-hub token regenerate` mints a fresh master
  token — reconfigure every client afterward. For host tokens, call
  `provision_hosts { rotate: true }` (from any client), which re-provisions
  every host with a new per-host token.

## Troubleshooting

- **`403` from the Host check** — the request's `Host` header isn't on the
  hub's allowlist (which always includes the public URL's own host). Add
  extra names with `--allowed-host`, or — if a proxy in front of the hub
  rewrites `Host` to something else — keep the Caddyfile's
  `header_up Host 127.0.0.1:4180` rewrite, which the loopback allowlist
  entry always accepts.
- **`401`** — wrong or missing token. Confirm the client sends
  `Authorization: Bearer <token>` with the exact current token
  (`fleet-hub token show`).
- **`refusing to serve plaintext http` at startup** — a routable `--bind`
  without an `https://` public URL (an `http://` one, or none at all). Use
  `https://` in front of a TLS proxy, bind `127.0.0.1`, or pass
  `--allow-plaintext` for a private-network or container-internal
  plaintext hop.
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
