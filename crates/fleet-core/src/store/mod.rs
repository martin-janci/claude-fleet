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
mod projects;
mod read_cursors;
mod reconcile;
mod reports;
mod rows;
mod schema;
mod sessions;
mod tasks;
#[cfg(test)]
mod test_support;
mod timeline;
mod tracker_items;
mod trackers;
mod usage;
mod work;
mod work_detect;
mod work_journal;
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
pub use participants::{ParticipantRow, RETIRED_RETENTION_SECS};
pub use read_cursors::CursorRow;
pub use reports::{ReportFilter, ReportRow};
pub use rows::*;
#[cfg(test)]
pub(crate) use schema::LATEST_SCHEMA_VERSION;
pub use sessions::PromptAckState;
pub use tracker_items::{github_covers, tracker_claims, ItemMeta, TrackerItemWrite, UpsertOutcome};
pub use trackers::{
    is_allowed_tracker_host, normalize_dc_site, normalize_provider_site, normalize_site_url,
    validate_credential_ref, validate_tracker_settings, validate_tracker_transport, Secret,
    TrackerConfig, TrackerCredential, TrackerRow, TrackerSettings, TrackerViewRow,
    TRACKER_AUTH_KINDS, TRACKER_PROVIDERS, TRACKER_STATES,
};
pub use work::{
    canonical_key, github_ref, normalize_work_ref, WorkItemRow, WorkLinkRow, WorkSummary,
    WorkTarget, WORK_LINK_SOURCES,
};
pub use work_detect::DetectionState;
pub use work_journal::{
    JournalRow, COMPACT_SUMMARY_CAP, COMPACT_SUMMARY_MAX_CHARS, JOURNAL_KINDS, PROGRESS_CAP,
};
pub use work_tidy::ReopenedWork;

pub struct Store {
    conn: Connection,
    bus: StoreBus,
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
}

/// The store's handle on its [`EventBus`]. Normally a pass-through; inside
/// [`Store::atomically`] it holds every emit and releases them only after the
/// transaction commits (dropped on rollback), so no event announces a write
/// that never persisted.
struct StoreBus {
    inner: Arc<dyn EventBus>,
    held: std::sync::Mutex<Option<Vec<RowChange>>>,
}

impl StoreBus {
    fn new(inner: Arc<dyn EventBus>) -> Self {
        Self {
            inner,
            held: std::sync::Mutex::new(None),
        }
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
        self.inner.emit(e);
    }
}

impl Store {
    pub fn open_with_bus(path: &std::path::Path, bus: Arc<dyn EventBus>) -> Result<Self> {
        let conn = Connection::open(path)?;
        let store = Self {
            conn,
            bus: StoreBus::new(bus),
            kills: Default::default(),
            message_notify: Arc::new(tokio::sync::Notify::new()),
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
        Ok(Self {
            conn,
            bus: StoreBus::new(Arc::new(crate::events::NoopEventBus)),
            kills: Default::default(),
            message_notify: Arc::new(tokio::sync::Notify::new()),
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
        };
        store.migrate()?;
        Ok(store)
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
                    self.bus.inner.emit(e);
                }
            }
        }
        result
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
}
