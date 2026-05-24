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

Always get `session_id` from `list_sessions` first. By default it returns
**slim summary rows** — id, host_alias, tmux_name, project_id, worktree_id,
status, claude_status, stuck_kind, lost_at, is_controller — enough to pick a
target without blowing past MCP token caps on a big fleet. To get the rich
auto-derived fields:

- `peer_status(session_id)` — adds `current_activity` (REPL footer line like
  "Sketching") and `context_pct` (percent of context window used) for one
  session in one call.
- `list_sessions { summary: false }` — full `SessionRow` for every match
  (`current_activity`, `context_pct`, `pr_url`, `notes`, …). Use sparingly.

Scope the call with optional filters (any combination): `host_alias`,
`project_id`, `status`, `claude_status`. **Ghosts are hidden by default** —
pass `include_lost: true` to surface them (required before
`recreate_session` can resurrect one).

Useful fields visible in the default summary:
- `claude_status` — `working` / `idle` / `stuck` / unknown
- `stuck_kind` — `auth_menu` / `confirmation` / `none`
- `lost_at` — non-null = ghost (lost from tmux)
- `is_controller` — true on the calling session once you've registered (see below)

Orient with `fleet_health` / `list_hosts` / `list_projects` when you don't yet
know what exists.

## Spawning a session — `new_session`

`new_session { host_alias, project_id, name }` starts a session in an existing
project — add `worktree_id` to land in a specific worktree, or omit it for the
project root. To start work on a **fresh branch**, pass
`new_worktree: "<branch>"` instead of `worktree_id`: it creates a new git
worktree + branch. By default the branch forks from the repo's **default
branch**; pass `base_branch: "dev"` to fork from another branch (it falls back
to the default branch if `dev` doesn't exist on that host). Remote hosts
auto-clone the repo if it's missing.

## Plain-shell sessions — `new_shell_session`

`new_session` runs Claude Code in the pane. When you want a **raw shell** —
to run a dev server, watch a build, exec ad-hoc commands — use
**`new_shell_session { host_alias, project_id, name, worktree_id?, new_worktree?,
base_branch?, start_command? }`** instead. Same project/worktree plumbing
(remote auto-clones, `new_worktree` forks a branch, etc.), but the pane is an
interactive login shell.

`start_command` runs once before the shell drops to an interactive prompt;
the pane stays alive after it exits, so the session survives even if the
command is `cargo run` and you Ctrl+C it. Use it for "leave this running":

```text
new_shell_session { host_alias: "mefistos", project_id: 7,
                    name: "ppt-web-dev", start_command: "pnpm dev:ppt" }
```

You steer a shell session the same way you steer a Claude one:

- `send_prompt(session_id, "ls -la")` — types the text + Enter (it's just
  `tmux send-keys`; works equally well for a REPL or a bash prompt).
- `capture_session(session_id)` — reads the pane.

The escalation ladder (stuck / recreate / restart) and the controller guard
both apply — shell sessions appear in `list_sessions` like any other.

## Host clipboard — `get_clipboard` / `set_clipboard`

When you need to hand text to the human at the keyboard (or read something
they just copied), use the clipboard tools. Both take a `host_alias` and
operate on that host's system clipboard:

- `get_clipboard { host_alias }` — returns the current clipboard contents.
  An empty clipboard returns `(clipboard is empty)` rather than an empty
  block.
- `set_clipboard { host_alias, content }` — overwrites the clipboard.
  Capped at 64 KiB.

The tools probe `wl-paste`/`wl-copy` (Wayland), then `xclip`, then `xsel`
(X11), then `pbpaste`/`pbcopy` (macOS) — the first installed helper wins.
On a headless Linux box with no display + none of those helpers installed,
the call fails with `E_CLIPBOARD_UNAVAILABLE` — that's a host config gap,
not a bug to retry.

## Identifying yourself — `register_self`

Before doing any session lifecycle work, call **`register_self { host_alias,
tmux_name }`** once at the top of your run. This records you as the fleet
controller in the store. Effects:

- `list_sessions` marks your row with `is_controller: true` so other
  controllers can recognise you.
- `kill_session` / `restart_session` / `recreate_session` will **refuse** to
  target the controller — they return `E_SELF_TARGET`. Pass `force: true` if
  you really mean to act on yourself (e.g. a deliberate restart).
- `broadcast_prompt` excludes the controller from its fan-out by default, so
  you never spam yourself.

Skip this only for read-only or one-shot tasks where you'll never be a
plausible target.

**Discover your identity programmatically** — never guess the alias.
`tmux_name` comes from `tmux display-message -p '#S'`; `host_alias` is
**configuration** (whatever the user picked in the host picker) and is not
derivable from `hostname`. The two diverge whenever the user renames a host,
so a hostname-based fast path silently breaks. Always look up the alias:

1. `tmux display-message -p '#S'` — your `tmux_name`.
2. `mcp__claude-fleet__list_sessions {}` — find the row whose `tmux_name`
   matches; take its `host_alias`.
3. `mcp__claude-fleet__register_self { host_alias: "<discovered>", tmux_name: "<#S>" }`.

Same pattern the `fleet-friendly-name` skill uses; see it for edge cases
(no matching row, multiple matches).

## Recovering a misbehaving session — escalation ladder

**Look before you climb.** Read the row from `list_sessions` (or call
`peer_status`) and use `claude_status`, `stuck_kind`, and `context_pct` to
pick the right rung — don't `recreate_session` something that just needs a
nudge.

Climb only as far as needed; each step is more destructive:

| Symptom | Action |
| --- | --- |
| `claude_status: stuck` with `stuck_kind: confirmation` / `auth_menu` | `send_prompt(session_id, "1")` (or the right keystroke) to dismiss it |
| `claude_status: idle` but waiting at a prompt | `send_prompt(session_id, "continue")` |
| `context_pct` near 100 — the conversation is full | `recreate_session` resumes the same Claude id in a fresh REPL |
| REPL itself wedged, but tmux fine | `restart_session` — relaunches Claude in place |
| Frozen / eating RAM / needs a clean slate | `recreate_session` — kills + rebuilds the tmux session in the same worktree, **resuming the same Claude conversation** |
| Lost from tmux (ghost — `status: "ghost"`; pass `include_lost: true` to see them in `list_sessions`) | `recreate_session` to bring it back, or `dismiss_ghost_session` to drop it |

`recreate_session` is destructive (kills the running process) but preserves
the conversation via session-id resume — prefer `send_prompt`/`restart` for
in-place fixes. If the target is the registered controller, pass
`force: true` or the call refuses with `E_SELF_TARGET`.

## Inspecting a session's work

Before reporting or acting, read the worktree (read-only): `repo_changes`
(status), `repo_diff(session_id, path)` (one file's diff), `repo_file`,
`repo_tree`; and git state via `repo_log` / `repo_branches` / `repo_commit` /
`repo_commit_diff`.

## Session timeline — `session_history`

Every status change, prompt send, stuck detection, kill, recreate, and
peer message is appended to a per-session event log. Pull it with
**`session_history { session_id, limit? }`** when you need the *story* of
what happened — `capture_session` only shows the current screen.

Recorded `kind`s: `status_change` · `prompt_sent` · `stuck` · `killed` ·
`recreated` · `message_sent` · `message_received`. The response is newest-
first; default `limit` is 50.

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
For the same fields across every session, call `list_sessions { summary:
false }` — the default summary mode omits `current_activity` and
`context_pct` to keep responses inside MCP token caps.

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

For one-to-many work use `broadcast_prompt` (next section); for one-to-one
coordination prefer `send_message`.

## Broadcasting to many sessions — `broadcast_prompt`

When the same instruction needs to reach every matching work session,
**`broadcast_prompt { host?, project_id?, status?, prompt, submit? }`** is the
fan-out. The filter is AND-combined; omit a field to leave it open:

- `host` — only sessions on this host alias
- `project_id` — only sessions in this project (from `list_projects`)
- `status` — only sessions whose `claude_status` matches (e.g. `"idle"`)

The call returns a per-session result vector (sent / failed counts + each
target's outcome) so you can see who got it. Two implicit guardrails: only
**work** sessions are eligible (background sessions are skipped), and the
registered controller is excluded — so you never broadcast to yourself.

Typical use: `broadcast_prompt { status: "idle", prompt: "git pull && pnpm test" }`
to wake every idle worker. Use `peer_status`/`list_sessions` first to set a
realistic `status` filter — broadcasting to `working` sessions interrupts
them mid-stream.

## Headless work

For unattended tasks use `new_bg_session(host, name, prompt)` and check progress
with `peek_session(session_id)`. (`peek_session` on an interactive session
returns an informational message — interactive sessions have no background job;
use `capture_session` for those.)

## Recovering from MCP errors

Three error classes, three different responses — do not blanket-retry:

- **Application errors** (`E_NOTFOUND`, `E_SELF_TARGET`, `E_LOCK`, `E_VALIDATION`, …):
  the call reached the server and the server said no. **Surface immediately**,
  then re-sync state before any retry. The canonical re-sync calls are
  `list_sessions`, `list_hosts`, `list_projects`, `list_worktrees` — they
  return fresh truth from the store. `E_NOTFOUND` on a session id usually
  means the row moved (ghost, recreated, renamed); `list_sessions { include_lost: true }`
  surfaces ghosts so `recreate_session` can revive them.

- **Transient transport errors** (timeout, connection drop, 5xx): retry **once**
  after a short wait, then re-sync state. The MCP server is local (or via
  reverse tunnel) and recovers quickly. If the second attempt also fails,
  surface — don't loop.

- **Destructive ops** (`kill_session`, `safe_kill_session`, `recreate_session`,
  `restart_session`, `delete_worktree`, `remove_host`, `dismiss_ghost_session`):
  **never auto-retry**. A timeout doesn't mean the op didn't happen — it may
  have succeeded server-side and the response was lost. Re-sync first
  (`list_sessions`/`list_hosts`/`list_worktrees`), confirm the actual state,
  then decide whether to act again.

**Self-healing pattern** — when a tool that takes a `session_id` errors with
`E_NOTFOUND`, your in-memory id is stale. Call `list_sessions` (or
`related_sessions { session_id }` if you have a sibling id) to find the
current row before retrying. Do not loop the same id.

## Common mistakes

- **Reading right after `send_prompt`** → empty/partial output. Wait first, then
  `capture_session`; loop.
- **Treating `capture_session` as a transcript** — it's the current screen;
  use `scrollback_lines` for history.
- **Jumping to `recreate_session`** for a session that just needed a nudge — try
  `send_prompt` / `restart_session` first.
- **Auto-retrying a destructive op after a timeout** — the op may have
  succeeded; re-sync state first (see "Recovering from MCP errors" above).
