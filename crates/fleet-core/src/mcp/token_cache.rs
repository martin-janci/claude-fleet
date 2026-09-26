//! The hub's in-memory copy of the bearer-token tables, for [`super::authorize`].
//!
//! Without it `authorize` locked the writer's `Mutex<Store>` on every request
//! — `/hook` included — to read `host_tokens` and `client_tokens`, so every
//! request queued behind whatever reconcile pass or hook transaction held it.
//!
//! # Why a revoked token is refused on the very next request
//!
//! The cache is keyed on the store's auth epoch (migration 057): triggers in
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

use crate::ipc_error::{codes, lock, IpcError};
use crate::store::{ClientTokenRow, HostTokenRow, ReadPool};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The token rows at one auth epoch.
struct Snapshot {
    epoch: i64,
    hosts: Arc<Vec<HostTokenRow>>,
    clients: Arc<Vec<ClientTokenRow>>,
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
        })
    }

    fn load(pool: &ReadPool) -> Result<Snapshot, IpcError> {
        let conn = pool.get().ok_or_else(no_connection)?;
        let (epoch, hosts, clients) = lock(conn)?.auth_snapshot()?;
        Ok(Snapshot {
            epoch,
            hosts: Arc::new(hosts),
            clients: Arc::new(clients),
        })
    }

    /// The per-host and live client token rows as of now.
    ///
    /// `Err` only when the read-only connection cannot answer (every pooled
    /// connection poisoned, a SQLite error): the caller then reads the
    /// tables through the writer, as `authorize` did before the cache —
    /// failing over to the authoritative path, never to a stale snapshot.
    #[allow(clippy::type_complexity)]
    pub fn tokens(&self) -> Result<(Arc<Vec<HostTokenRow>>, Arc<Vec<ClientTokenRow>>), IpcError> {
        let conn = self.pool.get().ok_or_else(no_connection)?;
        let current = lock(conn)?.auth_epoch()?;
        let mut snap = self.snapshot.lock().unwrap_or_else(|p| p.into_inner());
        if snap.epoch != current {
            // Epoch and rows in one read transaction: the rows are the ones
            // at the epoch recorded with them (possibly newer than
            // `current`, never older).
            let (epoch, hosts, clients) = lock(conn)?.auth_snapshot()?;
            *snap = Snapshot {
                epoch,
                hosts: Arc::new(hosts),
                clients: Arc::new(clients),
            };
        }
        Ok((Arc::clone(&snap.hosts), Arc::clone(&snap.clients)))
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
}

fn no_connection() -> IpcError {
    IpcError::new(
        codes::E_INTERNAL,
        "every read-only store connection is poisoned",
    )
}
