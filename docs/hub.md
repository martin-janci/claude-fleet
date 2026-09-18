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

`/healthz` is the one route that needs no bearer token and no `Host`
allowlist entry, because it reveals nothing: it never opens `state.db` and
never names a version, a host, a session or a setting — the fixed body only
means "this process is accepting HTTP". Every other route stays behind the
token. Probes therefore leave no rejected-request lines in the log.

The check does not open `state.db` and does not read the stored `mcp.port`:
if you run the hub on another port, set it with `FLEET_HUB_PORT`, not only
`--port`.

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

Two options:

```bash
fleet-hub pair --name kiosk --mode readonly   # observe only; the default is full
fleet-hub pair --name phone --ttl 120         # seconds the code stays valid (30–3600)
```

Pairing needs a **running** hub (`fleet-hub serve`): the code only means
something inside the process that will redeem it. `fleet-hub pair` reads the
master token out of the data dir and calls the hub's own `/mcp` on loopback,
so run it on the hub's machine, as the user the daemon runs as.

## Clients

```bash
fleet-hub client list
fleet-hub client list --include-revoked
fleet-hub client revoke phone
```

`client list` prints one line per client, newest first:

```
NAME   MODE      CREATED            LAST SEEN          REVOKED
phone  full      2026-09-17 09:20Z  2026-09-18 07:41Z  -
kiosk  readonly  2026-09-17 09:12Z  -                  -
```

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
- **Neither mode reaches fleet admin.** `provision_hosts`, `add_host`,
  `remove_host`, `hide_host`, `apply_sync`, `set_secret`, `pair_client` and
  `revoke_client` are master-token only, so a paired phone can neither
  re-provision the fleet nor pair a second device nor revoke your own client.
- A prompt typed on a phone always reaches an agent **marked** as untrusted
  input, naming the client it came from. `raw: true` is the master token's
  alone.

`revoke` takes effect on the client's very next request — the auth layer only
resolves live rows — and an open event stream ends within one heartbeat
(15 s). The row is kept, revoked, for the audit trail, and the name becomes
free to pair again:

```
revoked phone (paired 2026-09-17 09:12Z); its next request is refused and the name is free again
```

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

The stream sits behind the same bearer token as `/mcp` (a change stream names
sessions, hosts, projects and prompts), and a caller may hold eight of them at
once. A subscriber that falls far enough behind gets one `lagged` frame and
the stream closes — reconnect and re-list rather than assume continuity.

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

`init` and `serve` open `<data-dir>/state.db`, creating it when missing.
`token show` and `token regenerate` never create one: pointed at the wrong
data dir (or run as a user who cannot see it) they exit 1 with
`no hub database at <data-dir>/state.db; run fleet-hub init first (or pass
--data-dir)` instead of minting a token nothing uses.

Put it behind your own TLS-terminating proxy (the same role Caddy plays in
the Docker setup) and set `FLEET_HUB_PUBLIC_URL` — or let the hub terminate
TLS itself with `--tls cert`, which needs no proxy at all (see *Single binary
with its own certificate* below). Or skip the public URL
entirely and bind loopback, reaching it over Tailscale or an SSH tunnel of
your own: with no public URL configured, the hub behaves exactly like the
desktop app — it binds `127.0.0.1` and opens a reverse SSH tunnel to every
provisioned remote host. If you instead bind a non-loopback address with no
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
| `--allow-plaintext` | `FLEET_HUB_ALLOW_PLAINTEXT` (`1`/`true` or `0`/`false`) | `hub.allow_plaintext` | off |
| `--log-dir` | `FLEET_HUB_LOG_DIR` | — | `<data-dir>/logs` |
| `--tls off\|auto\|cert` | `FLEET_HUB_TLS` | `hub.tls` | `off` (`auto` is refused — see above) |
| `--tls-cert` | `FLEET_HUB_TLS_CERT` | `hub.tls_cert` | unset (required by `--tls cert`) |
| `--tls-key` | `FLEET_HUB_TLS_KEY` | `hub.tls_key` | unset (required by `--tls cert`) |

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
- **Client tokens.** A paired device holds a third kind of token: named,
  revocable, `full` or `readonly`, never the master and never fleet admin.
  Only its SHA-256 is stored. What crosses the room in the QR is a
  single-use, minutes-long pairing *code* in a URL fragment — not a token —
  and `POST /pair`, the one unauthenticated route besides `/healthz`, is
  rate-limited to one attempt per address every six seconds. See *Pair a
  phone* and *Clients* above.
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
- **`no hub is answering on 127.0.0.1:<port> — start fleet-hub serve first`**
  from `pair` / `client list` / `client revoke` — these three drive the
  *running* hub, not the database. Start the daemon, and point the command at
  the same data dir and port it runs with (`--data-dir`, `--port`, or the
  `FLEET_HUB_*` env the unit sets).
- **The phone says the pairing code is invalid** — a code is single-use, it
  expires (10 minutes by default), and a hub restart voids every outstanding
  one. Mint a fresh one with `fleet-hub pair`. A `429` instead means the
  address has spent its attempt budget: one every six seconds.
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
