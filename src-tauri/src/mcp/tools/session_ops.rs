//! MCP tools: listing, spawning, inspecting and addressing sessions.

use super::*;

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
            let s = self
                .store
                .lock()
                .map_err(|_| McpError::internal_error("E_LOCK: store mutex poisoned", None))?;
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
        Parameters(p): Parameters<RelatedSessionsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("related_sessions", &format!("session_id={}", p.session_id));
        let args = sessions::RelatedSessionsArgs {
            session_id: p.session_id,
        };
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
            let s = self
                .store
                .lock()
                .map_err(|_| McpError::internal_error("E_LOCK: store mutex poisoned", None))?;
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
        let s = self
            .store
            .lock()
            .map_err(|_| to_mcp_err(IpcError::lock()))?;
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
        Parameters(p): Parameters<CaptureSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "capture_session",
            &format!(
                "session_id={} scrollback_lines={:?} max_lines={:?}",
                p.session_id, p.scrollback_lines, p.max_lines
            ),
        );
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
        description = "Peek at a session's background Claude logs. Address it \
        with session_id (from list_sessions) OR claude_session_id (the id \
        new_bg_session returned; add host_alias while the fleet row does not \
        exist yet). Returns an informational message for interactive sessions \
        with no background job."
    )]
    pub(super) async fn peek_session(
        &self,
        Parameters(p): Parameters<PeekSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "peek_session",
            &format!(
                "session_id={:?} claude_session_id={:?} host={:?}",
                p.session_id, p.claude_session_id, p.host_alias
            ),
        );
        let resolved = {
            let s = self
                .store
                .lock()
                .map_err(|_| to_mcp_err(IpcError::lock()))?;
            crate::service::bg_sessions::resolve_peek_target(
                &s,
                p.session_id,
                p.host_alias.as_deref(),
                p.claude_session_id.as_deref(),
            )
        };
        let (host_alias, claude_id) = match resolved {
            Ok(pair) => pair,
            // A tracked interactive session with no Claude id is not an
            // error for the caller — say so instead of failing.
            Err(e) if e.code == "E_INVALID_STATE" => {
                return ok_json(
                    &"This session has no Claude session id yet — nothing to peek.".to_string(),
                );
            }
            Err(e) => return Err(to_mcp_err(e)),
        };
        let logs = crate::service::bg_sessions::peek_session(
            crate::service::bg_sessions::PeekSessionArgs {
                host_alias,
                claude_session_id: claude_id,
            },
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&logs)
    }

    #[tool(description = "Recreate a session: kill its tmux session and rebuild \
        it fresh in the same worktree, resuming the same Claude conversation. \
        Use when the session is frozen, OOM-killed or out of context, or to \
        revive a ghost — the conversation survives, the process does not. Works for \
        running or ghost sessions. Returns the session row as JSON.")]
    pub(super) async fn recreate_session(
        &self,
        Parameters(p): Parameters<RecreateSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("recreate_session", &format!("session_id={}", p.session_id));
        let row = sessions::recreate_session(
            sessions::RecreateSessionArgs {
                session_id: p.session_id,
                force: p.force,
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Dismiss a ghost session (lost from tmux): permanently \
        delete its row. Use when a ghost is not worth reviving — the row is \
        the only thing left to clean up. Errors if the session is not a \
        ghost.")]
    pub(super) async fn dismiss_ghost_session(
        &self,
        Parameters(p): Parameters<SessionIdParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "dismiss_ghost_session",
            &format!("session_id={}", p.session_id),
        );
        sessions::dismiss_ghost_session(
            sessions::DismissGhostSessionArgs {
                session_id: p.session_id,
            },
            &self.store,
        )
        .map_err(to_mcp_err)?;
        ok_json(&serde_json::json!({ "dismissed": p.session_id }))
    }

    #[tool(description = "Launch a supervised headless (background) Claude \
        session on a host with an initial prompt. Returns JSON with the new \
        claude_session_id AND the fleet row (`session`, registered by an \
        immediate reconcile; the key is absent if the agent was not matched \
        yet — it appears on the next tick) so the next call can be \
        peek_session { session_id }. The prompt becomes the row's default \
        friendly name and last_prompt.")]
    pub(super) async fn new_bg_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<NewBgSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "new_bg_session",
            &format!("host={} name={}", p.host_alias, p.name),
        );
        require_host(&caller, &p.host_alias, "the new background session")?;
        let res = crate::service::bg_sessions::new_bg_session_tracked(
            crate::service::bg_sessions::NewBgSessionArgs {
                host_alias: p.host_alias,
                name: p.name,
                prompt: p.prompt,
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&res)
    }

    // ── Orchestration (Wave 3 Track E) ───────────────────────────────────
}
