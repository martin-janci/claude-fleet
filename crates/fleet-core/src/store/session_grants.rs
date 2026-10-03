//! Session grants (multi-user M1, migration 096): the one way a second person
//! ever reaches a session row.
//!
//! A session started through fleet is `private` to its owner, so sharing has
//! to be a row somebody writes on purpose. This module is that row and the
//! four invariants the spec (§4.3, *What a grant may and may not do*) puts on
//! it. **They are enforced here, in the store, and not at the call sites** —
//! there are several callers (the MCP tools, the Tauri commands, the
//! `fleet-hub` CLI) and an invariant that lives in each of them is an
//! invariant one of them forgets:
//!
//! 1. **Only the owner creates a grant.** Every write carries
//!    `sessions.owner_person_id = ?granter` in its own `WHERE`, so SQL — not a
//!    Rust comparison — answers the question, and answers `false` for an
//!    unowned row without anyone having to remember that it would.
//! 2. **A grantee cannot grant on.** It is invariant 1 seen from the other
//!    side: a grantee is not the owner, so the same `WHERE` refuses them.
//!    Sharing is not transitive and there is no "may re-share" level.
//! 3. **Downward only.** A grant is created, narrowed (`drive` → `watch`) or
//!    revoked, and that is the entire list of things that can happen to one.
//!    There is no `widen`; [`Store::narrow_session_grant`] accepts one
//!    transition and nothing else; granting over a live grant refuses
//!    (`E_EXISTS`) rather than upgrading it; and **no function here writes a
//!    live grant's recipient columns**, because "re-home this grant to me" is
//!    a privacy bypass wearing a grant's clothes.
//! 4. **A grant never confers a terminal** — nor the attach command, nor the
//!    drop handler. Nothing about that is expressible in this table, so
//!    nothing here implies it either; it is enforced where the terminal is.
//!
//! **The ownership check is a column comparison, not `ViewScope::owns`.**
//! `ViewScope` is a `service/` type built from a `Caller`, and `store/` does
//! not import `service/` — the dependency runs the other way. The two are the
//! same rule written at the two layers that each need it: `ViewScope::owns`
//! compares the [`SessionRow`](super::SessionRow) this module's SQL produced;
//! this module compares the column. Both are written so the
//! `None == None` trap the spec calls out (an unowned row and a person-less
//! caller both being `None`) is not expressible: the granting person arrives
//! here as an `i64`, never an `Option<i64>`, and every comparison against the
//! nullable column happens inside SQLite.
//!
//! **Recipients are people, in M1.** `session_grants.org_id` exists so the
//! unique index and the `CHECK` are written once in their final shape, but
//! [`GrantRecipient::Org`] is refused with `E_INVALID`: an org names a
//! recipient *set* whose membership `work_admin { assign_client }` writes
//! under `Access::Master`, so an org grant would let an admin bind their own
//! device to the org and read the session with no grant touched and no owner
//! consent (spec §4.3, *Team sharing is out of M1*). It returns in M2 with
//! memberships and a defined reader.
//!
//! **A grant writes no `sessions` row, so it has no frame to ride** (spec
//! §3.8) — and sharing that announced nothing at all would leave a recipient
//! unable to learn the row exists. [`Store::announce_grant_change`] (T9) is
//! that announcement, in the `store/orgs.rs::announce_org_moves` shape: a
//! deliberate `row_version` bump, one transaction around the bumps, the rows
//! emitted after it commits — plus one `grant:changed` frame per change.
//! [`Store::grant_session`], [`Store::narrow_session_grant`] and
//! [`Store::revoke_session_grant`] call it; the two SWEEPS deliberately do
//! not (see each one's note), and are covered instead by [`grant_generation`]
//! and the `/events` keep-alive beat.
//!
//! What this module also carries is [`grant_generation`], so a long-lived
//! reader of a scope notices a revoke without waiting for a row to change on
//! its own.

use super::{now_unix, Store};
use crate::events::EventBus;
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Bumped by every grant change (create, narrow, revoke, and the sweep
/// [`Store::revoke_all_grants_for_person`] runs when a person is disabled).
/// A long-lived reader of a scope — an `/events` stream — compares it on each
/// frame and re-reads its grants when it moved, so a revoked share stops
/// being delivered from the very next frame rather than at the next keep-alive
/// beat.
///
/// It lives **here, beside the writes**, and not at a service call site, for
/// the reason `service/orgs.rs::ORG_GENERATION` is a cautionary tale: that one
/// has a single bump site in a tool handler, and a write path already misses
/// it (`mcp/pairing.rs` calls `set_client_org` without bumping). Every
/// function in this module that changes a grant bumps it, so a future one that
/// forgets is a function that did not go through here.
///
/// It is a process-local atomic, so it is an **optimisation and never the
/// guarantee**: a second process that revokes a grant does not move this
/// hub's counter. The guarantee is the periodic re-read of the scope; the
/// counter only makes the common case immediate. Over-bumping is harmless in
/// the same way — a reader re-reads its grants and finds them unchanged — so
/// it is bumped inside the transaction rather than after the commit, where a
/// rollback could leave a change unannounced.
static GRANT_GENERATION: AtomicU64 = AtomicU64::new(0);

/// See [`GRANT_GENERATION`].
pub fn grant_generation() -> u64 {
    GRANT_GENERATION.load(Ordering::SeqCst)
}

fn bump_grant_generation() {
    GRANT_GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// Watch: read the session and its content, drive nothing.
pub const GRANT_WATCH: &str = "watch";
/// Drive: watch, plus make the machine do work (`send_prompt`, and
/// `send_message { deliver, submit }`, which is the same pane write by another
/// route). **Not** kill, restart, rename, move, fork or re-share — those are
/// the `own` tier, whose one authoritative definition is the spec's §4.3
/// invariant 5, and no grant reaches it.
pub const GRANT_DRIVE: &str = "drive";

/// The two grantable levels, widest last — the order
/// [`Store::narrow_session_grant`] moves against. There is deliberately no
/// third entry: `own` is a tier nobody can be granted, not a level.
pub const GRANT_LEVELS: [&str; 2] = [GRANT_WATCH, GRANT_DRIVE];

/// Check a level before it becomes a row, and return it as it should be
/// STORED. `E_VALIDATE` for anything else — including `"own"`, which gets its
/// own sentence in the message because it is the plausible mistake.
pub fn validate_grant_level(level: &str) -> Result<&'static str, IpcError> {
    match level {
        GRANT_WATCH => Ok(GRANT_WATCH),
        GRANT_DRIVE => Ok(GRANT_DRIVE),
        "own" => Err(IpcError::new(
            codes::E_VALIDATE,
            "'own' is not a grantable level: it is the set of operations only \
             the owner may perform, and no grant ever reaches it",
        )),
        other => Err(IpcError::new(
            codes::E_VALIDATE,
            format!(
                "grant level {other:?} must be one of {}",
                GRANT_LEVELS.join(" | ")
            ),
        )),
    }
}

/// Who a grant is addressed to.
///
/// An enum rather than two arguments so that "an org recipient is refused in
/// M1" is a live, tested code path instead of a parameter nobody can pass:
/// the column is in the schema, and the refusal has to be somewhere a reader
/// of M2's diff will find it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantRecipient {
    /// A person — the only recipient M1 has.
    Person(i64),
    /// An org. **Refused** (`E_INVALID`): see this module's header and spec
    /// §4.3. Kept so the M2 work is a new arm here and not a migration.
    Org(i64),
}

/// One grant of one session to one recipient.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionGrantRow {
    pub id: i64,
    pub session_id: i64,
    /// The recipient, for every grant M1 writes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub person_id: Option<i64>,
    /// Reserved for M2; always `None` on a row this build wrote.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    /// [`GRANT_WATCH`] or [`GRANT_DRIVE`].
    pub level: String,
    /// The person who granted it — the owner at the time.
    pub granted_by: i64,
    pub granted_at: i64,
    /// Set once it stopped applying. The row stays, for the audit trail (the
    /// `client_tokens.revoked_at` convention): "A shared this with B and took
    /// it back" has to remain answerable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<i64>,
}

const GRANT_COLS: &str =
    "id, session_id, person_id, org_id, level, granted_by, granted_at, revoked_at";

fn map_grant(r: &rusqlite::Row<'_>) -> rusqlite::Result<SessionGrantRow> {
    Ok(SessionGrantRow {
        id: r.get(0)?,
        session_id: r.get(1)?,
        person_id: r.get(2)?,
        org_id: r.get(3)?,
        level: r.get(4)?,
        granted_by: r.get(5)?,
        granted_at: r.get(6)?,
        revoked_at: r.get(7)?,
    })
}

/// Whether `e` is a SQLite UNIQUE constraint violation. A private copy of
/// `clients::is_unique_violation`, exactly as `peer_links.rs` keeps one and
/// for the same stated reason: that one is private to its own module, the
/// constraint here (`idx_session_grants_live`) is unrelated to client tokens,
/// and a shared helper would add an import for a five-line match.
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

/// Revoke every live grant **to** `person`, on any session, at `at`.
///
/// Takes a connection so [`Store::disable_person`] can call it inside the
/// transaction that revokes the person's devices — the two halves of "this
/// colleague has left" must land together or not at all. `revoke_client_token_row`
/// is the same shape for the same reason.
///
/// This is the one grant write that is **not** the owner's, and it is not an
/// authority over a grant either: it narrows (every affected grant ends up
/// revoked), it names no session, and it is reached only by disabling the
/// recipient. M1 gives no admin any authority over a grant — "revoke a
/// departed member's grants" needs memberships to make "member" mean
/// anything, and it arrives in M2.
///
/// Returns how many grants it revoked.
pub(super) fn revoke_live_grants_to_person(
    conn: &rusqlite::Connection,
    person: i64,
    at: i64,
) -> rusqlite::Result<usize> {
    let n = conn.execute(
        "UPDATE session_grants SET revoked_at = ?2 \
         WHERE person_id = ?1 AND revoked_at IS NULL",
        rusqlite::params![person, at],
    )?;
    if n > 0 {
        bump_grant_generation();
    }
    Ok(n)
}

impl Store {
    /// Grant `to` access to session `session_id` at `level`, as `granter`.
    ///
    /// **Only the owner may call this, and the statement is what says so:**
    /// the `INSERT ... SELECT`'s `WHERE` carries
    /// `sessions.owner_person_id = ?granter`, which SQLite answers `false`
    /// both for a session owned by somebody else (a grantee included — rule 2
    /// is rule 1 from the other side) and for an `unclaimed` row, whose
    /// `owner_person_id` is NULL. `granter` is an `i64`, so the
    /// "person-less caller owns every unowned row" trap has nowhere to live.
    ///
    /// **It never upgrades.** There is deliberately no pre-check for an
    /// existing grant: `idx_session_grants_live` refuses the second live grant
    /// to the same recipient and the violation becomes `E_EXISTS`. The owner
    /// who wants a different level revokes and grants again — two of their own
    /// operations, neither of which is a widening of a live grant. (Re-granting
    /// after a revoke is a *new* grant and is allowed at any level, because
    /// the owner could have granted that level in the first place.)
    ///
    /// `E_INVALID` for an org recipient (M1 has none — see the module header),
    /// `E_VALIDATE` for an unknown level, for a grant to the owner themselves
    /// (it would confer nothing and would then show up as a share of the
    /// session with its own owner), and for a disabled recipient — which
    /// [`Store::disable_person`] revokes grants precisely to prevent, so
    /// writing a fresh one would undo it. `E_NOTFOUND` when no such session or
    /// person exists; `E_FORBIDDEN` when `granter` does not own the session.
    ///
    /// The two refusals are distinguishable **here** because the store is an
    /// internal API; a caller who may not see the row at all must be refused
    /// before it gets this far (by the request's scope), so that neither
    /// answer is an existence oracle.
    pub fn grant_session(
        &self,
        session_id: i64,
        to: GrantRecipient,
        level: &str,
        granter: i64,
    ) -> Result<SessionGrantRow, IpcError> {
        let person = match to {
            GrantRecipient::Person(p) => p,
            GrantRecipient::Org(org) => {
                return Err(IpcError::new(
                    codes::E_INVALID,
                    format!(
                        "sharing with an org (id {org}) is not part of M1: an org \
                         names a recipient set whose membership an admin writes, \
                         so the owner's consent would not be in it. Share with \
                         each person instead"
                    ),
                ))
            }
        };
        let level = validate_grant_level(level)?;
        if person == granter {
            return Err(IpcError::new(
                codes::E_VALIDATE,
                format!(
                    "person {person} cannot be granted session {session_id} by \
                     themselves: a grant to the caller confers nothing, and only \
                     the owner may grant at all"
                ),
            ));
        }
        let recipient = self
            .get_person(person)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("no person {person}")))?;
        if recipient.disabled_at.is_some() {
            return Err(IpcError::new(
                codes::E_VALIDATE,
                format!(
                    "person {person} ({:?}) is disabled; sharing with them would \
                     restore reach that disabling took away",
                    recipient.name
                ),
            ));
        }
        let now = now_unix();
        // The ownership rule, in the writing statement itself: no row is
        // selected to insert unless the session's owner column equals the
        // granter. A NULL column (an `unclaimed` row) selects nothing.
        let inserted = self
            .conn
            .execute(
                "INSERT INTO session_grants \
                   (session_id, person_id, level, granted_by, granted_at) \
                 SELECT ?1, ?2, ?3, ?4, ?5 FROM sessions \
                  WHERE sessions.id = ?1 AND sessions.owner_person_id = ?4",
                rusqlite::params![session_id, person, level, granter, now],
            )
            .map_err(|e| {
                if is_unique_violation(&e) {
                    IpcError::new(
                        codes::E_EXISTS,
                        format!(
                            "session {session_id} is already shared with person \
                             {person}; revoke that grant first — re-granting never \
                             raises a level"
                        ),
                    )
                } else {
                    IpcError::from(e)
                }
            })?;
        if inserted == 0 {
            return Err(self.refuse_unowned(session_id, granter, "share"));
        }
        let id = self.conn.last_insert_rowid();
        bump_grant_generation();
        let row = self.session_grant(id)?.ok_or_else(|| {
            IpcError::new(
                codes::E_INTERNAL,
                format!("grant {id} vanished right after it was written"),
            )
        })?;
        self.announce_grant_change(&[crate::events::GrantChanged {
            session_id,
            person_id: person,
            level: Some(level.to_string()),
        }])?;
        Ok(row)
    }

    /// Narrow the live grant of `session_id` to `person` from `drive` to
    /// `watch`, as the owner. **The only level change this module has**, and
    /// it has no twin: there is no function that raises one, which is why
    /// "downward only" is a property of the module's surface rather than of a
    /// check inside it.
    ///
    /// Idempotent: narrowing a grant that is already `watch` writes nothing
    /// and returns it, because the caller asked for a state the row is in and
    /// reporting that as a failure would only invite a retry loop.
    ///
    /// `E_FORBIDDEN` when the caller does not own the session (`owner` is an
    /// `i64` and the `UPDATE` carries the owner comparison, as in
    /// [`Store::grant_session`]); `E_NOTFOUND` when there is no live grant to
    /// narrow.
    pub fn narrow_session_grant(
        &self,
        session_id: i64,
        person: i64,
        owner: i64,
    ) -> Result<SessionGrantRow, IpcError> {
        let narrowed = self.conn.execute(
            "UPDATE session_grants SET level = 'watch' \
              WHERE session_id = ?1 AND person_id = ?2 AND revoked_at IS NULL \
                AND level = 'drive' \
                AND EXISTS (SELECT 1 FROM sessions \
                             WHERE sessions.id = session_grants.session_id \
                               AND sessions.owner_person_id = ?3)",
            rusqlite::params![session_id, person, owner],
        )?;
        if narrowed > 0 {
            bump_grant_generation();
            // Only on a real narrowing: the idempotent path (already
            // `watch`) changed nothing, and announcing it would move every
            // affected stream's generation for nothing.
            self.announce_grant_change(&[crate::events::GrantChanged {
                session_id,
                person_id: person,
                level: Some(GRANT_WATCH.to_string()),
            }])?;
        } else if !self.session_is_owned_by(session_id, owner)? {
            // Nothing changed and the caller is not the owner: the refusal is
            // the ownership one, whether or not a grant exists. Checked after
            // the write rather than before it so the write's own `WHERE` stays
            // the authority (this only decides which error to raise).
            return Err(self.refuse_unowned(session_id, owner, "narrow a grant on"));
        }
        self.live_grant(session_id, person)?.ok_or_else(|| {
            IpcError::new(
                codes::E_NOTFOUND,
                format!("session {session_id} is not shared with person {person}"),
            )
        })
    }

    /// Revoke the live grant of `session_id` to `person`, as the owner.
    ///
    /// The row stays with a `revoked_at` stamp — the audit trail — and every
    /// read in this module filters on it, so a revoked grant reaches nobody.
    /// `E_FORBIDDEN` for a caller who does not own the session, `E_NOTFOUND`
    /// when there was no live grant to revoke.
    ///
    /// Revoking a grant deliberately does **not** ride `auth_epoch`: that is
    /// the DEVICE mechanism (revoking a phone), and the two are different
    /// events with different mechanisms. [`grant_generation`] is this one.
    pub fn revoke_session_grant(
        &self,
        session_id: i64,
        person: i64,
        owner: i64,
    ) -> Result<SessionGrantRow, IpcError> {
        let now = now_unix();
        let revoked = self.conn.execute(
            "UPDATE session_grants SET revoked_at = ?4 \
              WHERE session_id = ?1 AND person_id = ?2 AND revoked_at IS NULL \
                AND EXISTS (SELECT 1 FROM sessions \
                             WHERE sessions.id = session_grants.session_id \
                               AND sessions.owner_person_id = ?3)",
            rusqlite::params![session_id, person, owner, now],
        )?;
        if revoked == 0 {
            if !self.session_is_owned_by(session_id, owner)? {
                return Err(self.refuse_unowned(session_id, owner, "revoke a grant on"));
            }
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("session {session_id} is not shared with person {person}"),
            ));
        }
        bump_grant_generation();
        let row = self
            .conn
            .query_row(
                &format!(
                    "SELECT {GRANT_COLS} FROM session_grants \
                      WHERE session_id = ?1 AND person_id = ?2 AND revoked_at = ?3 \
                      ORDER BY id DESC LIMIT 1"
                ),
                rusqlite::params![session_id, person, now],
                map_grant,
            )
            .optional()?
            .ok_or_else(|| {
                IpcError::new(
                    codes::E_INTERNAL,
                    format!("the grant of session {session_id} to {person} vanished mid-revoke"),
                )
            })?;
        self.announce_grant_change(&[crate::events::GrantChanged {
            session_id,
            person_id: person,
            level: None,
        }])?;
        Ok(row)
    }

    /// Revoke every live grant **to** `person`, on every session. The
    /// person's own sessions are untouched: still theirs, still `private`.
    ///
    /// It announces nothing either, and here the reason is structural: the
    /// same sweep runs inside [`Store::disable_person`]'s transaction, and
    /// emitting a frame from inside an open transaction is the one thing
    /// `announce_org_moves`' shape exists to avoid. [`grant_generation`] is
    /// bumped, so every affected stream ends on its next frame or beat.
    ///
    /// Reached by [`Store::disable_person`], which calls
    /// [`revoke_live_grants_to_person`] inside its own transaction; this is
    /// the same sweep standing on its own, for an operator path that needs it
    /// without disabling the person. It is not an authority over a grant —
    /// see that function's note.
    pub fn revoke_all_grants_for_person(&self, person: i64) -> Result<usize, IpcError> {
        let tx = self.conn.unchecked_transaction()?;
        let n = revoke_live_grants_to_person(&tx, person, now_unix())?;
        tx.commit()?;
        Ok(n)
    }

    /// Revoke every live grant ON `session_id`, whoever holds it. Returns how
    /// many it revoked.
    ///
    /// Reached by **`move_session`** and by nothing else (multi-user M1, T5):
    /// a move creates a new row with a new id on the target host, and the
    /// owner's decision of 2026-09-30 is that the grants are dropped rather
    /// than carried (spec §4.3, *A grant is on a row, so it does not travel*).
    /// The owner re-grants if they still want to.
    ///
    /// It announces nothing (no `grant:changed`): the only caller is
    /// `move_session`, which emits its own frames for the row it is moving,
    /// and [`grant_generation`] plus the `/events` keep-alive beat is what
    /// drops the ex-grantees' streams. A frame per revoked grant here would
    /// have to be built from a read this function does not take.
    ///
    /// Like [`Store::revoke_all_grants_for_person`] this takes no granter and
    /// is **not** an authority over a grant: it only ever revokes, it names a
    /// session rather than a recipient, and the operation that reaches it is
    /// owner-only one layer up — `move_session` is in the `own` tier (spec
    /// §4.3 invariant 5), so a grantee never gets this far. There is
    /// deliberately no inverse: nothing in this module restores a revoked
    /// grant.
    pub fn revoke_all_grants_on_session(&self, session_id: i64) -> Result<usize, IpcError> {
        let n = self.conn.execute(
            "UPDATE session_grants SET revoked_at = ?2 \
              WHERE session_id = ?1 AND revoked_at IS NULL",
            rusqlite::params![session_id, now_unix()],
        )?;
        if n > 0 {
            bump_grant_generation();
        }
        Ok(n)
    }

    /// Announce a change to who may see a session (multi-user M1, T9).
    ///
    /// A grant touches no `sessions` column, so there is no row event behind
    /// it; `store/orgs.rs::announce_org_moves` is the repo's one precedent
    /// for announcing a computed change, and this is the same shape down to
    /// the details: an EXPLICIT `row_version` bump (since migration 063 a
    /// same-value `UPDATE` no longer moves it on its own), one transaction
    /// around the bumps so a session shared with ten people does not fsync
    /// ten times on the caller's thread, and the frames emitted only after
    /// that transaction commits.
    ///
    /// **Two frames, and both are needed** (R6-j). The `session:updated`
    /// carries the row itself, which is how a new recipient learns the row
    /// exists at all — before the grant they could not see it, so no earlier
    /// frame of theirs ever named it. The `grant:changed` carries ids only
    /// and is how each client keeps its own GRANT SET current: access is
    /// derived on the client from the row plus that set, so a row event
    /// alone would leave a recipient holding a row they cannot classify.
    ///
    /// Returns how many `session:updated` frames went out (a session deleted
    /// between the bump and the read emits none, and its `grant:changed`
    /// still does — the recipient must hear that the grant ended whether or
    /// not the row survived).
    pub fn announce_grant_change(
        &self,
        changes: &[crate::events::GrantChanged],
    ) -> Result<usize, IpcError> {
        if changes.is_empty() {
            return Ok(0);
        }
        let mut sessions: Vec<i64> = changes.iter().map(|c| c.session_id).collect();
        sessions.sort_unstable();
        sessions.dedup();
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut bump =
                tx.prepare("UPDATE sessions SET row_version = row_version + 1 WHERE id = ?1")?;
            for id in &sessions {
                bump.execute(rusqlite::params![id])?;
            }
        }
        tx.commit()?;
        let mut announced = 0;
        for id in sessions {
            if let Some(row) = self.get_session_by_id(id)? {
                self.bus.session_updated(&row);
                announced += 1;
            }
        }
        for c in changes {
            self.bus.grant_changed(c);
        }
        Ok(announced)
    }

    /// Every live grant on `session_id`, person recipients first and oldest
    /// first — the Share sheet's read ("who can see this?").
    ///
    /// Live only. A revoked row is kept for the audit trail and is nobody's
    /// access, so it is not part of any answer this module gives.
    pub fn grants_for_session(&self, session_id: i64) -> Result<Vec<SessionGrantRow>, IpcError> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {GRANT_COLS} FROM session_grants \
              WHERE session_id = ?1 AND revoked_at IS NULL ORDER BY id"
        ))?;
        let rows = st.query_map(rusqlite::params![session_id], map_grant)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every live grant **to** `person`, as session id → level.
    ///
    /// The hot read: it runs once per request to build the caller's scope, and
    /// `idx_session_grants_person` is the index that serves it. A `BTreeMap`
    /// rather than a `Vec` because that is what both consumers want — the
    /// scope needs a lookup by session id with a canonical ordering (so a
    /// scope compares equal to an unchanged one), and `my_grants` serves the
    /// pairs as they are.
    ///
    /// An org grant — which M1 never writes — is not in the answer: the
    /// statement keys on `person_id`, so a row addressed to an org reaches no
    /// person by this path either.
    pub fn grants_for_person(&self, person: i64) -> Result<BTreeMap<i64, String>, IpcError> {
        let mut st = self.conn.prepare(
            "SELECT session_id, level FROM session_grants \
              WHERE person_id = ?1 AND revoked_at IS NULL",
        )?;
        let rows = st.query_map(rusqlite::params![person], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<BTreeMap<_, _>>>()?)
    }

    /// One grant by id, revoked or not — the read-back of a write.
    fn session_grant(&self, id: i64) -> Result<Option<SessionGrantRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {GRANT_COLS} FROM session_grants WHERE id = ?1"),
                [id],
                map_grant,
            )
            .optional()?)
    }

    /// The live grant of `session_id` to `person`, if any. At most one exists:
    /// `idx_session_grants_live` is what makes that true.
    fn live_grant(
        &self,
        session_id: i64,
        person: i64,
    ) -> Result<Option<SessionGrantRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {GRANT_COLS} FROM session_grants \
                      WHERE session_id = ?1 AND person_id = ?2 AND revoked_at IS NULL"
                ),
                rusqlite::params![session_id, person],
                map_grant,
            )
            .optional()?)
    }

    /// Does `person` own session `session_id`?
    ///
    /// The comparison happens in SQLite, against the nullable column, so the
    /// `(None, None)` equality the spec warns about cannot be written here at
    /// all: an unowned row answers `false` for every person, and this function
    /// has no way to be asked about a person-less caller.
    ///
    /// It is never the authority — each write carries the same comparison in
    /// its own `WHERE` — only the way a refused write picks its error.
    fn session_is_owned_by(&self, session_id: i64, person: i64) -> Result<bool, IpcError> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1 AND owner_person_id = ?2)",
            rusqlite::params![session_id, person],
            |r| r.get(0),
        )?)
    }

    /// The error a write refused by its own ownership `WHERE` deserves:
    /// `E_NOTFOUND` when there is no such session, `E_FORBIDDEN` when there
    /// is one and `person` does not own it. `what` is the verb, for the
    /// message.
    fn refuse_unowned(&self, session_id: i64, person: i64, what: &str) -> IpcError {
        let exists: rusqlite::Result<bool> = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1)",
            [session_id],
            |r| r.get(0),
        );
        match exists {
            Ok(false) => IpcError::new(codes::E_NOTFOUND, format!("no session {session_id}")),
            Ok(true) => IpcError::new(
                codes::E_FORBIDDEN,
                format!(
                    "only the owner of session {session_id} may {what} it; person \
                     {person} does not own it"
                ),
            ),
            Err(e) => IpcError::from(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The announcement (multi-user M1, T9): a grant writes no `sessions`
    /// column, so without [`Store::announce_grant_change`] sharing emits
    /// nothing at all and a recipient's client has no way to learn the row
    /// exists.
    ///
    /// Two frames per change, in this order, and the second is what keeps a
    /// client's own GRANT SET current — access is derived on the client from
    /// the row plus that set, so a row event alone would leave a recipient
    /// holding a row they cannot classify.
    #[test]
    fn every_grant_change_announces_the_row_and_the_grant() {
        let bus = std::sync::Arc::new(crate::events::RecordingEventBus::new());
        let s = Store::open_with_bus_in_memory(bus.clone() as std::sync::Arc<dyn EventBus>)
            .expect("store");
        let a = s.create_person("ann", None).expect("ann").id;
        let b = s.create_person("bob", None).expect("bob").id;
        let session = owned_session(&s, "work", Some(a));
        let before = s
            .get_session_by_id(session)
            .unwrap()
            .expect("row")
            .row_version;
        bus.take();

        s.grant_session(session, GrantRecipient::Person(b), GRANT_DRIVE, a)
            .expect("grant");
        assert_eq!(
            bus.take(),
            vec![
                format!("session:updated:{session}"),
                format!("grant:changed:{session}:{b}:drive"),
            ],
            "a grant announces the row, then the grant"
        );
        // The `row_version` bump is explicit, because the org/grant change is
        // computed and no column behind it moved (`announce_org_moves`' shape,
        // and since migration 063 a same-value UPDATE does not move it).
        assert!(
            s.get_session_by_id(session).unwrap().unwrap().row_version > before,
            "the frontend's merge guard orders by row_version"
        );

        s.narrow_session_grant(session, b, a).expect("narrow");
        assert_eq!(
            bus.take(),
            vec![
                format!("session:updated:{session}"),
                format!("grant:changed:{session}:{b}:watch"),
            ],
            "a narrowing announces the level now in force"
        );
        // Narrowing an already-`watch` grant changed nothing, so it says
        // nothing: announcing it would move every affected stream's
        // generation for a write that did not happen.
        s.narrow_session_grant(session, b, a).expect("idempotent");
        assert!(bus.take().is_empty(), "the idempotent path is silent");

        s.revoke_session_grant(session, b, a).expect("revoke");
        assert_eq!(
            bus.take(),
            vec![
                format!("session:updated:{session}"),
                format!("grant:changed:{session}:{b}:revoked"),
            ],
            "a revoke is the same frame with a null level"
        );
        // …and the frame really carries `null`, which is what the client
        // reads as "no longer a level I may act on".
        assert_eq!(
            crate::events::RowChange::GrantChanged(crate::events::GrantChanged {
                session_id: session,
                person_id: b,
                level: None,
            })
            .payload()["level"],
            serde_json::Value::Null
        );
    }

    /// A store with two live people besides the hub's own owner, and one
    /// session owned by `a`. Returns `(store, owner_a, person_b, session_id)`.
    fn shared_fixture() -> (Store, i64, i64, i64) {
        let s = Store::open_in_memory().expect("store");
        let a = s.create_person("ann", None).expect("ann").id;
        let b = s.create_person("bob", None).expect("bob").id;
        let session = owned_session(&s, "work", Some(a));
        (s, a, b, session)
    }

    /// A session row on host `h` owned by `owner` (`None` leaves it
    /// `unclaimed`, which is what reconcile produces). Written with SQL
    /// because the create paths that stamp an owner are a later task's.
    fn owned_session(s: &Store, name: &str, owner: Option<i64>) -> i64 {
        s.conn
            .execute("INSERT OR IGNORE INTO hosts (alias) VALUES ('h')", [])
            .expect("host");
        let visibility = if owner.is_some() {
            "private"
        } else {
            "unclaimed"
        };
        s.conn
            .execute(
                "INSERT INTO sessions (tmux_name, host_alias, created_at, last_activity_at, \
                                       status, started_at, owner_person_id, visibility) \
                 VALUES (?1, 'h', 1, 1, 'running', 1, ?2, ?3)",
                rusqlite::params![name, owner, visibility],
            )
            .expect("session");
        s.conn.last_insert_rowid()
    }

    /// The level of the live grant of `session` to `person`, or `None` —
    /// read through the per-person path a request actually uses, so every
    /// assertion below is an assertion about what the recipient can reach.
    fn granted_level(s: &Store, session: i64, person: i64) -> Option<String> {
        s.grants_for_person(person)
            .expect("grants")
            .get(&session)
            .cloned()
    }

    #[test]
    fn the_owner_shares_and_the_recipient_sees_exactly_that() {
        let (s, a, b, session) = shared_fixture();
        let g = s
            .grant_session(session, GrantRecipient::Person(b), GRANT_WATCH, a)
            .expect("ann shares her own session");
        assert_eq!(g.session_id, session);
        assert_eq!(g.person_id, Some(b));
        assert_eq!(g.org_id, None);
        assert_eq!(g.level, GRANT_WATCH);
        assert_eq!(g.granted_by, a);
        assert_eq!(g.revoked_at, None);
        assert_eq!(granted_level(&s, session, b).as_deref(), Some(GRANT_WATCH));
        // Ann is the owner, not a grantee: owning a session is not a grant,
        // and nothing here invents one for her.
        assert!(s.grants_for_person(a).unwrap().is_empty());
        let listed = s.grants_for_session(session).unwrap();
        assert_eq!(listed, vec![g]);
    }

    /// Invariant 1. Nobody but the owner — and an `unclaimed` row has no
    /// owner, so it has nobody who may share it either, which is the
    /// `(None, None)` case the predicate must answer "no" to.
    #[test]
    fn a_non_owner_cannot_grant() {
        let (s, a, b, session) = shared_fixture();
        let c = s.create_person("cat", None).unwrap().id;
        let e = s
            .grant_session(session, GrantRecipient::Person(c), GRANT_WATCH, b)
            .expect_err("bob does not own ann's session");
        assert_eq!(e.code, codes::E_FORBIDDEN);
        assert!(s.grants_for_person(c).unwrap().is_empty());

        // The hub's own personal owner is not a special case: being the
        // fleet's owner is not owning somebody else's session.
        let hub_owner = s.personal_owner_id().unwrap().expect("094 mints one");
        let e = s
            .grant_session(session, GrantRecipient::Person(c), GRANT_WATCH, hub_owner)
            .expect_err("the hub's owner does not own ann's session either");
        assert_eq!(e.code, codes::E_FORBIDDEN);

        // An `unclaimed` row: `owner_person_id` is NULL, so no person equals
        // it. This is the trap in column form — if the comparison were made
        // in Rust between two `Option`s, a person-less caller would own it.
        let orphan = owned_session(&s, "hand-started", None);
        let e = s
            .grant_session(orphan, GrantRecipient::Person(b), GRANT_WATCH, a)
            .expect_err("nobody owns an unclaimed row");
        assert_eq!(e.code, codes::E_FORBIDDEN);
        assert!(s.grants_for_session(orphan).unwrap().is_empty());

        // And an id nothing has is `E_NOTFOUND`, not a grant.
        let e = s
            .grant_session(session + 999, GrantRecipient::Person(b), GRANT_WATCH, a)
            .expect_err("no such session");
        assert_eq!(e.code, codes::E_NOTFOUND);
    }

    /// Invariant 2. Sharing is not transitive, at either level: holding a
    /// grant is not owning the row, so the same `WHERE` that stops a stranger
    /// stops a grantee.
    #[test]
    fn a_grantee_cannot_grant_on() {
        let (s, a, b, session) = shared_fixture();
        let c = s.create_person("cat", None).unwrap().id;
        for level in GRANT_LEVELS {
            s.grant_session(session, GrantRecipient::Person(b), level, a)
                .expect("ann shares with bob");
            let e = s
                .grant_session(session, GrantRecipient::Person(c), GRANT_WATCH, b)
                .expect_err("a grantee may not share on");
            assert_eq!(e.code, codes::E_FORBIDDEN, "at level {level}");
            assert!(s.grants_for_person(c).unwrap().is_empty());
            // Nor may they narrow or revoke: every write is the owner's.
            assert_eq!(
                s.narrow_session_grant(session, b, b)
                    .expect_err("not bob's to narrow")
                    .code,
                codes::E_FORBIDDEN
            );
            assert_eq!(
                s.revoke_session_grant(session, b, b)
                    .expect_err("not bob's to revoke")
                    .code,
                codes::E_FORBIDDEN
            );
            assert_eq!(granted_level(&s, session, b).as_deref(), Some(level));
            s.revoke_session_grant(session, b, a).expect("ann revokes");
        }
    }

    /// Invariant 3, the behavioural half: there is no sequence of calls that
    /// turns a `watch` grant into a `drive` one.
    #[test]
    fn watch_never_becomes_drive_by_any_path() {
        let (s, a, b, session) = shared_fixture();
        let first = s
            .grant_session(session, GrantRecipient::Person(b), GRANT_WATCH, a)
            .expect("watch");

        // 1. Granting `drive` over it refuses — it does not upgrade.
        let e = s
            .grant_session(session, GrantRecipient::Person(b), GRANT_DRIVE, a)
            .expect_err("a live grant is not re-granted");
        assert_eq!(e.code, codes::E_EXISTS);
        assert_eq!(granted_level(&s, session, b).as_deref(), Some(GRANT_WATCH));

        // 2. Granting `watch` again refuses too: the refusal is about the
        //    recipient already holding a grant, not about the level.
        assert_eq!(
            s.grant_session(session, GrantRecipient::Person(b), GRANT_WATCH, a)
                .expect_err("still one live grant per recipient")
                .code,
            codes::E_EXISTS
        );

        // 3. Narrowing is a no-op at `watch` — the only level change in the
        //    module moves the other way, and there is nothing below watch.
        let same = s
            .narrow_session_grant(session, b, a)
            .expect("narrowing a watch grant is idempotent");
        assert_eq!(same.level, GRANT_WATCH);
        assert_eq!(same.id, first.id);

        // 4. `drive` is only ever reached by a FRESH grant after a revoke —
        //    two operations only the owner can perform, and the first of them
        //    takes the access away. The original row is still `watch` and
        //    still revoked: no live grant was ever raised.
        s.revoke_session_grant(session, b, a).expect("revoke");
        assert_eq!(granted_level(&s, session, b), None);
        s.grant_session(session, GrantRecipient::Person(b), GRANT_DRIVE, a)
            .expect("a new grant after a revoke");
        let rows = s.session_grant(first.id).unwrap().expect("audit row stays");
        assert_eq!(rows.level, GRANT_WATCH);
        assert!(rows.revoked_at.is_some());

        // 5. And the one legitimate change still works, downward.
        let narrowed = s.narrow_session_grant(session, b, a).expect("drive→watch");
        assert_eq!(narrowed.level, GRANT_WATCH);
        assert_eq!(granted_level(&s, session, b).as_deref(), Some(GRANT_WATCH));
    }

    /// Invariant 3, the structural half — the test that is supposed to fail
    /// when somebody ADDS a function.
    ///
    /// It reads this module's own source (the `store/decisions.rs` and
    /// `net/tls.rs` precedent) and holds two things no behavioural test can:
    /// the exported surface is exactly the list below, and the only statement
    /// that writes `level` writes the narrow one. A new function that could
    /// raise a level or re-home a grant has to pass both, which it cannot do
    /// silently.
    #[test]
    fn the_public_surface_of_session_grants_cannot_raise_a_level_or_change_a_recipient() {
        let src = include_str!("session_grants.rs");
        let src = &src[..src.find("#[cfg(test)]").expect("the test module")];
        // Comment lines out, so the assertions below are about STATEMENTS and
        // not about prose that happens to quote one (this module's own header
        // quotes the ownership `WHERE` twice).
        let code = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        // (a) The surface, named. Add a function and this fails until you add
        //     it here — which is where you have to argue it cannot widen.
        let exported: Vec<&str> = code
            .lines()
            .filter_map(|l| {
                let l = l.trim_start();
                let rest = l
                    .strip_prefix("pub fn ")
                    .or_else(|| l.strip_prefix("pub(super) fn "))
                    .or_else(|| l.strip_prefix("pub(crate) fn "))?;
                Some(rest.split(['(', '<']).next().unwrap_or(rest))
            })
            .collect();
        assert_eq!(
            exported,
            [
                // reads: no write at all
                "grant_generation",
                "validate_grant_level",
                // the disablement sweep: revokes, never grants
                "revoke_live_grants_to_person",
                // the three writes: create, narrow, revoke — no widen
                "grant_session",
                "narrow_session_grant",
                "revoke_session_grant",
                "revoke_all_grants_for_person",
                // the move's sweep (T5): revokes every grant ON one session,
                // because the moved session is a NEW row and the old grants
                // are dropped rather than carried. Revokes only, names no
                // recipient, has no inverse.
                "revoke_all_grants_on_session",
                // the announcement (T9): it writes `sessions.row_version`
                // and emits two frames. It touches no `session_grants` row
                // at all — not a level, not a recipient — so it cannot
                // widen anything; what it can do is tell a client that a
                // grant moved, which is the opposite problem. Its `UPDATE`
                // is the only statement in this module that names a table
                // other than `session_grants`, and (e) below pins that.
                "announce_grant_change",
                // reads
                "grants_for_session",
                "grants_for_person",
            ],
            "the public surface of session_grants changed: a new function must \
             be unable to raise a grant's level or change its recipient, and \
             this list is where that is argued"
        );

        // (b) Exactly one statement writes a level, and it writes the narrow
        //     one. A `widen` would have to add a second.
        assert_eq!(
            code.matches("SET level").count(),
            1,
            "only the narrow writes a level"
        );
        assert!(code.contains("SET level = 'watch'"));

        // (c) Nothing re-homes a grant. A redirect — "share this with me
        //     instead" — would be a privacy bypass wearing a grant's clothes.
        for forbidden in [
            "SET person_id",
            "SET org_id",
            "SET session_id",
            "SET granted_by",
        ] {
            assert_eq!(
                code.matches(forbidden).count(),
                0,
                "no statement may write a live grant's {forbidden:?}"
            );
        }

        // (e) `announce_grant_change` writes exactly one thing outside this
        //     module's own table, and it is the deliberate `row_version`
        //     bump (`announce_org_moves`' shape). Anything else writing
        //     `sessions` from here would be a grant path that can change a
        //     session, which is not what a grant is.
        assert_eq!(
            code.matches("UPDATE sessions").count(),
            1,
            "the only `sessions` write here is the announcement's row_version bump"
        );
        assert!(code.contains("UPDATE sessions SET row_version = row_version + 1 WHERE id = ?1"));

        // (d) One INSERT, and it is the owner's. Its `WHERE` is the ownership
        //     rule, so a second insert site would be a second rule.
        assert_eq!(code.matches("INSERT INTO session_grants").count(), 1);
        assert_eq!(
            code.matches("owner_person_id = ?").count(),
            4,
            "the three writes and the predicate behind their error each carry \
             the owner comparison in SQL"
        );
    }

    /// The `COALESCE` in `idx_session_grants_live` is what makes "one live
    /// grant per recipient" true. Without it both rows insert, because exactly
    /// one recipient column is NULL by design and SQLite treats NULLs as
    /// distinct — so this asserts the index fires at the SQL level too, not
    /// only that `grant_session` returns `E_EXISTS`.
    #[test]
    fn a_live_grant_to_the_same_recipient_is_refused_not_upgraded() {
        let (s, a, b, session) = shared_fixture();
        s.grant_session(session, GrantRecipient::Person(b), GRANT_DRIVE, a)
            .expect("drive");
        let e = s
            .grant_session(session, GrantRecipient::Person(b), GRANT_WATCH, a)
            .expect_err("already shared");
        assert_eq!(e.code, codes::E_EXISTS);

        // The same thing one layer down: a hand-written duplicate is refused
        // by the index, so the refusal does not depend on a Rust pre-check
        // (there deliberately is none).
        let raw = s.conn.execute(
            "INSERT INTO session_grants (session_id, person_id, level, granted_by, granted_at) \
             VALUES (?1, ?2, 'watch', ?3, 1)",
            rusqlite::params![session, b, a],
        );
        assert!(
            raw.as_ref().err().is_some_and(is_unique_violation),
            "idx_session_grants_live must refuse a second live grant, got {raw:?}"
        );

        // A revoked grant blocks nothing: the index is partial, so re-sharing
        // after a revoke is an ordinary creation.
        s.revoke_session_grant(session, b, a).expect("revoke");
        s.grant_session(session, GrantRecipient::Person(b), GRANT_WATCH, a)
            .expect("sharing again after a revoke");
        assert_eq!(s.grants_for_session(session).unwrap().len(), 1);
        let all: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM session_grants WHERE session_id = ?1",
                [session],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(all, 2, "the revoked row stays, for the audit trail");
    }

    /// M1 has person recipients only. The column exists for M2; naming it is
    /// refused, and a row somebody writes by hand reaches no person either.
    #[test]
    fn an_org_recipient_is_refused_in_m1() {
        let (s, a, b, session) = shared_fixture();
        let org = s.add_org("platform", None, false).expect("org").id;
        let e = s
            .grant_session(session, GrantRecipient::Org(org), GRANT_WATCH, a)
            .expect_err("no org recipients in M1");
        assert_eq!(e.code, codes::E_INVALID);
        assert!(s.grants_for_session(session).unwrap().is_empty());

        // Written by hand (an M2 row, or a hand-edited database): the
        // per-person read keys on `person_id`, so it is nobody's access.
        s.conn
            .execute(
                "INSERT INTO session_grants (session_id, org_id, level, granted_by, granted_at) \
                 VALUES (?1, ?2, 'drive', ?3, 1)",
                rusqlite::params![session, org, a],
            )
            .expect("the column accepts it");
        assert!(s.grants_for_person(b).unwrap().is_empty());
        assert_eq!(granted_level(&s, session, b), None);

        // And the `CHECK` holds the other two shapes out of the table
        // entirely: a grant to nobody, and one to both.
        for (person, org) in [(None, None), (Some(b), Some(org))] {
            assert!(
                s.conn
                    .execute(
                        "INSERT INTO session_grants \
                           (session_id, person_id, org_id, level, granted_by, granted_at) \
                         VALUES (?1, ?2, ?3, 'watch', ?4, 1)",
                        rusqlite::params![session, person, org, a],
                    )
                    .is_err(),
                "exactly one of person_id / org_id must be set ({person:?}, {org:?})"
            );
        }
    }

    /// A level is `watch` or `drive`, and `own` is not a level at all —
    /// it is the tier of operations only the owner may perform.
    #[test]
    fn a_level_is_watch_or_drive_and_own_is_not_grantable() {
        let (s, a, b, session) = shared_fixture();
        for bad in ["own", "", "WATCH", "admin", "read"] {
            let e = s
                .grant_session(session, GrantRecipient::Person(b), bad, a)
                .expect_err("not a level");
            assert_eq!(e.code, codes::E_VALIDATE, "level {bad:?}");
        }
        assert!(validate_grant_level("own")
            .expect_err("own")
            .message
            .contains("only the owner"));
        assert!(s.grants_for_session(session).unwrap().is_empty());
    }

    /// `sessions.id` is a rowid alias (migration 001) and SQLite reuses the
    /// highest deleted value, so a grant that outlived its session would
    /// re-attach to whatever row lands on that id next. The cascade is pinned
    /// rather than trusted, because `delete_session` hand-deletes
    /// `session_events` instead of relying on one.
    #[test]
    fn deleting_a_session_cascades_its_grants_and_a_reused_rowid_inherits_none() {
        let (s, a, b, session) = shared_fixture();
        s.grant_session(session, GrantRecipient::Person(b), GRANT_DRIVE, a)
            .expect("share");
        let revoked_too = owned_session(&s, "other", Some(a));
        s.grant_session(revoked_too, GrantRecipient::Person(b), GRANT_WATCH, a)
            .expect("share");
        s.revoke_session_grant(revoked_too, b, a).expect("revoke");

        s.delete_session(session).expect("reap the session");
        let left: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM session_grants WHERE session_id = ?1",
                [session],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0, "the live grant went with the session");

        // Now reuse the id: with `AUTOINCREMENT` absent from `sessions`,
        // the next insert can land on the freed rowid. Force it, which is the
        // case a leaked grant would silently widen.
        s.conn
            .execute(
                "INSERT INTO sessions (id, tmux_name, host_alias, created_at, last_activity_at, \
                                       status, owner_person_id, visibility) \
                 VALUES (?1, 'someone-elses', 'h', 1, 1, 'running', NULL, 'unclaimed')",
                [session],
            )
            .expect("the id is free again");
        assert!(s.grants_for_session(session).unwrap().is_empty());
        assert!(!s.grants_for_person(b).unwrap().contains_key(&session));

        // A revoked grant cascades too — it is a row of the session, and the
        // cascade is not partial.
        s.delete_session(revoked_too).expect("reap the other");
        let left: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM session_grants WHERE session_id = ?1",
                [revoked_too],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0);
    }

    /// Disabling a person removes their REACH: their devices stop answering
    /// and every grant to them is revoked, in one transaction. Their own
    /// sessions are untouched — still theirs, still `private` — because M1 has
    /// no operation that re-attributes a departed colleague's work.
    #[test]
    fn disabling_a_person_revokes_their_tokens_and_every_grant_to_them() {
        let (s, a, b, session) = shared_fixture();
        let second = owned_session(&s, "second", Some(a));
        let theirs = owned_session(&s, "bobs-own", Some(b));
        s.grant_session(session, GrantRecipient::Person(b), GRANT_DRIVE, a)
            .expect("share one");
        s.grant_session(second, GrantRecipient::Person(b), GRANT_WATCH, a)
            .expect("share two");
        // A grant BY the departing person stays: it is the owner's share of
        // the owner's own session, and the owner here is still ann.
        let c = s.create_person("cat", None).unwrap().id;
        s.grant_session(theirs, GrantRecipient::Person(c), GRANT_WATCH, b)
            .expect("bob shares his own");
        s.insert_client_token("bobs-phone", &"b".repeat(64), "full")
            .expect("pair");
        s.set_client_person("bobs-phone", Some(b)).expect("bind");

        let before = grant_generation();
        s.disable_person(b).expect("bob leaves");

        assert!(s.grants_for_person(b).unwrap().is_empty());
        assert!(
            grant_generation() > before,
            "the sweep bumps the generation"
        );
        let live: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM client_tokens WHERE person_id = ?1 AND revoked_at IS NULL",
                [b],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(live, 0, "every device of theirs is revoked");
        // The audit trail: two revoked rows, not two deleted ones.
        let revoked: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM session_grants \
                  WHERE person_id = ?1 AND revoked_at IS NOT NULL",
                [b],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(revoked, 2);
        // Their own session is untouched, and so is the share they made of it.
        let row = s.get_session("bobs-own", "h").unwrap().expect("row");
        assert_eq!(row.owner_person_id, Some(b));
        assert_eq!(row.visibility, "private");
        assert_eq!(granted_level(&s, theirs, c).as_deref(), Some(GRANT_WATCH));
        // And a fresh share WITH them is refused rather than quietly restoring
        // the reach that was just taken away.
        assert_eq!(
            s.grant_session(session, GrantRecipient::Person(b), GRANT_WATCH, a)
                .expect_err("bob is disabled")
                .code,
            codes::E_VALIDATE
        );
    }

    /// The mirror of `store/read_pool.rs::the_auth_epoch_moves_on_every_token_write_and_not_on_a_touch`:
    /// a grant change moves the GRANT counter and the DEVICE epoch moves for
    /// device writes. Revoking a share and revoking a phone are different
    /// events with different mechanisms (rule 8), so neither may ride the
    /// other.
    ///
    /// The grant counter is process-wide, so a sibling test running in
    /// parallel can only inflate it: every assertion here is a lower bound on
    /// the rise and never an equality. The device epoch is a row in THIS
    /// store's database, so that half is exact.
    #[test]
    fn a_grant_change_bumps_grant_generation_and_not_the_auth_epoch() {
        let (s, a, b, session) = shared_fixture();
        let epoch = s.auth_epoch().unwrap();
        let last = std::cell::Cell::new(grant_generation());
        let moved = |what: &str| {
            let (was, now) = (last.get(), grant_generation());
            assert!(now > was, "{what} did not bump the grant generation");
            last.set(now);
            assert_eq!(
                s.auth_epoch().unwrap(),
                epoch,
                "{what} must not move the device auth epoch"
            );
        };
        s.grant_session(session, GrantRecipient::Person(b), GRANT_DRIVE, a)
            .expect("share");
        moved("granting");
        s.narrow_session_grant(session, b, a).expect("narrow");
        moved("narrowing");
        s.revoke_session_grant(session, b, a).expect("revoke");
        moved("revoking");
        s.grant_session(session, GrantRecipient::Person(b), GRANT_WATCH, a)
            .expect("share again");
        moved("granting again");
        assert_eq!(s.revoke_all_grants_for_person(b).unwrap(), 1);
        moved("revoking every grant to a person");

        // A refused write changes nothing. That it also announces nothing is
        // deliberately NOT asserted as "the counter stood still": the counter
        // is one process-wide atomic and the test binary runs these in
        // parallel, so only its MONOTONIC rise is observable from here. What
        // is observable is that the refused calls wrote no row — the sweep
        // finds nothing left to revoke, and the refused grant is an error.
        assert!(s
            .grant_session(session, GrantRecipient::Person(b), GRANT_WATCH, b)
            .is_err());
        assert_eq!(s.revoke_all_grants_for_person(b).unwrap(), 0);
        assert!(s.grants_for_person(b).unwrap().is_empty());

        // The device epoch still moves for a device write, on the same store:
        // the two mechanisms are both live, and separate.
        s.insert_client_token("phone", &"a".repeat(64), "full")
            .expect("pair");
        assert!(s.auth_epoch().unwrap() > epoch);
    }

    /// Narrowing and revoking name a grant that may not exist, and the
    /// answers are the ones a tool can report without guessing.
    #[test]
    fn narrowing_or_revoking_what_is_not_shared_is_not_found() {
        let (s, a, b, session) = shared_fixture();
        assert_eq!(
            s.narrow_session_grant(session, b, a)
                .expect_err("nothing to narrow")
                .code,
            codes::E_NOTFOUND
        );
        assert_eq!(
            s.revoke_session_grant(session, b, a)
                .expect_err("nothing to revoke")
                .code,
            codes::E_NOTFOUND
        );
        s.grant_session(session, GrantRecipient::Person(b), GRANT_DRIVE, a)
            .expect("share");
        s.revoke_session_grant(session, b, a).expect("revoke");
        // A second revoke is `E_NOTFOUND`, not a silent success: the grant it
        // names is not in force, and the row it would touch is history.
        assert_eq!(
            s.revoke_session_grant(session, b, a)
                .expect_err("already revoked")
                .code,
            codes::E_NOTFOUND
        );
        assert_eq!(
            s.narrow_session_grant(session, b, a)
                .expect_err("already revoked")
                .code,
            codes::E_NOTFOUND
        );
    }

    /// Two owners, two sessions, two recipients: the per-person read answers
    /// with that person's grants and nobody else's, and a grant to one person
    /// is not a grant to the other.
    #[test]
    fn grants_are_per_person_and_per_session() {
        let (s, a, b, session) = shared_fixture();
        let c = s.create_person("cat", None).unwrap().id;
        let bs = owned_session(&s, "bobs", Some(b));
        s.grant_session(session, GrantRecipient::Person(b), GRANT_WATCH, a)
            .expect("ann → bob");
        s.grant_session(session, GrantRecipient::Person(c), GRANT_DRIVE, a)
            .expect("ann → cat");
        s.grant_session(bs, GrantRecipient::Person(c), GRANT_WATCH, b)
            .expect("bob → cat");

        assert_eq!(
            s.grants_for_person(b).unwrap(),
            BTreeMap::from([(session, GRANT_WATCH.to_string())])
        );
        assert_eq!(
            s.grants_for_person(c).unwrap(),
            BTreeMap::from([
                (session, GRANT_DRIVE.to_string()),
                (bs, GRANT_WATCH.to_string())
            ])
        );
        assert!(s.grants_for_person(a).unwrap().is_empty());
        assert_eq!(
            s.grants_for_session(session)
                .unwrap()
                .iter()
                .map(|g| (g.person_id, g.level.clone()))
                .collect::<Vec<_>>(),
            vec![
                (Some(b), GRANT_WATCH.to_string()),
                (Some(c), GRANT_DRIVE.to_string())
            ]
        );
        // Ann narrowing her own share of her own session does not touch bob's.
        s.narrow_session_grant(session, c, a).expect("narrow cat");
        assert_eq!(
            s.grants_for_person(c).unwrap().get(&bs).map(String::as_str),
            Some(GRANT_WATCH)
        );
        assert_eq!(granted_level(&s, session, c).as_deref(), Some(GRANT_WATCH));
    }

    /// A share with somebody who does not exist is `E_NOTFOUND`, and a share
    /// with the owner themselves is refused: it would confer nothing and would
    /// then show up in the owner's own grant set.
    #[test]
    fn a_recipient_must_be_a_live_person_other_than_the_owner() {
        let (s, a, b, session) = shared_fixture();
        assert_eq!(
            s.grant_session(session, GrantRecipient::Person(9_999), GRANT_WATCH, a)
                .expect_err("no such person")
                .code,
            codes::E_NOTFOUND
        );
        assert_eq!(
            s.grant_session(session, GrantRecipient::Person(a), GRANT_WATCH, a)
                .expect_err("the owner already has it")
                .code,
            codes::E_VALIDATE
        );
        assert!(s.grants_for_session(session).unwrap().is_empty());
        s.grant_session(session, GrantRecipient::Person(b), GRANT_WATCH, a)
            .expect("a live colleague");
    }
}
