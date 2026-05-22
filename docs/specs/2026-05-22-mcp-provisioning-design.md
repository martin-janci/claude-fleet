# MCP control — Phase 3: provision each PC's Claude

**Date:** 2026-05-22
**Status:** Design — approved for implementation planning

## Summary

claude-fleet (one central instance) makes a Claude on every managed host able
to control the fleet. For each reachable host it (1) installs the
`claude-fleet-control` skill into `~/.claude/skills/`, (2) merges an HTTP MCP
server entry into the host's `~/.claude.json`, and (3) for remote hosts,
maintains a reverse SSH tunnel so the host can reach the central machine's
**localhost-bound** MCP server. Triggered by a "Provision hosts" action
(Settings UI + an MCP tool).

This is Phase 3 of the MCP-control feature (Phase 1: tools; Phase 2: the skill).
It depends on both: it ships the Phase-2 skill and exposes the Phase-1 tools.

## Motivation

After Phases 1–2, the control API and the skill exist, but wiring a Claude on
each machine to use them is manual. Phase 3 automates it: one click provisions
all hosts so any machine's Claude can drive the fleet — keeping the control
server localhost-bound (the existing security boundary) and reaching it from
remote hosts only through an authenticated SSH tunnel.

## Design decisions (resolved during brainstorming)

| Decision | Choice |
| --- | --- |
| How remote PCs reach the central localhost server | **App-managed reverse SSH tunnel** (`ssh -R`) per host. |
| How the MCP server is registered in Claude config | **Write/merge `~/.claude.json` directly** (no dependency on the `claude` CLI). |
| Trigger + scope | **"Provision hosts" action** provisions all reachable hosts; local + remote. |

## Background (verified)

- **MCP config:** user-scoped servers live in `~/.claude.json` under the
  top-level `mcpServers` key, shape
  `{ "type": "http", "url": "http://127.0.0.1:<port>/mcp", "headers": { "Authorization": "Bearer <token>" } }`.
  Hand-merging is safe — Claude Code preserves unknown sibling keys (e.g.
  `oauthAccount`, `projects`). **A Claude restart is required** to load a new
  MCP server. (https://code.claude.com/docs/en/mcp.md)
- **Skill:** `~/.claude/skills/<name>/SKILL.md`, **live-discovered** (no
  restart). (https://code.claude.com/docs/en/skills.md)
- **App primitives already present:** `SshClient::run(host, args, timeout)` over
  per-host ControlMaster; a remote-file writer (`ssh.rs`, `cat > <quoted path>`
  over the master); `~/.claude.json` is already read for account probing; the
  MCP server binds `127.0.0.1:<port>` only (`mcp.port`/`mcp.token`/`mcp.enabled`
  settings; default port 4180).

## Architecture

### Skill installer (`service/provision.rs`)

The repo's `skills/claude-fleet-control/SKILL.md` is bundled into the binary
via `include_str!`. For each host: `mkdir -p ~/.claude/skills/claude-fleet-control`
then write `SKILL.md` — over SSH (the `cat >` helper) for remote hosts, plain
`std::fs` for `local`. Idempotent (overwrite). Live-discovered, no restart.

### Config writer (`service/provision.rs`)

For each host:
1. Read `~/.claude.json` (remote: `ssh … cat ~/.claude.json`; local: fs). Empty
   or missing → start from `{}`.
2. Parse as JSON (`serde_json::Value`). If it won't parse → **fail this host**
   (never write garbage).
3. Merge `mcpServers["claude-fleet"] = { type:"http", url, headers:{Authorization:"Bearer <token>"} }`,
   preserving all other keys. `url` = `http://127.0.0.1:<port>/mcp` where
   `<port>` is the mcp port for `local`, or the tunnel's remote bind port for a
   remote host (same fixed port — see below). `<token>` from `mcp.token`.
4. Write a backup (`~/.claude.json.fleet-bak`) then the merged JSON back
   (remote: `cat >`; local: fs). Re-running with a new token just re-merges
   (idempotent).

A pure helper `merge_mcp_entry(existing: &str, url, token) -> Result<String>`
holds the parse/merge/serialize logic and is unit-tested.

### Reverse-tunnel supervisor (`service/tunnel.rs`)

For each provisioned **remote** host, spawn a long-lived
`ssh -R <port>:127.0.0.1:<mcp-port> -N -o ExitOnForwardFailure=yes
-o ServerAliveInterval=30 <host>` (reusing the host's SSH config/alias). The
remote bind `<port>` is a **single fixed port** (the mcp port): each remote host
is a separate machine and does not run the server, so the port is free — no
per-host allocation. So every remote host's `~/.claude.json` entry uses the same
`http://127.0.0.1:<mcp-port>/mcp`.

A `TunnelSupervisor` (held in Tauri state, like `McpRuntime`) tracks one child
per host. It:
- starts/ensures a tunnel when a remote host is provisioned and the MCP server
  is enabled;
- restarts a tunnel on unexpected child exit (capped exponential backoff);
- tears tunnels down on app exit, on MCP-disable, and on unprovision.

A pure helper `tunnel_argv(host, remote_port, mcp_port) -> Vec<String>` builds
the `ssh -R …` arguments and is unit-tested.

### Persisted state

A `provisioned` boolean per host (new column / setting) records which hosts to
re-establish tunnels for on app start (when MCP is enabled) and to re-target on
token rotation. `local` is always "provisionable" but never gets a tunnel.

### Orchestration + triggers

`service::provision::provision_hosts(store, ssh, tunnel_supervisor) ->
Vec<HostProvisionResult>`:
- For each non-hidden host: if unreachable → `Skipped("unreachable")`; else
  install skill + merge config; remote hosts also `ensure_tunnel`. `local` gets
  a direct-localhost URL and no tunnel. Mark `provisioned=true`.
- Per-host failures (`Failed("<reason>")`) never abort the others.
- Returns a status list `{ host, status: Provisioned|Skipped(reason)|Failed(reason), restart_hint }`.

Exposed three ways:
- **Tauri command** `provision_hosts` → Settings → Control API **"Provision
  hosts"** button (renders the per-host result table).
- **MCP tool** `provision_hosts` (so an AI can self-provision the fleet).

**Token rotation:** when `mcp_configure` regenerates the token, re-run
`provision_hosts` for hosts already `provisioned=true` (idempotent re-merge of
the new token). Tunnels are unaffected (port unchanged).

### Lifecycle coupling with the MCP server

Tunnels exist only while the server is enabled. `mcp_configure(enabled=false)`
stops the server and all tunnels; `enabled=true` (re)starts the server and
re-establishes tunnels for provisioned hosts. An mcp **port** change moves both
the tunnel (`-R <newport>:127.0.0.1:<newport>`) and the URL
(`http://127.0.0.1:<newport>/mcp`), so it restarts tunnels **and** re-provisions
the config on provisioned hosts.

## Error handling

- Unreachable/hidden host → `Skipped`, not an error.
- SSH / file-write failure on a host → `Failed("<reason>")`; other hosts
  continue.
- Unparseable remote `~/.claude.json` → `Failed`; never overwrite it (the
  backup is written only on a successful parse+merge).
- Tunnel spawn failure (`ExitOnForwardFailure`) → surfaced in the host's status;
  supervisor retries with backoff.
- Every interpolated value (host alias, paths, token) is shell-quoted / passed
  as argv; the token never crosses a public network (localhost + tunnel only).

## Post-provision UX

The result table notes: **restart Claude on each host** for the MCP server to
load (the config change needs a restart); the skill is picked up live. `local`
is labeled as direct (no tunnel).

## Testing

- **Pure/unit:** `merge_mcp_entry` (adds the entry; preserves `oauthAccount`/
  other keys; idempotent re-merge; missing/empty/`{}` input; rejects invalid
  JSON); `tunnel_argv` (correct `ssh -R <port>:127.0.0.1:<mcp> -N … host`);
  local-vs-remote URL/port selection.
- **Manual smoke:** click "Provision hosts" with at least one remote host →
  skill appears in that host's `~/.claude/skills/`, `~/.claude.json` gains the
  `mcpServers.claude-fleet` entry (siblings intact), the tunnel is up
  (`ss`/`lsof` on the remote shows localhost:<port>), and after restarting
  Claude there, `/mcp` lists `claude-fleet` and its tools work. Verify an
  unreachable host is `Skipped`, token regen re-provisions, and disabling the
  MCP server tears tunnels down.

## File-by-file change list

New:
- `src-tauri/src/service/provision.rs` — skill installer, config writer
  (`merge_mcp_entry`), `provision_hosts` orchestration + result types.
- `src-tauri/src/service/tunnel.rs` — `TunnelSupervisor`, `tunnel_argv`.

Modified:
- `src-tauri/src/commands/mcp.rs` — `provision_hosts` Tauri command; re-provision
  on token regen in `mcp_configure`; start/stop tunnels with the server.
- `src-tauri/src/mcp/tools.rs` — `provision_hosts` MCP tool.
- `src-tauri/src/store.rs` + a migration — `provisioned` flag per host.
- `src-tauri/src/lib.rs` — register the command + `TunnelSupervisor` state; tear
  down tunnels on window-destroyed (next to the SSH/PTY shutdown).
- `src/lib/SettingsDialog.svelte` (or the Control-API settings UI) + `mcp.ts` —
  "Provision hosts" button + per-host result table.
- `docs/control-api.md` — document the `provision_hosts` tool + the provisioning
  flow and the "restart Claude on each host" note.

## Out of scope

- Auto-provisioning on host-add (kept explicit/button-driven for v1).
- Unprovision UI beyond disabling the server (a per-host "unprovision" can come
  later; disabling MCP tears down all tunnels).
- Non-SSH transports / public (non-tunneled) exposure.
