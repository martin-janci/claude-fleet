//! Helpers shared by the MCP tool routers: error mapping, caller gates,
//! audit, response shaping, the summary types, and the private
//! `FleetTools` methods several tools call.

use super::*;
use crate::ipc_error::lock;
// Multi-user M1 (T6/T7/T8): the one visibility rule, and the result gate's
// session half. Named rather than reached through a path at each use, because
// several helpers here mention both.
use crate::service::view_scope::{ViewScope, Visibility};

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
pub(super) const ERR_CODE_KEY: &str = "code";

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

// `capture_session`'s cap, its blank-pane text and its truncation note moved
// to the service layer, beside `capture_session_output`
// (`sessions::shape_capture`), because the desktop's routed `capture_session`
// command (multi-user M1, T13) has a standalone arm that must shape a pane
// exactly as this server does. Nothing here wraps them any more; the tool
// calls `sessions::shape_capture` and the tests that pin the shaping call
// the service functions by their full path.

/// Default `limit` for `list_worktrees`. A fleet accumulates worktrees far
/// faster than sessions (every branch of every project on every host), and an
/// uncapped fleet-wide list measured ~21k tokens — more than this server's
/// whole tool surface. The result carries `total`, so a caller can see it is
/// holding a page and narrow with `project_id` / `host_alias`.
pub(super) const WORKTREES_DEFAULT_LIMIT: usize = 100;

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
    // `fleet-updater`'s token reaches `/update/*` only, never a tool.
    if caller.mode == TokenMode::Updater {
        return Err(mcp_err(
            "E_FORBIDDEN",
            format!(
                "{tool} is not available to an updater token ({})",
                caller.label()
            ),
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
        // A named token limited to some hosts (G2.8).
        _ => match caller.api_hosts() {
            Some(hosts) if !hosts.iter().any(|h| h == session_host) => Err(mcp_err(
                "E_FORBIDDEN",
                format!(
                    "{what} is on host {session_host}; this token reaches only {}",
                    if hosts.is_empty() {
                        "no host".to_string()
                    } else {
                        hosts.join(", ")
                    }
                ),
                None,
            )),
            _ => Ok(()),
        },
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

/// How deep into a session one call reaches (multi-user M1, task T7).
///
/// **Four levels, three of them grantable.** `watch`, `answer` (Orbit Fleet
/// 11.7) and `drive` are what a grant can carry; [`Reach::Own`] is a TIER, not a third level anybody can
/// be given (spec §4.3, *What a grant may and may not do*, invariant 5).
/// **That invariant holds the one authoritative list of the operations the
/// tier covers; it is cited here and deliberately not restated** — revision 4
/// of the plan carried three copies of the list and they disagreed with each
/// other on whether a `drive` grantee may kill, restart or rename the
/// owner's session. (They may not: `drive` is "make this machine do work",
/// not "dispose of it".)
///
/// For everything that invariant does *not* name, the classification is
/// mechanical, in this order:
///
/// 1. anything the spec's `own` invariant names → [`Reach::Own`];
/// 2. anything that writes to a pane, a row, a task or a tmux server →
///    [`Reach::Drive`];
/// 3. the rest → [`Reach::Read`].
///
/// **`readonly` in [`guard::TOOL_POLICIES`] cannot stand in for this.**
/// `quick_replies` is `readonly: false` and a read, `whoami` is
/// `readonly: false` and a read, and `inbox` is `readonly: true` and writes
/// the moment it is called with `mark_read`. The two questions are "may a
/// readonly TOKEN call this at all" and "how far into somebody else's
/// session does this one call reach", and they have different answers.
///
/// The level travels in the same call that resolves the row
/// ([`resolve_row_and_gate`] and its wrappers), so a tool cannot resolve a
/// target and then forget to say what it is about to do with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reach {
    /// Read the row, its transcript, its history, its worktree — the
    /// substance of a `watch` grant.
    Read,
    /// Answer the dialog on the session's pane: one key, and only the keys
    /// `send_prompt` admits for it. The owner, a `drive` grantee and an
    /// `answer` grantee ([`crate::service::view_scope::ViewScope::may_answer`]).
    Answer,
    /// Make the session's machine do work: a pane write, a row write, a
    /// task, a tmux command. The owner and a `drive` grantee.
    Drive,
    /// The owner alone (and the hub's own readers). No grant reaches it.
    Own,
}

impl Reach {
    /// The level this reach needs, as the refusal says it to a caller who
    /// has less. Never names the owner: who a session belongs to is not
    /// something a refusal is entitled to tell the refused.
    fn needed(self) -> &'static str {
        match self {
            Reach::Read => "watch",
            Reach::Answer => "answer",
            Reach::Drive => "drive",
            Reach::Own => "ownership (no grant confers it)",
        }
    }
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
    reach: Reach,
    what: &str,
) -> Result<(String, String), McpError> {
    let row = resolve_row_and_gate(s, caller, session_id, host_alias, tmux_name, reach, what)?;
    Ok((row.host_alias, row.tmux_name))
}

/// [`resolve_and_gate`] returning the whole row — for tools that also need
/// the id, `turn_seq` or `claude_session_id` of the target.
///
/// **Choke point 2** (multi-user M1, T7): both wrappers meet here, so this
/// is where the person gate goes. A check in `resolve_and_gate` above would
/// be bypassed by every caller of `resolve_row_and_gate`, and the two sets
/// of tools are both large.
pub(super) fn resolve_row_and_gate(
    s: &Store,
    caller: &Caller,
    session_id: Option<i64>,
    host_alias: Option<&str>,
    tmux_name: Option<&str>,
    reach: Reach,
    what: &str,
) -> Result<crate::store::SessionRow, McpError> {
    let row = sessions::resolve_session_target(s, session_id, host_alias, tmux_name)
        .map_err(to_mcp_err)?;
    require_host(caller, &row.host_alias, what)?;
    require_bound_client_sees(s, caller, &row)?;
    // Unconditional SIBLING of the org gate above, never a check nested
    // inside it: `require_bound_client_sees` returns `Ok(())` on its first
    // line for a client with no `org_id`, which is exactly the shape of
    // every person's device and of every per-host token — so a person check
    // written inside it would run for nobody (spec §4.4).
    require_person_sees(s, caller, &row, reach, what)?;
    Ok(row)
}

/// The person half of choke point 2: may THIS caller see `row` at all, and
/// if so, may they reach it this far?
///
/// Two refusals, and the difference between them is the whole design:
///
/// * a row the caller cannot see answers `E_NOTFOUND`, exactly as an id that
///   does not exist — no existence oracle, the discipline
///   [`crate::service::orgs::not_found`] already applies to the org
///   boundary. A session's metadata IS its content (spec §4.3, *What counts
///   as content*), so there is no shape of another person's row an
///   out-of-scope caller may hold;
/// * a row the caller CAN see, at too low a level, answers `E_FORBIDDEN`.
///   The caller is a grantee here, so telling them their grant is narrower
///   than this call reveals nothing they were not already told when it was
///   made.
///
/// The one exception is the per-host token standing in the wrong pane, which
/// gets its own code — see [`codes::E_PANE_UNPROVEN`].
///
/// The scope is built from the SAME store handle the row came from. A scope
/// read through the other handle races a grant created between the two,
/// which is a revoked share still being served.
pub(super) fn require_person_sees(
    s: &Store,
    caller: &Caller,
    row: &crate::store::SessionRow,
    reach: Reach,
    what: &str,
) -> Result<(), McpError> {
    person_sees(s, caller, row, reach, what).map_err(to_mcp_err)
}

/// [`require_person_sees`] in the service layer's own error type.
///
/// The gate is written ONCE, here, and `require_person_sees` is this plus
/// [`to_mcp_err`] — which is lossless, so the two refusals are the same
/// code, the same words and the same details. The reason it exists is the
/// long-poll re-check (T11): `service::tasks::AccessRecheck` runs inside
/// `fleet-core`'s service layer, which deals in `IpcError` and must not
/// know what an `McpError` or a [`Caller`] is.
fn person_sees(
    s: &Store,
    caller: &Caller,
    row: &crate::store::SessionRow,
    reach: Reach,
    what: &str,
) -> Result<(), IpcError> {
    let scope = caller.view_scope(s)?;
    if !scope.sees_session_row(row).is_visible() {
        // §4.4 clause 2, as it actually fails in the field. One token
        // authenticates every Claude on a host, so the agent that reaches
        // here with a real id is normally just in a different split of the
        // same window (or its row was reconciled onto a new pane id since
        // the MCP connection was made). `E_NOTFOUND` would send it hunting
        // for a session `tmux list-sessions` shows it; this says what is
        // actually wrong, and says nothing about whose the row is.
        // Only when the pane proof is the ONE thing missing: a row the org
        // boundary itself refuses is not this caller's to be told about,
        // whichever host it sits on.
        if scope.host.as_deref() == Some(row.host_alias.as_str())
            // `sees_session_row` above has ALREADY refused: this org call
            // only decides whether the refusal may name the pane.
            && scope.org.sees_session_org_only(&row.host_alias, row.org_id)
        {
            return Err(IpcError::new(
                codes::E_PANE_UNPROVEN,
                format!(
                    "{what}: session {} is on {} but this request proves no pane of it \
                     (a host token reaches an unclaimed session on its host, and the one \
                     session whose pane its X-Fleet-Pane header names)",
                    row.id, row.host_alias
                ),
            ));
        }
        return Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("session {} not found", row.id),
        ));
    }
    let allowed = match reach {
        // Visibility IS the read level: a caller who sees the row sees its
        // content (the two-armed `Visibility` above has no middle value).
        Reach::Read => true,
        Reach::Answer => scope.may_answer(row),
        Reach::Drive => scope.may_drive(row),
        Reach::Own => scope.may_own(row),
    };
    if allowed {
        return Ok(());
    }
    Err(IpcError::new(
        codes::E_FORBIDDEN,
        format!(
            "{what}: session {} needs {} and this access does not carry it",
            row.id,
            reach.needed()
        ),
    ))
}

/// The PREDICATE half of [`require_person_sees`]: does `caller` reach `row` at
/// `reach`?
///
/// For the one caller-facing place that DEGRADES instead of refusing —
/// `inbox`'s `mark_read`, which advances the owner's unread cursor. A watcher
/// asking for the documented default (`inbox { session_id }`, with
/// `mark_read` defaulted to `true`) must get the rows their grant promises;
/// what they must not do is blank the owner's unread view. So the read is
/// served and the write is dropped, rather than the call refused for a side
/// effect the caller never asked for by name.
///
/// Everywhere else `require_person_sees` is the right shape: a refusal with
/// the words, the code and the audit row a refusal needs.
pub(super) fn reaches_row(
    s: &Store,
    caller: &Caller,
    row: &crate::store::SessionRow,
    reach: Reach,
) -> Result<bool, McpError> {
    let scope = caller.view_scope(s).map_err(to_mcp_err)?;
    if !scope.sees_session_row(row).is_visible() {
        return Ok(false);
    }
    Ok(match reach {
        Reach::Read => true,
        Reach::Answer => scope.may_answer(row),
        Reach::Drive => scope.may_drive(row),
        Reach::Own => scope.may_own(row),
    })
}

/// The long-poll re-check for a SESSION-bound wait (T11): is the caller
/// that opened this wait still allowed to see the row it is waiting on?
///
/// Handed to `service::tasks::wait_for_session*` and
/// `service::messages::wait_for_reply` as a `&dyn
/// tasks::AccessRecheck`, which they call on every wake inside the lock
/// window that reads the row, and which the tool calls once more before it
/// builds the payload.
///
/// The row is re-read **by id**, from the handle the re-check is given.
/// Never by `(host_alias, tmux_name)`: that pair is reusable — a killed
/// session's tmux name is taken by the next one on that host — so a wait
/// resolved by name could be handed a DIFFERENT session's row than the one
/// whose access was gated at the top of the call. The id is the row.
pub(super) struct SessionRecheck<'a> {
    pub caller: &'a Caller,
    pub session_id: i64,
    pub reach: Reach,
    pub what: &'a str,
}

impl tasks::AccessRecheck for SessionRecheck<'_> {
    fn check(&self, s: &Store) -> Result<(), IpcError> {
        // A row that vanished mid-wait is `E_NOTFOUND`, which is also what
        // the wait loops themselves answer for it — and the only answer
        // that is not "permission by absence of evidence".
        let row = s.get_session_by_id(self.session_id)?.ok_or_else(|| {
            IpcError::new(
                codes::E_NOTFOUND,
                format!("session {} not found", self.session_id),
            )
        })?;
        person_sees(s, self.caller, &row, self.reach, self.what)
    }
}

/// The long-poll re-check for `wait_for_task` (T11): the task's own gate,
/// [`task_visible_at`], asked again on every wake.
///
/// A task is visible when the caller sees every session it names, so a
/// share revoked on the worker (or on the requester) ends the wait before
/// `task.result` — the worker's paragraph — is returned.
pub(super) struct TaskRecheck<'a> {
    pub caller: &'a Caller,
    pub task_id: i64,
    pub reach: Reach,
}

impl tasks::AccessRecheck for TaskRecheck<'_> {
    fn check(&self, s: &Store) -> Result<(), IpcError> {
        task_visible_at(s, self.caller, self.task_id, self.reach).map(|_| ())
    }
}

/// A paired client bound to an org (work graph M14) starts sessions only
/// where they would belong to its org (or to none): a session it creates in
/// another org's project would at once be invisible to it — work it cannot
/// see, assigned where it does not belong.
pub(super) fn require_bound_client_may_create(
    s: &Store,
    caller: &Caller,
    host: &str,
    project_id: i64,
) -> Result<(), McpError> {
    if caller.client.as_ref().is_none_or(|c| c.org_id.is_none()) {
        return Ok(());
    }
    // Its org's projects and hosts — and unassigned ones only while the
    // org's `bound_sees_unassigned` is on (D31).
    let scope = caller.org_scope(s).map_err(to_mcp_err)?;
    let org = s
        .org_for_new_session(host, project_id)
        .map_err(to_mcp_err)?;
    if !scope.sees_org(org) {
        return Err(mcp_err(
            codes::E_FORBIDDEN,
            "a client bound to an org starts sessions only in its own org's projects and hosts",
            None,
        ));
    }
    Ok(())
}

/// May this caller LAND a new session in `worktree_id` (multi-user M1, T8d)?
///
/// `new_session` and `new_shell_session` are exempt from a per-row tier
/// because they act on no existing row — and the `worktree_id` they take was
/// read as "a checkout, not a session". It is both: a pane started in a
/// checkout somebody else's live session is working in shares that working
/// tree, so `new_shell_session { worktree_id: <Bob's>, start_command: "cat
/// .env; git diff" }` followed by `capture_session` on the caller's OWN row
/// read the contents of Bob's tree — past `repo_file`'s `Reach::Read`, which
/// would have refused the same bytes asked for by his session id. Neither
/// fence in front of it stops that: `require_host` is a no-op for a caller
/// with no host binding, and `require_bound_client_may_create` returns on its
/// first line for a client whose `org_id` is `None`, which is every person's
/// own device.
///
/// So the LANDING is gated, and at `Reach::Drive` rather than `Read`: the new
/// pane can write in the tree (`git reset --hard`, an editor, a build), which
/// is more than reading one file out of it, and `Drive` is the level the same
/// crate already requires to make somebody's machine do work. `own` would be
/// wrong in the other direction — nothing of the existing session is
/// destroyed, relocated or re-shared.
///
/// `new_worktree` passes through untouched: a tree that does not exist yet has
/// no occupants, and `worktree_id` is `None` on that path.
///
/// The occupants are the ones `delete_worktree` gates
/// ([`crate::store::Store::occupant_session_ids_for_worktree`]) — the running
/// rows plus the rows a host LOST while pointing there, because a reboot
/// leaves the checkout and its uncommitted work exactly where they were.
///
/// The refusal names no session: the caller addressed a worktree, and a
/// sentence naming the occupant would make this an oracle for whose sessions
/// live where (the same reason `delete_worktree`'s refusal is the worktree's).
/// It is [`crate::service::sessions::LANDING_NOT_YOURS`], the same sentence
/// the service-layer gate uses — `work_link { start }` resolves its
/// `worktree_id` inside `tickets::plan_resolved` and takes
/// [`crate::service::sessions::require_may_land_in_worktree`] there, which is
/// the same rule by the same words (T9b).
/// `reach` is the caller's argument rather than a constant here, for the
/// reason [`FleetTools::gate_restore_plan`] takes its own: the literal then
/// sits in the HANDLER, where
/// `tests::every_session_addressed_tool_declares_its_reach` reads it, so
/// `new_session` and `new_shell_session` can carry a row in `SESSION_REACH`
/// instead of an exemption whose sentence would have to argue they gate
/// nothing.
pub(super) fn require_may_land_in_worktree(
    store: &Mutex<Store>,
    caller: &Caller,
    worktree_id: Option<i64>,
    reach: Reach,
) -> Result<(), McpError> {
    let Some(wid) = worktree_id else {
        return Ok(());
    };
    let occupants: Vec<i64> = {
        let s = lock(store).map_err(to_mcp_err)?;
        s.occupant_session_ids_for_worktree(wid)
            .map_err(|e| to_mcp_err(crate::ipc_error::IpcError::from(e)))?
    };
    for sid in occupants {
        let s = lock(store).map_err(to_mcp_err)?;
        let Ok(row) = crate::service::sessions::resolve_session_target(&s, Some(sid), None, None)
        else {
            // Gone between the two reads: nothing of anybody's to share.
            continue;
        };
        if !reaches_row(&s, caller, &row, reach)? {
            return Err(mcp_err(
                codes::E_FORBIDDEN,
                crate::service::sessions::LANDING_NOT_YOURS,
                None,
            ));
        }
    }
    Ok(())
}

/// A paired client bound to an org (work graph M14) acts on a host with no
/// session in play (`add_project`, `list_github_repos`) only when it may see
/// that host's org: its own, or none while its org's `bound_sees_unassigned`
/// is on (D31). Everyone else passes; a per-host token's own rule is
/// [`require_host`].
pub(super) fn require_bound_client_sees_host(
    s: &Store,
    caller: &Caller,
    host: &str,
) -> Result<(), McpError> {
    if caller.client.as_ref().is_none_or(|c| c.org_id.is_none()) {
        return Ok(());
    }
    let scope = caller.org_scope(s).map_err(to_mcp_err)?;
    if !scope.sees_org(s.host_org(host).map_err(to_mcp_err)?) {
        return Err(mcp_err(
            codes::E_FORBIDDEN,
            format!("host {host} is outside this client's org"),
            None,
        ));
    }
    Ok(())
}

/// A paired client bound to an org (work graph M14) reaches only its org's
/// and unassigned sessions — to read, prompt, kill or link them. Another
/// org's session answers exactly as one that does not exist. A per-host
/// token's own rule (its host, D7) is [`require_host`]; the PERSON half of
/// the same question — ownership, grants and §4.4's pane proof — is
/// [`require_person_sees`], which runs beside this, never inside it.
pub(super) fn require_bound_client_sees(
    s: &Store,
    caller: &Caller,
    row: &crate::store::SessionRow,
) -> Result<(), McpError> {
    if caller.client.as_ref().is_none_or(|c| c.org_id.is_none()) {
        return Ok(());
    }
    let scope = caller.org_scope(s).map_err(to_mcp_err)?;
    if scope.sees_row_org_only(row) {
        Ok(())
    } else {
        Err(mcp_err(
            codes::E_NOTFOUND,
            format!("session {} not found", row.id),
            None,
        ))
    }
}

/// Characters of a free-text argument shown readably in a confirm summary.
pub(super) const BOUND_TEXT_PREFIX: usize = 40;

/// A free-text argument bound into a confirm summary (work graph M9.7): a
/// readable, escaped prefix so the person approving sees what it is, and a
/// digest of the whole so a different text with the same prefix cannot
/// reuse the approval. `-` when absent.
pub(super) fn bound_text(text: Option<&str>) -> String {
    let Some(t) = text else {
        return "-".into();
    };
    let head: String = t.chars().take(BOUND_TEXT_PREFIX).collect();
    let more = if head.len() < t.len() { "…" } else { "" };
    format!("{head:?}{more}#{}", guard::content_digest(t))
}

/// A body (a prompt, a brief) bound into a confirm summary: its size and a
/// digest, never the text — the same rule as [`broadcast_summary`].
pub(super) fn bound_body(text: Option<&str>) -> String {
    match text {
        None => "-".into(),
        Some(t) => format!("bytes={}#{}", t.len(), guard::content_digest(t)),
    }
}

/// Bound confirmation summary for `new_session`: EVERY argument, so an
/// approval for one start cannot be replayed with another (say `kind:
/// "shell"` plus a `start_command`).
pub(super) fn new_session_summary(p: &NewSessionParams) -> String {
    format!(
        "host={} name={} project_id={} worktree_id={:?} new_worktree={} base_branch={} \
         kind={} start_command={} friendly_name={} resume_claude_session_id={} model={} effort={} profile={} agent={}",
        bound_text(Some(&p.host_alias)),
        bound_text(Some(&p.name)),
        p.project_id,
        p.worktree_id,
        bound_text(p.new_worktree.as_deref()),
        bound_text(p.base_branch.as_deref()),
        bound_text(p.kind.as_deref()),
        bound_text(p.start_command.as_deref()),
        bound_text(p.friendly_name.as_deref()),
        bound_text(p.resume_claude_session_id.as_deref()),
        bound_text(p.model.as_deref()),
        bound_text(p.effort.as_deref()),
        bound_text(p.profile.as_deref()),
        bound_text(p.agent.as_deref()),
    )
}

/// Bound confirmation summary for `new_shell_session`: every argument.
pub(super) fn new_shell_session_summary(p: &NewShellSessionParams) -> String {
    format!(
        "host={} name={} project_id={} worktree_id={:?} new_worktree={} base_branch={} \
         kind=\"shell\" start_command={}",
        bound_text(Some(&p.host_alias)),
        bound_text(Some(&p.name)),
        p.project_id,
        p.worktree_id,
        bound_text(p.new_worktree.as_deref()),
        bound_text(p.base_branch.as_deref()),
        bound_text(p.start_command.as_deref()),
    )
}

/// Bound confirmation summary for `new_bg_session`: every argument, the
/// prompt as a digest.
pub(super) fn new_bg_session_summary(a: &crate::service::bg_sessions::NewBgSessionArgs) -> String {
    format!(
        "host={} name={} requester_session_id={:?} project_id={:?} agent={} read_only={} \
         stop_after_secs={:?} stop_after_usd={:?} prompt={}",
        bound_text(Some(&a.host_alias)),
        bound_text(Some(&a.name)),
        a.requester_session_id,
        a.project_id,
        bound_text(a.agent.as_deref()),
        a.read_only,
        a.stop_after_secs,
        a.stop_after_usd,
        bound_body(Some(&a.prompt)),
    )
}

/// Bound confirmation summary for `dispatch_task` with `new_worker`: the
/// worker spec, the requester, `raw`, and the prompt as a digest.
pub(super) fn dispatch_new_worker_summary(
    spec: &NewWorkerSpec,
    requester_session_id: Option<i64>,
    raw: bool,
    prompt: &str,
) -> String {
    format!(
        "new_worker host={} project_id={} name={} requester_session_id={:?} raw={} prompt={}",
        bound_text(Some(&spec.host_alias)),
        spec.project_id,
        bound_text(spec.name.as_deref()),
        requester_session_id,
        raw,
        bound_body(Some(prompt)),
    )
}

/// Bound confirmation summary for a prompt or key the operator types into a
/// session (`send_prompt`, `run_prompt`, `queue_prompt`, `dispatch_task` to
/// an existing worker): the target and every argument that changes what
/// lands in the pane, the text as a readable prefix plus a digest of the
/// whole, so the person sees the brief and an approval for one text cannot
/// send another.
pub(super) fn prompt_summary(
    row: &crate::store::SessionRow,
    prompt: &str,
    keys: Option<&str>,
    submit: bool,
    force: bool,
    raw: bool,
) -> String {
    format!(
        "session={} host={} name={} keys={} submit={submit} force={force} raw={raw} prompt={}",
        row.id,
        bound_text(Some(&row.host_alias)),
        bound_text(Some(&row.tmux_name)),
        bound_text(keys),
        bound_text(Some(prompt)),
    )
}

/// Bound confirmation summary for `work_link { start | resume }`: every
/// argument either action reads, the brief as a digest. `repos` names the
/// `project_ids` (`id:owner/repo`), when they are known.
pub(super) fn work_link_start_summary(
    a: &crate::service::work::WorkLinkArgs,
    repos: &[String],
) -> String {
    format!(
        "{} key={} url={} item_id={:?} link_id={:?} host={} project_id={:?} project_ids={:?} \
         repos=[{}] mode={} name={} worktree={} with_brief={:?} brief={} force_cross_org={:?} \
         role={} parallel={:?}",
        a.action,
        bound_text(a.key.as_deref()),
        bound_text(a.url.as_deref()),
        a.item_id,
        a.link_id,
        bound_text(a.host_alias.as_deref()),
        a.project_id,
        a.project_ids,
        repos.join(", "),
        bound_text(a.mode.as_deref()),
        bound_text(a.name.as_deref()),
        bound_text(a.worktree.as_deref()),
        a.with_brief,
        bound_body(a.brief.as_deref()),
        a.force_cross_org,
        bound_text(a.role.as_deref()),
        a.parallel,
    )
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
/// turn's Stop would satisfy the wait and hand back the old reply. A
/// `failed` turn (a StopFailure) has ended too, and re-prompting is what its
/// attention reason asks for — the same set `wait_for_session { until:
/// "idle" }` accepts, so following this error's advice cannot loop.
/// `live` is the pane's reading, taken only for a stale-demoted row (its
/// `stale_demoted_at`; `store::trusted_status`): its stored `idle` alone is
/// not enough.
pub(super) fn run_prompt_ready(
    row: &crate::store::SessionRow,
    live: Option<&str>,
) -> Result<(), McpError> {
    match crate::store::trusted_status(row, live) {
        s if crate::store::turn_over(s) => Ok(()),
        None if crate::store::needs_pane_confirmation(row) => Err(mcp_err(
            "E_INVALID_STATE",
            format!(
                "session {} was demoted from working after a quiet spell and its pane could \
                 not confirm the turn is over (a long tool call looks the same) — \
                 wait_for_session {{ until: \"idle\" }} first",
                row.id
            ),
            None,
        )),
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
/// `[A-Za-z0-9_.:-]`, de-duplicated in order. The rule lives in the service
/// (`sessions::normalize_session_tags`), which the desktop's Label shares.
pub(super) fn normalize_tags(tags: Vec<String>) -> Result<Vec<String>, McpError> {
    crate::service::sessions::normalize_session_tags(tags).map_err(to_mcp_err)
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
    if caller.mode.is_single_purpose() {
        return;
    }
    // Task 2: a read-only tool (`guard::READONLY_TOOLS` — the exact set a
    // `readonly` token may call) writes NO audit row at all, not even a
    // quiet one. The tracing `audit()` line above (called separately, from
    // each tool body) already covers every call including reads; this is
    // only the persisted `session_events` row. A poll like the desktop's
    // conversation refresh alone produced ~720 of these an hour, each one
    // an INSERT+prune under the store mutex for a call that changed
    // nothing. This check runs BEFORE the store is even locked, so a read
    // never pays for the lock either. A refused call to a MUTATING tool is
    // still audited exactly as before: this only ever short-circuits reads,
    // and `enforce_mode` / `enforce_admin` (which decide "refused") run
    // after `persist_audit` returns in `call_tool` — see "Audit first so
    // refused calls are on the timeline too" there.
    if guard::is_readonly_tool(tool) {
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
    let _ = s.insert_session_event(session_id, "mcp_call", Some(&guard::scrub_line(&detail)));
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
        (None, None) => match &caller.api {
            Some(a) => format!("the Control API token {}", a.name),
            None => "the fleet controller".to_string(),
        },
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
    if caller.host_alias.is_some() && guard::NOT_FOR_HOST_TOKENS.contains(&tool) {
        return Err(mcp_err(
            "E_FORBIDDEN",
            format!(
                "{tool} is never a per-host token's ({} refused)",
                caller.label()
            ),
            None,
        ));
    }
    if guard::access_allows(caller, tool) {
        return Ok(());
    }
    let access = guard::policy(tool).map(|p| p.access);
    // Multi-user M1 (T2a): say WHOSE device it has to be. Both arms are
    // satisfied only by THIS hub's own personal owner, so a message that
    // said "a person's own paired device" told the one caller T2a was
    // written to stop — a colleague's phone, paired to the same hub — that
    // it should have got through. A refusal line is the only explanation a
    // blocked operator gets.
    let message = if access == Some(guard::Access::Person) {
        format!(
            "{tool} is for the fleet's operator or the hub owner's OWN paired device, \
             not a host's token, a device bound to an org, or another person's \
             device ({} refused)",
            caller.label()
        )
    } else if access == Some(guard::Access::PersonDevice) {
        format!(
            "{tool} is for the hub owner's OWN paired device — not another person's, \
             not one bound to an org; on the hub machine use fleet-hub settings \
             ({} refused)",
            caller.label()
        )
    } else if access == Some(guard::Access::Device) {
        format!(
            "{tool} is for a person's own paired device — the hub owner's, or an org \
             admin's; on the hub machine use fleet-hub org|client|person ({} refused)",
            caller.label()
        )
    } else if access == Some(guard::Access::HostToken) {
        // Multi-user M1 (T12). Says which caller it IS for, because the
        // refused one is usually the operator reaching for a claim: their
        // path is `fleet-hub session claim` on the hub machine, and naming it
        // is the only help a refusal line can give.
        format!(
            "{tool} is a per-host token's only: it is reachable from the agent in the              session's own pane, never from the master or a paired device — on the hub              machine use fleet-hub session claim ({} refused)",
            caller.label()
        )
    } else if guard::is_admin_tool(tool) {
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

/// Fail closed: a result the gate could not judge at all — a poisoned store
/// lock, or a scope that would not build — keeps no session row and no work
/// field.
///
/// Both halves, and the ROW half is the one M1 added: dropping the work
/// fields of a row the caller may not see at all was the old answer, and it
/// left the row (its name, its host, its project, its activity, its prompt)
/// in the bytes. The replacement has no scope to ask, so it asks nothing and
/// keeps nothing.
fn fence_everything(result: &mut CallToolResult) {
    // An org no scope can hold, so every work field a row still carries goes
    // with it.
    let nobody = crate::service::orgs::OrgScope::Host {
        alias: String::new(),
        org: None,
        isolated: Default::default(),
    };
    rewrite_json_content(result, |v| {
        crate::service::view_scope::drop_every_session_row(v);
        nobody.redact_json(v, &|_| Some(i64::MIN));
    });
}

/// What [`ViewScope::drop_invisible_rows`] asks of one row-shaped object: the
/// STORED row, read by id and judged by this scope.
///
/// Three answers collapse to [`Visibility::None`], and each is deliberate:
///
/// * no `id` and no `session_id` in the object
///   ([`crate::service::view_scope::session_row_id`]) — a projection the gate
///   cannot resolve is not a row it may pass;
/// * no such row in the store — a row that is gone cannot be judged, and
///   unlike an org (which the payload still carries honestly) its visibility
///   is exactly what a stale payload must not be trusted for;
/// * a failed read — the gate's job is to be the last net, so a broken net
///   holds everything back.
///
/// Memoised per call, because a result can carry one row many times (a page
/// plus a summary, a task's two ends) and the judgement is a row read.
fn visibility_resolver<'a>(
    s: &'a Store,
    scope: &'a ViewScope,
) -> impl Fn(&serde_json::Map<String, serde_json::Value>) -> Visibility + 'a {
    let memo: std::cell::RefCell<std::collections::HashMap<i64, Visibility>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    move |m| {
        let Some(id) = crate::service::view_scope::session_row_id(m) else {
            return Visibility::None;
        };
        if let Some(answer) = memo.borrow().get(&id) {
            return *answer;
        }
        let answer = match s.get_session_by_id(id) {
            Ok(Some(row)) => scope.sees_session_row(&row),
            Ok(None) | Err(_) => Visibility::None,
        };
        memo.borrow_mut().insert(id, answer);
        answer
    }
}

/// What [`crate::service::orgs::OrgScope::redact_json`] asks of one row-shaped
/// object: the row's org, read from the STORE by id — a projection that
/// dropped `org_id` cannot make a row look unassigned. An id the store has no
/// row for keeps the org it was serialised with (a kill's answer); any failed
/// lookup, and any object with no id at all, reads as an org no scope can
/// hold.
fn org_resolver<'a>(
    s: &'a Store,
) -> impl Fn(&serde_json::Map<String, serde_json::Value>) -> Option<i64> + 'a {
    // An org no scope can hold: any failed lookup reads as "not yours".
    const UNREADABLE: i64 = i64::MIN;
    move |m| {
        let own = m.get("org_id").and_then(serde_json::Value::as_i64);
        match m.get("id").and_then(serde_json::Value::as_i64) {
            Some(id) => match s.session_org(id) {
                Ok(Some(o)) => Some(o),
                Ok(None) => own,
                Err(_) => Some(UNREADABLE),
            },
            None => Some(UNREADABLE),
        }
    }
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
    /// [`Self::with_threshold`] at the default threshold, for tests that
    /// have no store; production callers hold the store and pass its value.
    #[cfg(test)]
    pub(super) fn new(is_controller: bool, row: crate::store::SessionRow) -> Self {
        Self::with_threshold(
            is_controller,
            row,
            crate::service::attention::DEFAULT_CONTEXT_RED_PCT,
            &crate::service::attention::Facts::default(),
        )
    }

    /// [`Self::new`] at the store's `health.context_red_pct`
    /// (`service::health::context_red_pct`), which every caller holding the
    /// store should pass so `context_full` and `fleet_health.context_red`
    /// agree — with the store's [`Store::attention_facts`], so the three
    /// `Blocked` reasons agree with `/events` (step 2.6).
    ///
    /// [`Store::attention_facts`]: crate::store::Store::attention_facts
    pub(super) fn with_threshold(
        is_controller: bool,
        row: crate::store::SessionRow,
        context_red_pct: f64,
        facts: &crate::service::attention::Facts,
    ) -> Self {
        Self {
            is_controller,
            needs_attention: crate::service::attention::needs_attention_in(
                &row,
                context_red_pct,
                facts,
            ),
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
    /// The org the client is bound to (work graph M14); absent: unbound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) org_id: Option<i64>,
    /// When the operator let this client manage the asset catalog
    /// (`catalog_admin`, migration 074); absent: not granted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) assets_admin_at: Option<i64>,
    /// WHOSE device this is (multi-user M1, migration 100); absent: nobody's,
    /// which means it sees no private session at all. `fleet-hub client list`
    /// prints it, and `fleet-hub client bind-person` changes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) person_id: Option<i64>,
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
            org_id: r.org_id,
            assets_admin_at: r.assets_admin_at,
            person_id: r.person_id,
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
    /// `true` for a row from another hub over a link (`from_addr` set): the
    /// preview is that peer's text with the untrusted-content marker
    /// stripped for room, so the flag carries what the marker would (D8:
    /// every rendering of peer text says it is untrusted). Absent for a
    /// local row. Unforgeable: a peer's own marker lines are neutralised in
    /// `apply.rs`, and this field comes from the row, never the body.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(super) untrusted: bool,
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
        // field AND the `untrusted` flag below, so stripping the marker
        // here loses no signal.
        let untrusted = m.from_addr.is_some();
        let preview_source: &str = if untrusted {
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
            untrusted,
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
        self.confirm_gate_with(tool, nonce, summary, caller, false)
    }

    /// [`Self::confirm_gate`], with `person` forcing a person's approval
    /// for this call whoever the caller is — as the operator's starts and
    /// kills always are — for a call site that decides that itself (see
    /// `add_project`'s `create_remote`).
    pub(super) fn confirm_gate_with(
        &self,
        tool: &str,
        nonce: Option<&str>,
        summary: &str,
        caller: &Caller,
        person: bool,
    ) -> Result<(), McpError> {
        debug_assert!(
            guard::needs_confirmation(tool) || guard::OPERATOR_CONFIRMS.contains(&tool),
            "{tool} is not in guard::CONFIRM_TOOLS"
        );
        // The operator's starts and kills are always confirmed (D12); for
        // everyone else only the `confirm: true` tools, and only with the
        // toggle on.
        let forced = person || guard::operator_must_confirm(caller.is_operator(), tool);
        if !forced && (!guard::needs_confirmation(tool) || !self.confirm_enabled()?) {
            return Ok(());
        }
        if forced && !self.guards.approver {
            return Err(mcp_err(
                "E_FORBIDDEN",
                format!(
                    "{tool} from {} needs a person to approve it, and this hub has \
                     no approver; ask the person to do it from the sidebar",
                    if caller.is_operator() {
                        "the operator"
                    } else {
                        "this caller"
                    }
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
        let req = confirms.request_from(tool, summary, &caller.label(), caller.is_operator());
        (self.guards.notify)(&req);
        Err(mcp_err(
            codes::E_CONFIRM_REQUIRED,
            format!(
                "{tool} needs approval on the claude-fleet desktop ({}); \
                 ask the user to approve it there, then retry with confirm_nonce={}",
                if person && !caller.is_operator() {
                    "a person approves this call whoever makes it"
                } else if forced {
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
        reach: Reach,
        what: &str,
    ) -> Result<(String, String), McpError> {
        let s = lock(&self.store).map_err(to_mcp_err)?;
        resolve_and_gate(&s, caller, session_id, host_alias, tmux_name, reach, what)
    }

    // `require_visible_session` was here (decision D7's org-only read gate).
    // It is GONE, collapsed into `resolve_row_and_gate` by multi-user M1's
    // T7, and the collapse is the point rather than a tidy-up: its first
    // line was `if !caller.is_scoped() { return Ok(()) }`, which is false
    // for the master AND for every unbound paired client — so for a phone it
    // checked nothing at all, while being the only gate on the tools that
    // called it (`repo_file`, which returns arbitrary worktree file
    // contents, and `session_history`, one of the four reads the spec calls
    // the substance of `watch`). Every former call site now resolves its
    // target through `resolve_target_row` with the reach it needs.

    /// **Choke point 3** (spec §5.1): the one net under EVERY tool result,
    /// whatever tool answered and whether it answered a result or an error's
    /// details.
    ///
    /// Two halves, under one lock and in one pass over the JSON:
    ///
    /// * the **session** half (multi-user M1, T8): a session row the
    ///   caller's [`ViewScope`] cannot see is REMOVED — out of its array, or
    ///   nulled where it was a field (`ViewScope::drop_invisible_rows`).
    /// * the **org** half (work graph M5): a row the caller may see keeps its
    ///   identity and loses the work of another company
    ///   (`OrgScope::redact_json`).
    ///
    /// Both resolve a row through the **store**, by id, never through the
    /// payload: a projection that dropped `org_id` cannot make a row look
    /// unassigned, and one that dropped `visibility` cannot make a private
    /// row look unclaimed.
    ///
    /// **It runs for every caller.** Until M1 the call site in `call_tool`
    /// was `if caller.is_scoped()` and the body returned early on
    /// `OrgScope::is_all` — and `is_scoped()` is false for the master and for
    /// every paired client bound to no org, which is precisely the caller M1
    /// introduces. So revision 3's "fail-closed backstop over every tool
    /// result" executed, for a person's phone, never. There is no early
    /// return left here: the only unrestricted scope is
    /// `ViewScope::internal`, the hub's own reader, which is built directly
    /// and never from a `Caller` (`drop_invisible_rows` states that rule
    /// where the rest of the type's rules live).
    ///
    /// Fails closed twice over: a poisoned lock or a scope that cannot be
    /// built drops every session row AND every work field
    /// ([`fence_everything`]) — not only the work fields, which was the old
    /// behaviour and which left the rows themselves in the answer.
    ///
    /// The store is read through `store`: the read pool for a tool that wrote
    /// nothing (`POOLED_READ_TOOLS`), else the writer, so a tool's gate sees
    /// its own writes.
    ///
    /// [`ViewScope`]: crate::service::view_scope::ViewScope
    pub(super) fn fence_result_via(
        &self,
        store: &Mutex<Store>,
        caller: &Caller,
        result: &mut CallToolResult,
    ) {
        let Ok(s) = store.lock() else {
            fence_everything(result);
            return;
        };
        let scope = match caller.view_scope(&s) {
            Ok(sc) => sc,
            Err(_) => {
                drop(s);
                fence_everything(result);
                return;
            }
        };
        let sees = visibility_resolver(&s, &scope);
        let org_of = org_resolver(&s);
        rewrite_json_content(result, |v| {
            // Rows first: a row that leaves the answer takes its work with
            // it, and the org half then has less to walk.
            scope.drop_invisible_rows(v, &sees);
            scope.org.redact_json(v, &org_of);
        });
    }

    /// [`Self::fence_result_via`] through the writer, exactly as `call_tool`
    /// gates a tool that may have written.
    ///
    /// The tests drive this rather than `ServerHandler::call_tool` itself
    /// because a `RequestContext` cannot be built outside rmcp (its `Peer`
    /// has no public constructor), so the call site is pinned by reading
    /// `mod.rs` instead — `the_result_gate_is_reached_for_every_caller`.
    #[cfg(test)]
    pub(super) fn fence_result_for(&self, caller: &Caller, result: &mut CallToolResult) {
        self.fence_result_via(&self.store, caller, result)
    }

    /// The ORG half alone, as the org-isolation matrix drives it
    /// (`tests_isolation.rs`), through the writer.
    ///
    /// Production has no such path any more — `call_tool` runs
    /// [`Self::fence_result_via`], which does both halves together. The
    /// matrix keeps the narrow form deliberately: what it asserts is the org
    /// boundary over results whose rows its fixture makes visible to every
    /// caller in it (each host token stands in its own row's pane, and every
    /// client is the hub's one person's device), and a matrix that also
    /// dropped rows would stop saying which boundary refused what. The
    /// session half has its own tests, and the gate's own call site has a
    /// regression guard that reads `mod.rs`
    /// (`the_result_gate_is_reached_for_every_caller`).
    #[cfg(test)]
    pub(super) fn redact_work_for(&self, caller: &Caller, result: &mut CallToolResult) {
        let Ok(s) = self.store.lock() else {
            fence_everything(result);
            return;
        };
        let scope = match caller.org_scope(&s) {
            Ok(sc) => sc,
            Err(_) => {
                drop(s);
                fence_everything(result);
                return;
            }
        };
        let org_of = org_resolver(&s);
        rewrite_json_content(result, |v| scope.redact_json(v, &org_of));
    }

    /// `send_message`'s RECIPIENT, through the one session gate (multi-user
    /// M1, T7).
    ///
    /// Before this, the far end of a message was checked only inside
    /// `service/messages.rs`'s `if !scope.is_all()` blocks, which never run
    /// for a person's device — so a phone could `deliver: true, submit:
    /// true` into any pane in the fleet. The gate belongs here, where every
    /// other session-addressed call already is, and it carries
    /// [`Reach::Drive`]: a message that pastes into a pane is `send_prompt`
    /// by another route (spec §4.3, invariant 5).
    ///
    /// Three shapes of recipient, and only one of them is ours to judge:
    ///
    /// * no `to_addr` — a plain `to_session_id`, gated here;
    /// * a `to_addr` naming a session of THIS fleet (no fleet part, or our
    ///   own) — resolved by `(host, name)` through the same gate, because
    ///   the address is only another spelling of the same local row;
    /// * a `to_addr` naming another fleet, a client or a hub — not a local
    ///   session, so there is no row here to judge. `send_remote` already
    ///   refuses `deliver` outright and the receiving hub applies its own
    ///   rules; a malformed address is left to `service/messages.rs`, which
    ///   has the sentence for it.
    ///
    /// The local fleet id is read through its own short lock BEFORE the
    /// store guard is taken: `Store` is a plain, non-reentrant
    /// `std::sync::Mutex` and `stored_local_fleet_id` locks it itself.
    pub(super) fn require_message_recipient(
        &self,
        caller: &Caller,
        p: &SendMessageParams,
    ) -> Result<(), McpError> {
        let by_addr = match p.to_addr.as_deref() {
            None => None,
            Some(raw) => {
                // A malformed address is not refused here: it reaches
                // `service::messages`, which answers it by name.
                let Ok(crate::service::address::Addr::Session { fleet, host, name }) =
                    crate::service::address::parse(raw)
                else {
                    return Ok(());
                };
                let local = crate::service::address::stored_local_fleet_id(&self.store)
                    .map_err(to_mcp_err)?;
                if fleet.is_some() && fleet != local {
                    // Another fleet's session: nothing local to gate.
                    return Ok(());
                }
                Some((host, name))
            }
        };
        let s = lock(&self.store).map_err(to_mcp_err)?;
        // Person-gated without the host fence in front: a recipient this
        // caller cannot reach — another person's, another org's, another
        // host's — answers exactly as a session that does not exist, which
        // is the answer `service/messages.rs` already gave for the two
        // fences it had. See `resolve_row_person_gated` for why the host
        // rule is not lost with it.
        let row = match &by_addr {
            Some((host, name)) => {
                sessions::resolve_session_target(&s, None, Some(host.as_str()), Some(name.as_str()))
            }
            None => sessions::resolve_session_target(&s, Some(p.to_session_id), None, None),
        }
        .map_err(to_mcp_err)?;
        require_person_sees(&s, caller, &row, Reach::Drive, "the message's recipient")
    }

    /// [`Self::resolve_target_row`] without [`require_host`] in front of it:
    /// the person gate alone, which answers `E_NOTFOUND` for every row this
    /// caller may not see — including one on another host.
    ///
    /// **Nothing is unfenced by leaving `require_host` out.**
    /// [`crate::service::view_scope::ViewScope::sees_session_row`]'s host arm
    /// already refuses a per-host token every row that is not on its own
    /// host, so the only thing that changes is the CODE: `E_NOTFOUND`
    /// instead of `E_FORBIDDEN`, and a sentence that does not name the other
    /// host.
    ///
    /// That is the right trade wherever the only useful answer for a row this
    /// caller cannot reach is "there is no such row":
    ///
    /// * **the `repo_*` reads and `session_history`**, whose subject IS the
    ///   content of somebody else's session (spec §4.3, *What counts as
    ///   content*), so a sentence naming the host it sits on tells a stranger
    ///   it exists;
    /// * **a BATCH item** — `work_link { tidy_apply }`, and the planned
    ///   sessions of `restore_host_sessions` — where every item has always
    ///   answered for itself and "another host's session reads as one that
    ///   does not exist" is the rule the batch already followed;
    /// * **`work_link { handover }`**, whose service fence answered the same
    ///   way before this task, and whose refusal is compared against an
    ///   unknown id by the isolation matrix (`same_as_unknown`) precisely so
    ///   the two stay indistinguishable.
    ///
    /// Everywhere else the host fence runs first and says which host the
    /// session is on, which is the more useful sentence for an agent that
    /// named a real session on the wrong machine — and no secret, since a
    /// host token has shell on its own box.
    pub(super) fn resolve_row_person_gated(
        &self,
        caller: &Caller,
        session_id: i64,
        reach: Reach,
        what: &str,
    ) -> Result<crate::store::SessionRow, McpError> {
        let s = lock(&self.store).map_err(to_mcp_err)?;
        let row = sessions::resolve_session_target(&s, Some(session_id), None, None)
            .map_err(to_mcp_err)?;
        require_person_sees(&s, caller, &row, reach, what)?;
        Ok(row)
    }

    /// `Reach::Drive` on every live session linked to a local work item
    /// (multi-user M1, T7).
    ///
    /// `work_link { name, item_id }` renames a local item, and a local item is
    /// only ever reachable THROUGH the sessions linked to it
    /// (`local::local_item_visible`, which returns `true` on `scope.is_all()` —
    /// i.e. for every paired client bound to no org). The title it rewrites is
    /// what the OWNER's sidebar shows for their own row, so it takes the same
    /// level as the naming half of the same arm.
    ///
    /// The refusal is the ITEM's, never the session's: this arm is addressed by
    /// an item id, and the org fence it composes with answers an out-of-scope
    /// item as an unknown item (`tests_isolation`'s `same_as_unknown` holds the
    /// two sentences equal). A refusal naming a session id here would break
    /// that AND tell the caller a row exists.
    ///
    /// **An item with no CONFIRMED link at all is nobody's to prove, so it
    /// is nobody's to write** (multi-user M1, T9e). This used to pass — "no
    /// link, nothing to protect" — and the loop below is why that was wrong:
    /// with an empty list the `for` body never runs and the gate returned
    /// `Ok(())` without examining anything, so for such an item the whole
    /// fence was [`crate::service::work::local::local_item_visible`]'s
    /// `if scope.is_all() { return Ok(true) }`, and `OrgScope::All` is what
    /// every person's own `full` device resolves to. The state is reachable
    /// in two moves and both are ordinary: `Store::local_item_links` selects
    /// `WHERE l.state = 'confirmed'`, so a merely SUGGESTED link does not
    /// count, and `unlink_session_work_held` DELETEs the link row outright
    /// (`DELETE FROM work_links WHERE id = ?1`) — so Ada's
    /// `work_link { name }` followed by Ada's `work_link { unlink }` left a
    /// `work_items` row any `full` device could rename and whose status any
    /// `full` device could set, which `status_set_by = 'person'` makes FINAL
    /// over the derived value.
    ///
    /// The answer is the one this milestone already settled for the other
    /// "no record to judge" shape, and it is the SAME sentence:
    /// [`crate::service::orgs::link_person_visible`]'s arm 3 (a link with no
    /// live participant and not one recorded conversation id) answers
    /// `view.host.is_some() || view.is_sole_person()` — rule 7 where it is
    /// really about rule 7, and nothing wider. A per-host token passes for
    /// §4.4's reason (a host's reach, no person dimension) and is then
    /// refused downstream anyway, because `local_item_visible`'s own org
    /// half finds no visible link for it either. Every other caller,
    /// the master included, is answered as an unknown item. The cost is
    /// stated plainly: an item whose last link its OWNER unlinked is no
    /// longer renameable by its owner either — the same fail-closed
    /// direction, and the same product cost, as a detached task
    /// ([`crate::service::tasks::task_visible_in_scope_pure`]).
    ///
    /// An item whose links have all ENDED is still its owner's: multi-user M1
    /// T9c added the second arm below, because
    /// [`crate::store::Store::local_item_links`] joins participants only
    /// `AND l.ended_at IS NULL`, so an ended link always yields
    /// `session_id = None` — and collecting `filter_map(|l| l.session_id)`
    /// therefore found NO occupants for an item every one of whose links had
    /// been reaped, and `work_link { name, item_id }` renamed another
    /// person's finished work. That is the same ended-half hole T9b closed
    /// for the link READS, through the same predicate
    /// ([`crate::service::orgs::link_person_visible`]).
    ///
    /// `local_item_links` is also a FOURTH store read that yields ended
    /// `work_links` rows, alongside `scope_links_for`'s three; the by-id
    /// `Store::get_work_link` and the live-only `live_work_sessions_for_key`
    /// complete that enumeration.
    pub(super) fn require_drive_on_item_sessions(
        &self,
        caller: &Caller,
        item_id: i64,
    ) -> Result<(), McpError> {
        let s = lock(self.reader()).map_err(to_mcp_err)?;
        let view = caller.view_scope(&s).map_err(to_mcp_err)?;
        let links = s.local_item_links(Some(item_id)).map_err(to_mcp_err)?;
        // Nothing confirmed to judge: fail CLOSED, as `link_person_visible`'s
        // arm 3 does for a link with nothing recorded. An empty list is not
        // "no owner to offend", it is "no evidence this is yours" — and it is
        // reachable in two ordinary moves (name, then unlink). See the doc.
        if links.is_empty() && !(view.host.is_some() || view.is_sole_person()) {
            return Err(to_mcp_err(crate::service::orgs::not_found(
                "work item",
                item_id,
            )));
        }
        for l in links {
            match l.session_id {
                Some(sid) => {
                    let Some(row) = s
                        .get_session_by_id(sid)
                        .map_err(|e| to_mcp_err(IpcError::from(e)))?
                    else {
                        continue;
                    };
                    if !reaches_row(&s, caller, &row, Reach::Drive)? {
                        return Err(to_mcp_err(crate::service::orgs::not_found(
                            "work item",
                            item_id,
                        )));
                    }
                }
                // No live participant: the ended half, judged by the one
                // ended-link predicate. `Reach::Drive` has no meaning for a
                // session that no longer exists — there is nothing to drive
                // — so the question is whether this caller may see the link
                // at all, which is what `work { links }` asks of it too.
                None => {
                    if !crate::service::orgs::link_person_visible(&s, &view, &l.link)
                        .map_err(to_mcp_err)?
                    {
                        return Err(to_mcp_err(crate::service::orgs::not_found(
                            "work item",
                            item_id,
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// [`Self::resolve_target`] returning the whole row.
    pub(super) fn resolve_target_row(
        &self,
        caller: &Caller,
        session_id: Option<i64>,
        host_alias: Option<&str>,
        tmux_name: Option<&str>,
        reach: Reach,
        what: &str,
    ) -> Result<crate::store::SessionRow, McpError> {
        let s = lock(&self.store).map_err(to_mcp_err)?;
        resolve_row_and_gate(&s, caller, session_id, host_alias, tmux_name, reach, what)
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
        let turns = transcript_turns(row.turn_seq, since_turn);
        let max_chars = max_chars
            .unwrap_or(transcript::DEFAULT_MAX_CHARS)
            .clamp(1, transcript::MAX_MAX_CHARS);
        let args =
            transcript::resolve_args(&self.store, row, turns, max_chars).map_err(to_mcp_err)?;
        transcript::fetch_transcript(args, &self.ssh)
            .await
            .map_err(to_mcp_err)
    }

    /// Ask a long poll's [`tasks::AccessRecheck`] once, outside the wait,
    /// under a fresh lock.
    ///
    /// Every long poll calls this immediately before it builds its
    /// payload. The in-loop re-check cannot be the last word on its own:
    /// between the final wake and `ok_json` a tool may take another await
    /// (`wait_for_session`'s pane probe, `run_prompt`'s transcript read),
    /// and that gap is exactly long enough for a revoke to land.
    pub(super) fn recheck_now(&self, recheck: &dyn tasks::AccessRecheck) -> Result<(), McpError> {
        let s = lock(&self.store).map_err(to_mcp_err)?;
        recheck.check(&s).map_err(to_mcp_err)
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

    /// A task the caller may see, and may reach this far into.
    ///
    /// A `TaskRow` carries `prompt`, `result` and `error` — the text one
    /// session sent another and the paragraph it sent back. That is session
    /// CONTENT, so the gate is the sessions' own: a task is visible when the
    /// caller sees every session it names
    /// (`service::tasks::task_visible_in_scope`), and the per-host token's
    /// §4.4 clauses are the ones it already has everywhere else.
    ///
    /// Before M1 this fenced on `caller.host_alias` alone, and
    /// `task_visible_to` opened with `let Some(host) = host else { return
    /// Ok(true) }` — so a caller with no host binding, which is the master
    /// AND every person's paired device, was handed every task in the fleet.
    /// That was already an org leak; with two people it is a privacy one.
    ///
    /// Unknown → `E_NOTFOUND`; invisible → `E_NOTFOUND` as well, so an id
    /// the caller may not see and an id that does not exist are the same
    /// answer (the old `E_FORBIDDEN` was an existence oracle over the whole
    /// task table). Visible but below `reach` → `E_FORBIDDEN`.
    pub(super) fn visible_task(
        &self,
        caller: &Caller,
        task_id: i64,
        reach: Reach,
    ) -> Result<crate::store::TaskRow, McpError> {
        let s = lock(&self.store).map_err(to_mcp_err)?;
        task_visible_at(&s, caller, task_id, reach).map_err(to_mcp_err)
    }
}

/// [`FleetTools::visible_task`] against a store handle the caller already
/// holds, in the service layer's error type.
///
/// Written once, here, for the same reason [`person_sees`] is: the
/// `wait_for_task` long poll re-checks this on every wake from inside
/// `service::tasks` (T11), where there is no `McpError` and no [`Caller`].
pub(super) fn task_visible_at(
    s: &Store,
    caller: &Caller,
    task_id: i64,
    reach: Reach,
) -> Result<crate::store::TaskRow, IpcError> {
    let task = s
        .get_task(task_id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("task {task_id} not found")))?;
    let scope = caller.view_scope(s)?;
    if !tasks::task_visible_in_scope(s, &task, &scope)? {
        return Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("task {task_id} not found"),
        ));
    }
    // The reach is judged on the WORKER: a task's drive-level act
    // (`cancel_task`) stops work on the worker's machine, and the
    // requester is only the session that asked for it. A task with no
    // worker row to judge is refused rather than allowed — there is
    // nothing to be the owner of.
    if reach != Reach::Read {
        let worker = match task.worker_session_id {
            Some(id) => s.get_session_by_id(id)?,
            None => None,
        };
        let ok = match (&worker, reach) {
            (Some(row), Reach::Drive) => {
                // §4.4: a per-host token drives what it can reach, and
                // the task's own visibility above is what it could
                // reach. Without this clause the agent that dispatched
                // a task could not cancel the worker it spawned — the
                // worker inherits the REQUESTER's owner (T5), so it is
                // not the machine's to own even though it is the
                // machine's to run.
                scope.may_drive(row) || scope.host.as_deref() == Some(row.host_alias.as_str())
            }
            // No equivalent for `own`: a pane proof is never ownership,
            // and a per-host token is never an owner.
            (Some(row), Reach::Own) => scope.may_own(row),
            (Some(row), Reach::Answer) => scope.may_answer(row),
            (Some(_), Reach::Read) | (None, _) => false,
        };
        if !ok {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                format!(
                    "task {task_id} needs {} on its worker session and this access \
                         does not carry it",
                    reach.needed()
                ),
            ));
        }
    }
    Ok(task)
}

// ---- smart caching (`fresh_for`) --------------------------------------------

/// The reader a `fresh_for` names is bound to the caller like a target
/// session: the cursor it advances is that session's OWN view of a stream
/// (migration 044, `docs/control-api.md`: "your own session id"), so a
/// foreign caller writing it would blind the reader to deltas — the silent
/// skip the design forbids. A per-host token may only name a session on its
/// own host (`E_FORBIDDEN` otherwise, like every other write it makes); the
/// row itself is then resolved through the caller's [`ViewScope`], which is
/// the ONE visibility rule (org boundary, §4.4's host clauses, ownership
/// and grants) rather than a second one written here.
///
/// The scope is consulted **unconditionally**, never behind
/// `caller.is_scoped()`: that predicate is false for the master and for
/// every person's paired device, so a guarded check left the cursor key
/// — `(reader session id, tool, resource key)`, with no caller in it —
/// shared between people. Person B naming a session of person A's wrote
/// B's own page hash under A's cursor, and A's next identical read
/// answered `unchanged` with no rows: exactly the blinding this helper
/// exists to prevent, made cross-person the moment `list_sessions` began
/// paging per person.
///
/// `Ok(false)` — the `reader_unknown` answer, under which no cursor is ever
/// written — is returned for an invisible row as well as a missing one, so
/// the two are indistinguishable and the call is no existence oracle. The
/// ONE helper every `fresh_for` tool calls, so the five cannot drift.
///
/// [`ViewScope`]: crate::service::view_scope::ViewScope
pub(super) fn resolve_reader(s: &Store, caller: &Caller, reader: i64) -> Result<bool, McpError> {
    let Some(row) = s
        .get_session_by_id(reader)
        .map_err(|e| to_mcp_err(IpcError::from(e)))?
    else {
        return Ok(false);
    };
    // The visibility check and NOTHING before it. `require_host` used to run
    // first, and its refusal names the other host ("fresh_for's session is on
    // host h2; this token is bound to h1"), so an agent probing ids learned
    // which ones exist and which machine each lives on — a one-bit oracle over
    // the whole `sessions` table, past the very doc comment above that says the
    // two cases are indistinguishable. Nothing is lost by dropping it:
    // `ViewScope::sees_session_row`'s host arm already refuses a per-host token
    // every row that is not on its own host, which is the argument
    // `resolve_row_person_gated` makes for itself.
    let scope = caller.view_scope(s).map_err(to_mcp_err)?;
    Ok(scope.sees_session_row(&row).is_visible())
}

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

/// How many turns a `since_turn` read asks for. `since_turn` is the client's
/// own number, so it is bounded to `0..=turn_seq` before the subtraction, as
/// `transcript::conv_turns_for` does: `i64::MIN` used to overflow it (a panic
/// in a debug build) and a very negative one asked for ~1e18 turns. Anything
/// out of range reads the last turn.
pub(super) fn transcript_turns(turn_seq: i64, since_turn: Option<i64>) -> usize {
    match since_turn {
        Some(t) if (0..=turn_seq).contains(&t) => usize::try_from(turn_seq - t).unwrap_or(0).max(1),
        _ => 1,
    }
}
