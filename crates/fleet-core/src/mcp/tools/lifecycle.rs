//! MCP tools: kill, restart, rename, review, repair, move and the host clipboard.

use super::*;
use crate::ipc_error::codes;
use crate::ipc_error::lock;
use crate::service::rewind;

#[tool_router(router = lifecycle_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Kill a session: a tmux session, or a background \
        agent row (`bg:<uuid>`, kind `bg`) via `claude stop`; an inactive \
        one (claude_status `stopped`) is removed from the list instead. Rows \
        of kind `external` (Claude running outside fleet) are refused with \
        E_INVALID_STATE: close them where they run. For disposable or \
        already-pushed work you want gone NOW; prefer safe_kill_session when \
        the worktree may hold unpushed work. Returns the killed id. May \
        return E_CONFIRM_REQUIRED when confirmation is on \
        (mcp.confirm_destructive).")]
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
            // `own`: it destroys the owner's work (spec §4.3, invariant 5).
            Reach::Own,
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

    #[tool(description = "A session's shell terminals: tmux sessions \
        <name>--sh<N> (N 1-9) beside its agent, started in the agent's \
        directory (at=home: home) and never listed as sessions. \
        action=list (default), open (n, or the lowest free) or close (n). \
        Closing one never stops the session. Returns the open terminals, \
        each with the command running in it.")]
    pub(super) async fn shell_terminals(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<ShellTerminalsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "shell_terminals",
            &format!(
                "session_id={} action={:?} n={:?}",
                p.session_id, p.action, p.n
            ),
        );
        let row = self.resolve_target_row(
            &caller,
            Some(p.session_id),
            None,
            None,
            // `own`: a terminal is a shell on the owner's host, which a
            // grant never confers (spec §4.3, invariant 4).
            Reach::Own,
            "the session whose terminals to manage",
        )?;
        let out = sessions::shell_terminals(
            sessions::ShellTerminalsArgs {
                session_id: row.id,
                action: p.action,
                n: p.n,
                at: p.at,
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&out)
    }

    #[tool(description = "Ask a running Claude session to persist its work \
        (commit + push), then arm deletion of its worktree + tmux session: \
        for retiring a session that may hold unpushed work. Returns the row \
        with safe_kill_state=requested; the delete fires only after the \
        SAFE_REMOVE_READY marker AND a clean-tree check ('ready' / 'failed' \
        arrive as row events).")]
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
        let row = self.resolve_target_row(
            &caller,
            p.session_id,
            p.host_alias.as_deref(),
            p.tmux_name.as_deref(),
            // `own`: `safe_kill_session` is a kill with a commit-and-push in
            // front of it, and the spec's invariant names both.
            Reach::Own,
            "the session to retire",
        )?;
        let (host_alias, tmux_name) = (row.host_alias, row.tmux_name);
        // Session care: the operator retires its own handoff without a card
        // at Autopilot (`guard::OPERATOR_CARE_TOOLS`).
        self.confirm_gate_target(
            "safe_kill_session",
            p.confirm_nonce.as_deref(),
            &format!("host={host_alias} name={tmux_name}"),
            &caller,
            GateTarget::Session(row.id),
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

    #[tool(description = "Rename a tmux session. Returns the updated row.")]
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
            // `own`: the tmux name is the row's identity and the spec's
            // invariant names `rename_session` — the label is content.
            Reach::Own,
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

    #[tool(description = "Set the session's friendly display name, once per \
        task by the in-session agent (3–6 words; empty clears). Returns the \
        updated row.")]
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
            // `drive`, NOT `own`, with spec §4.3's own reasoning (fix round 3,
            // which found this sentence saying something else): READING
            // `friendly_name` IS content — it is a row of that section's
            // content table, so a stranger never sees it — but WRITING it is
            // not an owner-only act, and the two halves are not in conflict.
            // `rename_session` changes the tmux NAME, which is the row's
            // ADDRESS; `set_session_tags` writes a durable classification the
            // work graph and the sidebar filters read. The friendly label is
            // the sidebar CAPTION, and fleet writes it itself without asking
            // anybody (`sessions::label_from_prompt` on a background agent's
            // first prompt, `fill_session_name` and `tickets::start_one` on a
            // start) — a field the hub rewrites on its own is not a field the
            // owner-only tier is protecting, and a driver correcting the
            // caption of the session it is driving changes nothing the owner
            // cannot see and re-set. The desktop's `SESSION_TIER` carries the
            // same tier, now with the same reason.
            Reach::Drive,
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

    #[tool(description = "Mark a session viewed now: the turns it has \
        finished read as seen. Returns the row.")]
    pub(super) async fn touch_session_viewed(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<sessions::TouchSessionViewedArgs>,
    ) -> Result<CallToolResult, McpError> {
        // `drive`: the stamp is on the ROW, one for everybody, so a watcher
        // looking must not clear what the owner has not yet seen. A watcher's
        // client is refused and simply keeps its own view of the row.
        self.resolve_target_row(
            &caller,
            Some(args.session_id),
            None,
            None,
            Reach::Drive,
            "the session viewed",
        )?;
        let row = sessions::touch_session_viewed(args, &self.store).map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Pin or unpin a session. A pinned session is \
        listed in the Pinned section, and kill_session, safe_kill_session and \
        tidy-up refuse it (E_PINNED) until it is unpinned. Returns the row.")]
    pub(super) async fn pin_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<sessions::PinSessionArgs>,
    ) -> Result<CallToolResult, McpError> {
        // `drive`: the pin is on the ROW, one for everybody, and it decides
        // whether the owner's own kill goes through.
        self.resolve_target_row(
            &caller,
            Some(args.session_id),
            None,
            None,
            Reach::Drive,
            "the session pinned",
        )?;
        audit(
            "pin_session",
            &format!("session_id={} pinned={}", args.session_id, args.pinned),
        );
        let row = sessions::pin_session(args, &self.store).map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Restart a tmux session in place (kill and \
        recreate): for a wedged Claude REPL whose tmux and worktree are \
        fine; cheaper than recreate_session. Returns the updated row.")]
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
        let row = self.resolve_target_row(
            &caller,
            p.session_id,
            p.host_alias.as_deref(),
            p.name.as_deref(),
            // `own`: it re-creates the session (spec §4.3, invariant 5). A
            // `drive` grantee may make the machine work, not dispose of it.
            Reach::Own,
            "the session to restart",
        )?;
        let (host_alias, name) = (row.host_alias, row.tmux_name);
        // A switch onto another login (review w02-8) takes the checks a
        // start on it takes: not onto a login fleet knows is signed out
        // (the pane would ask for `/login`), and not past
        // `accounts.pause_at` unless the person chose it. A person's
        // device picking a login in Switch account HAS chosen it (the picker
        // shows each login's headroom, and the desktop sends no
        // `over_limit_ok` on a restart), so only the operator, an agent's
        // per-host token, a single-purpose token and the master are held —
        // as a standalone desktop's `sessions::restart_session` lets the
        // person's switch through. Any paired device counts, an org-bound
        // one too (`fleet-hub pair --org` is still somebody's device):
        // `is_person_device` leaves those out, which this check must not.
        let persons_device = caller.is_client()
            && caller.host_alias.is_none()
            && !caller.mode.is_single_purpose()
            && !caller.is_operator();
        let person_chose = p.over_limit_ok.unwrap_or(false) || persons_device;
        if let Some(target) = p.profile.as_deref() {
            let target = Some(target.trim()).filter(|t| !t.is_empty());
            let s = lock(self.reader()).map_err(to_mcp_err)?;
            let switches = s
                .get_session(&name, &host_alias)
                .map_err(|e| to_mcp_err(e.into()))?
                .is_some_and(|r| {
                    r.agent == crate::store::AGENT_CLAUDE
                        && r.kind != "shell"
                        && r.claude_profile.as_deref() != target
                });
            if switches {
                if let Some(why) =
                    crate::service::login_health::signed_out_reason(&s, &host_alias, target)
                        .map_err(to_mcp_err)?
                {
                    return Err(to_mcp_err(IpcError::new(
                        codes::E_INVALID_STATE,
                        format!("{why}; sign it in again or switch to another login"),
                    )));
                }
                if !person_chose {
                    crate::service::account_limits::refuse_over_limit(
                        &s,
                        &host_alias,
                        target,
                        crate::store::now_unix(),
                    )
                    .map_err(to_mcp_err)?;
                }
            }
        }
        // The operator's restarts (a kill and a start) need a person (D12),
        // unless session care lets one of its own handoffs through
        // (`guard::OPERATOR_CARE_TOOLS`).
        self.confirm_gate_target(
            "restart_session",
            p.confirm_nonce.as_deref(),
            &format!(
                "host={} name={} force={} profile={} model={} effort={}",
                bound_text(Some(&host_alias)),
                bound_text(Some(&name)),
                p.force,
                bound_text(p.profile.as_deref()),
                bound_text(p.model.as_deref()),
                bound_text(p.effort.as_deref())
            ),
            &caller,
            GateTarget::Session(row.id),
        )?;
        let args = sessions::RestartSessionArgs {
            host_alias,
            name,
            force: p.force,
            profile: p.profile,
            model: p.model,
            effort: p.effort,
        };
        let row = sessions::restart_session(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Truncate a session's transcript into a new \
        conversation: \"fork\" starts a new session there, \"rewind\" \
        restarts this one. The original is unchanged. Returns the row (a \
        fork's is the new session).")]
    pub(super) async fn rewind_conversation(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<RewindConversationParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "rewind_conversation",
            &format!(
                "session_id={} mode={} anchor={:?}",
                p.session_id, p.mode, p.anchor_uuid
            ),
        );
        let mode = match p.mode.as_str() {
            "rewind" => rewind::RewindMode::Rewind,
            "fork" => rewind::RewindMode::Fork,
            other => {
                return Err(to_mcp_err(IpcError::new(
                    codes::E_INVALID,
                    format!("mode must be \"rewind\" or \"fork\", not {other:?}"),
                )))
            }
        };
        // Fence the target the way every other session-addressed write does
        // (`restart_session` above, `move_session`, `spawn_review`): resolve
        // the row through the store, so a per-host token cannot rewind
        // another host's session, and apply the org boundary and the person
        // gate on top — a session the caller may not see answers as a
        // missing one.
        //
        // This was TWO gates until multi-user M1's T7: a
        // `require_visible_session` call sat in front of the resolve,
        // checking the org boundary for a scoped caller and nothing at all
        // for anyone else. `resolve_target_row` now carries both, with the
        // reach.
        let row = self.resolve_target_row(
            &caller,
            Some(p.session_id),
            None,
            None,
            // `own`: fork, rewind and retry alike make a permanent verbatim
            // copy of the transcript (spec §4.3, invariant 5).
            Reach::Own,
            "the session to rewind",
        )?;
        // Both modes need a person when the operator asks (D12): rewind
        // rebuilds a live pane, the same rule restart_session follows, and
        // fork starts a new session — an operator's start is always
        // confirmed (M9.7). The mode is bound into the summary, so an
        // approved fork cannot be replayed as a rewind. For anyone else the
        // gate is a no-op (`rewind_conversation` is not `confirm: true`).
        self.confirm_gate(
            "rewind_conversation",
            p.confirm_nonce.as_deref(),
            &format!(
                "session_id={} mode={} anchor={:?} new_worktree={:?}",
                p.session_id, p.mode, p.anchor_uuid, p.new_worktree
            ),
            &caller,
        )?;
        let row = rewind::rewind_conversation(
            rewind::RewindArgs {
                session_id: row.id,
                anchor_uuid: p.anchor_uuid,
                mode,
                new_worktree: p.new_worktree,
            },
            &self.store,
            &self.ssh,
            &self.reg,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Spawn a review session: a new Claude session in \
        the source session's worktree, seeded with a review prompt. Returns \
        its row.")]
    pub(super) async fn spawn_review(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(SpawnReviewParams {
            mut args,
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
            // `own`: `spawn_review` creates a session in the owner's worktree on
            // the owner's host, with a terminal (spec §4.3, invariant 5). The
            // new session inherits the SOURCE's owner, never the caller's
            // (invariant 6), which T5 wired.
            Reach::Own,
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
        args.origin = Some({
            let s = lock(self.reader()).map_err(to_mcp_err)?;
            super::fleet::origin_for(&caller, &s)
        });
        let row = sessions::spawn_review(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Read a host's system clipboard (what Ctrl+V gives \
        there), via wl-paste, xclip, xsel or pbpaste. \
        E_CLIPBOARD_UNAVAILABLE if none is installed.")]
    pub(super) async fn get_clipboard(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<crate::service::clipboard::GetClipboardArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit("get_clipboard", &format!("host={}", args.host_alias));
        self.require_clipboard_host(&caller, &args.host_alias)?;
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

    #[tool(description = "Write a host's system clipboard, via wl-copy, \
        xclip, xsel or pbcopy. E_CLIPBOARD_UNAVAILABLE if none is installed. \
        May return E_CONFIRM_REQUIRED when confirmation is on \
        (mcp.confirm_destructive).")]
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
        self.require_clipboard_host(&caller, &p.host_alias)?;
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

    #[tool(description = "Repair a session workspace (the Repair workspace \
        button): make its directory a healthy git worktree on its branch \
        with its tmux session running there. Goes past the automatic checks: \
        may unregister a stale worktree entry, adopt its branch checkout \
        elsewhere, recreate the branch from base once origin confirms it is \
        gone, and respawn a pane whose directory vanished. No-op when \
        healthy. Call it after any tool answers E_REPAIR_REQUIRED, then \
        retry that tool. progress_token: steps arrive as repair:progress. \
        Returns a RepairReport. Errors: E_REPO_MISSING, \
        E_BRANCH_CHECKED_OUT, E_WORKSPACE_LOCKED, E_REPAIR_FAILED, \
        E_HOST_OFFLINE, E_CONFIRM_REQUIRED (gated by \
        mcp.confirm_destructive).")]
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
            // `drive`: it unregisters a worktree entry, re-paths a row and
            // respawns a pane — writes to the machine, not a copy, a relocation
            // or a destruction, so it is not in the spec's `own` list.
            Reach::Drive,
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
        let rep = crate::service::repair::repair_session_reporting(
            id,
            true,
            &self.store,
            &self.ssh,
            p.progress_token,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&rep)
    }

    #[tool(description = "Move a work session to another host, carrying its \
        work as it is: the transcript, unpushed commits, \
        staged/modified/untracked and small git-ignored files (.env), plus \
        the session's Claude directory and the project's Claude memory \
        (added without replacing anything; these two only warn). Nothing is \
        pushed, committed or stashed and the source worktree is never \
        modified; the target resumes the conversation and the source is \
        killed only once the target runs. strict refuses instead of \
        carrying. Errors: E_MOVE_MIDOP, E_MOVE_TARGET_DIRTY, \
        E_MOVE_TOO_LARGE, E_MOVE_CARRY, E_MOVE_PARTIAL (target started, both \
        sessions left), E_CONFIRM_REQUIRED, E_FORBIDDEN (cross-org; see \
        force_cross_org). Needs a token allowed on BOTH hosts (in practice the \
        master). Returns a moved report, a preview or a wait.")]
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
            // `own`: a move relocates the working tree, Claude directory and
            // project memory onto a host the caller names, where they are the
            // unix owner (spec §4.3, and the `move_session` section beneath it).
            // The grants on the source row are dropped by the move itself.
            Reach::Own,
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
            // `own`: `resolve_move` finishes or undoes a move by KILLING one of
            // the two sessions. It is the move's second half, and cannot be
            // narrower than the move.
            Reach::Own,
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

impl FleetTools {
    /// The clipboard tools' host fence (review w25-1): a host's clipboard is
    /// whatever the person at that keyboard last copied, a password
    /// included. A per-host token reaches only its own host's (the session
    /// on `build-box` must not read the Mac's), and an org-bound client only
    /// its org's hosts'.
    fn require_clipboard_host(&self, caller: &Caller, host_alias: &str) -> Result<(), McpError> {
        require_host(caller, host_alias, "the clipboard")?;
        let s = lock(&self.store).map_err(to_mcp_err)?;
        require_bound_client_sees_host(&s, caller, host_alias)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `new_worktree` reaches the fork engine: before this build it came back
    /// `E_UNSUPPORTED` at the top of `rewind_conversation`; now the first
    /// refusal it can meet is the engine's own (a session with no project
    /// has nowhere to create a worktree), before any I/O.
    #[tokio::test]
    async fn a_new_worktree_fork_is_no_longer_unsupported() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h1").unwrap();
        let id = s
            .upsert_session("sess", "h1", None, None, 0, 0, "running", None)
            .unwrap();
        s.set_claude_session_id(id, "11111111-1111-1111-1111-111111111111")
            .unwrap();
        let t = FleetTools::new(
            Arc::new(Mutex::new(s)),
            Arc::new(SshClient::new()),
            CancellationRegistry::new(),
            Arc::new(crate::service::tunnel::TunnelSupervisor::new()),
            McpGuards::new(Arc::new(|_: &guard::ConfirmRequest| {})),
        );
        let err = t
            .rewind_conversation(
                Extension(Caller::master()),
                Parameters(RewindConversationParams {
                    session_id: id,
                    anchor_uuid: None,
                    mode: "fork".into(),
                    new_worktree: Some("fork-of-sess".into()),
                    confirm_nonce: None,
                }),
            )
            .await
            .expect_err("no project to fork into");
        let text = format!("{err:?}");
        assert!(!text.contains("E_UNSUPPORTED"), "{text}");
        assert!(text.contains("no project to fork"), "{text}");
    }

    fn clipboard_tools() -> FleetTools {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("mac").unwrap();
        s.upsert_host("build-box").unwrap();
        let org = s.add_org("Acme", None, false).unwrap().id;
        s.set_host_org("build-box", Some(org)).unwrap();
        s.set_setting(crate::service::hub::SETTING_LOCAL_HOST, "false")
            .unwrap();
        FleetTools::new(
            Arc::new(Mutex::new(s)),
            Arc::new(SshClient::new()),
            CancellationRegistry::new(),
            Arc::new(crate::service::tunnel::TunnelSupervisor::new()),
            McpGuards::new(Arc::new(|_: &guard::ConfirmRequest| {})),
        )
    }

    fn host_token(alias: &str) -> Caller {
        Caller {
            api: None,
            host_alias: Some(alias.into()),
            client: None,
            mode: crate::mcp::auth::TokenMode::Full,
            pane: None,
            is_personal_owner: false,
            is_fleet_admin: false,
        }
    }

    fn org_bound_client(org_id: i64) -> Caller {
        Caller {
            api: None,
            host_alias: None,
            client: Some(crate::mcp::auth::ClientRef {
                id: 7,
                name: "acme-phone".into(),
                trusted: false,
                org_id: Some(org_id),
                person_id: Some(2),
            }),
            mode: crate::mcp::auth::TokenMode::Full,
            pane: None,
            is_personal_owner: false,
            is_fleet_admin: false,
        }
    }

    fn assert_forbidden(r: Result<CallToolResult, McpError>) {
        let e = r.expect_err("the fence refuses");
        assert!(e.message.starts_with("E_FORBIDDEN"), "{}", e.message);
    }

    /// Review w25-1: the session on `build-box` must not read or replace
    /// what the owner last copied on the Mac. Both tools refuse before any
    /// SSH, and before `set_clipboard`'s confirm gate.
    #[tokio::test]
    async fn a_host_token_reaches_no_other_hosts_clipboard() {
        let t = clipboard_tools();
        assert_forbidden(
            t.get_clipboard(
                Extension(host_token("build-box")),
                Parameters(crate::service::clipboard::GetClipboardArgs {
                    host_alias: "mac".into(),
                }),
            )
            .await,
        );
        assert_forbidden(
            t.set_clipboard(
                Extension(host_token("build-box")),
                Parameters(SetClipboardParams {
                    host_alias: "mac".into(),
                    content: "rm -rf ~".into(),
                    confirm_nonce: None,
                }),
            )
            .await,
        );
        // Its own host's clipboard stays reachable.
        assert!(t
            .require_clipboard_host(&host_token("build-box"), "build-box")
            .is_ok());
        assert!(t.require_clipboard_host(&Caller::master(), "mac").is_ok());
    }

    /// Review w25-1: an org-bound client reaches only its org's hosts'
    /// clipboards.
    #[tokio::test]
    async fn an_org_bound_client_reaches_only_its_orgs_clipboards() {
        let t = clipboard_tools();
        let org = {
            let s = lock(&t.store).unwrap();
            s.host_org("build-box").unwrap().unwrap()
        };
        let other = {
            let s = lock(&t.store).unwrap();
            let o = s.add_org("Other", None, false).unwrap().id;
            s.set_host_org("mac", Some(o)).unwrap();
            o
        };
        assert_ne!(org, other);
        assert_forbidden(
            t.get_clipboard(
                Extension(org_bound_client(org)),
                Parameters(crate::service::clipboard::GetClipboardArgs {
                    host_alias: "mac".into(),
                }),
            )
            .await,
        );
        assert_forbidden(
            t.set_clipboard(
                Extension(org_bound_client(org)),
                Parameters(SetClipboardParams {
                    host_alias: "mac".into(),
                    content: "x".into(),
                    confirm_nonce: None,
                }),
            )
            .await,
        );
        assert!(t
            .require_clipboard_host(&org_bound_client(org), "build-box")
            .is_ok());
    }
}
