# The desktop app as a hub client

**Date:** 2026-09-18
**Status:** Approved design, awaiting implementation plan
**Scope:** `src-tauri` (a remote mode for its command layer), a small client in
`crates/fleet-core` shared with nothing else, the Settings UI in `src/lib`, and
docs. No change to the hub's API, no change to how the embedded server behaves
when the desktop runs standalone.

## Why

Today the desktop app *is* a hub: it owns `state.db`, runs the reconcile tick,
holds every SSH connection, and serves the control API. Sub-project 1 made that
same core runnable headless, and the fleet now has somewhere better to live —
an always-on box that a phone can also reach. But the desktop cannot point at
it. Run both and you get two hubs managing the same hosts: two reconcile
passes, two sets of hooks fighting over which URL a host reports to, two
databases drifting apart.

The desktop should be able to be a *client* of the hub it already talks to over
MCP, so the fleet has exactly one brain and the laptop is a window onto it.

This is sub-project 5 of five, and the last piece of the original ask.

## Goals

- **One hub, many windows.** Pointed at a hub, the desktop shows the same
  fleet the phone shows, live, and steers it the same way.
- **Nothing lost in the common views.** Sessions, hosts, projects, worktrees,
  the conversation, prompts, history, tasks and the repository views work
  against a remote hub.
- **Standalone still works.** With no hub configured the app behaves exactly as
  it does today: its own database, its own tick, its own embedded server.
- **The switch is explicit and reversible.** Settings names the hub; clearing
  it returns to standalone. Neither direction migrates data behind the user's
  back.

## Non-goals

- The terminal against a remote hub. The PTY attaches to a local `ssh` or
  `tmux` process; a hub client would need the hub to stream a pane, which is
  its own design. In remote mode the terminal tab says so and offers the
  command to attach from a shell.
- Merging two databases. A desktop that has been a hub keeps its own file; the
  remote fleet is a separate view. Migration remains "copy `state.db` to the
  hub", as sub-project 1 documented.
- Offline editing. Remote mode needs the hub; when it is unreachable the app
  shows the last snapshot and disables actions, like the phone.
- Running both at once against the same hosts. The app refuses to start its own
  reconcile tick and its own embedded server while a hub is configured.

## Architecture

### The seam

Every Tauri command in `src-tauri/src/commands/` today calls a function in
`fleet_core::service::*` with a `Store` and an `SshClient`. Remote mode swaps
what sits behind the command, not the command itself:

```
Svelte  ──invoke──▶ commands/*  ──┬──▶ service::*            (standalone, today)
                                  └──▶ HubBackend ──▶ POST /mcp on the hub
```

`HubBackend` is a thin client: it maps a command to the MCP tool that already
exists for it, sends a JSON-RPC `tools/call`, and deserialises the same row
types the service layer returns — the hub's tools were built from those types,
so the shapes already match. Where a command has no tool (the PTY, the local
asset catalog authoring, diagnostics of the local process) it stays local and
the UI marks it.

A `Backend` enum resolved once at startup decides which path every command
takes. The choice comes from one setting, `hub.remote_url`, plus a client token
stored beside it.

### Pairing the desktop

The desktop pairs exactly as a phone does, because the hub cannot tell them
apart and should not: the operator runs `fleet-hub pair --name laptop` on the
hub and either scans nothing (there is no camera) or pastes the code into
Settings. The app exchanges it at `POST /pair` and stores the token in the OS
keychain through Tauri's existing secure-storage plugin, not in `state.db`.

### Live updates

Standalone, row changes come from the in-process event bus. In remote mode the
app subscribes to the hub's `GET /events` and feeds the same frontend event
names, so `src/lib/*.ts` stores keep patching in place exactly as they do now.
That is the whole reason the event names were kept identical to the desktop's:
one subscription replaces one bus, and no store code changes.

### What the UI shows

- A header badge naming the hub when remote, absent when standalone.
- The terminal tab, in remote mode, explains that the PTY is local-only and
  offers the `ssh`/`tmux attach` line for the selected session.
- Settings gains a Hub section: the URL, the paired client name, a Pair button
  that takes a code, and Disconnect. Disconnecting does not revoke the token —
  that is the operator's, from the hub.
- Everything a client token may not do (provisioning, adding or removing hosts,
  the asset sync apply) is disabled with the reason, rather than failing at the
  click. The desktop pairs as a client, so it inherits a client's limits.

### Errors

The app surfaces the hub's own errors verbatim: an `E_*` code and message from
a tool, `401` meaning the token was revoked (back to the Hub settings with an
explanation), `403` meaning the hub does not accept this Host header. A dropped
event stream reconnects with backoff and one refetch, showing a banner while
disconnected. No silent fallback to standalone: if a hub is configured and
unreachable, the app says so rather than quietly managing hosts itself.

## Testing

- **Backend mapping:** every command that has a remote path maps to the right
  tool with the right arguments, and deserialises a recorded hub response into
  the same type the service layer would have returned. Table-driven, against a
  fake transport; no network.
- **Mode resolution:** a configured hub selects remote; an empty setting
  selects standalone; remote mode starts neither the reconcile tick nor the
  embedded server (assert on the absence).
- **Event bridging:** a hub SSE frame produces the identical frontend event a
  local `RowChange` produces, name and payload, so the stores cannot tell the
  difference. This is the test that protects the whole "no store code changes"
  claim.
- **Refusals:** a client-forbidden command returns the hub's `E_FORBIDDEN`
  unchanged and the UI disables it up front (a frontend test).
- **Frontend:** the existing Vitest suite must pass untouched — that is the
  evidence remote mode did not disturb the stores.
- **Manual:** point the desktop at the hetzner hub, confirm the session list
  matches the phone's, send a prompt from the desktop and see it on the phone.

## Open questions settled here

1. **The desktop pairs as a client, not with the master token.** It is revocable
   per device like any other, and nothing in the app needs fleet admin.
2. **No PTY over the hub in this sub-project**, for the reasons above.
3. **No automatic migration.** A desktop that was a hub keeps its database;
   pointing it at a hub is a view change, not a data move.
