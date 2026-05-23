---
name: claude-fleet-control
description: Use when driving or checking on Claude Code sessions managed by claude-fleet over its MCP control server — listing/spawning sessions across machines, nudging or inspecting a running session, recovering a stuck/frozen/RAM-heavy session, or reviewing a session's worktree changes.
---

# Controlling claude-fleet

claude-fleet runs long-lived Claude Code sessions in tmux across machines (mac,
mefistos, hetzner). Its MCP server exposes tools to **observe** and **act on**
those sessions. The tool *descriptions* tell you what each does; this skill is
the workflow that ties them together — the parts that aren't obvious.

## The core loop: act, wait, observe

`send_prompt` only types text + Enter into the session's terminal. **It does not
return the reply.** Claude needs a few seconds to respond, and the output is the
live tmux *screen*, not a clean transcript. So driving a session is a loop:

1. `send_prompt(session_id, text)` — send the instruction.
2. **Wait** (~3–8s; longer for heavy work) before reading.
3. `capture_session(session_id)` — read the pane. Pass `scrollback_lines` (e.g.
   200) when you need more than the visible screen.
4. If the reply is incomplete or Claude is still working (streaming text, a
   spinner), wait and capture again. Repeat until you see Claude's input prompt
   box / a bare `>` with no activity (idle) or your task is done.

Always get `session_id` from `list_sessions` first (rows carry `id`,
`tmux_name`, `host_alias`, `status`). Orient with `fleet_health` / `list_hosts`
/ `list_projects` when you don't yet know what exists.

## Recovering a misbehaving session — escalation ladder

Climb only as far as needed; each step is more destructive:

| Symptom | Action |
| --- | --- |
| Waiting at a prompt / needs a nudge | `send_prompt(session_id, "continue")` |
| REPL itself wedged, but tmux fine | `restart_session` — relaunches Claude in place |
| Frozen / eating RAM / needs a clean slate | `recreate_session` — kills + rebuilds the tmux session in the same worktree, **resuming the same Claude conversation** |
| Lost from tmux (ghost — shows `status: "ghost"` in `list_sessions`) | `recreate_session` to bring it back, or `dismiss_ghost_session` to drop it |

`recreate_session` is destructive (kills the running process) but preserves the
conversation via session-id resume — prefer `send_prompt`/`restart` for
in-place fixes.

## Inspecting a session's work

Before reporting or acting, read the worktree (read-only): `repo_changes`
(status), `repo_diff(session_id, path)` (one file's diff), `repo_file`,
`repo_tree`; and git state via `repo_log` / `repo_branches` / `repo_commit` /
`repo_commit_diff`.

## Checking what a peer is doing — `peer_status`

You don't need to `capture_session` to find out whether a peer is working,
idle, or stuck. `peer_status(session_id)` returns the reconcile-derived
fields in one call:

- `claude_status` — `working` / `idle` / `stuck` / …
- `current_activity` — the REPL footer line ("Sketching", "Running test", …)
- `stuck_kind` — `auth_menu` / `confirmation` / `none`
- `context_pct` — percent of context window used

Use it before `send_message`/`send_prompt` to avoid pinging a peer that's
mid-stream, and before `broadcast_prompt` to pick a real `status` filter.
`list_sessions` carries the same fields if you want them for every session
at once.

## Peer-to-peer messaging — `send_message` + `inbox`

`send_prompt` types text directly into a peer's REPL, which interrupts
whatever they're doing. For coordination between sessions, prefer the
inbox:

1. `send_message { from_session_id, to_session_id, body, kind?, deliver? }` —
   appends a row to the recipient's inbox. The `from` id is preserved so the
   receiver knows who wrote, and `kind` is a free tag (`message` / `task` /
   `reply` / `alert`).
2. The recipient calls `inbox { session_id, unread_only?, mark_read? }` when
   it's ready — newest-first, and unread rows are marked read by default.

Set `deliver: true` to *also* type the message into the peer's pane with a
`[msg #id from name@host]:` header — useful for urgent prompts where you
want both a paper trail and immediate attention. The inbox row is the
source of truth; pane delivery is best-effort and `delivered_to_pane` /
`deliver_error` in the response tell you what happened.

A typical exchange:

```text
A → send_message(from=A, to=B, body="please review PR #42", kind="task")
B → inbox(session_id=B)                # reads, marks read
B → send_message(from=B, to=A, body="LGTM", kind="reply", deliver=true)
A → inbox(session_id=A)                # or sees the pane line directly
```

For one-to-many work you still want `broadcast_prompt` (typed straight into
every matching session's pane). For one-to-one coordination, prefer
`send_message`.

## Headless work

For unattended tasks use `new_bg_session(host, name, prompt)` and check progress
with `peek_session(session_id)`. (`peek_session` on an interactive session
returns an informational message — interactive sessions have no background job;
use `capture_session` for those.)

## Common mistakes

- **Reading right after `send_prompt`** → empty/partial output. Wait first, then
  `capture_session`; loop.
- **Treating `capture_session` as a transcript** — it's the current screen;
  use `scrollback_lines` for history.
- **Jumping to `recreate_session`** for a session that just needed a nudge — try
  `send_prompt` / `restart_session` first.
