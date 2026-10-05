//! Claiming an `unclaimed` session for a person (multi-user M1, T12).
//!
//! `unclaimed` is the safe holding state: the row a reconcile pass found
//! rather than one fleet started, so nobody can speak for it and nobody
//! learns anything about it — an out-of-scope caller is told a per-host COUNT
//! and nothing else (spec §4.3). This is how such a row stops being nobody's.
//!
//! **What authorises the claim is the pane, not the org and not the host.**
//! The caller is the agent inside the session, and it proves that by the
//! `X-Fleet-Pane` header its MCP entry carries: resolved once per request
//! into `ViewScope::proven_session` through
//! `Store::find_session_by_pane`, which filters on the host, excludes ghosts
//! and answers `None` on an ambiguous pane. So the check below is "the row
//! this request proves it is standing in IS the row it named", and a host
//! token that merely happens to be on the same machine is refused.
//!
//! **What that proof is worth, stated honestly.** Any process that can run
//! `tmux list-panes` on the host can enumerate every pane id there, so
//! presenting one proves HOST ACCESS, not pane occupancy — and under one
//! shared unix account behind one fleet alias the ids are small sequential
//! `%N` and the proof is guessable. The rule it rests on is one fleet host
//! ALIAS per unix account, hence one token each; `docs/hub.md` says so in
//! those words and this module does not claim more.

use super::*;
use crate::ipc_error::codes;
use crate::service::view_scope::ViewScope;

/// The timeline kind a claim writes. Recorded **quietly**: the loud writer
/// fans a `session:event` frame to every connected client, which for a row
/// that was invisible a moment ago would announce its existence — and its
/// host and id — to everyone, which is the leak the quiet variant's own doc
/// comment exists for.
pub const EVENT_CLAIMED: &str = "session_claimed";

/// Claim `session_id` for the person named `person`, as the agent whose pane
/// `scope` proves.
///
/// `by` is the caller's label (`Caller::label` — `host:<alias>`), for the
/// timeline detail. It names a machine, never a session or a pane.
///
/// Four refusals, each a different thing to do about it:
///
/// * **`E_NOTFOUND`** — this caller may not see the row at all. Exactly what
///   an id that does not exist answers, so neither is an existence oracle.
/// * **`E_INVALID_STATE`** — the row is there and this request proves no pane
///   of it. The everyday case: the claim has to run from the session's ACTIVE
///   pane, because `sessions.tmux_pane_id` is the pane the last reconcile
///   pass saw, so an agent in another split of the same window presents a
///   pane no row carries. Deliberately not `E_NOTFOUND`, which would send the
///   operator hunting a row they can see in `fleet-hub session unclaimed`.
/// * **`E_FORBIDDEN`** — the request proves a pane, and it is a DIFFERENT
///   row's. The caller is standing somewhere else and naming this one.
/// * **`E_EXISTS`** — the row already belongs to a person. A claim never
///   transfers ownership; the owner shares instead.
pub fn claim_session(
    s: &Store,
    session_id: i64,
    person: &str,
    scope: &ViewScope,
    by: &str,
) -> Result<SessionRow, IpcError> {
    let row = s.get_session_by_id(session_id)?.ok_or_else(|| {
        IpcError::new(codes::E_NOTFOUND, format!("session {session_id} not found"))
    })?;
    // Re-asked here even though the tool's gate already asked it: this
    // function is the one that writes an owner, and a later call site that
    // forgets the gate must not be the thing that makes it unguarded.
    if !scope.sees_session_row(&row).is_visible() {
        return Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("session {session_id} not found"),
        ));
    }
    // `Some(session_id)` on the right-hand side, so the `(None, None)` trap
    // has nowhere to live: a request that proves no pane matches no row.
    if scope.proven_session != Some(session_id) {
        return Err(match scope.proven_session {
            Some(other) => IpcError::new(
                codes::E_FORBIDDEN,
                format!(
                    "this request's pane is session {other}, not {session_id}: a claim is \
                     filed from the session's own pane, and proving one pane is not \
                     authority over another"
                ),
            ),
            None => IpcError::new(
                codes::E_INVALID_STATE,
                format!(
                    "session {session_id} is on this host but this request proves no pane \
                     of it: the claim runs from the session's ACTIVE pane, whose id fleet \
                     recorded (another split of the same window presents a pane no row \
                     carries, and so does a pane the last reconcile pass has not seen \
                     yet). Attach to the session and claim from there, or use fleet-hub \
                     session claim on the hub machine"
                ),
            ),
        });
    }
    if let Some(owner) = row.owner_person_id {
        return Err(IpcError::new(
            codes::E_EXISTS,
            format!(
                "session {session_id} already belongs to person {owner}; a claim never \
                 transfers ownership — its owner shares it instead"
            ),
        ));
    }
    let to = super::sharing::person_named(s, person)?;
    write_claim(s, session_id, to, by)
}

/// The write both claim paths share: the owner, `private`, and the quiet
/// timeline row.
///
/// `fleet-hub session claim` reaches it without a [`ViewScope`] — the
/// operator's authority is shell access to the hub machine, which §4.5
/// already concedes, and there is no pane to prove — so the checks above are
/// the TOOL's and this is the writing half. It still never re-owns: the
/// ownership rule is `claim_if_unclaimed`'s own `WHERE owner_person_id IS
/// NULL`, which no caller can talk past.
pub fn write_claim(
    s: &Store,
    session_id: i64,
    person: i64,
    by: &str,
) -> Result<SessionRow, IpcError> {
    if !s.claim_if_unclaimed(session_id, Some(person))? {
        // `claim_if_unclaimed` answers `Ok(false)` for "already exactly this
        // person's" and raises for everything else, so this is the idempotent
        // re-entry rather than a silent failure. Still not a success the
        // timeline should record twice.
        return row_now(s, session_id);
    }
    s.insert_session_event_quietly(
        session_id,
        None,
        EVENT_CLAIMED,
        Some(&format!("person={person} by={by}")),
    )?;
    row_now(s, session_id)
}

fn row_now(s: &Store, session_id: i64) -> Result<SessionRow, IpcError> {
    s.get_session_by_id(session_id)?.ok_or_else(|| {
        IpcError::new(
            codes::E_NOTFOUND,
            format!("session {session_id} was removed while it was being claimed"),
        )
    })
}
