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
mkdir -p ~/fleet-hub/ssh && cd ~/fleet-hub
curl -O https://raw.githubusercontent.com/martin-janci/claude-fleet/main/deploy/hub/docker-compose.yml
curl -O https://raw.githubusercontent.com/martin-janci/claude-fleet/main/deploy/hub/Caddyfile
curl -O https://raw.githubusercontent.com/martin-janci/claude-fleet/main/deploy/hub/fleet-hub.env.example
cp fleet-hub.env.example fleet-hub.env
# edit fleet-hub.env: set FLEET_HUB_DOMAIN and FLEET_HUB_PUBLIC_URL to your
# own domain (both point DNS at this machine; Caddy gets a cert automatically)
```

Mint the master token and the hub's SSH key before starting the daemon
properly (`docker compose run --rm` runs a one-off container against the
same named volumes the long-running services will use):

```bash
docker compose run --rm fleet-hub init          # prints the master token — save it
docker compose run --rm fleet-hub ssh-key       # prints the hub's SSH public key
```

Add the printed public key to `~/.ssh/authorized_keys` on every host you want
the hub to manage. Then create `./ssh/config` (bind-mounted at
`/home/fleet/.ssh/config` in the container) with one `Host` block per
machine, the same shape as `~/.ssh/config` for the desktop app:

```
Host devbox
    HostName 10.0.0.12
    User martin
```

The file must be owned by uid 1000 (the container's `fleet` user) and
readable by it — mode `0600` is the simplest way to guarantee that:

```bash
chmod 600 ./ssh/config
sudo chown 1000:1000 ./ssh/config
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

Create `/etc/fleet-hub.env` (same keys as `deploy/hub/fleet-hub.env.example`,
plus whatever else you want set — see *Configuration* below), then install
the unit:

```bash
sudo cp deploy/hub/fleet-hub.service /etc/systemd/system/
sudo useradd --system --create-home --shell /usr/sbin/nologin fleet   # if it doesn't exist yet
sudo systemctl daemon-reload
sudo systemctl enable --now fleet-hub
```

Put it behind your own TLS-terminating proxy (the same role Caddy plays in
the Docker setup) and set `FLEET_HUB_PUBLIC_URL`. Or skip the public URL
entirely and bind loopback, reaching it over Tailscale or an SSH tunnel of
your own: with no public URL configured, the hub behaves exactly like the
desktop app — it binds `127.0.0.1` and opens a reverse SSH tunnel to every
provisioned remote host.

## Configuration

`fleet-hub --help`, `fleet-hub serve --help` and `fleet-hub init --help` all
list these flags (every one is `global`, so it works both before and after a
subcommand — `fleet-hub token show --data-dir D` and
`fleet-hub token --data-dir D show` are equivalent). Precedence is
**flag > env > `hub.*` setting in state.db > default**.

| Flag | Env | Setting | Default |
|---|---|---|---|
| `--data-dir` | `FLEET_HUB_DATA_DIR` | — | platform data dir: `~/.local/share/claude-fleet` on Linux, `/var/lib/fleet-hub` in the Docker image |
| `--bind` | `FLEET_HUB_BIND` | `hub.bind` | `127.0.0.1` |
| `--port` | `FLEET_HUB_PORT` | `mcp.port` | `4180` |
| `--public-url` | `FLEET_HUB_PUBLIC_URL` | `hub.public_url` | unset (loopback + reverse tunnels) |
| `--allowed-host` (repeatable) | `FLEET_HUB_ALLOWED_HOSTS` (comma-separated) | `hub.allowed_hosts` | none — the public URL's own host is always accepted in addition to this list |
| `--local-host true\|false` | `FLEET_HUB_LOCAL_HOST` | `hub.local_host` | `false` |
| `--allow-plaintext` | `FLEET_HUB_ALLOW_PLAINTEXT` | — | off |
| `--log-dir` | `FLEET_HUB_LOG_DIR` | — | `<data-dir>/logs` |

`--allow-plaintext` permits a non-loopback bind with an `http://` public URL
(container-internal use only, e.g. between `fleet-hub` and `caddy` on the
compose network — see the compose file). Without it, a routable bind
(anything but `127.0.0.1`) combined with a plaintext public URL is refused
at startup: `refusing to serve plaintext http on <bind>: use an https://
public URL, bind to 127.0.0.1 behind a TLS proxy, or pass --allow-plaintext`.

`fleet-hub serve` logs to stderr and, once the data dir is writable, also to
`<log-dir>` (or wherever `--log-dir`/`FLEET_HUB_LOG_DIR` points).

`--local-host` (default `false`, unlike the desktop where it is implicitly
`true`) controls whether the hub's own machine is itself a managed fleet
host: with it off, reconcile never creates or probes a `local` host, and a
single-host refresh of `local` returns `E_NOTFOUND`.

## Migrating from the desktop

1. Quit the desktop app.
2. Copy its `state.db` into the hub's data dir:
   - macOS source: `~/Library/Application Support/sk.rlt.claude-fleet/state.db`
   - Linux source: `~/.local/share/claude-fleet/state.db`
3. Run `fleet-hub init --data-dir <hub-data-dir>` and start the hub.
4. Run `provision_hosts` again — the hub's URLs (and likely its port) differ
   from the desktop's loopback ones, so every host needs its hook block and
   `mcpServers` entry rewritten.

The desktop's `state.db` carries a `local` host row for the machine it ran
on. Since the hub defaults `hub.local_host` to `false`, that copied `local`
row is hidden automatically on first start — not deleted, just no longer
listed or probed.

> **WARNING:** on macOS, a bare `fleet-hub` with no `--data-dir` uses
> `~/Library/Application Support/sk.rlt.claude-fleet/` — the **same** data
> directory the desktop app uses (the hub deliberately shares the desktop's
> app id so a copied `state.db` lands where the desktop expects it). If you
> run the hub on a Mac that also runs the desktop app, always pass
> `--data-dir` (or set `FLEET_HUB_DATA_DIR`) to a directory of its own, or
> the hub will open and mutate the desktop's live database: enabling the
> control API and hiding its `local` host out from under it.

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
  and the hub logs a warning naming the tool and nonce.
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
  paired with an `http://` public URL. Use `https://` in front of a TLS
  proxy, bind `127.0.0.1`, or pass `--allow-plaintext` for a
  container-internal plaintext hop.
- **`could not bind`** — another process already holds the configured
  `--bind`/`--port`. Pick a different port, or find and stop what's using
  it.
- **A host shows `skipped: unreachable` from `provision_hosts`** — the
  hub's SSH key (`fleet-hub ssh-key`) is not in that host's
  `~/.ssh/authorized_keys`, or `./ssh/config` (bare binary: `~/.ssh/config`
  as the `fleet` user) is missing or unreadable by uid 1000. In Docker,
  confirm `./ssh/config` is owned by uid 1000 (`sudo chown 1000:1000
  ./ssh/config`) and has the right `Host` alias for the target.
- **Hooks never arrive (status stays stale, `safe_kill_session` never
  finalizes)** — the host cannot reach the hub's public URL. From the host
  itself, run `curl -sI https://fleet.example.com/mcp` and confirm it
  connects; check DNS, firewalls, and that Caddy has a valid certificate.
