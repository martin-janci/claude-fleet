//! MCP tools: prompts, broadcasts, messages, inbox and history.

use super::*;
use crate::ipc_error::lock;

#[tool_router(router = messaging_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Send and SUBMIT a prompt to a running Claude \
        session's REPL (pasted, then one Enter); the first prompt to an \
        unnamed session also names it. Marked untrusted unless raw=true \
        (master only) or a trusted client. keys presses a key instead. \
        Returns { delivered, session_id, turn_seq_before, queued, acked }: \
        pass turn_seq_before to wait_for_session { until: \"turn_gt\" } or \
        session_transcript { since_turn } for the reply (run_prompt does all \
        three). Refuses a blocked or stuck session (E_INVALID_STATE) unless \
        force=true; a working one queues it. acked: true = hook-confirmed, \
        false = not within 1.5 s (check capture_session), null = unknowable.")]
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
            // `drive`: a pane write is exactly what a `drive` grant is for (spec
            // §4.3, invariant 5's closing paragraph). A key alone is `answer`
            // (Orbit Fleet 11.7), narrowed below for a caller who cannot drive.
            if p.keys.is_some() {
                Reach::Answer
            } else {
                Reach::Drive
            },
            "the session to prompt",
        )?;
        // The operator's text (or key) reaches the session only once a
        // person approves it (F21); for any other caller this is a no-op.
        self.confirm_gate(
            "send_prompt",
            p.confirm_nonce.as_deref(),
            &prompt_summary(&row, &p.prompt, p.keys.as_deref(), p.submit, p.force, p.raw),
            &caller,
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
            let drives = {
                let s = lock(&self.store).map_err(to_mcp_err)?;
                reaches_row(&s, &caller, &row, Reach::Drive)?
            };
            if !drives {
                // An `answer` grant: the key must answer the dialog a FRESH
                // read of the pane shows, not the row's (up to a tick old).
                // The dialog can still close between this read and the press,
                // exactly as it can for the owner's card; what the read rules
                // out is pressing into a pane that shows no dialog at all.
                answer_key_kind(key).map_err(|why| mcp_err(codes::E_FORBIDDEN, why, None))?;
                let probe = sessions::session_activity(&self.store, &self.ssh, row.id)
                    .await
                    .map_err(to_mcp_err)?;
                answer_key_allowed(key, probe.pending_input.as_ref())
                    .map_err(|why| mcp_err(codes::E_FORBIDDEN, why, None))?;
            }
            // The client's check and its press, made one step here: the pane
            // is re-read now and the key goes out right after, with no round
            // trip to the client in between.
            if let Some(expect) = &p.expect {
                sessions::check_expected_dialog(&self.store, &self.ssh, row.id, expect, key)
                    .await
                    .map_err(to_mcp_err)?;
            }
            sessions::send_keys(&row.host_alias, &row.tmux_name, key, &self.store, &self.ssh)
                .await
                .map_err(to_mcp_err)?;
            // Re-read just this pane so the answered dialog leaves the row
            // (and a following one comes up) now, not on the next tick: an
            // answer mid-turn fires no hook.
            sessions::spawn_dialog_followup(
                Arc::clone(&self.store),
                Arc::clone(&self.ssh),
                row.id,
                sessions::DialogFollowup::Answered,
            );
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
        // The key names the TARGET as well as the id: the label is shared by
        // every agent behind one host token, so a bare id reused for another
        // session answered the first call's cached result and never sent.
        // The `send_prompt:` prefix keeps it apart from `send_message`'s.
        let dedupe_id = p
            .client_msg_id
            .as_deref()
            .map(|id| format!("send_prompt:{}:{id}", row.id));
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

    #[tool(description = "Send a prompt as a new turn when the session is \
        idle: typed now if it is, else kept and typed once its turn ends \
        (never into a dialog). Send later: not_before (unix secs) holds it \
        until then, until_limit_reset while the account is at its limit, \
        skip_if_archived drops it if the session is archived first. Marked \
        untrusted unless raw=true (master only). Returns { session_id, \
        delivered, queued_id }.")]
    pub(super) async fn queue_prompt(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<QueuePromptParams>,
    ) -> Result<CallToolResult, McpError> {
        // Prompt body intentionally not logged.
        audit("queue_prompt", &format!("session_id={}", p.session_id));
        let row = self.resolve_target_row(
            &caller,
            Some(p.session_id),
            None,
            None,
            // `drive`, as `send_prompt`: it ends as a pane write.
            Reach::Drive,
            "the session to prompt",
        )?;
        // As `send_prompt`: the operator's text waits for a person (F21).
        self.confirm_gate(
            "queue_prompt",
            p.confirm_nonce.as_deref(),
            &prompt_summary(&row, &p.prompt, None, true, false, p.raw),
            &caller,
        )?;
        let prompt = apply_marker(p.prompt, &marker_origin(&caller), &caller, p.raw)?;
        let out = sessions::queue_prompt(
            sessions::QueuePromptArgs {
                session_id: row.id,
                prompt,
                not_before: p.not_before,
                until_limit_reset: p.until_limit_reset,
                skip_if_archived: p.skip_if_archived,
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map_err(to_mcp_err)?;
        ok_json(&out)
    }

    #[tool(description = "A session's prompts from queue_prompt still \
        waiting, and any whose typing failed. cancel=<id> takes one back \
        instead.")]
    pub(super) async fn queued_prompts(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<QueuedPromptsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "queued_prompts",
            &format!("session_id={} cancel={:?}", p.session_id, p.cancel),
        );
        let row = self.resolve_target_row(
            &caller,
            Some(p.session_id),
            None,
            None,
            // Taking a prompt back changes what the pane will get: `drive`.
            // A list is the session's own pending input, read at `drive`
            // too, so a watcher never reads what an owner is about to send.
            Reach::Drive,
            "the session whose prompts to list",
        )?;
        if let Some(id) = p.cancel {
            sessions::cancel_queued_prompt(
                sessions::CancelQueuedPromptArgs {
                    session_id: row.id,
                    id,
                },
                &self.store,
            )
            .map_err(to_mcp_err)?;
        }
        let rows = sessions::queued_prompts(
            sessions::QueuedPromptsArgs { session_id: row.id },
            &self.store,
        )
        .map_err(to_mcp_err)?;
        ok_json(&rows)
    }

    #[tool(description = "Send one prompt to every matching work session \
        (not the controller), skipping blocked or stuck ones unless \
        status=\"blocked\". Returns per-session results. Rate-limited per \
        caller (default one call per 30 s; E_RATE_LIMITED with \
        retry_after_secs). Marked untrusted unless raw=true (master token \
        only). May return E_CONFIRM_REQUIRED when desktop confirmation is \
        on.")]
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
        // Who is broadcasting (multi-user M1, T7). The view scope is NOT
        // optional for a caller: a fan-out reaches only the sessions this
        // caller could have prompted one at a time, and a `None` here would
        // mean "every session in the fleet", which is the shape the whole
        // milestone exists to remove. Work graph M5's separate org scope is
        // gone with T10 — `view.org` is the same answer, and `may_drive`
        // already composes it.
        let view = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            caller.view_scope(&s).map_err(to_mcp_err)?
        };
        let filter = sessions::BroadcastFilter {
            host: p.host,
            project_id: p.project_id,
            status: p.status,
            // `BroadcastFilter.view` is the whole fence THIS layer applies:
            // it composes the org boundary (`may_drive` ->
            // `sees_session_row`) with the person half, so the `scope` field
            // M5 kept beside it is gone (multi-user M1, T10).
            //
            // It is not the whole fence the broadcast applies, and this
            // comment used to claim it was. The scope judges a snapshot
            // here; `service::sessions::broadcast_prompt` re-reads each
            // target's row by id and re-asks `may_drive` immediately before
            // that target's send, because delivery is one SSH round trip per
            // session and a tmux name is reusable (the T7 review).
            view,
        };
        let submit = p.submit.unwrap_or(true);
        let prompt = apply_marker(p.prompt, &marker_origin(&caller), &caller, p.raw)?;
        let summary = sessions::broadcast_prompt(filter, prompt, submit, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        ok_json_compact(&summary)
    }

    #[tool(description = "A session's recorded event timeline, newest first: \
        status changes, prompts, stuck, kills, and conversation events \
        (conversation_started, conversation_ended, compact_started, \
        compact_done, turn_done).")]
    pub(super) async fn session_history(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SessionHistoryParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "session_history",
            &format!("session_id={} fresh_for={:?}", p.session_id, p.fresh_for),
        );
        // `watch`: a session's recorded timeline is one of the four reads
        // the spec calls the substance of a watch grant. Until multi-user
        // M1's T7 this was `require_visible_session`, whose first line
        // (`if !caller.is_scoped()`) let every paired client past without a
        // check — so a second person's phone read anybody's timeline.
        self.resolve_row_person_gated(
            &caller,
            p.session_id,
            Reach::Read,
            "the session whose history to read",
        )?;
        let limit = bounded_limit(p.limit, 50);

        // fresh_for absent: no cursor touched, and 0 => [] as always. A
        // negative or huge `limit` no longer reaches SQLite as "no limit":
        // see `bounded_limit`.
        let Some(reader) = p.fresh_for else {
            let events = {
                let s = lock(&self.store).map_err(to_mcp_err)?;
                s.list_session_events(p.session_id, limit)
                    .map_err(to_mcp_err)?
            };
            return ok_json_compact(&events);
        };

        // ≥ 1, for the fresh_for path only: 0 or negative would either loop
        // `more:true, data:[]` forever (history/inbox's paging is
        // `limit`-driven, not offset-driven) or, unclamped, reach
        // `session_events_after` as an effectively unlimited SQL `LIMIT`.
        let limit = limit.max(1);

        let resource_key = p.session_id.to_string();
        let payload = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            let reader_exists = resolve_reader(&s, &caller, reader)?;
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
                // An unknown reader gets no cursor, so paging it from id 0
                // would hand it the same oldest page with `more: true` on
                // every call — forever, for a caller following `more`. It
                // gets the DEFAULT newest-first page instead (what no
                // `fresh_for` returns), `more: false`, reason stated.
                fresh::StreamStart::Full(Some(fresh::ResetReason::ReaderUnknown)) => (
                    s.list_session_events(p.session_id, limit)
                        .map_err(to_mcp_err)?,
                    false,
                ),
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

    #[tool(description = "The Claude conversations a session has run, newest \
        first: claude_session_id, started_at, ended_at, start_source \
        (startup, resume, clear, compact, fork, fleet, unknown), end_reason, \
        model, first_prompt, turns, compactions, current. Read an earlier \
        one with session_conversation. Read-only; a per-host token only for \
        sessions on its own host.")]
    pub(super) async fn session_conversations(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<SessionConversationsParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "session_conversations",
            &format!("session_id={} limit={:?}", p.session_id, p.limit),
        );
        // `watch`: the conversation list is a read of the session's own
        // history.
        let row = self.resolve_target_row(
            &caller,
            Some(p.session_id),
            None,
            None,
            Reach::Read,
            "the session",
        )?;
        let limit = p.limit.unwrap_or(20).clamp(1, 500);
        let rows = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            s.list_conversations(row.id, limit).map_err(to_mcp_err)?
        };
        ok_json_compact(&rows)
    }

    #[tool(description = "Send a peer-to-peer message (to_session_id or \
        to_addr) to the recipient's inbox; deliver also pastes it into the \
        pane, wake nudges an idle one. reply_to threads an answer. A \
        per-host token needs its own host (E_FORBIDDEN); the body is marked \
        untrusted unless raw=true (master only).")]
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
        // The SENDER, through the same gate as the recipient (multi-user M1,
        // T7). It must exist and, for a per-host caller, live on that host —
        // otherwise any agent could spoof any `from_session_id` — and it must
        // be a session this caller may speak IN. The bare `get_session_by_id`
        // this replaces was both halves of the hole at once: an existence
        // oracle over the whole `sessions` table (an unknown id answered
        // `E_NOTFOUND` while another person's real one passed), and a way for
        // B to write an inbox row and a `"session <A's id> on <A's host>"`
        // marker line attributed to A's private session.
        //
        // `drive`, like the recipient: a proven pane `may_drive`, so the
        // in-pane agent still speaks for its own session, while a watcher is
        // refused the right to speak in the owner's name. `Reach::Read`
        // would close the oracle and leave the spoof standing.
        let from_host = self
            .resolve_target_row(
                &caller,
                Some(p.from_session_id),
                None,
                None,
                Reach::Drive,
                "from_session_id",
            )?
            .host_alias;
        // The RECIPIENT, gated here (multi-user M1, T7). Until this task the
        // only check on the far end lived inside `service/messages.rs`'s
        // `if !scope.is_all()` blocks — which never run for a person's
        // device, so `send_message { deliver: true, submit: true }` typed
        // arbitrary text into any session's pane and pressed Enter. That is
        // a watch grant silently conferring drive, by a route revision 3's
        // deny list did not list at all.
        //
        // `drive`, not `own`: `send_message { deliver, submit }` IS
        // `send_prompt` by another route, and the spec settles it in
        // invariant 5's closing paragraph ("a pane write … sits at `drive`,
        // not at `own` … refusing it to a driver while allowing
        // `send_prompt` would be theatre"). `wake` is the same pane write
        // with a fixed nudge for a body, so it takes the same level. What
        // none of them may do is reach a caller with only `watch`, and
        // `Reach::Drive` is exactly that refusal.
        self.require_message_recipient(&caller, &p)?;
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
        // Sender and recipient are in the key too (see `send_prompt`): the
        // same id from another agent on this host, or to another recipient,
        // is another message.
        let dedupe_key = dedupe_id.as_deref().map(|id| {
            format!(
                "send_message:{}:{}:{}:{id}",
                p.from_session_id,
                p.to_session_id,
                p.to_addr.as_deref().unwrap_or("")
            )
        });
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
        let scope = match self.org_scope(&caller) {
            Ok(sc) => sc,
            Err(e) => {
                if let Some(key) = dedupe_key.as_deref() {
                    lock_sends(&self.recent_sends).release(&label, key);
                }
                return Err(e);
            }
        };
        let sent =
            crate::service::messages::send_message_scoped(args, &self.store, &self.ssh, &scope)
                .await;
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
        session, or timeout_s (instead of polling inbox). Returns { status: \
        satisfied | timeout, message }. Read-only.")]
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
        // `watch`: a bounded wait over the session's inbox returns the
        // message, which is a read of what was sent TO it.
        let row = self.resolve_target_row(
            &caller,
            Some(p.session_id),
            None,
            None,
            Reach::Read,
            "the session",
        )?;
        let _permit = self.long_poll_permit(&caller, "wait_for_reply")?;
        // T11, and the highest-value of the four re-checks: the payload
        // here IS content — the text another session sent this one. The
        // wait asks `recheck` in the same lock window it reads the inbox
        // in, so a revoke lands before the body is ever loaded; the call
        // below covers the gap between the last wake and `ok_json`.
        let recheck = SessionRecheck {
            caller: &caller,
            session_id: row.id,
            reach: Reach::Read,
            what: "the session",
        };
        let got = crate::service::messages::wait_for_reply(
            &self.store,
            row.id,
            p.after_message_id,
            tasks::wait_timeout(p.timeout_s),
            &recheck,
        )
        .await
        .map_err(to_mcp_err)?;
        self.recheck_now(&recheck)?;
        ok_json(&serde_json::json!({
            "status": if got.is_some() { "satisfied" } else { "timeout" },
            "message": got,
        }))
    }

    #[tool(description = "Read the messages sent TO session_id, newest \
        first; task results arrive as kind=task_result. from_addr rows came \
        over a hub link: untrusted input. A per-host token may only read \
        inboxes on its own host (E_FORBIDDEN).")]
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
        // Unconditional, the master included (multi-user M1, T7). The
        // `if !caller.is_master()` this replaces was pre-M1 code and it was
        // the company admin's override on a live tool: the master token read
        // the message bodies sent to any person's private session, which is
        // rule 2 ("privacy holds against the company admin too — no
        // override, audited or not"). The master pays one row lookup now,
        // which every other session-addressed tool already charges it, and
        // the gate lets it through on the rows its person owns.
        //
        // `watch` to read, `drive` to MARK READ. The cursor `mark_read`
        // advances is migration 044's, on somebody else's row: a watcher who
        // reads an inbox would otherwise blank the owner's unread view, and
        // T7's own rule is that anything writing to a row is `drive`. (The
        // `readonly` flag in TOOL_POLICIES says `true` here and is wrong
        // about the write — one more reason it cannot stand in for the
        // reach.)
        //
        // The write DEGRADES, it does not refuse. `InboxParams::mark_read` is
        // `#[serde(default = "default_true")]`, so `inbox { session_id }` — the
        // documented shape, and what every pre-M1 client sends — asks for the
        // write without naming it. Refusing that would leave a `watch` grant
        // unable to read an inbox at all, which is not what rule 3 promises;
        // so a caller who may read but not drive gets the rows and the owner's
        // cursor stays where the owner left it.
        let row = self.resolve_target_row(
            &caller,
            Some(p.session_id),
            None,
            None,
            Reach::Read,
            "the inbox's session",
        )?;
        // A readonly token reads the inbox (`inbox` is a readonly tool) but
        // stamping `read_at` is a write, so it is served as a watcher is.
        let mark_read = p.mark_read && caller.mode != TokenMode::Readonly && {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            super::support::reaches_row(&s, &caller, &row, Reach::Drive)?
        };
        let limit = bounded_limit(p.limit, 50);

        // fresh_for absent: no cursor touched, and 0 => [] as always. A
        // negative or huge `limit` no longer reaches SQLite as "no limit":
        // see `bounded_limit`.
        let Some(reader) = p.fresh_for else {
            let msgs = crate::service::messages::list_inbox(
                p.session_id,
                p.unread_only,
                limit,
                mark_read,
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

        // ≥ 1, matching session_history, for the fresh_for path only: 0 or
        // negative would either page forever (`more:true, data:[]`) or
        // reach the store as an effectively unlimited SQL `LIMIT`.
        let limit = limit.max(1);

        // `unread_only` is part of the resource key: a `true` cursor and a
        // `false` cursor watch different, independent sequences of "what
        // this reader has seen" — sharing one would let a `true` read
        // advance past rows a later `false` read never got to return (a
        // skip across filters), or vice versa.
        let resource_key = format!("{}:{}", p.session_id, p.unread_only);
        let payload = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            let reader_exists = resolve_reader(&s, &caller, reader)?;
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
                // See `session_history`: an unknown reader gets the default
                // newest-first page with `more: false`, never an endless
                // oldest page.
                fresh::StreamStart::Full(Some(fresh::ResetReason::ReaderUnknown)) => (
                    s.list_inbox(p.session_id, p.unread_only, limit)
                        .map_err(to_mcp_err)?,
                    false,
                ),
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
            if mark_read {
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

    #[tool(description = "What is a peer session doing: claude_status, \
        current_activity, stuck_kind, context_pct (plus host/name/status). \
        Cheap check before send_message or broadcast_prompt.")]
    pub(super) async fn peer_status(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<PeerStatusParams>,
    ) -> Result<CallToolResult, McpError> {
        audit("peer_status", &format!("session_id={}", p.session_id));
        // It reads a LOCAL row and answers its host, tmux name, status,
        // `claude_status`, `current_activity`, `stuck_kind` and
        // `context_pct` — so it is gated like every other read of one named
        // row (multi-user M1, T9b). Its table row used to say "`session_id`
        // names a session on the PEER fleet … there is no local row to gate",
        // which `service::messages::peer_status` contradicts on its first
        // statement; nothing leaked only because `PeerStatus` happens to
        // carry the three keys `looks_like_session_row` recognises, i.e. the
        // NET was the whole of the fence.
        self.resolve_row_person_gated(&caller, p.session_id, Reach::Read, "the peer status")?;
        let scope = self.org_scope(&caller)?;
        let status = crate::service::messages::peer_status(p.session_id, &self.store, &scope)
            .map_err(to_mcp_err)?;
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

/// The most rows `session_history` / `inbox` answer in one call.
pub(super) const READ_LIMIT_MAX: i64 = 500;

/// A caller's `limit`, bounded. A negative one used to reach SQLite as `LIMIT
/// -1`, i.e. no limit at all: any reader (a readonly phone included) could
/// dump a long-lived session's whole timeline, or its whole inbox with full
/// bodies, built and serialised under the one store lock every other caller
/// waits on — and with `mark_read`, stamp all of it read at once. Negative
/// and over-large both mean "as many as allowed"; 0 still answers nothing.
pub(super) fn bounded_limit(limit: Option<i64>, default: i64) -> i64 {
    match limit.unwrap_or(default) {
        n if n < 0 => READ_LIMIT_MAX,
        n => n.min(READ_LIMIT_MAX),
    }
}

#[cfg(test)]
mod limit_tests {
    use super::*;

    #[test]
    fn a_limit_is_bounded_both_ways() {
        assert_eq!(bounded_limit(None, 50), 50);
        assert_eq!(bounded_limit(Some(0), 50), 0);
        assert_eq!(bounded_limit(Some(7), 50), 7);
        assert_eq!(bounded_limit(Some(-1), 50), READ_LIMIT_MAX);
        assert_eq!(bounded_limit(Some(i64::MIN), 50), READ_LIMIT_MAX);
        assert_eq!(bounded_limit(Some(i64::MAX), 50), READ_LIMIT_MAX);
    }
}

/// May an `answer` grantee press `key` on a pane showing `dialog` (Orbit
/// Fleet 11.7)? PURE, so the rule is tested without a pane.
///
/// Only while a dialog is on screen, and only a key that answers one: a
/// digit the dialog numbers, Enter (the highlighted option), Escape (cancel)
/// and Tab (a multi-select question's next step). Never `C-c`, which
/// interrupts the session rather than answering it.
pub(crate) fn answer_key_allowed(
    key: crate::tmux::NamedKey,
    dialog: Option<&crate::service::pane_intel::PendingInput>,
) -> Result<(), String> {
    use crate::tmux::NamedKey;
    answer_key_kind(key)?;
    let Some(dialog) = dialog else {
        return Err("an answer grant only answers a dialog, and the pane shows none".into());
    };
    match key {
        NamedKey::Digit(d) if !dialog.options.iter().any(|o| o.n == d.get()) => {
            Err(format!("the dialog on the pane has no option {}", d.get()))
        }
        _ => Ok(()),
    }
}

/// The half of [`answer_key_allowed`] that needs no pane: refused before the
/// pane is read, so a key no dialog takes costs no round trip.
fn answer_key_kind(key: crate::tmux::NamedKey) -> Result<(), String> {
    use crate::tmux::NamedKey;
    match key {
        // A dialog's keys: its options, its cursor, confirm and cancel.
        NamedKey::Enter
        | NamedKey::Escape
        | NamedKey::Tab
        | NamedKey::Up
        | NamedKey::Down
        | NamedKey::Digit(_) => Ok(()),
        // Interrupting, editing the input line or switching modes drives
        // the session; it answers nothing.
        NamedKey::CtrlC
        | NamedKey::Ctrl(_)
        | NamedKey::Left
        | NamedKey::Right
        | NamedKey::BackTab => Err(format!(
            "an answer grant presses a dialog's keys, not {}",
            key.tmux_name()
        )),
    }
}

#[cfg(test)]
mod answer_key_tests {
    use super::answer_key_allowed;
    use crate::service::pane_intel::{PendingInput, PendingOption};
    use crate::tmux::NamedKey;

    fn dialog(n: u8) -> PendingInput {
        PendingInput {
            kind: "permission".into(),
            question: Some("Do you want to proceed?".into()),
            options: (1..=n)
                .map(|i| PendingOption {
                    n: i,
                    label: format!("option {i}"),
                    selected: i == 1,
                    checked: false,
                })
                .collect(),
            multi: false,
            detail: None,
        }
    }

    fn key(s: &str) -> NamedKey {
        NamedKey::parse(s).expect("a key")
    }

    #[test]
    fn an_answer_grant_presses_the_dialogs_own_keys() {
        let d = dialog(3);
        for k in ["1", "2", "3", "Enter", "Escape", "Tab", "Up", "Down"] {
            assert_eq!(answer_key_allowed(key(k), Some(&d)), Ok(()), "{k}");
        }
    }

    #[test]
    fn an_answer_grant_presses_nothing_else() {
        let d = dialog(3);
        assert!(
            answer_key_allowed(key("4"), Some(&d)).is_err(),
            "no option 4"
        );
        assert!(
            answer_key_allowed(key("C-c"), Some(&d)).is_err(),
            "C-c interrupts"
        );
        // The phone key bar's driving keys (14.14) are not a dialog's.
        for k in ["C-r", "C-u", "Left", "Right", "BTab"] {
            assert!(answer_key_allowed(key(k), Some(&d)).is_err(), "{k}");
        }
        for k in ["1", "Enter", "Escape", "Tab"] {
            assert!(
                answer_key_allowed(key(k), None).is_err(),
                "{k} with no dialog"
            );
        }
    }
}
