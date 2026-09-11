//! MCP tools: bounded waits, transcripts, run_prompt and tasks.

use super::*;

#[tool_router(router = orchestration_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Block until a session reaches a state, or time out. \
        until=\"idle\": claude_status is idle | completed | stopped | failed \
        (true even for a session that never started a turn). \
        until=\"turn_gt\": turn_seq > `turn` — pass the turn_seq_before that \
        send_prompt returned to wait for the reply to YOUR prompt. Polls the \
        store every 500 ms for up to timeout_s (default 120, max 600). \
        Returns JSON { status: satisfied | timeout, claude_status, turn_seq, \
        last_stop_at, stuck_kind }. Read-only. A per-host token may only \
        wait on sessions on its own host.")]
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

    #[tool(description = "Read a session's Claude Code transcript (the JSONL \
        Claude writes, not the pane) and return the last assistant turn as \
        plain text — text blocks verbatim, one summary line per tool call, \
        no thinking. since_turn returns every turn after that turn_seq \
        (use send_prompt's turn_seq_before). max_chars caps the text \
        (default 8000, max 64000; the END is kept). Errors: E_INVALID_STATE \
        (no claude_session_id yet), E_NO_TRANSCRIPT (nothing written yet). \
        Read-only; prefer it over capture_session for the reply text.")]
    pub(super) async fn session_transcript(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SessionTranscriptParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "session_transcript",
            &format!(
                "session_id={} since_turn={:?} max_chars={:?}",
                p.session_id, p.since_turn, p.max_chars
            ),
        );
        let row =
            self.resolve_target_row(&caller, Some(p.session_id), None, None, "the session")?;
        let text = self.transcript_for(&row, p.since_turn, p.max_chars).await?;
        if text.trim().is_empty() {
            return Ok(CallToolResult::success(vec![text_content(
                "(no assistant text in the requested turns)",
            )]));
        }
        Ok(CallToolResult::success(vec![text_content(text)]))
    }

    #[tool(description = "send_prompt + wait_for_session(turn_gt) + \
        session_transcript in one call: deliver the prompt, wait up to \
        timeout_s (default 120, max 600) for the turn to complete, and return \
        JSON { turn_seq, status: satisfied | timeout, transcript } where \
        transcript is the reply as plain text (null with transcript_error \
        when it cannot be read). Marked as untrusted unless raw=true (master \
        token only). Address the session with session_id.")]
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
        self.deliver_prompt(&row, prompt, true).await?;
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

    #[tool(description = "Dispatch a unit of work to a worker session and \
        track it as a task. Pass worker_session_id (an existing session) OR \
        new_worker { host_alias, project_id, name? } (spawns one via \
        new_session). The prompt is delivered with an appended instruction to \
        print FLEET_TASK_DONE_<nonce> on its own line followed by a \
        one-paragraph result; fleet detects the marker on the worker's next \
        Stop and flips the task to done with that paragraph as `result` \
        (also delivered to requester_session_id's inbox as kind=task_result). \
        Returns the task row (id, state=running, worker_session_id, …); \
        follow with wait_for_task. A per-host token must name a requester on \
        its own host. Marked as untrusted unless raw=true (master only).")]
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
            let s = self
                .store
                .lock()
                .map_err(|_| to_mcp_err(IpcError::lock()))?;
            let task = tasks::create_task(&s, p.requester_session_id, Some(worker.id), &p.prompt)
                .map_err(to_mcp_err)?;
            if p.requester_session_id.is_some() {
                let _ = s.set_parent_session_id(worker.id, p.requester_session_id);
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
        match self.deliver_prompt(&worker, body, true).await {
            Ok(_) => {}
            Err(e) => {
                if let Ok(s) = self.store.lock() {
                    let _ = tasks::fail_task(&s, task.id, &e.message);
                }
                return Err(e);
            }
        }
        let started = {
            let s = self
                .store
                .lock()
                .map_err(|_| to_mcp_err(IpcError::lock()))?;
            tasks::start_task(&s, &task).map_err(to_mcp_err)?
        };
        ok_json(&started)
    }

    #[tool(description = "Block until a task reaches done | failed | cancelled \
        or timeout_s elapses (default 120, max 600; polls every 500 ms). \
        Returns JSON { status: satisfied | timeout, task } — task.result \
        holds the worker's paragraph when done. Read-only. A per-host token \
        may only wait on tasks it requested or whose worker is on its host \
        (E_FORBIDDEN).")]
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
            let s = self
                .store
                .lock()
                .map_err(|_| to_mcp_err(IpcError::lock()))?;
            tasks::mark_task_result(&s, out.row)
        };
        ok_json(&serde_json::json!({
            "status": if out.satisfied { "satisfied" } else { "timeout" },
            "task": row,
        }))
    }

    #[tool(description = "List tasks, newest-first (default 50 rows). Filters: \
        requester_session_id, state (queued | running | done | failed | \
        cancelled). Read-only. A per-host token sees only tasks it requested \
        from its host or whose worker is on its host.")]
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
            let s = self
                .store
                .lock()
                .map_err(|_| to_mcp_err(IpcError::lock()))?;
            rows.into_iter()
                .map(|t| tasks::mark_task_result(&s, t))
                .collect()
        };
        ok_json_compact(&rows)
    }

    #[tool(description = "Cancel a queued or running task: marks it cancelled \
        (E_TASK_TERMINAL if it already finished). The worker session keeps \
        running — kill or re-prompt it separately if needed. May return \
        E_CONFIRM_REQUIRED when desktop confirmation is on. A per-host token \
        may only cancel tasks it can see (E_FORBIDDEN).")]
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
        let s = self
            .store
            .lock()
            .map_err(|_| to_mcp_err(IpcError::lock()))?;
        let row = tasks::cancel_task(&s, task.id, &format!("cancelled by {}", caller.label()))
            .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Replace a session's tags (short labels such as \
        `review`, `infra`, `wip`; up to 16 of 1–32 chars from [A-Za-z0-9_.:-]; \
        an empty list clears). Tags show in list_sessions rows and \
        list_sessions { tag } filters on them. Returns the updated row. \
        Address the session with session_id OR host_alias + tmux_name.")]
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
        let s = self
            .store
            .lock()
            .map_err(|_| to_mcp_err(IpcError::lock()))?;
        let updated = s
            .set_session_tags(row.id, &tags)
            .map_err(|e| to_mcp_err(IpcError::from(e)))?
            .ok_or_else(|| mcp_err("E_NOTFOUND", format!("session {} vanished", row.id), None))?;
        ok_json(&updated)
    }
}
