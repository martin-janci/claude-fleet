<!-- GENERATED FILE — do not edit by hand.
     Regenerate with: REGEN_DOCS=1 cargo test -p fleet-core reference_is_current -->

# claude-fleet Control API — Tool Reference

Auto-generated from the embedded MCP tool router. See [`control-api.md`](control-api.md) for the narrative guide.

## MCP tools

### `add_host`

Register a new SSH host. Probes it first; only persists the host if it is reachable. Returns the host row as JSON.

Parameters: `alias`, `ssh_alias`

### `apply_sync`

Apply a plan from plan_sync on hosts: writes files with compare-and-swap, backs up overwritten files, merges config (files end up mode 0600), installs plugins, writes the managed manifest, then re-scans. Master token only; requires confirmation. Returns per-host results; restart_required marks hosts whose Claude must be restarted.

Parameters: `confirm_nonce`, `force_partial`, `plan_id`

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

Report claude-fleet backend health: application version, SQLite schema version, database readiness, the cached fleet roll-up, and ESTIMATED token usage and cost (micro-USD) per host and per UTC day for the last 7 days. For a per-host token the usage fields cover only its own host. Returns JSON.

### `get_clipboard`

Read a host's current system clipboard (whatever a human would get from Ctrl+V on that machine). Probes wl-paste, xclip, xsel, pbpaste in order. E_CLIPBOARD_UNAVAILABLE if none is installed.

Parameters: `host_alias`

### `hide_host`

Hide or show a host. Hidden hosts are skipped during reconcile. Returns the updated host row as JSON.

Parameters: `alias`, `hidden`

### `import_assets`

Import a host's Claude config (~/.claude skills, agents, hooks, ~/.claude.json MCP servers, installed plugins) into the catalog repo working tree as IR assets. Never overwrites; collisions are reported. Only host_alias `local` is supported. Returns the import report as JSON.

Parameters: `dry_run`, `host_alias`

### `inbox`

Read a session's inbox — messages sent TO session_id, newest-first. Slim rows by default (metadata, reply_to, 80-char body preview); pass summary=false for full bodies. Task results arrive here as kind=task_result. mark_read (default true) flips returned unread rows to read — pass false to peek without consuming. A per-host token may only read inboxes of sessions on its own host (E_FORBIDDEN).

Parameters: `limit`, `mark_read`, `session_id`, `summary`, `unread_only`

### `kill_session`

Kill a session on a host: a tmux session by name, or a background agent row (name `bg:<uuid>`, kind `bg`) via `claude stop`. An inactive background agent (claude_status `stopped`) is removed from the list instead, without `claude stop`. Rows of kind `external` (interactive Claude sessions running outside fleet) are refused with E_INVALID_STATE — close them where they run. Use when the session's work is disposable or already pushed and you want it gone NOW; prefer safe_kill_session when the worktree may hold unpushed work. Returns the killed session's id. Address the session with session_id OR host_alias + name. May return E_CONFIRM_REQUIRED when desktop confirmation is on.

Parameters: `confirm_nonce`, `force`, `host_alias`, `name`, `session_id`

### `list_accounts`

List the cached Claude accounts seen across hosts. Returns JSON.

### `list_assets`

List the asset catalog (skills, agents, hooks, MCP servers, plugin refs) with each asset's per-host drift state from the last scan, plus unmanaged assets found on hosts and catalog parse problems. Requires catalog_configure + catalog_load in the app. Returns JSON.

### `list_clients`

List the paired client devices and what each one's token may do. The stored token digest is never returned — a client's token exists in plaintext only in the one /pair response that minted it. include_revoked also returns clients whose token was revoked (kept for the audit trail). Read-only, but master token only: the list names every paired device, so it is not a phone's to read. Returns JSON rows of { id, name, mode, created_at, last_seen_at, revoked_at }.

Parameters: `include_revoked`

### `list_hosts`

List all registered hosts with their reachability, claude/tmux versions, and linked account. Returns JSON.

### `list_layers`

List the catalog's layer definitions (layers/*.yaml) and each host's role + active contexts. Read-only. Requires catalog_configure + catalog_load in the app. Returns JSON.

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

Launch a supervised headless (background) Claude session on a host with an initial prompt. Returns JSON with the new claude_session_id AND the fleet row (`session`, registered by an immediate reconcile; the key is absent if the agent was not matched yet — it appears on the next tick) so the next call can be session_transcript { session_id }. The prompt becomes the row's default friendly name and last_prompt.

Parameters: `host_alias`, `name`, `prompt`

### `new_session`

Create a Claude Code tmux session on a host, in a project (and optional worktree). Pass new_worktree to fork a fresh worktree+branch (optional base_branch). Auto-clones the repo on remote hosts.

Parameters: `base_branch`, `host_alias`, `name`, `new_worktree`, `project_id`, `worktree_id`

### `new_shell_session`

Create a plain-shell tmux session on a host (no Claude Code in the pane — an interactive login shell). Same project/worktree plumbing as new_session, plus an optional start_command that runs once before the shell drops to an interactive prompt; the pane stays alive after it exits so you can attach or send-keys to it. Steer it with send_prompt (typed text + Enter) and read it with capture_session.

Parameters: `base_branch`, `host_alias`, `name`, `new_worktree`, `project_id`, `start_command`, `worktree_id`

### `pair_client`

Mint a single-use pairing code for a new client device (a phone, a laptop browser) and return the URL to show as a QR. The code — not a token — travels in the URL FRAGMENT, so no proxy or access log ever sees it; the device posts it to the hub's /pair once and gets a token of its own back. name must be 1-64 characters with no control characters and must not be one a live client already holds. mode is full (drive sessions fleet-wide) or readonly (observe only); fleet-admin tools are out of a client's reach either way. Codes live in memory only, so a hub restart invalidates every outstanding one. Master token only. Returns JSON { url, code, expires_in_s, name, mode }.

Parameters: `mode`, `name`, `ttl_s`

### `peek_session`

Deprecated: use session_transcript. Returns the session's last assistant turn from its transcript. Address it with session_id OR claude_session_id (+ host_alias while the fleet row does not exist yet).

Parameters: `claude_session_id`, `host_alias`, `session_id`

### `peer_status`

What is a peer session doing? Returns claude_status, current_activity, stuck_kind, context_pct (plus host/name/status) for one session. Cheap pre-check before send_message or broadcast_prompt.

Parameters: `session_id`

### `plan_sync`

Compute a sync plan: scan the selected hosts, compare every catalog asset with what is installed, and return per-host actions (create | update | overwrite | adopt | remove | plugin_install | noop | blocked) plus a plan_id valid for 10 minutes. Inventory states now include orphan (in the host's fleet manifest, no longer in the catalog). Nothing is written. Pass the plan_id to apply_sync.

Parameters: `host_alias`, `kind`, `name`

### `probe_host`

Re-probe a registered host's reachability and versions. Returns the updated host row as JSON.

Parameters: `alias`

### `propose_layers`

Propose an initial layer split from the last scan, grouping assets by the exact set of hosts they are installed on. The largest group becomes 'core'; assets on a single host are returned separately for triage. Read-only: writes nothing. Returns JSON.

### `provision_hosts`

Install fleet skills, the Stop / UserPromptSubmit / EnterWorktree http hooks, and this fleet's MCP server entry (with a per-host bearer token) into every reachable host's ~/.claude.json (reverse SSH tunnel for remote hosts when the hub is loopback-only; a hub with a public URL is reached directly). rotate=true mints fresh per-host tokens. Returns a per-host status list; each host must restart Claude to load the server.

Parameters: `rotate`

### `recreate_session`

Recreate a session: kill its tmux session and rebuild it fresh in the same worktree, resuming the same Claude conversation. Use when the session is frozen, OOM-killed or out of context, or to revive a ghost — the conversation survives, the process does not. Works for running or ghost sessions. Returns the session row as JSON.

Parameters: `force`, `session_id`

### `refresh_projects`

Rescan the local projects directory for new or removed repositories and worktrees. Returns the fresh project list. On a hub with hub.local_host off it returns E_NOTFOUND: that hub has no local projects directory to scan.

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

Explicitly repair a session's workspace (the same action as the Repair workspace button): make its directory a healthy git worktree on its branch and its tmux session run there. Unlike the automatic checks on create/restart/recreate/attach (which re-add a missing worktree from its existing branch, dropping its own stale git entry first only when the parent directory's dev:inode matches the one recorded while the worktree was healthy), this may unregister this worktree's own stale git entry (git worktree remove --force; never a blanket prune), adopt its branch's checkout elsewhere (refused when another fleet workspace uses it), recreate the branch from the base branch once origin confirms it is gone, run git worktree repair, and respawn a live pane whose directory vanished. No-op on a healthy session. Gated by mcp.confirm_destructive (retry with confirm_nonce). Returns a JSON RepairReport: cwd, healthy, actions (in order), warnings, branch_source, tmux (created|respawned), sibling_session_ids. Errors: E_REPO_MISSING (never faked with mkdir), E_BRANCH_CHECKED_OUT, E_WORKSPACE_LOCKED, E_REPAIR_FAILED, E_HOST_OFFLINE, E_CONFIRM_REQUIRED.

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

### `resolve_preview`

Compute the effective asset set for one host after its role and contexts are resolved, with provenance: which layer introduced each asset, which layers overrode it, and which layer excluded anything missing. Nothing is written. Requires catalog_configure + catalog_load in the app. Returns JSON.

Parameters: `host_alias`

### `restart_session`

Restart a tmux session (kill and recreate it in the same place). Use when the Claude REPL is wedged but tmux and the worktree are fine — an in-place relaunch, cheaper than recreate_session. Returns the updated session row as JSON. Address the session with session_id OR host_alias + name.

Parameters: `force`, `host_alias`, `name`, `session_id`

### `revoke_client`

Revoke a paired client's token by name. Its next request is refused (the auth layer only resolves live rows) and the name becomes free to pair again; the row itself is kept, revoked, for the audit trail. E_NOTFOUND when no live client holds that name. Master token only. Returns the revoked row as JSON.

Parameters: `name`

### `run_prompt`

send_prompt + wait_for_session(turn_gt) + session_transcript in one call: deliver the prompt, wait up to timeout_s (default 120, max 600) for the turn to complete, and return JSON { turn_seq, status: satisfied | timeout, transcript } where transcript is the reply as plain text (null with transcript_error when it cannot be read). Marked as untrusted unless raw=true (master token only). Address the session with session_id.

Parameters: `max_chars`, `prompt`, `raw`, `session_id`, `timeout_s`

### `safe_kill_session`

Ask a running Claude session to safely persist its work (commit + push), then arm deletion of its worktree + tmux session. Use when retiring a session whose worktree may hold unpushed work and you can wait for it to finish. Returns the row with safe_kill_state=requested; the actual delete fires only after the SAFE_REMOVE_READY marker AND a clean-tree check. Transitions ('ready', 'failed') arrive via row events. Address the session with session_id OR host_alias + tmux_name.

Parameters: `host_alias`, `session_id`, `tmux_name`

### `scan_assets`

Scan hosts for installed skills/agents/hooks/MCP servers/plugins and recompute each catalog asset's state (in_sync | drifted | missing | unmanaged | unsupported | orphan). Read-only on hosts. Returns per-host results as JSON.

Parameters: `host_alias`

### `send_message`

Send a peer-to-peer message from one session to another. The message is persisted to the recipient's inbox (read with `inbox`); set `deliver: true` to ALSO type the message into the recipient's tmux pane with a `[msg #id from name@host]:` header. The inbox row is the source of truth — it lands even if the pane delivery fails. Returns JSON with the new message id and the delivery outcome. Pass reply_to (an inbox message id) to thread an answer. A per-host token must send from a session on its own host (E_FORBIDDEN). The body is prefixed with an untrusted-content marker line unless raw=true (master token only).

Parameters: `body`, `deliver`, `from_session_id`, `kind`, `raw`, `reply_to`, `submit`, `to_session_id`

### `send_prompt`

Send and SUBMIT a prompt to a running Claude session's REPL (literal text, then one Enter). This is how you steer a session. Set submit=false to stage text in the REPL without submitting it. Address the session with session_id OR host_alias + tmux_name. The first prompt to a still-unnamed session also becomes its friendly name. The text is prefixed with an untrusted-content marker line unless raw=true (master token only). Returns JSON { delivered, session_id, turn_seq_before }: pass turn_seq_before to wait_for_session { until: "turn_gt" } or session_transcript { since_turn } to collect the reply (or use run_prompt, which does all three).

Parameters: `host_alias`, `prompt`, `raw`, `session_id`, `submit`, `tmux_name`

### `session_conversation`

Read a session's recent conversation as structured turns: each turn carries the human prompt, its timestamp, the turn's end timestamp, and items tagged by kind: text (assistant text), tool (a one-line tool summary, flagged when that call failed), compact (a context compaction with its trigger, pre-compaction tokens and summary), command (a slash command with its args and output) or interrupt (the user interrupted the turn). The response also carries events (this conversation's timeline events, oldest first, at most the newest 50) and context (the conversation's context-window usage, or null). turns defaults to 10 and is capped at 100; the character budget scales with it. Prefer this over session_transcript when you want the shape of the exchange rather than one flat blob. Pass claude_session_id (from session_conversations) to read an earlier conversation of the session instead of the current one. Read-only. Errors: E_INVALID (claude_session_id is not one of the session's conversations), E_INVALID_STATE (no claude_session_id yet), E_NO_TRANSCRIPT (nothing written yet).

Parameters: `claude_session_id`, `session_id`, `turns`

### `session_conversations`

List the Claude Code conversations a session has run, newest first: claude_session_id, started_at, ended_at, start_source (startup, resume, clear, compact, fork, fleet, unknown), end_reason, model, first_prompt, turns, compactions and current. Pass a claude_session_id to session_conversation to read an earlier one. limit defaults to 20 (max 500). Read-only. A per-host token may only list sessions on its own host.

Parameters: `limit`, `session_id`

### `session_history`

Return the recorded event timeline for a session (status changes, prompts, stuck, kills, and conversation events: conversation_started, conversation_ended, compact_started, compact_done, turn_done). Newest-first; pass `limit` to cap (default 50). Returns the events as JSON.

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

### `set_host_layers`

Replace a host's layer assignment: one optional role plus context layers in application order. Edits fleet state only, never catalog files. Requires catalog_configure + catalog_load in the app. Master token only. Returns the host's new assignment as JSON.

Parameters: `contexts`, `host_alias`, `role`

### `set_secret`

Store a value for a ${NAME} placeholder used by the catalog (global, or a per-host override with host_alias). Master token only. The value is never returned or logged.

Parameters: `host_alias`, `name`, `value`

### `set_session_tags`

Replace a session's tags (short labels such as `review`, `infra`, `wip`; up to 16 of 1–32 chars from [A-Za-z0-9_.:-]; an empty list clears). Tags show in list_sessions rows and list_sessions { tag } filters on them. Returns the updated row. Address the session with session_id OR host_alias + tmux_name.

Parameters: `host_alias`, `session_id`, `tags`, `tmux_name`

### `spawn_review`

Spawn a review session: a new Claude session in the source session's worktree, seeded with a review prompt. Returns the new review session row as JSON.

Parameters: `prompt`, `source_session_id`

### `usage_report`

Report ESTIMATED token usage and cost per session, host and UTC day, summed from each session's Claude Code transcript (collected every usage.interval_secs). Costs are micro-USD from a built-in per-model price table (override: usage.prices_json), not a bill. total and by_host sum the live session rows, each over its whole lifetime; by_day comes from the durable daily roll-up (killed sessions included). Optional host_alias; since_secs keeps only sessions whose usage changed in the last N seconds and scopes by_day to that window (default: every session, last 30 days). Sessions are sorted by cost, at most 200. A per-host token only sees its own host. Returns JSON.

Parameters: `host_alias`, `since_secs`

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
- `commands::projects::add_project`
- `commands::projects::list_github_repos`
- `commands::sessions::list_sessions`
- `commands::sessions::related_sessions`
- `commands::sessions::new_session`
- `commands::sessions::kill_session`
- `commands::sessions::safe_kill_session`
- `commands::sessions::inspect_safe_kill`
- `commands::sessions::discard_kill_session`
- `commands::worktrees::list_worktrees`
- `commands::worktrees::list_host_worktrees`
- `commands::worktrees::delete_worktree`
- `commands::sessions::repair_session`
- `commands::sessions::rename_session`
- `commands::sessions::set_session_friendly_name`
- `commands::sessions::session_history`
- `commands::sessions::session_conversations`
- `commands::sessions::session_conversation`
- `commands::sessions::session_activity`
- `commands::sessions::restart_session`
- `commands::sessions::send_prompt`
- `commands::sessions::spawn_review`
- `commands::sessions::recreate_session`
- `commands::move_session::move_session`
- `commands::sessions::dismiss_ghost_session`
- `commands::sessions::dismiss_agent_session`
- `commands::sessions::new_bg_session`
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
- `commands::hosts::set_account_nickname`
- `commands::account_usage::list_account_usage`
- `commands::account_usage::refresh_account_usage`
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
- `commands::assets::catalog_config`
- `commands::assets::catalog_configure`
- `commands::assets::catalog_load`
- `commands::assets::catalog_list_assets`
- `commands::assets::catalog_get_asset`
- `commands::assets::catalog_list_layers`
- `commands::assets::catalog_resolve_preview`
- `commands::assets::catalog_propose_layers`
- `commands::assets::catalog_set_host_layers`
- `commands::assets::catalog_layer_template`
- `commands::assets::catalog_write_layer`
- `commands::assets::catalog_delete_layer`
- `commands::assets::catalog_import_host`
- `commands::assets::assets_scan_hosts`
- `commands::assets::assets_inventory`
- `commands::assets::catalog_plan_sync`
- `commands::assets::catalog_apply_sync`
- `commands::assets::catalog_last_sync`
- `commands::assets::catalog_list_secrets`
- `commands::assets::catalog_set_secret`
- `commands::assets::catalog_delete_secret`
- `commands::assets::catalog_create_asset`
- `commands::assets::catalog_update_asset`
- `commands::assets::catalog_delete_asset`
- `commands::assets::catalog_add_resource`
- `commands::assets::catalog_remove_resource`
- `commands::assets::catalog_lint_asset`
- `commands::assets::catalog_lint_all`
- `commands::assets::catalog_commit_pending`
- `commands::assets::catalog_push`
- `commands::assets::catalog_repo_status`
- `commands::assets::catalog_template`
- `commands::assets::catalog_spawn_author_session`
- `pty::pty_open`
- `pty::pty_write`
- `pty::pty_resize`
- `pty::pty_close`
- `pty::pty_drain`
- `cancel_command`

