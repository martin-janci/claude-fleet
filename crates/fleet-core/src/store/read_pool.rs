//! Readers off the writer: a small pool of read-only connections on the
//! hub's `state.db`, and the auth epoch the token cache keys on.
//!
//! Every tool used to share the ONE writer connection's `Mutex<Store>`, so a
//! `list_hosts` waited behind a reconcile pass's per-host transaction, and
//! `authorize` locked it on every request just to read the token tables. In
//! WAL a reader never blocks a writer and is never blocked by one, and a new
//! read transaction sees every transaction committed before it began — on
//! any connection, in any process. A read that has no write of its own to
//! see can therefore run on its own connection.
//!
//! Each pooled connection is a `Mutex<Store>` opened with
//! [`Store::open_read_only`] (read-only, never migrated, the store's busy
//! timeout). `SQLITE_OPEN_NO_MUTEX` is sound there for the same reason the
//! writer's is: the `std::sync::Mutex` around each connection lets exactly
//! one thread use it at a time.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, TryLockError};

/// How many read-only connections the hub opens. Reads are short (no SSH, no
/// `.await` under a guard); three lets a phone's cold start (`list_sessions`,
/// `list_projects`, `list_hosts` in parallel) and the auth check proceed
/// without queueing behind each other.
pub const READ_POOL_SIZE: usize = 3;

/// A fixed set of read-only connections on one WAL database file.
pub struct ReadPool {
    conns: Vec<Mutex<Store>>,
    next: AtomicUsize,
}

impl ReadPool {
    /// Open `size` (at least 1) read-only connections on `path`.
    ///
    /// `Ok(None)` when the file is not in WAL: under a rollback journal a
    /// reader's shared lock blocks the writer's commit, so a pool would add
    /// contention instead of removing it — the caller keeps reading through
    /// the writer, exactly as before. Open the pool only AFTER the writer has
    /// opened (and so migrated and switched to WAL) the same file.
    pub fn open(path: &std::path::Path, size: usize) -> Result<Option<Self>> {
        let first = Store::open_read_only(path)?;
        let mode: String = first
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Ok(None);
        }
        let mut conns = vec![Mutex::new(first)];
        for _ in 1..size.max(1) {
            conns.push(Mutex::new(Store::open_read_only(path)?));
        }
        Ok(Some(Self {
            conns,
            next: AtomicUsize::new(0),
        }))
    }

    /// A connection to read through: the first free one, starting from a
    /// rotating cursor; when all are busy, the cursor's own (a pooled read is
    /// short, so waiting on it is bounded — it never waits on the writer).
    /// `None` only when every connection is poisoned.
    pub fn get(&self) -> Option<&Mutex<Store>> {
        let n = self.conns.len();
        let start = self.next.fetch_add(1, Ordering::Relaxed) % n;
        let mut fallback = None;
        for i in 0..n {
            let m = &self.conns[(start + i) % n];
            match m.try_lock() {
                Ok(_) => return Some(m),
                Err(TryLockError::WouldBlock) => {
                    fallback.get_or_insert(m);
                }
                Err(TryLockError::Poisoned(_)) => {}
            }
        }
        fallback
    }
}

/// Where a read with nothing of its own to see goes: a pooled connection
/// when the hub opened a pool, else — the desktop, tests, a non-WAL file —
/// the writer, exactly as before.
pub fn read_via<'a>(pool: Option<&'a ReadPool>, writer: &'a Mutex<Store>) -> &'a Mutex<Store> {
    pool.and_then(ReadPool::get).unwrap_or(writer)
}

impl Store {
    /// The auth epoch (migration 057): a counter the `auth_epoch_*` triggers
    /// bump in the same transaction as every write that can change what a
    /// bearer token resolves to — any insert, update or delete on
    /// `host_tokens`, and any change to `client_tokens` other than the
    /// `last_seen_at` liveness stamp. Triggers live in the database file, so
    /// a write from ANOTHER process (a `fleet-hub` CLI subcommand) bumps it
    /// too; a token cache that compares it on every request can never serve
    /// a revoked token past the revoking commit.
    pub fn auth_epoch(&self) -> Result<i64> {
        self.conn
            .query_row("SELECT epoch FROM auth_epoch WHERE id = 1", [], |r| {
                r.get(0)
            })
    }

    /// The epoch and the rows it describes, read in ONE read transaction so
    /// the rows are exactly the ones at that epoch. (Read apart, a revoke
    /// committing between the two reads could pair pre-revoke rows with the
    /// post-revoke epoch, and a cache would keep them.)
    pub fn auth_snapshot(
        &self,
    ) -> Result<(i64, Vec<HostTokenRow>, Vec<ClientTokenRow>), crate::ipc_error::IpcError> {
        let tx = self.conn.unchecked_transaction()?;
        let epoch = self.auth_epoch()?;
        let hosts = self.list_host_tokens()?;
        let clients = self.active_client_tokens()?;
        tx.commit()?;
        Ok((epoch, hosts, clients))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::NoopEventBus;

    fn file_store(dir: &tempfile::TempDir) -> (std::path::PathBuf, Store) {
        let path = dir.path().join("state.db");
        let s = Store::open_with_bus(&path, Arc::new(NoopEventBus)).unwrap();
        (path, s)
    }

    /// Every write path that changes what a token resolves to moves the
    /// epoch; the per-request liveness stamp does not (it would rebuild the
    /// cache once a minute per client for nothing).
    #[test]
    fn the_auth_epoch_moves_on_every_token_write_and_not_on_a_touch() {
        let s = Store::open_in_memory().unwrap();
        let last = std::cell::Cell::new(s.auth_epoch().unwrap());
        let moved = |s: &Store, what: &str| {
            let (was, now) = (last.get(), s.auth_epoch().unwrap());
            assert!(
                now > was,
                "{what} did not move the auth epoch ({was} -> {now})"
            );
            last.set(now);
        };
        s.upsert_host("box").unwrap();
        s.upsert_host_token("box", "t1").unwrap();
        moved(&s, "minting a host token");
        s.upsert_host_token("box", "t2").unwrap();
        moved(&s, "rotating a host token");
        s.set_host_token_mode("box", "readonly").unwrap();
        moved(&s, "a host token's mode change");
        s.delete_host("box").unwrap();
        moved(&s, "removing the host (its token row)");
        let row = s
            .insert_client_token("phone", &"a".repeat(64), "full")
            .unwrap();
        moved(&s, "pairing a client");
        s.set_client_trust("phone", true).unwrap();
        moved(&s, "trusting a client");
        s.set_client_trust("phone", false).unwrap();
        moved(&s, "untrusting a client");
        s.touch_client_token(row.id, 1_000).unwrap();
        assert_eq!(
            s.auth_epoch().unwrap(),
            last.get(),
            "a last_seen_at touch must not move the epoch"
        );
        s.revoke_client_token("phone").unwrap();
        moved(&s, "revoking a client");
        s.insert_client_token("hub-b", &"b".repeat(64), "peer")
            .unwrap();
        moved(&s, "pairing a peer");
        let link = s
            .conn
            .execute(
                "INSERT INTO peer_links (fleet_id, url, role, state, created_at, client_id) \
                 SELECT 'fleet-b', 'https://b', 'listener', 'connected', 1, id \
                   FROM client_tokens WHERE name = 'hub-b'",
                [],
            )
            .map(|_| s.conn.last_insert_rowid())
            .unwrap();
        s.revoke_peer_link(link, 2_000).unwrap();
        moved(&s, "removing a peer link (its listener client token)");
    }

    /// The snapshot's epoch names exactly the rows read with it.
    #[test]
    fn a_snapshot_carries_live_rows_at_its_epoch() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("box").unwrap();
        s.upsert_host_token("box", "t1").unwrap();
        s.insert_client_token("phone", &"a".repeat(64), "full")
            .unwrap();
        s.insert_client_token("old", &"c".repeat(64), "full")
            .unwrap();
        s.revoke_client_token("old").unwrap();
        let (epoch, hosts, clients) = s.auth_snapshot().unwrap();
        assert_eq!(epoch, s.auth_epoch().unwrap());
        assert_eq!(hosts.len(), 1);
        let names: Vec<_> = clients.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["phone"],
            "a revoked row never enters a snapshot"
        );
    }

    /// A pooled reader sees a write the writer committed immediately before,
    /// though it still sits in the WAL (no checkpoint yet).
    #[test]
    fn a_pooled_read_sees_a_write_committed_through_the_writer_just_before() {
        let dir = tempfile::tempdir().unwrap();
        let (path, writer) = file_store(&dir);
        let pool = ReadPool::open(&path, READ_POOL_SIZE)
            .unwrap()
            .expect("a WAL file gets a pool");
        let writer = Mutex::new(writer);
        for alias in ["a", "b", "c", "d"] {
            writer.lock().unwrap().upsert_host(alias).unwrap();
            let seen = read_via(Some(&pool), &writer)
                .lock()
                .unwrap()
                .list_hosts()
                .unwrap();
            assert!(
                seen.iter().any(|h| h.alias == alias),
                "the pooled read missed host {alias} the writer just committed"
            );
        }
        assert!(dir.path().join("state.db-wal").exists());
    }

    /// A file on a rollback journal gets no pool: readers there would take
    /// shared locks the writer's commit waits on.
    #[test]
    fn a_file_not_in_wal_gets_no_pool() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.db");
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch("PRAGMA journal_mode = DELETE; CREATE TABLE t (x);")
            .unwrap();
        drop(c);
        assert!(ReadPool::open(&path, READ_POOL_SIZE).unwrap().is_none());
    }

    /// `get` never hands out a connection another thread holds while a free
    /// one exists, and never blocks on the writer.
    #[test]
    fn get_prefers_a_free_connection_and_falls_back_to_the_writer_without_a_pool() {
        let dir = tempfile::tempdir().unwrap();
        let (path, writer) = file_store(&dir);
        let writer = Mutex::new(writer);
        let pool = ReadPool::open(&path, 2).unwrap().unwrap();
        let held = pool.conns[0].lock().unwrap();
        for _ in 0..4 {
            let got = pool.get().unwrap();
            assert!(
                std::ptr::eq(got, &pool.conns[1]),
                "a busy connection was handed out"
            );
        }
        drop(held);
        assert!(std::ptr::eq(read_via(None, &writer), &writer));
    }
}
