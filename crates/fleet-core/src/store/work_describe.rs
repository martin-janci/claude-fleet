//! The `describe` cache (see migration 068): one item's whole description,
//! held for `work.describe_cache_secs`. The only reader is
//! `service::work::describe`; nothing projects it onto the wire.
//!
//! Swept by the work retention pass as [`crate::store::RetentionTable`]'s
//! `Descriptions` — the same batched, capped path as every other swept table,
//! counted and deleted by the same two store functions, so the dry run and
//! the sweep cannot disagree. Its window is the tracker items' with a floor:
//! when `work.retention.tracker_items_days` is `0` (this repo's usual "keep
//! forever"), the describe cache is still swept at a fixed 30-day floor —
//! see `service::work::retention::describe_effective_days` and
//! `service::work::describe::DESCRIBE_CACHE_RETENTION_FLOOR_DAYS`. A
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
}

#[cfg(test)]
mod tests;
