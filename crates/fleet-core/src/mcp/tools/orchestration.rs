//! MCP tools: bounded waits, transcripts, run_prompt and tasks.

use super::*;
use crate::ipc_error::lock;

#[tool_router(router = orchestration_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Block until a session reaches a state, or \
        timeout_s. until=\"idle\": claude_status is idle | completed | \
        stopped | failed (true even before a first turn). until=\"turn_gt\": \
        turn_seq > `turn`; pass send_prompt's turn_seq_before to wait for \
        the reply to YOUR prompt. Polls every 500 ms. Returns { status: \
        satisfied | timeout, claude_status, turn_seq, last_stop_at, \
        stuck_kind }. Read-only; a per-host token only for sessions on its \
        own host.")]
    pub(super) async fn wait_for_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<WaitForSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "wait_for_session",
            &format!(
                "session_id={} until={} turn={:?} timeout_s={:?}",
                p.session_id, p.until, p.turn, p.timeout_s
            ),
        );
        let row =
            self.resolve_target_row(&caller, Some(p.session_id), None, None, "the session")?;
        let _permit = self.long_poll_permit(&caller, "wait_for_session")?;
        let cond = tasks::WaitCond::parse(&p.until, p.turn).map_err(to_mcp_err)?;
        let out =
            tasks::wait_for_session(&self.store, row.id, cond, tasks::wait_timeout(p.timeout_s))
                .await
                .map_err(to_mcp_err)?;
        ok_json(&serde_json::json!({
            "status": if out.satisfied { "satisfied" } else { "timeout" },
            "claude_status": out.row.claude_status,
            "turn_seq": out.row.turn_seq,
            "last_stop_at": out.row.last_stop_at,
            "stuck_kind": out.row.stuck_kind,
        }))
    }

    #[tool(description = "Read a session's Claude Code transcript (the \
        JSONL, not the pane): the last assistant turn as plain text, text \
        blocks verbatim, one line per tool call, no thinking. Errors: \
        E_INVALID_STATE (no claude_session_id yet), E_NO_TRANSCRIPT (nothing \
        written yet). Read-only; prefer it over capture_session for the \
        reply. unchanged costs no transcript read.")]
    pub(super) async fn session_transcript(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SessionTranscriptParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "session_transcript",
            &format!(
                "session_id={} since_turn={:?} max_chars={:?} fresh_for={:?}",
                p.session_id, p.since_turn, p.max_chars, p.fresh_for
            ),
        );
        let row =
            self.resolve_target_row(&caller, Some(p.session_id), None, None, "the session")?;

        // fresh_for absent: today's default, byte-identical, no cursor
        // touched — this branch must stay a straight pass-through.
        let Some(reader) = p.fresh_for else {
            let text = self.transcript_for(&row, p.since_turn, p.max_chars).await?;
            if text.trim().is_empty() {
                return Ok(CallToolResult::success(vec![text_content(
                    "(no assistant text in the requested turns)",
                )]));
            }
            return Ok(CallToolResult::success(vec![text_content(text)]));
        };

        // Scope 1: everything that needs the store, resolved BEFORE the
        // (async, SSH-backed) transcript read — never held across an await,
        // never re-entered once released (ruling: this codebase shipped a
        // real deadlock doing that).
        let resource_key = row.id.to_string();
        let (decision, generation, stored_anchor, stored_watermark) = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            let reader_exists = resolve_reader(&s, &caller, reader)?;
            let stored = s
                .get_read_cursor(reader, "session_transcript", &resource_key)
                .map_err(to_mcp_err)?;
            let generation = s.conversation_generation(row.id).map_err(to_mcp_err)?;
            let decision = stream_decision(
                reader_exists,
                stored.as_ref(),
                Some(row.turn_seq),
                generation,
            );
            let stored_anchor = stored.as_ref().and_then(|c| c.anchor.clone());
            let stored_watermark = stored.as_ref().and_then(|c| c.watermark);
            (decision, generation, stored_anchor, stored_watermark)
        };

        // `unchanged` means precisely: NO COMPLETED TURN since this reader's
        // last read. It is keyed on `turn_seq` (+ generation), and only a
        // Stop hook moves `turn_seq` — so an interrupted turn or a slash
        // command, which adds to the transcript file with no Stop behind
        // it, can be answered "unchanged" here while the session sits idle.
        // Nothing is lost: the anchor has not moved, so that content is
        // served with the next completed turn. Deliberately not a second
        // watermark (Ruling 18); documented in docs/control-api.md.
        if matches!(decision, fresh::StreamStart::Unchanged) {
            return Ok(CallToolResult::success(vec![text_content(format!(
                "(unchanged since your last read at turn {})",
                row.turn_seq
            ))]));
        }

        // `turn_seq` (+ generation) only says something changed; it is not
        // a position in the transcript FILE (an in-progress turn, an
        // interrupt, a slash command or a queued prompt each add a file
        // turn with no Stop behind it — see `TranscriptAnchor`'s doc). The
        // stored ANCHOR is the position. An `After` decision with no
        // anchor on record — never written, or unparseable — cannot be
        // positioned either, so it is answered full (`too_far_behind`)
        // instead of guessing which file turns are actually new.
        let anchor: Option<transcript::TranscriptAnchor> = stored_anchor
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok());
        let positioned_after = matches!(decision, fresh::StreamStart::After(_));
        let anchor_missing_for_after = positioned_after && anchor.is_none();
        let anchor_for_read = if positioned_after {
            anchor.as_ref()
        } else {
            None
        };

        let max_chars = p
            .max_chars
            .unwrap_or(transcript::DEFAULT_MAX_CHARS)
            .clamp(1, transcript::MAX_MAX_CHARS);
        // Today's default window (last turn) — only used when there is no
        // anchor to position from (a first `Full` read, or the
        // `too_far_behind` fallback below).
        let args = transcript::resolve_args(&self.store, &row, 1, max_chars).map_err(to_mcp_err)?;
        let delta = transcript::fetch_transcript_after(args, anchor_for_read, &self.ssh)
            .await
            .map_err(to_mcp_err)?;

        let decision_reason = match decision {
            fresh::StreamStart::Full(r) => r,
            _ => None,
        };
        let reason = if delta.too_far_behind || anchor_missing_for_after {
            Some(fresh::ResetReason::TooFarBehind)
        } else {
            decision_reason
        };

        // Scope 2: written only now that the read has succeeded, never for
        // a reader that does not exist — there is no one to remember a
        // cursor for. The generation is stored on every write here
        // (including this reset path), so the next read can detect a
        // boundary. When this read served nothing new (`delta.anchor` is
        // `None`), the PREVIOUS anchor is kept rather than cleared, so the
        // next read can still position from where the last one actually
        // left off.
        //
        // The watermark only advances to the CURRENT `row.turn_seq` when
        // this page was not itself truncated by `max_chars` (`!more`):
        // `turn_seq` is the cheap "anything changed" signal the Unchanged
        // fast path trusts, and while a multi-page catch-up is still in
        // progress the reader has NOT seen everything as of `turn_seq` yet
        // — advancing it early would make the next call answer Unchanged
        // (turn_seq already matches the head) despite pages still pending,
        // silently ending the catch-up. `fetch_transcript_after` never sets
        // `more` on the default-window (`Full`) path (it only ever serves
        // one turn), so this only holds an After read back, and only for
        // as many calls as the reader's own `max_chars` forces.
        if reason != Some(fresh::ResetReason::ReaderUnknown) {
            let anchor_to_store = match &delta.anchor {
                Some(a) => Some(serde_json::to_string(a).map_err(|e| {
                    McpError::internal_error(format!("serialize anchor: {e}"), None)
                })?),
                None => stored_anchor,
            };
            let watermark_to_store = if delta.more {
                stored_watermark.unwrap_or(row.turn_seq)
            } else {
                row.turn_seq
            };
            let s = lock(&self.store).map_err(to_mcp_err)?;
            s.put_stream_cursor(
                reader,
                "session_transcript",
                &resource_key,
                Some(row.id),
                watermark_to_store,
                generation,
                anchor_to_store.as_deref(),
            )
            .map_err(to_mcp_err)?;
        }

        let body = if delta.text.trim().is_empty() {
            "(no assistant text in the requested turns)".to_string()
        } else {
            delta.text
        };
        let body = format_transcript_more(body, delta.more);
        Ok(CallToolResult::success(vec![text_content(
            format_transcript_full(body, reason),
        )]))
    }

    #[tool(description = "Read a session conversation as structured turns \
        (session_transcript is one flat blob): each turn's prompt, \
        timestamps and items by kind (text, tool, subagent, compact, \
        command, interrupt; tool inputs and results are never included), \
        plus events (the conversation timeline) and context (context-window \
        usage, or null). Read-only. Errors: E_INVALID, E_INVALID_STATE, \
        E_NO_TRANSCRIPT.")]
    pub(super) async fn session_conversation(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SessionConversationParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "session_conversation",
            &format!(
                "session_id={} turns={:?} since_turn={:?} claude_session_id={:?} events_limit={:?}",
                p.session_id, p.turns, p.since_turn, p.claude_session_id, p.events_limit
            ),
        );
        let row =
            self.resolve_target_row(&caller, Some(p.session_id), None, None, "the session")?;
        let (turns, max_chars) = transcript::conv_limits(transcript::conv_turns_for(
            p.turns,
            p.since_turn,
            row.turn_seq,
        ));
        let conv = transcript::fetch_conversation_for_row(
            &self.store,
            &self.ssh,
            &row,
            p.claude_session_id.as_deref(),
            turns,
            max_chars,
            transcript::conv_events_limit(p.events_limit),
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json_compact(&conv)
    }

    #[tool(description = "send_prompt + wait_for_session(turn_gt) + \
        session_transcript in one call. Returns { turn_seq, status: \
        satisfied | timeout, transcript } (the reply as plain text; null \
        with transcript_error when unreadable). Marked untrusted unless \
        raw=true (master token only).")]
    pub(super) async fn run_prompt(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<RunPromptParams>,
    ) -> Result<CallToolResult, McpError> {
        // Prompt body intentionally not logged.
        audit(
            "run_prompt",
            &format!("session_id={} timeout_s={:?}", p.session_id, p.timeout_s),
        );
        let row = self.resolve_target_row(
            &caller,
            Some(p.session_id),
            None,
            None,
            "the session to prompt",
        )?;
        run_prompt_ready(&row)?;
        let _permit = self.long_poll_permit(&caller, "run_prompt")?;
        let prompt = apply_marker(p.prompt, &marker_origin(&caller), &caller, p.raw)?;
        let before = row.turn_seq;
        self.deliver_prompt(&row, prompt, true, false).await?;
        let out = tasks::wait_for_session(
            &self.store,
            row.id,
            tasks::WaitCond::TurnGt(before),
            tasks::wait_timeout(p.timeout_s),
        )
        .await
        .map_err(to_mcp_err)?;
        let (transcript, transcript_error) = match self
            .transcript_for(&out.row, Some(before), p.max_chars)
            .await
        {
            Ok(t) => (Some(t), None),
            Err(e) => (None, Some(e.message)),
        };
        ok_json(&serde_json::json!({
            "session_id": row.id,
            "turn_seq": out.row.turn_seq,
            "status": if out.satisfied { "satisfied" } else { "timeout" },
            "claude_status": out.row.claude_status,
            "transcript": transcript,
            "transcript_error": transcript_error,
        }))
    }

    #[tool(description = "Dispatch work to a worker session and track it as \
        a task. The prompt gets an appended instruction to print \
        FLEET_TASK_DONE_<nonce> on its own line followed by a one-paragraph \
        result; on the worker's next Stop fleet flips the task to done with \
        that paragraph as `result` (also sent to the requester's inbox as \
        kind=task_result). Returns the task row; follow with wait_for_task. \
        A per-host token must name a requester on its own host. Marked \
        untrusted unless raw=true (master only).")]
    pub(super) async fn dispatch_task(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<DispatchTaskParams>,
    ) -> Result<CallToolResult, McpError> {
        // Prompt body intentionally not logged.
        audit(
            "dispatch_task",
            &format!(
                "worker_session_id={:?} new_worker={:?} requester_session_id={:?}",
                p.worker_session_id,
                p.new_worker
                    .as_ref()
                    .map(|w| format!("{}:{}:{:?}", w.host_alias, w.project_id, w.name)),
                p.requester_session_id
            ),
        );
        if p.worker_session_id.is_some() == p.new_worker.is_some() {
            return Err(mcp_err(
                "E_INVALID",
                "pass exactly one of worker_session_id or new_worker",
                None,
            ));
        }
        // The requester (when given) must exist and, for a per-host caller,
        // live on that host — otherwise any agent could file tasks as anyone.
        if let Some(req) = p.requester_session_id {
            self.resolve_target_row(&caller, Some(req), None, None, "requester_session_id")?;
        }
        // Resolve or spawn the worker.
        let (worker, spawned) = match (p.worker_session_id, p.new_worker) {
            (Some(id), _) => (
                self.resolve_target_row(&caller, Some(id), None, None, "the worker session")?,
                false,
            ),
            (None, Some(spec)) => {
                // An empty name is new_session's "pick one for me": the
                // backend generates it (fill_session_name), so workers follow
                // the same convention as every other session.
                // B1: a per-host token may only spawn workers on its host
                // (it could otherwise read another host's output back via
                // wait_for_task / list_tasks / its inbox).
                require_host(&caller, &spec.host_alias, "the new worker")?;
                // A new worker is a start: the operator's needs a person (D12).
                self.confirm_gate(
                    "dispatch_task",
                    p.confirm_nonce.as_deref(),
                    &dispatch_new_worker_summary(&spec, p.requester_session_id, p.raw, &p.prompt),
                    &caller,
                )?;
                let name = spec.name.unwrap_or_default();
                let row = sessions::new_session(
                    sessions::NewSessionArgs {
                        host_alias: spec.host_alias,
                        project_id: spec.project_id,
                        worktree_id: None,
                        name,
                        call_id: None,
                        new_worktree: None,
                        base_branch: None,
                        kind: None,
                        start_command: None,
                        friendly_name: None,
                        resume_claude_session_id: None,
                    },
                    &self.store,
                    &self.ssh,
                    &self.reg,
                )
                .await
                .map_err(to_mcp_err)?;
                (row, true)
            }
            (None, None) => unreachable!("validated above"),
        };
        // Create the task row and link the worker to its requester.
        let task = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            let task = tasks::create_task(&s, p.requester_session_id, Some(worker.id), &p.prompt)
                .map_err(to_mcp_err)?;
            if let Some(req) = p.requester_session_id {
                let _ = s.set_parent_session_id(worker.id, Some(req));
                // The worker does the requester's work (work graph M2.2).
                let _ = s.inherit_worker_work(worker.id, req);
            }
            if let Some(cid) = worker.claude_session_id.as_deref() {
                let _ = s.set_task_worker_claude_id(task.id, cid);
            }
            task
        };
        if spawned {
            // A freshly launched REPL needs a moment before it accepts
            // typed input; wait (bounded) for its input chrome.
            tasks::wait_for_repl_ready(&self.ssh, &worker.host_alias, &worker.tmux_name).await;
        }
        let body = task_delivery_body(&p.prompt, &task.nonce, &caller, p.raw)?;
        match self.deliver_prompt(&worker, body, true, false).await {
            Ok(_) => {}
            Err(e) => {
                if let Ok(s) = self.store.lock() {
                    let _ = tasks::fail_task(&s, task.id, &e.message);
                }
                return Err(e);
            }
        }
        let started = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            tasks::start_task(&s, &task).map_err(to_mcp_err)?
        };
        ok_json(&started)
    }

    #[tool(description = "Block until a task is done | failed | cancelled, \
        or timeout_s (polls every 500 ms). Returns { status: satisfied | \
        timeout, task }; task.result holds the worker's paragraph. \
        Read-only. A per-host token may only wait on tasks it requested or \
        whose worker is on its host (E_FORBIDDEN).")]
    pub(super) async fn wait_for_task(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<WaitForTaskParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "wait_for_task",
            &format!("task_id={} timeout_s={:?}", p.task_id, p.timeout_s),
        );
        let _permit = self.long_poll_permit(&caller, "wait_for_task")?;
        let task = self.visible_task(&caller, p.task_id)?;
        let out = tasks::wait_for_task(&self.store, task.id, tasks::wait_timeout(p.timeout_s))
            .await
            .map_err(to_mcp_err)?;
        let row = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            tasks::mark_task_result(&s, out.row)
        };
        ok_json(&serde_json::json!({
            "status": if out.satisfied { "satisfied" } else { "timeout" },
            "task": row,
        }))
    }

    #[tool(description = "Tasks, newest first. Read-only. A per-host token \
        sees only tasks it requested from its host or whose worker is on its \
        host.")]
    pub(super) async fn list_tasks(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<ListTasksParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "list_tasks",
            &format!(
                "requester_session_id={:?} state={:?} limit={:?}",
                p.requester_session_id, p.state, p.limit
            ),
        );
        let rows = tasks::list_tasks_for(
            &self.store,
            p.requester_session_id,
            p.state.as_deref(),
            p.limit.unwrap_or(50),
            caller.host_alias.as_deref(),
        )
        .map_err(to_mcp_err)?;
        let rows: Vec<crate::store::TaskRow> = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            rows.into_iter()
                .map(|t| tasks::mark_task_result(&s, t))
                .collect()
        };
        ok_json_compact(&rows)
    }

    #[tool(description = "Cancel a queued or running task (E_TASK_TERMINAL \
        if it already finished). The worker session keeps running: kill or \
        re-prompt it separately. May return E_CONFIRM_REQUIRED when desktop \
        confirmation is on. A per-host token may only cancel tasks it can \
        see (E_FORBIDDEN).")]
    pub(super) async fn cancel_task(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<CancelTaskParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("cancel_task", &format!("task_id={}", p.task_id));
        let task = self.visible_task(&caller, p.task_id)?;
        self.confirm_gate(
            "cancel_task",
            p.confirm_nonce.as_deref(),
            &format!("task_id={} worker={:?}", task.id, task.worker_session_id),
            &caller,
        )?;
        let s = lock(&self.store).map_err(to_mcp_err)?;
        let row = tasks::cancel_task(&s, task.id, &format!("cancelled by {}", caller.label()))
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Replace a session's tags (short labels such as \
        `review`, `wip`; up to 16 of 1–32 chars from [A-Za-z0-9_.:-]), shown \
        and filterable in list_sessions. Returns the updated row.")]
    pub(super) async fn set_session_tags(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SetSessionTagsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "set_session_tags",
            &format!(
                "session_id={:?} host={:?} tmux={:?} tags={:?}",
                p.session_id, p.host_alias, p.tmux_name, p.tags
            ),
        );
        let row = self.resolve_target_row(
            &caller,
            p.session_id,
            p.host_alias.as_deref(),
            p.tmux_name.as_deref(),
            "the session to tag",
        )?;
        let tags = normalize_tags(p.tags)?;
        let s = lock(&self.store).map_err(to_mcp_err)?;
        let updated = s
            .set_session_tags(row.id, &tags)
            .map_err(|e| to_mcp_err(IpcError::from(e)))?
            .ok_or_else(|| mcp_err("E_NOTFOUND", format!("session {} vanished", row.id), None))?;
        ok_json(&updated)
    }

    #[tool(description = "Work links: {session_id} → its live links; \
        {key} → ended (past) links; neither → recently ended. action \
        context|resume_plan {key}; purge_impact; tickets (cached); lookup \
        {key|url}; trackers; scopes; orgs; org_suggestions; today {since}; card {key}; \
        tidy; reopened.")]
    pub(super) async fn work(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<crate::service::work::WorkArgs>,
    ) -> Result<CallToolResult, McpError> {
        use crate::service::work::{self as w, WorkAction};
        audit(
            "work",
            &format!(
                "action={:?} session_id={:?} key={:?} link_id={:?} host_alias={:?}",
                args.action, args.session_id, args.key, args.link_id, args.host_alias
            ),
        );
        // The one scope every action below runs under (work graph M5):
        // `All` for the master and clients, the host's org boundary for a
        // per-host token. The service functions filter with it.
        let scope = self.org_scope(&caller)?;
        match args.parsed_action().map_err(to_mcp_err)? {
            WorkAction::Links => {
                if let Some(id) = args.session_id {
                    self.resolve_target_row(&caller, Some(id), None, None, "the session")?;
                }
                ok_json_compact(&w::work(&args, &self.store, &scope).map_err(to_mcp_err)?)
            }
            WorkAction::Context => ok_json(
                &w::work_context(&args, &self.store, &self.ssh, &scope)
                    .await
                    .map_err(to_mcp_err)?,
            ),
            WorkAction::ResumePlan => ok_json_compact(
                &w::work_resume_plan(&args, &self.store, &self.ssh, &scope)
                    .await
                    .map_err(to_mcp_err)?,
            ),
            WorkAction::PurgeImpact => {
                ok_json(&w::work_purge_impact(&args, &self.store, &scope).map_err(to_mcp_err)?)
            }
            WorkAction::Tickets => {
                let rows = crate::service::trackers::tickets::tickets(
                    &self.store,
                    args.tracker_id,
                    args.view.as_deref(),
                    args.query.as_deref(),
                    args.limit,
                    &scope,
                )
                .map_err(to_mcp_err)?;
                ok_json_compact(&rows)
            }
            WorkAction::Lookup => {
                let reference = w::lookup_reference(&args).map_err(to_mcp_err)?;
                let t = crate::service::trackers::tickets::lookup(
                    &self.store,
                    reference,
                    &scope,
                    &crate::service::trackers::default_net(),
                )
                .await
                .map_err(to_mcp_err)?;
                ok_json(&t)
            }
            WorkAction::Trackers => ok_json_compact(
                &crate::service::trackers::tickets::trackers(&self.store, &scope)
                    .map_err(to_mcp_err)?,
            ),
            WorkAction::Scopes => ok_json_compact(
                &crate::service::orgs::scopes(&self.store, &scope).map_err(to_mcp_err)?,
            ),
            WorkAction::OrgSuggestions => ok_json_compact(
                &crate::service::orgs::org_suggestions(&self.store, &scope).map_err(to_mcp_err)?,
            ),
            WorkAction::Orgs => ok_json_compact(
                &crate::service::orgs::org_details(&self.store, &scope).map_err(to_mcp_err)?,
            ),
            WorkAction::Card => ok_json(
                &w::card::card(&self.store, args.key.as_deref().unwrap_or_default(), &scope)
                    .map_err(to_mcp_err)?,
            ),
            WorkAction::Today => ok_json_compact(
                &w::today::today(&self.store, args.since, &scope).map_err(to_mcp_err)?,
            ),
            WorkAction::Tidy => ok_json_compact(
                &crate::service::work::tidy::work_tidy(
                    &self.store,
                    &scope,
                    crate::service::catalog::now_secs(),
                )
                .map_err(to_mcp_err)?,
            ),
            WorkAction::Reopened => ok_json_compact(
                &crate::service::work::tidy::reopened(&self.store, &scope).map_err(to_mcp_err)?,
            ),
        }
    }

    #[tool(description = "Decide a session's work: action link (becomes its \
        primary; key or item_id), reject (sticky 'not this'; or a \
        suggestion's link_id), confirm (link_id), unlink (link_id). Returns \
        the updated row. trust_project {project_id, on}. resume {key, mode}: \
        new session on past work. start {key|url|item_id}: new session on a \
        ticket (project_ids: one per repo). handover {session_id}: ask it to \
        write its hand-off. archive|unarchive (UI only), snooze {days}|never \
        (tidy-up); dismiss {item_id} (reopened); tidy_apply {items}: kills (safe \
        kill when dirty).")]
    pub(super) async fn work_link(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<crate::service::work::WorkLinkArgs>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "work_link",
            &format!(
                "session_id={:?} action={} key={:?} item_id={:?} link_id={:?} source={:?} mode={:?} host_alias={:?}",
                args.session_id,
                args.action,
                args.key,
                args.item_id,
                args.link_id,
                args.source,
                args.mode,
                args.host_alias
            ),
        );
        if !crate::service::work::WORK_LINK_ACTIONS.contains(&args.action.as_str()) {
            return Err(mcp_err(
                "E_INVALID",
                format!(
                    "unknown work_link action {:?}; one of {}",
                    args.action,
                    crate::service::work::WORK_LINK_ACTIONS.join(", ")
                ),
                None,
            ));
        }
        let scope = self.org_scope(&caller)?;
        // A multi-repo start's projects are checked before anything else,
        // the operator's confirmation included: a request that can only be
        // refused is never put to a person.
        let multi = match (args.action.as_str(), args.project_ids.as_deref()) {
            ("start", Some(ids)) => Some(
                crate::service::trackers::tickets::multi_start_ids(args.project_id, ids)
                    .map_err(to_mcp_err)?,
            ),
            _ => None,
        };
        if caller.is_operator() && matches!(args.action.as_str(), "resume" | "start") {
            // Only ever gates the operator (D12): a session is about to exist.
            // (`work_link` is `confirm: true` for M7's tidy kills; a person's
            // start or resume is never gated.)
            let repos = self.repo_labels(args.project_ids.as_deref().unwrap_or_default())?;
            self.confirm_gate(
                "work_link",
                args.confirm_nonce.as_deref(),
                &work_link_start_summary(&args, &repos),
                &caller,
            )?;
        }
        if args.action == "handover" {
            // Work graph M9.3: ask a live session to write its hand-off (on
            // demand only, D9). The host and org fences are inside.
            let sid = args
                .session_id
                .ok_or_else(|| mcp_err("E_INVALID", "handover needs session_id", None))?;
            let row =
                crate::service::work::agent_handover::request(&self.store, &self.ssh, sid, &scope)
                    .await
                    .map_err(to_mcp_err)?;
            return ok_json(&row);
        }
        if args.action == "resume" {
            // The host fence (a per-host token resumes only onto its own
            // host) and the org fence are inside `resume_work`'s scope.
            let ra = crate::service::work::resume_args(&args).map_err(to_mcp_err)?;
            let row = crate::service::work::resume::resume_work(
                &self.store,
                &self.ssh,
                &self.reg,
                &ra,
                &scope,
            )
            .await
            .map_err(to_mcp_err)?;
            return ok_json(&row);
        }
        if args.action == "trust_project" {
            // Trust is fleet configuration, not one host's to change.
            if caller.host_alias.is_some() {
                return Err(mcp_err(
                    "E_FORBIDDEN",
                    "trust_project is not available to a per-host token",
                    None,
                ));
            }
            return ok_json(
                &crate::service::work::trust_project(&args, &self.store).map_err(to_mcp_err)?,
            );
        }
        if args.action == "dismiss" {
            // Reopened work is fleet-wide, not one host's to dismiss.
            if caller.host_alias.is_some() {
                return Err(mcp_err(
                    "E_FORBIDDEN",
                    "dismiss is not available to a per-host token",
                    None,
                ));
            }
            return ok_json(
                &crate::service::work::dismiss_reopened(&args, &self.store).map_err(to_mcp_err)?,
            );
        }
        if args.action == "tidy_apply" {
            let items = args.items.clone().unwrap_or_default();
            let summary = format!(
                "tidy_apply {}",
                items
                    .iter()
                    .map(|i| format!("{}:{}", i.session_id, i.action))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            // Kills: gated like kill_session when mcp.confirm_destructive is on.
            if items
                .iter()
                .any(|i| matches!(i.action.as_str(), "kill" | "safe_kill"))
            {
                self.confirm_gate(
                    "work_link",
                    args.confirm_nonce.as_deref(),
                    &summary,
                    &caller,
                )?;
            }
            let exec = crate::service::gc::RealGcExec {
                store: std::sync::Arc::clone(&self.store),
                ssh: std::sync::Arc::clone(&self.ssh),
            };
            let report = crate::service::work::tidy::tidy_apply(
                &self.store,
                &exec,
                &items,
                &scope,
                crate::service::catalog::now_secs(),
            )
            .await
            .map_err(to_mcp_err)?;
            return ok_json(&report);
        }
        if args.action == "start" {
            if let Some(ids) = multi.as_deref() {
                // Work graph M9.6: one sibling per repository, same branch.
                let out = crate::service::trackers::tickets::start_work_many(
                    &self.store,
                    &self.ssh,
                    &self.reg,
                    &crate::service::work::start_args(&args),
                    ids,
                    &scope,
                    &crate::service::trackers::default_net(),
                )
                .await
                .map_err(to_mcp_err)?;
                return ok_json(&out);
            }
            // The host fence (a per-host token starts only its own host's
            // tickets, on its own host) is inside `start_work`'s scope.
            let row = crate::service::trackers::tickets::start_work(
                &self.store,
                &self.ssh,
                &self.reg,
                &crate::service::work::start_args(&args),
                &scope,
                &crate::service::trackers::default_net(),
            )
            .await
            .map_err(to_mcp_err)?;
            return ok_json(&row);
        }
        let sid = args.session_id.ok_or_else(|| {
            mcp_err(
                "E_INVALID",
                format!("{} needs session_id", args.action),
                None,
            )
        })?;
        self.resolve_target_row(&caller, Some(sid), None, None, "the session")?;
        let row =
            crate::service::work::work_link(&args, &self.store, &scope).map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Trackers, orgs and retention; see action. \
        Never returns a secret.")]
    pub(super) async fn work_admin(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<crate::service::trackers::admin::WorkAdminArgs>,
    ) -> Result<CallToolResult, McpError> {
        use crate::service::trackers::admin::{self as a, AdminAction};
        // Master-only enforcement already happened centrally
        // (`enforce_admin`, `work_admin` is `Access::Master`). The audit line
        // is built by hand: the secret never reaches it, not even as a length.
        let summary = args.audit_summary();
        audit("work_admin", &summary);
        match AdminAction::parse(&args.action).map_err(to_mcp_err)? {
            AdminAction::Status => ok_json(
                &crate::service::work::retention::status(
                    &self.store,
                    crate::service::catalog::now_secs(),
                )
                .map_err(to_mcp_err)?,
            ),
            AdminAction::SweepNow => ok_json(&crate::service::work::retention::sweep(
                &self.store,
                crate::service::catalog::now_secs(),
            )),
            AdminAction::Test => {
                let id = args
                    .tracker_id
                    .ok_or_else(|| mcp_err("E_INVALID", "test needs tracker_id", None))?;
                let report =
                    a::test_tracker(id, &self.store, &crate::service::trackers::default_net())
                        .await
                        .map_err(to_mcp_err)?;
                ok_json(&report)
            }
            action if action.is_removal() => {
                self.confirm_gate(
                    "work_admin",
                    args.confirm_nonce.as_deref(),
                    &summary,
                    &caller,
                )?;
                ok_json(&a::admin_sync(&args, &self.store).map_err(to_mcp_err)?)
            }
            _ => ok_json(&a::admin_sync(&args, &self.store).map_err(to_mcp_err)?),
        }
    }

    /// `id:owner/repo` for each project id, for a confirm summary; an
    /// unknown id is shown bare (the start refuses it later).
    fn repo_labels(&self, ids: &[i64]) -> Result<Vec<String>, McpError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let s = lock(&self.store).map_err(to_mcp_err)?;
        Ok(ids
            .iter()
            .map(|id| match s.get_project(*id) {
                Ok(Some(p)) => format!("{id}:{}/{}", p.owner, p.repo),
                _ => id.to_string(),
            })
            .collect())
    }

    /// The caller's org scope (work graph M5), read under a short lock.
    pub(super) fn org_scope(
        &self,
        caller: &Caller,
    ) -> Result<crate::service::orgs::OrgScope, McpError> {
        let s = lock(&self.store).map_err(to_mcp_err)?;
        caller.org_scope(&s).map_err(to_mcp_err)
    }
}
