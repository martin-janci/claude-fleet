//! Who decided an item's status (design 2026-09-28 §2). The only writers of
//! `work_items.status_set_by` live here: a person's explicit setting, and the
//! derived stamp `work_tidy` makes when it sees a merged PR.

use super::{now_unix, Store, WorkItemRow};
use crate::ipc_error::{codes, IpcError};

/// The three values `status_category` may hold. `blocked` is deliberately not
/// among them: it is a property of a session, which the row already shows.
pub const STATUS_CATEGORIES: [&str; 3] = ["todo", "in_progress", "done"];

impl Store {
    /// A person sets a local item's status. `Ok(None)` when the id is unknown,
    /// so a caller outside the item's scope gets the answer an unknown id gets.
    ///
    /// Refused for a tracker item: `store::tracker_items` writes
    /// `status_category` on every sync, so the setting would be reverted on the
    /// next pass — worse than saying no.
    pub fn set_item_status(
        &self,
        item_id: i64,
        status: &str,
    ) -> Result<Option<WorkItemRow>, IpcError> {
        if !STATUS_CATEGORIES.contains(&status) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "a status is one of {} — `blocked` is a session's state, not an item's",
                    STATUS_CATEGORIES.join(", ")
                ),
            ));
        }
        let Some(before) = self.get_work_item(item_id)? else {
            return Ok(None);
        };
        if before.source != "local" {
            let which = before.key.clone().unwrap_or_else(|| before.title.clone());
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "{which}'s status belongs to its tracker; change it there, \
                     or track this work as a local item"
                ),
            ));
        }
        let now = now_unix();
        self.conn.execute(
            "UPDATE work_items SET status_category = ?1, status_set_by = 'person', \
             status_set_at = ?2, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![status, now, item_id],
        )?;
        self.emit_work_item(
            item_id,
            super::tracker_items::SessionChange {
                primary: true,
                suggested: false,
                rejected: false,
            },
        )?;
        self.get_work_item(item_id)
    }
}

#[cfg(test)]
mod tests;
