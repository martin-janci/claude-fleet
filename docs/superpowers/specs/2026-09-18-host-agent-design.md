# fleet-agent: an outbound transport for hosts the hub cannot reach

**Date:** 2026-09-18
**Status:** Approved design, awaiting implementation plan
**Scope:** a new `crates/fleet-agent` binary, a transport abstraction and an
agent registry in `crates/fleet-core`, one migration, hub-side wiring in
`crates/fleet-hub`, and docs. No desktop UI change, no change to how SSH hosts
behave.

## Why

The hub reaches every host by SSH today. That requires the hub to be able to
open a connection to the host: a routable address or a tunnel, plus a key in
the host's `authorized_keys`. A laptop behind a home router, a machine on a
corporate network, or anything on mobile tethering cannot be managed that way
without extra plumbing the operator has to build and maintain.

An agent inverts the direction: the host dials the hub, keeps one connection
open, and executes what the hub asks. Setup per host becomes one install
command with a token, and nothing has to be reachable from outside.

This is sub-project 4 of five. Sub-project 1 deliberately deferred it and kept
the SSH seam narrow so this could be added without touching the service layer.

## Goals

- **A host needs no inbound reachability.** No public address, no port
  forward, no tunnel, no key in `authorized_keys`.
- **The service layer does not change.** Everything above the transport keeps
  calling the same seven methods it calls today.
- **Per-host choice.** A fleet can mix SSH hosts and agent hosts; the host row
  says which, and the hub routes accordingly.
- **Setup is one command.** `fleet-agent install --hub https://X --token …`
  writes a service unit and starts dialling.
- **An agent is not a shell for the world.** It executes only what its hub
  sends over an authenticated connection it opened itself.

## Non-goals

- Replacing SSH. SSH stays the default and the only transport for hosts the
  hub can already reach.
- The interactive terminal. The desktop's PTY attaches over SSH; an agent host
  has no terminal view until that is designed separately.
- Agent-to-agent traffic, or an agent reaching anything but its own hub.
- Automatic migration of an existing SSH host to an agent. The operator
  installs the agent and flips the host's transport deliberately.

## Architecture

### The seam

`SshExec` (in `crates/fleet-core/src/ssh.rs`) is already the whole transport
contract: `run`, `run_bounded`, `run_bounded_capped`, `run_cancellable`,
`run_bounded_cancellable`, `upload_file`, `remote_home`. The agent transport
implements the same trait, so no service function changes.

```
service/*  ──▶ dyn SshExec ──┬──▶ SshClient            (today, unchanged)
                             └──▶ AgentTransport ──▶ AgentRegistry ──▶ one live WebSocket per host
```

A `HostTransport` resolver picks the implementation per host from the host row,
so a call for `mefistos` goes over SSH while a call for `laptop` goes to its
agent. Everything else — timeouts, cancellation, shell quoting, output caps —
stays where it is.

### The protocol

One WebSocket per host, opened by the agent to `wss://<hub>/agent`, upgraded
from an HTTP request carrying the host's existing per-host bearer token. The
hub already mints those and the agent install command receives one.

Frames are JSON, one request and one response per id:

| Hub → agent | Meaning |
|---|---|
| `exec { id, argv, stdin?, timeout_ms, cap_bytes? }` | Run this argv, no shell unless the argv says so |
| `upload { id, path, mode, bytes_b64 }` | Write a file, creating parents, with this mode |
| `cancel { id }` | Kill the child of an in-flight request |
| `ping { id }` | Liveness |

| Agent → hub | Meaning |
|---|---|
| `hello { agent_version, host_name, os }` | First frame after the upgrade |
| `result { id, exit_code, stdout_b64, stderr_b64, truncated }` | One per request |
| `pong { id }` | |

The agent enforces its own ceiling on output size and on concurrent
executions, and refuses a frame whose id it has already seen. The hub applies
the same wall clocks it applies to SSH, so a wedged agent cannot hold a tool
call open longer than an SSH host could.

### Registry and reconnection

The hub keeps an `AgentRegistry`: host alias → live connection, with the
connect time and the agent's reported version. A call for a host with no live
agent fails immediately with `E_AGENT_OFFLINE` rather than hanging, so the UI
and the tools report "unreachable" the way they already do for a down SSH host.

The agent reconnects with capped exponential backoff and a jittered delay, and
sends a heartbeat every 30 seconds. The hub drops a connection that misses two
heartbeats. A second connection for the same host replaces the first, so a
restarted agent recovers without waiting for a timeout.

### Host rows

Migration 034 adds `transport` to `hosts`: `'ssh'` (default, what every
existing row gets) or `'agent'`. (It was written as 033; `main` released its
own 033 first, so this one was renumbered when the branches merged. Migration
035 repairs databases created by the branch before that renumber — they
recorded 33 for *this* migration and would otherwise never be offered main's
033. See `035_host_layers_repair.sql`.) `add_host` gains a transport argument;
`list_hosts` returns it; the reconcile pass treats an agent host exactly like
an SSH host except that reachability comes from the registry rather than a
probe. Provisioning writes the same hooks and MCP entry over whichever
transport the host uses.

### The agent binary

`crates/fleet-agent`, a small binary with no dependency on `fleet-core`:

- `fleet-agent run --hub <url> --token <token>` — dial, serve, reconnect.
- `fleet-agent install --hub <url> --token <token> [--user]` — write a systemd
  unit (system or user), enable and start it; print what it wrote.
- `fleet-agent status` — is the service running, and is it connected.

It stores nothing but its configuration file (mode 0600, containing the hub URL
and the token) and logs through `tracing` to the journal. It runs as the user
whose sessions it manages, because that is whose tmux and Claude it drives.

### Security

- The agent opens the connection; the host needs no listening port.
- The token is the host's existing per-host token, so revoking or rotating it
  through the hub cuts the agent off exactly as it cuts off the host's hooks.
- The hub is authenticated by TLS; the agent refuses a plain `ws://` hub unless
  `--insecure` is passed, which the docs reserve for a loopback test.
- An agent executes only what arrives on its own connection. There is no
  inbound path, no port, and no way for another host to reach it.
- The hub's existing gates are unchanged: a `readonly` token still cannot call
  a mutating tool, and a client token still cannot reach fleet admin, whatever
  the transport underneath.

## Error handling

- No live agent: `E_AGENT_OFFLINE`, named in the tool descriptions the same way
  `E_SSH` is today.
- A frame the agent cannot parse: the connection is closed with a reason and
  the agent reconnects; the in-flight call fails with `E_AGENT_PROTOCOL`.
- Output past the cap: truncated with the flag set, matching the SSH path's
  behaviour.
- A duplicate request id: refused, so a replayed frame cannot run twice.

## Testing

- The protocol's encode and decode, including unknown frames and oversized
  payloads.
- `AgentTransport` against a fake registry: each of the seven trait methods
  maps to the right frame, honours its timeout, and reports `E_AGENT_OFFLINE`
  with no connection.
- The registry: connect, replace on reconnect, drop on missed heartbeats, and
  a call racing a disconnect.
- The agent's executor: argv execution, the output cap, cancellation killing
  the child, and a duplicate id refused.
- End to end in one process: a hub with an in-memory registry, an agent
  connected over a real loopback WebSocket, running `echo`, uploading a file,
  and surviving a reconnect.
- The existing SSH tests must pass untouched — that is the evidence the seam
  did not move.

## Open questions settled here

1. **WebSocket rather than a long-poll or gRPC.** It is one dependency the
   hub's axum stack already implies, survives proxies, and matches the
   request/response shape the transport needs.
2. **The per-host token rather than a new credential kind.** Revocation and
   rotation already exist for it, and an agent is the host.
3. **No PTY over the agent in this sub-project.** The terminal is the
   desktop's, over SSH; doing it properly means streaming and resize, and it
   deserves its own design.
