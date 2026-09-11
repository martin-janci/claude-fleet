//! Helpers shared by the MCP tool routers: error mapping, caller gates,
//! audit, response shaping, the summary types, and the private
//! `FleetTools` methods several tools call.

use super::*;

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

/// Map a backend `IpcError` to an MCP tool error, preserving the `E_*` code.
/// Structured `details` (e.g. `E_AMBIGUOUS` candidates) ride along as the
/// error's data so a caller can act on them without parsing prose.
pub(super) fn to_mcp_err(e: IpcError) -> McpError {
    let msg = match &e.details {
        Some(d) => format!("{}: {} {}", e.code, e.message, d),
        None => format!("{}: {}", e.code, e.message),
    };
    McpError::internal_error(msg, e.details)
}

/// Default `limit` for `repo_log` when the caller passes none. The Tauri UI
/// asks for more, but an MCP caller gets a token-capped page by default.
pub(super) const REPO_LOG_DEFAULT_LIMIT: u32 = 50;

/// Default `max_lines` for `capture_session`: the tail of the pane that is
/// returned when the caller does not choose a cap.
pub(super) const CAPTURE_DEFAULT_MAX_LINES: u32 = 200;

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
    McpError::internal_error(format!("{code}: {message}"), data)
}

/// The [`Caller`] the auth middleware attached to this request. The
/// streamable-HTTP transport stashes the HTTP `Parts` in the request
/// extensions; the middleware put the caller into `Parts.extensions`.
pub(super) fn caller_from_context(ctx: &RequestContext<RoleServer>) -> Option<Caller> {
    ctx.extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<Caller>().cloned())
}

/// Readonly-mode gate: pure so it can be unit-tested without a transport.
pub(super) fn enforce_mode(caller: &Caller, tool: &str) -> Result<(), McpError> {
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
/// act as / read sessions on its own host. Master callers pass.
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
/// the call. Free-text arguments are redacted by [`guard::redact_args`].
pub(super) fn persist_audit(
    store: &Mutex<Store>,
    tool: &str,
    args: Option<&JsonObject>,
    caller: &Caller,
) {
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
    let _ = s.insert_session_event(session_id, "mcp_call", Some(&detail));
}

/// Describe the origin of a delivered prompt for the untrusted-content marker.
pub(super) fn marker_origin(caller: &Caller) -> String {
    match &caller.host_alias {
        Some(h) => format!("an agent on host {h}"),
        None => "the fleet controller".to_string(),
    }
}

/// Prefix `text` with the untrusted-content marker unless the caller is the
/// master token AND asked for `raw` delivery. A per-host caller asking for
/// `raw` is refused outright (`E_FORBIDDEN`) rather than silently marked, so
/// an agent cannot believe it delivered unmarked text.
pub(super) fn apply_marker(
    text: String,
    from: &str,
    caller: &Caller,
    raw: bool,
) -> Result<String, McpError> {
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
/// `hide_host` are master-only, whatever the host token's mode.
pub(super) fn enforce_admin(caller: &Caller, tool: &str) -> Result<(), McpError> {
    if guard::is_admin_tool(tool) && !caller.is_master() {
        return Err(mcp_err(
            "E_FORBIDDEN",
            format!(
                "{tool} is a fleet-admin tool: master token only ({} refused)",
                caller.label()
            ),
            None,
        ));
    }
    Ok(())
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

/// Serialize a successful result to pretty JSON wrapped in a tool result.
pub(super) fn ok_json<T: serde::Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))?;
    Ok(CallToolResult::success(vec![text_content(json)]))
}

/// Compact JSON with all `null` fields recursively removed. Used by
/// list-style tools whose rows carry many `Option<>` columns — pretty-printing
/// plus `"field": null` repetitions blows past MCP token caps on big fleets.
/// Stripping nulls at the MCP boundary (rather than via `#[serde(skip)]` on
/// the row struct) keeps the Tauri event bus's value→null clearing intact.
pub(super) fn ok_json_compact<T: serde::Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let mut v = serde_json::to_value(value)
        .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))?;
    strip_nulls(&mut v);
    let json = serde_json::to_string(&v)
        .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))?;
    Ok(CallToolResult::success(vec![text_content(json)]))
}

pub(super) fn strip_nulls(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            map.retain(|_, val| !val.is_null());
            for val in map.values_mut() {
                strip_nulls(val);
            }
        }
        serde_json::Value::Array(arr) => {
            for val in arr.iter_mut() {
                strip_nulls(val);
            }
        }
        _ => {}
    }
}

/// A `SessionRow` augmented with the controller flag for the `list_sessions`
/// MCP output. `#[serde(flatten)]` keeps every original SessionRow field at the
/// top level, so adding `is_controller` does not break existing consumers.
#[derive(serde::Serialize)]
pub(super) struct SessionWithController {
    pub(super) is_controller: bool,
    #[serde(flatten)]
    pub(super) row: crate::store::SessionRow,
}

/// Slim row returned by `list_sessions` when `summary: true` (the default).
/// Trimmed to the fields a triage UI/agent actually needs to pick which session
/// to drill into; callers fetch full state via `peek_session` / `related_sessions`
/// or by re-calling with `summary: false`.
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
}

pub(super) const INBOX_PREVIEW_CHARS: usize = 80;

impl From<crate::store::SessionMessage> for InboxSummary {
    fn from(m: crate::store::SessionMessage) -> Self {
        let body_chars = m.body.chars().count();
        let body_preview: String = m.body.chars().take(INBOX_PREVIEW_CHARS).collect();
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
        let s = self
            .store
            .lock()
            .map_err(|_| mcp_err("E_LOCK", "store mutex poisoned", None))?;
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
            guard::needs_confirmation(tool),
            "{tool} is not in guard::CONFIRM_TOOLS"
        );
        if !self.confirm_enabled()? {
            return Ok(());
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
                "{tool} needs approval on the claude-fleet desktop (mcp.confirm_destructive is on); \
                 ask the user to approve it there, then retry with confirm_nonce={}",
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
        let s = self
            .store
            .lock()
            .map_err(|_| to_mcp_err(IpcError::lock()))?;
        resolve_and_gate(&s, caller, session_id, host_alias, tmux_name, what)
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
        let s = self
            .store
            .lock()
            .map_err(|_| to_mcp_err(IpcError::lock()))?;
        resolve_row_and_gate(&s, caller, session_id, host_alias, tmux_name, what)
    }

    /// Deliver a (already marked) prompt to a resolved session and return
    /// `{ delivered, session_id, turn_seq_before }`.
    pub(super) async fn deliver_prompt(
        &self,
        row: &crate::store::SessionRow,
        prompt: String,
        submit: bool,
    ) -> Result<serde_json::Value, McpError> {
        let args = sessions::SendPromptArgs {
            host_alias: row.host_alias.clone(),
            tmux_name: row.tmux_name.clone(),
            prompt,
            submit,
        };
        sessions::send_prompt(args, &self.store, &self.ssh)
            .await
            .map_err(to_mcp_err)?;
        Ok(serde_json::json!({
            "delivered": true,
            "session_id": row.id,
            "turn_seq_before": row.turn_seq,
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
        let claude_id = row.claude_session_id.clone().ok_or_else(|| {
            mcp_err(
                "E_INVALID_STATE",
                format!(
                    "session {} has no claude_session_id yet (not reconciled, or not a Claude session)",
                    row.id
                ),
                None,
            )
        })?;
        // Fallback cwd for sessions without a pane (bg) or whose pane
        // lookup fails: the worktree, else the project root.
        let (cwd, stored_path) = {
            let s = self
                .store
                .lock()
                .map_err(|_| to_mcp_err(IpcError::lock()))?;
            let wt = match row.worktree_id {
                Some(wid) => s.worktree_path(wid).ok().flatten(),
                None => None,
            };
            let cwd = match wt {
                Some(p) => Some(p),
                None => match row.project_id {
                    Some(pid) => s.project_base_path(pid).ok().flatten(),
                    None => None,
                },
            };
            (cwd, s.session_transcript_path(row.id).ok().flatten())
        };
        let turns = match since_turn {
            Some(t) => usize::try_from(row.turn_seq - t).unwrap_or(0).max(1),
            None => 1,
        };
        let is_bg = row.tmux_name.starts_with("bg:");
        transcript::fetch_transcript(
            transcript::TranscriptArgs {
                host_alias: row.host_alias.clone(),
                tmux_name: if is_bg {
                    None
                } else {
                    Some(row.tmux_name.clone())
                },
                transcript_path: stored_path,
                cwd,
                claude_session_id: claude_id,
                turns,
                max_chars: max_chars
                    .unwrap_or(transcript::DEFAULT_MAX_CHARS)
                    .clamp(1, transcript::MAX_MAX_CHARS),
            },
            &self.ssh,
        )
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
        let s = self
            .store
            .lock()
            .map_err(|_| to_mcp_err(IpcError::lock()))?;
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
