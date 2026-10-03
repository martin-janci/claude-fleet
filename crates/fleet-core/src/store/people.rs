//! People (multi-user M1, migration 094): the humans one hub knows, and
//! which person a paired device belongs to.
//!
//! Before M1 a hub knew token KINDS, not people — the master, a paired
//! client, a per-host token — so two colleagues paired to the same hub were
//! indistinguishable from one person with two phones. A `people` row is the
//! human; `client_tokens.person_id` is the device pointing at one.
//!
//! **The personal owner.** Every hub has exactly one, found by the
//! `is_personal_owner` flag under a partial unique index — never by name
//! (renameable, see [`Store::rename_person`]) and never by "the lowest id"
//! (an accident of insertion order). [`Store::personal_owner_id`] is one
//! index seek, and it answers `None` — never a substitute row — when the
//! flagged row is absent. Every caller treats that `None` as fail-closed:
//! a hub with no personal owner refuses session reads rather than serving
//! them to everybody, which is loud, recoverable and safe.
//!
//! **Only two writers move the flag:** migration 094 and
//! `Store::mint_personal_owner` (behind
//! `mcp::settings::ensure_personal_owner`). No public function here moves
//! it, and nothing re-homes it onto another row.

use super::{breaks_a_line, now_unix, Store};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;

/// Longest person name — the same bound `clients::MAX_CLIENT_NAME_LEN` puts
/// on a device name, for the same reason: a person's name is operator-facing
/// text that ends up in labels and, through a grant, in lines fleet writes
/// itself.
pub const MAX_PERSON_NAME_LEN: usize = 64;

/// The placeholder name migration 094 gives this hub's personal owner, and
/// the one `Store::mint_personal_owner` uses when the row is missing. It
/// is a placeholder and nothing else: nothing keys on it (see the module
/// docs), and the operator renames it whenever they like.
pub const PERSONAL_OWNER_NAME: &str = "owner";

/// What single-purpose machine a client token's `mode` names, phrased for an
/// operator, or `None` when the mode is a person's device (`full` /
/// `readonly`).
///
/// [`Store::set_client_person`] is the gate that refuses one; `fleet-hub
/// client bind-person` reads it BEFORE it creates anybody, so a refused bind
/// does not leave a `people` row behind. One sentence, two callers.
pub fn machine_token_kind(mode: &str) -> Option<&'static str> {
    match mode {
        "peer" => Some("a peer hub link"),
        "updater" => Some("an updater token"),
        _ => None,
    }
}

/// One person this hub knows.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PersonRow {
    pub id: i64,
    /// What the operator types and what a grant is addressed to. Unique
    /// among LIVE people (`idx_people_live_name`); a disabled person keeps
    /// their name for the audit trail without holding it against a new
    /// colleague.
    pub name: String,
    /// The profile layer (M2); `None` means "show `name`".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// This hub's own owner. Exactly one row can carry it
    /// (`idx_people_personal_owner`), and no function in this module moves
    /// it — see the module docs.
    pub is_personal_owner: bool,
    pub created_at: i64,
    /// When this person was disabled ([`Store::disable_person`]), or `None`
    /// while they are live. The row is never deleted: grants and (from
    /// migration 095) sessions point at it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_at: Option<i64>,
}

/// Every `PersonRow` column, in [`map_person`] order.
const PERSON_COLUMNS: &str = "id, name, display_name, is_personal_owner, created_at, disabled_at";

fn map_person(row: &rusqlite::Row) -> rusqlite::Result<PersonRow> {
    Ok(PersonRow {
        id: row.get(0)?,
        name: row.get(1)?,
        display_name: row.get(2)?,
        is_personal_owner: row.get::<_, i64>(3)? != 0,
        created_at: row.get(4)?,
        disabled_at: row.get(5)?,
    })
}

/// Check a person's name and return it as it should be STORED — trimmed.
///
/// The same rule [`super::validate_client_name`] applies to a device name,
/// and for the same reason: the name is operator-facing text that fleet
/// interpolates into lines of its own (a share notice, a session label, the
/// untrusted-content marker a grantee's prompt carries), and a CR, an LF or
/// one of the three line separators [`char::is_control`] misses could close
/// such a line early. The trim happens here rather than in each caller so a
/// row stored as `" ada "` cannot be one a grant addressed to `ada` misses.
pub fn validate_person_name(name: &str) -> Result<String, IpcError> {
    let invalid = |why: &str| {
        Err(IpcError::new(
            codes::E_VALIDATE,
            format!("person name {name:?} {why}"),
        ))
    };
    let trimmed = name.trim();
    let len = trimmed.chars().count();
    if len == 0 {
        return invalid("must not be empty");
    }
    if len > MAX_PERSON_NAME_LEN {
        return invalid(&format!(
            "is {len} characters; at most {MAX_PERSON_NAME_LEN} are allowed"
        ));
    }
    if trimmed.chars().any(breaks_a_line) {
        return invalid(
            "must not contain control characters or line separators \
             (a line break could split a marker line)",
        );
    }
    Ok(trimmed.to_string())
}

/// A display name as it should be stored: trimmed, one line, `None` when
/// blank. Bounded like the name itself — it is shown in the same places.
fn validate_display_name(display: Option<&str>) -> Result<Option<String>, IpcError> {
    let Some(d) = display.map(str::trim).filter(|d| !d.is_empty()) else {
        return Ok(None);
    };
    if d.chars().count() > MAX_PERSON_NAME_LEN || d.chars().any(breaks_a_line) {
        return Err(IpcError::new(
            codes::E_VALIDATE,
            format!("a display name is 1–{MAX_PERSON_NAME_LEN} characters, one line"),
        ));
    }
    Ok(Some(d.to_string()))
}

fn person_not_found(id: i64) -> IpcError {
    IpcError::new(codes::E_NOTFOUND, format!("person {id} not found"))
}

impl Store {
    /// Create a person. `E_EXISTS` when a LIVE person already holds the name
    /// (a disabled one does not block reuse — see `idx_people_live_name`).
    ///
    /// Never sets `is_personal_owner`: the flag is written by migration 094
    /// and by `Self::mint_personal_owner`, and by nothing else. A colleague
    /// added here is a second person, never a second owner — which the
    /// partial unique index would refuse anyway.
    pub fn create_person(
        &self,
        name: &str,
        display_name: Option<&str>,
    ) -> Result<PersonRow, IpcError> {
        let name = validate_person_name(name)?;
        let display_name = validate_display_name(display_name)?;
        let taken: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM people WHERE name = ?1 AND disabled_at IS NULL)",
            rusqlite::params![name],
            |r| r.get(0),
        )?;
        if taken {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!("a person named {name:?} already exists"),
            ));
        }
        self.conn.execute(
            "INSERT INTO people (name, display_name, is_personal_owner, created_at) \
             VALUES (?1, ?2, 0, ?3)",
            rusqlite::params![name, display_name, now_unix()],
        )?;
        let id = self.conn.last_insert_rowid();
        self.get_person(id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "person vanished right after insert"))
    }

    /// One person by id, disabled or not.
    pub fn get_person(&self, id: i64) -> Result<Option<PersonRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {PERSON_COLUMNS} FROM people WHERE id = ?1"),
                rusqlite::params![id],
                map_person,
            )
            .optional()?)
    }

    /// The LIVE person holding `name`, if any. Live only, because that is
    /// the row `idx_people_live_name` guarantees is unique: a name that has
    /// been used by a departed colleague and then by a new one would
    /// otherwise be ambiguous, and resolving it to the departed row is the
    /// wrong half of that ambiguity in every caller.
    pub fn get_person_by_name(&self, name: &str) -> Result<Option<PersonRow>, IpcError> {
        let name = name.trim();
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {PERSON_COLUMNS} FROM people \
                     WHERE name = ?1 AND disabled_at IS NULL"
                ),
                rusqlite::params![name],
                map_person,
            )
            .optional()?)
    }

    /// Every person, live first and then disabled, each group by name. The
    /// disabled rows are included on purpose: they carry `disabled_at`, and
    /// a caller that wants only live people filters on it rather than
    /// discovering later that a grant points at a row the list omitted.
    pub fn list_people(&self) -> Result<Vec<PersonRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {PERSON_COLUMNS} FROM people \
             ORDER BY (disabled_at IS NOT NULL), name COLLATE NOCASE, id"
        ))?;
        let rows = stmt.query_map([], map_person)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Rename a live person, and/or set their display name (`Some("")`
    /// clears it). `E_NOTFOUND` for an unknown id, `E_EXISTS` when another
    /// live person holds the new name.
    ///
    /// The personal owner is renamed through here like anyone else — the
    /// placeholder `owner` migration 094 writes is meant to be replaced. The
    /// flag does not move with the name, which is the whole reason
    /// [`Self::personal_owner_id`] keys on the flag.
    pub fn rename_person(
        &self,
        id: i64,
        name: Option<&str>,
        display_name: Option<&str>,
    ) -> Result<PersonRow, IpcError> {
        let cur = self.get_person(id)?.ok_or_else(|| person_not_found(id))?;
        let name = match name {
            Some(n) => {
                let n = validate_person_name(n)?;
                let taken: bool = self.conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM people \
                     WHERE name = ?1 AND disabled_at IS NULL AND id <> ?2)",
                    rusqlite::params![n, id],
                    |r| r.get(0),
                )?;
                if taken {
                    return Err(IpcError::new(
                        codes::E_EXISTS,
                        format!("a person named {n:?} already exists"),
                    ));
                }
                n
            }
            None => cur.name.clone(),
        };
        let display = match display_name {
            Some(d) => validate_display_name(Some(d))?,
            None => cur.display_name.clone(),
        };
        self.conn.execute(
            "UPDATE people SET name = ?2, display_name = ?3 WHERE id = ?1",
            rusqlite::params![id, name, display],
        )?;
        self.get_person(id)?.ok_or_else(|| person_not_found(id))
    }

    /// Disable a person: they have left, or the operator is taking their
    /// reach away.
    ///
    /// **Compound on purpose, not a flag.** Nothing anywhere reads
    /// `people.disabled_at` on the request path —
    /// [`Store::active_client_tokens`] filters on `client_tokens.revoked_at`
    /// alone, and there is no join to `people` — so a bare stamp would
    /// "disable" a person while every device of theirs kept working, and
    /// would merely free their name for a colleague. So this also revokes
    /// **every live `client_tokens` row bound to them**, which bumps
    /// `auth_epoch` (migration 060) in the same transaction and therefore
    /// lands within the next request, whichever process wrote it.
    ///
    /// Each device is revoked through `clients::revoke_client_token_row`, the
    /// one function that knows what revoking means (the stamp AND the
    /// client's update rows, migration 079). A raw
    /// `UPDATE client_tokens SET revoked_at` here would have been a second,
    /// quietly weaker definition of the word.
    ///
    /// **And every grant TO them** (migration 096), in the same transaction,
    /// through `session_grants::revoke_live_grants_to_person`. A device is the
    /// door; a grant is the reach behind it, and a disabled person who keeps
    /// their shares is one re-enable — or one token minted for them by any
    /// later path — away from having it all back. Grants they MADE stay: those
    /// belong to the session's owner, who is somebody else by definition.
    ///
    /// **Not the personal owner.** `disabled_at` is deliberately outside
    /// `idx_people_personal_owner` (migration 094) so a disabled owner cannot
    /// free the slot for a second one — which also means
    /// [`Store::personal_owner_id`] would keep answering with a disabled row
    /// and `Caller::is_personal_owner` would keep being true for their
    /// devices. Disabling the hub's own owner is not an operation M1 has, and
    /// a fail-OPEN shape in the single row every gate keys on is exactly what
    /// revision 6's decision (k) says must fail closed — so it is refused
    /// with `E_VALIDATE`. Rename the owner, or revoke the devices one by one.
    ///
    /// **What becomes of their own sessions: nothing.** Their rows stay
    /// private and owned by them, unreadable by anyone else — exactly the
    /// spec's Q9 answer for a departure. Disabling a person removes their
    /// reach; it never re-attributes their work, and M1 has no operation
    /// that does. The row is kept for the same reason: grants and (from
    /// migration 095) sessions point at it, and an id nothing has is the
    /// fail-closed end of every one of those pointers.
    ///
    /// Idempotent: disabling an already-disabled person keeps the original
    /// stamp and revokes whatever tokens have since been minted for them.
    /// `E_NOTFOUND` for an unknown id.
    pub fn disable_person(&self, id: i64) -> Result<PersonRow, IpcError> {
        let row = self.get_person(id)?.ok_or_else(|| person_not_found(id))?;
        if row.is_personal_owner {
            return Err(IpcError::new(
                codes::E_VALIDATE,
                format!(
                    "person {id} ({:?}) is this hub's personal owner and cannot be \
                     disabled: every gate keys on that row, and a disabled owner \
                     would still answer `personal_owner_id()`",
                    row.name
                ),
            ));
        }
        let at = now_unix();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE people SET disabled_at = COALESCE(disabled_at, ?2) WHERE id = ?1",
            rusqlite::params![id, at],
        )?;
        // Half of "disabled" is that no device of theirs answers any more —
        // through the one function that knows what revoking a device means,
        // not a second `UPDATE` of our own.
        let devices: Vec<i64> = {
            let mut stmt = tx.prepare(
                "SELECT id FROM client_tokens WHERE person_id = ?1 AND revoked_at IS NULL",
            )?;
            let ids = stmt.query_map(rusqlite::params![id], |r| r.get(0))?;
            ids.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for device in devices {
            super::clients::revoke_client_token_row(&tx, device, at)?;
        }
        // The other half of "their reach is gone": every live grant TO them,
        // on anybody's session, in this SAME transaction. Without it a
        // disabled person keeps every share, and a token minted for them by
        // any later path — a re-enable, an operator's mistake, M2's
        // membership work — restores reach the operator believed they had
        // removed. Revoking the devices alone only closes the doors that
        // exist today.
        //
        // Grants they MADE are untouched, and so are their own sessions: a
        // grant is the session owner's, and `disable_person` has no authority
        // over somebody else's share (M1 gives no admin one at all). This is
        // not that authority either — it narrows, it names no session, and it
        // is reached only by disabling the recipient.
        super::session_grants::revoke_live_grants_to_person(&tx, id, at)?;
        tx.commit()?;
        self.get_person(id)?.ok_or_else(|| person_not_found(id))
    }

    /// This hub's personal owner, or `None`.
    ///
    /// One index seek on `idx_people_personal_owner`. It returns `Some(id)`
    /// when the flagged row exists and **`None` when it does not** — it
    /// never falls back to the lowest id, to the only live person, or to any
    /// other row. Every caller treats `None` as fail-closed and says so in
    /// its own code: the scope builder yields a scope that sees no session
    /// rather than all of them, the create paths leave a new row unclaimed
    /// rather than attributing it to a guess, and `Access::Person` refuses.
    ///
    /// The state is unreachable in practice —
    /// `mcp::settings::ensure_personal_owner` runs at every `fleet-hub`
    /// entry point that mints the master token — and is specified anyway,
    /// because the alternative to a defined answer is an invented one.
    pub fn personal_owner_id(&self) -> Result<Option<i64>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM people WHERE is_personal_owner = 1",
                [],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// The ONE live person on this hub, when there is exactly one.
    ///
    /// `Some(id)` only when `people` holds exactly one row with no
    /// `disabled_at`; `None` for a hub with none and for a hub with two or
    /// more. It never falls back to the personal owner on a two-person hub,
    /// which is the whole point: the question it answers is "is this fleet
    /// still a single-person install?", and an answer that degrades to
    /// "the owner" would turn a shared hub's reads back into a single
    /// person's (multi-user M1, spec §4.3, *Who sees the count*).
    ///
    /// Two callers, and they are the same rule seen twice. The scope builder
    /// (`Caller::view_scope`) stamps `ViewScope::is_sole_person` from it, so
    /// an `unclaimed` row stays visible to the one person who could already
    /// see it yesterday (rule 7: the upgrade widens nothing). And
    /// `service::hosts::list_hosts` serves `HostRow.unclaimed_sessions` only
    /// to that person — on a hub with two, the count reaches a human through
    /// `fleet-hub`, never through the API.
    ///
    /// `LIMIT 2`, and `None` for anything but exactly one row, is the same
    /// shape — and the same reason — as
    /// [`Store::find_session_by_pane`](crate::store::Store::find_session_by_pane):
    /// an ambiguous answer is no answer.
    pub fn sole_enabled_person(&self) -> Result<Option<i64>, IpcError> {
        let mut st = self
            .conn
            .prepare("SELECT id FROM people WHERE disabled_at IS NULL LIMIT 2")?;
        let ids: Vec<i64> = st
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(match ids.len() {
            1 => ids.into_iter().next(),
            _ => None,
        })
    }

    /// Create this hub's personal owner if it has none, and return its id.
    ///
    /// One of the two writers of `is_personal_owner` (the other is migration
    /// 094); `pub(crate)` rather than `pub` so it stays that way, and
    /// reached through `mcp::settings::ensure_personal_owner`, which is
    /// where the entry points call it. Idempotent: with the flagged row
    /// present it writes nothing.
    ///
    /// The name is [`PERSONAL_OWNER_NAME`], the same placeholder the
    /// migration uses. If a live person already holds that name — only
    /// possible on a database whose flagged row was removed by hand after a
    /// colleague was added — this refuses with `E_EXISTS` rather than
    /// re-homing the fleet's owner onto somebody else's row.
    pub(crate) fn mint_personal_owner(&self) -> Result<i64, IpcError> {
        if let Some(id) = self.personal_owner_id()? {
            return Ok(id);
        }
        let taken: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM people WHERE name = ?1 AND disabled_at IS NULL)",
            rusqlite::params![PERSONAL_OWNER_NAME],
            |r| r.get(0),
        )?;
        if taken {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!(
                    "this hub has no personal owner and a person named \
                     {PERSONAL_OWNER_NAME:?} already exists; rename that person, \
                     then reopen the store"
                ),
            ));
        }
        self.conn.execute(
            "INSERT INTO people (name, is_personal_owner, created_at) VALUES (?1, 1, ?2)",
            rusqlite::params![PERSONAL_OWNER_NAME, now_unix()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Bind the live client named `name` to `person` (multi-user M1), or
    /// unbind it (`None`). Follows [`Store::set_client_org`]: the name is
    /// trimmed, the auth-epoch trigger of migration 094 invalidates every
    /// cached caller so the new binding holds from that client's next
    /// request on, and `E_NOTFOUND` means no live client holds the name (or
    /// the person does not exist).
    ///
    /// A peer hub link and an updater token are refused: neither is a
    /// person's device — one is another fleet, the other is `fleet-updater`
    /// acting for this hub — and giving either a person would make it a
    /// reader of that person's private sessions. Migration 094's backfill
    /// skips the same two modes, so the upgrade and this setter say the same
    /// thing.
    ///
    /// The person must be LIVE. Binding a device to a disabled person would
    /// undo [`Store::disable_person`]'s whole contract — it revokes every
    /// device of theirs precisely so none answers again — and nothing on the
    /// request path joins `people`, so the new device would simply work.
    /// `E_VALIDATE`, distinct from the `E_NOTFOUND` an unknown id gets.
    pub fn set_client_person(
        &self,
        name: &str,
        person: Option<i64>,
    ) -> Result<super::ClientTokenRow, IpcError> {
        let name = name.trim();
        if let Some(p) = person {
            let row = self.get_person(p)?.ok_or_else(|| person_not_found(p))?;
            if row.disabled_at.is_some() {
                return Err(IpcError::new(
                    codes::E_VALIDATE,
                    format!(
                        "person {p} ({:?}) is disabled; binding a device to them \
                         would restore the reach disabling took away",
                        row.name
                    ),
                ));
            }
        }
        // The id is captured BEFORE the write, for `revoke_client_token`'s
        // reason: a name paired, revoked and paired again leaves two rows
        // with that name, and re-fetching by the name alone could answer
        // with one this call did not touch.
        let live: Option<(i64, String)> = self
            .conn
            .query_row(
                "SELECT id, mode FROM client_tokens WHERE name = ?1 AND revoked_at IS NULL",
                rusqlite::params![name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((id, mode)) = live else {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("no active client token named '{name}'"),
            ));
        };
        if let Some(what) = machine_token_kind(&mode) {
            return Err(IpcError::new(
                codes::E_VALIDATE,
                format!("'{name}' is {what}; it is not a person's device"),
            ));
        }
        self.conn.execute(
            "UPDATE client_tokens SET person_id = ?2 WHERE id = ?1",
            rusqlite::params![id, person],
        )?;
        super::clients::get_client_token_by_id(&self.conn, id)?.ok_or_else(|| {
            IpcError::new(
                codes::E_INTERNAL,
                format!("client token {id} vanished right after binding"),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open_in_memory().expect("store")
    }

    /// Migration 094's insert: every hub has a personal owner from the
    /// moment its database exists, and it is the ONLY person in it.
    #[test]
    fn a_fresh_database_has_exactly_one_person_and_it_is_the_personal_owner() {
        let s = store();
        let people = s.list_people().unwrap();
        assert_eq!(people.len(), 1, "one person on a fresh database");
        assert!(people[0].is_personal_owner);
        assert_eq!(people[0].name, PERSONAL_OWNER_NAME);
        assert!(people[0].disabled_at.is_none());
        assert_eq!(s.personal_owner_id().unwrap(), Some(people[0].id));
    }

    /// The partial unique index, not a convention: a second flagged row is a
    /// constraint error at write time, however it is attempted.
    #[test]
    fn a_second_personal_owner_is_refused_by_the_index() {
        let s = store();
        let err = s
            .conn
            .execute(
                "INSERT INTO people (name, is_personal_owner, created_at) \
                 VALUES ('impostor', 1, 1)",
                [],
            )
            .unwrap_err();
        assert!(
            format!("{err}").contains("UNIQUE"),
            "a second personal owner must violate idx_people_personal_owner, got: {err}"
        );
        // A second UNFLAGGED person is of course fine.
        s.create_person("ada", None).unwrap();
        assert_eq!(s.list_people().unwrap().len(), 2);
        let ada = s.get_person_by_name("ada").unwrap().unwrap();
        assert!(!ada.is_personal_owner);
    }

    /// The flag survives a rename, and `personal_owner_id` never falls back
    /// to another row when the flagged one is gone. Both halves matter: the
    /// first is why the flag exists, the second is the fail-closed rule the
    /// scope builder, the create paths and `Access::Person` all rely on.
    #[test]
    fn personal_owner_id_keys_on_the_flag_and_never_falls_back() {
        let s = store();
        let owner = s.personal_owner_id().unwrap().expect("fresh db has one");
        s.rename_person(owner, Some("Martin"), Some("Martin J."))
            .unwrap();
        assert_eq!(
            s.personal_owner_id().unwrap(),
            Some(owner),
            "renaming the owner must not re-home the flag"
        );
        let row = s.get_person(owner).unwrap().unwrap();
        assert_eq!(row.name, "Martin");
        assert_eq!(row.display_name.as_deref(), Some("Martin J."));
        assert!(row.is_personal_owner);

        // A colleague, and then the flagged row removed by hand: the answer
        // is None, NOT the remaining person and NOT the lowest id.
        let ada = s.create_person("ada", None).unwrap();
        s.conn
            .execute("DELETE FROM people WHERE is_personal_owner = 1", [])
            .unwrap();
        assert_eq!(s.personal_owner_id().unwrap(), None);
        assert_eq!(
            s.list_people().unwrap().len(),
            1,
            "and the colleague is still there to have been fallen back to"
        );
        assert_eq!(s.list_people().unwrap()[0].id, ada.id);
    }

    /// A name belongs to one LIVE person at a time; a disabled one keeps
    /// their row and their name without holding it against a newcomer.
    #[test]
    fn a_live_name_is_unique_and_a_disabled_person_releases_it() {
        let s = store();
        let ada = s.create_person("  ada  ", None).unwrap();
        assert_eq!(ada.name, "ada", "stored trimmed");
        assert_eq!(
            s.create_person("ada", None).unwrap_err().code,
            codes::E_EXISTS
        );
        s.disable_person(ada.id).unwrap();
        let again = s.create_person("ada", None).unwrap();
        assert_ne!(again.id, ada.id, "a new row, not the old one resurrected");
        assert_eq!(
            s.get_person_by_name("ada").unwrap().map(|p| p.id),
            Some(again.id),
            "by name resolves to the live row"
        );
        assert!(s.get_person(ada.id).unwrap().unwrap().disabled_at.is_some());
    }

    /// `disable_person` is compound: the stamp AND every device of theirs.
    /// A bare flag would be a no-op that merely freed the name. (The third
    /// part — every grant TO them, migration 096 — is pinned next to the
    /// grants themselves, in
    /// `store/session_grants.rs::disabling_a_person_revokes_their_tokens_and_every_grant_to_them`.)
    #[test]
    fn disabling_a_person_revokes_their_devices_and_leaves_everyone_elses() {
        let s = store();
        let owner = s.personal_owner_id().unwrap().unwrap();
        let ada = s.create_person("ada", None).unwrap();
        s.insert_client_token("ada-phone", &"a".repeat(64), "full")
            .unwrap();
        s.insert_client_token("my-laptop", &"b".repeat(64), "full")
            .unwrap();
        s.set_client_person("ada-phone", Some(ada.id)).unwrap();
        s.set_client_person("my-laptop", Some(owner)).unwrap();

        // What "revoked" means, in full: the stamp AND the client's update
        // rows (migration 079). This is the half a raw
        // `UPDATE client_tokens SET revoked_at` here used to miss, which is
        // why the sweep goes through `clients::revoke_client_token_row`.
        let ada_phone = s
            .active_client_tokens()
            .unwrap()
            .into_iter()
            .find(|c| c.name == "ada-phone")
            .unwrap()
            .id;
        s.conn
            .execute(
                "INSERT INTO update_observed (target, component, version, reported_at) \
                 VALUES ('client:' || ?1, 'mobile', '0.4.1', 1)",
                [ada_phone],
            )
            .unwrap();
        let observed = |s: &Store| -> i64 {
            s.conn
                .query_row("SELECT COUNT(*) FROM update_observed", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(observed(&s), 1);

        let before = s.auth_epoch().unwrap();
        s.disable_person(ada.id).unwrap();
        assert!(
            s.auth_epoch().unwrap() > before,
            "revoking the devices bumps the epoch, so no cached caller survives"
        );
        assert_eq!(
            observed(&s),
            0,
            "the device's update state went with the revocation"
        );
        let live: Vec<String> = s
            .active_client_tokens()
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(live, vec!["my-laptop".to_string()]);
        // The revoked row keeps its binding: the audit trail says whose it
        // was, and an id nothing resolves to is the fail-closed end.
        let all = s.list_client_tokens(true).unwrap();
        let revoked = all.iter().find(|c| c.name == "ada-phone").unwrap();
        assert_eq!(revoked.person_id, Some(ada.id));
        assert!(revoked.revoked_at.is_some());

        // Idempotent, and it keeps the original stamp.
        let first = s.get_person(ada.id).unwrap().unwrap().disabled_at;
        assert_eq!(s.disable_person(ada.id).unwrap().disabled_at, first);
        assert_eq!(s.disable_person(9_999).unwrap_err().code, codes::E_NOTFOUND);
    }

    /// The hub's own owner is not disable-able. `disabled_at` is outside
    /// `idx_people_personal_owner` on purpose, so a disabled owner would keep
    /// answering `personal_owner_id()` and keep `is_personal_owner` true for
    /// every device of theirs — the fail-OPEN shape in the one row every gate
    /// keys on. Refused, and nothing is written.
    #[test]
    fn the_personal_owner_cannot_be_disabled() {
        let s = store();
        let owner = s.personal_owner_id().unwrap().unwrap();
        s.insert_client_token("my-laptop", &"a".repeat(64), "full")
            .unwrap();
        s.set_client_person("my-laptop", Some(owner)).unwrap();
        let before = s.auth_epoch().unwrap();
        assert_eq!(
            s.disable_person(owner).unwrap_err().code,
            codes::E_VALIDATE,
            "disabling the hub's owner is not an operation M1 has"
        );
        let row = s.get_person(owner).unwrap().unwrap();
        assert!(row.disabled_at.is_none(), "no stamp was written");
        assert!(row.is_personal_owner);
        assert_eq!(s.personal_owner_id().unwrap(), Some(owner));
        assert_eq!(
            s.active_client_tokens().unwrap().len(),
            1,
            "and their device was not revoked either"
        );
        assert_eq!(s.auth_epoch().unwrap(), before, "nothing was written");
    }

    /// A device is never bound to a DISABLED person: that would hand back the
    /// reach `disable_person` took away, since nothing on the request path
    /// joins `people`.
    #[test]
    fn a_device_cannot_be_bound_to_a_disabled_person() {
        let s = store();
        let ada = s.create_person("ada", None).unwrap();
        s.insert_client_token("phone", &"a".repeat(64), "full")
            .unwrap();
        s.disable_person(ada.id).unwrap();
        let err = s.set_client_person("phone", Some(ada.id)).unwrap_err();
        assert_eq!(err.code, codes::E_VALIDATE);
        assert!(err.message.contains("disabled"), "{}", err.message);
        assert_eq!(
            s.active_client_tokens().unwrap()[0].person_id,
            None,
            "the device stays unbound: nobody's, not hers"
        );
    }

    /// The spec's Q9 answer for a departure, pinned: disabling a person
    /// removes their reach and touches none of their work. Nothing is
    /// re-attributed, nothing is deleted, no session row moves — M1 has no
    /// operation that does. (T3 adds `sessions.owner_person_id`; this holds
    /// at the `sessions` level that `disable_person` writes no session row
    /// at all, which is what must stay true once the column exists.)
    #[test]
    fn a_disabled_persons_own_sessions_stay_private_and_theirs() {
        let s = store();
        let ada = s.create_person("ada", None).unwrap();
        s.upsert_host("box").unwrap();
        let sid = s
            .upsert_session("ada-1", "box", None, None, 1, 1, "running", None)
            .unwrap();
        let snapshot = |s: &Store| -> (i64, String) {
            s.conn
                .query_row(
                    "SELECT row_version, status FROM sessions WHERE id = ?1",
                    [sid],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap()
        };
        let before = snapshot(&s);

        s.disable_person(ada.id).unwrap();

        assert_eq!(
            snapshot(&s),
            before,
            "disabling a person must not write a single session row"
        );
        assert_eq!(
            s.conn
                .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1,
            "and must not delete their work either"
        );
        // Their row survives too: the pointers into it must stay resolvable.
        let row = s.get_person(ada.id).unwrap().expect("the row is kept");
        assert!(row.disabled_at.is_some());
        assert_eq!(row.name, "ada");
    }

    /// `set_client_person` follows `set_client_org` — including what it
    /// refuses. A peer hub link and an updater token are not a person's
    /// device, and an unknown person is `E_NOTFOUND`, not a silent unbind.
    #[test]
    fn set_client_person_refuses_a_machine_token_and_an_unknown_person() {
        let s = store();
        let ada = s.create_person("ada", None).unwrap();
        s.insert_client_token("hub-b", &"a".repeat(64), "peer")
            .unwrap();
        s.insert_client_token("updater", &"b".repeat(64), "updater")
            .unwrap();
        s.insert_client_token("phone", &"c".repeat(64), "full")
            .unwrap();
        for machine in ["hub-b", "updater"] {
            assert_eq!(
                s.set_client_person(machine, Some(ada.id)).unwrap_err().code,
                codes::E_VALIDATE,
                "{machine}"
            );
        }
        assert_eq!(
            s.set_client_person("phone", Some(9_999)).unwrap_err().code,
            codes::E_NOTFOUND
        );
        assert_eq!(
            s.set_client_person("nobody", Some(ada.id))
                .unwrap_err()
                .code,
            codes::E_NOTFOUND
        );
        let bound = s.set_client_person("phone", Some(ada.id)).unwrap();
        assert_eq!(bound.person_id, Some(ada.id));
        assert_eq!(
            s.set_client_person("phone", None).unwrap().person_id,
            None,
            "unbinding is allowed; it fails closed, it does not widen"
        );
    }

    /// A name is checked the way a device name is: one line, bounded, and
    /// stored trimmed.
    #[test]
    fn a_person_name_is_one_bounded_line() {
        let s = store();
        // Each separator is INTERIOR, not trailing: `str::trim` counts all
        // three of `LINE_SEPARATORS` as whitespace, so a trailing one is
        // trimmed away rather than refused — exactly as
        // `validate_client_name` trims it, and equally safe, because what is
        // stored is then one unbroken line.
        for bad in ["", "   ", "ada\nbob", "ada\u{2028}bob", "ada\u{0085}bob"] {
            assert_eq!(
                s.create_person(bad, None).unwrap_err().code,
                codes::E_VALIDATE,
                "{bad:?}"
            );
        }
        assert_eq!(
            s.create_person("ada\u{0085}", None).unwrap().name,
            "ada",
            "a TRAILING separator is trimmed, like any other whitespace"
        );
        assert_eq!(
            s.create_person(&"x".repeat(MAX_PERSON_NAME_LEN + 1), None)
                .unwrap_err()
                .code,
            codes::E_VALIDATE
        );
        assert!(s
            .create_person(&"x".repeat(MAX_PERSON_NAME_LEN), None)
            .is_ok());
        assert_eq!(
            s.create_person("bob", Some("bob\nthe builder"))
                .unwrap_err()
                .code,
            codes::E_VALIDATE
        );
    }
}
