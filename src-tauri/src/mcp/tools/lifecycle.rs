//! MCP tools: kill, restart, rename, review, repair, move and the host clipboard.

use super::*;

#[tool_router(router = lifecycle_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Kill a session on a host: a tmux session by name, or \
        a background agent row (name `bg:<uuid>`) via `claude stop` — the \
        latter is idempotent, so it also clears a stale row whose process \
        already died. Use when the session's work is disposable or already \
        pushed and you want it gone NOW; prefer safe_kill_session when the \
        worktree may hold unpushed work. Returns the killed session's id. \
        Address the session with session_id OR host_alias + name. May return \
        E_CONFIRM_REQUIRED when desktop confirmation is on.")]
    pub(super) async fn kill_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<KillSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "kill_session",
            &format!(
                "session_id={:?} host={:?} name={:?}",
                p.session_id, p.host_alias, p.name
            ),
        );
        let (host_alias, name) = self.resolve_target(
            &caller,
            p.session_id,
            p.host_alias.as_deref(),
            p.name.as_deref(),
            "the session to kill",
        )?;
        self.confirm_gate(
            "kill_session",
            p.confirm_nonce.as_deref(),
            &format!("host={host_alias} name={name} force={}", p.force),
            &caller,
        )?;
        let args = sessions::KillSessionArgs {
            host_alias,
            name,
            force: p.force,
        };
        let id = sessions::kill_session(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&id)
    }

    #[tool(description = "Ask a running Claude session to safely persist its \
        work (commit + push), then arm deletion of its worktree + tmux session. \
        Use when retiring a session whose worktree may hold unpushed work and \
        you can wait for it to finish. Returns the row with \
        safe_kill_state=requested; the actual delete fires only after the \
        SAFE_REMOVE_READY marker AND a clean-tree check. Transitions ('ready', \
        'failed') arrive via row events. Address the session with session_id \
        OR host_alias + tmux_name.")]
    pub(super) async fn safe_kill_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SafeKillSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "safe_kill_session",
            &format!(
                "session_id={:?} host={:?} session={:?}",
                p.session_id, p.host_alias, p.tmux_name
            ),
        );
        let (host_alias, tmux_name) = self.resolve_target(
            &caller,
            p.session_id,
            p.host_alias.as_deref(),
            p.tmux_name.as_deref(),
            "the session to retire",
        )?;
        let args = safe_kill::SafeKillSessionArgs {
            host_alias,
            tmux_name,
        };
        let row = safe_kill::safe_kill_session(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Rename a tmux session on a host. Returns the updated \
        session row as JSON. Address the session with session_id OR host_alias \
        + old_name.")]
    pub(super) async fn rename_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<RenameSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "rename_session",
            &format!(
                "session_id={:?} host={:?} {:?} -> {}",
                p.session_id, p.host_alias, p.old_name, p.new_name
            ),
        );
        let (host_alias, old_name) = self.resolve_target(
            &caller,
            p.session_id,
            p.host_alias.as_deref(),
            p.old_name.as_deref(),
            "the session to rename",
        )?;
        let args = sessions::RenameSessionArgs {
            host_alias,
            old_name,
            new_name: p.new_name,
        };
        let row = sessions::rename_session(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Set the session's friendly display name (shown when \
        the user toggles friendly names on). Called once per task by the \
        in-session agent — short (3–6 words). Empty string clears. Returns \
        the updated row. Address the session with session_id OR host_alias + \
        tmux_name.")]
    pub(super) async fn set_friendly_name(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SetFriendlyNameParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "set_friendly_name",
            &format!(
                "session_id={:?} host={:?} tmux={:?} label={:?}",
                p.session_id, p.host_alias, p.tmux_name, p.friendly_name
            ),
        );
        let (host_alias, tmux_name) = self.resolve_target(
            &caller,
            p.session_id,
            p.host_alias.as_deref(),
            p.tmux_name.as_deref(),
            "the session to label",
        )?;
        let args = sessions::SetFriendlyNameArgs {
            host_alias,
            tmux_name,
            friendly_name: p.friendly_name,
        };
        let row = sessions::set_session_friendly_name(args, &self.store).map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Restart a tmux session (kill and recreate it in the \
        same place). Use when the Claude REPL is wedged but tmux and the \
        worktree are fine — an in-place relaunch, cheaper than \
        recreate_session. Returns the updated session row as JSON. Address \
        the session with session_id OR host_alias + name.")]
    pub(super) async fn restart_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<RestartSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "restart_session",
            &format!(
                "session_id={:?} host={:?} name={:?}",
                p.session_id, p.host_alias, p.name
            ),
        );
        let (host_alias, name) = self.resolve_target(
            &caller,
            p.session_id,
            p.host_alias.as_deref(),
            p.name.as_deref(),
            "the session to restart",
        )?;
        let args = sessions::RestartSessionArgs {
            host_alias,
            name,
            force: p.force,
        };
        let row = sessions::restart_session(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Spawn a review session: a new Claude session in the \
        source session's worktree, seeded with a review prompt. Returns the \
        new review session row as JSON.")]
    pub(super) async fn spawn_review(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SpawnReviewParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "spawn_review",
            &format!("source_session_id={}", p.source_session_id),
        );
        // The review session is created on the source session's host.
        self.resolve_target_row(
            &caller,
            Some(p.source_session_id),
            None,
            None,
            "the session to review",
        )?;
        let args = sessions::SpawnReviewArgs {
            source_session_id: p.source_session_id,
            prompt: p.prompt,
            call_id: None,
        };
        let row = sessions::spawn_review(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Read a host's current system clipboard (whatever a \
        human would get from Ctrl+V on that machine). Probes wl-paste, xclip, \
        xsel, pbpaste in order. E_CLIPBOARD_UNAVAILABLE if none is installed.")]
    pub(super) async fn get_clipboard(
        &self,
        Parameters(p): Parameters<HostClipboardParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("get_clipboard", &format!("host={}", p.host_alias));
        let text = crate::service::clipboard::get_clipboard(
            crate::service::clipboard::GetClipboardArgs {
                host_alias: p.host_alias,
            },
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        // Empty clipboard would yield an empty text block, which the Anthropic
        // API rejects (see EMPTY_RESULT_PLACEHOLDER) — `ok_json` substitutes
        // safely for "" but only after JSON-encoding; say it explicitly.
        if text.is_empty() {
            return Ok(CallToolResult::success(vec![text_content(
                "(clipboard is empty)",
            )]));
        }
        ok_json(&text)
    }

    #[tool(description = "Write text to a host's system clipboard. Probes \
        wl-copy, xclip, xsel, pbcopy in order. Capped at 64 KiB. \
        E_CLIPBOARD_UNAVAILABLE if no clipboard helper is installed. May \
        return E_CONFIRM_REQUIRED when desktop confirmation is on.")]
    pub(super) async fn set_clipboard(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SetClipboardParams>,
    ) -> Result<CallToolResult, McpError> {
        // Content body intentionally not logged.
        audit(
            "set_clipboard",
            &format!("host={} bytes={}", p.host_alias, p.content.len()),
        );
        self.confirm_gate(
            "set_clipboard",
            p.confirm_nonce.as_deref(),
            &clipboard_summary(&p.host_alias, &p.content),
            &caller,
        )?;
        crate::service::clipboard::set_clipboard(
            crate::service::clipboard::SetClipboardArgs {
                host_alias: p.host_alias,
                content: p.content,
            },
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        Ok(CallToolResult::success(vec![text_content(
            "clipboard updated",
        )]))
    }

    #[tool(description = "Explicitly repair a session's workspace (the same \
        action as the Repair workspace button): make its directory a healthy \
        git worktree on its branch and its tmux session run there. Unlike the \
        automatic checks on create/restart/recreate/attach (which re-add a \
        missing worktree from its existing branch, dropping its own stale git \
        entry first only when the parent directory's dev:inode matches the one \
        recorded while the worktree was healthy), \
        this may unregister this worktree's own stale git entry \
        (git worktree remove --force; never a blanket prune), adopt its \
        branch's checkout elsewhere (refused when \
        another fleet workspace uses it), recreate the branch from the base \
        branch once origin confirms it is gone, run git worktree repair, and \
        respawn a live pane whose directory vanished. No-op on a healthy \
        session. Gated by mcp.confirm_destructive (retry with confirm_nonce). \
        Returns a JSON RepairReport: cwd, healthy, actions (in order), \
        warnings, branch_source, tmux (created|respawned), sibling_session_ids. \
        Errors: E_REPO_MISSING (never faked with mkdir), E_BRANCH_CHECKED_OUT, \
        E_WORKSPACE_LOCKED, E_REPAIR_FAILED, E_HOST_OFFLINE, E_CONFIRM_REQUIRED.")]
    pub(super) async fn repair_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<RepairSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "repair_session",
            &format!(
                "session_id={:?} host={:?} name={:?}",
                p.session_id, p.host_alias, p.name
            ),
        );
        // Mutating tool (not in READONLY_TOOLS): resolve + host-bind the
        // target like restart_session, then repair by the resolved row's id.
        let (host_alias, name) = self.resolve_target(
            &caller,
            p.session_id,
            p.host_alias.as_deref(),
            p.name.as_deref(),
            "the session to repair",
        )?;
        // Explicit repair can unregister a worktree entry, re-path a row and
        // respawn a live pane: destructive, so behind the desktop confirmation.
        self.confirm_gate(
            "repair_session",
            p.confirm_nonce.as_deref(),
            &format!("host={host_alias} name={name}"),
            &caller,
        )?;
        let id = {
            let s = self
                .store
                .lock()
                .map_err(|_| to_mcp_err(IpcError::lock()))?;
            s.get_session(&name, &host_alias)
                .map_err(|e| to_mcp_err(IpcError::from(e)))?
                .map(|r| r.id)
                .ok_or_else(|| {
                    to_mcp_err(IpcError::new(
                        "E_NOTFOUND",
                        format!("session {name} on {host_alias} not found"),
                    ))
                })?
        };
        let rep = crate::service::repair::repair_session(id, true, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&rep)
    }

    #[tool(description = "Move a work session to another host (replaces the \
        unbuilt Handoff): copy its Claude transcript to the target, create the \
        worktree there from the same branch, start it with --resume so the same \
        conversation continues, and only once the target is confirmed running \
        kill the source (keep_source=true leaves it running). Refused unless the \
        source worktree is clean (E_MOVE_DIRTY) and its branch is on origin with \
        nothing unpushed (E_MOVE_UNPUSHED; it never pushes for you); a \
        transcript over move.max_transcript_mb (default 200) is refused \
        (E_MOVE_TOO_LARGE). Nothing on the source changes before the target is \
        confirmed; a failure after the target started returns E_MOVE_PARTIAL \
        and leaves both sessions. Needs a token allowed on BOTH hosts (in \
        practice the master token). Gated by mcp.confirm_destructive (retry with \
        confirm_nonce). Returns a JSON MoveReport: source_session_id, \
        target_session_id, from_host, to_host, tmux_name, transcript_bytes, \
        source_killed, warnings, target (the new row, parent_session_id = source).")]
    pub(super) async fn move_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<MoveSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "move_session",
            &format!(
                "session_id={} target={} keep_source={}",
                p.session_id, p.target_host_alias, p.keep_source
            ),
        );
        crate::validate::host_alias(&p.target_host_alias).map_err(to_mcp_err)?;
        let row = self.resolve_target_row(
            &caller,
            Some(p.session_id),
            None,
            None,
            "the session to move",
        )?;
        require_move_hosts(&caller, &row.host_alias, &p.target_host_alias)?;
        self.confirm_gate(
            "move_session",
            p.confirm_nonce.as_deref(),
            &format!(
                "session_id={} from={} to={} keep_source={}",
                row.id, row.host_alias, p.target_host_alias, p.keep_source
            ),
            &caller,
        )?;
        let rep = crate::service::move_session::move_session(
            crate::service::move_session::MoveSessionArgs {
                session_id: row.id,
                target_host_alias: p.target_host_alias,
                keep_source: p.keep_source,
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&rep)
    }
}
