<!-- GENERATED FILE — do not edit by hand.
     Regenerate with: REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current -->

# claude-fleet Control API — Tool Reference

Auto-generated from the embedded MCP tool router. See [`control-api.md`](control-api.md) for the narrative guide.

## MCP tools

### `add_host`

Register a new SSH host. Probes it first; only persists the host if it is reachable. Returns the host row as JSON.

Parameters: `alias`, `ssh_alias`

### `broadcast_prompt`

Send the same prompt to every matching work session (excludes the controller). Returns per-session results. Rate-limited per caller (default one call per 30 s; E_RATE_LIMITED with retry_after_secs). Marked as untrusted unless raw=true (master token only). May return E_CONFIRM_REQUIRED when desktop confirmation is on.

Parameters: `confirm_nonce`, `host`, `project_id`, `prompt`, `raw`, `status`, `submit`

### `cancel_task`

Cancel a queued or running task: marks it cancelled (E_TASK_TERMINAL if it already finished). The worker session keeps running — kill or re-prompt it separately if needed. May return E_CONFIRM_REQUIRED when desktop confirmation is on. A per-host token may only cancel tasks it can see (E_FORBIDDEN).

Parameters: `confirm_nonce`, `task_id`

### `capture_session`

Capture a session's terminal output — the visible tmux pane, or include scrollback history (scrollback_lines). Use after send_prompt to read the session's reply. Returns the pane as plain text (not JSON), capped to the last max_lines lines (default 200).

Parameters: `max_lines`, `scrollback_lines`, `session_id`

### `delete_worktree`

Delete a git worktree on its host (no --force) and drop fleet's row. Refuses if an alive session points at it (override with force=true). Errors: E_WORKTREE_BUSY, E_NOTFOUND, E_GIT, E_CONFIRM_REQUIRED (desktop confirmation on).

Parameters: `confirm_nonce`, `force`, `worktree_id`

### `discover_hosts`

Discover SSH hosts from the user's ~/.ssh/config. These are candidates for add_host. Returns JSON.

### `dismiss_ghost_session`

Dismiss a ghost session (lost from tmux): permanently delete its row. Use when a ghost is not worth reviving — the row is the only thing left to clean up. Errors if the session is not a ghost.

Parameters: `session_id`

### `dispatch_task`

Dispatch a unit of work to a worker session and track it as a task. Pass worker_session_id (an existing session) OR new_worker { host_alias, project_id, name? } (spawns one via new_session). The prompt is delivered with an appended instruction to print FLEET_TASK_DONE_<nonce> on its own line followed by a one-paragraph result; fleet detects the marker on the worker's next Stop and flips the task to done with that paragraph as `result` (also delivered to requester_session_id's inbox as kind=task_result). Returns the task row (id, state=running, worker_session_id, …); follow with wait_for_task. A per-host token must name a requester on its own host. Marked as untrusted unless raw=true (master only).

Parameters: `new_worker`, `prompt`, `raw`, `requester_session_id`, `worker_session_id`

### `fleet_health`

Report claude-fleet backend health: application version, SQLite schema version, and database readiness. Returns JSON.

### `get_clipboard`

Read a host's current system clipboard (whatever a human would get from Ctrl+V on that machine). Probes wl-paste, xclip, xsel, pbpaste in order. E_CLIPBOARD_UNAVAILABLE if none is installed.

Parameters: `host_alias`

### `hide_host`

Hide or show a host. Hidden hosts are skipped during reconcile. Returns the updated host row as JSON.

Parameters: `alias`, `hidden`

### `inbox`

Read a session's inbox — messages sent TO session_id, newest-first. Slim rows by default (metadata, reply_to, 80-char body preview); pass summary=false for full bodies. Task results arrive here as kind=task_result. mark_read (default true) flips returned unread rows to read — pass false to peek without consuming. A per-host token may only read inboxes of sessions on its own host (E_FORBIDDEN).

Parameters: `limit`, `mark_read`, `session_id`, `summary`, `unread_only`

### `kill_session`

Kill a session on a host: a tmux session by name, or a background agent row (name `bg:<uuid>`) via `claude stop` — the latter is idempotent, so it also clears a stale row whose process already died. Use when the session's work is disposable or already pushed and you want it gone NOW; prefer safe_kill_session when the worktree may hold unpushed work. Returns the killed session's id. Address the session with session_id OR host_alias + name. May return E_CONFIRM_REQUIRED when desktop confirmation is on.

Parameters: `confirm_nonce`, `force`, `host_alias`, `name`, `session_id`

### `list_accounts`

List the cached Claude accounts seen across hosts. Returns JSON.

### `list_hosts`

List all registered hosts with their reachability, claude/tmux versions, and linked account. Returns JSON.

### `list_projects`

List discovered projects. Slim rows by default (id, owner, repo, worktree_count, last_session_at); pass summary=false for the full nested worktree tree.

Parameters: `summary`

### `list_sessions`

List tmux sessions across reachable hosts. Slim summary rows by default; pass summary=false for the full SessionRow. Optional filters: host_alias, project_id, status, claude_status, tag, include_lost (default false drops ghosts); `limit` caps the row count after filtering (default: all); `force` runs a reconcile pass first instead of serving the recent cache. claude_status is one of working | blocked | completed | failed | stopped | idle; stuck_kind is one of auth_menu | reconnect | trust_prompt | oom | press_enter; ci_status (full rows) is one of passing | failing | pending (null when the session has no PR or its PR has no checks).

Parameters: `claude_status`, `force`, `host_alias`, `include_lost`, `limit`, `project_id`, `status`, `summary`, `tag`

### `list_tasks`

List tasks, newest-first (default 50 rows). Filters: requester_session_id, state (queued | running | done | failed | cancelled). Read-only. A per-host token sees only tasks it requested from its host or whose worker is on its host.

Parameters: `limit`, `requester_session_id`, `state`

### `list_worktrees`

List git worktrees fleet knows about, each with its alive-session occupants (empty = free to delete via delete_worktree). Optional project filter.

Parameters: `project_id`

### `move_session`

Move a work session to another host (replaces the unbuilt Handoff): copy its Claude transcript to the target, create the worktree there from the same branch, start it with --resume so the same conversation continues, and only once the target is confirmed running kill the source (keep_source=true leaves it running). Refused unless the source worktree is clean (E_MOVE_DIRTY) and its branch is on origin with nothing unpushed (E_MOVE_UNPUSHED; it never pushes for you); a transcript over move.max_transcript_mb (default 200) is refused (E_MOVE_TOO_LARGE). Nothing on the source changes before the target is confirmed; a failure after the target started returns E_MOVE_PARTIAL and leaves both sessions. Needs a token allowed on BOTH hosts (in practice the master token). Gated by mcp.confirm_destructive (retry with confirm_nonce). Returns a JSON MoveReport: source_session_id, target_session_id, from_host, to_host, tmux_name, transcript_bytes, source_killed, warnings, target (the new row, parent_session_id = source).

Parameters: `confirm_nonce`, `keep_source`, `session_id`, `target_host_alias`

### `new_bg_session`

Launch a supervised headless (background) Claude session on a host with an initial prompt. Returns JSON with the new claude_session_id AND the fleet row (`session`, registered by an immediate reconcile; the key is absent if the agent was not matched yet — it appears on the next tick) so the next call can be peek_session { session_id }. The prompt becomes the row's default friendly name and last_prompt.

Parameters: `host_alias`, `name`, `prompt`

### `new_session`

Create a Claude Code tmux session on a host, in a project (and optional worktree). Pass new_worktree to fork a fresh worktree+branch (optional base_branch). Auto-clones the repo on remote hosts.

Parameters: `base_branch`, `host_alias`, `name`, `new_worktree`, `project_id`, `worktree_id`

### `new_shell_session`

Create a plain-shell tmux session on a host (no Claude Code in the pane — an interactive login shell). Same project/worktree plumbing as new_session, plus an optional start_command that runs once before the shell drops to an interactive prompt; the pane stays alive after it exits so you can attach or send-keys to it. Steer it with send_prompt (typed text + Enter) and read it with capture_session.

Parameters: `base_branch`, `host_alias`, `name`, `new_worktree`, `project_id`, `start_command`, `worktree_id`

### `peek_session`

Peek at a session's background Claude logs. Address it with session_id (from list_sessions) OR claude_session_id (the id new_bg_session returned; add host_alias while the fleet row does not exist yet). Returns an informational message for interactive sessions with no background job.

Parameters: `claude_session_id`, `host_alias`, `session_id`

### `peer_status`

What is a peer session doing? Returns claude_status, current_activity, stuck_kind, context_pct (plus host/name/status) for one session. Cheap pre-check before send_message or broadcast_prompt.

Parameters: `session_id`

### `probe_host`

Re-probe a registered host's reachability and versions. Returns the updated host row as JSON.

Parameters: `alias`

### `provision_hosts`

Install fleet skills, the Stop / UserPromptSubmit / EnterWorktree http hooks, and this fleet's MCP server entry (with a per-host bearer token) into every reachable host's ~/.claude.json (reverse SSH tunnel for remote hosts). rotate=true mints fresh per-host tokens. Returns a per-host status list; each host must restart Claude to load the server.

Parameters: `rotate`

### `recreate_session`

Recreate a session: kill its tmux session and rebuild it fresh in the same worktree, resuming the same Claude conversation. Use when the session is frozen, OOM-killed or out of context, or to revive a ghost — the conversation survives, the process does not. Works for running or ghost sessions. Returns the session row as JSON.

Parameters: `force`, `session_id`

### `refresh_projects`

Rescan the local projects directory for new or removed repositories and worktrees. Returns the fresh project list.

### `register_self`

Mark the calling session as the fleet controller; kill/recreate/restart refuse to target it without force. Address yourself with session_id (from whoami) OR host_alias + tmux_name. A per-host token may only register a session on its own host (E_FORBIDDEN).

Parameters: `host_alias`, `session_id`, `tmux_name`

### `related_sessions`

List sessions related to a given session — those sharing the same project and worktree. Returns JSON.

Parameters: `session_id`

### `remove_host`

Remove a registered host. Its sessions are orphaned. Returns the removed host row as JSON.

Parameters: `alias`

### `rename_session`

Rename a tmux session on a host. Returns the updated session row as JSON. Address the session with session_id OR host_alias + old_name.

Parameters: `host_alias`, `new_name`, `old_name`, `session_id`

### `repair_session`

Explicitly repair a session's workspace (the same action as the Repair workspace button): make its directory a healthy git worktree on its branch and its tmux session run there. Unlike the automatic checks on create/restart/recreate/attach (which only re-add a missing worktree git no longer lists, from its existing branch; only the opt-in reconcile tick also drops a stale entry automatically), this may unregister this worktree's own stale git entry (git worktree remove --force; never a blanket prune), adopt its branch's checkout elsewhere (refused when another fleet workspace uses it), recreate the branch from the base branch once origin confirms it is gone, run git worktree repair, and respawn a live pane whose directory vanished. No-op on a healthy session. Gated by mcp.confirm_destructive (retry with confirm_nonce). Returns a JSON RepairReport: cwd, healthy, actions (in order), warnings, branch_source, tmux (created|respawned), sibling_session_ids. Errors: E_REPO_MISSING (never faked with mkdir), E_BRANCH_CHECKED_OUT, E_WORKSPACE_LOCKED, E_REPAIR_FAILED, E_HOST_OFFLINE, E_CONFIRM_REQUIRED.

Parameters: `confirm_nonce`, `host_alias`, `name`, `session_id`

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

Commit log (branch graph) for a session's worktree. all=true (default) includes every branch. Returns a JSON array of commits with parents + ref decorations, newest first; `limit` defaults to 50 and `skip` pages through older history.

Parameters: `all`, `limit`, `session_id`, `skip`

### `repo_tree`

List a session's worktree files (tracked + untracked, gitignore respected). Returns JSON {entries, truncated}.

Parameters: `session_id`

### `restart_session`

Restart a tmux session (kill and recreate it in the same place). Use when the Claude REPL is wedged but tmux and the worktree are fine — an in-place relaunch, cheaper than recreate_session. Returns the updated session row as JSON. Address the session with session_id OR host_alias + name.

Parameters: `force`, `host_alias`, `name`, `session_id`

### `run_prompt`

send_prompt + wait_for_session(turn_gt) + session_transcript in one call: deliver the prompt, wait up to timeout_s (default 120, max 600) for the turn to complete, and return JSON { turn_seq, status: satisfied | timeout, transcript } where transcript is the reply as plain text (null with transcript_error when it cannot be read). Marked as untrusted unless raw=true (master token only). Address the session with session_id.

Parameters: `max_chars`, `prompt`, `raw`, `session_id`, `timeout_s`

### `safe_kill_session`

Ask a running Claude session to safely persist its work (commit + push), then arm deletion of its worktree + tmux session. Use when retiring a session whose worktree may hold unpushed work and you can wait for it to finish. Returns the row with safe_kill_state=requested; the actual delete fires only after the SAFE_REMOVE_READY marker AND a clean-tree check. Transitions ('ready', 'failed') arrive via row events. Address the session with session_id OR host_alias + tmux_name.

Parameters: `host_alias`, `session_id`, `tmux_name`

### `send_message`

Send a peer-to-peer message from one session to another. The message is persisted to the recipient's inbox (read with `inbox`); set `deliver: true` to ALSO type the message into the recipient's tmux pane with a `[msg #id from name@host]:` header. The inbox row is the source of truth — it lands even if the pane delivery fails. Returns JSON with the new message id and the delivery outcome. Pass reply_to (an inbox message id) to thread an answer. A per-host token must send from a session on its own host (E_FORBIDDEN). The body is prefixed with an untrusted-content marker line unless raw=true (master token only).

Parameters: `body`, `deliver`, `from_session_id`, `kind`, `raw`, `reply_to`, `submit`, `to_session_id`

### `send_prompt`

Send and SUBMIT a prompt to a running Claude session's REPL (literal text, then one Enter). This is how you steer a session. Set submit=false to stage text in the REPL without submitting it. Address the session with session_id OR host_alias + tmux_name. The first prompt to a still-unnamed session also becomes its friendly name. The text is prefixed with an untrusted-content marker line unless raw=true (master token only). Returns JSON { delivered, session_id, turn_seq_before }: pass turn_seq_before to wait_for_session { until: "turn_gt" } or session_transcript { since_turn } to collect the reply (or use run_prompt, which does all three).

Parameters: `host_alias`, `prompt`, `raw`, `session_id`, `submit`, `tmux_name`

### `session_history`

Return the recorded event timeline for a session (status changes, prompts, stuck, kills). Newest-first; pass `limit` to cap (default 50). Returns the events as JSON.

Parameters: `limit`, `session_id`

### `session_transcript`

Read a session's Claude Code transcript (the JSONL Claude writes, not the pane) and return the last assistant turn as plain text — text blocks verbatim, one summary line per tool call, no thinking. since_turn returns every turn after that turn_seq (use send_prompt's turn_seq_before). max_chars caps the text (default 8000, max 64000; the END is kept). Errors: E_INVALID_STATE (no claude_session_id yet), E_NO_TRANSCRIPT (nothing written yet). Read-only; prefer it over capture_session for the reply text.

Parameters: `max_chars`, `session_id`, `since_turn`

### `set_clipboard`

Write text to a host's system clipboard. Probes wl-copy, xclip, xsel, pbcopy in order. Capped at 64 KiB. E_CLIPBOARD_UNAVAILABLE if no clipboard helper is installed. May return E_CONFIRM_REQUIRED when desktop confirmation is on.

Parameters: `confirm_nonce`, `content`, `host_alias`

### `set_friendly_name`

Set the session's friendly display name (shown when the user toggles friendly names on). Called once per task by the in-session agent — short (3–6 words). Empty string clears. Returns the updated row. Address the session with session_id OR host_alias + tmux_name.

Parameters: `friendly_name`, `host_alias`, `session_id`, `tmux_name`

### `set_session_tags`

Replace a session's tags (short labels such as `review`, `infra`, `wip`; up to 16 of 1–32 chars from [A-Za-z0-9_.:-]; an empty list clears). Tags show in list_sessions rows and list_sessions { tag } filters on them. Returns the updated row. Address the session with session_id OR host_alias + tmux_name.

Parameters: `host_alias`, `session_id`, `tags`, `tmux_name`

### `spawn_review`

Spawn a review session: a new Claude session in the source session's worktree, seeded with a review prompt. Returns the new review session row as JSON.

Parameters: `prompt`, `source_session_id`

### `wait_for_session`

Block until a session reaches a state, or time out. until="idle": claude_status is idle | completed | stopped | failed (true even for a session that never started a turn). until="turn_gt": turn_seq > `turn` — pass the turn_seq_before that send_prompt returned to wait for the reply to YOUR prompt. Polls the store every 500 ms for up to timeout_s (default 120, max 600). Returns JSON { status: satisfied | timeout, claude_status, turn_seq, last_stop_at, stuck_kind }. Read-only. A per-host token may only wait on sessions on its own host.

Parameters: `session_id`, `timeout_s`, `turn`, `until`

### `wait_for_task`

Block until a task reaches done | failed | cancelled or timeout_s elapses (default 120, max 600; polls every 500 ms). Returns JSON { status: satisfied | timeout, task } — task.result holds the worker's paragraph when done. Read-only. A per-host token may only wait on tasks it requested or whose worker is on its host (E_FORBIDDEN).

Parameters: `task_id`, `timeout_s`

### `whoami`

Find your own fleet row from your tmux session name (`tmux display-message -p '#S'`). Returns the single matching session as JSON (id, host_alias, is_controller, …). E_NOTFOUND when fleet has not reconciled the session yet; E_AMBIGUOUS when the same name exists on several hosts — the error's details list {session_id, host_alias} candidates, pick yours and use session_id from then on.

Parameters: `tmux_name`

## Tauri IPC commands

Frontend commands registered in `src/lib.rs`:

- `commands::health::health_check`
- `commands::diagnostics::collect_diagnostics`
- `commands::diagnostics::open_log_folder`
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
- `commands::sessions::repair_session`
- `commands::sessions::rename_session`
- `commands::sessions::set_session_friendly_name`
- `commands::sessions::restart_session`
- `commands::sessions::send_prompt`
- `commands::sessions::spawn_review`
- `commands::sessions::recreate_session`
- `commands::move_session::move_session`
- `commands::sessions::dismiss_ghost_session`
- `commands::sessions::new_bg_session`
- `commands::sessions::peek_session`
- `commands::sessions::purge_project`
- `commands::sessions::get_fleet_settings`
- `commands::sessions::set_fleet_setting`
- `commands::tasks::list_tasks`
- `commands::tasks::cancel_task`
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
- `commands::mcp::list_host_tokens`
- `commands::mcp::set_host_token_mode`
- `commands::mcp::rotate_host_token`
- `commands::mcp::mcp_confirm`
- `commands::mcp::mcp_pending_confirms`
- `commands::onboarding::check_local_prereqs`
- `commands::onboarding::tunnel_status`
- `pty::pty_open`
- `pty::pty_write`
- `pty::pty_resize`
- `pty::pty_close`
- `pty::pty_drain`
- `cancel_command`

