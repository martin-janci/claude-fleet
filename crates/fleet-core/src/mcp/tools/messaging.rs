//! MCP tools: prompts, broadcasts, messages, inbox and history.

use super::*;
use crate::ipc_error::lock;

#[tool_router(router = messaging_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Send and SUBMIT a prompt to a running Claude \
        session's REPL (pasted, then one Enter). The first prompt to a \
        still-unnamed session also becomes its friendly name. \
        Marked untrusted unless raw=true (master only) or a trusted client. \
        keys=Enter|Escape|C-c presses a key instead (unmarked). \
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
                    format!("keys must be Enter, Escape or C-c, not {k:?}"),
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
        (default 50). Returns the events as JSON."
    )]
    pub(super) async fn session_history(
        &self,
        Parameters(p): Parameters<SessionHistoryParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("session_history", &format!("session_id={}", p.session_id));
        let limit = p.limit.unwrap_or(50);
        let events = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            s.list_session_events(p.session_id, limit)
                .map_err(to_mcp_err)?
        };
        ok_json_compact(&events)
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
        the pane. reply_to threads an answer. Per-host token needs its own \
        host (E_FORBIDDEN); body marked untrusted unless raw=true (master \
        only); repeat client_msg_id to avoid a double send.")]
    pub(super) async fn send_message(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SendMessageParams>,
    ) -> Result<CallToolResult, McpError> {
        // Body intentionally not logged.
        audit(
            "send_message",
            &format!(
                "from={} to={} to_addr={:?} kind={:?} deliver={}",
                p.from_session_id, p.to_session_id, p.to_addr, p.kind, p.deliver
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
        // `recent_sends` here (rather than a second table) means a
        // `client_msg_id` a caller also happens to use for `send_prompt` is
        // in the SAME namespace (keyed by caller label + id, not by tool);
        // that is a known sharp edge of reusing the table, not something
        // introduced here.
        let dedupe_id = p.client_msg_id.clone();
        if let Some(id) = dedupe_id.as_deref() {
            match lock_sends(&self.recent_sends).reserve(&label, id) {
                Reservation::Fresh => {}
                Reservation::Done(hit) => {
                    audit("send_message", &format!("dedupe client_msg_id={id}"));
                    return ok_json(&hit);
                }
                Reservation::Pending => {
                    audit("send_message", &format!("in flight client_msg_id={id}"));
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
        };
        let sent = crate::service::messages::send_message(args, &self.store, &self.ssh).await;
        let result = match sent {
            Ok(r) => r,
            Err(e) => {
                if let Some(id) = dedupe_id.as_deref() {
                    lock_sends(&self.recent_sends).release(&label, id);
                }
                return Err(to_mcp_err(e));
            }
        };
        let value = serde_json::to_value(&result)
            .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))?;
        if let Some(id) = dedupe_id.as_deref() {
            lock_sends(&self.recent_sends).complete(&label, id, value.clone());
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
        sessions on its own host (E_FORBIDDEN).")]
    pub(super) async fn inbox(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<InboxParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "inbox",
            &format!(
                "session_id={} unread_only={} mark_read={} summary={}",
                p.session_id, p.unread_only, p.mark_read, p.summary
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
        let limit = p.limit.unwrap_or(50);
        let msgs = crate::service::messages::list_inbox(
            p.session_id,
            p.unread_only,
            limit,
            p.mark_read,
            &self.store,
        )
        .map_err(to_mcp_err)?;
        if p.summary {
            let slim: Vec<InboxSummary> = msgs.into_iter().map(InboxSummary::from).collect();
            ok_json_compact(&slim)
        } else {
            ok_json_compact(&msgs)
        }
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
