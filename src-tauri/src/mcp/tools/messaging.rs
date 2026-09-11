//! MCP tools: prompts, broadcasts, messages, inbox and history.

use super::*;

#[tool_router(router = messaging_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Send and SUBMIT a prompt to a running Claude \
        session's REPL (literal text, then one Enter). This is how you steer a \
        session. Set submit=false to stage text in the REPL without submitting \
        it. Address the session with session_id OR host_alias + tmux_name. The \
        first prompt to a still-unnamed session also becomes its friendly name. \
        The text is prefixed with an untrusted-content marker line unless \
        raw=true (master token only). Returns JSON { delivered, session_id, \
        turn_seq_before }: pass turn_seq_before to wait_for_session \
        { until: \"turn_gt\" } or session_transcript { since_turn } to \
        collect the reply (or use run_prompt, which does all three).")]
    pub(super) async fn send_prompt(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SendPromptParams>,
    ) -> Result<CallToolResult, McpError> {
        // Prompt body intentionally not logged.
        audit(
            "send_prompt",
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
            "the session to prompt",
        )?;
        let prompt = apply_marker(p.prompt, &marker_origin(&caller), &caller, p.raw)?;
        let out = self.deliver_prompt(&row, prompt, p.submit).await?;
        ok_json(&out)
    }

    #[tool(description = "Send the same prompt to every matching work session \
        (excludes the controller). Returns per-session results. Rate-limited \
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
            let s = self
                .store
                .lock()
                .map_err(|_| mcp_err("E_LOCK", "store mutex poisoned", None))?;
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
        ok_json(&summary)
    }

    #[tool(
        description = "Return the recorded event timeline for a session (status \
        changes, prompts, stuck, kills). Newest-first; pass `limit` to cap \
        (default 50). Returns the events as JSON."
    )]
    pub(super) async fn session_history(
        &self,
        Parameters(p): Parameters<SessionHistoryParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("session_history", &format!("session_id={}", p.session_id));
        let limit = p.limit.unwrap_or(50);
        let events = {
            let s = self
                .store
                .lock()
                .map_err(|_| to_mcp_err(IpcError::new("E_LOCK", "store mutex poisoned")))?;
            s.list_session_events(p.session_id, limit)
                .map_err(to_mcp_err)?
        };
        ok_json(&events)
    }

    #[tool(
        description = "Send a peer-to-peer message from one session to another. \
        The message is persisted to the recipient's inbox (read with `inbox`); \
        set `deliver: true` to ALSO type the message into the recipient's tmux \
        pane with a `[msg #id from name@host]:` header. The inbox row is the \
        source of truth — it lands even if the pane delivery fails. Returns \
        JSON with the new message id and the delivery outcome. Pass reply_to \
        (an inbox message id) to thread an answer. A per-host token must \
        send from a session on its own host (E_FORBIDDEN). The body is \
        prefixed with an untrusted-content marker line unless raw=true \
        (master token only)."
    )]
    pub(super) async fn send_message(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SendMessageParams>,
    ) -> Result<CallToolResult, McpError> {
        // Body intentionally not logged.
        audit(
            "send_message",
            &format!(
                "from={} to={} kind={:?} deliver={}",
                p.from_session_id, p.to_session_id, p.kind, p.deliver
            ),
        );
        // The sender must exist and, for a per-host caller, live on that
        // host — otherwise any agent could spoof any `from_session_id`.
        let from_host = {
            let s = self
                .store
                .lock()
                .map_err(|_| mcp_err("E_LOCK", "store mutex poisoned", None))?;
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
        let args = crate::service::messages::SendMessageArgs {
            from_session_id: p.from_session_id,
            to_session_id: p.to_session_id,
            body,
            kind: p.kind,
            deliver: p.deliver,
            submit: p.submit,
            reply_to: p.reply_to,
        };
        let result = crate::service::messages::send_message(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json(&result)
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
        if !caller.is_master() {
            let host = {
                let s = self
                    .store
                    .lock()
                    .map_err(|_| mcp_err("E_LOCK", "store mutex poisoned", None))?;
                s.get_session_by_id(p.session_id)
                    .map_err(|e| to_mcp_err(IpcError::from(e)))?
                    .ok_or_else(|| {
                        mcp_err(
                            "E_NOTFOUND",
                            format!("session {} not found", p.session_id),
                            None,
                        )
                    })?
                    .host_alias
            };
            require_host(&caller, &host, "the inbox's session")?;
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
