//! Hub↔hub federation links (migration 045): one row per link per side, the
//! remote participants that stand for a foreign session, the outbox, and the
//! idempotent inbound insert. See
//! `docs/superpowers/specs/2026-09-24-hub-federation-design.md`.

use super::{now_unix, Store};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;

pub const LINK_ROLE_DIALER: &str = "dialer";
pub const LINK_ROLE_LISTENER: &str = "listener";
pub const LINK_CONNECTED: &str = "connected";
pub const LINK_RETRYING: &str = "retrying";
pub const LINK_REFUSED: &str = "refused";
pub const LINK_INCOMPATIBLE: &str = "incompatible";
/// A message waiting for a peer this long is failed back to its sender.
pub const PEER_PENDING_MAX_SECS: i64 = 7 * 24 * 60 * 60;

const LINK_COLUMNS: &str = "id, fleet_id, role, url, token, client_id, after, \
    pending_rejects, state, last_exchange_at, last_error, created_at, revoked_at";

/// One row of `peer_links`. Deliberately NOT `Serialize` — `token` is a
/// secret; use [`PeerLinkSummary`] wherever a link is reported outward.
#[derive(Debug, Clone)]
pub struct PeerLinkRow {
    pub id: i64,
    pub fleet_id: Option<String>,
    pub role: String,
    pub url: Option<String>,
    pub token: Option<String>,
    pub client_id: Option<i64>,
    pub after: i64,
    pub pending_rejects: Option<String>,
    pub state: String,
    pub last_exchange_at: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub revoked_at: Option<i64>,
}

/// The reportable shape of a link: everything but the secret token, plus the
/// count of its pending outbox rows.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PeerLinkSummary {
    pub id: i64,
    pub fleet_id: Option<String>,
    pub role: String,
    pub url: Option<String>,
    pub state: String,
    pub last_exchange_at: Option<i64>,
    pub last_error: Option<String>,
    pub pending: i64,
    pub revoked_at: Option<i64>,
}

/// One outbound row waiting for (or handed to) a peer.
#[derive(Debug, Clone)]
pub struct OutboxRow {
    pub id: i64,
    pub from_addr: String,
    pub to_addr: String,
    pub body: String,
    pub kind: String,
    pub reply_to: Option<i64>,
    pub sent_at: i64,
    pub wake: bool,
}

/// The outcome of [`Store::insert_inbound_remote`]: either a fresh row, or
/// the id of the row this exact `(remote_fleet_id, remote_message_id)`
/// already produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inbound {
    Inserted(i64),
    Duplicate(i64),
}

fn map_link(r: &rusqlite::Row<'_>) -> rusqlite::Result<PeerLinkRow> {
    Ok(PeerLinkRow {
        id: r.get(0)?,
        fleet_id: r.get(1)?,
        role: r.get(2)?,
        url: r.get(3)?,
        token: r.get(4)?,
        client_id: r.get(5)?,
        after: r.get(6)?,
        pending_rejects: r.get(7)?,
        state: r.get(8)?,
        last_exchange_at: r.get(9)?,
        last_error: r.get(10)?,
        created_at: r.get(11)?,
        revoked_at: r.get(12)?,
    })
}

/// Whether `e` is a SQLite UNIQUE constraint violation. A private copy of
/// `clients::is_unique_violation` — that one is private to its own module,
/// and this file's constraint (`idx_peer_links_live_fleet`) is unrelated to
/// client tokens, so a shared helper would only add an import for a five-line
/// match.
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

/// Maps a unique-constraint violation on `idx_peer_links_live_fleet` to
/// `E_EXISTS`; anything else passes through as the ordinary `E_SQLITE`
/// mapping.
fn map_live_fleet_conflict(e: rusqlite::Error) -> IpcError {
    if is_unique_violation(&e) {
        IpcError::new(
            codes::E_EXISTS,
            "this fleet is already linked the other way",
        )
    } else {
        IpcError::from(e)
    }
}

impl Store {
    pub fn insert_dialer_link(&self, url: &str, token: &str) -> Result<i64, IpcError> {
        self.conn.execute(
            "INSERT INTO peer_links (role, url, token, state, created_at) \
             VALUES ('dialer', ?1, ?2, 'retrying', ?3)",
            rusqlite::params![url, token, now_unix()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The dialer's handshake answer. If another live dialer row already has
    /// this fleet (a re-pair after a refusal), the new url and token move onto
    /// that row and this one is dropped, so its pending rows stay attached.
    pub fn adopt_dialer_fleet(&self, id: i64, fleet_id: &str) -> Result<i64, IpcError> {
        self.atomically(|s| {
            let other: Option<i64> = s
                .conn
                .query_row(
                    "SELECT id FROM peer_links WHERE fleet_id = ?1 AND revoked_at IS NULL \
                     AND role = 'dialer' AND id != ?2",
                    rusqlite::params![fleet_id, id],
                    |r| r.get(0),
                )
                .optional()?;
            match other {
                None => {
                    s.conn
                        .execute(
                            "UPDATE peer_links SET fleet_id = ?1 WHERE id = ?2",
                            rusqlite::params![fleet_id, id],
                        )
                        .map_err(map_live_fleet_conflict)?;
                    Ok(id)
                }
                Some(keep) => {
                    s.conn.execute(
                        "UPDATE peer_links SET \
                           url = (SELECT url FROM peer_links WHERE id = ?1), \
                           token = (SELECT token FROM peer_links WHERE id = ?1), \
                           state = 'retrying', last_error = NULL \
                         WHERE id = ?2",
                        rusqlite::params![id, keep],
                    )?;
                    s.conn
                        .execute("DELETE FROM peer_links WHERE id = ?1", [id])?;
                    Ok(keep)
                }
            }
        })
    }

    /// The listener's side of the handshake: the link for this client token,
    /// created on first sight, refused when the token claims another fleet.
    pub fn ensure_listener_link(
        &self,
        client_id: i64,
        fleet_id: &str,
    ) -> Result<PeerLinkRow, IpcError> {
        self.atomically(|s| {
            let found: Option<PeerLinkRow> = s
                .conn
                .query_row(
                    &format!("SELECT {LINK_COLUMNS} FROM peer_links WHERE client_id = ?1"),
                    [client_id],
                    map_link,
                )
                .optional()?;
            if let Some(row) = found {
                if row.revoked_at.is_some() {
                    return Err(IpcError::new(
                        codes::E_FORBIDDEN,
                        "this hub link was removed",
                    ));
                }
                if row.fleet_id.as_deref() != Some(fleet_id) {
                    return Err(IpcError::new(
                        codes::E_FORBIDDEN,
                        "this token is pinned to another fleet",
                    ));
                }
                return Ok(row);
            }
            // A re-pair of a fleet already linked: rebind its live row to the
            // new token so pending rows stay attached.
            let n = s.conn.execute(
                "UPDATE peer_links SET client_id = ?1, state = 'connected', last_error = NULL \
                 WHERE fleet_id = ?2 AND role = 'listener' AND revoked_at IS NULL",
                rusqlite::params![client_id, fleet_id],
            )?;
            if n == 0 {
                s.conn
                    .execute(
                        "INSERT INTO peer_links (fleet_id, role, client_id, state, created_at) \
                         VALUES (?1, 'listener', ?2, 'connected', ?3)",
                        rusqlite::params![fleet_id, client_id, now_unix()],
                    )
                    .map_err(map_live_fleet_conflict)?;
            }
            s.conn
                .query_row(
                    &format!("SELECT {LINK_COLUMNS} FROM peer_links WHERE client_id = ?1"),
                    [client_id],
                    map_link,
                )
                .map_err(IpcError::from)
        })
    }

    pub fn peer_link(&self, id: i64) -> Result<Option<PeerLinkRow>, IpcError> {
        self.conn
            .query_row(
                &format!("SELECT {LINK_COLUMNS} FROM peer_links WHERE id = ?1"),
                [id],
                map_link,
            )
            .optional()
            .map_err(IpcError::from)
    }

    /// A non-revoked link for `fleet_id`, of either role.
    pub fn live_peer_link_for_fleet(
        &self,
        fleet_id: &str,
    ) -> Result<Option<PeerLinkRow>, IpcError> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {LINK_COLUMNS} FROM peer_links \
                     WHERE fleet_id = ?1 AND revoked_at IS NULL"
                ),
                [fleet_id],
                map_link,
            )
            .optional()
            .map_err(IpcError::from)
    }

    pub fn live_dialer_links(&self) -> Result<Vec<PeerLinkRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {LINK_COLUMNS} FROM peer_links WHERE role = 'dialer' AND revoked_at IS NULL"
        ))?;
        let rows = stmt.query_map([], map_link)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn peer_link_summaries(&self) -> Result<Vec<PeerLinkSummary>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, fleet_id, role, url, state, last_exchange_at, last_error, revoked_at, \
                    (SELECT COUNT(*) FROM session_messages m \
                       JOIN participants p ON p.id = m.to_participant_id \
                      WHERE p.peer_link_id = peer_links.id AND m.peer_state = 'pending') \
             FROM peer_links ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(PeerLinkSummary {
                id: r.get(0)?,
                fleet_id: r.get(1)?,
                role: r.get(2)?,
                url: r.get(3)?,
                state: r.get(4)?,
                last_exchange_at: r.get(5)?,
                last_error: r.get(6)?,
                revoked_at: r.get(7)?,
                pending: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn set_peer_link_state(
        &self,
        id: i64,
        state: &str,
        last_error: Option<&str>,
        now: i64,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE peer_links SET state = ?1, last_error = ?2, last_exchange_at = ?3 \
             WHERE id = ?4",
            rusqlite::params![state, last_error, now, id],
        )?;
        Ok(())
    }

    pub fn set_peer_link_progress(
        &self,
        id: i64,
        after: i64,
        pending_rejects: Option<&str>,
        now: i64,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE peer_links SET after = ?1, pending_rejects = ?2, state = 'connected', \
                                   last_error = NULL, last_exchange_at = ?3 \
             WHERE id = ?4",
            rusqlite::params![after, pending_rejects, now, id],
        )?;
        Ok(())
    }

    /// Revoke link `id`: stamps `revoked_at`, revokes its listener client
    /// token (if any), and fails every pending outbox row on it. Returns how
    /// many rows were failed.
    pub fn revoke_peer_link(&self, id: i64, now: i64) -> Result<usize, IpcError> {
        self.atomically(|s| {
            s.conn.execute(
                "UPDATE peer_links SET revoked_at = ?1 WHERE id = ?2 AND revoked_at IS NULL",
                rusqlite::params![now, id],
            )?;
            let client_id: Option<i64> = s.conn.query_row(
                "SELECT client_id FROM peer_links WHERE id = ?1",
                [id],
                |r| r.get(0),
            )?;
            if let Some(cid) = client_id {
                s.conn.execute(
                    "UPDATE client_tokens SET revoked_at = ?1 WHERE id = ?2 AND revoked_at IS NULL",
                    rusqlite::params![now, cid],
                )?;
            }
            let pending_ids: Vec<i64> = {
                let mut stmt = s.conn.prepare(
                    "SELECT m.id FROM session_messages m \
                       JOIN participants p ON p.id = m.to_participant_id \
                      WHERE p.peer_link_id = ?1 AND m.peer_state = 'pending'",
                )?;
                let rows = stmt.query_map([id], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            let mut failed = 0;
            for mid in pending_ids {
                if s.fail_pending_locked(mid, "the hub link was removed")? {
                    failed += 1;
                }
            }
            Ok(failed)
        })
    }

    pub fn ensure_remote_participant(&self, link_id: i64, address: &str) -> Result<i64, IpcError> {
        if let Some(id) = self
            .conn
            .query_row(
                "SELECT id FROM participants WHERE address = ?1",
                [address],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
        {
            // `IS NOT` (NULL-safe) skips the write when this address is
            // already pinned to this link — the common case once a link is
            // established, and no-op writes are not free under the store's
            // single connection.
            self.conn.execute(
                "UPDATE participants SET peer_link_id = ?1 WHERE id = ?2 AND peer_link_id IS NOT ?1",
                rusqlite::params![link_id, id],
            )?;
            return Ok(id);
        }
        self.conn.execute(
            "INSERT INTO participants (kind, address, peer_link_id, created_at) \
             VALUES ('remote', ?1, ?2, ?3)",
            rusqlite::params![address, link_id, now_unix()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_outbound_remote(
        &self,
        from_session_id: i64,
        from_addr: &str,
        remote_participant_id: i64,
        body: &str,
        kind: &str,
        reply_to: Option<i64>,
        wake: bool,
    ) -> Result<i64, IpcError> {
        let from_p = self.ensure_participant_for_session(from_session_id)?;
        self.conn.execute(
            "INSERT INTO session_messages \
               (from_session_id, to_session_id, from_participant_id, to_participant_id, \
                body, kind, sent_at, reply_to, peer_state, peer_wake, peer_from_addr) \
             VALUES (?1, 0, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8, ?9)",
            rusqlite::params![
                from_session_id,
                from_p,
                remote_participant_id,
                body,
                kind,
                now_unix(),
                reply_to,
                wake as i64,
                from_addr
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.message_notify.notify_waiters();
        Ok(id)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_inbound_remote(
        &self,
        remote_fleet_id: &str,
        remote_message_id: i64,
        remote_participant_id: i64,
        to_session_id: i64,
        body: &str,
        kind: &str,
        reply_to: Option<i64>,
    ) -> Result<Inbound, IpcError> {
        if let Some(id) = self.local_id_for_remote(remote_fleet_id, remote_message_id)? {
            return Ok(Inbound::Duplicate(id));
        }
        let to_p = self.ensure_participant_for_session(to_session_id)?;
        self.conn.execute(
            "INSERT INTO session_messages \
               (from_session_id, to_session_id, from_participant_id, to_participant_id, \
                body, kind, sent_at, reply_to, remote_fleet_id, remote_message_id) \
             VALUES (0, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                to_session_id,
                remote_participant_id,
                to_p,
                body,
                kind,
                now_unix(),
                reply_to,
                remote_fleet_id,
                remote_message_id
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.message_notify.notify_waiters();
        Ok(Inbound::Inserted(id))
    }

    pub fn pending_outbox(
        &self,
        link_id: i64,
        after_id: i64,
        limit: i64,
    ) -> Result<Vec<OutboxRow>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT m.id, COALESCE(m.peer_from_addr, ''), p.address, m.body, m.kind, \
                    m.reply_to, m.sent_at, m.peer_wake \
             FROM session_messages m JOIN participants p ON p.id = m.to_participant_id \
             WHERE p.peer_link_id = ?1 AND m.peer_state = 'pending' AND m.id > ?2 \
             ORDER BY m.id ASC LIMIT ?3",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![link_id, after_id, limit], |r| {
                Ok(OutboxRow {
                    id: r.get(0)?,
                    from_addr: r.get(1)?,
                    to_addr: r.get(2)?,
                    body: r.get(3)?,
                    kind: r.get(4)?,
                    reply_to: r.get(5)?,
                    sent_at: r.get(6)?,
                    wake: r.get::<_, i64>(7)? != 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn has_pending_outbox(&self, link_id: i64, after_id: i64) -> Result<bool, IpcError> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_messages m \
                             JOIN participants p ON p.id = m.to_participant_id \
                            WHERE p.peer_link_id = ?1 AND m.peer_state = 'pending' AND m.id > ?2)",
            rusqlite::params![link_id, after_id],
            |r| r.get(0),
        )?)
    }

    pub fn mark_peer_accepted(&self, ids: &[i64]) -> Result<usize, IpcError> {
        if ids.is_empty() {
            return Ok(0);
        }
        let sql = format!(
            "UPDATE session_messages SET peer_state = 'accepted' \
             WHERE peer_state = 'pending' AND id IN ({phs})",
            phs = super::in_clause(ids.len())
        );
        let params = super::params_then(&[], ids);
        Ok(self.conn.execute(&sql, params.as_slice())?)
    }

    /// The shared body behind `mark_peer_undeliverable` and `revoke_peer_link`
    /// — the latter must not re-enter `atomically`, so it calls this directly
    /// from inside its own closure.
    fn fail_pending_locked(&self, id: i64, reason: &str) -> Result<bool, IpcError> {
        let n = self.conn.execute(
            "UPDATE session_messages SET peer_state = 'undeliverable' \
             WHERE id = ?1 AND peer_state = 'pending'",
            [id],
        )?;
        if n == 0 {
            return Ok(false);
        }
        let sender: i64 = self.conn.query_row(
            "SELECT from_session_id FROM session_messages WHERE id = ?1",
            [id],
            |r| r.get(0),
        )?;
        if sender != 0 {
            self.insert_session_event(
                sender,
                "message_undeliverable",
                Some(&format!("message {id} could not be delivered: {reason}")),
            )?;
        }
        Ok(true)
    }

    pub fn mark_peer_undeliverable(&self, id: i64, reason: &str) -> Result<bool, IpcError> {
        self.atomically(|s| s.fail_pending_locked(id, reason))
    }

    /// `pending` → `accepted` for this link's rows at or below `after` — the
    /// listener applying the dialer's watermark.
    pub fn handover_upto(&self, link_id: i64, after: i64) -> Result<usize, IpcError> {
        Ok(self.conn.execute(
            "UPDATE session_messages SET peer_state = 'accepted' \
             WHERE peer_state = 'pending' AND id <= ?2 \
               AND to_participant_id IN (SELECT id FROM participants WHERE peer_link_id = ?1)",
            rusqlite::params![link_id, after],
        )?)
    }

    pub fn local_id_for_remote(
        &self,
        remote_fleet_id: &str,
        remote_message_id: i64,
    ) -> Result<Option<i64>, IpcError> {
        self.conn
            .query_row(
                "SELECT id FROM session_messages \
                 WHERE remote_fleet_id = ?1 AND remote_message_id = ?2",
                rusqlite::params![remote_fleet_id, remote_message_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(IpcError::from)
    }

    /// `(remote_fleet_id, remote_message_id)` of message `id`, when it came
    /// from a peer.
    pub fn remote_ref_of(&self, id: i64) -> Result<Option<(String, i64)>, IpcError> {
        self.conn
            .query_row(
                "SELECT remote_fleet_id, remote_message_id FROM session_messages \
                 WHERE id = ?1 AND remote_fleet_id IS NOT NULL AND remote_message_id IS NOT NULL",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(IpcError::from)
    }

    /// Fails outbox rows that have been pending too long, or that sit on a
    /// revoked link; then drops any `remote` participant left with no
    /// messages at all. Returns how many rows were failed.
    pub fn sweep_peer_outbox(&self, now: i64, older_than_secs: i64) -> Result<usize, IpcError> {
        self.atomically(|s| {
            let stale: Vec<i64> = {
                let mut stmt = s.conn.prepare(
                    "SELECT id FROM session_messages \
                     WHERE peer_state = 'pending' AND sent_at <= ?1",
                )?;
                let rows = stmt.query_map([now - older_than_secs], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            let mut failed = 0;
            for id in stale {
                if s.fail_pending_locked(id, "no answer from the peer hub within 7 days")? {
                    failed += 1;
                }
            }

            let on_revoked: Vec<i64> = {
                let mut stmt = s.conn.prepare(
                    "SELECT m.id FROM session_messages m \
                       JOIN participants p ON p.id = m.to_participant_id \
                       JOIN peer_links l ON l.id = p.peer_link_id \
                     WHERE m.peer_state = 'pending' AND l.revoked_at IS NOT NULL",
                )?;
                let rows = stmt.query_map([], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            for id in on_revoked {
                if s.fail_pending_locked(id, "the hub link was removed")? {
                    failed += 1;
                }
            }

            s.conn.execute(
                "DELETE FROM participants WHERE kind = 'remote' AND id NOT IN ( \
                    SELECT from_participant_id FROM session_messages \
                     WHERE from_participant_id IS NOT NULL \
                    UNION \
                    SELECT to_participant_id FROM session_messages \
                     WHERE to_participant_id IS NOT NULL)",
                [],
            )?;

            Ok(failed)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn seed(s: &Store, name: &str) -> i64 {
        s.upsert_host("local").unwrap();
        s.upsert_session(name, "local", None, None, 0, 0, "running", None)
            .unwrap()
    }
    fn client(s: &Store, name: &str) -> i64 {
        s.insert_client_token(name, &format!("{:0>64}", name.len()), "peer")
            .unwrap()
            .id
    }
    const B: &str = "fleet-b";
    const ADDR: &str = "fleet-b/session/h/b1";

    #[test]
    fn a_listener_link_pins_its_fleet_to_its_token() {
        let s = Store::open_in_memory().unwrap();
        let c = client(&s, "hub-a");
        let l = s.ensure_listener_link(c, "fleet-a").unwrap();
        assert_eq!(l.role, LINK_ROLE_LISTENER);
        assert_eq!(s.ensure_listener_link(c, "fleet-a").unwrap().id, l.id);
        let e = s.ensure_listener_link(c, "fleet-x").unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_FORBIDDEN);
    }

    #[test]
    fn a_repaired_fleet_moves_onto_its_old_dialer_row() {
        let s = Store::open_in_memory().unwrap();
        let old = s.insert_dialer_link("https://b.example", "t1").unwrap();
        assert_eq!(s.adopt_dialer_fleet(old, B).unwrap(), old);
        s.set_peer_link_state(old, LINK_REFUSED, Some("401"), 1)
            .unwrap();
        let new = s.insert_dialer_link("https://b2.example", "t2").unwrap();
        let kept = s.adopt_dialer_fleet(new, B).unwrap();
        assert_eq!(kept, old, "pending rows stay on the old link");
        let row = s.peer_link(old).unwrap().unwrap();
        assert_eq!(row.url.as_deref(), Some("https://b2.example"));
        assert_eq!(row.token.as_deref(), Some("t2"));
        assert_eq!(row.state, LINK_RETRYING);
        assert!(s.peer_link(new).unwrap().is_none());
    }

    /// `idx_peer_links_live_fleet` is unique on `fleet_id` among live rows,
    /// regardless of role: a dialer row already pinned to `B` means a fresh
    /// listener row can never claim `B` too (both hubs dialing each other).
    /// `ensure_listener_link`'s create-on-first-sight INSERT is the write
    /// that hits it.
    #[test]
    fn ensure_listener_link_refuses_a_fleet_already_linked_the_other_way() {
        let s = Store::open_in_memory().unwrap();
        let dialer = s.insert_dialer_link("https://b.example", "t").unwrap();
        assert_eq!(s.adopt_dialer_fleet(dialer, B).unwrap(), dialer);

        let c = client(&s, "hub-b");
        let err = s.ensure_listener_link(c, B).unwrap_err();
        assert_eq!(err.code, crate::ipc_error::codes::E_EXISTS);
        assert!(
            err.message.contains("already linked the other way"),
            "{}",
            err.message
        );

        // The refused call wrote no row: only the original dialer link
        // exists, and the client token it was handed pins to nothing.
        assert_eq!(s.peer_link_summaries().unwrap().len(), 1);
        assert!(s
            .peer_link_summaries()
            .unwrap()
            .iter()
            .all(|l| l.role == LINK_ROLE_DIALER));
    }

    /// The mirror of the test above: a listener row already pinned to `B`
    /// means `adopt_dialer_fleet`'s plain `UPDATE ... SET fleet_id` (its
    /// no-other-dialer-row branch) is the write that hits the same
    /// constraint.
    #[test]
    fn adopt_dialer_fleet_refuses_a_fleet_already_linked_the_other_way() {
        let s = Store::open_in_memory().unwrap();
        let c = client(&s, "hub-b");
        let listener = s.ensure_listener_link(c, B).unwrap();
        assert_eq!(listener.fleet_id.as_deref(), Some(B));

        let fresh = s.insert_dialer_link("https://b.example", "t").unwrap();
        let err = s.adopt_dialer_fleet(fresh, B).unwrap_err();
        assert_eq!(err.code, crate::ipc_error::codes::E_EXISTS);
        assert!(
            err.message.contains("already linked the other way"),
            "{}",
            err.message
        );

        // The refused call left the fresh dialer row untouched and created
        // nothing: still exactly the listener row plus the unpinned dialer.
        assert_eq!(s.peer_link_summaries().unwrap().len(), 2);
        assert_eq!(s.peer_link(fresh).unwrap().unwrap().fleet_id, None);
    }

    #[test]
    fn an_inbound_message_is_inserted_once() {
        let s = Store::open_in_memory().unwrap();
        let b1 = seed(&s, "b1");
        let link = s
            .ensure_listener_link(client(&s, "hub-a"), "fleet-a")
            .unwrap();
        let from = s
            .ensure_remote_participant(link.id, "fleet-a/session/h/a1")
            .unwrap();
        let first = s
            .insert_inbound_remote("fleet-a", 17, from, b1, "hi", "message", None)
            .unwrap();
        let Inbound::Inserted(id) = first else {
            panic!("{first:?}")
        };
        assert!(matches!(
            s.insert_inbound_remote("fleet-a", 17, from, b1, "hi", "message", None)
                .unwrap(),
            Inbound::Duplicate(d) if d == id
        ));
        assert_eq!(s.list_inbox(b1, false, 10).unwrap().len(), 1);
        assert_eq!(s.local_id_for_remote("fleet-a", 17).unwrap(), Some(id));
    }

    #[test]
    fn the_outbox_is_handed_over_by_the_watermark() {
        let s = Store::open_in_memory().unwrap();
        let a1 = seed(&s, "a1");
        let link = s.insert_dialer_link("https://b.example", "t").unwrap();
        s.adopt_dialer_fleet(link, B).unwrap();
        let to = s.ensure_remote_participant(link, ADDR).unwrap();
        let m1 = s
            .insert_outbound_remote(
                a1,
                "fleet-a/session/local/a1",
                to,
                "one",
                "message",
                None,
                true,
            )
            .unwrap();
        let m2 = s
            .insert_outbound_remote(
                a1,
                "fleet-a/session/local/a1",
                to,
                "two",
                "message",
                None,
                false,
            )
            .unwrap();
        let page = s.pending_outbox(link, 0, 50).unwrap();
        assert_eq!(page.iter().map(|r| r.id).collect::<Vec<_>>(), vec![m1, m2]);
        assert_eq!(page[0].to_addr, ADDR);
        assert!(page[0].wake && !page[1].wake);
        assert_eq!(s.handover_upto(link, m1).unwrap(), 1);
        assert_eq!(s.pending_outbox(link, 0, 50).unwrap().len(), 1);
        assert!(s.has_pending_outbox(link, m1).unwrap());
        assert!(!s.has_pending_outbox(link, m2).unwrap());
    }

    #[test]
    fn an_undeliverable_message_tells_its_local_sender() {
        let s = Store::open_in_memory().unwrap();
        let a1 = seed(&s, "a1");
        let link = s.insert_dialer_link("https://b.example", "t").unwrap();
        let to = s.ensure_remote_participant(link, ADDR).unwrap();
        let m = s
            .insert_outbound_remote(
                a1,
                "fleet-a/session/local/a1",
                to,
                "x",
                "message",
                None,
                false,
            )
            .unwrap();
        assert!(s
            .mark_peer_undeliverable(m, "E_PARTICIPANT_UNKNOWN: no session b1 on h")
            .unwrap());
        assert!(!s.mark_peer_undeliverable(m, "again").unwrap(), "only once");
        let ev = s.list_session_events(a1, 50).unwrap();
        let hit = ev
            .iter()
            .find(|e| e.kind == "message_undeliverable")
            .expect("event");
        assert!(hit
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("E_PARTICIPANT_UNKNOWN"));
    }

    #[test]
    fn revoking_a_link_fails_its_pending_rows_and_its_token() {
        let s = Store::open_in_memory().unwrap();
        let a1 = seed(&s, "a1");
        let c = client(&s, "hub-b");
        let link = s.ensure_listener_link(c, B).unwrap();
        let to = s.ensure_remote_participant(link.id, ADDR).unwrap();
        s.insert_outbound_remote(
            a1,
            "fleet-a/session/local/a1",
            to,
            "x",
            "message",
            None,
            false,
        )
        .unwrap();
        assert_eq!(s.revoke_peer_link(link.id, 5).unwrap(), 1);
        assert!(!s.client_token_is_live(c).unwrap());
        assert!(s.live_peer_link_for_fleet(B).unwrap().is_none());
    }

    #[test]
    fn the_sweep_fails_week_old_pending_rows() {
        let s = Store::open_in_memory().unwrap();
        let a1 = seed(&s, "a1");
        let link = s.insert_dialer_link("https://b.example", "t").unwrap();
        let to = s.ensure_remote_participant(link, ADDR).unwrap();
        let m = s
            .insert_outbound_remote(
                a1,
                "fleet-a/session/local/a1",
                to,
                "x",
                "message",
                None,
                false,
            )
            .unwrap();
        s.conn_ref()
            .execute("UPDATE session_messages SET sent_at = 0 WHERE id = ?1", [m])
            .unwrap();
        assert_eq!(
            s.sweep_peer_outbox(PEER_PENDING_MAX_SECS + 1, PEER_PENDING_MAX_SECS)
                .unwrap(),
            1
        );
        assert!(s.pending_outbox(link, 0, 50).unwrap().is_empty());
    }

    #[test]
    fn summaries_never_carry_the_token() {
        let s = Store::open_in_memory().unwrap();
        s.insert_dialer_link("https://b.example", "secret-token-value")
            .unwrap();
        let json = serde_json::to_string(&s.peer_link_summaries().unwrap()).unwrap();
        assert!(!json.contains("secret-token-value"), "{json}");
    }

    #[test]
    fn a_remote_end_reads_back_as_its_address_and_a_local_row_is_unchanged() {
        let s = Store::open_in_memory().unwrap();
        let b1 = seed(&s, "b1");
        let b2 = seed(&s, "b2");
        let link = s
            .ensure_listener_link(client(&s, "hub-a"), "fleet-a")
            .unwrap();
        let from = s
            .ensure_remote_participant(link.id, "fleet-a/session/h/a1")
            .unwrap();
        s.insert_inbound_remote("fleet-a", 1, from, b1, "hi", "message", None)
            .unwrap();
        s.insert_message(b2, b1, "local", "message", None).unwrap();
        let inbox = s.list_inbox(b1, false, 10).unwrap();
        let remote = inbox.iter().find(|m| m.body == "hi").unwrap();
        assert_eq!(remote.from_addr.as_deref(), Some("fleet-a/session/h/a1"));
        assert_eq!(remote.from_session_id, 0);
        let local = inbox.iter().find(|m| m.body == "local").unwrap();
        let json = serde_json::to_value(local).unwrap();
        assert!(
            json.get("from_addr").is_none() && json.get("to_addr").is_none(),
            "{json}"
        );
    }

    #[test]
    fn a_retired_recipient_of_a_remote_message_writes_no_event_for_session_zero() {
        let s = Store::open_in_memory().unwrap();
        let b1 = seed(&s, "b1");
        let link = s
            .ensure_listener_link(client(&s, "hub-a"), "fleet-a")
            .unwrap();
        let from = s
            .ensure_remote_participant(link.id, "fleet-a/session/h/a1")
            .unwrap();
        s.insert_inbound_remote("fleet-a", 1, from, b1, "hi", "message", None)
            .unwrap();
        s.delete_session(b1).unwrap();
        s.sweep_retired_participants(i64::MAX / 2, 0).unwrap();
        let n: i64 = s
            .conn_ref()
            .query_row(
                "SELECT COUNT(*) FROM session_events WHERE session_id = 0",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    }
}
