//! MCP tools: listing, spawning, inspecting and addressing sessions.

use super::*;
use crate::ipc_error::lock;

#[tool_router(router = session_ops_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "List tmux sessions across reachable hosts. Slim \
        summary rows by default; pass summary=false for the full SessionRow. \
        Optional filters: host_alias, project_id, status, claude_status, tag, \
        include_lost (default false drops ghosts); `limit` caps the row count \
        after filtering (default: all); `force` runs a reconcile pass first \
        instead of serving the recent cache. claude_status is one of working | \
        blocked | completed | failed | stopped | idle; stuck_kind is one of \
        auth_menu | reconnect | trust_prompt | oom | press_enter; ci_status \
        (full rows) is one of passing | failing | pending (null when the \
        session has no PR or its PR has no checks).")]
    pub(super) async fn list_sessions(
        &self,
        Parameters(p): Parameters<ListSessionsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "list_sessions",
            &format!(
                "host={:?} project={:?} status={:?} claude_status={:?} include_lost={} summary={} limit={:?} force={} tag={:?}",
                p.host_alias,
                p.project_id,
                p.status,
                p.claude_status,
                p.include_lost,
                p.summary,
                p.limit,
                p.force,
                p.tag,
            ),
        );
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
        let tagged = rows
            .into_iter()
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
                true
            })
            .map(|row| {
                let is_controller = controller
                    .as_ref()
                    .is_some_and(|(h, t)| *h == row.host_alias && *t == row.tmux_name);
                SessionWithController { is_controller, row }
            })
            // `limit` applies AFTER the filters so a filtered page is a real
            // page of matches, not the first N rows of the whole fleet.
            .take(p.limit.unwrap_or(usize::MAX));
        if p.summary {
            let slim: Vec<SessionSummary> = tagged.map(SessionSummary::from).collect();
            ok_json_compact(&slim)
        } else {
            let full: Vec<SessionWithController> = tagged.collect();
            ok_json_compact(&full)
        }
    }

    #[tool(description = "List sessions related to a given session — those \
        sharing the same project and worktree. Returns JSON.")]
    pub(super) async fn related_sessions(
        &self,
        Parameters(args): Parameters<sessions::RelatedSessionsArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "related_sessions",
            &format!("session_id={}", args.session_id),
        );
        ok_json(&sessions::related_sessions(args, &self.store).map_err(to_mcp_err)?)
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
        Parameters(p): Parameters<WhoamiParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("whoami", &format!("tmux={}", p.tmux_name));
        let s = lock(&self.store).map_err(to_mcp_err)?;
        let row = sessions::find_session_by_tmux_name(&s, &p.tmux_name).map_err(to_mcp_err)?;
        let controller = s
            .get_controller()
            .map_err(|e| to_mcp_err(IpcError::from(e)))?;
        let is_controller = controller
            .as_ref()
            .is_some_and(|(h, t)| *h == row.host_alias && *t == row.tmux_name);
        ok_json(&SessionWithController { is_controller, row })
    }

    #[tool(description = "Create a Claude Code tmux session on a host, in a \
        project (and optional worktree). Pass new_worktree to fork a fresh \
        worktree+branch (optional base_branch). Auto-clones the repo on \
        remote hosts.")]
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
        let args = sessions::NewSessionArgs {
            host_alias: p.host_alias,
            project_id: p.project_id,
            worktree_id: p.worktree_id,
            name: p.name,
            call_id: None,
            new_worktree: p.new_worktree,
            base_branch: p.base_branch,
            // Shell-kind sessions and per-start commands are not exposed on the
            // MCP surface yet; the GUI is the only path for those.
            kind: None,
            start_command: None,
            // MCP callers don't pick a label; let the service derive one from
            // the branch via `humanize::humanize_branch`.
            friendly_name: None,
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
        description = "Deprecated: use session_transcript. Returns the session's last assistant turn from its transcript. Address it with session_id OR claude_session_id (+ host_alias while the fleet row does not exist yet)."
    )]
    pub(super) async fn peek_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<PeekSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "peek_session",
            &format!(
                "session_id={:?} claude_session_id={:?} host={:?}",
                p.session_id, p.claude_session_id, p.host_alias
            ),
        );
        // Gate a fleet id up front, so even the "no Claude id yet" answer
        // is not given for another host's session.
        if let Some(id) = p.session_id {
            self.resolve_target(&caller, Some(id), None, None, "the session to peek")?;
        }
        let resolved = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            crate::service::bg_sessions::resolve_peek_target(
                &s,
                p.session_id,
                p.host_alias.as_deref(),
                p.claude_session_id.as_deref(),
            )
            .and_then(|(host_alias, claude_id)| {
                // The fleet row, when there is one, supplies the pane, cwd and
                // stored transcript path that locate the file precisely.
                let row = match p.session_id {
                    Some(id) => s.get_session_by_id(id)?,
                    None => s
                        .get_session_by_claude_id(&claude_id)?
                        .filter(|r| r.host_alias == host_alias),
                };
                Ok((host_alias, claude_id, row))
            })
        };
        let (host_alias, claude_id, row) = match resolved {
            Ok(target) => target,
            // A tracked interactive session with no Claude id is not an
            // error for the caller — say so instead of failing.
            Err(e) if e.code == "E_INVALID_STATE" => {
                return ok_json(
                    &"This session has no Claude session id yet — nothing to peek.".to_string(),
                );
            }
            Err(e) => return Err(to_mcp_err(e)),
        };
        // The claude_session_id path resolves its host here.
        require_host(&caller, &host_alias, "the session to peek")?;
        let text = match row {
            Some(row) => self.transcript_for(&row, None, None).await?,
            // Untracked (a new_bg_session id before reconcile): the read
            // script finds the file by session id alone.
            None => crate::service::transcript::fetch_transcript(
                crate::service::transcript::TranscriptArgs {
                    host_alias,
                    tmux_name: None,
                    transcript_path: None,
                    cwd: None,
                    claude_session_id: claude_id,
                    turns: 1,
                    max_chars: crate::service::transcript::DEFAULT_MAX_CHARS,
                },
                &self.ssh,
            )
            .await
            .map_err(to_mcp_err)?,
        };
        ok_json(&text)
    }

    #[tool(description = "Recreate a session: kill its tmux session and rebuild \
        it fresh in the same worktree, resuming the same Claude conversation. \
        Use when the session is frozen, OOM-killed or out of context, or to \
        revive a ghost — the conversation survives, the process does not. Works for \
        running or ghost sessions. Returns the session row as JSON.")]
    pub(super) async fn recreate_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<sessions::RecreateSessionArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "recreate_session",
            &format!("session_id={}", args.session_id),
        );
        // Gate on the stored row's host: a per-host token must not kill and
        // rebuild a session on another host by naming its fleet id.
        self.resolve_target(
            &caller,
            Some(args.session_id),
            None,
            None,
            "the session to recreate",
        )?;
        let row = sessions::recreate_session(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(
        description = "Restore sessions a host lost to a reboot or a tmux server restart: \
        resume each lost tmux session's Claude conversation in its original worktree, under its \
        original name. Call with dry_run=true first to get the plan (no ssh, no writes). \
        Concurrency and pacing come from the restore.batch_size / restore.stagger_ms settings. \
        One failing session never fails the others; the result lists each session's outcome. \
        One restore per host at a time: a second call while one runs gets E_INVALID_STATE. A \
        lost fleet controller is skipped (recreate it with recreate_session force=true). A call \
        that timed out may have partially completed: sessions keep coming back after it; re-run \
        with dry_run=true to see what is still lost. \
        First-run prompts in a resumed session are not answered: they surface as stuck_kind."
    )]
    pub(super) async fn restore_host_sessions(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<sessions::RestoreHostSessionsArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "restore_host_sessions",
            &format!("host={} dry_run={}", args.host_alias, args.dry_run),
        );
        require_host(&caller, &args.host_alias, "the lost sessions")?;
        let report = sessions::restore_host_sessions(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&report)
    }

    #[tool(
        description = "Read-only: scan ~/.claude/projects on a host for recent Claude \
        conversations (e.g. ones a reboot left without a pane), rank them relative to the host's \
        boot (rank_hint: before_boot | after_boot | stale | unknown), and enrich each with the \
        fleet ids it can infer: project_id, worktree_id, and existing_session_id — set when a \
        fleet session on the host (live or lost) already holds that claude_session_id; restore \
        such a row with restore_host_sessions, not new_session. resumable is true only when \
        new_session would start the pane in exactly the transcript's cwd (a registered worktree \
        or the project root); anywhere else (a subdirectory, an unregistered worktree) \
        claude --resume cannot find the transcript and a new, empty conversation would start, \
        so do not resume it. derived_tmux_name is set only for a resumable candidate; it may \
        differ from a session's original name for a second session on the same worktree, so \
        treat it as a hint. Restore a resumable one with new_session { host_alias, project_id, \
        worktree_id, name: derived_tmux_name, resume_claude_session_id }. limit caps how many \
        transcripts (newest first) are read: default 50, max 500."
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
        friendly name and last_prompt.")]
    pub(super) async fn new_bg_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<crate::service::bg_sessions::NewBgSessionArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "new_bg_session",
            &format!("host={} name={}", args.host_alias, args.name),
        );
        require_host(&caller, &args.host_alias, "the new background session")?;
        let res = crate::service::bg_sessions::new_bg_session_tracked(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&res)
    }

    // ── Orchestration (Wave 3 Track E) ───────────────────────────────────
}
