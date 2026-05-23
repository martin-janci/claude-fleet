# Changelog

## [0.3.0] - 2026-05-23

### Features

- **Background sessions:** background (`claude --bg`) sessions now appear in
  `list_sessions`, with a sidebar bg-agent filter toggle + badge and a
  `BgSessionPanel` that auto-polls the session's Claude logs.
- **Worktree robustness:** detect a worktree whose directory has gone missing,
  ghost its sessions, two-phase mark→auto-prune reconcile, a `recreate_worktree`
  command, and a "missing" badge + inline Recreate button in ProjectDetails.
- **Friendly names:** the in-session agent can set a short display name via the
  `set_friendly_name` MCP tool; the sidebar shows it when toggled on.
- **Fleet observability:** proactive background reconcile tick; `fleet_health`
  per-host/per-status roll-up; pane-tail intel (current activity, derived
  status, stuck detection, context %); persistent session event timeline +
  `session_history` tool.
- **Peer coordination:** peer-to-peer messaging + `peer_status` tool;
  `broadcast_prompt` fan-out to matching sessions; controller identity with
  self-target guardrails on kill/recreate/restart.
- **Files tab:** syntax highlighting in the viewer (YAML, TOML, Markdown) with
  type icons; clean state when a session's worktree is deleted; calm message for
  directory entries; git history, branches & branch tree.
- **Terminal:** OSC 52 clipboard support for remote copy.
- **Sidebar:** remember/restore last selected session; 8h and 3d recency filters.
- **App lifecycle:** kill other app instances on startup.
- **Docs/CI:** control-API reference generated from the tool router with a
  CI staleness gate.

### Bug Fixes

- **Sessions:** `recreate`/`spawn_review` use the correct cwd on remote hosts;
  `set_friendly_name` no longer rejects background session rows.
- **MCP:** mount the control API at `/mcp` via `route_service`; never emit empty
  text blocks from tool results; re-establish tunnels on startup; shrink
  `list_sessions` payload to fit token caps.
- **Status:** tighten OOM match and make `stuck_kind` clearable.
- **Provision:** document the `local` host-alias fallback in the managed
  CLAUDE.md block.

### Documentation

- Add RELEASING guide and release automation (release-please) configuration.
- Add `claude-fleet-repo` developer skill and refresh `claude-fleet-control`.

## [0.2.0] - phase 2

Initial baseline (multi-host, accounts, cross-host sessions, prompt transfer,
async/events rework, MCP control API).
