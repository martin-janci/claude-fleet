//! The `describe` cache (see migration 068): one item's whole description,
//! held for `work.describe_cache_secs`. The only reader is
//! `service::work::describe`; nothing projects it onto the wire.
//!
//! Swept by the work retention pass alongside the tracker items it belongs
//! to (`service::work::retention`), with a floor: when
//! `work.retention.tracker_items_days` is `0` (this repo's usual "keep
//! forever"), the describe cache is swept at a fixed 30-day floor instead —
//! see `service::work::describe::DESCRIBE_CACHE_RETENTION_FLOOR_DAYS`. A
//! full-text cache with no floor at all would just be `DESCRIPTION_MAX_CHARS`'s
//! cap reopened by the back door.

use super::{now_unix, Store};
use crate::ipc_error::IpcError;
use rusqlite::OptionalExtension;

impl Store {
    /// The cached description, when there is one at most `ttl_secs` old.
    /// Inclusive: an entry fetched exactly `ttl_secs` ago (`now - fetched_at
    /// == ttl_secs`) is still served, not treated as expired. `ttl_secs == 0`
    /// turns the cache off without clearing it.
    pub fn cached_description(
        &self,
        item_id: i64,
        ttl_secs: i64,
        now: i64,
    ) -> Result<Option<String>, IpcError> {
        if ttl_secs <= 0 {
            return Ok(None);
        }
        Ok(self
            .conn
            .query_row(
                "SELECT body FROM work_item_descriptions \
                 WHERE item_id = ?1 AND fetched_at >= ?2",
                rusqlite::params![item_id, now - ttl_secs],
                |r| r.get::<_, String>(0),
            )
            .optional()?)
    }

    pub fn put_description(&self, item_id: i64, body: &str, chars: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "INSERT INTO work_item_descriptions (item_id, body, chars, fetched_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(item_id) DO UPDATE SET \
               body = excluded.body, chars = excluded.chars, fetched_at = excluded.fetched_at",
            rusqlite::params![item_id, body, chars, now_unix()],
        )?;
        Ok(())
    }

    /// Delete at most `limit` rows fetched before `older_than`, oldest
    /// `item_id` first — one statement, so the caller holds the lock for one
    /// batch only, exactly like `sweep_tracker_writes` (this cache is not a
    /// [`crate::store::RetentionTable`]: its own single column,
    /// `fetched_at`, needs no CTE). Called in a loop by
    /// `service::work::retention::sweep_capped`, batched and capped the same
    /// as every other table that pass sweeps.
    pub fn sweep_descriptions(&self, older_than: i64, limit: usize) -> Result<usize, IpcError> {
        if limit == 0 {
            return Ok(0);
        }
        Ok(self.conn.execute(
            "DELETE FROM work_item_descriptions WHERE item_id IN (\
               SELECT item_id FROM work_item_descriptions \
               WHERE fetched_at < ?1 ORDER BY item_id LIMIT ?2)",
            rusqlite::params![older_than, limit as i64],
        )?)
    }

    /// Rows currently in the describe cache — `work_admin { action: status }`'s
    /// dry-run count for it (it holds third-party full text, so it earns a
    /// row count like the other retention-swept tables, even though it is
    /// not one of them).
    pub fn describe_cache_rows(&self) -> Result<i64, IpcError> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM work_item_descriptions", [], |r| {
                r.get(0)
            })?)
    }

    /// How many describe-cache rows are older than `older_than` — the
    /// dry-run count `sweep_descriptions(older_than, usize::MAX)` would
    /// remove.
    pub fn describe_cache_eligible(&self, older_than: i64) -> Result<i64, IpcError> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM work_item_descriptions WHERE fetched_at < ?1",
            rusqlite::params![older_than],
            |r| r.get(0),
        )?)
    }
}

#[cfg(test)]
mod tests;
