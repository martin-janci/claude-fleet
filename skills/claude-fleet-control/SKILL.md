---
name: claude-fleet-control
description: Use when driving or checking on Claude Code sessions managed by claude-fleet over its MCP control server — listing/spawning sessions across machines, nudging or inspecting a running session, recovering a stuck/frozen/RAM-heavy session, or reviewing a session's worktree changes.
---

# Controlling claude-fleet

claude-fleet runs long-lived Claude Code sessions in tmux across machines. Its
MCP server exposes tools to **observe** and **act on** those sessions. The tool
descriptions say what each tool does and which parameters it takes; this skill
is the workflow that ties them together — the parts that aren't obvious.

## Status vocabulary

Every session row carries two derived fields. These are the only values that
exist (quoted verbatim from the backend enums; a test fails if they drift) —
anything else is a doc bug, not a state to handle.

```text
claude_status: working | blocked | completed | failed | stopped | idle   (null = unknown)
stuck_kind:    auth_menu | reconnect | trust_prompt | oom | press_enter  (null = not stuck)
```

- `claude_status` — `working` = generating / running tools; `blocked` = waiting
  on input (permission prompt, question, stuck state); `idle` = turn over, REPL
  at its prompt; `completed` / `failed` / `stopped` are terminal states (mostly
  background sessions).
- `stuck_kind` — `auth_menu`, `reconnect`, `trust_prompt`, `oom`, `press_enter`;
  set from the pane tail, and when set `claude_status` is usually `blocked`.

Also useful: `status` (`running` / `ghost`), `lost_at` (non-null = ghost),
`is_controller` (true on your own row once registered), `context_pct` (percent
of the context window used; only in `peer_status` or `summary: false` rows).

## Finding sessions — `list_sessions`

Always get `session_id` from `list_sessions` first. By default it returns slim
summary rows (id, host_alias, tmux_name, project_id, worktree_id, status,
claude_status, stuck_kind, lost_at, is_controller). Filters AND-combine:
`host_alias`, `project_id`, `status`, `claude_status`; `limit` caps the rows
after filtering; `include_lost: true` surfaces ghosts (required before
`recreate_session` can revive one). For `current_activity` / `context_pct` on
one session call `peer_status`; for every session pass `summary: false`
(expensive — use sparingly). Orient with `fleet_health` / `list_hosts` /
`list_projects` / `list_worktrees` when you don't yet know what exists.

## Identifying yourself — `register_self`

Before any session lifecycle work, call `register_self { host_alias, tmux_name }`
once. It marks your row `is_controller: true`, makes `kill_session` /
`restart_session` / `recreate_session` refuse to target you (`E_SELF_TARGET`
unless `force: true`), and excludes you from `broadcast_prompt`.

**Never guess the alias.** `tmux_name` is `tmux display-message -p '#S'`;
`host_alias` is configuration (whatever the user named the host in the picker)
and is not derivable from `hostname`. Look it up:

1. `tmux display-message -p '#S'` — your `tmux_name`.
2. `list_sessions {}` — find the row whose `tmux_name` matches; take its `host_alias`.
3. `register_self { host_alias: "<discovered>", tmux_name: "<#S>" }`.

The `fleet-friendly-name` skill uses the same lookup and covers the edge cases
(no matching row, multiple matches).

## Spawning — `new_session` / `new_shell_session` / `new_bg_session`

- `new_session { host_alias, project_id, name }` runs Claude Code in a tmux
  pane. Add `worktree_id` to land in an existing worktree, or
  `new_worktree: "<branch>"` (+ optional `base_branch`) to create one. Remote
  hosts auto-clone the repo if it is missing.
- `new_shell_session` — same plumbing, but the pane is a plain login shell;
  `start_command` runs once and the shell stays alive after it exits (dev
  servers, watchers). Steer it with `send_prompt` / `capture_session` exactly
  like a Claude session.
- `new_bg_session { host_alias, name, prompt }` — supervised headless run; rows are
  named `bg:<uuid>`. Track with `peek_session`; `capture_session` does not
  apply (no pane). `kill_session` stops it via `claude stop`.

## Steering — the act, wait, observe loop

`send_prompt` only types text + Enter into the pane. **It does not return the
reply**, and the output is the live tmux screen, not a transcript:

1. `send_prompt(session_id, text)`.
2. Wait ~3–8 s (longer for heavy work).
3. `capture_session(session_id)` — returns plain text, the last 200 lines by
   default (`max_lines`, 0 = no cap); add `scrollback_lines` for history.
4. Still streaming / spinner? Wait and capture again until the REPL is back at
   its input prompt (or `peer_status` reports `idle`).

For coordination between sessions prefer the inbox over interrupting a peer:
`send_message { from_session_id, to_session_id, body, kind?, deliver? }` and
`inbox { session_id, unread_only?, mark_read? }`. `from == to` returns
`E_SELF_TARGET`; there is no `force` override for this case. Check
`peer_status` before prompting a peer that may be mid-stream. For one-to-many use
`broadcast_prompt { host?, project_id?, status?, prompt }` — `status` filters on
`claude_status` (e.g. `"idle"`); work sessions only, controller excluded.

`session_history { session_id, limit? }` is the per-session event log
(`status_change`, `prompt_sent`, `stuck`, `killed`, `recreated`,
`message_sent`, `message_received`, `safe_kill_requested`, `safe_kill_ready`,
`safe_kill_failed`, `safe_kill_send_failed`; newest first) — the *story*,
where `capture_session` is only the current screen.

## Recovering — escalation ladder

Look before you climb: read `claude_status`, `stuck_kind`, `context_pct`.
Each rung is more destructive than the last.

| Symptom | Action |
| --- | --- |
| `stuck_kind: press_enter` / `trust_prompt` / `auth_menu` | `send_prompt` the right keystroke (Enter, `y`, a menu number) |
| `stuck_kind: reconnect` | wait and re-check; `restart_session` if it persists |
| `claude_status: idle` but you expected work | `send_prompt(session_id, "continue")` |
| `context_pct` near 100 | `recreate_session` — fresh REPL, same Claude conversation |
| REPL wedged, tmux fine | `restart_session` — relaunch Claude in place |
| `stuck_kind: oom`, frozen, eating RAM | `recreate_session` — kills + rebuilds the tmux session in the same worktree, resuming the conversation |
| Ghost (`status: "ghost"`, needs `include_lost: true`) | `recreate_session` to revive, or `dismiss_ghost_session` to drop |

`recreate_session` kills the running process but keeps the conversation via
session-id resume; prefer `send_prompt` / `restart_session` for in-place fixes.

## Reviewing a session's work

Read the worktree without touching it: `repo_changes` (git status),
`repo_diff(session_id, path)`, `repo_file`, `repo_tree`; git state via
`repo_log` (50 newest commits by default; `skip` pages), `repo_branches`,
`repo_commit`, `repo_commit_diff`. `spawn_review { source_session_id, prompt }`
starts a separate review session in the same worktree; `related_sessions`
lists siblings sharing a project + worktree.

## Retiring a session safely — `safe_kill_session`

`kill_session` stops the process immediately. `safe_kill_session` asks the
session to commit + push first, then arms deletion of its worktree + tmux
session; the delete only fires after the session's ready marker AND a
clean-tree check (transitions arrive as row events; `E_SAFE_KILL_IN_PROGRESS`
means one is already armed). Use it when the worktree may hold unpushed work;
`delete_worktree` afterwards if the worktree should go too.

## Host clipboard — `get_clipboard` / `set_clipboard`

Hand text to (or read it from) the human at the keyboard of `host_alias`.
`E_CLIPBOARD_UNAVAILABLE` on a headless box means no clipboard helper is
installed — a host config gap, not something to retry.

## Recovering from MCP errors

Errors come back as `E_<CODE>: message`. Three classes, three responses:

- **Application errors** (`E_NOTFOUND`, `E_INVALID`, `E_VALIDATE`,
  `E_SELF_TARGET`, `E_BG_SESSION`, `E_HOST_OFFLINE`, `E_TMUX`, `E_LOCK`, …):
  the server said no. Surface immediately, then re-sync (`list_sessions`,
  `list_hosts`, `list_projects`, `list_worktrees`) before any retry.
  `E_NOTFOUND` on a session id means your id is stale (ghosted, recreated,
  renamed) — re-list, never loop the same id.
- **Transient transport errors** (timeout, connection drop, 5xx): retry once
  after a short wait, then re-sync. If it fails again, surface.
- **Destructive ops** (`kill_session`, `safe_kill_session`, `recreate_session`,
  `restart_session`, `delete_worktree`, `remove_host`, `dismiss_ghost_session`):
  never auto-retry — a timeout may still have succeeded server-side. Re-sync,
  confirm the actual state, then decide.

## Common mistakes

- Reading right after `send_prompt` → empty/partial output. Wait, then capture; loop.
- Treating `capture_session` as a transcript — it is the current screen; use
  `scrollback_lines` / `session_history` for history.
- Jumping to `recreate_session` for a session that needed a nudge.
- Guessing `host_alias` from `hostname` — always look it up via `list_sessions`.
- Auto-retrying a destructive op after a timeout.
