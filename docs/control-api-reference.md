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

### `discover_hosts`

Discover SSH hosts from the user's ~/.ssh/config. These are candidates for add_host. Returns JSON.

### `dismiss_ghost_session`

Dismiss a ghost session (lost from tmux): permanently delete its row. Errors if the session is not a ghost.

Parameters: `session_id`

### `fleet_health`

Report claude-fleet backend health: application version, SQLite schema version, and database readiness. Returns JSON.

### `hide_host`

Hide or show a host. Hidden hosts are skipped during reconcile. Returns the updated host row as JSON.

Parameters: `alias`, `hidden`

### `kill_session`

Kill a tmux session on a host. Returns the killed session's id.

Parameters: `force`, `host_alias`, `name`

### `list_accounts`

List the cached Claude accounts seen across hosts. Returns JSON.

### `list_hosts`

List all registered hosts with their reachability, claude/tmux versions, and linked account. Returns JSON.

### `list_projects`

List all discovered projects with their worktrees. Returns JSON.

### `list_sessions`

Reconcile and list all tmux sessions across every reachable host. This is the primary way to see fleet state. Each row carries an is_controller flag (true for the registered controller session). JSON.

### `new_bg_session`

Launch a supervised headless (background) Claude session on a host with an initial prompt. Returns the new Claude session id as JSON; track progress with peek_session.

Parameters: `host_alias`, `name`, `prompt`

### `new_session`

Create a new Claude Code tmux session on a host, in the given project (and optional worktree). Auto-clones the repo on remote hosts if missing. Returns the new session row as JSON.

Parameters: `host_alias`, `name`, `project_id`, `worktree_id`

### `peek_session`

Peek at a session's background Claude logs. Returns an informational message for interactive sessions with no background job.

Parameters: `session_id`

### `probe_host`

Re-probe a registered host's reachability and versions. Returns the updated host row as JSON.

Parameters: `alias`

### `provision_hosts`

Install the claude-fleet-control skill and register this fleet's MCP server into every reachable host's Claude config (~/.claude.json), with a reverse SSH tunnel for remote hosts. Returns a per-host status list. Each host must restart Claude to load the server.

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

### `send_prompt`

Send and SUBMIT a prompt to a running Claude session's REPL (literal text, then one Enter). This is how you steer a session. Set submit=false to stage text in the REPL without submitting it.

Parameters: `host_alias`, `prompt`, `submit`, `tmux_name`

### `session_history`

Return the recorded event timeline for a session (status changes, prompts, stuck, kills). Newest-first; pass `limit` to cap (default 50). Returns the events as JSON.

Parameters: `limit`, `session_id`

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
- `commands::sessions::rename_session`
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

