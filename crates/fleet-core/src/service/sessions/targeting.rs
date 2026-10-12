//! Resolving which session a call addresses (by id, or host + tmux name),
//! related sessions, and the controller self-target guard.

use super::*;
use crate::ipc_error::codes;
use crate::ipc_error::lock;

#[derive(Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "RelatedSessionsParams")]
pub struct RelatedSessionsArgs {
    /// Fleet session id.
    pub session_id: i64,
}

/// The sessions sharing one's project and worktree, under the caller's scope
/// (work graph M5, D7 + multi-user M1, T7): sessions of an org isolated from
/// the caller's host, and
/// sessions belonging to another person, are left out — and an anchor that is
/// one of them reads as missing.
///
/// The anchor check is **unconditional**. It used to sit behind
/// `if !scope.is_all()`, which is true for the master and for every paired
/// client bound to no org, so in practice nothing fenced the anchor at all:
/// the answer `[my own session 31]` for an anchor id that is somebody else's
/// private session told the caller that session lives in the same project and
/// worktree as theirs, while a nonexistent id answered `[]`. The result gate
/// (T8) cannot close that — it drops rows, and cannot turn "not yours" into
/// `E_NOTFOUND`.
pub fn related_sessions_scoped(
    args: RelatedSessionsArgs,
    store: &Mutex<Store>,
    view: &crate::service::view_scope::ViewScope,
) -> Result<Vec<SessionRow>, IpcError> {
    let scope = &view.org;
    let s = lock(store)?;
    // One answer for both "no such row" and "not yours", in the one sentence
    // `orgs::not_found` gives everywhere else: the two must be
    // indistinguishable or the call is an existence oracle. (It used to be
    // `rusqlite::Error::QueryReturnedNoRows`, i.e. `E_SQLITE: Query returned
    // no rows`, for the missing case and nothing at all for the invisible one.)
    match s.get_session_by_id(args.session_id)? {
        Some(r) if view.sees_session_row(&r).is_visible() => {}
        _ => return Err(crate::service::orgs::not_found("session", args.session_id)),
    }
    let mut rows = s
        .list_related_sessions(args.session_id)
        .map_err(IpcError::from)?;
    // The org half and the person half, in one line: `sees_session_row` is
    // the person half and composes the org answer for a caller-built scope,
    // while `sees_row_org_only` is what still fences an internal-and-narrowed
    // reader (see `scope_guard_tests::ORG_HALF_SITES`).
    rows.retain(|r| scope.sees_row_org_only(r) && view.sees_session_row(r).is_visible());
    for r in rows.iter_mut() {
        scope.redact_row_org_only(r);
    }
    Ok(rows)
}

/// Session addressing for the control API (MCP-6). Every name-addressed
/// tool accepts EITHER a fleet `session_id` OR the `(host_alias, tmux_name)`
/// pair; this resolves whichever was given to the stored row. Precedence:
/// `session_id` when present (host/name are then ignored), else both parts
/// of the pair are required.
///
/// Errors: `E_INVALID` when neither form is complete, `E_NOTFOUND` when the
/// id / pair matches no row.
pub fn resolve_session_target(
    s: &Store,
    session_id: Option<i64>,
    host_alias: Option<&str>,
    tmux_name: Option<&str>,
) -> Result<SessionRow, IpcError> {
    if let Some(id) = session_id {
        return s
            .get_session_by_id(id)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("session {id} not found")));
    }
    match (host_alias, tmux_name) {
        (Some(host), Some(name)) if !host.trim().is_empty() && !name.trim().is_empty() => {
            crate::validate::host_alias(host)?;
            crate::validate::tmux_name_lookup(name)?;
            s.get_session(name, host)?.ok_or_else(|| {
                IpcError::new(
                    codes::E_NOTFOUND,
                    format!("session {name} not found on {host}"),
                )
            })
        }
        _ => Err(IpcError::new(
            codes::E_INVALID,
            "pass session_id, or both host_alias and tmux_name",
        )),
    }
}

/// The INTERNAL-reader form of [`find_session_by_tmux_name_scoped`]: the ONE
/// fleet row whose `tmux_name` matches, across every person and every org.
///
/// **No request path may call this.** It builds `ViewScope::internal()`, which
/// is the hub's own unrestricted reader, and its `E_AMBIGUOUS` details name
/// every candidate's `(session_id, host_alias)` — metadata of somebody's
/// private session (multi-user M1, rules 1 and 6). `whoami` used to call it and
/// now calls the scoped form (`mcp/tools/session_ops.rs`), which is the whole
/// reason the scoped form exists. `#[cfg(test)]` so the unscoped shape cannot
/// be reached by a new tool at all: `only_caller_view_scope_constructs_a_view_scope`
/// polices `ViewScope::for_caller`, not `ViewScope::internal`.
#[cfg(test)]
pub fn find_session_by_tmux_name(s: &Store, tmux_name: &str) -> Result<SessionRow, IpcError> {
    find_session_by_tmux_name_scoped(
        s,
        tmux_name,
        &crate::service::view_scope::ViewScope::internal(),
    )
}

/// [`find_session_by_tmux_name`] among the sessions `scope` may see — the
/// WHOLE question, org and person: an isolated org's same-named session
/// (work graph M5, D7) and another PERSON's same-named session (multi-user
/// M1, rules 1 and 6) neither match nor show up among the ambiguity's
/// candidates.
///
/// The person half is not optional here, and the `E_AMBIGUOUS` arm below is
/// why: its `details` carry `(session_id, host_alias)` for every match, so a
/// scope that filtered by org alone would answer any caller — a paired
/// client, a per-host token — with the id and host of another person's
/// private session. A row this scope cannot see is answered exactly as an id
/// that does not exist — the same discipline `require_person_sees`
/// (`mcp/tools/support.rs`) states for the id-addressed path.
pub fn find_session_by_tmux_name_scoped(
    s: &Store,
    tmux_name: &str,
    scope: &crate::service::view_scope::ViewScope,
) -> Result<SessionRow, IpcError> {
    crate::validate::tmux_name_lookup(tmux_name)?;
    // `WHERE tmux_name=?` directly, instead of loading every session (lost
    // ones included) and filtering in Rust. No `lost_at` filter here: a
    // ghost must still be considered for the running-preference / ambiguity
    // logic below.
    let all: Vec<SessionRow> = s
        .find_sessions_by_tmux_name(tmux_name, None)?
        .into_iter()
        .filter(|r| scope.sees_session_row(r).is_visible())
        .collect();
    // A ghost left behind on another host must not make a live session
    // ambiguous: prefer running rows, fall back to everything.
    let running: Vec<SessionRow> = all
        .iter()
        .filter(|r| r.status == "running")
        .cloned()
        .collect();
    let matches = if running.is_empty() { all } else { running };
    match matches.len() {
        0 => Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("no session named {tmux_name} on any host"),
        )),
        1 => Ok(matches.into_iter().next().expect("one match")),
        _ => {
            let candidates: Vec<serde_json::Value> = matches
                .iter()
                .map(|r| serde_json::json!({ "session_id": r.id, "host_alias": r.host_alias }))
                .collect();
            Err(IpcError::new(
                codes::E_AMBIGUOUS,
                format!(
                    "{} sessions are named {tmux_name}; pass session_id or host_alias",
                    matches.len()
                ),
            )
            .with_details(serde_json::json!({ "candidates": candidates })))
        }
    }
}

/// Refuse to operate on the registered controller session unless `force`.
///
/// Returns `Err(E_SELF_TARGET)` when `(host, name)` equals the registered
/// controller `(host, tmux_name)` and `force` is false. Always `Ok` when no
/// controller is registered, the target is a different session, or `force` is
/// set. Pure — the caller reads the controller from the store first.
pub fn guard_not_controller(
    controller: Option<&(String, String)>,
    host: &str,
    name: &str,
    force: bool,
) -> Result<(), IpcError> {
    if force {
        return Ok(());
    }
    if let Some((c_host, c_name)) = controller {
        if c_host == host && c_name == name {
            return Err(IpcError::new(
                codes::E_SELF_TARGET,
                format!(
                    "{name} on {host} is the registered fleet controller; \
                     pass force=true to target it anyway"
                ),
            ));
        }
    }
    Ok(())
}

/// The one refusal sentence for a landing that is not this caller's to make.
///
/// Shared by the service-layer gate below and by
/// `mcp::tools::support::require_may_land_in_worktree`, so the two cannot
/// drift into two different sentences for one rule. It names no session: the
/// caller addressed a worktree, and a sentence naming the occupant would make
/// the refusal an oracle for whose sessions live where (the same reason
/// `delete_worktree`'s refusal is the worktree's).
pub const LANDING_NOT_YOURS: &str =
    "that worktree is occupied by a session that is not yours to drive: \
     a pane started there would share its working tree";

/// May this caller LAND a new session in `worktree_id` (multi-user M1; the
/// `new_session` / `new_shell_session` half is T8d, this one T9b)?
///
/// A `worktree_id` reads as "a checkout, not a session". It is both: a pane
/// started in a checkout somebody else's live session is working in shares
/// that working tree, so a start there followed by `capture_session` on the
/// caller's OWN row reads the contents of that tree — past the `Reach::Read`
/// every `repo_*` tool takes for the same bytes.
///
/// `Drive`, not `Read`: the new pane can WRITE in the tree (`git reset
/// --hard`, an editor, a build), which is more than reading one file out of
/// it. `own` would be wrong in the other direction — nothing of the existing
/// session is destroyed, relocated or re-shared.
///
/// The occupants are the ones `delete_worktree` gates
/// ([`crate::store::Store::occupant_session_ids_for_worktree`]): the running
/// rows plus the rows a host LOST while pointing there, because a reboot
/// leaves the checkout and its uncommitted work exactly where they were. A
/// worktree that does not exist yet (`None`, every `new_worktree` start) has
/// no occupants and passes.
pub fn require_may_land_in_worktree(
    s: &Store,
    view: &crate::service::view_scope::ViewScope,
    worktree_id: Option<i64>,
) -> Result<(), IpcError> {
    let Some(wid) = worktree_id else {
        return Ok(());
    };
    for sid in s.occupant_session_ids_for_worktree(wid)? {
        // Gone between the two reads: nothing of anybody's to share.
        let Some(row) = s.get_session_by_id(sid)? else {
            continue;
        };
        if !view.may_drive(&row) {
            return Err(IpcError::new(codes::E_FORBIDDEN, LANDING_NOT_YOURS));
        }
    }
    Ok(())
}
