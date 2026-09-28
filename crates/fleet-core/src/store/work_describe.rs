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
    /// The cached description, when there is one younger than `ttl_secs`.
    /// `ttl_secs == 0` turns the cache off without clearing it.
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

    /// Drop everything fetched before `older_than`. Called by the work
    /// retention pass.
    pub fn sweep_descriptions(&self, older_than: i64) -> Result<usize, IpcError> {
        Ok(self.conn.execute(
            "DELETE FROM work_item_descriptions WHERE fetched_at < ?1",
            rusqlite::params![older_than],
        )?)
    }
}

#[cfg(test)]
mod tests;
