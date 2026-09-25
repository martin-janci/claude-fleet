//! Hub↔hub federation links (migration 054): one row per link per side, the
//! remote participants that stand for a foreign session, the outbox, and the
//! idempotent inbound insert. See
//! `docs/superpowers/specs/2026-09-24-hub-federation-design.md`.

use super::{now_unix, Store, PARTICIPANT_REMOTE};
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
/// A live LISTENER link counts as down in [`Store::peer_links_down`] once its
/// last served exchange is older than this (or it has none yet) — a dialer
/// link's own state already says so, but a listener has no "state" of its
/// own to go stale: it only ever moves when a handshake or an exchange
/// writes it (G14c).
pub const LISTENER_STALE_SECS: i64 = 120;

const LINK_COLUMNS: &str = "id, fleet_id, role, url, token, client_id, after, \
    pending_rejects, state, last_exchange_at, last_error, created_at, revoked_at";

/// One row of `peer_links`. Deliberately NOT `Serialize` — `token` is a
/// secret; use [`PeerLinkSummary`] wherever a link is reported outward. Its
/// `Debug` is hand-written for the same reason: the token prints as
/// `<redacted>`.
#[derive(Clone)]
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

impl std::fmt::Debug for PeerLinkRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerLinkRow")
            .field("id", &self.id)
            .field("fleet_id", &self.fleet_id)
            .field("role", &self.role)
            .field("url", &self.url)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("client_id", &self.client_id)
            .field("after", &self.after)
            .field("pending_rejects", &self.pending_rejects)
            .field("state", &self.state)
            .field("last_exchange_at", &self.last_exchange_at)
            .field("last_error", &self.last_error)
            .field("created_at", &self.created_at)
            .field("revoked_at", &self.revoked_at)
            .finish()
    }
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

/// The outcome of [`Store::adopt_dialer_fleet`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adopted {
    /// The link to use: this row, now pinned to the fleet, or the older
    /// stopped row it was merged into (this one is gone).
    Link(i64),
    /// The fleet's live dialer link (this id) is still running, so nothing
    /// changed: this row stays unpinned and asks again later.
    Waiting(i64),
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

/// Like [`map_live_fleet_conflict`], but for `ensure_listener_link`'s
/// create-on-first-sight insert (G10). A dialer's own handshake retries on
/// `E_EXISTS` until the operator resolves it (the dialer's `is_terminal`
/// excludes that code), which is fine when the conflict is the dialer's OWN
/// row to fix. A LISTENER conflict here means "fleet X already dials this
/// hub" — nothing the listener side does will ever make that go away by
/// retrying, so the same collision is `E_FORBIDDEN` instead: terminal for
/// whichever dialer hits it.
fn map_listener_live_fleet_conflict(e: rusqlite::Error) -> IpcError {
    if is_unique_violation(&e) {
        IpcError::new(
            codes::E_FORBIDDEN,
            "this fleet is already linked the other way",
        )
    } else {
        IpcError::from(e)
    }
}

/// The `message_undeliverable` reason `sweep_peer_outbox` gives a stale
/// outbox row (G24): derived from `older_than_secs` rather than the
/// hard-coded "7 days" the sweep's default happens to be today, so the text
/// stays honest if that default (or a test, or a future settable one) ever
/// differs. Whole days when it divides evenly, else whole hours, else plain
/// seconds — the coarsest unit that describes the window exactly.
fn stale_outbox_reason(older_than_secs: i64) -> String {
    const DAY: i64 = 24 * 60 * 60;
    const HOUR: i64 = 60 * 60;
    let (n, unit) = if older_than_secs > 0 && older_than_secs % DAY == 0 {
        (older_than_secs / DAY, "day")
    } else if older_than_secs > 0 && older_than_secs % HOUR == 0 {
        (older_than_secs / HOUR, "hour")
    } else {
        (older_than_secs, "second")
    };
    let plural = if n == 1 { "" } else { "s" };
    format!("no answer from the peer hub within {n} {unit}{plural}")
}

impl Store {
    pub fn insert_dialer_link(&self, url: &str, token: &str) -> Result<i64, IpcError> {
        self.conn.execute(
            &format!(
                "INSERT INTO peer_links (role, url, token, state, created_at) \
                 VALUES ('{LINK_ROLE_DIALER}', ?1, ?2, '{LINK_RETRYING}', ?3)"
            ),
            rusqlite::params![url, token, now_unix()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The dialer's handshake answer. If another live dialer row already has
    /// this fleet and that row has STOPPED (`refused` or `incompatible` — a
    /// re-pair after a refusal), the new url and token move onto that row and
    /// this one is dropped, so its pending rows stay attached. A live row in
    /// any other state is a working link: this handshake cannot take it over
    /// (any newly paired hub could otherwise claim a third fleet's id and
    /// receive its messages), so nothing changes and the answer is
    /// [`Adopted::Waiting`] — the caller asks again later, and merges once
    /// that row has stopped (a re-pair racing the old loop's refusal). A
    /// fleet this hub already listens for is `E_EXISTS`.
    ///
    /// The merge keeps the old row's outbox, not its watermark: `after` is
    /// the peer's word (the highest message id it ever answered with), and a
    /// hostile peer that once answered `i64::MAX` would otherwise have every
    /// row the honest peer queues after the re-pair marked accepted without
    /// delivery, until `peer remove` + `peer add`. Starting from 0 costs
    /// nothing — the peer hands over only rows still `pending`, and a row
    /// this hub already stored comes back as a duplicate (accepted, stored
    /// once) — so a re-pair is as clean as a fresh link.
    pub fn adopt_dialer_fleet(&self, id: i64, fleet_id: &str) -> Result<Adopted, IpcError> {
        self.atomically(|s| {
            let other: Option<(i64, String)> = s
                .conn
                .query_row(
                    &format!(
                        "SELECT id, state FROM peer_links WHERE fleet_id = ?1 \
                         AND revoked_at IS NULL AND role = '{LINK_ROLE_DIALER}' AND id != ?2"
                    ),
                    rusqlite::params![fleet_id, id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            match other {
                None => {
                    s.conn
                        .execute(
                            "UPDATE peer_links SET fleet_id = ?1 WHERE id = ?2",
                            rusqlite::params![fleet_id, id],
                        )
                        .map_err(|e| match map_live_fleet_conflict(e) {
                            c if c.code == codes::E_EXISTS => IpcError::new(
                                codes::E_EXISTS,
                                format!(
                                    "fleet {fleet_id} is already linked the other way \
                                     (it dials this hub); remove one of the two links"
                                ),
                            ),
                            other => other,
                        })?;
                    Ok(Adopted::Link(id))
                }
                Some((live, state)) if state != LINK_REFUSED && state != LINK_INCOMPATIBLE => {
                    Ok(Adopted::Waiting(live))
                }
                Some((keep, _)) => {
                    s.conn.execute(
                        &format!(
                            "UPDATE peer_links SET \
                               url = (SELECT url FROM peer_links WHERE id = ?1), \
                               token = (SELECT token FROM peer_links WHERE id = ?1), \
                               state = '{LINK_RETRYING}', last_error = NULL, after = 0 \
                             WHERE id = ?2"
                        ),
                        rusqlite::params![id, keep],
                    )?;
                    s.conn
                        .execute("DELETE FROM peer_links WHERE id = ?1", [id])?;
                    Ok(Adopted::Link(keep))
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
            // new token so pending rows stay attached — but ONLY once the old
            // token is revoked (or gone). A second live token claiming the
            // fleet is refused: otherwise any paired peer could claim another
            // fleet's id and take over its link (its outbox, its sender
            // addresses, its watermark).
            let live: Option<(i64, Option<i64>)> = s
                .conn
                .query_row(
                    &format!(
                        "SELECT id, client_id FROM peer_links \
                         WHERE fleet_id = ?1 AND role = '{LINK_ROLE_LISTENER}' AND revoked_at IS NULL"
                    ),
                    [fleet_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let mut n = 0;
            if let Some((link_id, old_client)) = live {
                if let Some(old) = old_client {
                    if s.client_token_is_live(old)? {
                        return Err(IpcError::new(
                            codes::E_FORBIDDEN,
                            format!(
                                "fleet {fleet_id} is already linked to another peer token; \
                                 revoke that client first (fleet-hub client revoke <name>)"
                            ),
                        ));
                    }
                }
                n = s.conn.execute(
                    &format!(
                        "UPDATE peer_links SET client_id = ?1, state = '{LINK_CONNECTED}', \
                                               last_error = NULL WHERE id = ?2"
                    ),
                    rusqlite::params![client_id, link_id],
                )?;
            }
            if n == 0 {
                s.conn
                    .execute(
                        &format!(
                            "INSERT INTO peer_links (fleet_id, role, client_id, state, created_at) \
                             VALUES (?1, '{LINK_ROLE_LISTENER}', ?2, '{LINK_CONNECTED}', ?3)"
                        ),
                        rusqlite::params![fleet_id, client_id, now_unix()],
                    )
                    // G10: a listener-side "already linked the other way"
                    // conflict is terminal (E_FORBIDDEN), not E_EXISTS — the
                    // dialer's own handshake keeps E_EXISTS (see
                    // `map_live_fleet_conflict`, used above and in
                    // `adopt_dialer_fleet`), which its retry loop can work
                    // through; a listener conflict never resolves itself.
                    .map_err(map_listener_live_fleet_conflict)?;
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
            "SELECT {LINK_COLUMNS} FROM peer_links \
             WHERE role = '{LINK_ROLE_DIALER}' AND revoked_at IS NULL"
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

    /// Live links this hub should worry about (G14c), for `fleet_health`'s
    /// `peer_links_down`: a DIALER counts as down whenever its `state` isn't
    /// `connected` (its retry loop already tracks this — `retrying`,
    /// `refused`, `incompatible`). A LISTENER has no such state of its own
    /// (`ensure_listener_link` only ever writes `connected`), so it counts as
    /// down instead when its client token has been revoked, or it has never
    /// served an exchange, or its last one is older than
    /// [`LISTENER_STALE_SECS`] — the only signals a listener has that its
    /// dialer stopped showing up. A revoked link is not "down": it is gone,
    /// and already excluded everywhere else in this module.
    pub fn peer_links_down(&self, now: i64) -> Result<u32, IpcError> {
        let stale_before = now - LISTENER_STALE_SECS;
        let n: i64 = self.conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM peer_links l WHERE l.revoked_at IS NULL AND ( \
                   (l.role = '{LINK_ROLE_DIALER}' AND l.state != '{LINK_CONNECTED}') \
                   OR (l.role = '{LINK_ROLE_LISTENER}' AND ( \
                     l.client_id IS NULL \
                     OR EXISTS (SELECT 1 FROM client_tokens c \
                                 WHERE c.id = l.client_id AND c.revoked_at IS NOT NULL) \
                     OR l.last_exchange_at IS NULL \
                     OR l.last_exchange_at < ?1 \
                   )) \
                 )"
            ),
            [stale_before],
            |r| r.get(0),
        )?;
        Ok(n as u32)
    }

    pub fn set_peer_link_state(
        &self,
        id: i64,
        state: &str,
        last_error: Option<&str>,
        now: i64,
    ) -> Result<(), IpcError> {
        // `revoked_at IS NULL`: a handler parked across a revoke must not
        // write a revoked link back to life when it returns.
        self.conn.execute(
            "UPDATE peer_links SET state = ?1, last_error = ?2, last_exchange_at = ?3 \
             WHERE id = ?4 AND revoked_at IS NULL",
            rusqlite::params![state, last_error, now, id],
        )?;
        Ok(())
    }

    /// [`Self::set_peer_link_state`] for a dialer loop: applies only while the
    /// row still carries `token`, the credentials the loop was started with.
    /// A loop outlived by a re-pair (new token moved onto its row) cannot
    /// write over the row. Returns whether the row was written.
    ///
    /// Does NOT stamp `last_exchange_at` (G14a): the dialer loop calls this
    /// only on a failed or refused exchange (`Fence::state` in `dial.rs`), so
    /// stamping it here made a link that has not actually talked to its peer
    /// in a while look freshly exchanged to an operator reading
    /// `last_exchange_at`, the moment it started failing. A SUCCESSFUL
    /// exchange stamps it instead — [`Self::set_dialer_link_progress`] on the
    /// dialer side, `set_peer_link_state` on a served listener exchange.
    pub fn set_dialer_link_state(
        &self,
        id: i64,
        token: &str,
        state: &str,
        last_error: Option<&str>,
        _now: i64,
    ) -> Result<bool, IpcError> {
        let n = self.conn.execute(
            "UPDATE peer_links SET state = ?1, last_error = ?2 \
             WHERE id = ?3 AND token = ?4 AND revoked_at IS NULL",
            rusqlite::params![state, last_error, id, token],
        )?;
        Ok(n > 0)
    }

    /// The dialer-side success path: stamps `last_exchange_at`, fenced on
    /// `token` like [`Self::set_dialer_link_state`].
    pub fn set_dialer_link_progress(
        &self,
        id: i64,
        token: &str,
        after: i64,
        pending_rejects: Option<&str>,
        now: i64,
    ) -> Result<bool, IpcError> {
        let n = self.conn.execute(
            "UPDATE peer_links SET after = ?1, pending_rejects = ?2, state = 'connected', \
                                   last_error = NULL, last_exchange_at = ?3 \
             WHERE id = ?4 AND token = ?5 AND revoked_at IS NULL",
            rusqlite::params![after, pending_rejects, now, id, token],
        )?;
        Ok(n > 0)
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

    /// Start a new `peer_exchange` generation for link `link_id` and return
    /// it. A parked handler holding an older one is superseded.
    pub fn bump_peer_generation(&self, link_id: i64) -> u64 {
        let mut g = self
            .peer_generations
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let next = g.get(&link_id).copied().unwrap_or(0) + 1;
        g.insert(link_id, next);
        next
    }

    /// The current `peer_exchange` generation of link `link_id`.
    pub fn peer_generation(&self, link_id: i64) -> u64 {
        self.peer_generations
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&link_id)
            .copied()
            .unwrap_or(0)
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
            &format!(
                "INSERT INTO participants (kind, address, peer_link_id, created_at) \
                 VALUES ('{PARTICIPANT_REMOTE}', ?1, ?2, ?3)"
            ),
            rusqlite::params![address, link_id, now_unix()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Insert a `pending` row addressed to a remote participant.
    ///
    /// INVARIANT (G18): once inserted, a row addressed to a remote
    /// participant (`to_participant_id` a `remote`-kind participant) is
    /// NEVER DELETED — only ever moved `pending` → `accepted` /
    /// `undeliverable` in place. Two things depend on that id staying put
    /// forever: the LISTENER side's `after` watermark on this row's peer
    /// link (a dialer hands over "everything through id N"; a deleted row
    /// under that id would make a later `handover_upto` silently skip
    /// nothing being wrong, or — worse — a REUSED id under N mean something
    /// never actually delivered), and the peer's own dedup key
    /// (`remote_message_id`, which the far end derives from this row's id —
    /// see `insert_inbound_remote`'s `(remote_fleet_id, remote_message_id)`
    /// uniqueness). Neither `sweep_retired_participants` nor
    /// `sweep_peer_outbox` (nor anything else in this module) may delete an
    /// outbound-remote `session_messages` row; both are pinned by
    /// `sweep_never_deletes_an_outbound_remote_row` below.
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

    /// `pending` → `accepted` for those of `ids` that are on link
    /// `link_id` — a peer's results settle its own link's rows only.
    pub fn mark_peer_accepted(&self, link_id: i64, ids: &[i64]) -> Result<usize, IpcError> {
        if ids.is_empty() {
            return Ok(0);
        }
        let sql = format!(
            // `?1` comes first in the text: each bare `?` of the IN list then
            // numbers on from it (2, 3, …), matching `params_then`'s order.
            "UPDATE session_messages SET peer_state = 'accepted' \
             WHERE peer_state = 'pending' \
               AND to_participant_id IN (SELECT id FROM participants WHERE peer_link_id = ?1) \
               AND id IN ({phs})",
            phs = super::in_clause(ids.len())
        );
        let params = super::params_then(&[&link_id], ids);
        Ok(self.conn.execute(&sql, params.as_slice())?)
    }

    /// The shared body behind `mark_peer_undeliverable` and `revoke_peer_link`
    /// — the latter must not re-enter `atomically`, so it calls this directly
    /// from inside its own closure.
    ///
    /// G16: the sender is told at its participant's CURRENT `session_id`, not
    /// the raw `from_session_id` this row was written with — the sender may
    /// have moved since (a new `sessions` row, the participant re-pointed at
    /// it) or that id reused by an unrelated session. `from_participant_id`
    /// is the stable identity; a NULL or retired participant has nowhere
    /// live to deliver the notice, so it is skipped rather than posted to a
    /// stale or reused id.
    fn fail_pending_locked(&self, id: i64, reason: &str) -> Result<bool, IpcError> {
        let n = self.conn.execute(
            "UPDATE session_messages SET peer_state = 'undeliverable' \
             WHERE id = ?1 AND peer_state = 'pending'",
            [id],
        )?;
        if n == 0 {
            return Ok(false);
        }
        let from_participant_id: Option<i64> = self.conn.query_row(
            "SELECT from_participant_id FROM session_messages WHERE id = ?1",
            [id],
            |r| r.get(0),
        )?;
        if let Some(pid) = from_participant_id {
            let current_session: Option<i64> = self
                .conn
                .query_row(
                    "SELECT session_id FROM participants WHERE id = ?1 AND retired_at IS NULL",
                    [pid],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            if let Some(sid) = current_session {
                self.insert_session_event(
                    sid,
                    "message_undeliverable",
                    Some(&format!("message {id} could not be delivered: {reason}")),
                )?;
            }
        }
        Ok(true)
    }

    /// Fail pending row `id` — only when it is on link `link_id`, so a
    /// peer's rejection cannot reach another link's rows. The sweep and
    /// `revoke_peer_link` fail rows by their own selection and call
    /// `fail_pending_locked` directly.
    pub fn mark_peer_undeliverable(
        &self,
        link_id: i64,
        id: i64,
        reason: &str,
    ) -> Result<bool, IpcError> {
        self.atomically(|s| {
            let on_link: bool = s.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_messages m \
                                 JOIN participants p ON p.id = m.to_participant_id \
                                WHERE m.id = ?1 AND p.peer_link_id = ?2)",
                rusqlite::params![id, link_id],
                |r| r.get(0),
            )?;
            if !on_link {
                return Ok(false);
            }
            s.fail_pending_locked(id, reason)
        })
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

    /// Whether message `id` went OUTBOUND on link `link_id`: it carries a
    /// `peer_state` (only ever set by `insert_outbound_remote`, never on a
    /// local or inbound row) and its recipient participant belongs to that
    /// link. For G12: `apply.rs`'s `map_reply_to` uses this to restrict its
    /// own-fleet `{own_fleet, id}` branch to rows THIS link actually sent,
    /// so a peer cannot thread a reply onto a local-only message or another
    /// fleet's exchange just by naming a local id.
    pub fn is_outbound_on_link(&self, message_id: i64, link_id: i64) -> Result<bool, IpcError> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_messages m \
                             JOIN participants p ON p.id = m.to_participant_id \
                            WHERE m.id = ?1 AND m.peer_state IS NOT NULL \
                              AND p.peer_link_id = ?2)",
            rusqlite::params![message_id, link_id],
            |r| r.get(0),
        )?)
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
            let reason = stale_outbox_reason(older_than_secs);
            for id in stale {
                if s.fail_pending_locked(id, &reason)? {
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
                &format!(
                    "DELETE FROM participants WHERE kind = '{PARTICIPANT_REMOTE}' AND id NOT IN ( \
                        SELECT from_participant_id FROM session_messages \
                         WHERE from_participant_id IS NOT NULL \
                        UNION \
                        SELECT to_participant_id FROM session_messages \
                         WHERE to_participant_id IS NOT NULL)"
                ),
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

    /// I1: a dialer loop's writes are fenced on the token it started with.
    /// After a re-pair moves a new token onto the row, the old loop's state
    /// and progress writes change nothing; the new token's do.
    #[test]
    fn a_dialer_write_applies_only_under_the_token_it_was_made_with() {
        let s = Store::open_in_memory().unwrap();
        let link = s.insert_dialer_link("https://b.example", "old").unwrap();
        s.adopt_dialer_fleet(link, B).unwrap();
        // A re-pair merges only into a stopped row (C1).
        s.set_peer_link_state(link, LINK_REFUSED, Some("E_UNAUTHORIZED: gone"), 1)
            .unwrap();
        let tmp = s.insert_dialer_link("https://b2.example", "new").unwrap();
        assert_eq!(s.adopt_dialer_fleet(tmp, B).unwrap(), Adopted::Link(link));
        assert!(!s
            .set_dialer_link_state(link, "old", LINK_REFUSED, Some("E_UNAUTHORIZED: x"), 5)
            .unwrap());
        assert!(!s
            .set_dialer_link_progress(link, "old", 9, Some("[]"), 5)
            .unwrap());
        let row = s.peer_link(link).unwrap().unwrap();
        assert_eq!((row.state.as_str(), row.after), (LINK_RETRYING, 0));
        assert!(row.last_error.is_none() && row.pending_rejects.is_none());
        assert!(s.set_dialer_link_progress(link, "new", 9, None, 5).unwrap());
        assert!(s
            .set_dialer_link_state(link, "new", LINK_RETRYING, Some("HTTP 502"), 6)
            .unwrap());
        let row = s.peer_link(link).unwrap().unwrap();
        assert_eq!(row.after, 9);
        assert_eq!(row.last_error.as_deref(), Some("HTTP 502"));
        s.revoke_peer_link(link, 7).unwrap();
        assert!(!s
            .set_dialer_link_state(link, "new", LINK_CONNECTED, None, 8)
            .unwrap());
    }

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

    /// C1: a second live peer token claiming a fleet already linked to
    /// another live token is refused, and the link keeps its token — else
    /// any paired hub could take over another fleet's link (its outbox, its
    /// sender addresses, its watermark).
    #[test]
    fn a_live_peer_token_cannot_take_over_another_tokens_fleet() {
        let s = Store::open_in_memory().unwrap();
        let c1 = client(&s, "hub-a");
        let link = s.ensure_listener_link(c1, "fleet-a").unwrap();
        let c2 = client(&s, "hub-intruder");
        let e = s.ensure_listener_link(c2, "fleet-a").unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_FORBIDDEN);
        assert!(
            e.message.contains("revoke that client first"),
            "{}",
            e.message
        );
        assert_eq!(s.peer_link(link.id).unwrap().unwrap().client_id, Some(c1));
    }

    /// C1, the legitimate re-pair: once the old token is revoked, a new
    /// token for the same fleet rebinds the live row, so the pending outbox
    /// rows stay attached to it.
    #[test]
    fn after_the_old_token_is_revoked_a_new_token_rebinds_and_keeps_pending_rows() {
        let s = Store::open_in_memory().unwrap();
        let a1 = seed(&s, "a1");
        let c1 = client(&s, "hub-a");
        let link = s.ensure_listener_link(c1, "fleet-a").unwrap();
        let to = s
            .ensure_remote_participant(link.id, "fleet-a/session/h/x")
            .unwrap();
        s.insert_outbound_remote(
            a1,
            "fleet-b/session/local/a1",
            to,
            "x",
            "message",
            None,
            false,
        )
        .unwrap();
        s.revoke_client_token("hub-a").unwrap();
        let c2 = client(&s, "hub-a-again");
        let again = s.ensure_listener_link(c2, "fleet-a").unwrap();
        assert_eq!(again.id, link.id);
        assert_eq!(again.client_id, Some(c2));
        assert_eq!(s.pending_outbox(link.id, 0, 50).unwrap().len(), 1);
    }

    /// I1, store half: settling is scoped to the link — the dialer's
    /// `apply_results(link, …)` rests on this for `accepted` entries.
    #[test]
    fn settling_a_row_on_another_link_changes_nothing() {
        let s = Store::open_in_memory().unwrap();
        let a1 = seed(&s, "a1");
        let l1 = s.insert_dialer_link("https://b.example", "t1").unwrap();
        let l2 = s.insert_dialer_link("https://c.example", "t2").unwrap();
        let to = s
            .ensure_remote_participant(l2, "fleet-c/session/h/c1")
            .unwrap();
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
        assert_eq!(s.mark_peer_accepted(l1, &[m]).unwrap(), 0);
        assert!(!s.mark_peer_undeliverable(l1, m, "E_X: no").unwrap());
        assert_eq!(s.pending_outbox(l2, 0, 50).unwrap().len(), 1);
        assert_eq!(s.mark_peer_accepted(l2, &[m]).unwrap(), 1);
    }

    /// M3: a state write never brings a revoked link back — a listener
    /// handler parked across a revoke writes `connected` when it returns.
    #[test]
    fn a_revoked_link_is_never_resurrected_by_a_state_write() {
        let s = Store::open_in_memory().unwrap();
        let link = s.insert_dialer_link("https://b.example", "t").unwrap();
        s.set_peer_link_state(link, LINK_RETRYING, Some("boom"), 1)
            .unwrap();
        s.revoke_peer_link(link, 2).unwrap();
        s.set_peer_link_state(link, LINK_CONNECTED, None, 9)
            .unwrap();
        assert!(!s.set_dialer_link_progress(link, "t", 5, None, 9).unwrap());
        let row = s.peer_link(link).unwrap().unwrap();
        assert_eq!(row.state, LINK_RETRYING);
        assert_eq!(row.last_exchange_at, Some(1));
        assert_eq!(row.after, 0);
    }

    /// M6: the row carries the plaintext token; `{:?}` must not.
    #[test]
    fn debug_never_prints_the_token() {
        let s = Store::open_in_memory().unwrap();
        let id = s
            .insert_dialer_link("https://b.example", "secret-token-value")
            .unwrap();
        let row = s.peer_link(id).unwrap().unwrap();
        let dbg = format!("{row:?}");
        assert!(!dbg.contains("secret-token-value"), "{dbg}");
        assert!(dbg.contains("<redacted>"), "{dbg}");
    }

    #[test]
    fn a_repaired_fleet_moves_onto_its_old_dialer_row() {
        let s = Store::open_in_memory().unwrap();
        let old = s.insert_dialer_link("https://b.example", "t1").unwrap();
        assert_eq!(s.adopt_dialer_fleet(old, B).unwrap(), Adopted::Link(old));
        s.set_peer_link_state(old, LINK_REFUSED, Some("401"), 1)
            .unwrap();
        let new = s.insert_dialer_link("https://b2.example", "t2").unwrap();
        let kept = s.adopt_dialer_fleet(new, B).unwrap();
        assert_eq!(
            kept,
            Adopted::Link(old),
            "pending rows stay on the old link"
        );
        let row = s.peer_link(old).unwrap().unwrap();
        assert_eq!(row.url.as_deref(), Some("https://b2.example"));
        assert_eq!(row.token.as_deref(), Some("t2"));
        assert_eq!(row.state, LINK_RETRYING);
        assert!(s.peer_link(new).unwrap().is_none());
    }

    /// A re-pair merge keeps the old row's outbox but sheds its watermark:
    /// `after` is the peer's word, and a hostile peer that once answered
    /// `i64::MAX` would otherwise have every row the honest peer queues
    /// after the re-pair marked accepted without delivery.
    #[test]
    fn a_re_pair_merge_sheds_the_old_watermark() {
        let s = Store::open_in_memory().unwrap();
        let old = s.insert_dialer_link("https://b.example", "t1").unwrap();
        assert_eq!(s.adopt_dialer_fleet(old, B).unwrap(), Adopted::Link(old));
        assert!(s
            .set_dialer_link_progress(old, "t1", i64::MAX, None, 1)
            .unwrap());
        assert_eq!(s.peer_link(old).unwrap().unwrap().after, i64::MAX);
        s.set_peer_link_state(old, LINK_REFUSED, Some("401"), 2)
            .unwrap();
        let new = s.insert_dialer_link("https://b2.example", "t2").unwrap();
        assert_eq!(s.adopt_dialer_fleet(new, B).unwrap(), Adopted::Link(old));
        let row = s.peer_link(old).unwrap().unwrap();
        assert_eq!(
            row.after, 0,
            "the planted watermark does not survive a re-pair"
        );
        assert_eq!(row.token.as_deref(), Some("t2"));
    }

    /// C1: a newly paired hub whose handshake claims a fleet that already has
    /// a working (or merely retrying) dialer link cannot take that link over:
    /// the claim waits on the live link, which keeps its url, token and
    /// state, and the new row is left as it was. Only a `refused` or
    /// `incompatible` row takes new credentials (a re-pair).
    #[test]
    fn a_new_row_cannot_take_over_a_live_link_for_its_fleet() {
        let s = Store::open_in_memory().unwrap();
        let live = s.insert_dialer_link("https://b.example", "t-b").unwrap();
        assert_eq!(s.adopt_dialer_fleet(live, B).unwrap(), Adopted::Link(live));
        assert!(s.set_dialer_link_progress(live, "t-b", 3, None, 1).unwrap());
        for state in [LINK_CONNECTED, LINK_RETRYING] {
            s.set_peer_link_state(live, state, None, 2).unwrap();
            let claimant = s.insert_dialer_link("https://evil.example", "t-c").unwrap();
            assert_eq!(
                s.adopt_dialer_fleet(claimant, B).unwrap(),
                Adopted::Waiting(live),
                "{state}"
            );
            let row = s.peer_link(live).unwrap().unwrap();
            assert_eq!(row.url.as_deref(), Some("https://b.example"));
            assert_eq!(row.token.as_deref(), Some("t-b"));
            assert_eq!((row.state.as_str(), row.after), (state, 3));
            let c = s
                .peer_link(claimant)
                .unwrap()
                .expect("the claimant row stays");
            assert!(c.fleet_id.is_none());
        }
        // A refused or incompatible row is the re-pair case: it merges.
        for state in [LINK_REFUSED, LINK_INCOMPATIBLE] {
            s.set_peer_link_state(live, state, Some("E_UNAUTHORIZED: x"), 3)
                .unwrap();
            let again = s
                .insert_dialer_link("https://b2.example", &format!("t-{state}"))
                .unwrap();
            assert_eq!(
                s.adopt_dialer_fleet(again, B).unwrap(),
                Adopted::Link(live),
                "{state}"
            );
            let row = s.peer_link(live).unwrap().unwrap();
            assert_eq!(row.token.as_deref(), Some(format!("t-{state}").as_str()));
            assert_eq!(row.state, LINK_RETRYING);
        }
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
        assert_eq!(
            s.adopt_dialer_fleet(dialer, B).unwrap(),
            Adopted::Link(dialer)
        );

        let c = client(&s, "hub-b");
        let err = s.ensure_listener_link(c, B).unwrap_err();
        // G10: terminal for the dialer on the other end (E_FORBIDDEN), not
        // E_EXISTS — a listener-side "already linked the other way" never
        // resolves itself by retrying, unlike a dialer's own handshake
        // conflict (`adopt_dialer_fleet_refuses_a_fleet_already_linked_the_other_way`
        // below keeps E_EXISTS for that path).
        assert_eq!(err.code, crate::ipc_error::codes::E_FORBIDDEN);
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
            .mark_peer_undeliverable(link, m, "E_PARTICIPANT_UNKNOWN: no session b1 on h")
            .unwrap());
        assert!(
            !s.mark_peer_undeliverable(link, m, "again").unwrap(),
            "only once"
        );
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

    /// G14a: the dialer loop calls `set_dialer_link_state` only on a failed
    /// or refused exchange (`Fence::state` in `dial.rs`), so it must not
    /// stamp `last_exchange_at` — that would make a link that has not
    /// talked to its peer in a while look freshly exchanged the moment it
    /// started failing. A SUCCESSFUL exchange (`set_dialer_link_progress`)
    /// still stamps it.
    #[test]
    fn set_dialer_link_state_does_not_stamp_last_exchange_at_but_progress_does() {
        let s = Store::open_in_memory().unwrap();
        let link = s.insert_dialer_link("https://b.example", "t").unwrap();
        assert!(s
            .set_dialer_link_state(link, "t", LINK_RETRYING, Some("HTTP 502"), 5)
            .unwrap());
        let row = s.peer_link(link).unwrap().unwrap();
        assert_eq!(row.state, LINK_RETRYING);
        assert_eq!(row.last_error.as_deref(), Some("HTTP 502"));
        assert_eq!(
            row.last_exchange_at, None,
            "a failed exchange must not look like a recent one"
        );

        assert!(s.set_dialer_link_progress(link, "t", 9, None, 7).unwrap());
        let row = s.peer_link(link).unwrap().unwrap();
        assert_eq!(
            row.last_exchange_at,
            Some(7),
            "a SUCCESSFUL exchange stamps it"
        );
    }

    /// G14c: a live LISTENER link has no `state` of its own that can go
    /// stale (`ensure_listener_link` only ever writes `connected`), so
    /// `peer_links_down` watches its client token and its last served
    /// exchange instead.
    #[test]
    fn peer_links_down_counts_a_stale_or_revoked_listener() {
        let s = Store::open_in_memory().unwrap();
        let c = client(&s, "hub-a");
        let link = s.ensure_listener_link(c, "fleet-a").unwrap();
        // Never served an exchange yet: down.
        assert_eq!(s.peer_links_down(1_000).unwrap(), 1);

        // A served exchange (as `listen.rs` stamps on success) — fresh, not
        // down.
        s.set_peer_link_state(link.id, LINK_CONNECTED, None, 1_000)
            .unwrap();
        assert_eq!(
            s.peer_links_down(1_000 + LISTENER_STALE_SECS - 1).unwrap(),
            0
        );
        // Past the staleness window: down again.
        assert_eq!(
            s.peer_links_down(1_000 + LISTENER_STALE_SECS + 1).unwrap(),
            1
        );

        // Fresh again, but its client token was revoked: still down —
        // revoking the token does not revoke the link itself (G8 Ruling 6).
        let now = 1_000 + LISTENER_STALE_SECS + 1;
        s.set_peer_link_state(link.id, LINK_CONNECTED, None, now)
            .unwrap();
        s.revoke_client_token("hub-a").unwrap();
        assert_eq!(s.peer_links_down(now).unwrap(), 1);
        assert!(
            s.peer_link(link.id).unwrap().unwrap().revoked_at.is_none(),
            "the link itself is not revoked, only its token"
        );
    }

    /// G16: `fail_pending_locked` must tell the sender wherever it CURRENTLY
    /// lives, not the raw `from_session_id` baked into the row at send
    /// time — the sender may have moved since (a fresh `sessions` row, the
    /// participant re-pointed at it, as a session move does).
    #[test]
    fn fail_pending_locked_notifies_the_senders_current_session_after_a_move() {
        let s = Store::open_in_memory().unwrap();
        let old = seed(&s, "old");
        let moved_to = seed(&s, "moved-to");
        let link = s.insert_dialer_link("https://b.example", "t").unwrap();
        let to = s.ensure_remote_participant(link, ADDR).unwrap();
        let m = s
            .insert_outbound_remote(
                old,
                "fleet-a/session/local/old",
                to,
                "x",
                "message",
                None,
                false,
            )
            .unwrap();
        let sender_p = s.participant_for_session(old).unwrap().unwrap().id;
        s.repoint_participant(sender_p, moved_to).unwrap();

        assert!(s.mark_peer_undeliverable(link, m, "gone").unwrap());

        let stale = s.list_session_events(old, 10).unwrap();
        assert!(
            !stale.iter().any(|e| e.kind == "message_undeliverable"),
            "nothing lands on the id the sender left behind: {stale:?}"
        );
        let moved = s.list_session_events(moved_to, 10).unwrap();
        assert!(
            moved.iter().any(|e| e.kind == "message_undeliverable"),
            "the notice follows the sender to where it lives now: {moved:?}"
        );
    }

    /// G18: an outbound-remote `session_messages` row's id must never
    /// disappear — the listener's `after` watermark and the peer's own
    /// dedup key both depend on it. Neither sweep may delete it, however far
    /// past its own window it is. See the invariant doc on
    /// `insert_outbound_remote`.
    #[test]
    fn sweep_never_deletes_an_outbound_remote_row() {
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
        // Well past both sweeps' windows, the way the existing "week-old
        // pending row" test ages a row: force `sent_at` back rather than
        // trying to outrun the real wall clock with `now`.
        s.conn_ref()
            .execute("UPDATE session_messages SET sent_at = 0 WHERE id = ?1", [m])
            .unwrap();
        let far_future = PEER_PENDING_MAX_SECS + 1;
        s.sweep_peer_outbox(far_future, PEER_PENDING_MAX_SECS)
            .unwrap();
        s.sweep_retired_participants(far_future, crate::store::RETIRED_RETENTION_SECS)
            .unwrap();

        let still_there: i64 = s
            .conn_ref()
            .query_row(
                "SELECT COUNT(*) FROM session_messages WHERE id = ?1",
                [m],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(still_there, 1, "an outbound-remote row is never deleted");
        let state: String = s
            .conn_ref()
            .query_row(
                "SELECT peer_state FROM session_messages WHERE id = ?1",
                [m],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            state, "undeliverable",
            "it was failed in place, not dropped"
        );
    }

    /// G24: the reason text is derived from `older_than_secs`, not a
    /// hard-coded "7 days" — the coarsest unit (days, then hours, then
    /// plain seconds) that describes the window exactly.
    #[test]
    fn stale_outbox_reason_reads_the_coarsest_exact_unit() {
        assert_eq!(
            stale_outbox_reason(7 * 24 * 60 * 60),
            "no answer from the peer hub within 7 days"
        );
        assert_eq!(
            stale_outbox_reason(24 * 60 * 60),
            "no answer from the peer hub within 1 day"
        );
        assert_eq!(
            stale_outbox_reason(2 * 60 * 60),
            "no answer from the peer hub within 2 hours"
        );
        assert_eq!(
            stale_outbox_reason(90),
            "no answer from the peer hub within 90 seconds"
        );
        assert_eq!(
            stale_outbox_reason(1),
            "no answer from the peer hub within 1 second"
        );
    }

    /// G24: the sweep's own event text reflects whatever `older_than_secs`
    /// it was actually called with, not the module's 7-day default.
    #[test]
    fn the_sweep_writes_a_reason_that_matches_its_own_older_than_secs() {
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
        s.sweep_peer_outbox(3600, 3600).unwrap();
        let ev = s.list_session_events(a1, 10).unwrap();
        let hit = ev
            .iter()
            .find(|e| e.kind == "message_undeliverable")
            .expect("event");
        assert!(
            hit.detail
                .as_deref()
                .unwrap_or("")
                .contains("within 1 hour"),
            "{:?}",
            hit.detail
        );
    }

    /// The store half of G12: `apply.rs`'s `map_reply_to` (L3) restricts its
    /// own-fleet branch to rows this link actually sent, so a peer cannot
    /// thread onto a local-only message or another link's exchange.
    #[test]
    fn is_outbound_on_link_is_true_only_for_that_links_own_outbound_row() {
        let s = Store::open_in_memory().unwrap();
        let a1 = seed(&s, "a1");
        let b1 = seed(&s, "b1");
        let l1 = s.insert_dialer_link("https://b.example", "t1").unwrap();
        let l2 = s.insert_dialer_link("https://c.example", "t2").unwrap();
        let to1 = s.ensure_remote_participant(l1, ADDR).unwrap();
        let out = s
            .insert_outbound_remote(
                a1,
                "fleet-a/session/local/a1",
                to1,
                "x",
                "message",
                None,
                false,
            )
            .unwrap();
        assert!(s.is_outbound_on_link(out, l1).unwrap());
        assert!(
            !s.is_outbound_on_link(out, l2).unwrap(),
            "another link's own-fleet id must not match"
        );

        // A purely local message is not outbound on any link.
        let local = s.insert_message(a1, b1, "hi", "message", None).unwrap();
        assert!(!s.is_outbound_on_link(local, l1).unwrap());

        // An inbound (peer-originated) row is not "outbound" either.
        let from = s
            .ensure_remote_participant(l1, "fleet-a/session/h/a2")
            .unwrap();
        let inbound = match s
            .insert_inbound_remote("fleet-a", 1, from, b1, "hi", "message", None)
            .unwrap()
        {
            Inbound::Inserted(id) => id,
            other => panic!("{other:?}"),
        };
        assert!(!s.is_outbound_on_link(inbound, l1).unwrap());
    }
}
