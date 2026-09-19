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
3. **Install the agent, on the host.** Paste the token into stdin rather
   than the command line. `--token` also works, but it shows up in the
   process list and in your shell history.
   ```bash
   # a system unit (the default), run as the user whose sessions it drives:
   sudo fleet-agent install --hub https://fleet.example.com --token-file -
   # or a user unit, no root needed:
   fleet-agent install --user --hub https://fleet.example.com --token-file -
   ```
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
4. **Check it,** on the host with `fleet-agent status [--user]`, or from any
   client with `agent_status`. `fleet-agent status` exits `0` when the
   service is running and connected, and `3` otherwise. It reads the
   agent's own report through `systemctl show`; the agent keeps no state
   file.
5. **Provision it:** `probe_host { alias: "laptop" }` (or wait for the next
   reconcile pass), then `provision_hosts`. Provisioning runs over the
   agent, exactly as it would over SSH.

Without systemd, `fleet-agent run --config <path>` (or
`run --hub <url> --token-file -`) runs the same loop in the foreground.

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
  that along. The desktop does the same thing through
  `set_host_token_mode`. A token that `fleet-hub agent-token` mints for a
  host that had none is `full`, and the command warns on stderr when a
  token is not.

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
  `remove_host`, `hide_host`, `apply_sync`, `set_secret`, `set_host_layers`,
  `pair_client`, `revoke_client` and `list_clients` are master-token only, so
  a paired phone can neither re-provision the fleet nor pair a second device
  nor revoke your own client — nor even enumerate the other devices you have
  paired.
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

## Point a desktop at the hub

Settings → **Hub**. On the hub, mint a code and paste it:

```bash
fleet-hub pair --name laptop     # prints a code; it dies on first use
```

The desktop pairs as an ordinary client — the hub cannot tell it from a phone
and should not. It stores the client token in the OS keychain (macOS) or an
owner-only 0600 file (elsewhere), never in `state.db` and never in a log
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
- the URL is plain `http://` to a host that is not loopback, and
  `hub.allow_plaintext` is not set;
- `hub.remote_url` does not parse;
- the settings cannot be read, but a client token is stored, which proves the
  app was paired.

In that state it runs no reconcile tick, no account-usage poll, no embedded
control API and no event stream. Every fleet command is refused with
`E_HUB_UNAVAILABLE` and the reason. A red banner at the top of the window
names the hub and the reason, with a button to Settings → Hub. There you can
pair again, or Disconnect to go back to standalone. Either takes effect at the
next launch.

Falling back to standalone would be the dangerous choice. You pointed the app
at a hub, so the fleet is the hub's. An app that quietly started reconciling
it again would be a second brain for the same hosts, and that is the failure
this mode exists to prevent. With no `hub.remote_url` at all, the app is
standalone exactly as before.

### What is different from standalone

- **Live, from the hub.** The desktop follows the hub's `GET /events` and
  re-emits every change as the same frontend event a local change would have
  produced, so the window updates itself. When that stream drops it reconnects
  with backoff, re-lists sessions, hosts, tasks and accounts once, and shows a
  banner — "what you see may be out of date", the attempt number and the
  reason — until it is back. A stream that goes silent (not even the hub's
  15-second keep-alive) for about 40 seconds is treated as dead, which is what
  a laptop that slept and woke on another network looks like.
- **The fleet is the hub's.** No reconcile tick, no account-usage poll and no
  embedded control API in the desktop; two brains for one fleet is the failure
  this mode exists to prevent. The footer's version, database and schema are
  the hub's too — the badge beside them says whose.
- **A prompt sent from the desktop reaches the agent marked *untrusted*,**
  exactly as one typed on a phone does. `apply_marker` refuses `raw=true` to
  any non-master caller and a paired client is never the master. Correct
  behaviour, and the one behavioural difference in the common path.
- **Destructive confirmations are answered on the hub.** With
  `mcp.confirm_destructive` on, `kill_session`, `delete_worktree`,
  `move_session` and `cancel_task` come back `E_CONFIRM_REQUIRED`. The
  desktop's confirmation dialog answers *its own* queue, which is empty in
  this mode. Approve it on the hub — this window will follow: the approved
  change arrives over the hub's event stream like any other, so there is
  nothing to refresh.
- **Fleet administration is refused.** Adding, removing and hiding hosts,
  provisioning, per-host tokens, asset sync and sync secrets: a client is not
  the fleet's administrator. Those controls are disabled in the interface with
  the reason rather than failing at the click.
- **The terminal is local-only.** The PTY attaches a local `ssh`/`tmux`
  process and the hub streams no pane. The terminal tab shows the
  `ssh <host>` / `tmux attach -t <session>` line for the selected session
  instead of a dead pane.
- **The asset catalog and the setup checklist** are about the machine that
  owns the fleet, so they show the reason instead of their panels. (The hub
  does serve the catalog's asset list, `list_assets`, to any paired client;
  what it does not serve is the configuration and git checkout the Assets
  panel is built on.)
- **A revoked or rotated token** comes back `E_UNAUTHORIZED` on every call;
  the error says to pair again in Settings → Hub.

### Parity or refusal

A desktop mutation is routed to the hub **only where the desktop's arguments
map one-to-one onto the tool's parameters**, checked field by field. Where
they do not, the command *refuses* instead of routing.

`new_session` set the rule. `NewSessionArgs` carries `kind`, `start_command`
and `friendly_name`; the tool's `NewSessionParams` carries none of them, and a
shell session is a different tool entirely. Routing it would have
**succeeded** while silently dropping the label the user typed. A refusal is
visible; a dropped field is not. `repair_session` is refused for the same
reason: the tool always runs the *explicit* repair, and the desktop's
automatic pre-attach check has no counterpart.

Do not "fix" one of these refusals by wiring a lossy mapping. If a tool grows
the missing parameters, route it then.

### Known limitations

- **The terminal works over SSH only.** A hub client has no in-app terminal;
  attach from a shell with the command the terminal tab shows.
- **Projects and worktrees are not re-listed on reconnect**, because their
  list tools answer a different shape from their events. They refresh when
  the window regains focus.
- **Not yet run as an app.** At the time of writing this mode is verified by
  its test suites only: the desktop has not been launched against a real hub,
  and the macOS keychain path has not been compiled or run. Report anything
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
  Only its SHA-256 is stored. What crosses the room in the QR is a
  single-use, minutes-long pairing *code* in a URL fragment — not a token —
  and `POST /pair`, the one unauthenticated route besides `/healthz`, is
  rate-limited to one attempt per address every six seconds. See *Pair a
  phone* and *Clients* above.
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
