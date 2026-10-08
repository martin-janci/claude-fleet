//! Lost and found: adopting a tmux pane that was started outside fleet
//! (redesign step 4.8).
//!
//! Reconcile already records every tmux session it finds on a host, so a
//! pane somebody started by hand (`tmux new -s scratch; claude`) has a row.
//! What it lacks is fleet's word that it is fleet's: `started_at` is NULL
//! ("since tmux created the session, fleet did not start it"), and on a hub
//! it is `unclaimed`. Adopting is that word, given by a person: the row gets
//! `started_at` (fleet runs it from now on), the adopter as its owner when it
//! has none, and an `session_adopted` line on its timeline. Nothing on the
//! host changes: the pane, its process and its tmux name stay as they are.
//!
//! Restore (a conversation whose pane is gone) stays `restore_host_sessions`
//! and `discover_lost_sessions`; adopting is only for a pane that is alive.

use super::*;
use crate::ipc_error::{codes, lock};

/// The timeline kind an adoption writes. Quiet, like a claim
/// ([`EVENT_CLAIMED`]): the row may have been `unclaimed` a moment ago, and
/// the loud writer would announce it to every connected client.
pub const EVENT_ADOPTED: &str = "session_adopted";

#[derive(Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct AdoptSessionArgs {
    /// Fleet session id of a live tmux session fleet did not start
    /// (`started_at` is null).
    pub session_id: i64,
    /// Who adopts it: the row's owner when it has none. Set in Rust from the
    /// connection (`mcp::tools::fleet::owner_for`), never read from the
    /// request, exactly like `NewSessionArgs::owner_person_id`; skipped both
    /// ways, so a hub client never sends it either.
    #[serde(skip)]
    pub owner_person_id: Option<i64>,
}

/// True for a row fleet would list under Lost and found as "outside fleet":
/// a pane-backed session that is alive and that fleet did not start.
pub fn is_outside_fleet(row: &SessionRow) -> bool {
    row.started_at.is_none()
        && row.status != "ghost"
        && row.lost_at.is_none()
        && !matches!(row.kind.as_str(), "bg" | "external")
}

/// Adopt `args.session_id`: see the module header. Refuses with
/// `E_NOTFOUND` for no such row, and `E_INVALID_STATE` for a row that is not
/// [`is_outside_fleet`] (already fleet's, lost, or not a tmux pane), naming
/// what to do instead.
pub fn adopt_session(args: AdoptSessionArgs, store: &Mutex<Store>) -> Result<SessionRow, IpcError> {
    let s = lock(store)?;
    let row = s.get_session_by_id(args.session_id)?.ok_or_else(|| {
        IpcError::new(
            codes::E_NOTFOUND,
            format!("session {} not found", args.session_id),
        )
    })?;
    if !is_outside_fleet(&row) {
        let why = if row.status == "ghost" || row.lost_at.is_some() {
            "its pane is gone; restore it instead (restore_host_sessions)"
        } else if matches!(row.kind.as_str(), "bg" | "external") {
            "it runs outside tmux, so there is no pane to adopt"
        } else {
            "fleet already runs it"
        };
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("session {} cannot be adopted: {why}", row.id),
        ));
    }
    let id = row.id;
    s.atomically(|s| {
        s.set_started_at(id, now_unix())?;
        s.claim_if_unclaimed(id, args.owner_person_id)?;
        let detail = args.owner_person_id.map(|p| format!("person={p}"));
        s.insert_session_event_quietly(id, None, EVENT_ADOPTED, detail.as_deref())?;
        Ok(())
    })?;
    s.get_session_by_id(id)?.ok_or_else(|| {
        IpcError::new(
            codes::E_NOTFOUND,
            format!("session {id} was removed while it was being adopted"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> (Mutex<Store>, i64) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("scratch", "local", None, None, 1, 1, "running", None)
            .unwrap();
        (Mutex::new(s), id)
    }

    fn adopt(store: &Mutex<Store>, id: i64, owner: Option<i64>) -> Result<SessionRow, IpcError> {
        adopt_session(
            AdoptSessionArgs {
                session_id: id,
                owner_person_id: owner,
            },
            store,
        )
    }

    #[test]
    fn adopting_a_pane_started_outside_fleet_makes_it_the_persons() {
        let (store, id) = seeded();
        let ada = lock(&store).unwrap().create_person("ada", None).unwrap().id;
        let before = lock(&store)
            .unwrap()
            .get_session_by_id(id)
            .unwrap()
            .unwrap();
        assert!(
            is_outside_fleet(&before),
            "a reconcile-found row is outside fleet"
        );
        assert_eq!(before.owner_person_id, None);

        let row = adopt(&store, id, Some(ada)).unwrap();
        assert!(row.started_at.is_some(), "fleet runs it from now on");
        assert_eq!(row.owner_person_id, Some(ada), "the adopter owns it");
        assert!(!is_outside_fleet(&row));
        let events = lock(&store).unwrap().list_session_events(id, 10).unwrap();
        assert!(
            events.iter().any(|e| e.kind == EVENT_ADOPTED
                && e.detail.as_deref() == Some(&format!("person={ada}")[..])),
            "the timeline records who adopted it: {events:?}"
        );
    }

    #[test]
    fn adopting_twice_is_refused_and_never_re_owns() {
        let (store, id) = seeded();
        let (ada, bob) = {
            let s = lock(&store).unwrap();
            (
                s.create_person("ada", None).unwrap().id,
                s.create_person("bob", None).unwrap().id,
            )
        };
        adopt(&store, id, Some(ada)).unwrap();
        let err = adopt(&store, id, Some(bob)).unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE);
        assert!(err.message.contains("already runs it"), "{}", err.message);
        let row = lock(&store)
            .unwrap()
            .get_session_by_id(id)
            .unwrap()
            .unwrap();
        assert_eq!(row.owner_person_id, Some(ada));
    }

    #[test]
    fn a_row_fleet_started_or_lost_or_paneless_is_not_adoptable() {
        let (store, id) = seeded();
        lock(&store).unwrap().set_started_at(id, 5).unwrap();
        assert_eq!(
            adopt(&store, id, None).unwrap_err().code,
            codes::E_INVALID_STATE
        );

        let (store, id) = seeded();
        lock(&store)
            .unwrap()
            .conn_ref()
            .execute("UPDATE sessions SET status='ghost' WHERE id=?1", [id])
            .unwrap();
        let err = adopt(&store, id, None).unwrap_err();
        assert!(
            err.message.contains("restore it instead"),
            "{}",
            err.message
        );

        let (store, id) = seeded();
        lock(&store)
            .unwrap()
            .conn_ref()
            .execute("UPDATE sessions SET kind='external' WHERE id=?1", [id])
            .unwrap();
        let err = adopt(&store, id, None).unwrap_err();
        assert!(err.message.contains("outside tmux"), "{}", err.message);

        let (store, _) = seeded();
        assert_eq!(
            adopt(&store, 999, None).unwrap_err().code,
            codes::E_NOTFOUND
        );
    }

    #[test]
    fn adopting_without_a_person_leaves_the_row_unclaimed() {
        let (store, id) = seeded();
        let row = adopt(&store, id, None).unwrap();
        assert!(row.started_at.is_some());
        assert_eq!(row.owner_person_id, None);
    }
}
