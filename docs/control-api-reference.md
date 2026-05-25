<!-- GENERATED FILE — do not edit by hand.
     Regenerate with: REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current -->

# claude-fleet Control API — Tool Reference

Auto-generated from the embedded MCP tool router. See [`control-api.md`](control-api.md) for the narrative guide.

## MCP tools

### `add_host`

Register a new SSH host. Probes it first; only persists the host if it is reachable. Returns the host row as JSON.

Parameters: `alias`, `ssh_alias`

### `broadcast_prompt`

Send the same prompt to every matching work session (excludes the controller). Returns per-session results.

Parameters: `host`, `project_id`, `prompt`, `status`, `submit`

### `capture_session`

Capture a session's terminal output — the visible tmux pane, or include scrollback history. Use after send_prompt to read the session's reply. Returns the pane text.

Parameters: `scrollback_lines`, `session_id`

### `delete_worktree`

Delete a git worktree on its host (no --force) and drop fleet's row. Refuses if an alive session points at it (override with force=true). Errors: E_WORKTREE_BUSY, E_NOTFOUND, E_GIT.

Parameters: `force`, `worktree_id`

### `discover_hosts`

Discover SSH hosts from the user's ~/.ssh/config. These are candidates for add_host. Returns JSON.

### `dismiss_ghost_session`

Dismiss a ghost session (lost from tmux): permanently delete its row. Errors if the session is not a ghost.

Parameters: `session_id`

### `fleet_health`

Report claude-fleet backend health: application version, SQLite schema version, and database readiness. Returns JSON.

### `get_clipboard`

Read a host's current system clipboard (whatever a human would get from Ctrl+V on that machine). Probes wl-paste, xclip, xsel, pbpaste in order. E_CLIPBOARD_UNAVAILABLE if none is installed.

Parameters: `host_alias`

### `hide_host`

Hide or show a host. Hidden hosts are skipped during reconcile. Returns the updated host row as JSON.

Parameters: `alias`, `hidden`

### `inbox`

Read a session's inbox — messages sent TO session_id, newest-first. Slim rows by default (metadata + 80-char body preview); pass summary=false for full bodies. mark_read (default true) flips returned unread rows to read — pass false to peek without consuming.

Parameters: `limit`, `mark_read`, `session_id`, `summary`, `unread_only`

### `kill_session`

Kill a tmux session on a host. Returns the killed session's id.

Parameters: `force`, `host_alias`, `name`

### `list_accounts`

List the cached Claude accounts seen across hosts. Returns JSON.

### `list_hosts`

List all registered hosts with their reachability, claude/tmux versions, and linked account. Returns JSON.

### `list_projects`

List discovered projects. Slim rows by default (id, owner, repo, worktree_count, last_session_at); pass summary=false for the full nested worktree tree.

Parameters: `summary`

### `list_sessions`

List tmux sessions across reachable hosts. Slim summary rows by default; pass summary=false for the full SessionRow. Optional filters: host_alias, project_id, status, claude_status, include_lost (default false drops ghosts).

Parameters: `claude_status`, `host_alias`, `include_lost`, `project_id`, `status`, `summary`

### `list_worktrees`

List git worktrees fleet knows about, each with its alive-session occupants (empty = free to delete via delete_worktree). Optional project filter.

Parameters: `project_id`

### `new_bg_session`

Launch a supervised headless (background) Claude session on a host with an initial prompt. Returns the new Claude session id as JSON; track progress with peek_session.

Parameters: `host_alias`, `name`, `prompt`

### `new_session`

Create a Claude Code tmux session on a host, in a project (and optional worktree). Pass new_worktree to fork a fresh worktree+branch (optional base_branch). Auto-clones the repo on remote hosts.

Parameters: `base_branch`, `host_alias`, `name`, `new_worktree`, `project_id`, `worktree_id`

### `new_shell_session`

Create a plain-shell tmux session on a host (no Claude Code in the pane — an interactive login shell). Same project/worktree plumbing as new_session, plus an optional start_command that runs once before the shell drops to an interactive prompt; the pane stays alive after it exits so you can attach or send-keys to it. Steer it with send_prompt (typed text + Enter) and read it with capture_session.

Parameters: `base_branch`, `host_alias`, `name`, `new_worktree`, `project_id`, `start_command`, `worktree_id`

### `peek_session`

Peek at a session's background Claude logs. Returns an informational message for interactive sessions with no background job.

Parameters: `session_id`

### `peer_status`

What is a peer session doing? Returns claude_status, current_activity, stuck_kind, context_pct (plus host/name/status) for one session. Cheap pre-check before send_message or broadcast_prompt.

Parameters: `session_id`

### `probe_host`

Re-probe a registered host's reachability and versions. Returns the updated host row as JSON.

Parameters: `alias`

### `provision_hosts`

Install fleet skills and register this fleet's MCP server into every reachable host's ~/.claude.json (reverse SSH tunnel for remote hosts). Returns a per-host status list; each host must restart Claude to load the server.

### `recreate_session`

Recreate a session: kill its tmux session and rebuild it fresh in the same worktree, resuming the same Claude conversation. Works for running or ghost sessions. Returns the session row as JSON.

Parameters: `force`, `session_id`

### `refresh_projects`

Rescan the local projects directory for new or removed repositories and worktrees. Returns the fresh project list.

### `register_self`

Mark the calling session as the fleet controller; kill/recreate/restart refuse to target it without force.

Parameters: `host_alias`, `tmux_name`

### `related_sessions`

List sessions related to a given session — those sharing the same project and worktree. Returns JSON.

Parameters: `session_id`

### `remove_host`

Remove a registered host. Its sessions are orphaned. Returns the removed host row as JSON.

Parameters: `alias`

### `rename_session`

Rename a tmux session on a host. Returns the updated session row as JSON.

Parameters: `host_alias`, `new_name`, `old_name`

### `repo_branches`

List local + remote branches for a session's worktree with ahead/behind. Returns JSON array.

Parameters: `session_id`

### `repo_changes`

List a session's changed files (git status) in its worktree. Returns JSON array of changed files.

Parameters: `session_id`

### `repo_commit`

One commit's metadata + changed files. Returns JSON {hash, subject, body, author, date, files}.

Parameters: `hash`, `session_id`

### `repo_commit_diff`

Diff of one file within a commit. Returns JSON {path, diff, binary, truncated}.

Parameters: `hash`, `path`, `session_id`

### `repo_diff`

Unified diff for one worktree file vs HEAD (untracked files render as all-added). Returns JSON {path, diff, binary, truncated}.

Parameters: `path`, `session_id`

### `repo_file`

Read one worktree file's contents (capped). Returns JSON {path, content, truncated, binary, size}.

Parameters: `path`, `session_id`

### `repo_log`

Commit log (branch graph) for a session's worktree. all=true (default) includes every branch. Returns JSON array of commits with parents + ref decorations.

Parameters: `all`, `limit`, `session_id`, `skip`

### `repo_tree`

List a session's worktree files (tracked + untracked, gitignore respected). Returns JSON {entries, truncated}.

Parameters: `session_id`

### `restart_session`

Restart a tmux session (kill and recreate it in the same place). Returns the updated session row as JSON.

Parameters: `force`, `host_alias`, `name`

### `safe_kill_session`

Ask a running Claude session to safely persist its work (commit + push), then arm deletion of its worktree + tmux session. Returns the row with safe_kill_state=requested; the actual delete fires only after the SAFE_REMOVE_READY marker AND a clean-tree check. Transitions ('ready', 'failed') arrive via row events.

Parameters: `host_alias`, `tmux_name`

### `send_message`

Send a peer-to-peer message from one session to another. The message is persisted to the recipient's inbox (read with `inbox`); set `deliver: true` to ALSO type the message into the recipient's tmux pane with a `[msg #id from name@host]:` header. The inbox row is the source of truth — it lands even if the pane delivery fails. Returns JSON with the new message id and the delivery outcome.

Parameters: `body`, `deliver`, `from_session_id`, `kind`, `submit`, `to_session_id`

### `send_prompt`

Send and SUBMIT a prompt to a running Claude session's REPL (literal text, then one Enter). This is how you steer a session. Set submit=false to stage text in the REPL without submitting it.

Parameters: `host_alias`, `prompt`, `submit`, `tmux_name`

### `session_history`

Return the recorded event timeline for a session (status changes, prompts, stuck, kills). Newest-first; pass `limit` to cap (default 50). Returns the events as JSON.

Parameters: `limit`, `session_id`

### `set_clipboard`

Write text to a host's system clipboard. Probes wl-copy, xclip, xsel, pbcopy in order. Capped at 64 KiB. E_CLIPBOARD_UNAVAILABLE if no clipboard helper is installed.

Parameters: `content`, `host_alias`

### `set_friendly_name`

Set the session's friendly display name (shown when the user toggles friendly names on). Called once per task by the in-session agent — short (3–6 words). Empty string clears. Returns the updated row.

Parameters: `friendly_name`, `host_alias`, `tmux_name`

### `spawn_review`

Spawn a review session: a new Claude session in the source session's worktree, seeded with a review prompt. Returns the new review session row as JSON.

Parameters: `prompt`, `source_session_id`

## Tauri IPC commands

Frontend commands registered in `src/lib.rs`:

- `commands::health::health_check`
- `commands::projects::list_projects`
- `commands::projects::refresh_projects`
- `commands::sessions::list_sessions`
- `commands::sessions::related_sessions`
- `commands::sessions::new_session`
- `commands::sessions::kill_session`
- `commands::sessions::safe_kill_session`
- `commands::sessions::inspect_safe_kill`
- `commands::sessions::discard_kill_session`
- `commands::worktrees::list_worktrees`
- `commands::worktrees::delete_worktree`
- `commands::sessions::rename_session`
- `commands::sessions::set_session_friendly_name`
- `commands::sessions::restart_session`
- `commands::sessions::send_prompt`
- `commands::sessions::spawn_review`
- `commands::sessions::recreate_session`
- `commands::sessions::dismiss_ghost_session`
- `commands::sessions::new_bg_session`
- `commands::sessions::peek_session`
- `commands::sessions::purge_project`
- `commands::files::repo_changes`
- `commands::files::repo_tree`
- `commands::files::repo_file`
- `commands::files::repo_diff`
- `commands::upload::upload_to_session`
- `commands::history::repo_log`
- `commands::history::repo_branches`
- `commands::history::repo_commit`
- `commands::history::repo_commit_diff`
- `commands::mutate::repo_checkout`
- `commands::mutate::repo_checkout_commit`
- `commands::mutate::repo_create_branch`
- `commands::mutate::repo_delete_branch`
- `commands::mutate::repo_stage`
- `commands::mutate::repo_unstage`
- `commands::mutate::repo_commit_create`
- `commands::mutate::repo_fetch`
- `commands::mutate::repo_pull`
- `commands::mutate::repo_push`
- `commands::hosts::discover_hosts`
- `commands::hosts::list_hosts`
- `commands::hosts::list_accounts`
- `commands::hosts::add_host`
- `commands::hosts::probe_host`
- `commands::hosts::probe_ssh_alias`
- `commands::hosts::remove_host`
- `commands::hosts::hide_host`
- `commands::mcp::mcp_status`
- `commands::mcp::mcp_configure`
- `commands::mcp::install_fleet_hook`
- `commands::mcp::provision_hosts`
- `pty::pty_open`
- `pty::pty_write`
- `pty::pty_resize`
- `pty::pty_close`
- `pty::pty_drain`
- `cancel_command`

