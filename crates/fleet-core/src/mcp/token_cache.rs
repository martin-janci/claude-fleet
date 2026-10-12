//! The hub's in-memory copy of the bearer-token tables, for [`super::authorize`].
//!
//! Without it `authorize` locked the writer's `Mutex<Store>` on every request
//! — `/hook` included — to read `host_tokens` and `client_tokens`, so every
//! request queued behind whatever reconcile pass or hook transaction held it.
//!
//! # Why a revoked token is refused on the very next request
//!
//! The cache is keyed on the store's auth epoch (migration 060): triggers in
//! the database bump it inside the transaction of every write that can change
//! what a token resolves to — mint, rotate, mode change, host removal, pair,
//! trust, untrust, revoke, peer-link removal — whichever process wrote it.
//! [`TokenCache::tokens`] reads the epoch on a read-only pooled connection on
//! EVERY request (one single-row read, never the writer's lock) and rebuilds
//! when it differs from the snapshot's. A request that starts after a
//! revoking commit therefore reads the new epoch and never sees the revoked
//! row. There is no time-based staleness and no in-process invalidate call
//! to forget: a `fleet-hub peer remove` in another process is covered by the
//! same trigger as the `revoke_client` tool.
//!
//! What is NOT cached: the master token. It was, and stays, the value the
//! server was started with (`start_with_listener`'s `token`); `fleet-hub token
//! regenerate` takes effect on the next start, as before.
//!
//! # The hub's personal owner rides along
//!
//! Multi-user M1 needs one more fact per request — `Store::personal_owner_id()`,
//! which decides [`crate::mcp::Caller::is_personal_owner`] — and `authorize`
//! must not take the writer's lock to answer it. It is read here, on the same
//! read-only connection and the same pass as the token rows, and then
//! remembered: the flagged `people` row is written exactly twice in the life
//! of a database (by migration 098 and by `ensure_personal_owner`), so it is
//! not something the auth epoch has to track.
//!
//! A `None` is **never** remembered. A hub that cannot say whose it is is
//! mis-provisioned; it must keep asking, so that the next `ensure_personal_owner`
//! — or an operator's repair — takes effect without a restart, and it fails
//! closed meanwhile (T1).

use crate::ipc_error::{codes, lock, IpcError};
use crate::store::{ClientTokenRow, ControlTokenRow, HostTokenRow, ReadPool};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The token rows at one auth epoch.
struct Snapshot {
    epoch: i64,
    hosts: Arc<Vec<HostTokenRow>>,
    clients: Arc<Vec<ClientTokenRow>>,
    controls: Arc<Vec<ControlTokenRow>>,
}

/// Everything `authorize` matches a bearer token against, as of one auth
/// epoch, and the hub's personal owner.
pub struct AuthRows {
    pub hosts: Arc<Vec<HostTokenRow>>,
    pub clients: Arc<Vec<ClientTokenRow>>,
    /// Named Control API tokens (migration 157), live ones; expired ones are
    /// refused by time in `auth::resolve_token_at`.
    pub controls: Arc<Vec<ControlTokenRow>>,
    pub personal_owner: Option<i64>,
}

/// How often, at most, `authorize` stamps one client's `last_seen_at` — the
/// same minute the store's own guard in `touch_client_token` enforces, kept
/// here too so a request that is not due never touches the writer's lock.
const TOUCH_INTERVAL_SECS: i64 = 60;

/// Token rows kept in memory, validated against the auth epoch per request.
pub struct TokenCache {
    pool: Arc<ReadPool>,
    snapshot: Mutex<Snapshot>,
    /// Per client id, when `authorize` last stamped its `last_seen_at`.
    touched: Mutex<HashMap<i64, i64>>,
    /// Per host alias, when `authorize` last stamped its token's
    /// `last_used_at` (Orbit Fleet 11.4).
    touched_hosts: Mutex<HashMap<String, i64>>,
    /// Per named token id, when `authorize` last stamped its `last_used_at`.
    touched_controls: Mutex<HashMap<i64, i64>>,
    /// The hub's personal owner (multi-user M1), once it has been read.
    /// `None` means "not known yet, ask again" — never "this hub has none",
    /// which is why the module docs above forbid caching a `None`.
    owner: Mutex<Option<i64>>,
}

impl TokenCache {
    /// Build the cache from the store now (the hub's start), reading through
    /// `pool`.
    pub fn new(pool: Arc<ReadPool>) -> Result<Self, IpcError> {
        let snapshot = Self::load(&pool)?;
        Ok(Self {
            pool,
            snapshot: Mutex::new(snapshot),
            touched: Mutex::new(HashMap::new()),
            touched_hosts: Mutex::new(HashMap::new()),
            touched_controls: Mutex::new(HashMap::new()),
            owner: Mutex::new(None),
        })
    }

    fn load(pool: &ReadPool) -> Result<Snapshot, IpcError> {
        let conn = pool.get().ok_or_else(no_connection)?;
        let (epoch, hosts, clients, controls) = lock(conn)?.auth_snapshot()?;
        Ok(Snapshot {
            epoch,
            hosts: Arc::new(hosts),
            clients: Arc::new(clients),
            controls: Arc::new(controls),
        })
    }

    /// The per-host and live client token rows as of now, and the hub's
    /// personal owner — everything `auth::check_request` needs, off one
    /// pooled connection.
    ///
    /// `Err` only when the read-only connection cannot answer (every pooled
    /// connection poisoned, a SQLite error): the caller then reads the
    /// tables through the writer, as `authorize` did before the cache —
    /// failing over to the authoritative path, never to a stale snapshot.
    pub fn tokens(&self) -> Result<AuthRows, IpcError> {
        let conn = self.pool.get().ok_or_else(no_connection)?;
        let current = lock(conn)?.auth_epoch()?;
        let owner = self.personal_owner(conn)?;
        let mut snap = self.snapshot.lock().unwrap_or_else(|p| p.into_inner());
        if snap.epoch != current {
            // Epoch and rows in one read transaction: the rows are the ones
            // at the epoch recorded with them (possibly newer than
            // `current`, never older).
            let (epoch, hosts, clients, controls) = lock(conn)?.auth_snapshot()?;
            *snap = Snapshot {
                epoch,
                hosts: Arc::new(hosts),
                clients: Arc::new(clients),
                controls: Arc::new(controls),
            };
        }
        Ok(AuthRows {
            hosts: Arc::clone(&snap.hosts),
            clients: Arc::clone(&snap.clients),
            controls: Arc::clone(&snap.controls),
            personal_owner: owner,
        })
    }

    /// The hub's personal owner, read through `conn` the first time and
    /// remembered after.
    ///
    /// Not keyed on the auth epoch, and deliberately so: nothing in the
    /// `people` table is a bearer token, and the flagged row is written by
    /// migration 098 and by `ensure_personal_owner` only — an id that has
    /// been answered once does not change under a running hub. A `None` is
    /// not remembered, so a hub that is missing the row asks again on every
    /// request and picks the id up the moment one exists.
    fn personal_owner(&self, conn: &Mutex<crate::store::Store>) -> Result<Option<i64>, IpcError> {
        let known = *self.owner.lock().unwrap_or_else(|p| p.into_inner());
        if known.is_some() {
            return Ok(known);
        }
        let read = lock(conn)?.personal_owner_id()?;
        if read.is_some() {
            *self.owner.lock().unwrap_or_else(|p| p.into_inner()) = read;
        }
        Ok(read)
    }

    /// Whether client `id` is due a `last_seen_at` stamp at `now`. Marks it
    /// stamped when it is, so concurrent requests from one phone do not all
    /// go for the writer; [`Self::untouch`] takes that back when the stamp
    /// could not be written.
    pub fn touch_due(&self, id: i64, now: i64) -> bool {
        let mut t = self.touched.lock().unwrap_or_else(|p| p.into_inner());
        match t.get(&id) {
            Some(&at) if now - at < TOUCH_INTERVAL_SECS => false,
            _ => {
                t.insert(id, now);
                true
            }
        }
    }

    /// Forget a stamp that was not written (the writer was busy), so the
    /// next request tries again.
    pub fn untouch(&self, id: i64) {
        self.touched
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&id);
    }

    /// [`Self::touch_due`] for a host token, by alias.
    pub fn host_touch_due(&self, alias: &str, now: i64) -> bool {
        let mut t = self.touched_hosts.lock().unwrap_or_else(|p| p.into_inner());
        match t.get(alias) {
            Some(&at) if now - at < TOUCH_INTERVAL_SECS => false,
            _ => {
                t.insert(alias.to_string(), now);
                true
            }
        }
    }

    /// [`Self::touch_due`] for a named token, by id.
    pub fn control_touch_due(&self, id: i64, now: i64) -> bool {
        let mut t = self
            .touched_controls
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        match t.get(&id) {
            Some(&at) if now - at < TOUCH_INTERVAL_SECS => false,
            _ => {
                t.insert(id, now);
                true
            }
        }
    }

    /// [`Self::untouch`] for a named token.
    pub fn control_untouch(&self, id: i64) {
        self.touched_controls
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&id);
    }

    /// [`Self::untouch`] for a host token, by alias.
    pub fn host_untouch(&self, alias: &str) {
        self.touched_hosts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(alias);
    }
}

fn no_connection() -> IpcError {
    IpcError::new(
        codes::E_INTERNAL,
        "every read-only store connection is poisoned",
    )
}
