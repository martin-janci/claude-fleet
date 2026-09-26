<!-- GENERATED FILE — do not edit by hand.
     Regenerate with: REGEN_DOCS=1 cargo test -p fleet-core reference_is_current -->

# claude-fleet Control API — Tool Reference

Auto-generated from the embedded MCP tool router. See [`control-api.md`](control-api.md) for the narrative guide.

## MCP tools

### `add_host`

Register a host. transport "ssh" (default) is probed first and persisted only if reachable; "agent" (a host the hub cannot reach; it runs fleet-agent and dials in) is persisted unprobed, unreachable until its agent connects (token: `fleet-hub agent-token <alias>` on the hub). Returns the host row.

Parameters: `alias`, `ssh_alias`, `transport`

### `agent_status`

Which agent hosts (transport "agent") have a fleet-agent connected: since (unix s), version, host name, OS. Offline ones show connected=false; a call for one fails fast with E_AGENT_OFFLINE. enabled=false where no agents are accepted (the desktop).

### `apply_sync`

Apply a plan_sync plan: writes files with compare-and-swap, backs up overwritten files, merges config (files end up mode 0600), installs plugins, writes the managed manifest, then re-scans. Master token only; requires confirmation. restart_required marks hosts whose Claude must be restarted.

Parameters: `confirm_nonce`, `force_partial`, `plan_id`

### `broadcast_prompt`

Send one prompt to every matching work session (not the controller), skipping blocked or stuck ones unless status="blocked". Returns per-session results. Rate-limited per caller (default one call per 30 s; E_RATE_LIMITED with retry_after_secs). Marked untrusted unless raw=true (master token only). May return E_CONFIRM_REQUIRED when desktop confirmation is on.

Parameters: `confirm_nonce`, `host`, `project_id`, `prompt`, `raw`, `status`, `submit`

### `cancel_task`

Cancel a queued or running task (E_TASK_TERMINAL if it already finished). The worker session keeps running: kill or re-prompt it separately. May return E_CONFIRM_REQUIRED when desktop confirmation is on. A per-host token may only cancel tasks it can see (E_FORBIDDEN).

Parameters: `confirm_nonce`, `task_id`

### `capture_session`

Capture a session's terminal: the visible tmux pane, plus scrollback_lines of history. Use after send_prompt to read the reply. Returns plain text, not JSON.

Parameters: `max_lines`, `scrollback_lines`, `session_id`

### `delete_worktree`

Delete a git worktree on its host (no --force) and drop fleet's row. Refuses if an alive session points at it, unless force. Errors: E_WORKTREE_BUSY, E_NOTFOUND, E_GIT, E_CONFIRM_REQUIRED (desktop confirmation on).

Parameters: `confirm_nonce`, `force`, `worktree_id`

### `discover_hosts`

SSH hosts in the user's ~/.ssh/config: candidates for add_host.

### `discover_lost_sessions`

Read-only: scan ~/.claude/projects on a host for conversations fleet has no live pane for, ranked against the host's boot (rank_hint: before_boot | after_boot | stale | unknown), with project_id, worktree_id and existing_session_id (set when a fleet row, live or lost, holds it: restore_host_sessions handles that one). resumable is true only when the pane would start in exactly the transcript's cwd; anywhere else claude --resume silently starts an empty conversation, so do not resume it. Resume with new_session { host_alias, project_id, worktree_id, name: derived_tmux_name (a hint), resume_claude_session_id }.

Parameters: `host_alias`, `limit`

### `dismiss_ghost_session`

Permanently delete a ghost session's row (lost from tmux, not worth reviving). Errors if it is not a ghost.

Parameters: `session_id`

### `dispatch_task`

Dispatch work to a worker session and track it as a task. The prompt gets an appended instruction to print FLEET_TASK_DONE_<nonce> on its own line followed by a one-paragraph result; on the worker's next Stop fleet flips the task to done with that paragraph as `result` (also sent to the requester's inbox as kind=task_result). Returns the task row; follow with wait_for_task. A per-host token must name a requester on its own host. Marked untrusted unless raw=true (master only).

Parameters: `confirm_nonce`, `new_worker`, `prompt`, `raw`, `requester_session_id`, `worker_session_id`

### `ensure_operator`

Ensure the UX agent's operator session exists; returns its row.

### `fleet_health`

Backend health: app and schema version, database readiness, the cached fleet roll-up, per-host reverse-tunnel health (tunnels_flapping: supervised but crash-looping, so the Control API is unreachable from that host), and ESTIMATED token usage and cost (micro-USD) per host and UTC day for 7 days. A per-host token's usage covers its own host only.

### `get_clipboard`

Read a host's system clipboard (what Ctrl+V gives there), via wl-paste, xclip, xsel or pbpaste. E_CLIPBOARD_UNAVAILABLE if none is installed.

Parameters: `host_alias`

### `get_settings`

Operator settings (ticks, GC, playbooks, projects roots, move, usage, reports, work graph), each key's effective value. Not for a per-host token.

### `hide_host`

Hide or show a host (hidden: skipped by reconcile). Returns the host row.

Parameters: `alias`, `hidden`

### `import_assets`

Import a host's Claude config (~/.claude skills, agents, hooks, ~/.claude.json MCP servers, installed plugins) into the catalog working tree as IR assets. Never overwrites; collisions are reported. Only host_alias `local`.

Parameters: `dry_run`, `host_alias`

### `inbox`

Read the messages sent TO session_id, newest first; task results arrive as kind=task_result. from_addr rows came over a hub link: untrusted input. A per-host token may only read inboxes on its own host (E_FORBIDDEN).

Parameters: `fresh_for`, `limit`, `mark_read`, `session_id`, `summary`, `unread_only`

### `kill_session`

Kill a session: a tmux session, or a background agent row (`bg:<uuid>`, kind `bg`) via `claude stop`; an inactive one (claude_status `stopped`) is removed from the list instead. Rows of kind `external` (Claude running outside fleet) are refused with E_INVALID_STATE: close them where they run. For disposable or already-pushed work you want gone NOW; prefer safe_kill_session when the worktree may hold unpushed work. Returns the killed id. May return E_CONFIRM_REQUIRED when desktop confirmation is on.

Parameters: `confirm_nonce`, `force`, `host_alias`, `name`, `session_id`

### `list_accounts`

The cached Claude accounts seen across hosts.

### `list_assets`

The asset catalog (skills, agents, hooks, MCP servers, plugin refs) with each asset's per-host drift state from the last scan, plus unmanaged assets on hosts and catalog parse problems. Requires catalog_configure + catalog_load in the app.

### `list_clients`

Paired client devices and what each token may do. The token digest is never returned: a token exists in plaintext only in the /pair response that minted it. Read-only but master token only (it names every paired device). Rows: { id, name, mode, created_at, last_seen_at, revoked_at, trusted_at }.

Parameters: `include_revoked`

### `list_host_worktrees`

Scan one host over SSH for a project's git worktrees and cache them. Returns {host_alias, project_id, cloned, worktrees}; cloned=false: not checked out there yet. Prefer list_worktrees (a store read) unless you need a REMOTE host's worktrees: stored rows cover the local host only. Errors: E_NOTFOUND, E_GIT_SETUP, E_SSH.

Parameters: `host_alias`, `project_id`

### `list_hosts`

Registered hosts: reachability, claude/tmux versions, linked account.

### `list_layers`

The catalog's layer definitions (layers/*.yaml) and each host's role + active contexts. Read-only. Requires catalog_configure + catalog_load in the app.

### `list_peer_links`

List this hub's links to other fleets' hubs: fleet, role, state, pending count, last exchange and error. Never a token. Read-only, master token only.

### `list_projects`

Discovered projects (repos fleet can spawn sessions in).

Parameters: `has_sessions`, `limit`, `summary`

### `list_sessions`

List tmux sessions across reachable hosts: slim rows unless summary=false; the filters combine. claude_status is one of working | blocked | completed | failed | stopped | idle; stuck_kind is one of auth_menu | reconnect | trust_prompt | oom | press_enter; ci_status (full rows) is one of passing | failing | pending (null without a PR or checks).

Parameters: `claude_status`, `force`, `fresh_for`, `host_alias`, `include_lost`, `limit`, `needs_attention`, `project_id`, `status`, `summary`, `tag`, `view`

### `list_tasks`

Tasks, newest first. Read-only. A per-host token sees only tasks it requested from its host or whose worker is on its host.

Parameters: `limit`, `requester_session_id`, `state`

### `list_worktrees`

Git worktrees fleet knows about, with their alive-session occupants (0 = free to delete via delete_worktree). Returns {total, worktrees}: total counts every match. Narrow with project_id / host_alias: a fleet-wide call answers hundreds of rows.

Parameters: `host_alias`, `limit`, `project_id`, `summary`

### `move_session`

Move a work session to another host, carrying its work as it is: the transcript, unpushed commits, staged/modified/untracked and small git-ignored files (.env), plus the session's Claude directory and the project's Claude memory (added without replacing anything; these two only warn). Nothing is pushed, committed or stashed and the source worktree is never modified; the target resumes the conversation and the source is killed only once the target runs. strict refuses instead of carrying. Errors: E_MOVE_MIDOP, E_MOVE_TARGET_DIRTY, E_MOVE_TOO_LARGE, E_MOVE_CARRY, E_MOVE_PARTIAL (target started, both sessions left), E_CONFIRM_REQUIRED, E_FORBIDDEN (cross-org; see force_cross_org). Needs a token allowed on BOTH hosts (in practice the master). Returns a moved report, a preview or a wait.

Parameters: `clean_target`, `confirm_nonce`, `dry_run`, `force_cross_org`, `keep_source`, `session_id`, `strict`, `target_host_alias`, `when`

### `new_bg_session`

Launch a supervised headless (background) Claude session on a host with an initial prompt, which becomes its default friendly name. Returns the claude_session_id AND the fleet row (`session`; absent until reconcile matches it, on the next tick) for session_transcript { session_id }.

Parameters: `confirm_nonce`, `host_alias`, `name`, `prompt`, `requester_session_id`

### `new_session`

Create a Claude Code tmux session on a host, in a project (and optional worktree, or a fresh one with new_worktree). Auto-clones the repo on remote hosts.

Parameters: `base_branch`, `confirm_nonce`, `friendly_name`, `host_alias`, `kind`, `name`, `new_worktree`, `project_id`, `resume_claude_session_id`, `start_command`, `worktree_id`

### `new_shell_session`

Create a plain-shell tmux session (an interactive login shell, no Claude Code): new_session's project/worktree plumbing plus a start_command. Steer it with send_prompt and read it with capture_session.

Parameters: `base_branch`, `confirm_nonce`, `host_alias`, `name`, `new_worktree`, `project_id`, `start_command`, `worktree_id`

### `operator_status`

Whether the UX agent can work, and why not: absent|lost|no_mcp|token_revoked|no_host.

### `pair_client`

Mint a single-use pairing code for a new client device (phone, browser) and return the URL to show as a QR. The code (not a token) travels in the URL FRAGMENT, so no proxy or access log sees it; the device posts it to /pair once for a token of its own. name: 1-64 chars, no control characters, not a live client's. mode full drives sessions fleet-wide, readonly observes, peer is another hub's link (see peer_exchange); fleet-admin tools stay out of a client's reach. Codes are in memory only: a hub restart voids them. Master token only. Returns { url, code, expires_in_s, name, mode, trusted }.

Parameters: `mode`, `name`, `trusted`, `ttl_s`

### `peer_exchange`

Hub-to-hub link exchange (peer tokens only): deliver messages and acks, receive this hub's messages for the caller. Long-polls up to wait_ms. See docs/hub.md, Link two hubs.

Parameters: `after`, `fleet_id`, `proto`, `results`, `send`, `wait_ms`

### `peer_status`

What is a peer session doing: claude_status, current_activity, stuck_kind, context_pct (plus host/name/status). Cheap check before send_message or broadcast_prompt.

Parameters: `session_id`

### `plan_sync`

Compute a sync plan: scan the hosts, compare every catalog asset with what is installed, and return per-host actions (create | update | overwrite | adopt | remove | plugin_install | plugin_update | noop | blocked) plus a plan_id for apply_sync. plugin_update fires when a pinned plugin's catalog version changes; a host left on the old version stays blocked. orphan: in the host's fleet manifest, no longer in the catalog. Nothing is written.

Parameters: `host_alias`, `kind`, `name`

### `probe_host`

Re-probe a host's reachability and versions. Returns the host row.

Parameters: `alias`

### `propose_layers`

Propose a layer split from the last scan, grouping assets by the exact set of hosts they are on: the largest group becomes 'core'; single-host assets come back separately for triage. Read-only.

### `provision_hosts`

Install fleet skills, the Stop / UserPromptSubmit / EnterWorktree http hooks and this fleet's MCP server entry (per-host bearer token) into every reachable host's ~/.claude.json (a reverse SSH tunnel when the hub is loopback-only). Returns per-host status; each host must restart Claude to load it.

Parameters: `rotate`

### `recreate_session`

Recreate a session: kill its tmux session and rebuild it in the same worktree, resuming the same Claude conversation. For a frozen, OOM-killed or out-of-context session, or to revive a ghost: the conversation survives, the process does not. Returns the row.

Parameters: `confirm_nonce`, `force`, `session_id`

### `refresh_projects`

Rescan the local projects directory for new or removed repositories and worktrees; returns the project list. E_NOTFOUND on a hub with hub.local_host off (no local projects directory).

### `register_self`

Mark the calling session as the fleet controller; kill/recreate/restart refuse to target it without force. A per-host token may only register a session on its own host (E_FORBIDDEN).

Parameters: `host_alias`, `session_id`, `tmux_name`

### `related_sessions`

Sessions sharing this session's project and worktree.

Parameters: `session_id`

### `remove_host`

Remove a host; its sessions are orphaned. Returns the removed row.

Parameters: `alias`

### `rename_session`

Rename a tmux session. Returns the updated row.

Parameters: `host_alias`, `new_name`, `old_name`, `session_id`

### `repair_session`

Repair a session workspace (the Repair workspace button): make its directory a healthy git worktree on its branch with its tmux session running there. Goes past the automatic checks: may unregister a stale worktree entry, adopt its branch checkout elsewhere, recreate the branch from base once origin confirms it is gone, and respawn a pane whose directory vanished. No-op when healthy. Call it after any tool answers E_REPAIR_REQUIRED, then retry that tool. Returns a RepairReport. Errors: E_REPO_MISSING, E_BRANCH_CHECKED_OUT, E_WORKSPACE_LOCKED, E_REPAIR_FAILED, E_HOST_OFFLINE, E_CONFIRM_REQUIRED (gated by mcp.confirm_destructive).

Parameters: `confirm_nonce`, `host_alias`, `name`, `session_id`

### `repo_branches`

Local + remote branches of a session's worktree, with ahead/behind.

Parameters: `session_id`

### `repo_changes`

A session's changed files (git status) in its worktree.

Parameters: `session_id`

### `repo_commit`

One commit's metadata + changed files: {hash, subject, body, author, date, files}.

Parameters: `hash`, `session_id`

### `repo_commit_diff`

One file's diff within a commit: {path, diff, binary, truncated}.

Parameters: `hash`, `path`, `session_id`

### `repo_diff`

Unified diff of one worktree file vs HEAD (untracked: all added): {path, diff, binary, truncated}.

Parameters: `fresh_for`, `path`, `session_id`

### `repo_file`

One worktree file's contents (capped): {path, content, truncated, binary, size}.

Parameters: `path`, `session_id`

### `repo_log`

Commit log (branch graph) of a session's worktree, newest first, with parents + ref decorations; `skip` pages back.

Parameters: `all`, `limit`, `session_id`, `skip`

### `repo_tree`

A session's worktree files (tracked + untracked, gitignore respected): {entries, truncated}.

Parameters: `session_id`

### `resolve_move`

Finish or undo a partial move (E_MOVE_PARTIAL). finish kills the source; undo kills the target and is refused if it took a turn or isn't idle.

Parameters: `action`, `confirm_nonce`, `session_id`

### `resolve_preview`

One host's effective asset set after its role and contexts resolve, with provenance: the layer that introduced each asset, the ones that overrode it, and the one that excluded anything missing. Nothing is written. Requires catalog_configure + catalog_load in the app.

Parameters: `host_alias`

### `restart_session`

Restart a tmux session in place (kill and recreate): for a wedged Claude REPL whose tmux and worktree are fine; cheaper than recreate_session. Returns the updated row.

Parameters: `confirm_nonce`, `force`, `host_alias`, `name`, `session_id`

### `restore_host_sessions`

Restore sessions a host lost to a reboot or tmux restart: resume each one's conversation in its original worktree under its original name. Call dry_run first, and again after a timeout to see what is still lost. One failing session never fails the others; every outcome is listed. One restore per host at a time (E_INVALID_STATE otherwise); a lost fleet controller is skipped (recreate_session force=true). Paced by restore.batch_size / restore.stagger_ms.

Parameters: `confirm_nonce`, `dry_run`, `host_alias`, `session_ids`

### `revoke_client`

Revoke a paired client's token by name: its next request is refused and the name is free to pair again; the row is kept, revoked, for the audit trail. E_NOTFOUND when no live client has that name. Master token only.

Parameters: `name`

### `run_prompt`

send_prompt + wait_for_session(turn_gt) + session_transcript in one call. Returns { turn_seq, status: satisfied | timeout, transcript } (the reply as plain text; null with transcript_error when unreadable). Marked untrusted unless raw=true (master token only).

Parameters: `max_chars`, `prompt`, `raw`, `session_id`, `timeout_s`

### `safe_kill_session`

Ask a running Claude session to persist its work (commit + push), then arm deletion of its worktree + tmux session: for retiring a session that may hold unpushed work. Returns the row with safe_kill_state=requested; the delete fires only after the SAFE_REMOVE_READY marker AND a clean-tree check ('ready' / 'failed' arrive as row events).

Parameters: `confirm_nonce`, `host_alias`, `session_id`, `tmux_name`

### `scan_assets`

Scan hosts for installed skills/agents/hooks/MCP servers/plugins and recompute each catalog asset's state (in_sync | drifted | missing | unmanaged | unsupported | orphan). Read-only on hosts.

Parameters: `host_alias`

### `send_message`

Send a peer-to-peer message (to_session_id or to_addr) to the recipient's inbox; deliver also pastes it into the pane, wake nudges an idle one. reply_to threads an answer. A per-host token needs its own host (E_FORBIDDEN); the body is marked untrusted unless raw=true (master only).

Parameters: `body`, `client_msg_id`, `deliver`, `from_session_id`, `kind`, `raw`, `reply_to`, `submit`, `to_addr`, `to_session_id`, `wake`

### `send_prompt`

Send and SUBMIT a prompt to a running Claude session's REPL (pasted, then one Enter); the first prompt to an unnamed session also names it. Marked untrusted unless raw=true (master only) or a trusted client. keys presses a key instead. Returns { delivered, session_id, turn_seq_before, queued, acked }: pass turn_seq_before to wait_for_session { until: "turn_gt" } or session_transcript { since_turn } for the reply (run_prompt does all three). Refuses a blocked or stuck session (E_INVALID_STATE) unless force=true; a working one queues it. acked: true = hook-confirmed, false = not within 1.5 s (check capture_session), null = unknowable.

Parameters: `client_msg_id`, `force`, `host_alias`, `keys`, `prompt`, `raw`, `session_id`, `submit`, `tmux_name`

### `session_activity`

What the session's pane shows now: claude_status, stuck_kind, current_activity, waiting_for and the spinner line. One capture, nothing stored: the cheap read behind a live indicator (capture_session is the whole pane). E_INVALID_STATE outside tmux.

Parameters: `session_id`

### `session_conversation`

Read a session conversation as structured turns (session_transcript is one flat blob): each turn's prompt, timestamps and items by kind (text, tool, subagent, compact, command, interrupt; tool inputs and results are never included), plus events (the conversation timeline) and context (context-window usage, or null). Read-only. Errors: E_INVALID, E_INVALID_STATE, E_NO_TRANSCRIPT.

Parameters: `claude_session_id`, `events_limit`, `session_id`, `since_turn`, `turns`

### `session_conversations`

The Claude conversations a session has run, newest first: claude_session_id, started_at, ended_at, start_source (startup, resume, clear, compact, fork, fleet, unknown), end_reason, model, first_prompt, turns, compactions, current. Read an earlier one with session_conversation. Read-only; a per-host token only for sessions on its own host.

Parameters: `limit`, `session_id`

### `session_history`

A session's recorded event timeline, newest first: status changes, prompts, stuck, kills, and conversation events (conversation_started, conversation_ended, compact_started, compact_done, turn_done).

Parameters: `fresh_for`, `limit`, `session_id`

### `session_transcript`

Read a session's Claude Code transcript (the JSONL, not the pane): the last assistant turn as plain text, text blocks verbatim, one line per tool call, no thinking. Errors: E_INVALID_STATE (no claude_session_id yet), E_NO_TRANSCRIPT (nothing written yet). Read-only; prefer it over capture_session for the reply. unchanged costs no transcript read.

Parameters: `fresh_for`, `max_chars`, `session_id`, `since_turn`

### `set_client_trust`

Grant or withdraw trust in a paired client by name: a trusted client's prompts and messages are delivered without the untrusted-content marker, like the master's raw=true. Trust a device you type on, never an agent's token. E_NOTFOUND for an unknown live name. Master token only.

Parameters: `name`, `trusted`

### `set_clipboard`

Write a host's system clipboard, via wl-copy, xclip, xsel or pbcopy. E_CLIPBOARD_UNAVAILABLE if none is installed. May return E_CONFIRM_REQUIRED when desktop confirmation is on.

Parameters: `confirm_nonce`, `content`, `host_alias`

### `set_friendly_name`

Set the session's friendly display name, once per task by the in-session agent (3–6 words; empty clears). Returns the updated row.

Parameters: `friendly_name`, `host_alias`, `session_id`, `tmux_name`

### `set_host_layers`

Replace a host's layer assignment: one optional role plus context layers. Edits fleet state only, never catalog files. Requires catalog_configure + catalog_load in the app. Master token only.

Parameters: `contexts`, `host_alias`, `role`

### `set_secret`

Store a value for a catalog ${NAME} placeholder (global, or per host with host_alias). Master token only. The value is never returned or logged.

Parameters: `host_alias`, `name`, `value`

### `set_session_tags`

Replace a session's tags (short labels such as `review`, `wip`; up to 16 of 1–32 chars from [A-Za-z0-9_.:-]), shown and filterable in list_sessions. Returns the updated row.

Parameters: `host_alias`, `session_id`, `tags`, `tmux_name`

### `set_setting`

Change one get_settings key, validated; E_INVALID otherwise. mcp.*, hub.* and controller.* are refused. Not for a per-host token. Returns the settings.

Parameters: `key`, `value`

### `spawn_review`

Spawn a review session: a new Claude session in the source session's worktree, seeded with a review prompt. Returns its row.

Parameters: `confirm_nonce`, `prompt`, `source_session_id`

### `usage_report`

ESTIMATED token usage and cost per session, host and UTC day, from each session's transcript (collected every usage.interval_secs). Costs are micro-USD from a built-in price table (usage.prices_json), not a bill. total and by_host sum live rows over their lifetime; by_day is the durable daily roll-up (killed sessions included). Sessions sorted by cost, at most 200. A per-host token only sees its own host.

Parameters: `host_alias`, `since_secs`

### `wait_for_reply`

Block until the next message arrives for a session, or timeout_s (instead of polling inbox). Returns { status: satisfied | timeout, message }. Read-only.

Parameters: `after_message_id`, `session_id`, `timeout_s`

### `wait_for_session`

Block until a session reaches a state, or timeout_s. until="idle": claude_status is idle | completed | stopped | failed (true even before a first turn). until="turn_gt": turn_seq > `turn`; pass send_prompt's turn_seq_before to wait for the reply to YOUR prompt. Polls every 500 ms. Returns { status: satisfied | timeout, claude_status, turn_seq, last_stop_at, stuck_kind }. Read-only; a per-host token only for sessions on its own host.

Parameters: `session_id`, `timeout_s`, `turn`, `until`

### `wait_for_task`

Block until a task is done | failed | cancelled, or timeout_s (polls every 500 ms). Returns { status: satisfied | timeout, task }; task.result holds the worker's paragraph. Read-only. A per-host token may only wait on tasks it requested or whose worker is on its host (E_FORBIDDEN).

Parameters: `task_id`, `timeout_s`

### `whoami`

Find your own fleet row from your tmux session name (`tmux display-message -p '#S'`). E_NOTFOUND until fleet has reconciled it; E_AMBIGUOUS when the name exists on several hosts: pick yours from the error's {session_id, host_alias} candidates and use session_id from then on.

Parameters: `tmux_name`

### `work`

Work links: {session_id} → its live links; {key} → ended (past) links; neither → recently ended. action context|resume_plan {key}; purge_impact; tickets (cached); lookup {key|url}; trackers; scopes; orgs; org_suggestions; today {since}; card {key}; tidy; reopened.

Parameters: `action`, `host_alias`, `host_aliases`, `key`, `limit`, `link_id`, `project_id`, `query`, `session_id`, `since`, `tracker_id`, `url`, `view`, `with_brief`

### `work_admin`

Trackers, orgs and retention; see action. Never returns a secret.

Parameters: `action`, `auth_kind`, `auto_tidy`, `color`, `confirm_nonce`, `credential_ref`, `host_alias`, `isolate_sessions`, `name`, `org_id`, `owner`, `path_prefix`, `provider`, `repo`, `rule_id`, `secret`, `settings`, `site_url`, `tracker_id`, `transport`, `username`

### `work_link`

Decide a session's work: action link (becomes its primary; key or item_id), reject (sticky 'not this'; or a suggestion's link_id), confirm (link_id), unlink (link_id). Returns the updated row. trust_project {project_id, on}. resume {key, mode}: new session on past work. start {key|url|item_id}: new session on a ticket (project_ids: one per repo). handover {session_id}: ask it to write its hand-off. archive|unarchive (UI only), snooze {days}|never (tidy-up); dismiss {item_id} (reopened); tidy_apply {items}: kills (safe kill when dirty).

Parameters: `action`, `brief`, `confirm_nonce`, `days`, `force_cross_org`, `host_alias`, `item_id`, `items`, `key`, `link_id`, `mode`, `name`, `on`, `project_id`, `project_ids`, `session_id`, `source`, `title`, `url`, `with_brief`, `worktree`

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
- `commands::work::session_work_links`
- `commands::work::link_session_work`
- `commands::work::reject_session_work`
- `commands::work::unlink_session_work`
- `commands::work::confirm_session_work`
- `commands::work::work_tidy`
- `commands::work::work_reopened`
- `commands::work::archive_session_work`
- `commands::work::unarchive_session_work`
- `commands::work::snooze_tidy`
- `commands::work::never_tidy`
- `commands::work::tidy_apply`
- `commands::work::dismiss_reopened`
- `commands::work::set_work_project_trust`
- `commands::work::work_resume_plan`
- `commands::work::resume_work`
- `commands::work::work_purge_impact`
- `commands::work::work_today`
- `commands::work::work_ticket_card`
- `commands::work::request_work_handover`
- `commands::work::list_local_work_items`
- `commands::work::name_session_work`
- `commands::work::rename_work_item`
- `commands::trackers::add_tracker`
- `commands::trackers::update_tracker`
- `commands::trackers::set_tracker_credential`
- `commands::trackers::test_tracker`
- `commands::trackers::remove_tracker`
- `commands::trackers::tracker_sync_metrics`
- `commands::trackers::work_retention_status`
- `commands::trackers::work_retention_sweep`
- `commands::trackers::list_trackers`
- `commands::trackers::work_tickets`
- `commands::trackers::work_lookup`
- `commands::trackers::start_work_multi`
- `commands::trackers::start_work`
- `commands::orgs::add_org`
- `commands::orgs::update_org`
- `commands::orgs::remove_org`
- `commands::orgs::add_org_rule`
- `commands::orgs::remove_org_rule`
- `commands::orgs::assign_host_org`
- `commands::orgs::assign_tracker_org`
- `commands::orgs::work_scopes`
- `commands::orgs::list_orgs`
- `commands::orgs::org_suggestions`
- `commands::sessions::session_history`
- `commands::sessions::session_conversations`
- `commands::sessions::session_conversation`
- `commands::sessions::session_tool_detail`
- `commands::sessions::session_activity`
- `commands::sessions::restart_session`
- `commands::sessions::send_prompt`
- `commands::sessions::spawn_review`
- `commands::sessions::recreate_session`
- `commands::sessions::restore_host_sessions`
- `commands::sessions::discover_lost_sessions`
- `commands::move_session::move_session`
- `commands::resolve_move::resolve_move`
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
- `commands::upload::pick_attachments`
- `commands::upload::attachment_preview`
- `commands::upload::attachment_describe`
- `commands::upload::upload_attachments`
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
- `commands::operator::ensure_operator`
- `commands::operator::operator_status`
- `commands::hub::hub_status`
- `commands::hub::hub_pair`
- `commands::hub::hub_disconnect`
- `commands::hub::hub_connection`
- `commands::hub::hub_stranded_token`
- `commands::hub::report_client_error`
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

