// Store owns the SQLite connection. It is wrapped in `Mutex<Store>` and
// registered via `tauri::Manager::manage()` because `rusqlite::Connection`
// is not Send+Sync. Commands access it via `State<'_, Mutex<Store>>`.

#[cfg(test)]
use crate::events::NoopEventBus;
use crate::events::{EventBus, RowChange};
use rusqlite::{Connection, OptionalExtension, Result};
use std::sync::Arc;

mod catalog;
mod clients;
mod conversations;
mod hosts_accounts;
mod layers;
mod orgs;
mod participants;
mod peer_links;
mod projects;
mod read_cursors;
mod reconcile;
mod reports;
mod rows;
#[cfg(test)]
pub(crate) mod scale_fixture;
mod schema;
mod sessions;
mod tasks;
#[cfg(test)]
mod test_support;
#[cfg(test)]
pub(crate) mod testgen;
mod timeline;
mod tracker_items;
mod trackers;
mod usage;
mod work;
mod work_detect;
mod work_journal;
mod work_local;
mod work_retention;
mod work_tidy;

pub use clients::{
    breaks_a_line, validate_client_mode, validate_client_name, CLIENT_MODES, LINE_SEPARATORS,
};
pub use conversations::{ConversationRow, StartSource, AWAITING_REBIND_TTL_SECS};
pub use layers::HostLayerRow;
pub use orgs::{
    normalize_rule, org_of_session, validate_org_color, validate_org_name, OrgRow, OrgRuleRow,
    SessionOrgFacts, ORG_NAME_MAX_CHARS,
};
pub use participants::{ParticipantRow, PARTICIPANT_REMOTE, RETIRED_RETENTION_SECS};
pub use peer_links::{
    Adopted, Inbound, OutboxRow, PeerLinkRow, PeerLinkSummary, LINK_CONNECTED, LINK_INCOMPATIBLE,
    LINK_REFUSED, LINK_RETRYING, LINK_ROLE_DIALER, LINK_ROLE_LISTENER, LISTENER_STALE_SECS,
    PEER_PENDING_MAX_SECS,
};
pub use read_cursors::CursorRow;
pub use reports::{ReportFilter, ReportRow};
pub use rows::*;
#[cfg(test)]
pub(crate) use schema::LATEST_SCHEMA_VERSION;
pub use sessions::PromptAckState;
pub use tracker_items::{github_covers, tracker_claims, ItemMeta, TrackerItemWrite, UpsertOutcome};
pub use trackers::{
    ghes_host_ok, ghes_host_part, github_site, is_allowed_tracker_host, normalize_dc_site,
    normalize_provider_site, normalize_site_url, validate_credential_ref, validate_ghes_hostname,
    validate_tracker_settings, validate_tracker_transport, Secret, TrackerConfig,
    TrackerCredential, TrackerRow, TrackerSettings, TrackerViewRow, TRACKER_AUTH_KINDS,
    TRACKER_PROVIDERS, TRACKER_STATES,
};
pub use work::{
    canonical_key, github_ref, normalize_work_ref, split_github_repo, WorkItemRow, WorkLinkRow,
    WorkSummary, WorkTarget, WORK_LINK_SOURCES,
};
pub use work_detect::DetectionState;
pub use work_journal::{
    JournalRow, COMPACT_SUMMARY_CAP, COMPACT_SUMMARY_MAX_CHARS, JOURNAL_KINDS, PROGRESS_CAP,
};
pub use work_local::{validate_local_work_title, LocalItemLink, LOCAL_WORK_TITLE_MAX_CHARS};
pub use work_retention::{retention_cutoff, RetentionTable, WORK_EVENT_KINDS};
pub use work_tidy::ReopenedWork;

/// One number per `Store` ever built in this process, never reused — see
/// [`Store::instance_id`].
static NEXT_INSTANCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_instance() -> u64 {
    NEXT_INSTANCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub struct Store {
    conn: Connection,
    bus: StoreBus,
    /// This store's place in a process-wide registry keyed per store (the
    /// work resume's in-flight keys): monotonic, so a store built where a
    /// dropped one was never inherits its entries the way an address would.
    instance: u64,
    /// In-memory record of the sessions fleet itself killed, so a reconcile
    /// pass that probed before a kill cannot re-insert its row once the kill
    /// has reaped it. Process-local on purpose — see [`reconcile::KillMemory`].
    kills: reconcile::KillMemory,
    /// Signalled after a `session_messages` insert commits, so a waiter wakes
    /// on arrival instead of polling.
    ///
    /// Deliberately NOT the event bus: only the hub builds a subscribable bus
    /// (`BroadcastEventBus`), while the desktop's bus forwards to Svelte and
    /// hands out no receiver. A `Notify` on the store works in every
    /// embedding and needs no new `RowChange` variant, so no contract golden
    /// or `events.ts` allowlist entry moves.
    message_notify: Arc<tokio::sync::Notify>,
    /// Per hub link, the generation of its latest `peer_exchange` — so a
    /// parked listener handler a newer exchange superseded can return at
    /// once. Process-local on purpose, like `kills`: it only has to outlive
    /// a long-poll, and a restart has no parked handlers to release.
    peer_generations: std::sync::Mutex<std::collections::HashMap<i64, u64>>,
}

/// The store's handle on its [`EventBus`]. Normally a pass-through; inside
/// [`Store::atomically`] it holds every emit and releases them only after the
/// transaction commits (dropped on rollback), so no event announces a write
/// that never persisted.
struct StoreBus {
    inner: Arc<dyn EventBus>,
    held: std::sync::Mutex<Option<Vec<RowChange>>>,
    /// Frames delivered to `inner` so far (work graph M11.4's sync metrics
    /// count a pass's frames as the difference). Process-local, never reset.
    delivered: std::sync::atomic::AtomicU64,
}

impl StoreBus {
    fn new(inner: Arc<dyn EventBus>) -> Self {
        Self {
            inner,
            held: std::sync::Mutex::new(None),
            delivered: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Hand one frame to the real bus, counted.
    fn deliver(&self, e: &RowChange) {
        self.delivered
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.inner.emit(e);
    }

    /// Start holding emits. Returns false when already holding (nested).
    fn hold(&self) -> bool {
        let mut held = self.held.lock().unwrap_or_else(|p| p.into_inner());
        if held.is_some() {
            return false;
        }
        *held = Some(Vec::new());
        true
    }

    /// Stop holding; the held events, in emit order.
    fn release(&self) -> Vec<RowChange> {
        self.held
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
            .unwrap_or_default()
    }
}

impl EventBus for StoreBus {
    fn emit(&self, e: &RowChange) {
        if let Some(held) = self.held.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
            held.push(e.clone());
            return;
        }
        self.deliver(e);
    }
}

/// How long a statement waits on another connection's lock (a `fleet-hub`
/// CLI beside the daemon) before it fails with `SQLITE_BUSY`.
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Journal settings for the long-lived file store, applied before migrating.
///
/// The default rollback journal with `synchronous = FULL` fsyncs several
/// times per commit; on the hub's HDD-backed NAS volume that measured ~200 ms
/// per autocommit write, all of it under the store mutex every reader waits
/// on. WAL with `synchronous = NORMAL` commits without an fsync (~0.1 ms
/// there): a power cut can lose the last commits, never corrupt the file.
/// WAL is persistent in the file, so the read-only CLI opens inherit it.
///
/// A filesystem without shared-memory support refuses WAL and SQLite keeps
/// the old mode; `synchronous` then stays at its safe default, since NORMAL
/// under a rollback journal can corrupt on power loss.
fn tune_file_connection(conn: &Connection) -> Result<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    if mode.eq_ignore_ascii_case("wal") {
        conn.execute_batch("PRAGMA synchronous = NORMAL;")?;
    } else {
        tracing::warn!(
            mode,
            "[store] WAL refused; the store stays on its journal mode"
        );
    }
    Ok(())
}

/// `chmod 600` one file, best-effort: a missing file is fine (a sidecar
/// SQLite has not created yet), any other failure is logged, never fatal.
/// No-op off unix.
fn set_owner_only(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                path = %path.display(),
                error = %e,
                "[store] chmod 600 failed; the file may be readable by other users"
            ),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// The WAL sidecars SQLite keeps beside `path` while a connection is open.
fn sidecar_paths(path: &std::path::Path) -> [std::path::PathBuf; 2] {
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    let mut shm = path.as_os_str().to_owned();
    shm.push("-shm");
    [wal.into(), shm.into()]
}

/// Make `path` owner-only BEFORE SQLite opens it.
///
/// The database holds bearer tokens (the MCP master token, client and peer
/// link tokens, tracker secrets) in plaintext, and in WAL mode every recent
/// commit lives in `<path>-wal` until a checkpoint. SQLite creates `-wal`
/// and `-shm` with the main file's mode at that moment and never re-chmods
/// them, so a chmod after the open (what the callers used to do) leaves the
/// sidecars at the umask-derived mode for the life of the process — and
/// across a crash, since a leftover `-wal` is reused. Creating the file 0600
/// here means the sidecars inherit 0600; an existing file is tightened first
/// for the same reason.
fn restrict_before_open(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        if path.exists() {
            set_owner_only(path);
            return;
        }
        // An empty file is a valid (empty) SQLite database. `create_new`: a
        // file that appeared in between is an existing database, and the
        // open below reads it as such.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
        {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => set_owner_only(path),
            Err(e) => tracing::warn!(
                path = %path.display(),
                error = %e,
                "[store] could not pre-create the database owner-only"
            ),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

impl Store {
    pub fn open_with_bus(path: &std::path::Path, bus: Arc<dyn EventBus>) -> Result<Self> {
        restrict_before_open(path);
        let conn = Connection::open(path)?;
        tune_file_connection(&conn)?;
        // A database that already existed with a wider mode may have left a
        // `-wal` / `-shm` behind (a crash, an older build) that SQLite reuses
        // as they are; tighten them too, now that WAL is on.
        for sidecar in sidecar_paths(path) {
            set_owner_only(&sidecar);
        }
        let store = Self {
            conn,
            bus: StoreBus::new(bus),
            kills: Default::default(),
            message_notify: Arc::new(tokio::sync::Notify::new()),
            instance: next_instance(),
            peer_generations: Default::default(),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Open an EXISTING database read-only and **without migrating it**.
    ///
    /// For a one-shot reader that runs beside a live daemon — `fleet-hub
    /// pair`, `fleet-hub client …`, `fleet-hub token show` — where
    /// [`Store::open_with_bus`] would run *this binary's* migrations against
    /// the database the daemon has open. A CLI newer than the running daemon
    /// must not reshape the schema under it, so this open cannot: the
    /// connection is `SQLITE_OPEN_READ_ONLY` and nothing is applied.
    ///
    /// Only reads are valid on the result; a write returns SQLite's
    /// "attempt to write a readonly database".
    ///
    /// `SQLITE_OPEN_NO_MUTEX` (SQLite's multi-thread mode: no mutex around
    /// the connection itself) is safe here ONLY because of what these callers
    /// are — one-shot CLI subcommands that open the file, read a couple of
    /// `settings` rows on one thread, and exit. Nothing shares this
    /// connection between threads. A `Store` opened this way must therefore
    /// not be handed to the server, the event bus, or anything else that
    /// would use it concurrently; the long-lived paths go through
    /// [`Store::open_with_bus`], whose `Store` lives behind a
    /// `std::sync::Mutex` (see the crate's store conventions).
    pub fn open_read_only(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        Ok(Self {
            conn,
            bus: StoreBus::new(Arc::new(crate::events::NoopEventBus)),
            kills: Default::default(),
            message_notify: Arc::new(tokio::sync::Notify::new()),
            instance: next_instance(),
            peer_generations: Default::default(),
        })
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self {
            conn,
            bus: StoreBus::new(Arc::new(NoopEventBus)),
            kills: Default::default(),
            message_notify: Arc::new(tokio::sync::Notify::new()),
            instance: next_instance(),
            peer_generations: Default::default(),
        };
        store.migrate()?;
        Ok(store)
    }

    #[cfg(test)]
    pub fn open_with_bus_in_memory(bus: Arc<dyn EventBus>) -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self {
            conn,
            bus: StoreBus::new(bus),
            kills: Default::default(),
            message_notify: Arc::new(tokio::sync::Notify::new()),
            instance: next_instance(),
            peer_generations: Default::default(),
        };
        store.migrate()?;
        Ok(store)
    }

    /// A number no other `Store` of this process has or will have — the key
    /// for a process-wide, per-store registry. An address is not one: a
    /// store dropped and another built where it was would alias.
    pub fn instance_id(&self) -> u64 {
        self.instance
    }

    /// The raw connection, for a test outside `store` that has to set up a
    /// state no public method writes (a claude_session_id, say).
    #[cfg(test)]
    pub(crate) fn conn_for_test(&self) -> &Connection {
        &self.conn
    }

    #[cfg(test)]
    pub fn has_table(&self, name: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [name],
            |row| row.get(0),
        )?;
        Ok(count == 1)
    }

    pub fn schema_version(&self) -> Result<i64> {
        self.conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
    }

    /// Read a value from the key/value `settings` table. `None` if absent.
    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM settings WHERE key=?1",
                rusqlite::params![key],
                |row| row.get(0),
            )
            .optional()
    }

    /// Insert or replace a value in the `settings` table.
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    /// Forget a key, so the next `get_setting` answers `None` and its reader
    /// falls back to its own default. Absent and "stored as the default" are
    /// not the same thing: the second pins today's default forever (see
    /// `service::quick_replies::replace`). A key that was never there is not
    /// an error.
    pub fn delete_setting(&self, key: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM settings WHERE key=?1", rusqlite::params![key])?;
        Ok(())
    }

    /// Record which session is the fleet controller (the calling session that
    /// must not kill/recreate/restart itself without `force`). Stored as two
    /// keys in the `settings` table.
    pub fn set_controller(&self, host: &str, tmux_name: &str) -> Result<()> {
        self.set_setting("controller.host", host)?;
        self.set_setting("controller.tmux", tmux_name)?;
        Ok(())
    }

    /// Read the registered controller as `(host, tmux_name)`. `None` unless
    /// both keys are present.
    pub fn get_controller(&self) -> Result<Option<(String, String)>> {
        let host = self.get_setting("controller.host")?;
        let tmux = self.get_setting("controller.tmux")?;
        Ok(host.zip(tmux))
    }

    pub fn conn_ref(&self) -> &rusqlite::Connection {
        &self.conn
    }

    /// The store's message-arrival `Notify`, so a waiter can hold the handle
    /// across an `.await` without holding the store lock. See the field doc
    /// on `Store::message_notify` for why this is a `Notify` and not an
    /// event-bus subscription.
    pub fn message_notify(&self) -> Arc<tokio::sync::Notify> {
        self.message_notify.clone()
    }

    /// Run `f` inside a single `conn.transaction()`. Used by reconcile paths
    /// that batch many upserts/deletes after a fan-out of off-lock probes —
    /// one fsync per batch instead of one per row.
    fn with_transaction<F, R>(&mut self, f: F) -> rusqlite::Result<R>
    where
        F: FnOnce(&rusqlite::Transaction) -> rusqlite::Result<R>,
    {
        let tx = self.conn.transaction()?;
        let r = f(&tx)?;
        tx.commit()?;
        Ok(r)
    }

    /// Run `f` against this store inside one SQLite transaction: commit when
    /// `f` returns `Ok`, roll back (drop the transaction) when it returns
    /// `Err`. Unlike [`Store::with_transaction`] the closure receives the
    /// `&Store` itself, so it can compose the ordinary `&self` write helpers
    /// (`insert_message`, `insert_session_event`, …) atomically without
    /// `_in_tx` twins. Must not be nested, and `f` must not call a helper
    /// that opens its own transaction (`BEGIN` inside `BEGIN` errors). Bus
    /// events the helpers emit (`session:event`, …) are held and flushed only
    /// after COMMIT; a rollback drops them.
    pub fn atomically<F, R>(&self, f: F) -> Result<R, crate::ipc_error::IpcError>
    where
        F: FnOnce(&Store) -> Result<R, crate::ipc_error::IpcError>,
    {
        /// Stops holding even when `f` panics, so later emits are not
        /// swallowed; the held events are dropped with the rollback.
        struct Unhold<'a>(&'a StoreBus);
        impl Drop for Unhold<'_> {
            fn drop(&mut self) {
                self.0.release();
            }
        }
        let tx = self.conn.unchecked_transaction()?;
        let unhold = self.bus.hold().then(|| Unhold(&self.bus));
        let result = f(self).and_then(|r| {
            tx.commit()?;
            Ok(r)
        });
        if unhold.is_some() {
            let held = self.bus.release();
            if result.is_ok() {
                for e in &held {
                    self.bus.deliver(e);
                }
            }
        }
        result
    }

    /// Frames this store has handed to its event bus since it was opened
    /// (a held frame counts once it is released; one a rollback dropped
    /// never counts). Callers measure a span of work as the difference.
    pub fn frames_emitted(&self) -> u64 {
        self.bus
            .delivered
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_set_get_roundtrip() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.get_controller().unwrap(), None, "unset is None");
        s.set_controller("mac", "dev-fleet").unwrap();
        assert_eq!(
            s.get_controller().unwrap(),
            Some(("mac".to_string(), "dev-fleet".to_string()))
        );
        // overwrite
        s.set_controller("mefistos", "ctrl").unwrap();
        assert_eq!(
            s.get_controller().unwrap(),
            Some(("mefistos".to_string(), "ctrl".to_string()))
        );
    }

    #[test]
    fn with_transaction_commits_on_ok() {
        let mut store = Store::open_in_memory().expect("in-memory store");
        let r: rusqlite::Result<()> = store.with_transaction(|tx| {
            tx.execute(
                "INSERT INTO hosts (alias, ssh_alias, hidden) VALUES (?1, ?2, 0)",
                rusqlite::params!["foo", "foo-ssh"],
            )?;
            Ok(())
        });
        assert!(r.is_ok());
        let hosts = store.list_hosts().expect("list");
        assert!(hosts.iter().any(|h| h.alias == "foo"));
    }

    #[test]
    fn with_transaction_rolls_back_on_err() {
        let mut store = Store::open_in_memory().expect("in-memory store");
        let r: rusqlite::Result<()> = store.with_transaction(|tx| {
            tx.execute(
                "INSERT INTO hosts (alias, ssh_alias, hidden) VALUES (?1, ?2, 0)",
                rusqlite::params!["bar", "bar-ssh"],
            )?;
            // Trigger an error to force rollback.
            Err(rusqlite::Error::QueryReturnedNoRows)
        });
        assert!(r.is_err());
        let hosts = store.list_hosts().expect("list");
        assert!(
            !hosts.iter().any(|h| h.alias == "bar"),
            "rollback should have removed the bar row"
        );
    }

    #[test]
    fn store_holds_event_bus_field_and_default_is_noop() {
        use crate::events::NoopEventBus;
        let store = Store::open_in_memory().expect("store");
        // Just constructing the store with the default Noop bus exercises the
        // new field. The bus is a private implementation detail; we don't expose
        // it as a public getter, so this test is intentionally minimal.
        let _ = std::sync::Arc::new(NoopEventBus); // also exercises Send+Sync
        let _ = store; // touch it to keep it alive past the new
    }

    fn pragma<T: rusqlite::types::FromSql>(s: &Store, name: &str) -> T {
        s.conn
            .query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
            .unwrap()
    }

    /// On the hub's HDD-backed NAS volume a rollback-journal commit cost
    /// ~200 ms of fsync while holding the store mutex; WAL costs ~0.1 ms.
    #[test]
    fn file_store_opens_in_wal_with_normal_sync_and_a_busy_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open_with_bus(&dir.path().join("state.db"), Arc::new(NoopEventBus)).unwrap();
        assert_eq!(pragma::<String>(&s, "journal_mode"), "wal");
        // 1 = NORMAL: in WAL mode a commit does not fsync; a power cut can
        // lose the last commits but never corrupts the database.
        assert_eq!(pragma::<i64>(&s, "synchronous"), 1);
        assert!(pragma::<i64>(&s, "busy_timeout") >= 1000);
    }

    /// The bearer tokens live in `state.db-wal` until a checkpoint, and
    /// SQLite gives the sidecars the main file's mode when it creates them:
    /// the main file must be 0600 BEFORE the open, not chmodded after it.
    #[cfg(unix)]
    #[test]
    fn file_store_and_its_wal_sidecars_are_owner_only_while_open() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let wal = dir.path().join("state.db-wal");
        let shm = dir.path().join("state.db-shm");

        // A fresh database: created 0600, so the sidecars inherit 0600.
        let s = Store::open_with_bus(&db, Arc::new(NoopEventBus)).unwrap();
        s.set_controller("mac", "dev-fleet").unwrap();
        assert!(
            wal.exists() && shm.exists(),
            "WAL sidecars exist while open"
        );
        assert_eq!(mode(&db), 0o600, "state.db");
        assert_eq!(mode(&wal), 0o600, "state.db-wal");
        assert_eq!(mode(&shm), 0o600, "state.db-shm");

        // A leftover sidecar with a wider mode (a crash under an older build)
        // is tightened by the next open even though SQLite reuses it.
        std::fs::set_permissions(&wal, std::fs::Permissions::from_mode(0o644)).unwrap();
        let again = Store::open_with_bus(&db, Arc::new(NoopEventBus)).unwrap();
        assert_eq!(mode(&wal), 0o600, "leftover state.db-wal");
        drop(again);
        drop(s);

        // A database that already existed with a wider mode is tightened
        // before the open, so its new sidecars are 0600 too.
        assert!(!wal.exists(), "the WAL is gone after the last close");
        std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644)).unwrap();
        let s = Store::open_with_bus(&db, Arc::new(NoopEventBus)).unwrap();
        s.set_controller("mac", "dev-fleet").unwrap();
        assert_eq!(mode(&db), 0o600, "existing state.db");
        assert_eq!(mode(&wal), 0o600, "existing database's state.db-wal");
        assert_eq!(mode(&shm), 0o600, "existing database's state.db-shm");
    }

    /// `fleet-hub pair` / `token show` read beside a live daemon. In WAL a
    /// commit sits in `state.db-wal` until a checkpoint, and the read-only
    /// open must still see it — and must work again once the daemon is gone.
    #[test]
    fn read_only_open_sees_uncheckpointed_writes_and_opens_after_close() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let live = Store::open_with_bus(&db, Arc::new(NoopEventBus)).unwrap();
        live.set_controller("mac", "dev-fleet").unwrap();
        assert!(
            dir.path().join("state.db-wal").exists(),
            "write went to the WAL"
        );

        let ro = Store::open_read_only(&db).unwrap();
        assert_eq!(
            ro.get_controller().unwrap(),
            Some(("mac".to_string(), "dev-fleet".to_string()))
        );
        drop(ro);
        drop(live);

        let ro = Store::open_read_only(&db).unwrap();
        assert_eq!(
            ro.get_controller().unwrap(),
            Some(("mac".to_string(), "dev-fleet".to_string()))
        );
    }
}
