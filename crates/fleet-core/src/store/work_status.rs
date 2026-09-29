//! Who decided an item's status (design 2026-09-28 §2). The only writers of
//! `work_items.status_set_by` live here: a person's explicit setting, and the
//! derived stamp `work_tidy` makes when it sees a merged PR. Both are also
//! the only local-item writers of `status_changed_at` (a tracker item's is
//! written by `store::tracker_items` on every sync) — without it, the tidy
//! planner's `done_long` (`service/gc/tidy.rs`) can never see a local item
//! as long done, whoever set it.

use super::{now_unix, Store, WorkItemRow};
use crate::ipc_error::{codes, IpcError};

/// The three values `status_category` may hold. `blocked` is deliberately not
/// among them: it is a property of a session, which the row already shows.
pub const STATUS_CATEGORIES: [&str; 3] = ["todo", "in_progress", "done"];

/// The live-lifted status of a `work_items` row aliased `i` in scope, as one
/// SQL expression — the same precedence
/// `service::work::status::effective_status` computes in Rust (design
/// 2026-09-28 §2, fix round 2 of native item status task 4): a person's
/// setting or a stamped `done` (`status_set_by`) is final; otherwise a
/// confirmed link whose session is presently working lifts a LOCAL item
/// (`source = 'local'`) to `in_progress`; otherwise the stored value,
/// normalised to one of the three categories. `NULL` when `i` is no item at
/// all (a bare key).
///
/// SQLite cannot call into Rust, so this duplicates `effective_status`'s
/// logic rather than calling it — `store::rows::tests` and
/// `store::work::tests` check this expression against `effective_status`
/// directly (the same scenarios, same expected answers) so the two do not
/// quietly drift apart.
///
/// A macro, not a `const`, so `SESSION_COLUMNS` and `primary_work_by_session`
/// can `concat!` it (the same reason `session_org_sql!` is a macro). No
/// parameters: every call site aliases the item `i`, and each use sits in
/// its own correlated subquery, so the inner alias names (`es_l`/`es_p`/
/// `es_s`) never collide across uses.
#[macro_export]
macro_rules! effective_status_sql {
    () => {
        "CASE WHEN i.id IS NULL THEN NULL \
              WHEN i.status_set_by IN ('person', 'derived') THEN \
                CASE i.status_category WHEN 'done' THEN 'done' \
                                        WHEN 'in_progress' THEN 'in_progress' \
                                        ELSE 'todo' END \
              WHEN i.source = 'local' AND EXISTS ( \
                     SELECT 1 FROM work_links es_l \
                       JOIN participants es_p ON es_p.id = es_l.participant_id \
                                              AND es_p.retired_at IS NULL \
                       JOIN sessions es_s     ON es_s.id = es_p.session_id \
                      WHERE es_l.item_id = i.id AND es_l.ended_at IS NULL \
                        AND es_l.state = 'confirmed' AND es_s.claude_status = 'working') \
                THEN 'in_progress' \
              ELSE CASE i.status_category WHEN 'done' THEN 'done' \
                                           WHEN 'in_progress' THEN 'in_progress' \
                                           ELSE 'todo' END \
         END"
    };
}

impl Store {
    /// A person sets a local item's status. `Ok(None)` when the id is unknown,
    /// so a caller outside the item's scope gets the answer an unknown id gets.
    ///
    /// Refused for a tracker item: `store::tracker_items` writes
    /// `status_category` on every sync, so the setting would be reverted on the
    /// next pass — worse than saying no.
    ///
    /// Also stamps `status_changed_at`: it is the only local-item writer of
    /// that column (only tracker sync wrote it before), and without it a
    /// person's own `done` could never age into `done_idle` — the tidy
    /// planner's `done_long` reads `status_changed_at`, not `status_set_at`.
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
             status_set_at = ?2, status_changed_at = ?2, updated_at = ?2 WHERE id = ?3",
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

    /// Record that a local item's work was delivered, once.
    ///
    /// Stamped rather than computed because the merged-PR signal lives on
    /// `sessions.pr_signals` and dies with its session: a computed `done` would
    /// silently revert to `todo` once the work's session was swept, which is
    /// the worst behaviour for the one status a release depends on.
    ///
    /// Returns whether it wrote. It never overrides a person (`status_set_by =
    /// 'person'`), never touches a tracker item, and never writes twice — so a
    /// tidy pass may call it every tick without emitting an event per tick.
    /// Also stamps `status_changed_at`, for the same reason `set_item_status`
    /// does: it is what the tidy planner's `done_long` reads.
    ///
    /// Called from `Store::tidy_sessions` — a read-shaped path: the Tauri
    /// `work_tidy` command and the MCP tool both reach it just by listing
    /// candidates. That is intentional (moving the stamp to the GC sweep
    /// alone would mean a person listing candidates does not see delivered
    /// work as `done` until the next sweep) and safe, because this method is
    /// idempotent and writes at most once per item regardless of how many
    /// times a read calls it.
    pub fn stamp_derived_done(&self, item_id: i64) -> Result<bool, IpcError> {
        let wrote = self.conn.execute(
            "UPDATE work_items SET status_category = 'done', status_set_by = 'derived', \
             status_set_at = ?1, status_changed_at = ?1, updated_at = ?1 \
             WHERE id = ?2 AND source = 'local' \
               AND COALESCE(status_set_by, '') <> 'person' \
               AND NOT (status_category = 'done' AND status_set_by = 'derived')",
            rusqlite::params![now_unix(), item_id],
        )? == 1;
        if wrote {
            self.emit_work_item(
                item_id,
                super::tracker_items::SessionChange {
                    primary: true,
                    suggested: false,
                    rejected: false,
                },
            )?;
        }
        Ok(wrote)
    }
}

#[cfg(test)]
mod tests;
