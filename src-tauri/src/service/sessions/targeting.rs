//! Resolving which session a call addresses (by id, or host + tmux name),
//! related sessions, and the controller self-target guard.

use super::*;

#[derive(Deserialize)]
pub struct RelatedSessionsArgs {
    pub session_id: i64,
}

pub fn related_sessions(
    args: RelatedSessionsArgs,
    store: &Mutex<Store>,
) -> Result<Vec<SessionRow>, IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.list_related_sessions(args.session_id)
        .map_err(IpcError::from)
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
            .ok_or_else(|| IpcError::new("E_NOTFOUND", format!("session {id} not found")));
    }
    match (host_alias, tmux_name) {
        (Some(host), Some(name)) if !host.trim().is_empty() && !name.trim().is_empty() => {
            crate::validate::host_alias(host)?;
            crate::validate::tmux_name_lookup(name)?;
            s.get_session(name, host)?.ok_or_else(|| {
                IpcError::new("E_NOTFOUND", format!("session {name} not found on {host}"))
            })
        }
        _ => Err(IpcError::new(
            "E_INVALID",
            "pass session_id, or both host_alias and tmux_name",
        )),
    }
}

/// `whoami` for an in-session agent (MCP-6): the ONE fleet row whose
/// `tmux_name` matches. Default names are project-derived, so the same name
/// can exist on several hosts — that is `E_AMBIGUOUS`, with the candidates'
/// `(session_id, host_alias)` in `details` so the caller can retry with
/// `session_id`.
pub fn find_session_by_tmux_name(s: &Store, tmux_name: &str) -> Result<SessionRow, IpcError> {
    crate::validate::tmux_name_lookup(tmux_name)?;
    let all: Vec<SessionRow> = s
        .list_all_sessions()?
        .into_iter()
        .filter(|r| r.tmux_name == tmux_name)
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
            "E_NOTFOUND",
            format!("no session named {tmux_name} on any host"),
        )),
        1 => Ok(matches.into_iter().next().expect("one match")),
        _ => {
            let candidates: Vec<serde_json::Value> = matches
                .iter()
                .map(|r| serde_json::json!({ "session_id": r.id, "host_alias": r.host_alias }))
                .collect();
            Err(IpcError::new(
                "E_AMBIGUOUS",
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
                "E_SELF_TARGET",
                format!(
                    "{name} on {host} is the registered fleet controller; \
                     pass force=true to target it anyway"
                ),
            ));
        }
    }
    Ok(())
}
