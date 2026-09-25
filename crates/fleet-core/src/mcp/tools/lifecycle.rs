//! MCP tools: kill, restart, rename, review, repair, move and the host clipboard.

use super::*;
use crate::ipc_error::codes;
use crate::ipc_error::lock;

#[tool_router(router = lifecycle_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Kill a session on a host: a tmux session by name, or \
        a background agent row (name `bg:<uuid>`, kind `bg`) via `claude stop`. \
        An inactive background agent (claude_status `stopped`) is removed from \
        the list instead, without `claude stop`. Rows of kind `external` \
        (interactive Claude sessions running outside fleet) are refused with \
        E_INVALID_STATE — close them where they run. Use when the session's work is disposable or already \
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
        self.confirm_gate(
            "safe_kill_session",
            p.confirm_nonce.as_deref(),
            &format!("host={host_alias} name={tmux_name}"),
            &caller,
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
        // The operator's restarts (a kill and a start) need a person (D12).
        self.confirm_gate(
            "restart_session",
            p.confirm_nonce.as_deref(),
            &format!(
                "host={} name={} force={}",
                bound_text(Some(&host_alias)),
                bound_text(Some(&name)),
                p.force
            ),
            &caller,
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
        Parameters(SpawnReviewParams {
            args,
            confirm_nonce,
        }): Parameters<SpawnReviewParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "spawn_review",
            &format!("source_session_id={}", args.source_session_id),
        );
        // The review session is created on the source session's host.
        let source = self.resolve_target_row(
            &caller,
            Some(args.source_session_id),
            None,
            None,
            "the session to review",
        )?;
        self.confirm_gate(
            "spawn_review",
            confirm_nonce.as_deref(),
            &format!(
                "source_session_id={} host={} name={} prompt={}",
                args.source_session_id,
                bound_text(Some(&source.host_alias)),
                bound_text(Some(&source.tmux_name)),
                bound_body(Some(&args.prompt))
            ),
            &caller,
        )?;
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
        Parameters(args): Parameters<crate::service::clipboard::GetClipboardArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit("get_clipboard", &format!("host={}", args.host_alias));
        let text = crate::service::clipboard::get_clipboard(args, &self.ssh)
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

    #[tool(
        description = "Repair a session workspace (the Repair workspace button): make its \
        directory a healthy git worktree on its branch and its tmux session run \
        there. Goes past the automatic create/restart/attach checks — may \
        unregister this worktree stale git entry, adopt its branch checkout \
        elsewhere, recreate the branch from base once origin confirms it is gone, \
        and respawn a pane whose directory vanished. No-op on a healthy session. \
        Call it after any tool answers E_REPAIR_REQUIRED, then retry that tool. \
        Gated by mcp.confirm_destructive. Returns a RepairReport (cwd, healthy, \
        actions, warnings, branch_source, tmux, sibling_session_ids). Errors: \
        E_REPO_MISSING, E_BRANCH_CHECKED_OUT, E_WORKSPACE_LOCKED, E_REPAIR_FAILED, \
        E_HOST_OFFLINE, E_CONFIRM_REQUIRED."
    )]
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
            let s = lock(&self.store).map_err(to_mcp_err)?;
            s.get_session(&name, &host_alias)
                .map_err(|e| to_mcp_err(IpcError::from(e)))?
                .map(|r| r.id)
                .ok_or_else(|| {
                    to_mcp_err(IpcError::new(
                        codes::E_NOTFOUND,
                        format!("session {name} on {host_alias} not found"),
                    ))
                })?
        };
        let rep = crate::service::repair::repair_session(id, true, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&rep)
    }

    #[tool(
        description = "Move a work session to another host, carrying its work as it is: the \
        Claude transcript, unpushed commits, staged/modified/untracked files and \
        small git-ignored files (.env); also the session's Claude directory \
        (subagent transcripts, tool results) and the project's Claude memory, \
        added to the target without replacing anything there (these two only \
        warn). Nothing is pushed, committed or stashed \
        and the source worktree is never modified; the target resumes the same \
        conversation and the source is killed only once the target runs \
        (keep_source=true leaves it). strict=true refuses instead of carrying: \
        E_MOVE_DIRTY, E_MOVE_UNPUSHED. \
        Errors: E_MOVE_MIDOP, E_MOVE_TARGET_DIRTY, \
        E_MOVE_TOO_LARGE, E_MOVE_CARRY, E_MOVE_PARTIAL (target started, both \
        sessions left), E_CONFIRM_REQUIRED, E_FORBIDDEN (cross-org; see \
        force_cross_org). Needs a token allowed on BOTH hosts (in practice the \
        master). Returns a moved report, a preview or a wait."
    )]
    // `clean_target`'s prose lives on the parameter itself rather than in the
    // sentence above: the served tool surface is capped
    // (`the_served_definition_budget_stays_bounded`), and a flag documented
    // where the client reads its schema costs the surface once, not twice.
    pub(super) async fn move_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<MoveSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        let dry_run = p.dry_run;
        let when = p.when;
        audit(
            "move_session",
            &format!(
                "session_id={} target={} keep_source={} strict={} clean_target={} dry_run={} \
                 when_is={:?} force_cross_org={}",
                p.session_id,
                p.target_host_alias,
                p.keep_source,
                p.strict,
                p.clean_target,
                dry_run,
                when,
                p.force_cross_org
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
        // `dry_run` changes nothing, and a `when` of `cancel` PREVENTS a
        // move — for both, asking the user to confirm would put a dialog
        // between them and the safe action. A `when` of `idle` is a
        // (deferred) move and keeps the gate. Safe to skip either way only
        // because `into_args` below maps these same fields onto
        // `MoveSessionArgs`: a preview still reveals the source's file list
        // and the target's state, and a cancel still needs to know which
        // host it targets, so the access checks above stay unconditional
        // regardless.
        if !dry_run && when != crate::service::move_session::When::Cancel {
            self.confirm_gate(
                "move_session",
                p.confirm_nonce.as_deref(),
                &format!(
                    "session_id={} from={} to={} keep_source={} strict={} clean_target={}",
                    row.id,
                    row.host_alias,
                    p.target_host_alias,
                    p.keep_source,
                    p.strict,
                    p.clean_target
                ),
                &caller,
            )?;
        }
        let outcome =
            crate::service::move_session::move_session(p.into_args(row.id), &self.store, &self.ssh)
                .await
                .map_err(to_mcp_err)?;
        ok_json(&outcome)
    }

    #[tool(
        description = "Finish or undo a partial move (E_MOVE_PARTIAL). finish kills the \
        source; undo kills the target and is refused if it took a turn or isn't idle."
    )]
    pub(super) async fn resolve_move(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<ResolveMoveParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::service::move_session::resolve::ResolveMoveAction;
        audit(
            "resolve_move",
            &format!("session_id={} action={}", p.session_id, p.action),
        );
        let action = match p.action.as_str() {
            "finish" => ResolveMoveAction::Finish,
            "undo" => ResolveMoveAction::Undo,
            other => {
                return Err(to_mcp_err(IpcError::new(
                    codes::E_VALIDATE,
                    format!("action must be \"finish\" or \"undo\", not {other:?}"),
                )))
            }
        };
        self.resolve_target(
            &caller,
            Some(p.session_id),
            None,
            None,
            "the partial move to resolve",
        )?;
        self.confirm_gate(
            "resolve_move",
            p.confirm_nonce.as_deref(),
            &format!("session_id={} action={}", p.session_id, p.action),
            &caller,
        )?;
        let rep = crate::service::move_session::resolve::resolve_move(
            crate::service::move_session::resolve::ResolveMoveArgs {
                session_id: p.session_id,
                action,
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&rep)
    }
}
