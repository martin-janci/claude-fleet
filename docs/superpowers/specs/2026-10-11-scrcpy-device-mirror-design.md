# Device mirror (scrcpy) over SSH and the hub — design

Status: approved in brainstorming 2026-10-11; not implemented.
Area: Debug devices (`docs/debug-devices.md`, *Not built yet* → "Live screen
streaming and input").

## Goal

From the desktop's Debug devices page, open a **live, interactive mirror** of
an Android device (phone or emulator) attached to *any* fleet host, using a
native `scrcpy` window on the machine running the desktop. The device's host
is reached:

1. **locally** — the device is on this machine;
2. **over SSH** — this machine has an SSH route to the host;
3. **through the hub** — the hub relays the connection, to an SSH host *or*
   to an agent host (one that dials the hub and that nothing can dial).

Success: on the Debug devices page, an Android row in state `device` has a
**Mirror** button; clicking it opens a scrcpy window with video and input
within a few seconds, on each of the three routes; closing the window
cleans everything up.

## Non-goals (v1)

- iOS (scrcpy is Android-only); iOS keeps screenshots.
- A viewer embedded in the app (WebCodecs) or in fleet-mobile.
- The *adb bridge for native tools* (Android Studio / gradle on host B using
  host A's devices). The relay built here is its transport, so it becomes a
  small follow-up, but it is not in this scope.
- A terminal for agent hosts. Same remark: the agent stream frames make it
  possible later.
- Audio tuning, recording, multiple displays — whatever scrcpy does by
  default is what the user gets.

## Background: how scrcpy reaches a remote adb server

scrcpy (≥ 2.0) drives the *local* `adb` client, which honours
`ADB_SERVER_SOCKET=tcp:<host>:<port>`. With `--force-adb-forward`, scrcpy runs
`adb forward tcp:<P> localabstract:scrcpy_<scid>` — executed by the **remote**
adb server, so `<P>` listens on the remote host's `127.0.0.1` — and then the
scrcpy client connects to `--tunnel-host`:`--tunnel-port`. So a remote mirror
needs exactly two TCP endpoints on the device host's loopback:

| remote endpoint        | used by                              | local side          |
|------------------------|--------------------------------------|---------------------|
| `127.0.0.1:5037`       | adb client → adb server              | `ADB_SERVER_SOCKET=tcp:127.0.0.1:<L1>` |
| `127.0.0.1:<P>`        | scrcpy client → scrcpy-server socket | `--tunnel-host 127.0.0.1 --tunnel-port <L2>` |

`<P>` comes from scrcpy's `--port` (default range `27183:27199`).

**Version trap.** The local adb client and the remote adb server must speak
the same adb protocol version. On a mismatch the local client sends
`host:kill` to the server it is pointed at and starts its own — over a relay
that would **kill the remote host's adb server**, cutting off every other
session using it. The mirror therefore checks versions before running any
adb client against a relayed socket (see *Preflight*).

## Architecture

```
 desktop machine                                  device host
 ┌───────────────────────────────┐
 │ scrcpy ──adb──▶ 127.0.0.1:L1 ─┼─┐
 │ scrcpy ───────▶ 127.0.0.1:L2 ─┼─┤  Relay listener (fleet-core)
 │                               │ │   per accepted connection:
 │                               │ └─▶ Dialer::dial(host, port)
 └───────────────────────────────┘        │
        ┌─────────────────────────────────┼──────────────────────────┐
        │ SshDialer: ssh -W 127.0.0.1:port <alias>  ──────────────▶  │ adb server :5037
        │ HubDialer: wss://hub/relay?host&port                       │ scrcpy fwd :P
        │      hub ─▶ SSH host:   ssh -W 127.0.0.1:port <alias>  ──▶ │
        │      hub ─▶ agent host: TcpOpen/TcpData frames ─▶ agent ─▶ │ (agent dials 127.0.0.1:port)
        └────────────────────────────────────────────────────────────┘
```

One abstraction — a local TCP listener whose connections are spliced to a
`Dialer` — serves all remote routes. The local route uses no relay at all.

### Units

| unit | crate / path | purpose |
|------|--------------|---------|
| `relay` | `crates/fleet-core/src/relay/` | `Dialer` trait, `RelayListener`, the port allowlist, `SshDialer`, `HubDialer`, byte splicing |
| `device_mirror` | `crates/fleet-core/src/service/device_mirror/` | route choice, preflight, scrcpy spawn, mirror registry, teardown |
| `/relay` route | `crates/fleet-core/src/mcp/relay_route.rs` (+ wiring in `mcp/mod.rs`, next to `/agent`) | hub side: authorise, dial SSH host or agent, splice WebSocket ↔ stream |
| agent stream frames | `crates/fleet-proto/src/lib.rs`, `crates/fleet-agent/src/conn.rs` (+ a `tcp.rs`) | `Tcp*` frames, credit window, loopback dial on the agent |
| hub-side agent streams | `crates/fleet-core/src/agent/` | open/route/close `Tcp*` streams on a connected agent |
| Tauri commands | `src-tauri/src/commands/device_mirror.rs` | `start_device_mirror`, `stop_device_mirror`, `list_device_mirrors` |
| UI | Debug devices page (`debug_device` resource + its Svelte view) | Mirror / Stop buttons, route badge, errors |

`relay` and `device_mirror` are Tauri-free (they live in fleet-core) so the
same code can later serve the adb bridge and fleet-mobile's hub.

## 1. Relay core (`fleet-core::relay`)

```rust
pub trait Dialer: Send + Sync {
    /// Open one bidirectional byte stream to 127.0.0.1:<port> on `host`.
    async fn dial(&self, host: &str, port: u16) -> Result<BoxedStream, IpcError>;
}
```

- **Allowlist** (`relay::allowed_port`): `5037` and `27183..=27199`. Every
  dialer and the hub route check it; nothing else is ever dialled. The
  target address is always `127.0.0.1`, never caller-supplied.
- **`RelayListener`** binds `127.0.0.1:0` (loopback only), accepts, dials,
  and splices with `tokio::io::copy_bidirectional`. It lives exactly as long
  as the mirror that owns it; dropping it closes the listener and aborts its
  in-flight connections.
- **`SshDialer`** spawns `ssh <mux_opts(host)> -W 127.0.0.1:<port> -- <alias>`
  through `proc::command` (never `Command::new`), alias quoted where a shell
  is involved (`shell::quote`; argv form needs none). It reuses
  `SshClient::mux_opts` so a mirror's many short adb connections share one
  ControlMaster instead of paying a handshake each. The child's stdio is the
  stream; the child is killed when the stream drops.
- **`HubDialer`** opens `wss://<hub>/relay?host=<alias>&port=<port>` with the
  client's hub token (the same credential and HTTP client policy as the
  `/events` stream; `http_client` gains WebSocket permission for this one
  path). Binary WebSocket messages carry raw bytes; a close frame (with a
  reason) ends the stream.

## 2. Hub `/relay` route

`GET /relay?host=<alias>&port=<n>` → WebSocket upgrade.

**Authorisation**, in order, each with its own refusal code:

1. A **person** caller (master token, paired desktop/phone client). A
   per-host token is refused in v1 (`E_FORBIDDEN`): a host's Claude uses
   devices through the `debug_devices` tool.
2. `host` is a known host within the caller's org scope — the same rule as
   the Debug devices inventory (*Who may use what*).
3. `port` passes `relay::allowed_port` (`E_INVALID`).
4. Limits: at most 16 concurrent relay streams per client and 32 per host
   (`E_LIMIT`); a stream with no bytes in either direction for 10 minutes is
   closed.

Claims are not enforced here — port 5037 is host-wide, not per-device. The
desktop takes the advisory claim (see *Mirror lifecycle*); the relay enforces
host scope.

**Dialling:** if `SshClient::routes_through_agent(host)` → the agent stream
(§3); otherwise `SshDialer` from the hub (the hub's own `ssh/config`). An
agent host with no connected agent → `E_AGENT_OFFLINE`; an agent whose
negotiated proto is below 3 → `E_AGENT_PROTOCOL` ("update fleet-agent on
<host>").

Every open logs one line (caller, host, port, route) and every close its byte
counts and reason. The route is part of the wire contract:
`CONTRACT_REVISION` 17 → 18 and the desktop's `MAX_HUB_CONTRACT` move
together (see the memory note *Contract revision bump needs the bounds too*);
a desktop talking to a revision-17 hub does not offer the hub route and says
why.

## 3. Agent stream frames (`fleet-proto`, `fleet-agent`)

`PROTO_VERSION` 2 → 3; `MIN_SUPPORTED_PROTO` stays 1. The hub sends `Tcp*`
frames **only** on a connection whose negotiated proto is ≥ 3, so an older
agent never sees them (and would skip them anyway: unknown kinds after the
handshake are ignorable).

Hub → agent:

- `TcpOpen { id, port }` — agent dials `127.0.0.1:<port>` after re-checking
  the allowlist itself (the agent does not trust the hub's check).
- `TcpData { id, bytes_b64 }` — at most 32 KiB raw per frame, well under
  `MAX_FRAME_BYTES`.
- `TcpAck { id, bytes }` — returns send credit.
- `TcpClose { id, reason }`.

Agent → hub: `TcpOpened { id }`, `TcpOpenFailed { id, error }`, and the same
`TcpData` / `TcpAck` / `TcpClose`.

**Flow control:** each direction of each stream has a 256 KiB credit window;
a sender stops reading its socket when its credit is exhausted and resumes on
`TcpAck`. This keeps a video stream from monopolising the agent's single
WebSocket and starving `Exec` results. At most 8 open streams per agent
connection (`TcpOpenFailed` beyond). All streams close when the connection
drops. Stream I/O runs in its own task per stream and writes through the
connection's existing outbound channel; it never blocks the exec path.

## 4. Desktop: mirror service and commands

`device_mirror::start(host, serial)` (Tauri command `start_device_mirror`;
local to the desktop — **not** routed to the hub even in hub-client mode,
because it spawns a local process):

1. **Route choice** (setting `devices.mirror_route`: `auto` | `ssh` | `hub`,
   default `auto`):
   - `local` — the host is this machine → plain
     `scrcpy --serial <serial>`, no relay, no preflight beyond tool checks.
   - `ssh` — this machine has a `Host` entry for the alias and an
     `SshDialer` connection to port 5037 succeeds within 5 s.
   - `hub` — hub-client mode and the hub's contract ≥ 18.
   In `auto`, try `ssh`, then `hub`. The chosen route is returned and shown.
2. **Tools:** local `scrcpy` (setting `devices.scrcpy_path`, else `PATH`)
   and local `adb` (`ANDROID_HOME` / `ANDROID_SDK_ROOT` / `PATH`, the same
   lookup the scan script uses). Missing → `E_TOOL_MISSING` with the install
   hint (`brew install scrcpy`).
3. **Preflight over the relay**, speaking the adb wire protocol directly
   (no adb client is run yet):
   - `host:version` → the remote server's protocol version; compared with
     the local `adb version`. Mismatch → `E_ADB_VERSION_MISMATCH` naming
     both versions. No answer → the remote adb server is not running →
     start it with the existing `debug_devices` run path
     (`adb start-server`) and retry once.
   - `host:list-forward` → pick the first `P` in `27183..=27199` not
     already forwarded on that host. None free → `E_LIMIT`.
4. **Claim** the device (`debug_devices` claim, note `scrcpy mirror`). Held
   by someone else → `E_CONFLICT` naming the holder.
5. **Spawn**, through `proc::command`:
   `scrcpy --serial <serial> --force-adb-forward --port <P>
   --tunnel-host 127.0.0.1 --tunnel-port <L2>
   --window-title "<device name> @ <host>"`, with
   `ADB_SERVER_SOCKET=tcp:127.0.0.1:<L1>` and `ADB=<local adb>`. On the hub
   route add `--video-bit-rate 4M --max-size 1280`.
6. **Register** the mirror (id, host, serial, route, ports, pid, started_at)
   and emit `device_mirror:changed`.

**Teardown** — on scrcpy exit or `stop_device_mirror`: kill scrcpy if
running, drop both listeners (aborting their connections), remove the remote
forward `tcp:<P>` if scrcpy left it (`host-serial:<serial>:killforward:tcp:<P>`
over the relay), release the claim, emit `device_mirror:changed`. A non-zero
exit within the first 10 s is reported as `E_MIRROR_FAILED` with the tail of
scrcpy's stderr (bounded, as the tunnel supervisor does).

App quit stops every mirror. Mirrors are not persisted; they do not survive
a restart.

`list_device_mirrors` returns the registry for the page.

## 5. UI (Debug devices page)

- **Mirror** button on Android rows with `state = device`; disabled with the
  reason on other rows (iOS: "scrcpy mirrors Android only").
- While running: the row shows the route badge (`local` / `ssh` / `hub`) and
  a **Stop** button; the page listens to `device_mirror:changed`.
- Errors render with the code's message and its hint (install scrcpy, update
  the agent, version mismatch, claimed by …).
- Two settings rows (`devices.mirror_route`, `devices.scrcpy_path`) in the
  settings `SPECS`; regenerate settings docs.

## Errors

| code | new? | when |
|------|------|------|
| `E_TOOL_MISSING` | new | no local `scrcpy` or `adb` |
| `E_ADB_VERSION_MISMATCH` | new | local adb ≠ remote adb server protocol |
| `E_NO_ROUTE` | new | `ssh` failed and there is no usable hub route |
| `E_MIRROR_FAILED` | new | scrcpy exited early; carries its stderr tail |
| `E_HUB_CONTRACT` | existing | hub route wanted, hub contract < 18 |
| `E_AGENT_OFFLINE` | existing | agent host with no connected agent |
| `E_AGENT_PROTOCOL` | existing | agent proto < 3 ("update fleet-agent on <host>") |
| `E_CONFLICT` | existing | device claimed by someone else |
| `E_LIMIT` | existing | relay stream limits, or no free forward port |
| `E_FORBIDDEN` | existing | per-host token on `/relay`, or host outside scope |
| `E_INVALID` | existing | port outside the allowlist |

New codes are added to `ipc_error.rs` with frontend handling (hint text).

## Security notes

- The relay only ever reaches `127.0.0.1` on allowlisted ports of a host
  the caller could already see in the inventory. Port 5037 gives full adb
  control of every device on that host — the same power the
  `debug_devices` tool's `run` already grants a person.
- Local listeners bind loopback only and exist only while the mirror runs.
  Another local user of the same machine could connect to them during that
  window; adb has no authentication to add. Accepted for v1 and documented.
- The agent re-checks the allowlist; a compromised hub still cannot make an
  agent dial off-host or an arbitrary port.
- The adb version preflight exists specifically so the mirror can never
  `host:kill` a remote adb server.

## Testing

- **relay:** `RelayListener` + a test `Dialer` against an in-process echo
  server (round trip, half-close, drop aborts); `SshDialer` against a fake
  `ssh` written with `fake_exec::write_exec` (ETXTBSY-safe) that splices to
  the echo server; allowlist table test.
- **adb preflight:** a fake adb server (in-process TCP) answering
  `host:version` / `host:list-forward`; mismatch refusal; port pick.
- **mirror lifecycle:** stub `scrcpy` (records argv/env, exits on signal)
  — route choice, argv/env, claim taken and released, teardown on early exit
  and on stop, registry events.
- **hub `/relay`:** each authorisation refusal; SSH route with the fake
  `ssh`; agent route against the fake agent (`agent/fake.rs`).
- **proto:** `Tcp*` round-trip and unknown-kind skip on a proto-2 peer;
  credit window (sender stalls at 256 KiB until ack); 8-stream cap.
- **agent e2e** (`crates/fleet-agent-e2e`): hub ↔ real agent stream to an
  echo server, including a 10 MB transfer under the window and a close
  mid-transfer.
- **hub contract:** `REGEN_HUB_CONTRACT=1`; control-API reference
  (`REGEN_DOCS=1`) for the three new Tauri commands; settings docs
  (`REGEN_SETTINGS_DOCS=1`); hub verdict rows if the new commands need one.
- **Manual acceptance** (not automatable; none of Debug devices has run on
  real hardware yet): emulator on this Mac (local); a device on a LAN host
  over SSH; the same host via the hub from off-LAN; an agent host.

## Delivery: two PRs

1. **Relay core + SSH + local + UI** — `relay` (allowlist, listener,
   `SshDialer`), `device_mirror` (all of §4 with the `local` and `ssh`
   routes), Tauri commands, the page buttons, settings, docs. Useful on its
   own on the LAN.
2. **Hub relay + agent streams** — `/relay` route, `HubDialer`, contract 18,
   `fleet-proto` 3 with `Tcp*` frames, agent stream handling, hub-side agent
   streams, e2e, docs (`hub.md`, `debug-devices.md`).

Docs touched: `docs/debug-devices.md` (new *Mirror* section, *Not built yet*
updated), `docs/hub.md` (`/relay`, limits, refusal codes, agent proto 3),
`docs/status.md`, `docs/architecture.md` (relay unit).
