//! MCP tools: bounded waits, transcripts, run_prompt and tasks.

use super::*;
use crate::ipc_error::lock;
use crate::service::tasks::{AccessRecheck as _, PaneProbe as _};

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
        // `watch`: a bounded wait reports the row's status and turn count
        // and writes nothing.
        let row = self.resolve_target_row(
            &caller,
            Some(p.session_id),
            None,
            None,
            Reach::Read,
            "the session",
        )?;
        let _permit = self.long_poll_permit(&caller, "wait_for_session")?;
        let cond = tasks::WaitCond::parse(&p.until, p.turn).map_err(to_mcp_err)?;
        // T11: this wait can sit for ten minutes, so the gate above is not
        // the last word — `recheck` runs it again on every wake and once
        // more below, before the payload.
        let recheck = SessionRecheck {
            caller: &caller,
            session_id: row.id,
            reach: Reach::Read,
            what: "the session",
        };
        // A stale-demoted row's `idle` is a guess: the wait asks its pane.
        let out = tasks::wait_for_session_probed(
            &self.store,
            row.id,
            cond,
            tasks::wait_timeout(p.timeout_s),
            tasks::POLL_INTERVAL,
            &tasks::LivePaneProbe {
                store: &self.store,
                ssh: &self.ssh,
            },
            tasks::STALE_PANE_PROBE_EVERY,
            &recheck,
        )
        .await
        .map_err(to_mcp_err)?;
        // The pane probe above is the one await the wait takes OUTSIDE its
        // own lock window, so the last wake's re-check is not quite the
        // last word either. One more, immediately before `ok_json`.
        self.recheck_now(&recheck)?;
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
        // `watch`: the transcript is the clearest case of what a watch
        // grant is FOR (spec §4.3, *What counts as content*).
        let row = self.resolve_target_row(
            &caller,
            Some(p.session_id),
            None,
            None,
            Reach::Read,
            "the session",
        )?;

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
        // `watch`: the transcript, rendered as turns.
        let row = self.resolve_target_row(
            &caller,
            Some(p.session_id),
            None,
            None,
            Reach::Read,
            "the session",
        )?;
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

    #[tool(description = "One tool call's input and result (omitted by \
        session_conversation): { id, name, input, edit {file_path, old, new} \
        | null, command | null, result | null, is_error }; texts capped at \
        8000 chars. Read-only. Errors: as session_conversation, E_NOTFOUND.")]
    pub(super) async fn session_tool_detail(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SessionToolDetailParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "session_tool_detail",
            &format!(
                "session_id={} tool_use_id={} claude_session_id={:?}",
                p.session_id, p.tool_use_id, p.claude_session_id
            ),
        );
        // `watch`: one tool call out of the same transcript.
        let row = self.resolve_target_row(
            &caller,
            Some(p.session_id),
            None,
            None,
            Reach::Read,
            "the session",
        )?;
        let detail = transcript::fetch_tool_detail(
            &self.store,
            &self.ssh,
            &row,
            p.claude_session_id.as_deref(),
            &p.tool_use_id,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json_compact(&detail)
    }

    #[tool(description = "A short Claude-written summary of what a session \
        did since a time (unix seconds), for whoever may read it: { text | \
        null, check: off | shadow | passed | failed | unchecked, since, turns, \
        model, host_alias, at }. Runs one claude -p on the session's host \
        under its account (booked as watch_summary); only with the session \
        org's consent; text is null when nothing happened since or when the \
        Jev check hid it. Errors: E_FORBIDDEN (no consent), E_CLAUDE_CLI, \
        E_TIMEOUT, as session_conversation.")]
    pub(super) async fn session_summary_since(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SessionSummarySinceParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "session_summary_since",
            &format!("session_id={} since={}", p.session_id, p.since),
        );
        // `watch`: a retelling of the same transcript (redesign 11.11).
        let row = self.resolve_target_row(
            &caller,
            Some(p.session_id),
            None,
            None,
            Reach::Read,
            "the session",
        )?;
        let decide = crate::service::decide::DecideCtx::jev(std::sync::Arc::clone(&self.store));
        let summary = crate::service::watch_summary::summarize_since(
            &self.store,
            &self.ssh,
            &decide,
            &row,
            p.since,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json_compact(&summary)
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
            // `drive`: `run_prompt` is `send_prompt` plus a wait and a
            // transcript read, so it cannot be narrower than `send_prompt`.
            Reach::Drive,
            "the session to prompt",
        )?;
        // As `send_prompt`: the operator's text waits for a person (F21).
        self.confirm_gate(
            "run_prompt",
            p.confirm_nonce.as_deref(),
            &prompt_summary(&row, &p.prompt, None, true, false, p.raw),
            &caller,
        )?;
        // A stale-demoted row reads `idle` only because nothing moved; a
        // long tool call looks exactly like that. Ask the pane (S5 + F2).
        // The demotion's memory (`stale_demoted_at`), not the attention
        // stamp: an attach or the TTL ends the reason, not the guess.
        let live = if crate::store::needs_pane_confirmation(&row) {
            tasks::LivePaneProbe {
                store: &self.store,
                ssh: &self.ssh,
            }
            .pane_status(row.id)
            .await
        } else {
            None
        };
        run_prompt_ready(&row, live.as_deref())?;
        let _permit = self.long_poll_permit(&caller, "run_prompt")?;
        // T11, and the honest limit of it. `run_prompt` is three acts:
        // deliver, wait, read the transcript. The LAST TWO are recalled by
        // a revoke; the first is not. The pane probe above took an await
        // between the gate and here, so this re-check is what stands
        // between a share revoked during that probe and a prompt typed
        // into the owner's session — but once the keys are in the pane
        // there is no un-sending them, and nothing below pretends there
        // is (`docs/hub.md` → *A revoked share, precisely*, DoD 6).
        let recheck = SessionRecheck {
            caller: &caller,
            session_id: row.id,
            reach: Reach::Drive,
            what: "the session to prompt",
        };
        self.recheck_now(&recheck)?;
        let prompt = apply_marker(p.prompt, &marker_origin(&caller), &caller, p.raw)?;
        let before = row.turn_seq;
        self.deliver_prompt(&row, prompt, true, false).await?;
        let out = tasks::wait_for_session(
            &self.store,
            row.id,
            tasks::WaitCond::TurnGt(before),
            tasks::wait_timeout(p.timeout_s),
            &recheck,
        )
        .await
        .map_err(to_mcp_err)?;
        // Before the transcript is even READ, let alone returned: the
        // reply to a prompt this caller may no longer drive is content it
        // may no longer have.
        self.recheck_now(&recheck)?;
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
        // Its OWNER is kept (multi-user M1, T5): a worker spawned for a task
        // does the requester's work in the requester's name, so the row
        // belongs to whoever owns the requesting session — not to the per-host
        // token that asked, which is a machine and owns nothing.
        let requester_owner = match p.requester_session_id {
            Some(req) => {
                self.resolve_target_row(
                    &caller,
                    Some(req),
                    None,
                    None,
                    // `drive` on the REQUESTER, not `watch`. Naming a
                    // session as requester WRITES to it three ways: a
                    // `tasks` row bound to it, a `task_done` row on its
                    // timeline, and — on the worker's next Stop — an INBOX
                    // MESSAGE whose body came out of the worker
                    // (`service::tasks::complete_task`). With `new_worker`
                    // it also decides whose the new session is: the worker
                    // inherits the requester's `owner_person_id` below, so a
                    // watcher could otherwise start a session owned by the
                    // grantor, on a host the watcher picked, running the
                    // watcher's prompt and spending the grantor's AI account.
                    //
                    // T7's mechanical rule — anything that writes to a pane,
                    // a row, a task or a tmux server is `Drive` — and what
                    // `share.ts` already says (`dispatch_task: 'drive'`).
                    // Writing a message body into another person's inbox is
                    // exactly what `require_message_recipient`'s `Drive`
                    // exists to refuse; this is the same write by another
                    // route. The agent inside the requesting session keeps
                    // reaching it: a per-host token `may_drive` the one row
                    // its pane proves (§4.4 clause 2).
                    Reach::Drive,
                    "requester_session_id",
                )?
                .owner_person_id
            }
            None => None,
        };
        // Resolve or spawn the worker.
        let (worker, spawned) = match (p.worker_session_id, p.new_worker) {
            (Some(id), _) => {
                let row = self.resolve_target_row(
                    &caller,
                    Some(id),
                    None,
                    None,
                    // `drive`: a dispatch makes the worker's machine do
                    // work — a prompt into its pane, by way of a task row.
                    Reach::Drive,
                    "the worker session",
                )?;
                // The task's prompt is typed into the worker's pane, so the
                // operator's needs a person as its `send_prompt` does (F21).
                self.confirm_gate(
                    "dispatch_task",
                    p.confirm_nonce.as_deref(),
                    &format!(
                        "worker requester_session_id={:?} {}",
                        p.requester_session_id,
                        prompt_summary(&row, &p.prompt, None, true, false, p.raw)
                    ),
                    &caller,
                )?;
                (row, false)
            }
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
                        model: None,
                        effort: None,
                        profile: None,
                        agent: None,
                        // Who asked for the worker (migration 124): the
                        // connection, as for `new_session`.
                        origin: lock(self.reader())
                            .ok()
                            .map(|s| super::fleet::origin_for(&caller, &s)),
                        // The requester's owner (above), else this
                        // connection's own person. A dispatch with no
                        // requester is somebody asking fleet directly, so it
                        // is theirs; a per-host token resolves to neither and
                        // the worker lands `unclaimed`.
                        over_limit_ok: false,
                        owner_person_id: requester_owner.or_else(|| {
                            lock(self.reader())
                                .ok()
                                .and_then(|s| super::fleet::owner_for(&caller, &s))
                        }),
                        start_token: None,
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
            // The job shows as an agent subtask of the requester's work.
            tasks::mirror_dispatched(&s, &task, p.requester_session_id, worker.id);
            if let Some(cid) = worker.claude_session_id.as_deref() {
                let _ = s.set_task_worker_claude_id(task.id, cid);
            }
            task
        };
        if spawned {
            // A freshly launched REPL needs a moment before it accepts
            // typed input; wait (bounded) for it to settle. A dialog (the
            // trust prompt, whose default answer is "No, exit") is never
            // typed into: the task fails, naming it, instead.
            use crate::service::sessions::seed::{wait_for_repl, ReplWait};
            if let ReplWait::Dialog(why) = wait_for_repl(&self.store, &self.ssh, &worker).await {
                let e = crate::ipc_error::IpcError::new(
                    crate::ipc_error::codes::E_INVALID_STATE,
                    format!(
                        "worker {} is waiting on a dialog ({why}); answer it in the session, \
                         then dispatch the task again",
                        worker.tmux_name
                    ),
                );
                if let Ok(s) = self.store.lock() {
                    let _ = tasks::fail_task(&s, task.id, &e.message);
                }
                return Err(to_mcp_err(e));
            }
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
        // The visibility check comes FIRST, and the permit second. This is
        // the only long poll where the order mattered: a caller who may not
        // see the task was burning one of its
        // `guard::MAX_LONG_POLLS_PER_CALLER` slots on the way to being
        // refused (multi-user M1, T7). `watch`: the wait reads the task and
        // its result.
        let task = self.visible_task(&caller, p.task_id, Reach::Read)?;
        let _permit = self.long_poll_permit(&caller, "wait_for_task")?;
        // T11: re-checked on every wake and once more below. `task.result`
        // is the worker's own paragraph, so a share revoked on either of
        // the task's sessions must end the wait, not complete it.
        let recheck = TaskRecheck {
            caller: &caller,
            task_id: task.id,
            reach: Reach::Read,
        };
        let out = tasks::wait_for_task(
            &self.store,
            task.id,
            tasks::wait_timeout(p.timeout_s),
            &recheck,
        )
        .await
        .map_err(to_mcp_err)?;
        let row = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            // Same lock window as the row that is about to be returned.
            recheck.check(&s).map_err(to_mcp_err)?;
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
        // A `TaskRow` carries the `prompt` one session sent another and
        // the `result` that came back: session content, so the page is cut
        // against the caller's own view scope and not against its host
        // alias alone (multi-user M1, T7). The host filter stays where it
        // was — in SQL — and composes with it.
        let view = {
            let s = lock(self.reader()).map_err(to_mcp_err)?;
            caller.view_scope(&s).map_err(to_mcp_err)?
        };
        let rows = tasks::list_tasks_for(
            &self.store,
            p.requester_session_id,
            p.state.as_deref(),
            p.limit.unwrap_or(50),
            caller.host_alias.as_deref(),
            &view,
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
        // `drive` on the worker: cancelling stops work on somebody's
        // machine, which is what a `drive` grant is for and what a `watch`
        // grant is not.
        let task = self.visible_task(&caller, p.task_id, Reach::Drive)?;
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
            // `own`: the spec's invariant names `set_session_tags` beside
            // `rename_session` — a label on somebody's row is content.
            Reach::Own,
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

    #[tool(description = "Answer a session's related_session proposal \
        {session_id, run_id, linked}: linked=true (Link) keeps the other \
        session listed as linked, false (Not related) withdraws it. Nothing \
        is stopped or merged. Returns the session's row.")]
    pub(super) async fn decide_related_session(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<DecideRelatedSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "decide_related_session",
            &format!(
                "session_id={} run_id={} linked={}",
                p.session_id, p.run_id, p.linked
            ),
        );
        // `own`: the proposal is about this person's own sessions (N1 never
        // proposes another person's), so answering it is the owner's call.
        let row = self.resolve_target_row(
            &caller,
            Some(p.session_id),
            None,
            None,
            Reach::Own,
            "the session the proposal is about",
        )?;
        let args = crate::service::decide::related_session::DecideRelatedSessionArgs {
            session_id: row.id,
            run_id: p.run_id,
            linked: p.linked,
        };
        let updated =
            crate::service::decide::related_session::decide_related_session(args, &self.store)
                .map_err(to_mcp_err)?;
        ok_json(&updated)
    }

    #[tool(description = "Work links: {session_id} → its live links; \
        {key} → ended (past) links; neither → recently ended. action \
        context|resume_plan {key}; purge_impact; tickets (cached); lookup \
        {key|url}; trackers; scopes; orgs; org_suggestions; today {since}; card {key}; \
        describe {key} (the tracker's whole description, cached); \
        tidy; reopened. Work view: tree {filters, cursor} (archived: false hides \
        archived tasks); task {task_id}; \
        session_tasks; review; rules; rule_preview {rule}; views; org_impact. \
        buckets {kind?}: sprints, releases; bucket {bucket_id}. missions; \
        mission {mission_id, before_event?}. attachment {attachment_id}: a \
        file's bytes.")]
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
        //
        // Multi-user M1 (T7): the org half alone cannot tell two people on
        // one hub apart, so the arms that answer a PAGE of session-derived
        // rows — `today`, `tree`, `task`, `review`, `context`, `resume_plan`,
        // `tidy` — take the WHOLE scope (`view_scope`), which carries this
        // one as `.org`. Everything else keeps the org scope, either because
        // it names one row and is gated per row, or because it answers no
        // session at all (both tables in
        // `tests::every_session_addressed_tool_declares_its_reach`).
        let view_scope = self.view_scope(&caller)?;
        let scope = view_scope.org.clone();
        match args.parsed_action().map_err(to_mcp_err)? {
            WorkAction::Links => {
                if let Some(id) = args.session_id {
                    // `watch`: `work { links }` reads one session's links.
                    self.resolve_target_row(
                        &caller,
                        Some(id),
                        None,
                        None,
                        Reach::Read,
                        "the session",
                    )?;
                }
                // And the TWO OTHER forms of the same arm — `{ key }` and no
                // argument at all — answer a PAGE of links, so they take the
                // whole `view_scope` like every other page below (T8d). The
                // per-row gate above cannot cover them: neither names a
                // session.
                ok_json_compact(&w::work(&args, &self.store, &view_scope).map_err(to_mcp_err)?)
            }
            WorkAction::Context => ok_json(
                &w::work_context(&args, &self.store, &self.ssh, &view_scope)
                    .await
                    .map_err(to_mcp_err)?,
            ),
            WorkAction::ResumePlan => ok_json_compact(
                &w::work_resume_plan(&args, &self.store, &self.ssh, &view_scope)
                    .await
                    .map_err(to_mcp_err)?,
            ),
            WorkAction::PurgeImpact => {
                ok_json(&w::work_purge_impact(&args, &self.store, &scope).map_err(to_mcp_err)?)
            }
            WorkAction::Tickets => {
                let rows = crate::service::trackers::tickets::tickets_and_tasks(
                    &self.store,
                    args.tracker_id,
                    args.view.as_deref(),
                    args.query.as_deref(),
                    args.limit,
                    args.include_local == Some(true),
                    &view_scope,
                )
                .map_err(to_mcp_err)?;
                ok_json_compact(&rows)
            }
            WorkAction::Lookup => {
                let reference = w::lookup_reference(&args).map_err(to_mcp_err)?;
                let t = crate::service::trackers::tickets::lookup(
                    &self.store,
                    reference,
                    &view_scope,
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
                &crate::service::orgs::scopes(&self.store, &view_scope).map_err(to_mcp_err)?,
            ),
            WorkAction::OrgSuggestions => ok_json_compact(
                &crate::service::orgs::org_suggestions(&self.store, &view_scope)
                    .map_err(to_mcp_err)?,
            ),
            WorkAction::Orgs => ok_json_compact(
                &crate::service::orgs::org_details(
                    &self.store,
                    &view_scope,
                    crate::service::orgs::AdminView::for_caller(&caller, &self.store)
                        .map_err(to_mcp_err)?,
                )
                .map_err(to_mcp_err)?,
            ),
            WorkAction::Card => ok_json(
                &w::card::card(
                    &self.store,
                    args.key.as_deref().unwrap_or_default(),
                    &view_scope,
                )
                .map_err(to_mcp_err)?,
            ),
            WorkAction::Describe => {
                let key = args
                    .key
                    .as_deref()
                    .ok_or_else(|| mcp_err("E_INVALID", "describe needs key", None))?;
                ok_json(
                    &w::describe::describe(
                        &self.store,
                        &scope,
                        key,
                        &crate::service::trackers::default_net(),
                    )
                    .await
                    .map_err(to_mcp_err)?,
                )
            }
            WorkAction::Today => ok_json_compact(
                &w::today::today(&self.store, args.since, &view_scope).map_err(to_mcp_err)?,
            ),
            WorkAction::Tidy => ok_json_compact(
                &crate::service::work::tidy::work_tidy(
                    &self.store,
                    &view_scope,
                    crate::service::catalog::now_secs(),
                )
                .map_err(to_mcp_err)?,
            ),
            // Both carry session-DERIVED counts (and `reopened` the host of
            // one specific past session), so both take the whole scope: the
            // counts are recomputed over the links this caller may see (T8d).
            WorkAction::Reopened => ok_json_compact(
                &crate::service::work::tidy::reopened(&self.store, &view_scope)
                    .map_err(to_mcp_err)?,
            ),
            WorkAction::LocalItems => ok_json_compact(
                &crate::service::work::local::local_items(&self.store, &view_scope)
                    .map_err(to_mcp_err)?,
            ),
            // Work graph M14: the Work view. Its reads build from the read
            // pool (one pass over every item and link), never the writer.
            WorkAction::Tree => ok_json_compact(
                &w::view::tree(
                    self.reader(),
                    &view_scope,
                    &w::view::TreeArgs {
                        filters: args.filters.clone().unwrap_or_default(),
                        cursor: args.cursor.clone(),
                        limit: args.limit,
                        per_task: args.per_task,
                        sections: args.sections.clone().unwrap_or_default(),
                        with_review_total: args.with_review_total == Some(true),
                        with_missions: args.with_missions == Some(true),
                    },
                )
                .map_err(to_mcp_err)?,
            ),
            WorkAction::Task => {
                let task_id = args
                    .task_id
                    .as_deref()
                    .ok_or_else(|| mcp_err("E_INVALID", "task needs task_id", None))?;
                let mut detail =
                    w::view::task(self.reader(), &view_scope, task_id).map_err(to_mcp_err)?;
                // Which comments are the caller's own (to delete), from the
                // stored authorship: the answer may withhold the author.
                if !detail.comments.is_empty() {
                    let s = lock(&self.store).map_err(to_mcp_err)?;
                    let label = caller.label();
                    for c in &mut detail.comments {
                        c.mine = s
                            .get_comment(c.id)
                            .map_err(to_mcp_err)?
                            .is_some_and(|row| row.written_by(view_scope.person, &label));
                    }
                }
                // And which attachments, the same way.
                if !detail.attachments.is_empty() {
                    let s = lock(&self.store).map_err(to_mcp_err)?;
                    let label = caller.label();
                    for a in &mut detail.attachments {
                        a.mine = s
                            .get_attachment(a.id)
                            .map_err(to_mcp_err)?
                            .is_some_and(|row| row.written_by(view_scope.person, &label));
                    }
                }
                ok_json_compact(&detail)
            }
            WorkAction::SessionTasks => {
                let id = args
                    .session_id
                    .ok_or_else(|| mcp_err("E_INVALID", "session_tasks needs session_id", None))?;
                // `watch`: the Work view's read of one session's tasks.
                self.resolve_target_row(&caller, Some(id), None, None, Reach::Read, "the session")?;
                ok_json_compact(
                    &w::view::session_tasks(self.reader(), &view_scope, id).map_err(to_mcp_err)?,
                )
            }
            WorkAction::Review => ok_json_compact(
                &w::view::review(
                    self.reader(),
                    &view_scope,
                    args.cursor.as_deref(),
                    args.limit,
                )
                .map_err(to_mcp_err)?,
            ),
            WorkAction::Rules => {
                ok_json_compact(&w::structure::rules(self.reader(), &scope).map_err(to_mcp_err)?)
            }
            WorkAction::RulePreview => {
                let rule = args
                    .rule
                    .as_ref()
                    .ok_or_else(|| mcp_err("E_INVALID", "rule_preview needs rule", None))?;
                ok_json_compact(
                    &w::structure::rule_preview(self.reader(), &scope, rule).map_err(to_mcp_err)?,
                )
            }
            WorkAction::Views => {
                ok_json_compact(&w::structure::views(self.reader(), &scope).map_err(to_mcp_err)?)
            }
            // Sprints and releases (design 2026-09-28 §7): org-fenced by
            // the bucket's own org, members by each item's.
            WorkAction::Buckets => ok_json_compact(
                &w::buckets::buckets(&self.store, &view_scope, args.kind.as_deref())
                    .map_err(to_mcp_err)?,
            ),
            WorkAction::Bucket => {
                let id = args
                    .bucket_id
                    .ok_or_else(|| mcp_err("E_INVALID", "bucket needs bucket_id", None))?;
                ok_json_compact(
                    &w::buckets::bucket(&self.store, &view_scope, id).map_err(to_mcp_err)?,
                )
            }
            // Missions (orchestration O1): a mission is a PERSON's, so the
            // whole `view_scope` — its owner, its org's members, the org
            // boundary first (`missions::sees_mission`).
            WorkAction::Missions => ok_json_compact(
                &w::missions::missions(&self.store, &view_scope).map_err(to_mcp_err)?,
            ),
            WorkAction::Mission => {
                let id = args
                    .mission_id
                    .ok_or_else(|| mcp_err("E_INVALID", "mission needs mission_id", None))?;
                ok_json_compact(
                    &w::missions::mission(&self.store, &view_scope, id, args.before_event)
                        .map_err(to_mcp_err)?,
                )
            }
            // A task attachment's bytes: its item's org fence (outside it,
            // unknown), the author withheld as `task` withholds it.
            WorkAction::Attachment => {
                let mut data = w::attachments::attachment(&args, &self.store, &view_scope)
                    .map_err(to_mcp_err)?;
                {
                    let s = lock(&self.store).map_err(to_mcp_err)?;
                    data.attachment.mine = s
                        .get_attachment(data.attachment.id)
                        .map_err(to_mcp_err)?
                        .is_some_and(|row| row.written_by(view_scope.person, &caller.label()));
                }
                ok_json_compact(&data)
            }
            WorkAction::OrgImpact => {
                let task_id = args
                    .task_id
                    .as_deref()
                    .ok_or_else(|| mcp_err("E_INVALID", "org_impact needs task_id", None))?;
                // A page of `ImpactLink { session_id, name, host }`, so the
                // whole `view_scope` (T8d): the `is_all()` check inside is the
                // authority to MOVE an org, never a privacy fence.
                ok_json_compact(
                    &w::structure::org_impact(self.reader(), &view_scope, task_id, args.org_id)
                        .map_err(to_mcp_err)?,
                )
            }
        }
    }

    #[tool(description = "Decide a session's work: action link (becomes its \
        primary; key or item_id), reject (sticky 'not this'; or a \
        suggestion's link_id), confirm (link_id), unlink (link_id). Returns \
        the updated row. switch {link_id, key|item_id}: end that link, take \
        the primary. trust_project {project_id, on}. resume {key, mode}: \
        new session on past work. start {key|url|item_id}: new session on a \
        ticket (project_ids: one per repo; parallel: beside a live one); \
        preview_start: where it would land, nothing made. abandon_start \
        {session_id}: undo an unused start. run {item_id, role?}: an \
        attempt at the item in its own session and worktree, tracked as a task. \
        handover {session_id}: ask it to \
        write its hand-off. summarize {key, link_id}: \
        a Claude-written summary of past work. archive|unarchive (UI only), snooze {days}|never \
        (tidy-up); dismiss {item_id} (reopened); tidy_apply {items}: kills (safe \
        kill when dirty). set_status {item_id, status}: a person's status for \
        work with no ticket. edit {item_id, title?, notes?, assignees?, \
        due_at?}: a person edits work with no ticket. attach {item_id, name, \
        mime, data_base64, comment_id?}: a file on a task; attachment_delete \
        {attachment_id}. create {title, parent?, \
        notes?, assignees?, due_at?}: a task or subtask. propose {parent, title, why?}: a subtask a person accepts \
        or rejects {item_id, no session_id}. bucket_add|bucket_remove \
        {bucket_id, item_id}: sprint/release. mission_save {mission, \
        mission_id?, item_id?: root}; mission_state {mission_id, status} (paused \
        reopens a finished one); \
        mission_repo {project_id, role?, on?}; mission_item {item_id, on?}; \
        mission_delete; mission_import {plan}. dep {item_id, depends_on, on?}; hold {item_id, on?}; \
        propose_tree {parent, tree}; accept_many | undo_accept {item_ids}. \
        done_when {item_id, done_when: [ci[:check] | review | test[:cmd] | \
        person | text]}; verify {item_id, line, ok, note?}: a person's check. \
        mission_start {mission_id, step?}: take the next steps; retry \
        {item_id, note?}; mission_plan: ask the planner; card_decide \
        {card_id, ok, note?}; mission_grant {mission_id, level, hours?, \
        budget_cents?, hosts?, max_parallel?, profile?}; mission_revoke; \
        missions_pause_all. mission_release_note {mission_id}: a drafted \
        release note of a completed mission; today_brief {refresh?, org_id?, \
        since?}: Today's morning brief, drafted only on refresh; \
        mission_triage {mission_id, refresh?}: a stuck mission's card, Jev's \
        proposed outcome and next step (never applied), and with refresh its \
        drafted words. \
        Work view: \
        primary:false links a secondary; expected_* guard (E_CONFLICT).")]
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
        if caller.is_operator() && matches!(args.action.as_str(), "resume" | "start" | "run") {
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
        if args.action == "summarize" {
            // Work graph M13.4c: a Claude-written summary of a dead session
            // (on demand only, D10 / D27). It spends a model call, so the
            // operator's request is confirmed like a start, and refused on a
            // hub, where no one can approve it. The host and org fences are
            // inside.
            let key = args
                .key
                .as_deref()
                .ok_or_else(|| mcp_err("E_INVALID", "summarize needs key", None))?;
            let link_id = args
                .link_id
                .ok_or_else(|| mcp_err("E_INVALID", "summarize needs link_id", None))?;
            if caller.is_operator() {
                self.confirm_gate(
                    "work_link",
                    args.confirm_nonce.as_deref(),
                    &format!(
                        "Summarise past work {} (link {link_id}) with a model call on its host",
                        bound_text(Some(key))
                    ),
                    &caller,
                )?;
            }
            // Multi-user M1 (T7): whose conversation this is, before the
            // model call that reads the whole of it. The link is resolved
            // once, by `summary`'s own function, so the gate and the run
            // cannot disagree about which conversation is meant; the
            // confirmation above comes first so that the operator's
            // approve-then-refuse path keeps answering in that order.
            // The WHOLE scope (T9c): the link is resolved through
            // `scope_links_for`, so another person's ended link answers
            // `no_such_link` here rather than leaking its host and
            // resumability through a different refusal.
            let view_scope = self.view_scope(&caller)?;
            let past = {
                let s = lock(&self.store).map_err(to_mcp_err)?;
                crate::service::work::summary::planned_conversation(&s, key, link_id, &view_scope)
                    .map_err(to_mcp_err)?
            };
            self.require_conversation_person(&caller, Some(&past.claude_session_id), || {
                crate::service::work::summary::no_such_link(&past.key, link_id)
            })?;
            let out = crate::service::work::summary::summarize(
                &self.store,
                self.ssh.as_ref(),
                key,
                link_id,
                &view_scope,
            )
            .await
            .map_err(to_mcp_err)?;
            return ok_json(&out);
        }
        if args.action == "handover" {
            // Work graph M9.3: ask a live session to write its hand-off (on
            // demand only, D9). The host and org fences are inside.
            let sid = args
                .session_id
                .ok_or_else(|| mcp_err("E_INVALID", "handover needs session_id", None))?;
            // `drive`: a handover request TYPES A PROMPT into the session's
            // pane and waits for the reply — the same pane write
            // `send_message { deliver, submit }` is, so it takes the same
            // level (and the desktop's `SESSION_TIER` agrees: F2c put
            // `request_work_handover` at `drive` for this reason).
            //
            // Person-gated WITHOUT the host fence in front, so a session
            // this caller may not reach answers exactly as an unknown id —
            // the answer the service fence already gave here, and the one
            // the isolation matrix compares against an unknown id.
            self.resolve_row_person_gated(
                &caller,
                sid,
                Reach::Drive,
                "the session to ask for a handover",
            )?;
            let row =
                crate::service::work::agent_handover::request(&self.store, &self.ssh, sid, &scope)
                    .await
                    .map_err(to_mcp_err)?;
            return ok_json(&row);
        }
        if args.action == "resume" {
            // The host fence (a per-host token resumes only onto its own
            // host) and the org fence are inside `resume_work`'s scope.
            let mut ra = crate::service::work::resume_args(&args).map_err(to_mcp_err)?;
            // Whose resume this is (multi-user M1, T5): the fallback when the
            // candidate names no conversation at all. The conversation's own
            // owner still wins, and the gate below has already refused a
            // conversation that is not this caller's.
            ra.owner = {
                let s = lock(self.reader()).map_err(to_mcp_err)?;
                super::fleet::owner_for(&caller, &s)
            };
            // Multi-user M1 (T7): the PERSON fence, which no scope carries.
            // A resume replays the candidate's whole transcript into a new
            // session on its host and answers that row, so it is the `own`
            // tier and it is checked against the conversation's own owner.
            // T5's `reject_foreign_conversation` does not reach this path:
            // `resume_session_args` derives the new row's `owner_person_id`
            // FROM the conversation, so there it compares a value with
            // itself. The candidate is chosen exactly as the resume chooses
            // it (`resume_source` is `plan_resume`), in every mode.
            let (link_id, cid) = {
                let s = lock(&self.store).map_err(to_mcp_err)?;
                crate::service::work::resume::resume_source(&s, &ra.key, ra.link_id, &scope)
                    .map_err(to_mcp_err)?
            };
            self.require_conversation_person(&caller, cid.as_deref(), || {
                crate::service::work::resume::no_such_ended_link(
                    &ra.key,
                    link_id.unwrap_or_default(),
                )
            })?;
            // The WHOLE scope (T9c), for the words of the two concurrency
            // refusals: the guards inside plan with `ViewScope::internal()`
            // so they see every live session on the key, and this is what
            // decides whether the `E_EXISTS` may name one.
            let view_scope = self.view_scope(&caller)?;
            let row = crate::service::work::resume::resume_work(
                &self.store,
                &self.ssh,
                &self.reg,
                &ra,
                &scope,
                &view_scope,
            )
            .await
            .map_err(to_mcp_err)?;
            return ok_json(&row);
        }
        if args.action == "trust_project" {
            // Trust is fleet configuration, not one host's (nor one bound
            // client's, M14) to change.
            // This is the org boundary, not a privacy fence: `trust_project` names no session:
            // it asks whether this caller may change FLEET configuration, which a per-host token
            // and an org-bound client may not.
            if caller.is_scoped() {
                return Err(mcp_err(
                    "E_FORBIDDEN",
                    "trust_project is not available to a per-host token or an org-bound client",
                    None,
                ));
            }
            return ok_json(
                &crate::service::work::trust_project(&args, &self.store).map_err(to_mcp_err)?,
            );
        }
        if args.action == "dismiss" {
            // Reopened work is fleet-wide, not one host's (nor one bound
            // client's, M14) to dismiss.
            // This is the org boundary, not a privacy fence: the same fleet-wide configuration
            // question for `dismiss`.
            if caller.is_scoped() {
                return Err(mcp_err(
                    "E_FORBIDDEN",
                    "dismiss is not available to a per-host token or an org-bound client",
                    None,
                ));
            }
            return ok_json(
                &crate::service::work::dismiss_reopened(&args, &self.store).map_err(to_mcp_err)?,
            );
        }
        // Shared work context (design 2026-09-29): native tasks, subtasks
        // and agent proposals. The scope gates are inside.
        if args.action == "create" {
            let made = crate::service::work::local::create_task(&args, &self.store, &scope)
                .map_err(to_mcp_err)?;
            // K5: which group a standalone task belongs in (off by default).
            if made.parent_id.is_none() {
                crate::service::decide::work_placement::spawn_ask(&self.store, made.id);
                // J7: whether it repeats an open tracker ticket (off by default).
                crate::service::decide::tracker_duplicate::spawn_ask(&self.store, made.id);
            }
            return ok_json(&made);
        }
        if args.action == "propose" {
            // The proposer is the caller's own session when it names one
            // (through the same gate as any session argument, so another
            // host's session name is never written or echoed), else the
            // caller's label.
            let proposer = match args.session_id {
                Some(sid) => {
                    let r = self.resolve_target_row(
                        &caller,
                        Some(sid),
                        None,
                        None,
                        // `drive`, like every other `work` action that
                        // writes a row about one session: the proposal is
                        // STORED in this session's name, so a caller who
                        // may only watch it cannot put words in its mouth.
                        Reach::Drive,
                        "the session",
                    )?;
                    // ONE spelling, shared with the fence that has to
                    // recognise it again (multi-user M1):
                    // `work::view::proposer_label`, whose doc says why.
                    crate::service::work::view::proposer_label(&r)
                }
                None => caller.label(),
            };
            let made = crate::service::work::local::propose(&args, &self.store, &scope, &proposer)
                .map_err(to_mcp_err)?;
            // K4: whether it repeats an existing task (off by default).
            crate::service::decide::duplicate::spawn_ask(&self.store, vec![made.id]);
            return ok_json(&made);
        }
        if args.action == "propose_tree" {
            // `propose`'s proposer, through the same gate at the same reach:
            // every proposal of the tree is stored in this session's name.
            let proposer = match args.session_id {
                Some(sid) => {
                    let r = self.resolve_target_row(
                        &caller,
                        Some(sid),
                        None,
                        None,
                        Reach::Drive,
                        "the session",
                    )?;
                    crate::service::work::view::proposer_label(&r)
                }
                None => caller.label(),
            };
            let made =
                crate::service::work::local::propose_tree(&args, &self.store, &scope, &proposer)
                    .map_err(to_mcp_err)?;
            crate::service::decide::duplicate::spawn_ask(
                &self.store,
                made.iter().map(|r| r.id).collect(),
            );
            return ok_json(&made);
        }
        // A person's decision on many proposals at once (orchestration O2),
        // and taking it back. Scoped callers are refused inside, before any
        // row; each item's mission is fenced on the whole `view_scope`. The
        // operator is unscoped, so it is refused here, as for `accept`.
        if args.action == "accept_many" {
            refuse_operator_decision(&caller)?;
            let view_scope = self.view_scope(&caller)?;
            return ok_json(
                &crate::service::work::graph::accept_many(&args, &self.store, &view_scope)
                    .map_err(to_mcp_err)?,
            );
        }
        if args.action == "undo_accept" {
            refuse_operator_decision(&caller)?;
            let view_scope = self.view_scope(&caller)?;
            return ok_json(
                &crate::service::work::graph::undo_accept(&args, &self.store, &view_scope)
                    .map_err(to_mcp_err)?,
            );
        }
        // An edge of the graph, or a hold (orchestration O2): a person's plan,
        // so never a per-host or peer token (`mission_caller`); the item's
        // own sessions pass the drive gate, as for `set_status`.
        if args.action == "dep" {
            mission_caller(&caller)?;
            self.require_drive_on_item_sessions(&caller, graph_item(&args)?)?;
            let view_scope = self.view_scope(&caller)?;
            return ok_json(
                &crate::service::work::graph::dep(&args, &self.store, &view_scope)
                    .map_err(to_mcp_err)?,
            );
        }
        if args.action == "hold" {
            mission_caller(&caller)?;
            self.require_drive_on_item_sessions(&caller, graph_item(&args)?)?;
            let view_scope = self.view_scope(&caller)?;
            return ok_json(
                &crate::service::work::graph::hold(&args, &self.store, &view_scope)
                    .map_err(to_mcp_err)?,
            );
        }
        if args.action == "done_when" {
            mission_caller(&caller)?;
            self.require_drive_on_item_sessions(&caller, graph_item(&args)?)?;
            let view_scope = self.view_scope(&caller)?;
            return ok_json(
                &crate::service::work::verify::set_done_when(&args, &self.store, &view_scope)
                    .map_err(to_mcp_err)?,
            );
        }
        if args.action == "verify" {
            refuse_operator_decision(&caller)?;
            let view_scope = self.view_scope(&caller)?;
            return ok_json(
                &crate::service::work::verify::verify(&args, &self.store, &view_scope)
                    .map_err(to_mcp_err)?,
            );
        }
        // A proposal decision is `accept` or `reject` with nothing else named;
        // `reject` WITH a session, a link or a key is the link decision, which
        // falls through to the shared tail below and takes its `Reach::Drive`.
        //
        // Spelled with `matches!` rather than `args.action == "reject"`: that
        // literal is how `mcp::tools::tests`' `umbrella_arms` finds the ARM
        // that serves an action, and a bare one in this CONDITION made the
        // scanner read `reject` as having its own arm — hiding the fact that
        // its session-addressed shape is gated by the tail, which is the claim
        // `WORK_ACTION_REACH` and `WORK_LINK_TAIL_ACTIONS` carry for it.
        // Identical in behaviour: the `if` short-circuits on `accept`, so
        // widening this binding to cover `accept` changes no outcome.
        let proposal_decision = args.session_id.is_none()
            && args.link_id.is_none()
            && args.key.is_none()
            && matches!(args.action.as_str(), "accept" | "reject");
        if args.action == "accept" || proposal_decision {
            // The operator is an unbound client, so its scope is `All` and
            // `decide`'s scope fence lets it through; but it is an agent, and
            // an agent never decides a proposal — not its own, not a worker's.
            refuse_operator_decision(&caller)?;
            return ok_json(
                &crate::service::work::local::decide(
                    &args,
                    &self.store,
                    &scope,
                    args.action == "accept",
                )
                .map_err(to_mcp_err)?,
            );
        }
        if args.action == "name" {
            // Work graph M11.1: name new local work on a session, or rename
            // a local item. The host and org fences are inside, and answer
            // another host's session as an unknown one (no existence oracle),
            // so the session is not resolved through the host gate here.
            // The caller decides whose link it is (D34): an agent's reads
            // `agent`, never a person's `manual`.
            use crate::service::work::local;
            return match (args.session_id, args.item_id) {
                (Some(sid), None) => {
                    // `drive`: naming work ON a session writes that session's
                    // work graph — a `manual`/`agent` link, a settled
                    // suggestion and a row-version bump the owner's sidebar
                    // re-groups on. The same level as every other
                    // per-session work write in this handler, and what
                    // `share.ts` already says (`name_session_work: 'drive'`).
                    //
                    // Person-gated WITHOUT the host fence in front, so a
                    // session this caller may not reach answers exactly as an
                    // unknown id — the answer `name_session_work_as`'s own
                    // service fence already gives (multi-user M1, T7).
                    self.resolve_row_person_gated(
                        &caller,
                        sid,
                        Reach::Drive,
                        "the session to name work on",
                    )?;
                    ok_json(
                        &local::name_session_work_as(
                            &args,
                            &self.store,
                            &scope,
                            caller.work_decider(),
                        )
                        .map_err(to_mcp_err)?,
                    )
                }
                (None, Some(item_id)) => {
                    // The rename half addresses a local work ITEM, not a
                    // session, and `local_item_visible` fences it by the
                    // item's links. A local item is reached through the
                    // sessions linked to it, so every live one of them must be
                    // this caller's to drive — the same level as the naming
                    // half, since the title it rewrites is what the owner's
                    // sidebar shows for their own row.
                    self.require_drive_on_item_sessions(&caller, item_id)?;
                    ok_json(
                        &local::rename_local_item(&args, &self.store, &scope)
                            .map_err(to_mcp_err)?,
                    )
                }
                _ => Err(mcp_err(
                    "E_INVALID",
                    "name takes exactly one of session_id (name new work) or item_id (rename)",
                    None,
                )),
            };
        }
        if args.action == "set_status" {
            // A person's status for work with no ticket (design
            // 2026-09-28 §2). The ORG fence is inside `set_status`: an item
            // outside the scope answers as an unknown id, and a tracker
            // item is `E_INVALID`, naming the ticket.
            let item_id = args
                .item_id
                .ok_or_else(|| mcp_err("E_INVALID", "set_status needs item_id", None))?;
            let status = args
                .status
                .as_deref()
                .ok_or_else(|| mcp_err("E_INVALID", "set_status needs status", None))?;
            // And the PERSON fence, which this arm did not have (multi-user
            // M1, T9d). The org fence inside `set_status` opens with
            // `if scope.is_all() { return Ok(true) }`, and `OrgScope::All`
            // is what an ordinary person's own device resolves to — so for
            // every person on the hub it was a no-op. `work { local_items }`
            // lists every local item's id by design, so any `full` device
            // could pick another person's item and set its status, which
            // `status_set_by = 'person'` makes FINAL over the derived value:
            // a stranger permanently marking somebody else's live work done.
            // The same gate and the same level as the rename half above —
            // the status is what the owner's own sidebar shows for their row.
            self.require_drive_on_item_sessions(&caller, item_id)?;
            return ok_json(
                &crate::service::work::status::set_status(&self.store, &scope, item_id, status)
                    .map_err(to_mcp_err)?,
            );
        }
        if args.action == "edit" {
            // Task editing: a local item's title, notes, assignees and due date. The
            // ORG fence is inside `edit_local_item` (an item outside the
            // scope answers as an unknown id, a tracker's ticket is
            // `E_INVALID`); the PERSON fence is the rename half's and
            // `set_status`'s — the text it rewrites is what the owner's own
            // sidebar shows for their row.
            let item_id = args
                .item_id
                .ok_or_else(|| mcp_err("E_INVALID", "edit needs item_id", None))?;
            self.require_drive_on_item_sessions(&caller, item_id)?;
            return ok_json(
                &crate::service::work::local::edit_local_item(&args, &self.store, &scope)
                    .map_err(to_mcp_err)?,
            );
        }
        // Task comments: about the ITEM, so its org fence (inside: an item
        // outside the scope answers as unknown), and the person fence is
        // `edit`'s — a comment is written into the owner's task. Deleting
        // one is its author's alone.
        if args.action == "comment" {
            let item_id = args
                .item_id
                .ok_or_else(|| mcp_err("E_INVALID", "comment needs item_id", None))?;
            self.require_drive_on_item_sessions(&caller, item_id)?;
            let person = self.view_scope(&caller)?.person;
            return ok_json(
                &crate::service::work::local::comment(
                    &args,
                    &self.store,
                    &scope,
                    &caller.label(),
                    person,
                )
                .map_err(to_mcp_err)?,
            );
        }
        if args.action == "comment_delete" {
            let person = self.view_scope(&caller)?.person;
            return ok_json(
                &crate::service::work::local::comment_delete(
                    &args,
                    &self.store,
                    &scope,
                    &caller.label(),
                    person,
                )
                .map_err(to_mcp_err)?,
            );
        }
        // Task attachments: a comment's fences — the item's org fence
        // inside, `edit`'s person gate for adding, the author's alone for
        // deleting.
        if args.action == "attach" {
            let item_id = args
                .item_id
                .ok_or_else(|| mcp_err("E_INVALID", "attach needs item_id", None))?;
            self.require_drive_on_item_sessions(&caller, item_id)?;
            let person = self.view_scope(&caller)?.person;
            return ok_json(
                &crate::service::work::attachments::attach(
                    &args,
                    &self.store,
                    &scope,
                    &caller.label(),
                    person,
                )
                .map_err(to_mcp_err)?,
            );
        }
        if args.action == "attachment_delete" {
            let person = self.view_scope(&caller)?.person;
            return ok_json(
                &crate::service::work::attachments::attachment_delete(
                    &args,
                    &self.store,
                    &scope,
                    &caller.label(),
                    person,
                )
                .map_err(to_mcp_err)?,
            );
        }
        if args.action == "set_parent" {
            // Epics (design 2026-09-28 §3, §7): a local item filed under an
            // epic or a task. The ORG fence is inside `set_parent` (the item
            // and the parent each answer as unknown outside the scope); the
            // PERSON fence is `edit`'s — where a person's live work is filed
            // is theirs to drive.
            let item_id = args
                .item_id
                .ok_or_else(|| mcp_err("E_INVALID", "set_parent needs item_id", None))?;
            self.require_drive_on_item_sessions(&caller, item_id)?;
            return ok_json(
                &crate::service::work::local::set_parent(&args, &self.store, &scope)
                    .map_err(to_mcp_err)?,
            );
        }
        // Sprint and release membership (design 2026-09-28 §7): the team's
        // plan, which a session does not decide — so never a per-host token
        // (`planned_item`). The org fence (bucket and item, each answering as
        // unknown outside the scope) is inside, and the person fence is
        // `set_status`'s: planning someone's live work is theirs to drive.
        // A personal bucket is its person's alone, so the whole
        // `view_scope` decides which bucket the caller sees.
        if args.action == "bucket_add" {
            let item_id = planned_item(&caller, &args)?;
            self.require_drive_on_item_sessions(&caller, item_id)?;
            let view_scope = self.view_scope(&caller)?;
            return ok_json(
                &crate::service::work::buckets::bucket_add(&args, &self.store, &view_scope)
                    .map_err(to_mcp_err)?,
            );
        }
        if args.action == "bucket_remove" {
            let item_id = planned_item(&caller, &args)?;
            self.require_drive_on_item_sessions(&caller, item_id)?;
            let view_scope = self.view_scope(&caller)?;
            return ok_json(
                &crate::service::work::buckets::bucket_remove(&args, &self.store, &view_scope)
                    .map_err(to_mcp_err)?,
            );
        }
        // A person's sprints and releases (owner decision 2026-10-10):
        // their personal ones, and an org's team ones as its admin — or
        // member, when the org allows it (`buckets::may_plan`, inside). Never
        // a per-host token: a session does not decide a plan.
        if args.action == "bucket_admin" {
            if caller.host_alias.is_some() {
                return Err(mcp_err(
                    "E_FORBIDDEN",
                    "a session does not plan sprints or releases; a person does",
                    None,
                ));
            }
            let op = args.bucket_op.as_ref().ok_or_else(|| {
                mcp_err(
                    "E_INVALID",
                    "bucket_admin needs bucket_op: {action: bucket_create | bucket_update \
                     | bucket_close | bucket_delete, ...}",
                    None,
                )
            })?;
            // The master is the fleet's administrator: what its work_admin
            // may do, its bucket_admin may too.
            let view_scope = if caller.is_master() {
                crate::service::view_scope::ViewScope::internal()
            } else {
                self.view_scope(&caller)?
            };
            return ok_json(
                &crate::service::work::buckets::person_admin(op, &self.store, &view_scope)
                    .map_err(to_mcp_err)?,
            );
        }
        // Missions (orchestration O1): a person's plan, which a session does
        // not run — so never a per-host or peer token (`mission_caller`).
        // The org and person fences are inside (`missions::changeable`), on
        // the whole `view_scope`; adding an item also passes the item's own
        // sessions' person gate, as a sprint does.
        if args.action.starts_with("mission_") {
            mission_caller(&caller)?;
            let view_scope = self.view_scope(&caller)?;
            use crate::service::work::missions as ms;
            use crate::service::work::orchestrate as orch;
            return match args.action.as_str() {
                "mission_save" => {
                    ok_json(&ms::save(&args, &self.store, &view_scope).map_err(to_mcp_err)?)
                }
                "mission_state" => {
                    ok_json(&ms::set_state(&args, &self.store, &view_scope).map_err(to_mcp_err)?)
                }
                "mission_repo" => {
                    ok_json(&ms::repo(&args, &self.store, &view_scope).map_err(to_mcp_err)?)
                }
                "mission_item" => {
                    let item_id = args
                        .item_id
                        .ok_or_else(|| mcp_err("E_INVALID", "mission_item needs item_id", None))?;
                    self.require_drive_on_item_sessions(&caller, item_id)?;
                    ok_json(&ms::item(&args, &self.store, &view_scope).map_err(to_mcp_err)?)
                }
                "mission_delete" => {
                    ok_json(&ms::delete(&args, &self.store, &view_scope).map_err(to_mcp_err)?)
                }
                "mission_import" => ok_json(
                    &crate::service::work::plan_import::import(&args, &self.store, &view_scope)
                        .map_err(to_mcp_err)?,
                ),
                // The loop (orchestration O4–O6): a person takes the next
                // steps, asks the planner, signs or ends a grant.
                "mission_start" => ok_json(
                    &orch::start(&args, &self.mission_deps(), &view_scope)
                        .await
                        .map_err(to_mcp_err)?,
                ),
                "mission_plan" => ok_json(
                    &orch::plan_now(&args, &self.mission_deps(), &view_scope)
                        .await
                        .map_err(to_mcp_err)?,
                ),
                "mission_grant" => {
                    ok_json(&orch::grant(&args, &self.store, &view_scope).map_err(to_mcp_err)?)
                }
                "mission_revoke" => {
                    ok_json(&orch::revoke(&args, &self.store, &view_scope).map_err(to_mcp_err)?)
                }
                // Redesign 9.11: an LLM draft, on demand.
                "mission_release_note" => ok_json(
                    &orch::drafts::release_note(&args, &self.mission_deps(), &view_scope)
                        .await
                        .map_err(to_mcp_err)?,
                ),
                // Redesign 9.10: a stuck mission's card; Jev proposes,
                // nothing changes.
                "mission_triage" => ok_json(
                    &orch::triage::triage(&args, &self.mission_deps(), &view_scope)
                        .await
                        .map_err(to_mcp_err)?,
                ),
                other => Err(mcp_err(
                    "E_INVALID",
                    format!("unknown work_link action {other:?}"),
                    None,
                )),
            };
        }
        // Another attempt at a mission's item, a card of its confirm queue,
        // and Pause all (orchestration O4–O6): a person's, never a
        // session's; the mission fences are inside.
        if args.action == "retry" {
            mission_caller(&caller)?;
            let view_scope = self.view_scope(&caller)?;
            return ok_json(
                &crate::service::work::orchestrate::retry(&args, &self.mission_deps(), &view_scope)
                    .await
                    .map_err(to_mcp_err)?,
            );
        }
        if args.action == "card_decide" {
            mission_caller(&caller)?;
            let view_scope = self.view_scope(&caller)?;
            return ok_json(
                &crate::service::work::orchestrate::decide_card(
                    &args,
                    &self.mission_deps(),
                    &view_scope,
                )
                .await
                .map_err(to_mcp_err)?,
            );
        }
        // Redesign 9.11: Today's morning brief, a person's, on their own
        // view of today; drafted only when they ask for a refresh.
        if args.action == "today_brief" {
            mission_caller(&caller)?;
            let view_scope = self.view_scope(&caller)?;
            return ok_json(
                &crate::service::work::orchestrate::drafts::brief(
                    &args,
                    &self.mission_deps(),
                    &view_scope,
                )
                .await
                .map_err(to_mcp_err)?,
            );
        }
        if args.action == "missions_pause_all" {
            mission_caller(&caller)?;
            let view_scope = self.view_scope(&caller)?;
            return ok_json(
                &crate::service::work::orchestrate::pause_all(&self.store, &view_scope)
                    .map_err(to_mcp_err)?,
            );
        }
        if args.action == "tidy_apply" {
            let mut items = args.items.clone().unwrap_or_default();
            // The batch's size is the size the CALLER asked for, judged
            // before the gate below thins it: a batch every item of which
            // the gate refused is a batch that was asked, and must answer
            // per item, not `E_INVALID`.
            crate::service::work::tidy::check_apply_items(items.len()).map_err(to_mcp_err)?;
            // Multi-user M1 (T7), per item and by the item's own action.
            //
            // `kill` and `safe_kill` are the spec's `own` tier —
            // `kill_session` / `safe_kill_session` are named there, and the
            // batch form of an operation can never be wider than the
            // single-session one (F2a took the same decision on the desktop
            // side, which is why `tidy_apply` is `own` in `SESSION_TIER`).
            // `archive`, `unarchive`, `snooze`, `never` and `keep` are the
            // UI-only bookkeeping the standalone `work_link` actions are,
            // so they take the same `drive` those do: a tier is decided by
            // what the call DOES, and reading one as `own` because its
            // siblings are would refuse a driver a snooze.
            //
            // A refused item is a refused ITEM, not a refused call: this
            // batch has always answered per item ("session N not found"),
            // and failing the whole call would turn one unreachable row
            // into a tidy-up a person cannot run at all. The gate's own
            // sentence rides the item's `error`.
            //
            // Both run BEFORE the confirmation, so a request that can only
            // be refused is never put to a person — the same ordering the
            // multi-repo start above uses.
            let mut refused: Vec<crate::service::work::tidy::TidyApplyResult> = Vec::new();
            items.retain(|item| {
                let reach = match item.action.as_str() {
                    "kill" | "safe_kill" => Reach::Own,
                    _ => Reach::Drive,
                };
                match self.resolve_row_person_gated(
                    &caller,
                    item.session_id,
                    reach,
                    "a session to tidy up",
                ) {
                    Ok(_) => true,
                    Err(e) => {
                        refused.push(crate::service::work::tidy::TidyApplyResult {
                            session_id: item.session_id,
                            action: item.action.clone(),
                            ok: false,
                            outcome: None,
                            error: Some(e.message.to_string()),
                        });
                        false
                    }
                }
            });
            if items.is_empty() {
                // Every item was refused above. The batch still answers,
                // per item, as it always has.
                return ok_json(&crate::service::work::tidy::TidyApplyReport { results: refused });
            }
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
            let mut report = crate::service::work::tidy::tidy_apply(
                &self.store,
                &exec,
                &items,
                &scope,
                crate::service::catalog::now_secs(),
            )
            .await
            .map_err(to_mcp_err)?;
            report.results.extend(refused);
            return ok_json(&report);
        }
        if args.action == "abandon_start" {
            // Task → session P-6: cancel a start that made a session and a
            // checkout nobody has used yet. It KILLS the session, so it takes
            // the kill's tier (`own`, as `kill_session` and tidy-up's kills
            // do), through the host fence, and the kill's confirmation —
            // always for the operator (D12). The service refuses (`E_DIRTY`)
            // anything with work in it, before anything is touched.
            let sid = args
                .session_id
                .ok_or_else(|| mcp_err("E_INVALID", "abandon_start needs session_id", None))?;
            let row = self.resolve_target_row(
                &caller,
                Some(sid),
                None,
                None,
                Reach::Own,
                "the session whose start to cancel",
            )?;
            // What the cancel would act on, before the confirmation: a
            // session no start made is refused, never put to a person.
            {
                let s = lock(&self.store).map_err(to_mcp_err)?;
                crate::service::work::abandon::plan_abandon(&s, row.id).map_err(to_mcp_err)?;
            }
            self.confirm_gate(
                "work_link",
                args.confirm_nonce.as_deref(),
                &format!(
                    "Cancel the start of {} on {}: end it, remove its new checkout and branch",
                    row.tmux_name, row.host_alias
                ),
                &caller,
            )?;
            let out = crate::service::work::abandon::abandon_start(&self.store, &self.ssh, row.id)
                .await
                .map_err(to_mcp_err)?;
            return ok_json(&out);
        }
        if args.action == "preview_start" {
            // Task → session spec P-1: where `start` with these arguments would
            // land, what it would send and what is in the way. Nothing is
            // made, so nothing is confirmed, the operator's included; the
            // fences are `plan_resolved`'s own, through the WHOLE scope as the
            // start's are (its conflicts name a session only when the caller
            // may see it). Its own action, not a flag on `start`: a hub older
            // than this answers `E_INVALID "unknown work_link action"` instead
            // of starting a session it was only asked to preview.
            if args.project_ids.is_some() {
                return Err(mcp_err(
                    "E_INVALID",
                    "preview_start previews one repository: pass project_id, not project_ids",
                    None,
                ));
            }
            let (owner, origin) = {
                let s = lock(self.reader()).map_err(to_mcp_err)?;
                (
                    super::fleet::owner_for(&caller, &s),
                    super::fleet::origin_for(&caller, &s),
                )
            };
            let view_scope = self.view_scope(&caller)?;
            // Jev K1 asks only when its gate opens (off by default).
            let decide = crate::service::decide::DecideCtx::jev(std::sync::Arc::clone(&self.store));
            let draft = args.draft_brief == Some(true);
            if draft && caller.is_operator() {
                // A draft spends a model call, like a summary: confirmed.
                self.confirm_gate(
                    "work_link",
                    args.confirm_nonce.as_deref(),
                    &format!(
                        "Draft the brief for {} with a model call on its host",
                        bound_text(args.key.as_deref())
                    ),
                    &caller,
                )?;
            }
            let mut preview = crate::service::trackers::tickets::preview_start_decided(
                &self.store,
                &crate::service::work::start_args_owned(
                    &args,
                    caller.work_decider(),
                    owner,
                    Some(origin.clone()),
                ),
                &view_scope,
                &crate::service::trackers::default_net(),
                Some(&decide),
            )
            .await
            .map_err(to_mcp_err)?;
            if draft {
                // Redesign 6.10: on the planned host, under the same scope
                // the preview was planned with.
                crate::service::work::brief_draft::draft_into(
                    &self.store,
                    self.ssh.as_ref(),
                    &mut preview,
                    &view_scope,
                )
                .await
                .map_err(to_mcp_err)?;
            }
            return ok_json(&preview);
        }
        if args.action == "start" {
            // The caller decides whose start it is (D34): an agent's links
            // `agent_started`, never a person's `started`.
            //
            // And WHOSE the new session is (multi-user M1, T5): this
            // connection's own person. Without it `start_one` fell back to
            // `hub_personal_owner`, so a second person's start created a
            // private session owned by the HUB's owner — readable by somebody
            // who did not ask for it, and `null` in the answer to the person
            // who did, because T8 drops a row the caller may not see.
            let (owner, origin) = {
                let s = lock(self.reader()).map_err(to_mcp_err)?;
                (
                    super::fleet::owner_for(&caller, &s),
                    super::fleet::origin_for(&caller, &s),
                )
            };
            // The whole scope, for the two things a start does that reach an
            // EXISTING row (multi-user M1, T9b): its `E_EXISTS` prose names
            // the session already on the key, and the branch slug it plans
            // can resolve to a checkout somebody else's session is working
            // in, where the new pane would land. Both are fenced inside
            // `tickets::plan_resolved` — the landing one by
            // `service::sessions::require_may_land_in_worktree`, at
            // `may_drive` strength, per planned sibling for the multi-repo
            // form, which is why the gate is there and not here.
            let view_scope = self.view_scope(&caller)?;
            if let Some(ids) = multi.as_deref() {
                // Work graph M9.6: one sibling per repository, same branch.
                let out = crate::service::trackers::tickets::start_work_many(
                    &self.store,
                    &self.ssh,
                    &self.reg,
                    &crate::service::work::start_args_owned(
                        &args,
                        caller.work_decider(),
                        owner,
                        Some(origin.clone()),
                    ),
                    ids,
                    &view_scope,
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
                &crate::service::work::start_args_owned(
                    &args,
                    caller.work_decider(),
                    owner,
                    Some(origin.clone()),
                ),
                &view_scope,
                &crate::service::trackers::default_net(),
            )
            .await
            .map_err(to_mcp_err)?;
            return ok_json(&row);
        }
        if args.action == "run" {
            if caller.host_alias.is_some() || caller.mode == crate::mcp::auth::TokenMode::Peer {
                // Orchestration O0: a run is a person's or the operator's. A
                // worker's own Claude proposes work; it never starts another
                // worker on it (task → session spec §6, isolation). Refused
                // before any lookup, so it answers the same for every item.
                return Err(mcp_err(
                    "E_FORBIDDEN",
                    "run is for a person or the operator; a per-host agent may propose instead",
                    None,
                ));
            }
            // Orchestration O0: one attempt at an existing item, through the
            // start path (its project, host and worktree, the brief), tracked
            // as a task whose first prompt carries the done marker. The
            // fences are the start's own, under the whole scope.
            let (owner, origin) = {
                let s = lock(self.reader()).map_err(to_mcp_err)?;
                (
                    super::fleet::owner_for(&caller, &s),
                    super::fleet::origin_for(&caller, &s),
                )
            };
            let view_scope = self.view_scope(&caller)?;
            let out = crate::service::work::run::run_item(
                &self.store,
                &self.ssh,
                &self.reg,
                &crate::service::work::start_args_owned(
                    &args,
                    caller.work_decider(),
                    owner,
                    Some(origin.clone()),
                ),
                args.role.as_deref().unwrap_or("implement"),
                &view_scope,
                &crate::service::trackers::default_net(),
            )
            .await
            .map_err(to_mcp_err)?;
            return ok_json(&out);
        }
        // Work graph M14.1c: the Work view's structure and batch decisions.
        // Who may write what is inside (`structure`): never a per-host
        // token; a bound client its own org's placements and views only;
        // rules and org moves the master and unbound clients only.
        {
            use crate::service::work::structure as st;
            let need_task = || {
                args.task_id.as_deref().ok_or_else(|| {
                    mcp_err("E_INVALID", format!("{} needs task_id", args.action), None)
                })
            };
            match args.action.as_str() {
                "place" => {
                    // The whole scope, because what `place` ANSWERS is the
                    // task — `WorkTask.sessions: Vec<TaskLink>`, every link
                    // of it with its name, host, branch and live
                    // `claude_status` (T9b). The write is a group; the answer
                    // is a page of sessions.
                    let view_scope = self.view_scope(&caller)?;
                    return ok_json(
                        &st::place(
                            &self.store,
                            &view_scope,
                            need_task()?,
                            args.group.as_deref(),
                            args.note.as_deref(),
                            args.expected_version,
                            &caller.label(),
                        )
                        .map_err(to_mcp_err)?,
                    );
                }
                "assign_org" => {
                    // The whole scope, because the `E_CONFLICT` this answers
                    // when the preview went stale carries a FRESH `OrgImpact`
                    // in its details, links and all, and because the impact
                    // token it compares is a hash over that list (T8d).
                    let view_scope = self.view_scope(&caller)?;
                    return ok_json(
                        &st::assign_org(
                            &self.store,
                            &view_scope,
                            need_task()?,
                            args.org_id,
                            args.impact_token.as_deref(),
                        )
                        .map_err(to_mcp_err)?,
                    );
                }
                "rule_save" => {
                    let rule = args
                        .rule
                        .as_ref()
                        .ok_or_else(|| mcp_err("E_INVALID", "rule_save needs rule", None))?;
                    return ok_json(&st::rule_save(&self.store, &scope, rule).map_err(to_mcp_err)?);
                }
                "rule_delete" => {
                    let id = args
                        .rule_id
                        .ok_or_else(|| mcp_err("E_INVALID", "rule_delete needs rule_id", None))?;
                    return ok_json(
                        &st::rule_delete(&self.store, &scope, id, args.expected_version)
                            .map_err(to_mcp_err)?,
                    );
                }
                "view_save" => {
                    let v = args
                        .view
                        .as_ref()
                        .ok_or_else(|| mcp_err("E_INVALID", "view_save needs view", None))?;
                    return ok_json(&st::view_save(&self.store, &scope, v).map_err(to_mcp_err)?);
                }
                "view_delete" => {
                    let id = args
                        .view_id
                        .ok_or_else(|| mcp_err("E_INVALID", "view_delete needs view_id", None))?;
                    return ok_json(
                        &st::view_delete(&self.store, &scope, id, args.expected_version)
                            .map_err(to_mcp_err)?,
                    );
                }
                "decide_batch" => {
                    let decisions = args.decisions.as_deref().unwrap_or_default();
                    // Each decision's session passes the same gate a single
                    // decision's does (the host fence, the bound client's
                    // session fence), with the gate's own code and sentence.
                    let gate = |sid: i64| -> Result<(), IpcError> {
                        self.resolve_target_row(
                            &caller,
                            Some(sid),
                            None,
                            None,
                            // `drive`, per item: a batch of per-session
                            // work-graph decisions cannot be wider than
                            // the single decision below, which is the
                            // same write by another name.
                            Reach::Drive,
                            "the session",
                        )
                        .map(|_| ())
                        .map_err(ipc_of_mcp)
                    };
                    return ok_json(
                        &st::decide_batch(
                            &self.store,
                            &scope,
                            caller.work_decider(),
                            decisions,
                            &gate,
                        )
                        .map_err(to_mcp_err)?,
                    );
                }
                _ => {}
            }
        }
        let sid = args.session_id.ok_or_else(|| {
            mcp_err(
                "E_INVALID",
                format!("{} needs session_id", args.action),
                None,
            )
        })?;
        // `drive`: every `work_link` action that reaches here writes the
        // session's own work graph — link, reject, confirm, unlink, ack,
        // reconsider, set_primary, archive, snooze. They are row writes
        // about one session, and none of them is in the spec's `own` list.
        //
        // The actions that ARE have already returned above, each gated where
        // it returns, and the three are not gated the same way because they
        // are not addressed the same way: `tidy_apply` names sessions, so it
        // takes this very gate per item at `Reach::Own`; `summarize` and
        // `resume` name a work key and a link id, so no session gate can see
        // them and they are checked against the CONVERSATION's owner instead
        // (`require_conversation_person`).
        self.resolve_target_row(&caller, Some(sid), None, None, Reach::Drive, "the session")?;
        // Task → session P-3, asked for with `ack_live: false`: the task's
        // other live sessions, named only when this caller may see them.
        crate::service::work::check_live_elsewhere(&args, &self.store, &self.view_scope(&caller)?)
            .map_err(to_mcp_err)?;
        // The caller, not the `source` it passes, decides whether this is a
        // person's decision or an agent's (D34). A decision on one link by
        // id also answers that link's new version (`link_version`).
        let row = crate::service::work::work_link_decided(
            &args,
            &self.store,
            &scope,
            caller.work_decider(),
        )
        .map_err(to_mcp_err)?;
        ok_json(&row)
    }

    #[tool(description = "Trackers, orgs, retention, usage counts, sprints \
        and releases; see action. Never returns a secret.")]
    pub(super) async fn work_admin(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(args): Parameters<crate::service::trackers::admin::WorkAdminArgs>,
    ) -> Result<CallToolResult, McpError> {
        use crate::service::trackers::admin::{self as a, AdminAction};
        // `Access::Person` let in the master and the hub owner's own unbound
        // device. A device reaches the trackers only (connect, test,
        // disconnect from the phone), and only when trusted and `full`;
        // orgs, retention, usage and buckets stay the master's. The audit
        // line is built by hand: the secret never reaches it, not even as a
        // length.
        let summary = args.audit_summary();
        audit("work_admin", &summary);
        let action = AdminAction::parse(&args.action).map_err(to_mcp_err)?;
        if !caller.is_master() {
            if !action.is_tracker_action() {
                return Err(mcp_err(
                    "E_FORBIDDEN",
                    "a device manages trackers here (list, add, update, set_credential, \
                     test, remove); the rest of work_admin is the hub operator's",
                    None,
                ));
            }
            super::fleet::owner_device_admin(&caller, "managing trackers")?;
            // Review r04 S1: a device sends the secret itself. A
            // `credential_ref` makes the hub read one of its own files or
            // environment variables and send it to the tracker's site; a
            // private network or an extra CA reaches past the hub's network
            // fence. All three stay the hub operator's.
            if args.credential_ref.is_some() {
                return Err(mcp_err(
                    "E_FORBIDDEN",
                    "a device sends the tracker's secret itself; a credential reference \
                     (env: or file: on the hub) is the hub operator's",
                    None,
                ));
            }
            let fenced = |k: &str| {
                args.settings
                    .as_ref()
                    .and_then(|s| s.get(k))
                    .is_some_and(|v| !v.is_null() && *v != serde_json::json!(false))
            };
            if fenced("allow_private_network") || fenced("extra_ca") {
                return Err(mcp_err(
                    "E_FORBIDDEN",
                    "allow_private_network and extra_ca reach past the hub's network \
                     fence; they are the hub operator's",
                    None,
                ));
            }
        }
        match action {
            // M11.4's sync metrics and M12.3's retention (rows, dry run,
            // last sweep), each read under its own short locks.
            AdminAction::Status => {
                let trackers = a::admin_sync(&args, &self.store).map_err(to_mcp_err)?;
                let retention = crate::service::work::retention::status(
                    &self.store,
                    crate::service::catalog::now_secs(),
                )
                .map_err(to_mcp_err)?;
                ok_json(&serde_json::json!({ "trackers": trackers, "retention": retention }))
            }
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

    /// The person gate for the two `work_link` actions that take over a
    /// CONVERSATION without naming a session row (multi-user M1, T7).
    ///
    /// `resume` replays a transcript into a new session on its host, spending
    /// that host's AI account, and answers the new `SessionRow`; `summarize`
    /// forks the conversation for a model-written précis and stores it as a
    /// durable journal row. Both are the spec's `own` tier — §4.3 invariant 5
    /// names `work_link { summarize }` outright, and the plan's T7 handoff
    /// table puts `resume_work` there ("it takes over a conversation, like
    /// `rewind_conversation`"). Neither carries a `session_id`: they are
    /// addressed by a work key and a link id, so `resolve_target_row`'s gate
    /// below never sees them, and the only identity that outlives the dead
    /// row they act on is T3's durable `conversation_owners` record.
    ///
    /// The predicate is
    /// [`crate::service::view_scope::ViewScope::sees_past_conversation`], and
    /// it asks three questions in order so that the two mechanisms M1 has
    /// cannot disagree: a SURVIVING `sessions` row decides first
    /// (`may_own` — the same answer `session_transcript` gives for that row),
    /// then T3's durable `conversation_owners` record, and only a conversation
    /// with neither passes.
    ///
    /// That order matters for an `unclaimed` row, and it is the half this gate
    /// first shipped without. Migration 099's triggers record an owner only
    /// `WHEN NEW.owner_person_id IS NOT NULL`, so every reconcile-discovered
    /// session has a real transcript and NO record — and
    /// `conversation_owner_allows` answers `None => true`. On a hub with two
    /// people that made the same past work `E_NOTFOUND` through
    /// `session_transcript` and summarisable through `work_link { summarize }`.
    /// Rule 7 ("the upgrade widens nothing", and must not narrow a
    /// single-person install) is what the bare pass-through is for, and it is
    /// kept for exactly the case rule 7 is about: nothing recorded AND no
    /// surviving row, i.e. a genuinely pre-M1 conversation whose row was
    /// reaped. `ViewScope::is_sole_person` keeps the one-person install whole
    /// on the first arm.
    ///
    /// A per-host token proves no person, so it reaches a conversation only
    /// through the pane it is standing in — a pane proof says "I am standing
    /// in this session", never whose work it is, and `may_own` refuses it
    /// (`ViewScope::may_own`'s own reasoning).
    ///
    /// `not_found` is the LINK's own refusal, never `E_FORBIDDEN`: the thing
    /// addressed here is a link, and one whose conversation is not this
    /// caller's must read exactly like a link that does not exist — the
    /// answer its org fence already gives.
    pub(super) fn require_conversation_person(
        &self,
        caller: &Caller,
        claude_session_id: Option<&str>,
        not_found: impl FnOnce() -> IpcError,
    ) -> Result<(), McpError> {
        let Some(cid) = claude_session_id else {
            // No conversation recorded, so nothing to own: the action says so
            // itself further in ("no Claude conversation was recorded",
            // `E_NO_TRANSCRIPT`), and inventing a refusal here would hide it.
            return Ok(());
        };
        let s = lock(&self.store).map_err(to_mcp_err)?;
        let view = caller.view_scope(&s).map_err(to_mcp_err)?;
        if view.sees_past_conversation(&s, cid).map_err(to_mcp_err)? {
            return Ok(());
        }
        Err(to_mcp_err(not_found()))
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

    /// The caller's FULL scope — org and person (multi-user M1, T6) — read
    /// under the same short lock as [`Self::org_scope`], whose answer it
    /// carries as its `.org` field.
    ///
    /// A handler that answers a PAGE of session-derived rows takes this and
    /// nothing else: `Caller::org_scope` cannot tell two people on one hub
    /// apart (it is `OrgScope::All` for every paired client bound to no org
    /// and for the master alike), so an arm fenced by the org scope alone
    /// serves every private session in the fleet to anybody's phone. The
    /// coverage gate holds each such arm to the word `view_scope` by name
    /// (`mcp::tools::tests::every_session_addressed_tool_declares_its_reach`,
    /// clause 4), which is why this is threaded rather than rebuilt inside
    /// each service function.
    pub(super) fn view_scope(
        &self,
        caller: &Caller,
    ) -> Result<crate::service::view_scope::ViewScope, McpError> {
        let s = lock(&self.store).map_err(to_mcp_err)?;
        caller.view_scope(&s).map_err(to_mcp_err)
    }
}

/// A coded [`McpError`] (built by `mcp_err` / `to_mcp_err`) back as the
/// [`IpcError`] it carries — its code, its sentence without the code
/// prefix, its details — for a batch item's result (work graph M14.1c).
fn ipc_of_mcp(e: McpError) -> IpcError {
    let data = e.data.as_ref();
    let code = data
        .and_then(|d| d.get(super::support::ERR_CODE_KEY))
        .and_then(|c| c.as_str())
        .unwrap_or(codes::E_FORBIDDEN)
        .to_string();
    let message = e
        .message
        .strip_prefix(&format!("{code}: "))
        .unwrap_or(&e.message)
        .to_string();
    IpcError {
        code,
        message,
        details: None,
    }
}

/// The item a `work_link { dep | hold }` changes.
fn graph_item(args: &crate::service::work::WorkLinkArgs) -> Result<i64, McpError> {
    args.item_id
        .ok_or_else(|| mcp_err("E_INVALID", format!("{} needs item_id", args.action), None))
}

impl super::FleetTools {
    /// What the mission loop needs to take a step from this server.
    fn mission_deps(&self) -> crate::service::work::orchestrate::Deps {
        crate::service::work::orchestrate::Deps {
            store: self.store.clone(),
            ssh: self.ssh.clone(),
            reg: self.reg.clone(),
            net: crate::service::trackers::default_net(),
        }
    }
}

/// Refuse a `work_link { mission_* }` from a per-host or peer token: a
/// mission is a person's plan, and a session or another hub does not run
/// one (orchestration O1).
fn mission_caller(caller: &Caller) -> Result<(), McpError> {
    if caller.host_alias.is_some() || caller.mode == crate::mcp::auth::TokenMode::Peer {
        return Err(mcp_err(
            "E_FORBIDDEN",
            "a session does not run missions; a person does",
            None,
        ));
    }
    Ok(())
}

/// The item a `work_link { bucket_add | bucket_remove }` plans, refusing a
/// per-host token first: a session does not decide the team's plan
/// (design 2026-09-28 §7).
fn planned_item(
    caller: &Caller,
    args: &crate::service::work::WorkLinkArgs,
) -> Result<i64, McpError> {
    if caller.host_alias.is_some() {
        return Err(mcp_err(
            "E_FORBIDDEN",
            "a session does not plan sprints or releases; a person does",
            None,
        ));
    }
    args.item_id
        .ok_or_else(|| mcp_err("E_INVALID", format!("{} needs item_id", args.action), None))
}

/// The operator is an unbound client, so its view scope is `All` and the
/// scope fences of `decide`, `accept_many`, `undo_accept` and `verify` let it
/// through; but it is an agent, and an agent never decides a proposal (not
/// its own, not a worker's) nor records a check as verified: those are a
/// person's acts (transition plan, "Where AI never decides").
fn refuse_operator_decision(caller: &Caller) -> Result<(), McpError> {
    if caller.is_operator() {
        return Err(mcp_err(
            "E_FORBIDDEN",
            "a person decides proposals and verifies work, from the desktop or \
             the phone; the operator may only propose",
            None,
        ));
    }
    Ok(())
}
