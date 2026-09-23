//! MCP tools: prompts, broadcasts, messages, inbox and history.

use super::*;
use crate::ipc_error::lock;

#[tool_router(router = messaging_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Send and SUBMIT a prompt to a running Claude \
        session's REPL (pasted, then one Enter). The first prompt to a \
        still-unnamed session also becomes its friendly name. \
        Marked untrusted unless raw=true (master only) or a trusted client. \
        keys=Enter|Escape|C-c|1-9 presses a key instead (unmarked; 1-9 \
        answers pending_input). \
        Returns JSON { delivered, session_id, turn_seq_before, queued, acked \
        }: pass turn_seq_before to wait_for_session \
        { until: \"turn_gt\" } or session_transcript { since_turn } to \
        collect the reply (or use run_prompt, which does all three). \
        Refuses a blocked or stuck session (E_INVALID_STATE) unless \
        force=true; a working session queues it (queued=true). acked: true \
        = hook-confirmed, false = not within 1.5 s (check capture_session), \
        null = unknowable. Repeat a client_msg_id to retry without \
        delivering twice.")]
    pub(super) async fn send_prompt(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SendPromptParams>,
    ) -> Result<CallToolResult, McpError> {
        // Prompt body intentionally not logged.
        audit(
            "send_prompt",
            &format!(
                "session_id={:?} host={:?} session={:?} keys={:?}",
                p.session_id, p.host_alias, p.tmux_name, p.keys
            ),
        );
        let row = self.resolve_target_row(
            &caller,
            p.session_id,
            p.host_alias.as_deref(),
            p.tmux_name.as_deref(),
            "the session to prompt",
        )?;
        if let Some(k) = p.keys.as_deref() {
            let key = crate::tmux::NamedKey::parse(k).ok_or_else(|| {
                mcp_err(
                    codes::E_VALIDATE,
                    format!(
                        "keys must be {}, not {k:?}",
                        crate::tmux::NamedKey::VOCABULARY
                    ),
                    None,
                )
            })?;
            if !p.prompt.is_empty() {
                return Err(mcp_err(
                    codes::E_VALIDATE,
                    "keys and a non-empty prompt cannot be sent together",
                    None,
                ));
            }
            sessions::send_keys(&row.host_alias, &row.tmux_name, key, &self.store, &self.ssh)
                .await
                .map_err(to_mcp_err)?;
            return ok_json(&serde_json::json!({
                "delivered": true,
                "session_id": row.id,
                "turn_seq_before": row.turn_seq,
            }));
        }
        let label = caller.label();
        // The key is RESERVED before the send, not written after it. A send
        // is an SSH round trip plus up to 1.5 s of ack wait, and a caller
        // that gives up and retries does so DURING that window — which a
        // write-on-success cache does not cover at all: both calls miss, both
        // deliver, and the session gets the prompt twice.
        let dedupe_id = p.client_msg_id.clone();
        if let Some(id) = dedupe_id.as_deref() {
            match lock_sends(&self.recent_sends).reserve(&label, id) {
                Reservation::Fresh => {}
                Reservation::Done(hit) => {
                    audit("send_prompt", &format!("dedupe client_msg_id={id}"));
                    return ok_json(&hit);
                }
                Reservation::Pending => {
                    audit("send_prompt", &format!("in flight client_msg_id={id}"));
                    return Err(mcp_err(
                        "E_IN_FLIGHT",
                        "a send with this client_msg_id is still in progress",
                        None,
                    ));
                }
            }
        }
        // From here every exit must either complete the reservation or
        // release it: a key left `Pending` refuses the caller's own retry,
        // which is the one thing `client_msg_id` exists to allow.
        let delivered = async {
            let prompt = apply_marker(p.prompt, &marker_origin(&caller), &caller, p.raw)?;
            self.deliver_prompt(&row, prompt, p.submit, p.force).await
        }
        .await;
        let out = match delivered {
            Ok(out) => out,
            Err(e) => {
                if let Some(id) = dedupe_id.as_deref() {
                    lock_sends(&self.recent_sends).release(&label, id);
                }
                return Err(e);
            }
        };
        if let Some(id) = dedupe_id.as_deref() {
            lock_sends(&self.recent_sends).complete(&label, id, out.clone());
        }
        ok_json(&out)
    }

    #[tool(description = "Send the same prompt to every matching work session \
        (excludes the controller), skipping blocked or stuck ones unless \
        status=\"blocked\". Returns per-session results. Rate-limited \
        per caller (default one call per 30 s; E_RATE_LIMITED with \
        retry_after_secs). Marked as untrusted unless raw=true (master token \
        only). May return E_CONFIRM_REQUIRED when desktop confirmation is on.")]
    pub(super) async fn broadcast_prompt(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<BroadcastPromptParams>,
    ) -> Result<CallToolResult, McpError> {
        // Prompt body intentionally not logged.
        audit(
            "broadcast_prompt",
            &format!(
                "host={:?} project_id={:?} status={:?}",
                p.host, p.project_id, p.status
            ),
        );
        let filter_summary = broadcast_summary(
            p.host.as_deref(),
            p.project_id,
            p.status.as_deref(),
            &p.prompt,
        );
        // Confirmation first: a refused-then-approved retry must not burn the
        // caller's rate-limit slot on the initial E_CONFIRM_REQUIRED.
        self.confirm_gate(
            "broadcast_prompt",
            p.confirm_nonce.as_deref(),
            &filter_summary,
            &caller,
        )?;
        let interval = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            guard::broadcast_interval(
                s.get_setting(guard::SETTING_BROADCAST_INTERVAL)
                    .ok()
                    .flatten(),
            )
        };
        if let Err(wait) = self.guards.rate.check(&caller.label(), interval) {
            let secs = wait.as_secs().max(1);
            return Err(mcp_err(
                codes::E_RATE_LIMITED,
                format!(
                    "broadcast_prompt is limited to one call per {}s per caller; retry in {secs}s",
                    interval.as_secs()
                ),
                Some(serde_json::json!({ "retry_after_secs": secs })),
            ));
        }
        let filter = sessions::BroadcastFilter {
            host: p.host,
            project_id: p.project_id,
            status: p.status,
        };
        let submit = p.submit.unwrap_or(true);
        let prompt = apply_marker(p.prompt, &marker_origin(&caller), &caller, p.raw)?;
        let summary = sessions::broadcast_prompt(filter, prompt, submit, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json_compact(&summary)
    }

    #[tool(
        description = "Return the recorded event timeline for a session (status \
        changes, prompts, stuck, kills, and conversation events: \
        conversation_started, conversation_ended, compact_started, \
        compact_done, turn_done). Newest-first; pass `limit` to cap \
        (default 50). Returns the events as JSON. fresh_for returns only \
        what is new since your last read."
    )]
    pub(super) async fn session_history(
        &self,
        Parameters(p): Parameters<SessionHistoryParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "session_history",
            &format!("session_id={} fresh_for={:?}", p.session_id, p.fresh_for),
        );
        // ≥ 1: 0 or negative would either loop `more:true, data:[]` forever
        // (history/inbox's paging is `limit`-driven, not offset-driven) or,
        // unclamped, reach `session_events_after` as an effectively
        // unlimited SQL `LIMIT`.
        let limit = p.limit.unwrap_or(50).max(1);

        // fresh_for absent: today's default, byte-identical, no cursor
        // touched.
        let Some(reader) = p.fresh_for else {
            let events = {
                let s = lock(&self.store).map_err(to_mcp_err)?;
                s.list_session_events(p.session_id, limit)
                    .map_err(to_mcp_err)?
            };
            return ok_json_compact(&events);
        };

        let resource_key = p.session_id.to_string();
        let payload = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            let reader_exists = s
                .get_session_by_id(reader)
                .map_err(|e| to_mcp_err(e.into()))?
                .is_some();
            let stored = s
                .get_read_cursor(reader, "session_history", &resource_key)
                .map_err(to_mcp_err)?;
            let head = s.max_session_event_id(p.session_id).map_err(to_mcp_err)?;
            // No generation for this tool: pass None on both sides.
            let decision = stream_decision(reader_exists, stored.as_ref(), head, None);
            let (rows, more) = match decision {
                fresh::StreamStart::Unchanged => (Vec::new(), false),
                fresh::StreamStart::After(after) => {
                    page_events(&s, p.session_id, after, limit).map_err(to_mcp_err)?
                }
                fresh::StreamStart::Full(_) => {
                    page_events(&s, p.session_id, 0, limit).map_err(to_mcp_err)?
                }
            };
            let reset = match decision {
                fresh::StreamStart::Full(r) => r,
                _ => None,
            };
            let unchanged = matches!(decision, fresh::StreamStart::Unchanged);
            let mut data = serde_json::to_value(&rows)
                .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))?;
            crate::json::strip_nulls(&mut data);
            // The cursor advances only now that serialisation succeeded — a
            // failure building the response must not silently mark rows as
            // delivered.
            if let (Some(last), false) = (
                rows.last().map(|e| e.id),
                reset == Some(fresh::ResetReason::ReaderUnknown),
            ) {
                s.put_stream_cursor(
                    reader,
                    "session_history",
                    &resource_key,
                    Some(p.session_id),
                    last,
                    None,
                    None,
                )
                .map_err(to_mcp_err)?;
            }
            fresh::envelope(unchanged, reset, more, data)
        };
        ok_json(&payload)
    }

    #[tool(
        description = "List the Claude Code conversations a session has run, newest \
        first: claude_session_id, started_at, ended_at, start_source (startup, resume, \
        clear, compact, fork, fleet, unknown), end_reason, model, first_prompt, turns, \
        compactions and current. Pass a claude_session_id to session_conversation to \
        read an earlier one. limit defaults to 20 (max 500). Read-only. A per-host \
        token may only list sessions on its own host."
    )]
    pub(super) async fn session_conversations(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SessionConversationsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "session_conversations",
            &format!("session_id={} limit={:?}", p.session_id, p.limit),
        );
        let row =
            self.resolve_target_row(&caller, Some(p.session_id), None, None, "the session")?;
        let limit = p.limit.unwrap_or(20).clamp(1, 500);
        let rows = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            s.list_conversations(row.id, limit).map_err(to_mcp_err)?
        };
        ok_json_compact(&rows)
    }

    #[tool(description = "Send a peer-to-peer message (to_session_id or \
        to_addr) to the recipient's inbox; deliver=true also pastes it into \
        the pane, wake=true nudges an idle one instead. reply_to threads an \
        answer. Per-host token needs its own host (E_FORBIDDEN); body \
        marked untrusted unless raw=true (master only); repeat \
        client_msg_id to avoid a double send.")]
    pub(super) async fn send_message(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SendMessageParams>,
    ) -> Result<CallToolResult, McpError> {
        // Body intentionally not logged.
        audit(
            "send_message",
            &format!(
                "from={} to={} to_addr={:?} kind={:?} deliver={} wake={}",
                p.from_session_id, p.to_session_id, p.to_addr, p.kind, p.deliver, p.wake
            ),
        );
        // The sender must exist and, for a per-host caller, live on that
        // host — otherwise any agent could spoof any `from_session_id`.
        let from_host = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            s.get_session_by_id(p.from_session_id)
                .map_err(|e| to_mcp_err(IpcError::from(e)))?
                .ok_or_else(|| {
                    mcp_err(
                        "E_NOTFOUND",
                        format!("from session {} not found", p.from_session_id),
                        None,
                    )
                })?
                .host_alias
        };
        require_host(&caller, &from_host, "from_session_id")?;
        let body = apply_marker(
            p.body,
            &format!("session {} on {from_host}", p.from_session_id),
            &caller,
            p.raw,
        )?;
        let label = caller.label();
        // The key is RESERVED before the send, not written after it — see
        // the doc comment on `send_prompt`'s identical dance. Reusing
        // `recent_sends` here (rather than a second table) means the raw
        // `(caller, id)` pair is shared with `send_prompt` — a caller that
        // reused one `client_msg_id` across both tools would otherwise get
        // `send_prompt`'s cached `{ delivered, session_id, turn_seq_before
        // }` replayed as a `send_message` "success" with no inbox row ever
        // written. The `send_message:` prefix gives this tool its own slice
        // of the shared map instead; `send_prompt`'s own key stays bare so
        // its behaviour and tests are untouched.
        let dedupe_id = p.client_msg_id.clone();
        let dedupe_key = dedupe_id.as_deref().map(|id| format!("send_message:{id}"));
        if let Some(key) = dedupe_key.as_deref() {
            match lock_sends(&self.recent_sends).reserve(&label, key) {
                Reservation::Fresh => {}
                Reservation::Done(hit) => {
                    audit(
                        "send_message",
                        &format!(
                            "dedupe client_msg_id={}",
                            dedupe_id.as_deref().unwrap_or("")
                        ),
                    );
                    return ok_json(&hit);
                }
                Reservation::Pending => {
                    audit(
                        "send_message",
                        &format!(
                            "in flight client_msg_id={}",
                            dedupe_id.as_deref().unwrap_or("")
                        ),
                    );
                    return Err(mcp_err(
                        "E_IN_FLIGHT",
                        "a send with this client_msg_id is still in progress",
                        None,
                    ));
                }
            }
        }
        // From here every exit must either complete the reservation or
        // release it: a key left `Pending` refuses the caller's own retry,
        // which is the one thing `client_msg_id` exists to allow.
        let args = crate::service::messages::SendMessageArgs {
            from_session_id: p.from_session_id,
            to_session_id: p.to_session_id,
            to_addr: p.to_addr,
            body,
            kind: p.kind,
            deliver: p.deliver,
            submit: p.submit,
            reply_to: p.reply_to,
            wake: p.wake,
        };
        let sent = crate::service::messages::send_message(args, &self.store, &self.ssh).await;
        let result = match sent {
            Ok(r) => r,
            Err(e) => {
                if let Some(key) = dedupe_key.as_deref() {
                    lock_sends(&self.recent_sends).release(&label, key);
                }
                return Err(to_mcp_err(e));
            }
        };
        let value = serde_json::to_value(&result)
            .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))?;
        if let Some(key) = dedupe_key.as_deref() {
            lock_sends(&self.recent_sends).complete(&label, key, value.clone());
        }
        ok_json(&value)
    }

    #[tool(description = "Block until the next message arrives for a \
        session, or timeout_s elapses (default 120, max 600) — avoids \
        polling inbox. Returns { status: satisfied | timeout, message }. \
        Read-only.")]
    pub(super) async fn wait_for_reply(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<WaitForReplyParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "wait_for_reply",
            &format!(
                "session_id={} after_message_id={:?} timeout_s={:?}",
                p.session_id, p.after_message_id, p.timeout_s
            ),
        );
        let row =
            self.resolve_target_row(&caller, Some(p.session_id), None, None, "the session")?;
        let _permit = self.long_poll_permit(&caller, "wait_for_reply")?;
        let got = crate::service::messages::wait_for_reply(
            &self.store,
            row.id,
            p.after_message_id,
            tasks::wait_timeout(p.timeout_s),
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&serde_json::json!({
            "status": if got.is_some() { "satisfied" } else { "timeout" },
            "message": got,
        }))
    }

    #[tool(description = "Read a session's inbox — messages sent TO \
        session_id, newest-first. Slim rows by default (metadata, reply_to, \
        80-char body preview); pass summary=false for full bodies. Task \
        results arrive here as kind=task_result. mark_read \
        (default true) flips returned unread rows to read — pass false to \
        peek without consuming. A per-host token may only read inboxes of \
        sessions on its own host (E_FORBIDDEN). fresh_for returns only what \
        is new since your last read.")]
    pub(super) async fn inbox(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<InboxParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "inbox",
            &format!(
                "session_id={} unread_only={} mark_read={} summary={} fresh_for={:?}",
                p.session_id, p.unread_only, p.mark_read, p.summary, p.fresh_for
            ),
        );
        // Master skips the lookup; a per-host token is gated to its own host
        // by `require_host` inside. A paired client also lands here and, like
        // the master, carries no host binding, so the gate passes — the
        // lookup only costs it an `E_NOTFOUND` on an unknown session. Reading
        // any session's inbox is what a paired phone is for; the tools it
        // must NOT reach are gated by `enforce_admin`, not here.
        if !caller.is_master() {
            self.resolve_target_row(
                &caller,
                Some(p.session_id),
                None,
                None,
                "the inbox's session",
            )?;
        }
        // ≥ 1, matching session_history: 0 or negative would either page
        // forever (`more:true, data:[]`) or reach the store as an
        // effectively unlimited SQL `LIMIT`.
        let limit = p.limit.unwrap_or(50).max(1);

        // fresh_for absent: today's default, byte-identical, no cursor
        // touched.
        let Some(reader) = p.fresh_for else {
            let msgs = crate::service::messages::list_inbox(
                p.session_id,
                p.unread_only,
                limit,
                p.mark_read,
                &self.store,
            )
            .map_err(to_mcp_err)?;
            return if p.summary {
                let slim: Vec<InboxSummary> = msgs.into_iter().map(InboxSummary::from).collect();
                ok_json_compact(&slim)
            } else {
                ok_json_compact(&msgs)
            };
        };

        // `unread_only` is part of the resource key: a `true` cursor and a
        // `false` cursor watch different, independent sequences of "what
        // this reader has seen" — sharing one would let a `true` read
        // advance past rows a later `false` read never got to return (a
        // skip across filters), or vice versa.
        let resource_key = format!("{}:{}", p.session_id, p.unread_only);
        let payload = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            let reader_exists = s
                .get_session_by_id(reader)
                .map_err(|e| to_mcp_err(e.into()))?
                .is_some();
            let stored = s
                .get_read_cursor(reader, "inbox", &resource_key)
                .map_err(to_mcp_err)?;
            let head = s.max_inbox_id(p.session_id).map_err(to_mcp_err)?;
            // No generation for this tool: pass None on both sides.
            let decision = stream_decision(reader_exists, stored.as_ref(), head, None);
            let (rows, more) = match decision {
                fresh::StreamStart::Unchanged => (Vec::new(), false),
                fresh::StreamStart::After(after) => {
                    page_inbox(&s, p.session_id, after, p.unread_only, limit).map_err(to_mcp_err)?
                }
                fresh::StreamStart::Full(_) => {
                    page_inbox(&s, p.session_id, 0, p.unread_only, limit).map_err(to_mcp_err)?
                }
            };
            let reset = match decision {
                fresh::StreamStart::Full(r) => r,
                _ => None,
            };
            // mark_read applies to the rows actually returned — exactly as
            // the non-fresh_for path, just sourced from this page instead of
            // list_inbox's own newest-first fetch. Best-effort, matching
            // `service::messages::list_inbox`: a failure here must not fail
            // the read that already succeeded.
            if p.mark_read {
                let ids: Vec<i64> = rows
                    .iter()
                    .filter(|m| m.read_at.is_none())
                    .map(|m| m.id)
                    .collect();
                if !ids.is_empty() {
                    let _ = s.mark_messages_read(&ids, p.session_id);
                }
            }
            let last_id = rows.last().map(|m| m.id);
            let unchanged = matches!(decision, fresh::StreamStart::Unchanged);
            let mut data = if p.summary {
                let slim: Vec<InboxSummary> = rows.into_iter().map(InboxSummary::from).collect();
                serde_json::to_value(&slim)
            } else {
                serde_json::to_value(&rows)
            }
            .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))?;
            crate::json::strip_nulls(&mut data);
            // The cursor advances only now that serialisation succeeded — a
            // failure building the response must not silently mark rows as
            // delivered.
            if let (Some(last), false) = (last_id, reset == Some(fresh::ResetReason::ReaderUnknown))
            {
                s.put_stream_cursor(
                    reader,
                    "inbox",
                    &resource_key,
                    Some(p.session_id),
                    last,
                    None,
                    None,
                )
                .map_err(to_mcp_err)?;
            }
            fresh::envelope(unchanged, reset, more, data)
        };
        ok_json(&payload)
    }

    #[tool(description = "What is a peer session doing? Returns claude_status, \
        current_activity, stuck_kind, context_pct (plus host/name/status) for \
        one session. Cheap pre-check before send_message or broadcast_prompt.")]
    pub(super) async fn peer_status(
        &self,
        Parameters(p): Parameters<PeerStatusParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("peer_status", &format!("session_id={}", p.session_id));
        let status =
            crate::service::messages::peer_status(p.session_id, &self.store).map_err(to_mcp_err)?;
        ok_json(&status)
    }
}

// ---- fresh_for paging: oldest-first, `more` when truncated ------------------

/// One page of `session_history`'s timeline strictly after `after`, oldest
/// first. `more` is true when the underlying page was longer than `limit` —
/// fetched as `limit + 1` and truncated, never `LIMIT limit` alone, so a
/// truncated page is always detectable rather than silently equal to a
/// complete one.
fn page_events(
    s: &Store,
    session_id: i64,
    after: i64,
    limit: i64,
) -> Result<(Vec<crate::store::SessionEvent>, bool), IpcError> {
    let limit = limit.max(1);
    // `saturating_add`, not `+`: callers already clamp `limit` to ≥ 1, but
    // an unclamped `i64::MAX` here would panic (debug) or wrap (release)
    // instead of just capping the page at `MAX_READ_BYTES`-scale reality.
    let mut rows = s.session_events_after(session_id, after, limit.saturating_add(1))?;
    let more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    Ok((rows, more))
}

/// [`page_events`]'s `inbox` counterpart.
fn page_inbox(
    s: &Store,
    session_id: i64,
    after: i64,
    unread_only: bool,
    limit: i64,
) -> Result<(Vec<crate::store::SessionMessage>, bool), IpcError> {
    let limit = limit.max(1);
    let mut rows = s.inbox_after(session_id, after, unread_only, limit.saturating_add(1))?;
    let more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    Ok((rows, more))
}
