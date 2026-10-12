//! Access requests (gap plan G4.2, migration 160): a person a session is
//! shared with asks its owner for a wider level — "Ask Martin for Answer" on
//! the Watch board.
//!
//! A request confers nothing. It is a note to the owner, and the owner
//! answers it with the operations the owner already has
//! (`store/session_grants.rs`):
//!
//! * **granting** revokes the asker's own grant (if any) and writes a fresh
//!   one at the asked level, in one transaction — the "revoke and share
//!   again" route that invariant 3 names, so a live grant still never widens;
//! * **declining** only stamps the request.
//!
//! The rules, each a clause of the statement that writes or a check before it:
//!
//! 1. Only someone the session is already shared with may ask (any level,
//!    through a person or an org grant), and only for a level wider than the
//!    one they hold. A stranger has nothing to ask about; asking for `watch`
//!    or `own` is refused (the CHECK says so too).
//! 2. One open request per (session, person) — `idx_access_requests_open`.
//! 3. A declined ask holds a repeat back for [`ACCESS_REQUEST_COOLDOWN_SECS`],
//!    so an owner is not asked again the moment they said no.
//! 4. Only the session's owner answers, and the owner comparison is in the
//!    statement (`sessions.owner_person_id = ?owner`), as every grant write
//!    is. A request whose asker no longer holds any grant is not shown to the
//!    owner and cannot be granted: the owner took that person's access away,
//!    and a grant from an old note would undo it without them noticing.
//!
//! Every change announces one `grant:changed` frame for the asker, which the
//! hub fences to exactly the asker and the owner: `level` is the asker's level
//! now in force, and `request` the level of a request that just opened.

use super::session_grants::{bump_grant_generation, grant_rank};
use super::{now_unix, Store, GRANT_ANSWER, GRANT_DRIVE};
use crate::events::EventBus;
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;

/// How long a declined ask holds back the same person asking again about the
/// same session.
pub const ACCESS_REQUEST_COOLDOWN_SECS: i64 = 60 * 60;

/// One request.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AccessRequestRow {
    pub id: i64,
    pub session_id: i64,
    /// Who asks.
    pub person_id: i64,
    /// The level asked for: `answer` or `drive`.
    pub level: String,
    pub requested_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<i64>,
    /// `granted`, `declined` or `withdrawn`, once resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<i64>,
}

const COLS: &str =
    "id, session_id, person_id, level, requested_at, resolved_at, resolution, resolved_by";

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AccessRequestRow> {
    Ok(AccessRequestRow {
        id: r.get(0)?,
        session_id: r.get(1)?,
        person_id: r.get(2)?,
        level: r.get(3)?,
        requested_at: r.get(4)?,
        resolved_at: r.get(5)?,
        resolution: r.get(6)?,
        resolved_by: r.get(7)?,
    })
}

fn is_unique_violation(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::ConstraintViolation,
                extended_code: rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE,
            },
            _
        )
    )
}

/// A level someone may ask for: wider than `watch`, never `own`.
fn askable_level(level: &str) -> Result<&'static str, IpcError> {
    match level {
        GRANT_ANSWER => Ok(GRANT_ANSWER),
        GRANT_DRIVE => Ok(GRANT_DRIVE),
        other => Err(IpcError::new(
            codes::E_VALIDATE,
            format!("ask for answer or drive, not {other:?}: a share starts at watch, and own is never granted"),
        )),
    }
}

impl Store {
    /// `person` asks the owner of `session_id` for `level`. See the module
    /// header for the rules. `E_FORBIDDEN` when the session is not shared
    /// with them, `E_VALIDATE` for a level that is not wider than theirs (or
    /// not askable, or their own session), `E_EXISTS` while an ask waits,
    /// `E_INVALID_STATE` within the cooldown after a decline.
    pub fn request_access(
        &self,
        session_id: i64,
        person: i64,
        level: &str,
    ) -> Result<AccessRequestRow, IpcError> {
        let level = askable_level(level)?;
        if self.session_is_owned_by(session_id, person)? {
            return Err(IpcError::new(
                codes::E_VALIDATE,
                format!("session {session_id} is yours: there is nobody to ask"),
            ));
        }
        let Some(current) = self.grants_for_person(person)?.remove(&session_id) else {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                format!(
                    "session {session_id} is not shared with person {person}; only someone it \
                     is shared with can ask its owner for more"
                ),
            ));
        };
        if grant_rank(level) <= grant_rank(&current) {
            return Err(IpcError::new(
                codes::E_VALIDATE,
                format!("you already hold {current} on session {session_id}"),
            ));
        }
        let now = now_unix();
        let declined_at: Option<i64> = self.conn.query_row(
            "SELECT MAX(resolved_at) FROM access_requests \
              WHERE session_id = ?1 AND person_id = ?2 AND resolution = 'declined'",
            rusqlite::params![session_id, person],
            |r| r.get(0),
        )?;
        if let Some(at) = declined_at {
            if now - at < ACCESS_REQUEST_COOLDOWN_SECS {
                let mins = (ACCESS_REQUEST_COOLDOWN_SECS - (now - at) + 59) / 60;
                return Err(IpcError::new(
                    codes::E_INVALID_STATE,
                    format!(
                        "the owner declined your last ask on session {session_id}; you can ask \
                         again in {mins} min"
                    ),
                ));
            }
        }
        self.conn
            .execute(
                "INSERT INTO access_requests (session_id, person_id, level, requested_at) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![session_id, person, level, now],
            )
            .map_err(|e| {
                if is_unique_violation(&e) {
                    IpcError::new(
                        codes::E_EXISTS,
                        format!(
                            "you already asked for more on session {session_id}; the owner has \
                             not answered yet"
                        ),
                    )
                } else {
                    IpcError::from(e)
                }
            })?;
        let row = self
            .access_request(self.conn.last_insert_rowid())?
            .ok_or_else(|| {
                IpcError::new(
                    codes::E_INTERNAL,
                    "an access request vanished right after it was written",
                )
            })?;
        self.bus.grant_changed(&crate::events::GrantChanged {
            session_id,
            person_id: person,
            level: Some(current),
            request: Some(level.to_string()),
        });
        Ok(row)
    }

    /// One request by id, whatever its state.
    pub fn access_request(&self, id: i64) -> Result<Option<AccessRequestRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLS} FROM access_requests WHERE id = ?1"),
                [id],
                map_row,
            )
            .optional()?)
    }

    /// The open requests on sessions `owner` owns — on `session_id` only when
    /// given — oldest first, leaving out any whose asker no longer holds a
    /// grant or already holds the level asked (rule 4).
    pub fn open_access_requests_for_owner(
        &self,
        owner: i64,
        session_id: Option<i64>,
    ) -> Result<Vec<AccessRequestRow>, IpcError> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {} FROM access_requests r JOIN sessions s ON s.id = r.session_id \
              WHERE r.resolved_at IS NULL AND s.owner_person_id = ?1 \
                AND (?2 IS NULL OR r.session_id = ?2) \
              ORDER BY r.requested_at, r.id",
            COLS.split(", ")
                .map(|c| format!("r.{c}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))?;
        let rows = st
            .query_map(rusqlite::params![owner, session_id], map_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            if self.ask_still_stands(&r)?.is_some() {
                out.push(r);
            }
        }
        Ok(out)
    }

    /// `person`'s own open requests, oldest first (`my_grants`).
    pub fn open_access_requests_by(&self, person: i64) -> Result<Vec<AccessRequestRow>, IpcError> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {COLS} FROM access_requests \
              WHERE person_id = ?1 AND resolved_at IS NULL ORDER BY requested_at, id"
        ))?;
        let rows = st.query_map([person], map_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The asker's level now, when the ask still means something: they hold
    /// a grant, and it is narrower than the level asked.
    fn ask_still_stands(&self, r: &AccessRequestRow) -> Result<Option<String>, IpcError> {
        let current = self.grants_for_person(r.person_id)?.remove(&r.session_id);
        Ok(current.filter(|c| grant_rank(c) < grant_rank(&r.level)))
    }

    /// The owner answers request `id`: `grant` gives the asker the asked
    /// level, otherwise it is declined. `E_NOTFOUND` for no such open request
    /// on a session `owner` owns (one answer for both, so the id is no oracle),
    /// `E_INVALID_STATE` when the ask no longer stands (rule 4; it is closed
    /// as declined), `E_VALIDATE` when the asker has been disabled.
    pub fn resolve_access_request(
        &self,
        id: i64,
        owner: i64,
        grant: bool,
    ) -> Result<AccessRequestRow, IpcError> {
        let not_found = || {
            IpcError::new(
                codes::E_NOTFOUND,
                format!("no open access request {id} on a session of yours"),
            )
        };
        let r = self.access_request(id)?.ok_or_else(not_found)?;
        if r.resolved_at.is_some() || !self.session_is_owned_by(r.session_id, owner)? {
            return Err(not_found());
        }
        let stands = self.ask_still_stands(&r)?;
        let now = now_unix();
        if grant && stands.is_some() {
            self.check_person_recipient(r.session_id, r.person_id, owner)?;
            let tx = self.conn.unchecked_transaction()?;
            // The owner's revoke and fresh share, in one transaction: a live
            // grant is never widened in place (session_grants invariant 3).
            // Both statements carry the ownership clause themselves.
            tx.execute(
                "UPDATE session_grants SET revoked_at = ?3 \
                  WHERE session_id = ?1 AND person_id = ?2 AND revoked_at IS NULL \
                    AND EXISTS (SELECT 1 FROM sessions \
                                 WHERE sessions.id = session_grants.session_id \
                                   AND sessions.owner_person_id = ?4)",
                rusqlite::params![r.session_id, r.person_id, now, owner],
            )?;
            let inserted = tx.execute(
                "INSERT INTO session_grants \
                   (session_id, person_id, org_id, level, granted_by, granted_at) \
                 SELECT ?1, ?2, NULL, ?3, ?4, ?5 FROM sessions \
                  WHERE sessions.id = ?1 AND sessions.owner_person_id = ?4",
                rusqlite::params![r.session_id, r.person_id, r.level, owner, now],
            )?;
            let closed = tx.execute(
                "UPDATE access_requests SET resolved_at = ?2, resolution = 'granted', \
                        resolved_by = ?3 \
                  WHERE id = ?1 AND resolved_at IS NULL",
                rusqlite::params![id, now, owner],
            )?;
            if inserted != 1 || closed != 1 {
                // Dropped without a commit: nothing above happened.
                return Err(not_found());
            }
            tx.commit()?;
            bump_grant_generation();
            let level = self.grants_for_person(r.person_id)?.remove(&r.session_id);
            self.announce_grant_change(&[crate::events::GrantChanged {
                session_id: r.session_id,
                person_id: r.person_id,
                level,
                request: None,
            }])?;
        } else {
            // A stale ask (rule 4) is closed as withdrawn, not declined: the
            // owner said nothing about it, so it starts no cooldown.
            let resolution = if grant { "withdrawn" } else { "declined" };
            let closed = self.conn.execute(
                "UPDATE access_requests SET resolved_at = ?2, resolution = ?4, \
                        resolved_by = ?3 \
                  WHERE id = ?1 AND resolved_at IS NULL \
                    AND EXISTS (SELECT 1 FROM sessions \
                                 WHERE sessions.id = access_requests.session_id \
                                   AND sessions.owner_person_id = ?3)",
                rusqlite::params![id, now, owner, resolution],
            )?;
            if closed != 1 {
                return Err(not_found());
            }
            let level = self.grants_for_person(r.person_id)?.remove(&r.session_id);
            self.bus.grant_changed(&crate::events::GrantChanged {
                session_id: r.session_id,
                person_id: r.person_id,
                level,
                request: None,
            });
            if grant {
                return Err(IpcError::new(
                    codes::E_INVALID_STATE,
                    format!(
                        "person {} no longer holds a share of session {} narrower than {}, so \
                         the ask was closed; share again if you still want to",
                        r.person_id, r.session_id, r.level
                    ),
                ));
            }
        }
        self.access_request(id)?.ok_or_else(not_found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::RecordingEventBus;
    use crate::store::{GrantRecipient, GRANT_WATCH};
    use std::sync::Arc;

    fn share(s: &Store, session: i64, person: i64, level: &str, owner: i64) {
        s.grant_session(session, GrantRecipient::Person(person), level, owner)
            .expect("share");
    }

    struct Fx {
        s: Store,
        bus: Arc<RecordingEventBus>,
        ann: i64,
        bob: i64,
        cleo: i64,
        session: i64,
    }

    /// `ann` owns one session, shared with `bob` at watch; `cleo` holds
    /// nothing.
    fn fx() -> Fx {
        let bus = Arc::new(RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone() as Arc<dyn EventBus>).expect("store");
        let ann = s.create_person("ann", None).expect("ann").id;
        let bob = s.create_person("bob", None).expect("bob").id;
        let cleo = s.create_person("cleo", None).expect("cleo").id;
        s.conn
            .execute("INSERT OR IGNORE INTO hosts (alias) VALUES ('h')", [])
            .expect("host");
        s.conn
            .execute(
                "INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, \
                                       status, started_at, owner_person_id, visibility) \
                 VALUES ('work', 'h', 1, 1, 'running', 1, ?1, 'private')",
                [ann],
            )
            .expect("session");
        let session = s.conn.last_insert_rowid();
        share(&s, session, bob, GRANT_WATCH, ann);
        bus.take();
        Fx {
            s,
            bus,
            ann,
            bob,
            cleo,
            session,
        }
    }

    fn level_of(fx: &Fx, person: i64) -> Option<String> {
        fx.s.grants_for_person(person).unwrap().remove(&fx.session)
    }

    #[test]
    fn a_watcher_asks_and_the_owner_hears_it() {
        let fx = fx();
        let r =
            fx.s.request_access(fx.session, fx.bob, GRANT_ANSWER)
                .expect("ask");
        assert_eq!(r.level, "answer");
        assert_eq!(
            fx.bus.take(),
            vec![format!(
                "grant:changed:{}:{}:watch:asks:answer",
                fx.session, fx.bob
            )],
            "one frame, for the asker and (through the fence) the owner"
        );
        let open = fx.s.open_access_requests_for_owner(fx.ann, None).unwrap();
        assert_eq!(
            open.iter().map(|r| r.person_id).collect::<Vec<_>>(),
            vec![fx.bob]
        );
        assert_eq!(fx.s.open_access_requests_by(fx.bob).unwrap().len(), 1);
        assert!(
            fx.s.open_access_requests_for_owner(fx.bob, None)
                .unwrap()
                .is_empty(),
            "the asker is not the owner and sees no owner list"
        );
        // A request confers nothing.
        assert_eq!(level_of(&fx, fx.bob).as_deref(), Some("watch"));
    }

    #[test]
    fn only_a_recipient_asks_and_only_for_more() {
        let fx = fx();
        let e =
            fx.s.request_access(fx.session, fx.cleo, GRANT_ANSWER)
                .unwrap_err();
        assert_eq!(
            e.code,
            codes::E_FORBIDDEN,
            "a stranger has nothing to ask about"
        );
        let e =
            fx.s.request_access(fx.session, fx.ann, GRANT_DRIVE)
                .unwrap_err();
        assert_eq!(e.code, codes::E_VALIDATE, "the owner asks nobody");
        for bad in ["watch", "own", "admin"] {
            let e = fx.s.request_access(fx.session, fx.bob, bad).unwrap_err();
            assert_eq!(e.code, codes::E_VALIDATE, "{bad} is not askable");
        }
        fx.s.request_access(fx.session, fx.bob, GRANT_DRIVE)
            .expect("ask");
        let e =
            fx.s.request_access(fx.session, fx.bob, GRANT_ANSWER)
                .unwrap_err();
        assert_eq!(
            e.code,
            codes::E_EXISTS,
            "one open ask per person and session"
        );
    }

    #[test]
    fn granting_reshares_at_the_asked_level() {
        let fx = fx();
        let r =
            fx.s.request_access(fx.session, fx.bob, GRANT_ANSWER)
                .unwrap();
        fx.bus.take();
        let e = fx.s.resolve_access_request(r.id, fx.bob, true).unwrap_err();
        assert_eq!(e.code, codes::E_NOTFOUND, "only the owner answers");
        let done =
            fx.s.resolve_access_request(r.id, fx.ann, true)
                .expect("grant");
        assert_eq!(done.resolution.as_deref(), Some("granted"));
        assert_eq!(level_of(&fx, fx.bob).as_deref(), Some("answer"));
        assert_eq!(
            fx.bus.take(),
            vec![
                format!("session:updated:{}", fx.session),
                format!("grant:changed:{}:{}:answer", fx.session, fx.bob),
            ]
        );
        // The old grant was revoked, not widened: two rows, one live.
        let rows: (i64, i64) = fx
            .s
            .conn
            .query_row(
                "SELECT COUNT(*), SUM(revoked_at IS NULL) FROM session_grants WHERE person_id = ?1",
                [fx.bob],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(rows, (2, 1));
        let e = fx.s.resolve_access_request(r.id, fx.ann, true).unwrap_err();
        assert_eq!(
            e.code,
            codes::E_NOTFOUND,
            "an answered ask is answered once"
        );
        assert!(fx
            .s
            .open_access_requests_for_owner(fx.ann, None)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_decline_holds_the_next_ask_back() {
        let fx = fx();
        let r =
            fx.s.request_access(fx.session, fx.bob, GRANT_DRIVE)
                .unwrap();
        fx.bus.take();
        let done =
            fx.s.resolve_access_request(r.id, fx.ann, false)
                .expect("decline");
        assert_eq!(done.resolution.as_deref(), Some("declined"));
        assert_eq!(level_of(&fx, fx.bob).as_deref(), Some("watch"));
        assert_eq!(
            fx.bus.take(),
            vec![format!("grant:changed:{}:{}:watch", fx.session, fx.bob)],
            "the asker hears the answer"
        );
        let e =
            fx.s.request_access(fx.session, fx.bob, GRANT_DRIVE)
                .unwrap_err();
        assert_eq!(e.code, codes::E_INVALID_STATE);
        // Once the cooldown has passed, they may ask again.
        fx.s.conn
            .execute(
                "UPDATE access_requests SET resolved_at = resolved_at - ?1",
                [ACCESS_REQUEST_COOLDOWN_SECS + 1],
            )
            .unwrap();
        fx.s.request_access(fx.session, fx.bob, GRANT_DRIVE)
            .expect("ask again");
    }

    #[test]
    fn a_revoked_asker_is_not_granted_from_an_old_note() {
        let fx = fx();
        let r =
            fx.s.request_access(fx.session, fx.bob, GRANT_DRIVE)
                .unwrap();
        fx.s.revoke_session_grant(fx.session, fx.bob, fx.ann)
            .unwrap();
        assert!(
            fx.s.open_access_requests_for_owner(fx.ann, None)
                .unwrap()
                .is_empty(),
            "the owner took the access away; the ask is not shown"
        );
        let e = fx.s.resolve_access_request(r.id, fx.ann, true).unwrap_err();
        assert_eq!(e.code, codes::E_INVALID_STATE);
        assert_eq!(level_of(&fx, fx.bob), None, "nothing was granted");
        assert_eq!(
            fx.s.access_request(r.id)
                .unwrap()
                .unwrap()
                .resolution
                .as_deref(),
            Some("withdrawn"),
            "the stale ask is closed, with no cooldown"
        );
    }

    #[test]
    fn grant_details_name_who_shared_and_when() {
        let fx = fx();
        let d = fx.s.grant_details_for_person(fx.bob).unwrap();
        let g = d.get(&fx.session).expect("detail");
        assert_eq!(
            (g.level.as_str(), g.granted_by, g.org_id),
            ("watch", fx.ann, None)
        );
    }

    #[test]
    fn a_deleted_session_takes_its_requests_with_it() {
        let fx = fx();
        fx.s.request_access(fx.session, fx.bob, GRANT_ANSWER)
            .unwrap();
        fx.s.conn
            .execute("DELETE FROM sessions WHERE id = ?1", [fx.session])
            .unwrap();
        let n: i64 =
            fx.s.conn
                .query_row("SELECT COUNT(*) FROM access_requests", [], |r| r.get(0))
                .unwrap();
        assert_eq!(n, 0);
    }
}
