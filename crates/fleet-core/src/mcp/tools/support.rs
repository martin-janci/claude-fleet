//! Helpers shared by the MCP tool routers: error mapping, caller gates,
//! audit, response shaping, the summary types, and the private
//! `FleetTools` methods several tools call.

use super::*;
use crate::ipc_error::lock;

// --- shared helpers --------------------------------------------------------

/// Emit a one-line audit record for a tool call. A remote-control surface
/// that can mutate the fleet should be traceable; this logs the tool name and
/// the identifying (non-secret) arguments. Prompt *bodies* are never logged.
/// The persisted counterpart (`session_events` kind `mcp_call`) is written
/// centrally in `ServerHandler::call_tool` — see [`persist_audit`].
pub(super) fn audit(tool: &str, detail: &str) {
    // Mutating calls are the audit trail (info); read-only calls are what
    // agents poll all the time (debug). Identifying args only, never bodies.
    if guard::is_readonly_tool(tool) {
        tracing::debug!(tool, detail, "[mcp] tool call");
    } else {
        tracing::info!(tool, detail, "[mcp] tool call");
    }
}

/// Key under which [`mcp_err`] stores the `E_*` code in `McpError::data`.
/// `ServerHandler::call_tool` reads it back to tell a tool-execution error
/// (→ `CallToolResult { is_error: true }`) from an rmcp protocol error.
const ERR_CODE_KEY: &str = "code";

/// Map a backend `IpcError` to an MCP tool error, preserving the `E_*` code.
/// Structured `details` (e.g. `E_AMBIGUOUS` candidates) ride along as the
/// error's data so a caller can act on them without parsing prose.
pub(super) fn to_mcp_err(e: IpcError) -> McpError {
    mcp_err(&e.code, e.message, e.details)
}

/// Translate a router outcome for the wire (MCP spec: tool *execution*
/// failures are a `CallToolResult` with `is_error: true`, which the client
/// shows the model as the tool's output so it can correct the call; JSON-RPC
/// errors are for *protocol* failures such as an unknown tool or malformed
/// arguments). A coded error (built by [`mcp_err`] / [`to_mcp_err`], or by
/// the readonly / admin / no-caller gates) becomes the result; anything else
/// — rmcp's own `invalid_params` — passes through unchanged.
pub(super) fn tool_error_result(e: McpError) -> Result<CallToolResult, McpError> {
    let Some(code) = e
        .data
        .as_ref()
        .and_then(|d| d.get(ERR_CODE_KEY))
        .and_then(|c| c.as_str())
        .map(str::to_string)
    else {
        return Err(e);
    };
    let details = e
        .data
        .as_ref()
        .and_then(|d| d.get("details"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    // `message` is "CODE: message[ details]"; keep the human line as-is for
    // the text block and the bare message for the structured field.
    let bare = e
        .message
        .strip_prefix(&format!("{code}: "))
        .unwrap_or(&e.message)
        .to_string();
    let bare = match &details {
        serde_json::Value::Null => bare,
        d => bare
            .strip_suffix(&format!(" {d}"))
            .unwrap_or(&bare)
            .to_string(),
    };
    let mut r = CallToolResult::error(vec![Content::text(e.message.to_string())]);
    r.structured_content = Some(serde_json::json!({
        "code": code,
        "message": bare,
        "details": details,
    }));
    Ok(r)
}

/// Default `limit` for `repo_log` when the caller passes none. The Tauri UI
/// asks for more, but an MCP caller gets a token-capped page by default.
pub(super) const REPO_LOG_DEFAULT_LIMIT: u32 = 50;

/// Default `max_lines` for `capture_session`: the tail of the pane that is
/// returned when the caller does not choose a cap.
pub(super) const CAPTURE_DEFAULT_MAX_LINES: u32 = 200;

/// Default `limit` for `list_worktrees`. A fleet accumulates worktrees far
/// faster than sessions (every branch of every project on every host), and an
/// uncapped fleet-wide list measured ~21k tokens — more than this server's
/// whole tool surface. The result carries `total`, so a caller can see it is
/// holding a page and narrow with `project_id` / `host_alias`.
pub(super) const WORKTREES_DEFAULT_LIMIT: usize = 100;

/// Keep only the last `max` lines of `text`. Returns the kept text plus the
/// total line count so the caller can say how much was dropped. `max == 0`
/// means no cap.
pub(super) fn tail_lines(text: &str, max: u32) -> (String, usize) {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    if max == 0 || total <= max as usize {
        return (text.to_string(), total);
    }
    (lines[total - max as usize..].join("\n"), total)
}

/// Render a pane capture for the caller: the last `max` lines, prefixed with
/// a truncation note when lines were dropped.
pub(super) fn capture_response(text: &str, max: u32) -> String {
    let (kept, total) = tail_lines(text, max);
    if total > kept.lines().count() {
        format!(
            "[capture_session: showing the last {} of {} lines — raise max_lines \
             (0 = no cap) to see more]\n{}",
            kept.lines().count(),
            total,
            kept
        )
    } else {
        kept
    }
}

/// Build an MCP tool error carrying an `E_*` code and optional structured data.
pub(super) fn mcp_err(
    code: &str,
    message: impl std::fmt::Display,
    data: Option<serde_json::Value>,
) -> McpError {
    let details = data.unwrap_or(serde_json::Value::Null);
    let msg = match &details {
        serde_json::Value::Null => format!("{code}: {message}"),
        d => format!("{code}: {message} {d}"),
    };
    McpError::internal_error(
        msg,
        Some(serde_json::json!({ ERR_CODE_KEY: code, "details": details })),
    )
}

/// The [`Caller`] the auth middleware attached to this request. The
/// streamable-HTTP transport stashes the HTTP `Parts` in the request
/// extensions; the middleware put the caller into `Parts.extensions`.
pub(super) fn caller_from_context(ctx: &RequestContext<RoleServer>) -> Option<Caller> {
    ctx.extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<Caller>().cloned())
}

/// Mode gate: pure so it can be unit-tested without a transport. A `Peer`
/// token (a linked hub) may call `peer_exchange` and nothing else, and
/// `peer_exchange` is reachable only by a `Peer` token — this is the one
/// place that rule is enforced for `/mcp`; `auth::refuses_peer` is the same
/// rule for `/events` and `/report`.
pub(super) fn enforce_mode(caller: &Caller, tool: &str) -> Result<(), McpError> {
    let peer_tool = tool == crate::mcp::auth::PEER_TOOL;
    if caller.mode == TokenMode::Peer && !peer_tool {
        return Err(mcp_err(
            "E_FORBIDDEN",
            format!("{tool} is not available to a hub link ({})", caller.label()),
            None,
        ));
    }
    if peer_tool && caller.mode != TokenMode::Peer {
        return Err(mcp_err(
            "E_FORBIDDEN",
            format!(
                "{tool} is for a linked hub's peer token only ({} refused)",
                caller.label()
            ),
            None,
        ));
    }
    if caller.mode == TokenMode::Readonly && !guard::is_readonly_tool(tool) {
        return Err(mcp_err(
            "E_FORBIDDEN",
            format!(
                "{tool} is not available to a readonly token ({})",
                caller.label()
            ),
            None,
        ));
    }
    Ok(())
}

/// Host-binding gate for identity-bearing tools: a per-host caller may only
/// act as / read sessions on its own host. The master token and a paired
/// client both pass — neither carries a `host_alias`, so they are unbound.
pub(super) fn require_host(
    caller: &Caller,
    session_host: &str,
    what: &str,
) -> Result<(), McpError> {
    match &caller.host_alias {
        Some(h) if h != session_host => Err(mcp_err(
            "E_FORBIDDEN",
            format!("{what} is on host {session_host}; this token is bound to {h}"),
            None,
        )),
        _ => Ok(()),
    }
}

/// A move touches two hosts, so the caller must be allowed on both: a
/// per-host token can only "move" within its own host, which `move_session`
/// refuses — in practice only the master token can move a session.
pub(super) fn require_move_hosts(
    caller: &Caller,
    source_host: &str,
    target_host: &str,
) -> Result<(), McpError> {
    require_host(caller, source_host, "the session to move")?;
    require_host(caller, target_host, "the move target host")
}

/// `resolve_session_target` + `require_host` on the RESOLVED row: the host
/// binding is checked against where the session actually lives, never
/// against the caller-supplied `host_alias` (which is optional and ignored
/// when `session_id` is given). Pure over a `&Store` so it is unit-testable.
pub(super) fn resolve_and_gate(
    s: &Store,
    caller: &Caller,
    session_id: Option<i64>,
    host_alias: Option<&str>,
    tmux_name: Option<&str>,
    what: &str,
) -> Result<(String, String), McpError> {
    let row = resolve_row_and_gate(s, caller, session_id, host_alias, tmux_name, what)?;
    Ok((row.host_alias, row.tmux_name))
}

/// [`resolve_and_gate`] returning the whole row — for tools that also need
/// the id, `turn_seq` or `claude_session_id` of the target.
pub(super) fn resolve_row_and_gate(
    s: &Store,
    caller: &Caller,
    session_id: Option<i64>,
    host_alias: Option<&str>,
    tmux_name: Option<&str>,
    what: &str,
) -> Result<crate::store::SessionRow, McpError> {
    let row = sessions::resolve_session_target(s, session_id, host_alias, tmux_name)
        .map_err(to_mcp_err)?;
    require_host(caller, &row.host_alias, what)?;
    Ok(row)
}

/// Bound confirmation summary for `set_clipboard`: host, byte count AND a
/// digest of the content, so an approval cannot be replayed with different
/// same-length text. The text itself never appears (it may be a secret).
pub(super) fn clipboard_summary(host_alias: &str, content: &str) -> String {
    format!(
        "host={host_alias} bytes={} sha={}",
        content.len(),
        guard::content_digest(content)
    )
}

/// Bound confirmation summary for `broadcast_prompt`: the filters plus a
/// digest of the prompt (never the prompt body).
pub(super) fn broadcast_summary(
    host: Option<&str>,
    project_id: Option<i64>,
    status: Option<&str>,
    prompt: &str,
) -> String {
    format!(
        "host={host:?} project_id={project_id:?} status={status:?} prompt={}",
        guard::content_digest(prompt)
    )
}

/// `run_prompt` precondition (S5): the session must be between turns.
/// `turn_seq_before` is read before delivery, so mid-turn the PREVIOUS
/// turn's Stop would satisfy the wait and hand back the old reply.
pub(super) fn run_prompt_ready(row: &crate::store::SessionRow) -> Result<(), McpError> {
    match row.claude_status.as_deref() {
        Some("idle") | Some("completed") | Some("stopped") => Ok(()),
        other => Err(mcp_err(
            "E_INVALID_STATE",
            format!(
                "session {} is {}; run_prompt needs it idle (a mid-turn Stop would return the \
                 previous reply) — wait_for_session {{ until: \"idle\" }} first",
                row.id,
                other.unwrap_or("of unknown status")
            ),
            None,
        )),
    }
}

/// How long a send waits for the REPL's `UserPromptSubmit` hook before
/// reporting `acked: false`, and how often it looks.
pub(super) const ACK_WAIT: std::time::Duration = std::time::Duration::from_millis(1_500);
pub(super) const ACK_POLL: std::time::Duration = std::time::Duration::from_millis(100);

/// May a prompt be delivered to this row now? `Err(E_INVALID_STATE)` for a
/// session that is `blocked` or has a `stuck_kind` (Enter would answer its
/// dialog) unless `force`. `Ok(queued)`: true when the session is `working`
/// and the prompt will be submitted, so Claude Code queues it behind the
/// running turn.
pub(super) fn delivery_gate(
    row: &crate::store::SessionRow,
    force: bool,
    submit: bool,
) -> Result<bool, McpError> {
    let blocked_on = match (row.claude_status.as_deref(), row.stuck_kind.as_deref()) {
        (_, Some(kind)) => Some(kind.to_string()),
        (Some("blocked"), None) => Some("a dialog".to_string()),
        _ => None,
    };
    if let (Some(what), false) = (blocked_on, force) {
        return Err(mcp_err(
            "E_INVALID_STATE",
            format!(
                "session {} is waiting on {what}; Enter would answer it — resolve it in the \
                 terminal, or pass force: true to type into it anyway",
                row.id
            ),
            None,
        ));
    }
    Ok(submit && row.claude_status.as_deref() == Some("working"))
}

/// Poll `prompt_submit_seq` until it passes `seq_before` (the REPL took the
/// prompt) or `wait` elapses. Lock, read, unlock — never across the sleep.
pub(super) async fn await_prompt_ack(
    store: &Mutex<crate::store::Store>,
    row_id: i64,
    seq_before: i64,
    wait: std::time::Duration,
) -> Result<bool, McpError> {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let seq = {
            let s = lock(store).map_err(to_mcp_err)?;
            s.prompt_ack_state(row_id)
                .map_err(|e| to_mcp_err(e.into()))?
                .map(|st| st.prompt_submit_seq)
        };
        match seq {
            None => return Ok(false),
            Some(seq) if seq > seq_before => return Ok(true),
            Some(_) => {}
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(ACK_POLL.min(deadline - now)).await;
    }
}

/// The turn number a caller waits past to collect THIS prompt's reply. A
/// prompt queued behind a running turn is answered by the turn after it;
/// one that was acked now (the `working` status was stale) is the next turn.
pub(super) fn turn_seq_before(turn_seq: i64, queued: bool, acked: Option<bool>) -> i64 {
    if queued && acked != Some(true) {
        turn_seq + 1
    } else {
        turn_seq
    }
}

/// Whether this delivery can produce a meaningful `acked` at all.
///
/// Three things make it unknowable, and none of them is a failure:
/// nothing was submitted (`submit: false` stages text, no hook fires), the
/// prompt was QUEUED behind a running turn (Claude Code fires
/// `UserPromptSubmit` when the queued prompt STARTS, which is whenever that
/// turn ends — not within [`ACK_WAIT`]), or no hook has ever reached this
/// row (an un-provisioned host: nothing will ever stamp it). Waiting in
/// those cases buys a guaranteed `false`, which reads as "the send failed".
pub(super) fn ack_knowable(submit: bool, queued: bool, hooks_seen: bool) -> bool {
    submit && !queued && hooks_seen
}

/// Whether this body is a bare Enter rather than a prompt — the one delivery
/// that walks past [`delivery_gate`].
///
/// The gate refuses a `blocked` or stuck session because Enter would ANSWER
/// its dialog. Pressing Enter is exactly what the Conversation tab's "Press
/// Enter" chip (and `stuck_kind: press_enter`) is for, so the gate's own
/// reason for refusing is the caller's reason for calling: an empty body has
/// to bypass it, or a session stuck on a Press-Enter prompt cannot be
/// unstuck from a hub client at all.
///
/// It reads the body AFTER [`apply_marker`], because that is what
/// [`FleetTools::deliver_prompt`] is handed. An empty prompt from an
/// untrusted caller arrives as the marker line and nothing else, so
/// [`guard::strip_marker`] is what makes "empty" recognisable on both paths;
/// a marked non-empty prompt keeps its body and is not affected.
///
/// Only an EMPTY body qualifies. Whitespace is text: it would be typed into
/// the REPL, so the gate still owns it.
pub(super) fn bypasses_gate(body: &str) -> bool {
    guard::strip_marker(body).is_empty()
}

/// The text a task worker receives (S8): the requester's prompt behind the
/// untrusted-content marker and closed by [`guard::UNTRUSTED_END`], THEN the
/// fleet-authored completion instruction outside that block. A master
/// `raw` dispatch has no untrusted block at all.
pub(super) fn task_delivery_body(
    prompt: &str,
    nonce: &str,
    caller: &Caller,
    raw: bool,
) -> Result<String, McpError> {
    let marked = apply_marker(
        prompt.trim_end().to_string(),
        &marker_origin(caller),
        caller,
        raw,
    )?;
    let block = if raw {
        marked
    } else {
        format!("{marked}\n{}", guard::UNTRUSTED_END)
    };
    Ok(tasks::with_instruction(&block, nonce))
}

/// Validate `set_session_tags` input: at most 16 tags, each 1–32 chars of
/// `[A-Za-z0-9_.:-]`, de-duplicated in order. Pure so it is unit-testable.
pub(super) fn normalize_tags(tags: Vec<String>) -> Result<Vec<String>, McpError> {
    if tags.len() > 16 {
        return Err(mcp_err("E_VALIDATE", "at most 16 tags per session", None));
    }
    let mut out: Vec<String> = Vec::with_capacity(tags.len());
    for t in tags {
        let t = t.trim().to_string();
        if t.is_empty() || t.chars().count() > 32 {
            return Err(mcp_err(
                "E_VALIDATE",
                format!("tag {t:?} must be 1–32 characters"),
                None,
            ));
        }
        if !t
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'))
        {
            return Err(mcp_err(
                "E_VALIDATE",
                format!("tag {t:?} may only contain letters, digits, _ . : -"),
                None,
            ));
        }
        if !out.contains(&t) {
            out.push(t);
        }
    }
    Ok(out)
}

/// Which session an audit row should attach to, resolved from the tool's
/// own addressing arguments; falls back to the registered controller (the
/// desktop / orchestrating agent). `None` when nothing resolves — the row
/// is then skipped rather than attached to the wrong session.
pub(super) fn find_audit_session(store: &Store, args: Option<&JsonObject>) -> Option<i64> {
    let by_id = |key: &str| args.and_then(|a| a.get(key)).and_then(|v| v.as_i64());
    if let Some(id) = by_id("session_id").or_else(|| by_id("from_session_id")) {
        return Some(id);
    }
    if let Some(id) = by_id("source_session_id") {
        return Some(id);
    }
    let by_str = |key: &str| {
        args.and_then(|a| a.get(key))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    if let (Some(host), Some(name)) = (
        by_str("host_alias"),
        by_str("tmux_name").or_else(|| by_str("name")),
    ) {
        if let Ok(Some(row)) = store.get_session(&name, &host) {
            return Some(row.id);
        }
    }
    let (host, name) = store.get_controller().ok().flatten()?;
    store.get_session(&name, &host).ok().flatten().map(|r| r.id)
}

/// Persist an audit row for a tool call into `session_events` (kind
/// `mcp_call`). Best-effort: every failure is swallowed so it can never block
/// the call. Free-text arguments are redacted by [`guard::redact_args`], and
/// the summary it produces is LOSSY by design: prompt and message bodies
/// never reach the row, only their length.
///
/// The whole detail — not just the summary — goes through
/// [`guard::scrub_line`], because the caller label is interpolated too and a
/// paired client's name is the one part of it this fleet did not author. A
/// line break there could otherwise forge a second audit record.
pub(super) fn persist_audit(
    store: &Mutex<Store>,
    tool: &str,
    args: Option<&JsonObject>,
    caller: &Caller,
) {
    // `peer_exchange` carries a linked hub's message bodies inside its `send`
    // array, which `redact_args` (top-level keys only) would render as raw
    // JSON onto the controller's timeline — once per long-poll. It writes no
    // row; its own `audit` log line carries counts only.
    if tool == crate::mcp::auth::PEER_TOOL {
        return;
    }
    // G1 (review): this runs BEFORE `enforce_mode` in `call_tool`, so a
    // `Peer` token's call to any OTHER tool — refused a moment later — would
    // otherwise still land on the controller's timeline: `tool` is entirely
    // peer-chosen and never truncated, and `find_audit_session` falls back
    // to the controller when the peer-supplied args name no real session. A
    // hub link may only ever reach `peer_exchange` (handled above), so any
    // other tool it names is refused and must leave no trace.
    if caller.mode == TokenMode::Peer {
        return;
    }
    let Ok(s) = store.lock() else { return };
    let Some(session_id) = find_audit_session(&s, args) else {
        return;
    };
    let summary = guard::redact_args(args);
    let detail = if summary.is_empty() {
        format!("{tool} by {}", caller.label())
    } else {
        format!("{tool} by {}: {summary}", caller.label())
    };
    // A read is written but not announced. The timeline and `session_history`
    // still carry it; what goes away is one `session:event` frame per read to
    // every connected client — measured at ~720 an hour from the desktop's
    // own conversation poll alone, each 253 B, and each one telling a
    // read-only paired phone what the operator was doing. A write keeps its
    // announcement: those are the events a client is watching for.
    let _ = if guard::READONLY_TOOLS.contains(&tool) {
        s.insert_session_event_quietly(
            session_id,
            None,
            "mcp_call",
            Some(&guard::scrub_line(&detail)),
        )
    } else {
        s.insert_session_event(session_id, "mcp_call", Some(&guard::scrub_line(&detail)))
    };
}

/// Describe the origin of a delivered prompt for the untrusted-content marker.
/// A paired client is named as such: its text is not the controller's, and
/// the receiving agent should see where it really came from.
///
/// The result is always ONE line. Client names are validated at pairing
/// (`store::validate_client_name`), but this is the last line of defence for
/// a row that predates that check: a CR/LF here would close the marker early
/// and place attacker-chosen text above a marked prompt, where the receiving
/// agent would read it as fleet's own words. Every control character goes,
/// not only CR/LF — and with them `U+2028`, `U+2029` and `U+0085`, which
/// `char::is_control` does not cover but a renderer or an LLM may well read
/// as a line break.
pub(super) fn marker_origin(caller: &Caller) -> String {
    let origin = match (&caller.host_alias, &caller.client) {
        (Some(h), _) => format!("an agent on host {h}"),
        (None, Some(c)) => format!("the paired client {}", c.name),
        (None, None) => "the fleet controller".to_string(),
    };
    origin
        .chars()
        .map(|c| {
            if crate::store::breaks_a_line(c) {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// Prefix `text` with the untrusted-content marker unless the caller is the
/// master token AND asked for `raw` delivery, or is a paired client the
/// operator has vouched for. A per-host caller asking for `raw` is refused
/// outright (`E_FORBIDDEN`) rather than silently marked, so an agent cannot
/// believe it delivered unmarked text. An ordinary paired client is not the
/// master ([`Caller::is_master`] checks `client` too), so it is refused here
/// as well: text typed on a phone reaches an agent marked — unless the
/// operator trusted that device (`client_tokens.trusted_at`,
/// [`Caller::is_trusted_client`]), in which case its words are the
/// operator's own and go through unmarked whatever `raw` says. The audit
/// row still names the client; only the receiving agent stops being told to
/// distrust it.
pub(super) fn apply_marker(
    text: String,
    from: &str,
    caller: &Caller,
    raw: bool,
) -> Result<String, McpError> {
    if caller.is_trusted_client() {
        return Ok(text);
    }
    if raw {
        if caller.is_master() {
            return Ok(text);
        }
        return Err(mcp_err(
            "E_FORBIDDEN",
            format!(
                "raw=true is reserved for the master token; {} must deliver marked text",
                caller.label()
            ),
            None,
        ));
    }
    Ok(guard::mark_untrusted(&text, from))
}

/// Fleet-admin gate: `provision_hosts` / `add_host` / `remove_host` /
/// `hide_host` / `apply_sync` / `set_secret` and the client-credential tools
/// (`pair_client` / `revoke_client` / `list_clients`) are master-only,
/// whatever the host token's mode — and whatever a paired client's mode,
/// since [`Caller::is_master`] is false for a client too.
///
/// Fails CLOSED on the tool name: refuses unless the caller is master OR the
/// tool is on [`guard::CLIENT_TOOLS`] — not merely "unless it's on
/// `guard::ADMIN_TOOLS`". `guard::ADMIN_TOOLS` and `guard::CLIENT_TOOLS`
/// partition every real router tool (enforced by the exhaustiveness test in
/// `tools::tests`), so this refuses exactly the same tools as an
/// `is_admin_tool` check for every tool that exists today; the difference is
/// a tool that exists but was never classified — that now needs the master
/// too, instead of defaulting open. The wording says which case it is: a
/// real, deliberately master-only tool gets the "fleet-admin tool" message,
/// while a name in neither list — nothing a client may ever call — gets a
/// message that does not claim it as a real admin tool.
pub(super) fn enforce_admin(caller: &Caller, tool: &str) -> Result<(), McpError> {
    if caller.is_master() || guard::is_client_tool(tool) {
        return Ok(());
    }
    let message = if guard::is_admin_tool(tool) {
        format!(
            "{tool} is a fleet-admin tool: master token only ({} refused)",
            caller.label()
        )
    } else {
        format!(
            "{tool} is not a client-callable tool ({} refused)",
            caller.label()
        )
    };
    Err(mcp_err("E_FORBIDDEN", message, None))
}

/// Substituted for an otherwise-empty text block. The Anthropic API rejects
/// empty text content outright ("text content blocks must be non-empty"), and
/// when prompt caching tags such a block the request fails harder still
/// ("cache_control cannot be set for empty text blocks"). Tool results flow
/// into the calling session's conversation as `tool_result` blocks, so an
/// empty/whitespace-only result would surface there as an empty text block and
/// poison that session's next API call. We never emit one — this sentinel keeps
/// every block non-empty.
pub(super) const EMPTY_RESULT_PLACEHOLDER: &str = "(no output)";

/// Apply `f` to every JSON text block of a result, re-serialising only the
/// blocks it changed (a non-JSON block is left alone).
pub(super) fn rewrite_json_content(
    result: &mut CallToolResult,
    mut f: impl FnMut(&mut serde_json::Value),
) {
    for c in result.content.iter_mut() {
        let Some(t) = c.as_text() else {
            continue;
        };
        let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&t.text) else {
            continue;
        };
        let before = v.clone();
        f(&mut v);
        if v != before {
            *c = text_content(v.to_string());
        }
    }
    if let Some(sc) = result.structured_content.as_mut() {
        f(sc);
    }
}

/// Fail closed: drop every work field of every session row in a result.
fn strip_all_work(result: &mut CallToolResult) {
    let nobody = crate::service::orgs::OrgScope::Host {
        alias: String::new(),
        org: None,
        isolated: Default::default(),
    };
    rewrite_json_content(result, |v| nobody.redact_json(v, &|_| Some(i64::MIN)));
}

/// Build a text content block guaranteed to be non-empty. Empty or
/// whitespace-only text is replaced with [`EMPTY_RESULT_PLACEHOLDER`]. Every
/// tool result must go through here (directly or via [`ok_json`]) so the fleet
/// never hands a Claude session an empty text block to serialize.
pub(super) fn text_content(text: impl Into<String>) -> Content {
    let text = text.into();
    if text.trim().is_empty() {
        Content::text(EMPTY_RESULT_PLACEHOLDER)
    } else {
        Content::text(text)
    }
}

/// Serialize a successful result to compact JSON wrapped in a tool result.
///
/// Compact, not pretty: a tool result is read by a model, not by a human, and
/// the indentation of `to_string_pretty` measured ~25% of the payload on this
/// API's own reports (`fleet_health`, `list_hosts`, `usage_report`) — tokens
/// spent on whitespace in every caller's context. Nulls are kept here; use
/// [`ok_json_compact`] for list/report shapes, where dropping them matters too.
pub(super) fn ok_json<T: serde::Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string(value)
        .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))?;
    Ok(CallToolResult::success(vec![text_content(json)]))
}

/// Compact JSON with all `null` fields recursively removed. Used by
/// list-style tools whose rows carry many `Option<>` columns — pretty-printing
/// plus `"field": null` repetitions blows past MCP token caps on big fleets.
/// Stripping nulls at the MCP boundary (rather than via `#[serde(skip)]` on
/// the row struct) keeps the Tauri event bus's value→null clearing intact.
pub(super) fn ok_json_compact<T: serde::Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    ok_json_compact_view(value, None)
}

/// The `serde_json::Value` [`compact_json_string`] serializes — split out so
/// a snapshot tool (`list_sessions`) can hash the exact `Value` it later
/// places in the `fresh_for` envelope's `data`, via [`snapshot_decision`],
/// rather than a separately-serialized string that could drift from it.
pub(super) fn compact_json_value<T: serde::Serialize>(
    value: &T,
    view: Option<&[&str]>,
) -> Result<serde_json::Value, McpError> {
    let mut v = serde_json::to_value(value)
        .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))?;
    if let Some(fields) = view {
        super::views::project_rows(&mut v, fields);
    }
    strip_nulls(&mut v);
    Ok(v)
}

/// The compact JSON string [`ok_json_compact_view`] returns.
pub(super) fn compact_json_string<T: serde::Serialize>(
    value: &T,
    view: Option<&[&str]>,
) -> Result<String, McpError> {
    let v = compact_json_value(value, view)?;
    serde_json::to_string(&v)
        .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))
}

/// [`ok_json_compact`] with an optional named projection applied first — the
/// one place a `view` narrows a list, because this is already where the value
/// is walked for `strip_nulls`.
///
/// `None` is the whole point of the signature: a caller that asks for no view
/// takes the identical path, so the wire stays byte-for-byte what it was
/// (`tests::a_view_is_opt_in_and_the_default_answer_is_byte_identical`).
/// The order matters too — project, THEN strip: a field the view keeps but
/// this row has no value for must vanish like every other null, not come
/// back as `"ci_status":null` for a client that has never seen one.
pub(super) fn ok_json_compact_view<T: serde::Serialize>(
    value: &T,
    view: Option<&[&str]>,
) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![text_content(
        compact_json_string(value, view)?,
    )]))
}

// One home, because the hub's `/events` broadcast strips the same rows for
// the same clients (see `crate::json`): a divergence here would mean
// `list_sessions` and the `session:updated` frame for one of its rows
// disagreeing about what a null field looks like on the wire.
pub(super) use crate::json::strip_nulls;

/// A `SessionRow` augmented with the controller flag for the `list_sessions`
/// MCP output. `#[serde(flatten)]` keeps every original SessionRow field at the
/// top level, so adding `is_controller` does not break existing consumers.
#[derive(serde::Serialize)]
pub(super) struct SessionWithController {
    pub(super) is_controller: bool,
    /// Why this session needs a person, and since when — `None` when it does
    /// not. See [`crate::service::attention`] for what counts.
    ///
    /// Computed on read and stamped here rather than stored, so there is no
    /// column that can go stale and no migration to carry. Here rather than
    /// on the row type itself, because the row is what the desktop
    /// deserialises straight back into `SessionRow`: a derived field on it
    /// would be a wire field the store cannot fill.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) needs_attention: Option<crate::service::attention::Attention>,
    #[serde(flatten)]
    pub(super) row: crate::store::SessionRow,
}

impl SessionWithController {
    pub(super) fn new(is_controller: bool, row: crate::store::SessionRow) -> Self {
        Self {
            is_controller,
            needs_attention: crate::service::attention::needs_attention(&row),
            row,
        }
    }
}

/// A paired client as the control API reports it. Built field by field from
/// [`crate::store::ClientTokenRow`] on purpose: `token_sha256` is the one
/// column that must never leave the hub, and a `#[serde(skip)]` on the row
/// would be one derive away from leaking it through some other serializer.
/// Adding a column to the row therefore cannot silently publish it here.
#[derive(serde::Serialize)]
pub(super) struct ClientSummary {
    pub(super) id: i64,
    pub(super) name: String,
    pub(super) mode: String,
    pub(super) created_at: i64,
    pub(super) last_seen_at: Option<i64>,
    pub(super) revoked_at: Option<i64>,
    pub(super) trusted_at: Option<i64>,
}

impl From<crate::store::ClientTokenRow> for ClientSummary {
    fn from(r: crate::store::ClientTokenRow) -> Self {
        Self {
            id: r.id,
            name: r.name,
            mode: r.mode,
            created_at: r.created_at,
            last_seen_at: r.last_seen_at,
            revoked_at: r.revoked_at,
            trusted_at: r.trusted_at,
        }
    }
}

/// Slim row returned by `list_sessions` when `summary: true` (the default).
/// Trimmed to the fields a triage UI/agent actually needs to pick which session
/// to drill into; callers fetch full state via `peer_status` /
/// `related_sessions` or by re-calling with `summary: false`.
#[derive(serde::Serialize)]
pub(super) struct SessionSummary {
    pub(super) id: i64,
    pub(super) host_alias: String,
    pub(super) tmux_name: String,
    pub(super) project_id: Option<i64>,
    pub(super) worktree_id: Option<i64>,
    pub(super) status: String,
    pub(super) claude_status: Option<String>,
    pub(super) stuck_kind: Option<String>,
    pub(super) lost_at: Option<i64>,
    pub(super) is_controller: bool,
    pub(super) tags: Vec<String>,
}

impl From<SessionWithController> for SessionSummary {
    fn from(s: SessionWithController) -> Self {
        Self {
            id: s.row.id,
            host_alias: s.row.host_alias,
            tmux_name: s.row.tmux_name,
            project_id: s.row.project_id,
            worktree_id: s.row.worktree_id,
            status: s.row.status,
            claude_status: s.row.claude_status,
            stuck_kind: s.row.stuck_kind,
            lost_at: s.row.lost_at,
            is_controller: s.is_controller,
            tags: s.row.tags,
        }
    }
}

/// Slim row returned by `inbox` when `summary: true` (the default). Replaces
/// the full message `body` with a length hint + 80-char preview — the bulk of
/// an inbox response is body text, and triage usually only needs metadata +
/// "is this the one I'm looking for?". Callers fetch full bodies by
/// re-calling with `summary: false` (and `mark_read: false` to keep peek
/// semantics).
#[derive(serde::Serialize)]
pub(super) struct InboxSummary {
    pub(super) id: i64,
    pub(super) from_session_id: i64,
    pub(super) to_session_id: i64,
    pub(super) kind: String,
    pub(super) sent_at: i64,
    pub(super) read_at: Option<i64>,
    pub(super) reply_to: Option<i64>,
    pub(super) body_chars: usize,
    pub(super) body_preview: String,
    /// The remote sender's address (migration 054) when `from_session_id`
    /// is `0`; absent for a local message, matching `SessionMessage`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) from_addr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) to_addr: Option<String>,
}

pub(super) const INBOX_PREVIEW_CHARS: usize = 80;

impl From<crate::store::SessionMessage> for InboxSummary {
    fn from(m: crate::store::SessionMessage) -> Self {
        let body_chars = m.body.chars().count();
        // G19 (review): a remote message's stored `body` is already wrapped
        // in the untrusted-content marker (`apply.rs`'s `mark_untrusted`) —
        // for a marker sized like a typical fleet id and address, that alone
        // can run well past `INBOX_PREVIEW_CHARS`, so an unstripped preview
        // is all marker and no message. `from_addr` is set only for a
        // remote row, and the row is still flagged foreign via that same
        // field either way, so stripping the marker here loses no signal.
        let preview_source: &str = if m.from_addr.is_some() {
            guard::strip_marker(&m.body)
        } else {
            &m.body
        };
        let body_preview: String = preview_source.chars().take(INBOX_PREVIEW_CHARS).collect();
        Self {
            id: m.id,
            from_session_id: m.from_session_id,
            to_session_id: m.to_session_id,
            kind: m.kind,
            sent_at: m.sent_at,
            read_at: m.read_at,
            reply_to: m.reply_to,
            body_chars,
            body_preview,
            from_addr: m.from_addr,
            to_addr: m.to_addr,
        }
    }
}

/// Slim row returned by `list_projects` when `summary: true` (the default).
/// Drops the bulky nested worktree array (paths can be 60+ chars each) in
/// favor of a count; callers fetch worktrees per project via
/// `list_worktrees { project_id }` or re-call with `summary: false`.
#[derive(serde::Serialize)]
pub(super) struct ProjectSummary {
    pub(super) id: i64,
    pub(super) owner: String,
    pub(super) repo: String,
    pub(super) worktree_count: usize,
    pub(super) last_session_at: Option<i64>,
}

impl From<crate::service::projects::ProjectTreeRow> for ProjectSummary {
    fn from(t: crate::service::projects::ProjectTreeRow) -> Self {
        Self {
            id: t.project.id,
            owner: t.project.owner,
            repo: t.project.repo,
            worktree_count: t.worktrees.len(),
            last_session_at: t.project.last_session_at,
        }
    }
}

/// Slim row returned by `list_worktrees` when `summary: true` (the default).
/// Drops the worktree's `path` (60+ chars each, and derivable from the
/// project) and the occupant rows in favour of a count — the one thing a
/// caller reads them for is whether the worktree is free to delete. A fleet
/// with a few hundred worktrees answered ~21k tokens before this shape
/// existed; `summary: false` still returns the full occupancy rows.
#[derive(serde::Serialize)]
pub(super) struct WorktreeSummary {
    pub(super) id: i64,
    pub(super) project_id: i64,
    pub(super) host_alias: String,
    pub(super) name: String,
    pub(super) branch: Option<String>,
    pub(super) occupants: usize,
}

impl From<crate::service::worktrees::WorktreeOccupancy> for WorktreeSummary {
    fn from(w: crate::service::worktrees::WorktreeOccupancy) -> Self {
        Self {
            id: w.worktree.id,
            project_id: w.worktree.project_id,
            host_alias: w.worktree.host_alias,
            name: w.worktree.name,
            branch: w.worktree.branch,
            occupants: w.occupants.len(),
        }
    }
}

/// The host a `usage_report` covers: a per-host caller is pinned to its own
/// host (another host is `E_FORBIDDEN`); the master token may pick any host
/// or none.
pub(super) fn usage_scope(
    caller: &Caller,
    requested: Option<&str>,
) -> Result<Option<String>, McpError> {
    if let Some(h) = requested {
        require_host(caller, h, "the requested host")?;
        crate::validate::host_alias(h).map_err(to_mcp_err)?;
    }
    Ok(caller
        .host_alias
        .clone()
        .or_else(|| requested.map(str::to_string)))
}

impl FleetTools {
    /// True when the operator turned on desktop confirmation for
    /// destructive calls (`mcp.confirm_destructive`).
    pub(super) fn confirm_enabled(&self) -> Result<bool, McpError> {
        let s = lock(&self.store).map_err(to_mcp_err)?;
        Ok(s.get_setting(guard::SETTING_CONFIRM_DESTRUCTIVE)
            .map_err(|e| to_mcp_err(IpcError::from(e)))?
            .as_deref()
            == Some("true"))
    }

    /// Confirmation gate for the destructive tools. With the toggle off this
    /// is a no-op. With it on: no nonce → mint one, notify the desktop, and
    /// return `E_CONFIRM_REQUIRED` carrying it; an approved nonce → proceed
    /// (single use); a denied one → `E_FORBIDDEN`; pending/unknown →
    /// `E_CONFIRM_REQUIRED` again (a fresh nonce for unknown).
    pub(super) fn confirm_gate(
        &self,
        tool: &str,
        nonce: Option<&str>,
        summary: &str,
        caller: &Caller,
    ) -> Result<(), McpError> {
        debug_assert!(
            guard::needs_confirmation(tool) || guard::OPERATOR_CONFIRMS.contains(&tool),
            "{tool} is not in guard::CONFIRM_TOOLS"
        );
        // The operator's starts and kills are always confirmed (D12); for
        // everyone else only the `confirm: true` tools, and only with the
        // toggle on.
        let forced = guard::operator_must_confirm(caller.is_operator(), tool);
        if !forced && (!guard::needs_confirmation(tool) || !self.confirm_enabled()?) {
            return Ok(());
        }
        if forced && !self.guards.approver {
            return Err(mcp_err(
                "E_FORBIDDEN",
                format!(
                    "{tool} from the operator needs a person to approve it, and this hub has \
                     no approver; ask the person to do it from the sidebar"
                ),
                None,
            ));
        }
        let confirms = &self.guards.confirms;
        if let Some(n) = nonce {
            match confirms.consume(n, tool, summary) {
                ConfirmState::Approved => return Ok(()),
                ConfirmState::Denied => {
                    return Err(mcp_err(
                        "E_FORBIDDEN",
                        format!("{tool} was denied on the desktop"),
                        None,
                    ))
                }
                ConfirmState::Pending => {
                    return Err(mcp_err(
                        codes::E_CONFIRM_REQUIRED,
                        format!(
                            "{tool} is awaiting approval on the desktop; retry with the same confirm_nonce once approved"
                        ),
                        Some(serde_json::json!({ "confirm_nonce": n })),
                    ))
                }
                ConfirmState::Unknown => {} // expired / replayed — issue a fresh one
            }
        }
        let req = confirms.request(tool, summary, &caller.label());
        (self.guards.notify)(&req);
        Err(mcp_err(
            codes::E_CONFIRM_REQUIRED,
            format!(
                "{tool} needs approval on the claude-fleet desktop ({}); \
                 ask the user to approve it there, then retry with confirm_nonce={}",
                if forced {
                    "the operator's starts and kills always do"
                } else {
                    "mcp.confirm_destructive is on"
                },
                req.nonce
            ),
            Some(serde_json::json!({ "confirm_nonce": req.nonce })),
        ))
    }

    /// Resolve a session-addressed tool's target (MCP-6) to the stored
    /// `(host_alias, tmux_name)` pair and apply the caller's host binding to
    /// the RESOLVED row — a per-host token cannot reach a session on another
    /// host by naming its fleet id. See `sessions::resolve_session_target`.
    pub(super) fn resolve_target(
        &self,
        caller: &Caller,
        session_id: Option<i64>,
        host_alias: Option<&str>,
        tmux_name: Option<&str>,
        what: &str,
    ) -> Result<(String, String), McpError> {
        let s = lock(&self.store).map_err(to_mcp_err)?;
        resolve_and_gate(&s, caller, session_id, host_alias, tmux_name, what)
    }

    /// Decision D7 on a session-addressed read a per-host token may make of
    /// any host's session (history, repo reads): a session of an org
    /// isolated from the caller's answers exactly as a missing one.
    pub(super) fn require_visible_session(
        &self,
        caller: &Caller,
        session_id: i64,
    ) -> Result<(), McpError> {
        if caller.host_alias.is_none() {
            return Ok(());
        }
        let s = lock(&self.store).map_err(to_mcp_err)?;
        let scope = caller.org_scope(&s).map_err(to_mcp_err)?;
        match s
            .get_session_by_id(session_id)
            .map_err(|e| to_mcp_err(IpcError::from(e)))?
        {
            Some(row) if !scope.sees_row(&row) => Err(mcp_err(
                codes::E_NOTFOUND,
                format!("session {session_id} not found"),
                None,
            )),
            _ => Ok(()),
        }
    }

    /// The org boundary's backstop over EVERY tool result a per-host token
    /// receives (work graph M5): each session row anywhere in the JSON loses
    /// the work its reader may not see (`OrgScope::redact_json`), the row's
    /// org read from the store by id, never from the payload — a projection
    /// that dropped `org_id` cannot make a row look unassigned. Fails
    /// closed: if the scope or a row's org cannot be read, every work field
    /// goes.
    pub(super) fn redact_work_for(&self, caller: &Caller, result: &mut CallToolResult) {
        let Ok(s) = self.store.lock() else {
            strip_all_work(result);
            return;
        };
        let scope = match caller.org_scope(&s) {
            Ok(sc) => sc,
            Err(_) => {
                drop(s);
                strip_all_work(result);
                return;
            }
        };
        if scope.is_all() {
            return;
        }
        // An org no scope can hold: any failed lookup reads as "not yours".
        const UNREADABLE: i64 = i64::MIN;
        let org_of = |m: &serde_json::Map<String, serde_json::Value>| -> Option<i64> {
            let own = m.get("org_id").and_then(serde_json::Value::as_i64);
            match m.get("id").and_then(serde_json::Value::as_i64) {
                // A row that is gone (a kill's answer) keeps the org it was
                // serialised with.
                Some(id) => match s.session_org(id) {
                    Ok(Some(o)) => Some(o),
                    Ok(None) => own,
                    Err(_) => Some(UNREADABLE),
                },
                None => Some(UNREADABLE),
            }
        };
        rewrite_json_content(result, |v| scope.redact_json(v, &org_of));
    }

    /// [`Self::resolve_target`] returning the whole row.
    pub(super) fn resolve_target_row(
        &self,
        caller: &Caller,
        session_id: Option<i64>,
        host_alias: Option<&str>,
        tmux_name: Option<&str>,
        what: &str,
    ) -> Result<crate::store::SessionRow, McpError> {
        let s = lock(&self.store).map_err(to_mcp_err)?;
        resolve_row_and_gate(&s, caller, session_id, host_alias, tmux_name, what)
    }

    /// Deliver a (already marked) prompt to a resolved session and return
    /// `{ delivered, session_id, turn_seq_before, queued, acked }`.
    ///
    /// `acked` is `true` once the session's `UserPromptSubmit` hook stamped
    /// the row after the send, `false` when it did not within [`ACK_WAIT`],
    /// and `null` when it cannot be known ([`ack_knowable`]).
    ///
    /// **`false` is not "the send failed".** It is "not confirmed within
    /// 1.5 s": the text is in the pane either way, and a slow hook, a busy
    /// host or a REPL that took the paste without firing all look the same
    /// from here. A caller that needs certainty reads the pane
    /// (`capture_session`); a caller that believes Enter did not land sends
    /// an EMPTY prompt, which is a bare Enter and nothing else.
    ///
    /// There is deliberately no automatic Enter retry. It existed, and it
    /// cannot be made safe: the only evidence available is a 1.5 s
    /// non-answer, and between that read and the retry the session may have
    /// opened a permission dialog — into which the retry presses Enter,
    /// choosing whatever is highlighted. A missing Enter costs a round trip;
    /// an Enter into a dialog approves something nobody approved.
    ///
    /// An EMPTY body ([`bypasses_gate`]) is that bare Enter, and it skips
    /// [`delivery_gate`] entirely — pressing Enter into a stuck session is
    /// the whole point of it, so the gate's reason for refusing is the
    /// caller's reason for calling. Nothing is typed, nothing is queued, and
    /// there is no ack to wait for. An empty body with `submit: false` is
    /// the one combination that types nothing AND presses nothing, so it is
    /// refused up front with `E_VALIDATE` instead of silently no-opping.
    pub(super) async fn deliver_prompt(
        &self,
        row: &crate::store::SessionRow,
        prompt: String,
        submit: bool,
        force: bool,
    ) -> Result<serde_json::Value, McpError> {
        let send = |prompt: String| {
            sessions::send_prompt(
                sessions::SendPromptArgs {
                    host_alias: row.host_alias.clone(),
                    tmux_name: row.tmux_name.clone(),
                    prompt,
                    submit,
                    keys: None,
                },
                &self.store,
                &self.ssh,
            )
        };
        let bare = bypasses_gate(&prompt);
        if bare && !submit {
            return Err(mcp_err(
                "E_VALIDATE",
                "nothing to deliver: an empty prompt with submit: false types nothing and presses nothing",
                None,
            ));
        }
        if bare {
            // The marker line is dropped with the rest: what goes to the pane
            // is the key press, not a sentence about where it came from.
            send(String::new()).await.map_err(to_mcp_err)?;
            return Ok(serde_json::json!({
                "delivered": true,
                "session_id": row.id,
                "turn_seq_before": row.turn_seq,
                "queued": false,
                "acked": serde_json::Value::Null,
            }));
        }
        let queued = delivery_gate(row, force, submit)?;
        let before = {
            let s = lock(&self.store).map_err(to_mcp_err)?;
            s.prompt_ack_state(row.id)
                .map_err(|e| to_mcp_err(e.into()))?
        };
        send(prompt).await.map_err(to_mcp_err)?;
        let acked = match before {
            Some(st) if ack_knowable(submit, queued, st.hooks_seen) => {
                Some(await_prompt_ack(&self.store, row.id, st.prompt_submit_seq, ACK_WAIT).await?)
            }
            _ => None,
        };
        Ok(serde_json::json!({
            "delivered": true,
            "session_id": row.id,
            "turn_seq_before": turn_seq_before(row.turn_seq, queued, acked),
            "queued": queued,
            "acked": acked,
        }))
    }

    /// Read a session's transcript (the last turn, or every turn after
    /// `since_turn`) as plain text.
    pub(super) async fn transcript_for(
        &self,
        row: &crate::store::SessionRow,
        since_turn: Option<i64>,
        max_chars: Option<usize>,
    ) -> Result<String, McpError> {
        let turns = match since_turn {
            Some(t) => usize::try_from(row.turn_seq - t).unwrap_or(0).max(1),
            None => 1,
        };
        let max_chars = max_chars
            .unwrap_or(transcript::DEFAULT_MAX_CHARS)
            .clamp(1, transcript::MAX_MAX_CHARS);
        let args =
            transcript::resolve_args(&self.store, row, turns, max_chars).map_err(to_mcp_err)?;
        transcript::fetch_transcript(args, &self.ssh)
            .await
            .map_err(to_mcp_err)
    }

    /// A long-poll permit for `caller`, or `E_RATE_LIMITED` when it already
    /// holds [`guard::MAX_LONG_POLLS_PER_CALLER`] bounded waits.
    pub(super) fn long_poll_permit(
        &self,
        caller: &Caller,
        tool: &str,
    ) -> Result<guard::LongPollPermit, McpError> {
        self.long_polls.try_acquire(&caller.label()).ok_or_else(|| {
            mcp_err(
                codes::E_RATE_LIMITED,
                format!(
                    "{tool}: {} already has {} bounded waits in flight; let one return first",
                    caller.label(),
                    guard::MAX_LONG_POLLS_PER_CALLER
                ),
                Some(serde_json::json!({ "retry_after_secs": 1 })),
            )
        })
    }

    /// A task the caller may see: master sees all; a per-host token only
    /// tasks it requested from its host or that target a worker on its
    /// host. Unknown → E_NOTFOUND; invisible → E_FORBIDDEN.
    pub(super) fn visible_task(
        &self,
        caller: &Caller,
        task_id: i64,
    ) -> Result<crate::store::TaskRow, McpError> {
        let s = lock(&self.store).map_err(to_mcp_err)?;
        let task = s
            .get_task(task_id)
            .map_err(to_mcp_err)?
            .ok_or_else(|| mcp_err("E_NOTFOUND", format!("task {task_id} not found"), None))?;
        if !tasks::task_visible_to(&s, &task, caller.host_alias.as_deref()).map_err(to_mcp_err)? {
            return Err(mcp_err(
                "E_FORBIDDEN",
                format!(
                    "task {task_id} involves no session on this token's host ({})",
                    caller.label()
                ),
                None,
            ));
        }
        Ok(task)
    }
}

// ---- smart caching (`fresh_for`) --------------------------------------------

/// The gate every `fresh_for`-aware tool applies before
/// [`fresh::decide_stream`] even sees the stored cursor: a `fresh_for` that
/// names no session cannot be trusted, so it is answered as a full read with
/// [`fresh::ResetReason::ReaderUnknown`] regardless of what the cursor says.
/// Kept pure (no store, no I/O) so it can be tested without a fixture that
/// can serve a real read.
pub(super) fn stream_decision(
    reader_exists: bool,
    stored: Option<&crate::store::CursorRow>,
    head: Option<i64>,
    generation: Option<i64>,
) -> fresh::StreamStart {
    if !reader_exists {
        return fresh::StreamStart::Full(Some(fresh::ResetReason::ReaderUnknown));
    }
    fresh::decide_stream(stored, head, generation)
}

/// [`snapshot_decision`]'s result: the envelope to return, plus the hash to
/// persist via `put_snapshot_cursor` — `None` when nothing should be
/// written (an unknown reader, or a read that came back unchanged).
pub(super) struct SnapshotDecision {
    pub(super) envelope: serde_json::Value,
    pub(super) new_hash: Option<String>,
}

/// The snapshot-tool decision `repo_diff` and `list_sessions` share: an
/// unknown reader gets the full `data` with `ReaderUnknown` stated and no
/// cursor written; a known reader whose stored hash matches gets
/// `unchanged` with no payload and nothing to write; anything else gets the
/// payload and a hash to store.
///
/// Hashes `serde_json::to_string(&data)` — the CANONICAL bytes `data` itself
/// re-serializes to (this crate builds `serde_json::Value` without the
/// `preserve_order` feature, so a `Value` object always serializes with its
/// keys in sorted order, deterministically, however it was constructed).
/// `data` is exactly the `serde_json::Value` this function places in the
/// envelope's `data` field, so "the stored hash matches" and "the bytes
/// this call would send are unchanged" are the same claim, not two
/// serializations that merely happen to agree.
pub(super) fn snapshot_decision(
    reader_exists: bool,
    stored_hash: Option<&str>,
    data: serde_json::Value,
) -> Result<SnapshotDecision, McpError> {
    let canonical = serde_json::to_string(&data)
        .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))?;
    let hash = fresh::snapshot_hash(&canonical);

    if !reader_exists {
        return Ok(SnapshotDecision {
            envelope: fresh::envelope(false, Some(fresh::ResetReason::ReaderUnknown), false, data),
            new_hash: None,
        });
    }
    if stored_hash == Some(hash.as_str()) {
        return Ok(SnapshotDecision {
            envelope: fresh::envelope(true, None, false, serde_json::Value::Null),
            new_hash: None,
        });
    }
    Ok(SnapshotDecision {
        envelope: fresh::envelope(false, None, false, data),
        new_hash: Some(hash),
    })
}

/// The continuation note `session_transcript` appends when
/// [`transcript::TranscriptDelta::more`] is set: turns remain past this
/// page's `max_chars` budget. The anchor already advanced past everything
/// in `body` (never past what was not sent), so re-reading with the SAME
/// `fresh_for` picks up exactly where this page left off — visible, not a
/// silent truncation.
pub(super) fn format_transcript_more(body: String, more: bool) -> String {
    if more {
        format!(
            "{body}\n[more: additional new turns follow — call session_transcript \
             again with the same fresh_for to continue]"
        )
    } else {
        body
    }
}

/// The `Full(reason)` text `session_transcript` returns: the raw transcript,
/// prefixed with a cursor-reset banner when `reason` is `Some` — never a
/// silent reset back to "just the last turn".
pub(super) fn format_transcript_full(raw: String, reason: Option<fresh::ResetReason>) -> String {
    match reason {
        Some(r) => format!(
            "[cursor reset: {} — earlier turns may not be shown; see session_conversations]\n{raw}",
            r.as_str()
        ),
        None => raw,
    }
}

// ---- per-call wall clock ----------------------------------------------------

/// Cap for [`guard::Deadline::LongPoll`]: tools that are themselves bounded
/// long-polls (`timeout_s` ≤ 600), so the wire cap sits above their own
/// maximum.
pub(super) const LONG_POLL_CAP: std::time::Duration = std::time::Duration::from_secs(660);

/// Cap for [`guard::Deadline::Lifecycle`]: tools that compose several SSH
/// round trips or spawn processes on a host (session lifecycle, provisioning,
/// host probes, fan-outs, reads that may page through large files).
pub(super) const LIFECYCLE_CAP: std::time::Duration = std::time::Duration::from_secs(300);

/// Cap for [`guard::Deadline::Quick`]: everything else — store reads and
/// single SSH round trips. Also the default for a tool with no
/// [`guard::TOOL_POLICIES`] row.
pub(super) const QUICK_CAP: std::time::Duration = std::time::Duration::from_secs(60);

/// Wall-clock cap for one tool call, from `tool`'s [`guard::Deadline`] class
/// in [`guard::TOOL_POLICIES`]. An unknown name gets the quick cap; the
/// exhaustiveness test in `tools::tests` guarantees every served tool has a
/// row.
pub fn tool_deadline(tool: &str) -> std::time::Duration {
    match guard::policy(tool) {
        Some(p) => {
            let work = match p.deadline {
                guard::Deadline::LongPoll => LONG_POLL_CAP,
                guard::Deadline::Lifecycle => LIFECYCLE_CAP,
                guard::Deadline::Quick => QUICK_CAP,
            };
            // A confirm-gated call may sit blocked on a human. The class cap
            // bounds the WORK; the confirmation window is time the call is
            // MEANT to spend waiting, so it is added rather than competed
            // with. Without this the nonce outlives the call waiting on it
            // and an approval arrives to an already-failed call.
            if p.confirm {
                work + guard::CONFIRM_TTL
            } else {
                work
            }
        }
        None => {
            // Reachable only for a name the router does not serve (rmcp then
            // answers "tool not found") — the exhaustiveness test keeps every
            // served tool with exactly one TOOL_POLICIES row.
            tracing::debug!(tool, "[mcp] unclassified tool name gets the quick cap");
            QUICK_CAP
        }
    }
}

/// The `E_TIMEOUT` tool result for a call that outran [`tool_deadline`].
pub(super) fn timeout_result(tool: &str, limit: std::time::Duration) -> CallToolResult {
    let secs = limit.as_secs();
    let mut r = CallToolResult::error(vec![Content::text(format!(
        "E_TIMEOUT: {tool} exceeded its {secs} s limit; the call may have partially completed"
    ))]);
    r.structured_content = Some(serde_json::json!({
        "code": "E_TIMEOUT",
        "tool": tool,
        "limit_secs": secs,
    }));
    r
}

/// Run one tool call under its wall clock. On elapse the future is dropped
/// — safe because no code path holds the store guard across an `.await`,
/// SSH children are reaped by `SshClient::run_child`'s own clock, and the
/// long-poll permit releases in `Drop` — and the caller gets
/// [`timeout_result`].
pub(super) async fn bounded<F>(
    tool: &str,
    limit: std::time::Duration,
    fut: F,
) -> Result<CallToolResult, McpError>
where
    F: std::future::Future<Output = Result<CallToolResult, McpError>>,
{
    match tokio::time::timeout(limit, fut).await {
        Ok(outcome) => outcome,
        Err(_elapsed) => {
            tracing::warn!(
                tool,
                limit_secs = limit.as_secs(),
                "[mcp] tool call timed out"
            );
            Ok(timeout_result(tool, limit))
        }
    }
}
