# Hub daemon: a headless claude-fleet that needs no desktop app

**Date:** 2026-09-17
**Status:** Approved design, awaiting implementation plan
**Scope:** new workspace layout (`crates/fleet-core`, `crates/fleet-hub`,
`src-tauri`), a runtime-spawn seam in the core, configurable bind address and
Host allowlist for the control API, public-URL provisioning, an opt-out for
the implicit `local` host, Docker + systemd packaging, `docs/hub.md`, CI.
No frontend change. No new MCP tool. New `hub.*` keys in the `settings`
table; no migration.

## Why

Today the hub is the Tauri desktop app: it owns `state.db`, runs the
reconcile tick, holds every SSH connection, and serves the control API on
`127.0.0.1` on the laptop. Remote hosts reach it only through reverse SSH
tunnels, and nothing works while the laptop sleeps or the app is closed.

The goal is a fleet that runs without any local client, from any always-on
box, set up in a few commands, so that other clients (a mobile app first,
later the desktop app itself) can talk to one hub over HTTPS.

This is sub-project 1 of five:

1. **Hub daemon** (this spec).
2. **Client access** — per-client tokens with QR pairing, a server-sent
   events stream, `session_conversation` as an MCP tool, built-in ACME TLS.
3. **Mobile app** — Compose Multiplatform, Android + iOS, its own repo.
4. **Host agent** — an outbound-dialing per-host transport for NAT'd hosts.
5. **Desktop as hub client** — the Tauri app talks to a remote hub.

Hub-to-host transport stays **SSH from the hub** in this sub-project
(decided 2026-09-17; the agent transport is sub-project 4). The SSH seam is
narrow — `SshClient::{run, run_bounded, run_bounded_capped,
run_bounded_cancellable, run_cancellable, upload_file, remote_home}` plus
the `SshExec` trait the tests fake — so a second transport can be added
later without touching the service layer.

## Non-goals

- Client tokens, QR pairing, events stream, TLS termination inside the
  daemon (sub-project 2). Until then TLS comes from Caddy in the compose
  file or from the operator's own proxy / Tailscale.
- Any change to the desktop UI, the PTY, or the Tauri commands beyond
  `use` paths.
- The desktop app talking to a remote hub (sub-project 5).
- A per-host agent (sub-project 4).
- Migrating state automatically. Copying `state.db` is the migration.
- Running two hubs against the same hosts. A host's hook and MCP entries
  point at exactly one hub.

## Architecture

### Workspace

A `Cargo.toml` at the repository root declares a workspace with three
members. `src-tauri` keeps its package name (`claude-fleet`) and its
`[lib]` block so the Tauri CLI, `tauri.conf.json` and the release script
keep working.

```
Cargo.toml                    # [workspace] members = ["crates/*", "src-tauri"]
Cargo.lock                    # moves to the root (workspace lock)
crates/fleet-core/            # package fleet-core, lib only, NO tauri dependency
crates/fleet-hub/             # package fleet-hub, bin `fleet-hub`, depends on fleet-core
src-tauri/                    # package claude-fleet (Tauri app), depends on fleet-core
```

`fleet-core` receives, unchanged in content, every module of
`src-tauri/src` except the ones listed under *stays*:

| Moves to `fleet-core` | Stays in `src-tauri` |
|---|---|
| `cancel`, `claude_agents`, `claude_cli`, `events` (trait, `RowChange`, `NoopEventBus`, `RecordingEventBus`), `humanize`, `ipc_error`, `logging`, `mcp` (whole tree incl. `tools/`, `doc_gen` test), `projects`, `repo_url`, `service` (whole tree), `shell`, `ssh`, `ssh_config`, `ssh_fake` (test), `store` (whole tree), `tmux`, `validate`, `fleet_e2e_tests`, `no_eprintln_tests` | `commands/` (every Tauri command), `pty.rs`, `lib.rs` (bootstrap: env recovery, `tauri::Builder`, managed state), `main.rs`, `AppHandleEventBus` (moves out of `events.rs` into `src-tauri/src/app_events.rs`) |

`fleet-core` re-exports its modules at the crate root
(`pub mod service; pub mod store; …`). The Tauri crate adds
`use fleet_core::{…}` lines and command bodies change only their paths
(`crate::service::x` → `fleet_core::service::x`). `IpcError` keeps its name.

`pub(crate)` items that a Tauri command reaches today become `pub` in the
core; nothing else changes visibility.

The `no_eprintln_tests` guard (no `eprintln!` in the tree) and the
`reference_is_current` doc test move with their modules; `doc_gen` resolves
`docs/` relative to the workspace root (`CARGO_MANIFEST_DIR/../..`).

### Runtime-spawn seam

Five call sites spawn background work through `tauri::async_runtime::spawn`
because the Tauri `setup` closure runs on the main thread with no tokio
context: `service/hooks.rs` (2), `service/tick.rs` (2), `mcp/mod.rs` (1).

`fleet-core` adds `rt.rs`:

```rust
static INSTALLED: OnceLock<tokio::runtime::Handle> = OnceLock::new();

/// Install the runtime an embedder wants core tasks to run on. Idempotent;
/// a second call is ignored.
pub fn install(handle: tokio::runtime::Handle);

/// Spawn on the current tokio runtime when inside one, else on the
/// installed handle. Panics with an actionable message when neither exists.
pub fn spawn<F>(fut: F) -> tokio::task::JoinHandle<F::Output>
where F: Future + Send + 'static, F::Output: Send + 'static;
```

The five sites call `crate::rt::spawn`. The daemon runs under
`#[tokio::main]`, so `Handle::try_current()` always succeeds there. The
Tauri app calls `fleet_core::rt::install(h)` in `setup` with the tokio handle
from `tauri::async_runtime::handle()` (`RuntimeHandle::Tokio(h)`), before the
first tick or MCP start. The comment that explains *why* the Tauri runtime
was used moves onto `install`.

### Event bus

`EventBus` (trait), `RowChange`, `NoopEventBus` and `RecordingEventBus` stay
in `fleet-core::events`. `AppHandleEventBus` stays in the Tauri crate. The
daemon uses `NoopEventBus` in this sub-project; the broadcast bus for the
events stream is sub-project 2 and slots in behind the same trait.

### The `fleet-hub` binary

`clap`-based CLI. Subcommands:

| Command | Effect |
|---|---|
| `fleet-hub init` | Creates the data dir (0700), opens/creates `state.db` (0600), mints the master token if absent, writes `hub.*` settings from flags, prints the token **once** to stdout. Idempotent: re-running keeps the existing token unless `--regenerate-token`. |
| `fleet-hub serve` | Opens the store, installs nothing (tokio main), starts the reconcile tick, the account-usage tick, the tunnel supervisor (only when no public URL, see *Reachability*), and the control API (`/mcp` + `/hook`). Blocks until SIGTERM/SIGINT, then stops the server, stops tunnels, shuts SSH masters down (`SshClient::shutdown_all`). Exit code 0 on a clean stop, 1 on a start failure (bind error printed). |
| `fleet-hub token show` / `token regenerate` | Prints / rotates the master token (same semantics as the Settings buttons). |
| `fleet-hub ssh-key` | Prints the hub user's `~/.ssh/id_ed25519.pub`, generating the key pair with `ssh-keygen -t ed25519 -N ""` when absent. This is the per-host setup step: paste into the host's `authorized_keys`. |

Configuration precedence: flag > `FLEET_HUB_<NAME>` env > `settings` row >
default. Every option has all three spellings:

| Flag | Env | Setting | Default |
|---|---|---|---|
| `--data-dir` | `FLEET_HUB_DATA_DIR` | — | `$XDG_DATA_HOME/claude-fleet` (Linux), platform appdata dir otherwise; `/var/lib/fleet-hub` in the image |
| `--bind` | `FLEET_HUB_BIND` | `hub.bind` | `127.0.0.1` |
| `--port` | `FLEET_HUB_PORT` | `mcp.port` | `4180` |
| `--public-url` | `FLEET_HUB_PUBLIC_URL` | `hub.public_url` | unset |
| `--allowed-host` (repeatable) | `FLEET_HUB_ALLOWED_HOSTS` (comma) | `hub.allowed_hosts` | loopback + public URL host |
| `--local-host` / `--no-local-host` | `FLEET_HUB_LOCAL_HOST` | `hub.local_host` | `false` in the daemon, `true` in the desktop |
| `--log-dir` | `FLEET_HUB_LOG_DIR` | — | `<data-dir>/logs` (same `logging::init`) |
| `--allow-plaintext` | `FLEET_HUB_ALLOW_PLAINTEXT` | — | off; permits a non-loopback bind with an `http://` public URL (container-internal use only) |

`serve` persists the resolved `hub.*` values into `settings` so that MCP
tools reading settings (e.g. provisioning) see the same answer the process
runs with. `mcp.enabled` is forced to `true` by `serve`; the daemon has no
"off" state — stop the process.

The data dir is the same layout the desktop uses (`state.db`, `logs/`), and
the schema is the shared `store` MIGRATIONS, so **copying a desktop
`state.db` into the daemon's data dir is the migration path**. Hosts,
projects, sessions, tags, friendly names, history and host tokens carry
over; the `local` row is ignored when `hub.local_host` is false (see below).

### Reachability

Three changes in `fleet-core::mcp`:

1. **Bind address.** `mcp::start` takes `bind: IpAddr` instead of hard-coding
   `Ipv4Addr::LOCALHOST`. The desktop always passes loopback (the invariant
   comment moves to the desktop call site). The daemon passes `hub.bind`.
2. **Host / Origin allowlist.** `AuthState` gains `allowed_hosts:
   Arc<Vec<String>>` (lower-cased host names with optional `:port`). The
   middleware keeps its current rule — loopback `Host`/`Origin` is always
   accepted, a missing `Origin` is fine — and additionally accepts a `Host`
   or `Origin` whose host part is in the list. Anything else stays `403`
   before the token is checked. The desktop passes an empty list (no change
   in behaviour). The daemon passes `hub.allowed_hosts`, which defaults to the
   public URL's host (with and without its port).
3. **Base URL for provisioning.** `hooks_install::hook_url(port)` and the
   `mcpServers.claude-fleet.url` builder in `service::provision` take a base
   URL string instead of a port: `http://127.0.0.1:<port>` when
   `hub.public_url` is unset (today's behaviour), else the public URL with
   any trailing `/` stripped. `provision_hosts` skips step 6 (reverse tunnel)
   when a public URL is set, and the tunnel supervisor is not started by
   `serve` in that case. `disable`/re-enable semantics in the desktop are
   unchanged.

Without a public URL the daemon behaves exactly like the desktop hub —
loopback hooks over reverse tunnels — so a deployment that reaches the
daemon only over Tailscale still works.

Security posture stays: bearer token on every request (master or per-host),
constant-time comparison, tokens never logged, `state.db` 0600. Exposing the
daemon on `0.0.0.0` without TLS is refused: `serve` exits with an error when
`bind` is non-loopback, `public_url` is `http://`, and `--allow-plaintext`
is not given. The compose file terminates TLS in Caddy and passes the
upstream `Host` header rewritten to `127.0.0.1:4180`, so the allowlist can
stay at its default.

### No implicit `local` host

`reconcile_sessions_with` upserts the `local` row on every pass and probes
the local Claude account. `ReconcileDeps` gains `local_host: bool`; when
false, both the upsert and `sync_local_account` are skipped and a pre-existing
`local` row is left as it is but treated like any other host that is
`hidden` (never probed, never listed as reachable). `spawn_reconcile_tick`
and `reconcile_now` read the flag from `hub.local_host` once per pass. The
desktop leaves the setting unset, which resolves to `true`.

`list_hosts`/`fleet_health` therefore report no `local` host from the daemon.

### Local tool dependencies on the hub

Two host operations run a process on the hub machine rather than over SSH,
both gated on the `local` host: `list_github_repos_with` runs `gh` for
`local`, and worktree adoption runs `git -C <local path>`. With
`hub.local_host = false` neither fires. The image still ships `git` and
`openssh-client` (the SSH transport is the system `ssh` binary; `scp`
backs `upload_file`), and no `gh`.

### Packaging

- `crates/fleet-hub/Dockerfile`: multi-stage; builder `rust:1-bookworm`
  builds `fleet-hub` only (`cargo build -p fleet-hub --release`); runtime
  `debian:bookworm-slim` + `openssh-client git ca-certificates tini`, user
  `fleet` (uid 1000), `VOLUME /var/lib/fleet-hub /home/fleet/.ssh`,
  `ENTRYPOINT ["tini","--","fleet-hub"]`, `CMD ["serve"]`. Image
  `ghcr.io/martin-janci/fleet-hub`, built by a new `hub-image.yml` workflow
  on tags and on `workflow_dispatch`.
- `deploy/hub/docker-compose.yml`: `fleet-hub` + `caddy` with a two-line
  `Caddyfile` (`reverse_proxy fleet-hub:4180 { header_up Host 127.0.0.1:4180 }`),
  env file with `FLEET_HUB_PUBLIC_URL`, `FLEET_HUB_BIND=0.0.0.0`,
  `FLEET_HUB_ALLOW_PLAINTEXT=1` (plaintext is container-internal only).
- `deploy/hub/fleet-hub.service`: systemd unit for a bare binary
  (`ExecStart=fleet-hub serve`, `EnvironmentFile=/etc/fleet-hub.env`,
  `DynamicUser=no`, `StateDirectory=fleet-hub`).
- `docs/hub.md`: setup in this order — run the container, `fleet-hub
  ssh-key`, add the key to each host, `add_host` (via any MCP client or the
  `discover_hosts` from a mounted `~/.ssh/config`), `provision_hosts`,
  connect a client with the master token. A *Coexistence with the desktop*
  section states that a host provisioned by the hub reports hooks to the hub
  only; a desktop app still lists that host's sessions through reconcile but
  loses hook-driven `idle`/`working` and task completion until sub-project 5.
  A *Migrating from the desktop* section: stop the app, copy `state.db`,
  start the daemon, re-provision.

### CI and scripts

- `ci.yml` rust job: `working-directory: .`, `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test
  --workspace`; `cargo deny --manifest-path Cargo.toml check` (`deny.toml` already lives at
  the repository root, so only the manifest path changes).
- New `hub-headless` job: `ubuntu-latest` **without** installing the Tauri
  apt packages, `cargo build -p fleet-hub --locked`. This is the regression
  guard for the split: any Tauri dependency creeping into the core fails it.
- `scripts/ci-local.sh`: same command changes; `--rust-only` also runs the
  headless build when the Tauri libs are missing instead of failing.
- `scripts/release.sh`: `VERSION_FILES` unchanged (`src-tauri/Cargo.toml`
  is still the desktop's version); `crates/fleet-hub/Cargo.toml` gets the
  same bump and `Cargo.lock` path becomes the root lock. `fleet-core` stays
  at `0.1.0`-style independent versioning; only binaries carry the release
  version.
- `CLAUDE.md` build section: new commands, the workspace map, and the
  `hub-headless` note. `REGEN_DOCS` invocation path updates to
  `--manifest-path crates/fleet-core/Cargo.toml`.

## Data flow

```
phone / laptop client ──HTTPS──▶ Caddy ──▶ fleet-hub :4180 /mcp
                                              │
                        hooks (Stop, …) ◀──── │ ──ssh──▶ host A (tmux, claude)
   host A ──HTTPS /hook──▶ Caddy ──▶ fleet-hub │ ──ssh──▶ host B
                                              └──▶ state.db (data dir)
```

Provisioning writes `https://fleet.example.com/hook` and
`https://fleet.example.com/mcp` into each host, with that host's token.

## Error handling

- `serve` bind failure → printed, exit 1 (no retry loop; the supervisor
  restarts the unit/container).
- Non-loopback bind + `http://` public URL without `--allow-plaintext` →
  refused at startup with the reason.
- A public URL that fails to parse, or whose scheme is not `http`/`https`,
  is rejected by `init`/`serve` (`E_VALIDATE` text, exit 2).
- Store open failure → same actionable message the desktop prints (corrupt
  DB hint), exit 1.
- SIGTERM during a reconcile pass: the pass finishes (bounded by its own
  wall clocks), then the loop exits; the MCP server stops accepting new
  connections immediately and drains in-flight calls (existing graceful
  shutdown).

## Testing

Unit (all in `fleet-core` unless noted):

- `rt::spawn` runs on the current runtime inside tokio; uses the installed
  handle from a plain thread; panics with the documented message with
  neither.
- `authorize`: loopback still accepted; allowlisted `Host` accepted;
  allowlisted `Origin` accepted; non-listed host `403`; port-qualified and
  bare forms both match; comparison is case-insensitive.
- `hook_url(base)` / MCP entry URL for a loopback base and for a public
  base with and without a trailing slash.
- `provision_hosts` with a public URL: no tunnel step, entries carry the
  public URL; without: unchanged (existing tests).
- Reconcile with `local_host: false`: no `local` upsert, no local account
  probe, other hosts reconciled (fake SSH).
- Config precedence (`fleet-hub`): flag > env > setting > default; plaintext
  refusal; URL validation.
- `mcp::start` with a non-loopback bind actually listens there (bind
  `0.0.0.0:0`, connect via `127.0.0.1`).

CI: the `hub-headless` job (above) is the acceptance test for the crate
split.

Manual acceptance (recorded in the PR):

1. On hetzner: `docker compose up -d` with the public URL set; `fleet-hub
   init` printed a token.
2. `fleet-hub ssh-key`, key added to `claude-fleet-trn` and
   `claude-fleet-oci`; `add_host` + `provision_hosts` succeeded (status
   `provisioned`, no tunnel).
3. With the desktop app **closed**: from a phone browser or curl over the
   public HTTPS URL, `fleet_health`, `list_sessions`, `send_prompt` to a trn
   session, `wait_for_session`, `session_transcript` all succeed with the
   master token; a wrong token gets `401`; a request with a foreign `Host`
   gets `403`.
4. The session's `claude_status` flips `working` → `idle` via hooks
   (`session_history` shows the `Stop`).
5. `docker compose restart` keeps the token, hosts and sessions.

## Files

New: `Cargo.toml` (root), `crates/fleet-core/{Cargo.toml,src/lib.rs,src/rt.rs}`,
`crates/fleet-hub/{Cargo.toml,src/main.rs,src/cli.rs,src/config.rs,Dockerfile}`,
`deploy/hub/{docker-compose.yml,Caddyfile,fleet-hub.env.example,fleet-hub.service}`,
`.github/workflows/hub-image.yml`, `docs/hub.md`.

Moved (git mv, content unchanged apart from paths): every module in the
*moves* column above.

Changed: `src-tauri/{Cargo.toml,src/lib.rs,src/commands/*.rs,src/pty.rs}`
(paths, `rt::install`), `fleet-core::mcp::{mod.rs,auth.rs}` (bind,
allowlist), `service::{hooks_install.rs,provision.rs}` (base URL, tunnel
skip), `service::sessions::reconcile.rs` + `service::tick.rs` (`local_host`),
`service::{hooks.rs,tick.rs}` + `mcp/mod.rs` (`rt::spawn`),
`.github/workflows/ci.yml`, `scripts/{ci-local.sh,release.sh}`, `CLAUDE.md`,
`docs/{control-api.md,concepts.md,RELEASING.md}` (hub mentions).

## Open points settled here

- **Crate names**: `fleet-core`, `fleet-hub` (binary `fleet-hub`, image
  `ghcr.io/martin-janci/fleet-hub`). The desktop package stays
  `claude-fleet`.
- **Where the workspace root lives**: repository root, so `cargo` commands
  run from the top and the Tauri CLI keeps `src-tauri` as its project dir.
- **Two hubs**: not supported; documented.
- **`local` in the daemon**: off by default, switchable for someone who runs
  the bare binary on a box that also runs Claude.
