//! MCP tools: listing, spawning, inspecting and addressing sessions.

use super::*;
use crate::ipc_error::lock;

#[tool_router(router = session_ops_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "List tmux sessions across reachable hosts. Slim \
        summary rows by default; pass summary=false for the full SessionRow. \
        Optional filters: host_alias, project_id, status, claude_status, tag, \
        needs_attention (rows that want a person; each carries why and since), \
        include_lost (default false drops ghosts); `limit` caps the row count \
        after filtering (default: all); `force` runs a reconcile pass first \
        instead of serving the recent cache. claude_status is one of working | \
        blocked | completed | failed | stopped | idle; stuck_kind is one of \
        auth_menu | reconnect | trust_prompt | oom | press_enter; ci_status \
        (full rows) is one of passing | failing | pending (null when the \
        session has no PR or its PR has no checks). fresh_for returns only \
        what is new since your last read.")]
    pub(super) async fn list_sessions(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<ListSessionsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "list_sessions",
            &format!(
                "host={:?} project={:?} status={:?} claude_status={:?} include_lost={} summary={} view={:?} limit={:?} force={} tag={:?} fresh_for={:?}",
                p.host_alias,
                p.project_id,
                p.status,
                p.claude_status,
                p.include_lost,
                p.summary,
                p.view,
                p.limit,
                p.force,
                p.tag,
                p.fresh_for,
            ),
        );
        // Parsed before the listing runs: an unknown view must cost the
        // caller a refusal, not a reconcile pass whose result it then
        // cannot have.
        let view = p.view.as_deref().map(SessionView::parse).transpose()?;
        let rows = if p.force {
            sessions::refresh_sessions(&self.store, &self.ssh).await
        } else {
            sessions::list_sessions(&self.store, &self.ssh).await
        }
        .map_err(to_mcp_err)?;
        let controller = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            s.get_controller()
                .map_err(|e| to_mcp_err(IpcError::from(e)))?
        };
        // Work graph M5: a per-host token lists only the sessions its org
        // may see (D7, `isolate_sessions`), and each row without the work
        // of other orgs — here, before `fresh_for` hashes the page, and not
        // only in the result gate.
        let scope = self.org_scope(&caller)?;
        let tagged = rows
            .into_iter()
            .filter(|row| scope.sees_row(row))
            .map(|mut row| {
                scope.redact_row(&mut row);
                row
            })
            .filter(|row| {
                if !p.include_lost && row.lost_at.is_some() {
                    return false;
                }
                if let Some(h) = &p.host_alias {
                    if &row.host_alias != h {
                        return false;
                    }
                }
                if let Some(pid) = p.project_id {
                    if row.project_id != Some(pid) {
                        return false;
                    }
                }
                if let Some(st) = &p.status {
                    if &row.status != st {
                        return false;
                    }
                }
                if let Some(cs) = &p.claude_status {
                    if row.claude_status.as_deref() != Some(cs.as_str()) {
                        return false;
                    }
                }
                if let Some(tag) = &p.tag {
                    if !row.tags.iter().any(|t| t == tag) {
                        return false;
                    }
                }
                // Free: the rows are already in hand under one store lock, so
                // this is one predicate more over what was read anyway — and
                // it is the difference between a phone fetching 44 rows to
                // find 3 and fetching 3.
                if let Some(want) = p.needs_attention {
                    if crate::service::attention::needs_attention(row).is_some() != want {
                        return false;
                    }
                }
                true
            })
            .map(|row| {
                let is_controller = controller
                    .as_ref()
                    .is_some_and(|(h, t)| *h == row.host_alias && *t == row.tmux_name);
                SessionWithController::new(is_controller, row)
            })
            // `limit` applies AFTER the filters so a filtered page is a real
            // page of matches, not the first N rows of the whole fleet.
            .take(p.limit.unwrap_or(usize::MAX));
        // A view is a projection OF the full row, so it outranks `summary`
        // rather than composing with it: the slim shape drops
        // `friendly_name`, `current_activity` and `last_activity_at`, which
        // are three of the columns a phone draws — the exact reason that
        // client asks for full rows today. `summary` keeps its default of
        // true, so a caller that names a view need not also say
        // `summary: false` to be understood.
        //
        // fresh_for absent: today's default, byte-identical, no cursor
        // touched — kept as a literal early return (not folded into the
        // hashing branch below) so the two paths can never drift apart.
        let Some(reader) = p.fresh_for else {
            return match (view, p.summary) {
                (Some(v), _) => {
                    let full: Vec<SessionWithController> = tagged.collect();
                    ok_json_compact_view(&full, Some(v.fields()))
                }
                (None, true) => {
                    let slim: Vec<SessionSummary> = tagged.map(SessionSummary::from).collect();
                    ok_json_compact(&slim)
                }
                (None, false) => {
                    let full: Vec<SessionWithController> = tagged.collect();
                    ok_json_compact(&full)
                }
            };
        };

        // `fresh_for` present: build the EXACT same fields the branch above
        // would have sent — `compact_json_value` is what `compact_json_string`
        // (in turn what `ok_json_compact` / `ok_json_compact_view` call)
        // builds internally — and hash THAT `Value`, never a curated subset.
        // This is why the default slim shape (which drops `last_activity_at`
        // and `current_activity`, the two fields a reconcile pass bumps
        // constantly) makes `unchanged` fire usefully: a `summary: false`
        // caller hashes those two churny fields too, so it will rarely see
        // `unchanged`.
        //
        // Sorted by session id first, on this path only: `list_all_sessions`
        // (the store's underlying query) orders by `last_activity_at DESC` —
        // the very field the slim shape just dropped because reconcile bumps
        // it constantly. Two sessions trading activity would otherwise
        // reorder this array with no field actually differing, changing the
        // hash for a caller who could not possibly see why. The DEFAULT path
        // above keeps today's order untouched — only the opt-in envelope's
        // row order changes here.
        let data = match (view, p.summary) {
            (Some(v), _) => {
                let mut full: Vec<SessionWithController> = tagged.collect();
                full.sort_by_key(|s| s.row.id);
                compact_json_value(&full, Some(v.fields()))?
            }
            (None, true) => {
                let mut slim: Vec<SessionSummary> = tagged.map(SessionSummary::from).collect();
                slim.sort_by_key(|s| s.id);
                compact_json_value(&slim, None)?
            }
            (None, false) => {
                let mut full: Vec<SessionWithController> = tagged.collect();
                full.sort_by_key(|s| s.row.id);
                compact_json_value(&full, None)?
            }
        };

        let resource_key = list_sessions_resource_key(&p);
        // Read-then-write: validate the reader and read the stored hash
        // before deciding anything, so `unchanged` can never be answered to
        // a reader session that no longer exists.
        let (reader_exists, stored_hash) = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            let reader_exists = resolve_reader(&s, &caller, reader)?;
            let stored_hash = s
                .get_read_cursor(reader, "list_sessions", &resource_key)
                .map_err(to_mcp_err)?
                .and_then(|c| c.content_hash);
            (reader_exists, stored_hash)
        };

        let decision = snapshot_decision(reader_exists, stored_hash.as_deref(), data)?;
        // Written only now that the payload was built successfully;
        // `snapshot_decision` never returns a hash for an unknown reader or
        // an unchanged read.
        if let Some(hash) = &decision.new_hash {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            s.put_snapshot_cursor(reader, "list_sessions", &resource_key, None, hash)
                .map_err(to_mcp_err)?;
        }
        ok_json(&decision.envelope)
    }

    #[tool(description = "List sessions related to a given session — those \
        sharing the same project and worktree. Returns JSON.")]
    pub(super) async fn related_sessions(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<sessions::RelatedSessionsArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "related_sessions",
            &format!("session_id={}", args.session_id),
        );
        let scope = self.org_scope(&caller)?;
        ok_json_compact(
            &sessions::related_sessions_scoped(args, &self.store, &scope).map_err(to_mcp_err)?,
        )
    }

    #[tool(description = "Mark the calling session as the fleet controller; \
        kill/recreate/restart refuse to target it without force. Address \
        yourself with session_id (from whoami) OR host_alias + tmux_name. A \
        per-host token may only register a session on its own host \
        (E_FORBIDDEN).")]
    pub(super) async fn register_self(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<RegisterSelfParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "register_self",
            &format!(
                "session_id={:?} host={:?} tmux={:?}",
                p.session_id, p.host_alias, p.tmux_name
            ),
        );
        let (host_alias, tmux_name) = self.resolve_target(
            &caller,
            p.session_id,
            p.host_alias.as_deref(),
            p.tmux_name.as_deref(),
            "the session to register",
        )?;
        {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            s.set_controller(&host_alias, &tmux_name)
                .map_err(|e| to_mcp_err(IpcError::from(e)))?;
        }
        ok_json(&serde_json::json!({
            "controller": { "host_alias": host_alias, "tmux_name": tmux_name }
        }))
    }

    #[tool(description = "Find your own fleet row from your tmux session name \
        (`tmux display-message -p '#S'`). Returns the single matching session \
        as JSON (id, host_alias, is_controller, …). E_NOTFOUND when fleet has \
        not reconciled the session yet; E_AMBIGUOUS when the same name exists \
        on several hosts — the error's details list {session_id, host_alias} \
        candidates, pick yours and use session_id from then on.")]
    pub(super) async fn whoami(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<WhoamiParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("whoami", &format!("tmux={}", p.tmux_name));
        // Scoped so the store guard is released before
        // `stored_local_fleet_id` takes it again below — `Store` is a plain
        // (non-reentrant) `std::sync::Mutex`.
        let (row, is_controller) = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            let scope = caller.org_scope(&s).map_err(to_mcp_err)?;
            let row = sessions::find_session_by_tmux_name_scoped(&s, &p.tmux_name, &scope)
                .map_err(to_mcp_err)?;
            let controller = s
                .get_controller()
                .map_err(|e| to_mcp_err(IpcError::from(e)))?;
            let is_controller = controller
                .as_ref()
                .is_some_and(|(h, t)| *h == row.host_alias && *t == row.tmux_name);
            (row, is_controller)
        };
        // Reported, never minted: `whoami` is a read, and a readonly token
        // can call it — a read must not write (final review, Minor 7). Null
        // means no address has yet needed a fleet comparison, which is the
        // only thing that mints one (`ensure_local_fleet_id`).
        let fleet_id =
            crate::service::address::stored_local_fleet_id(&self.store).map_err(to_mcp_err)?;
        let mut payload = serde_json::to_value(SessionWithController::new(is_controller, row))
            .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))?;
        if let serde_json::Value::Object(map) = &mut payload {
            map.insert(
                "fleet_id".to_string(),
                match fleet_id {
                    Some(id) => serde_json::Value::String(id),
                    None => serde_json::Value::Null,
                },
            );
        }
        ok_json(&payload)
    }

    #[tool(description = "Create a Claude Code tmux session on a host, in a \
        project (and optional worktree). Pass new_worktree to fork a fresh \
        worktree+branch (optional base_branch). Auto-clones the repo on \
        remote hosts. Optional kind=\"shell\" runs a plain interactive shell \
        instead (see new_shell_session for the same thing with start_command); \
        optional friendly_name sets the sidebar label (omit / empty to derive \
        one from the branch).")]
    pub(super) async fn new_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<NewSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "new_session",
            &format!("host={} name={}", p.host_alias, p.name),
        );
        // A per-host token may only spawn on its own host (B1).
        require_host(&caller, &p.host_alias, "the new session")?;
        self.confirm_gate(
            "new_session",
            p.confirm_nonce.as_deref(),
            &new_session_summary(&p),
            &caller,
        )?;
        let args = sessions::NewSessionArgs {
            host_alias: p.host_alias,
            project_id: p.project_id,
            worktree_id: p.worktree_id,
            name: p.name,
            call_id: None,
            new_worktree: p.new_worktree,
            base_branch: p.base_branch,
            kind: p.kind,
            start_command: p.start_command,
            // Omit / empty -> the service derives one from the branch via
            // `humanize::humanize_branch`.
            friendly_name: p.friendly_name,
            resume_claude_session_id: p.resume_claude_session_id,
        };
        let row = sessions::new_session(args, &self.store, &self.ssh, &self.reg)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(
        description = "Create a plain-shell tmux session on a host (no Claude \
        Code in the pane — an interactive login shell). Same project/worktree \
        plumbing as new_session, plus an optional start_command that runs once \
        before the shell drops to an interactive prompt; the pane stays alive \
        after it exits so you can attach or send-keys to it. Steer it with \
        send_prompt (typed text + Enter) and read it with capture_session."
    )]
    pub(super) async fn new_shell_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<NewShellSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "new_shell_session",
            &format!("host={} name={}", p.host_alias, p.name),
        );
        // A per-host token may only spawn on its own host (B1).
        require_host(&caller, &p.host_alias, "the new session")?;
        self.confirm_gate(
            "new_shell_session",
            p.confirm_nonce.as_deref(),
            &new_shell_session_summary(&p),
            &caller,
        )?;
        let args = sessions::NewSessionArgs {
            host_alias: p.host_alias,
            project_id: p.project_id,
            worktree_id: p.worktree_id,
            name: p.name,
            call_id: None,
            new_worktree: p.new_worktree,
            base_branch: p.base_branch,
            kind: Some("shell".to_string()),
            start_command: p.start_command,
            // Let the service derive a humanised label from the branch.
            friendly_name: None,
            resume_claude_session_id: None,
        };
        let row = sessions::new_session(args, &self.store, &self.ssh, &self.reg)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Capture a session's terminal output — the visible \
        tmux pane, or include scrollback history (scrollback_lines). Use after \
        send_prompt to read the session's reply. Returns the pane as plain \
        text (not JSON), capped to the last max_lines lines (default 200).")]
    pub(super) async fn capture_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<CaptureSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "capture_session",
            &format!(
                "session_id={} scrollback_lines={:?} max_lines={:?}",
                p.session_id, p.scrollback_lines, p.max_lines
            ),
        );
        // A pane can show secrets: a per-host token reads only its own host.
        self.resolve_target(
            &caller,
            Some(p.session_id),
            None,
            None,
            "the session to capture",
        )?;
        let text = sessions::capture_session_output(
            p.session_id,
            &self.store,
            &self.ssh,
            p.scrollback_lines,
        )
        .await
        .map_err(to_mcp_err)?;
        // A blank pane (fresh/cleared session) yields empty output. Returning it
        // verbatim would put an empty text block into the caller's conversation;
        // say so explicitly instead. `text_content` is the backstop for any
        // residual whitespace-only capture.
        if text.trim().is_empty() {
            return Ok(CallToolResult::success(vec![text_content(
                "(session pane is empty — nothing to capture)",
            )]));
        }
        // Plain text, not `ok_json`: a JSON-encoded string turns every newline
        // into `\n` and doubles the token cost of a pane dump for no benefit.
        let max = p.max_lines.unwrap_or(CAPTURE_DEFAULT_MAX_LINES);
        Ok(CallToolResult::success(vec![text_content(
            capture_response(&text, max),
        )]))
    }

    #[tool(
        description = "What the session's pane shows right now: claude_status, \
        stuck_kind, current_activity, waiting_for and the spinner line. One capture, \
        nothing stored — the cheap read behind a live indicator, where \
        capture_session is the whole pane. E_INVALID_STATE outside tmux. JSON."
    )]
    pub(super) async fn session_activity(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SessionActivityParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("session_activity", &format!("session_id={}", p.session_id));
        // The probe carries a slice of pane intel, so it is gated exactly
        // like `capture_session`: a per-host token reads only its own host.
        self.resolve_target(
            &caller,
            Some(p.session_id),
            None,
            None,
            "the session to probe",
        )?;
        let probe = sessions::session_activity(&self.store, &self.ssh, p.session_id)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&probe)
    }

    #[tool(description = "Recreate a session: kill its tmux session and rebuild \
        it fresh in the same worktree, resuming the same Claude conversation. \
        Use when the session is frozen, OOM-killed or out of context, or to \
        revive a ghost — the conversation survives, the process does not. Works for \
        running or ghost sessions. Returns the session row as JSON.")]
    pub(super) async fn recreate_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(RecreateSessionParams {
            args,
            confirm_nonce,
        }): Parameters<RecreateSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "recreate_session",
            &format!("session_id={}", args.session_id),
        );
        // Gate on the stored row's host: a per-host token must not kill and
        // rebuild a session on another host by naming its fleet id.
        let (host, name) = self.resolve_target(
            &caller,
            Some(args.session_id),
            None,
            None,
            "the session to recreate",
        )?;
        // The operator's recreates (a kill and a start) need a person (D12).
        self.confirm_gate(
            "recreate_session",
            confirm_nonce.as_deref(),
            &format!(
                "session_id={} host={} name={} force={}",
                args.session_id,
                bound_text(Some(&host)),
                bound_text(Some(&name)),
                args.force
            ),
            &caller,
        )?;
        let row = sessions::recreate_session(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(
        description = "Restore sessions a host lost to a reboot or tmux restart: resume each \
        one's Claude conversation in its original worktree under its original name. dry_run=true \
        returns the plan (no ssh, no writes) — call it first, and again after a timeout to see \
        what is still lost. One failing session never fails the others; the result lists every \
        outcome. One restore per host at a time (E_INVALID_STATE otherwise); a lost fleet \
        controller is skipped (recreate_session force=true). Paced by restore.batch_size / \
        restore.stagger_ms."
    )]
    pub(super) async fn restore_host_sessions(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(RestoreHostSessionsParams {
            args,
            confirm_nonce,
        }): Parameters<RestoreHostSessionsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "restore_host_sessions",
            &format!("host={} dry_run={}", args.host_alias, args.dry_run),
        );
        require_host(&caller, &args.host_alias, "the lost sessions")?;
        // A real restore starts sessions: the operator's needs a person
        // (D12). A dry run only reads the plan.
        if !args.dry_run {
            self.confirm_gate(
                "restore_host_sessions",
                confirm_nonce.as_deref(),
                &format!(
                    "host={} session_ids={:?}",
                    bound_text(Some(&args.host_alias)),
                    args.session_ids
                ),
                &caller,
            )?;
        }
        let report = sessions::restore_host_sessions(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&report)
    }

    #[tool(
        description = "Read-only: scan ~/.claude/projects on a host for Claude conversations \
        fleet has no live pane for, ranked against the host's boot (rank_hint: before_boot | \
        after_boot | stale | unknown) and enriched with project_id, worktree_id and \
        existing_session_id — the last set when a fleet row (live or lost) already holds that \
        conversation, which restore_host_sessions handles, not this. resumable is true only when \
        the pane would start in exactly the transcript's cwd; anywhere else claude --resume \
        silently starts an empty conversation instead, so do not resume it. Resume one with \
        new_session { host_alias, project_id, worktree_id, name: derived_tmux_name (a hint), \
        resume_claude_session_id }. limit: newest first, default 50, max 500."
    )]
    pub(super) async fn discover_lost_sessions(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<sessions::DiscoverLostSessionsArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "discover_lost_sessions",
            &format!("host={} limit={:?}", args.host_alias, args.limit),
        );
        require_host(&caller, &args.host_alias, "the lost sessions")?;
        let candidates = sessions::discover_lost_sessions(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&candidates)
    }

    #[tool(description = "Dismiss a ghost session (lost from tmux): permanently \
        delete its row. Use when a ghost is not worth reviving — the row is \
        the only thing left to clean up. Errors if the session is not a \
        ghost.")]
    pub(super) async fn dismiss_ghost_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<sessions::DismissGhostSessionArgs>,
    ) -> Result<CallToolResult, McpError> {
        let session_id = args.session_id;
        audit("dismiss_ghost_session", &format!("session_id={session_id}"));
        self.resolve_target(
            &caller,
            Some(session_id),
            None,
            None,
            "the ghost to dismiss",
        )?;
        sessions::dismiss_ghost_session(args, &self.store).map_err(to_mcp_err)?;
        ok_json(&serde_json::json!({ "dismissed": session_id }))
    }

    #[tool(description = "Launch a supervised headless (background) Claude \
        session on a host with an initial prompt. Returns JSON with the new \
        claude_session_id AND the fleet row (`session`, registered by an \
        immediate reconcile; the key is absent if the agent was not matched \
        yet — it appears on the next tick) so the next call can be \
        session_transcript { session_id }. The prompt becomes the row's default \
        friendly name and last_prompt. Pass requester_session_id (yours, from \
        whoami) to list it under that session's background work.")]
    pub(super) async fn new_bg_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(NewBgSessionParams {
            args,
            confirm_nonce,
        }): Parameters<NewBgSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "new_bg_session",
            &format!("host={} name={}", args.host_alias, args.name),
        );
        require_host(&caller, &args.host_alias, "the new background session")?;
        self.confirm_gate(
            "new_bg_session",
            confirm_nonce.as_deref(),
            &new_bg_session_summary(&args),
            &caller,
        )?;
        // The requester (when given) must exist and, for a per-host caller,
        // live on that host — otherwise any agent could parent a background
        // session onto somebody else's conversation. Same gate as
        // `dispatch_task`; `parent_session_id` has no foreign key to catch it
        // later.
        if let Some(req) = args.requester_session_id {
            self.resolve_target_row(&caller, Some(req), None, None, "requester_session_id")?;
        }
        let res = crate::service::bg_sessions::new_bg_session_tracked(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&res)
    }

    #[tool(description = "Ensure the UX agent's operator session exists; returns its row.")]
    pub(super) async fn ensure_operator(&self) -> Result<CallToolResult, McpError> {
        audit("ensure_operator", "");
        let row = crate::service::operator::ensure_operator(&self.store, &self.ssh, &self.reg)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(
        description = "Whether the UX agent can work, and why not: absent|lost|no_mcp|token_revoked|no_host."
    )]
    pub(super) async fn operator_status(&self) -> Result<CallToolResult, McpError> {
        audit("operator_status", "");
        let status = crate::service::operator::operator_status(&self.store).map_err(to_mcp_err)?;
        ok_json(&status)
    }

    // ── Orchestration (Wave 3 Track E) ───────────────────────────────────
}

/// `list_sessions`'s snapshot cursor key: a fingerprint of every filter
/// param that SHAPES the output — `host_alias`, `project_id`, `status`,
/// `claude_status`, `include_lost`, `summary`, `limit`, `tag`, `view`,
/// `needs_attention`. `force` and `fresh_for` are excluded: neither changes
/// what "the same filters" means, so including them would split one caller's
/// repeated reads across needless cursors. A plain JSON encoding of a
/// fixed-field-order struct is already deterministic; it is hashed only to
/// keep the stored key short, reusing `fresh::snapshot_hash` rather than a
/// second hasher. Two calls with identical filters share a cursor; two
/// different filters never do.
#[derive(serde::Serialize)]
struct ListSessionsFilterFingerprint<'a> {
    host_alias: &'a Option<String>,
    project_id: Option<i64>,
    status: &'a Option<String>,
    claude_status: &'a Option<String>,
    include_lost: bool,
    summary: bool,
    limit: Option<usize>,
    tag: &'a Option<String>,
    view: &'a Option<String>,
    needs_attention: Option<bool>,
}

fn list_sessions_resource_key(p: &ListSessionsParams) -> String {
    let fp = ListSessionsFilterFingerprint {
        host_alias: &p.host_alias,
        project_id: p.project_id,
        status: &p.status,
        claude_status: &p.claude_status,
        include_lost: p.include_lost,
        summary: p.summary,
        limit: p.limit,
        tag: &p.tag,
        view: &p.view,
        needs_attention: p.needs_attention,
    };
    let json = serde_json::to_string(&fp).expect("fixed-shape fingerprint always serializes");
    fresh::snapshot_hash(&json)
}
