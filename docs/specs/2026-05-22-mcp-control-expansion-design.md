# Expand MCP control + fleet-control skill + per-PC provisioning

**Date:** 2026-05-22
**Status:** Design — approved; implemented in phases (Phase 1 first)

## Summary

Give a Claude instance fuller control of claude-fleet, and make that control
reachable from any machine:

1. **Phase 1 — Expand the MCP control API** with read-and-observe and
   lifecycle/files tools (so an AI can *see* a session, not just send prompts).
2. **Phase 2 — A `claude-fleet-control` skill** that teaches a Claude instance
   the observe-then-act workflow over those tools.
3. **Phase 3 — Provision each PC's Claude**: claude-fleet (one central
   instance) installs the skill and registers its MCP control server into the
   Claude config on every managed host, reachable via a reverse SSH tunnel to
   the central machine's localhost server.

Phases are sequenced (each depends on the previous) and verified independently.
**This spec details Phase 1 fully; Phases 2–3 are specified at the design level
and get their own implementation plans when reached.**

## Motivation

The MCP control API today lets an AI manage hosts/projects/sessions and
`send_prompt`, but it cannot *read* a session's output — so it can't actually
drive a conversation. Several newer capabilities (recreate, background
sessions, the Files/git views) also aren't exposed. And there's no packaged way
for a Claude on any machine to use the fleet — the operator must wire up the
MCP by hand. This feature closes the observe gap, exposes the newer
capabilities, packages the usage as a skill, and automates getting that skill +
MCP onto each PC's Claude.

## Deployment model (resolved)

**One central claude-fleet instance.** Its MCP control server stays
**localhost-bound** (the existing security boundary: `http://127.0.0.1:<port>/mcp`
+ 256-bit bearer token). Remote PCs reach it through a **reverse SSH tunnel**:
the central machine already holds an SSH ControlMaster to each host, so it adds
`ssh -R <hostport>:127.0.0.1:<mcp-port>` — exposing the central server at
`http://127.0.0.1:<hostport>/mcp` *on that host*. The server never binds beyond
localhost; no token travels over a public network unencrypted.

---

## Phase 1 — Expand the MCP control API

### Scope

Add ~13 tools in three groups (read session output; session-lifecycle gaps;
files & git **read-only** — no mutating git). Out of scope: mutating git
(commit/checkout/branch/push), terminal keystroke injection beyond the existing
`send_prompt`.

### Architecture

The MCP tools (`src-tauri/src/mcp/tools.rs`) call shared logic, exactly as the
existing session tools call `crate::service::sessions::*`. Session lifecycle and
background-session functions already live in `service/`. The Files/git logic
currently lives in `commands/files.rs` + `commands/history.rs` as
`#[tauri::command]` handlers taking `tauri::State`; their bodies use only the
shared `commands::repo` helpers (`session_target`, `repo_script`, `run_in_repo`,
`repo_err`, `diff_from_bytes`) plus pure parsers, so they are refactored to
plain functions that both the Tauri command and the MCP tool call.

**Reuse refactor:** for each of the 8 Files/history commands, extract the body
into a plain function

```rust
pub async fn <name>_impl(
    args: <Args>,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<<T>, IpcError>
```

and reduce the `#[tauri::command]` to a one-line wrapper
(`<name>_impl(args, &store, &ssh).await`). Deref coercion makes
`&State<Arc<Mutex<Store>>>` pass where `&Mutex<Store>` is expected, and the MCP
context (`self.store: Arc<Mutex<Store>>`, `self.ssh: Arc<SshClient>`) passes
`&self.store` / `&self.ssh`. (`session_target` is already
`fn(&Mutex<Store>, i64)`.)

**Capture helper:** add `service::sessions::capture_session_output(session_id,
store, ssh, scrollback_lines: Option<u32>) -> Result<String, IpcError>` that
resolves `(host, name)` via `session_target`, builds `tmux capture-pane -p`
(visible) or `-p -S -<N>` (scrollback) through `exec_for(host, ssh)`, and
returns the text. (Reuses `TmuxExec::capture_pane`; add a scrollback variant if
the trait method only captures the visible screen.)

### New tools (`mcp/tools.rs`)

Each tool: a `Parameters` struct, an `audit(tool, detail)` call, invoke the
impl/service fn, return `ok_json(&result)` (or text for capture). Errors map via
the existing `to_mcp_err`.

**Read session output:**
- `capture_session { session_id, scrollback_lines? }` → pane text.
- `peek_session { session_id }` → background-session logs via
  `service::bg_sessions::peek_session` (already degrades gracefully to
  "interactive session — no background logs").

**Session lifecycle:**
- `recreate_session { session_id }` → `service::sessions::recreate_session`.
- `dismiss_ghost_session { session_id }` → `service::sessions::dismiss_ghost_session`.
- `new_bg_session { host_alias, name, prompt }` → `service::bg_sessions::new_bg_session`.

**Files & git (read-only):**
- `repo_changes { session_id }`
- `repo_tree { session_id }`
- `repo_file { session_id, path }`
- `repo_diff { session_id, path }`
- `repo_log { session_id, all?, limit?, skip? }`
- `repo_branches { session_id }`
- `repo_commit { session_id, hash }`
- `repo_commit_diff { session_id, hash, path }`

### Error handling & safety

- Read tools are side-effect-free. Lifecycle tools mutate but already exist for
  the GUI; each is `audit`ed. The MCP server stays off-by-default + localhost +
  bearer token.
- Frontend-supplied values are already validated by the impls
  (`repo_rel_path`, `commit_hash`, `git_ref`, `host_alias`, `tmux_name`); MCP
  inputs flow through the same validation.

### Testing

- The `*_impl` extraction is behavior-preserving — covered by the existing
  files/history parser tests (`parse_status_z`, `parse_log`, etc.) and a
  compile-through of the Tauri wrappers.
- `capture_session_output`: unit-test the scrollback-arg shaping (visible vs
  `-S -<N>`).
- MCP layer: follow the existing `mcp/tools.rs` test patterns (param
  deserialization / tool registration) for the new tools.
- `docs/control-api.md` updated with the new tool list.

### Phase 1 file-by-file

Modified:
- `src-tauri/src/commands/files.rs` — extract `repo_changes_impl` / `repo_tree_impl`
  / `repo_file_impl` / `repo_diff_impl`; commands become wrappers.
- `src-tauri/src/commands/history.rs` — extract `repo_log_impl` /
  `repo_branches_impl` / `repo_commit_impl` / `repo_commit_diff_impl`.
- `src-tauri/src/service/sessions.rs` — add `capture_session_output`.
- `src-tauri/src/mcp/tools.rs` — add the ~13 tools + param structs.
- `docs/control-api.md` — document the new tools.

---

## Phase 2 — `claude-fleet-control` skill (design level)

A Claude Code skill checked into the repo at
`skills/claude-fleet-control/SKILL.md` (source of truth; Phase 3 installs it
onto hosts).

- **Frontmatter:** `name: claude-fleet-control`, `description:` triggering on
  "control the fleet / drive a claude-fleet session / list/spawn/recreate
  sessions across machines".
- **Body:** the observe-then-act workflow over the MCP tools:
  1. Orient — `fleet_health`, `list_hosts`, `list_projects`, `list_sessions`.
  2. Drive a session — `send_prompt`, then **`capture_session`** to read the
     reply; loop until the task is done (poll with a short delay).
  3. Lifecycle — `new_session` / `spawn_review` / `restart_session` /
     `recreate_session` / `kill_session` / `dismiss_ghost_session`;
     `new_bg_session` + `peek_session` for headless work.
  4. Inspect — `repo_changes` / `repo_diff` / `repo_log` / `repo_branches` to
     review a session's worktree before acting.
  - Safety notes: `recreate_session` kills running Claude state; prefer
    `restart`/`send_prompt` for in-place recovery.

To resolve in Phase 2's plan: exact tool-call examples and any chunking guidance
for large `capture_session` / `repo_diff` outputs.

---

## Phase 3 — Provision each PC's Claude (design level)

claude-fleet (central) makes each managed host's Claude able to control the
fleet. For each reachable host, three **idempotent** steps:

1. **Reverse SSH tunnel** — extend the per-host SSH layer to add
   `-R <hostport>:127.0.0.1:<mcp-port>` so the central control server is
   reachable at `http://127.0.0.1:<hostport>/mcp` on that host. Local/central
   machine needs no tunnel.
2. **Register the MCP server** in the host's Claude config — over SSH, run
   `claude mcp add --transport http claude-fleet
   http://127.0.0.1:<hostport>/mcp --header "Authorization: Bearer <token>"`
   (idempotent: remove-then-add, or check existing). Re-provision when the
   token is regenerated.
3. **Install the skill** — write `skills/claude-fleet-control/SKILL.md` into the
   host's `~/.claude/skills/claude-fleet-control/` over SSH.

A "Provision hosts" action (Settings → Control API, and an MCP tool) triggers
this for all reachable hosts; the local machine gets a direct
`http://127.0.0.1:<mcp-port>/mcp` entry + the skill, no tunnel.

**To resolve in Phase 3's plan (genuine open questions):**
- Tunnel lifecycle: persistent vs on-demand; per-host `hostport` allocation;
  re-establish on ControlMaster reconnect; teardown on disable.
- `claude mcp add` (per-host, requires the `claude` CLI) vs writing the host's
  Claude MCP config file directly — pick the more robust/idempotent one.
- Token rotation: re-run provisioning on every host when the token regenerates.
- Failure handling: a host that's unreachable or lacks the `claude` CLI is
  skipped with a clear status, not a hard failure.

## Phasing & dependencies

Phase 1 is independently valuable and unblocks the rest (the skill documents
Phase 1's tools; provisioning ships the skill). Build and verify Phase 1 →
Phase 2 → Phase 3, each with its own implementation plan.
