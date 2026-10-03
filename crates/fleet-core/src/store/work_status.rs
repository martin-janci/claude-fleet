//! Who decided an item's status (design 2026-09-28 §2). The only writers of
//! `work_items.status_set_by` live here: a person's explicit setting, and the
//! derived stamp made when a session's PR is seen merged — by
//! `Store::set_pr_signals`, where that fact is recorded, and by
//! `Store::tidy_sessions`, which reads the same signal. Both are also
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
/// 2026-09-28 §2): `NULL` when `i` is no item at all (a bare key) or its
/// stored `status_category` is itself empty (the schema never produces
/// this, but the expression must not manufacture a `todo` nobody said —
/// matching `effective_status`'s own `None` for the same input); a
/// person's setting or a stamped `done` (`status_set_by`) is final; a
/// job's status (`'task'`, an agent subtask mirroring its dispatched job)
/// is final like a stamped `done`;
/// otherwise a confirmed link whose session is presently working lifts a
/// LOCAL item (`source = 'local'`) to `in_progress`; otherwise the stored
/// value, normalised to one of the three categories.
///
/// THREE places express "a confirmed, unended `work_links` row, naming THIS
/// item, whose session is `claude_status = 'working'`", and they must all
/// read the same, because a caller can see the same item's lift through any
/// of them and they must agree:
///
/// 1. the `EXISTS` here (a session row, via `SESSION_COLUMNS` and
///    `Store::primary_work_by_session`);
/// 2. `Store::work_items_with_working_session`'s `WHERE` (`Graph::load` for
///    the Work view, and `card.rs` for one item);
/// 3. `service::work::handover::gather_stored`'s `has_working_session`,
///    which filters an already-fetched, already-org-fenced list in Rust
///    rather than asking SQL — it holds the item, so the `item_id` half of
///    the condition is `l.item_id == item.id`. It reads a KEY-shaped list
///    (`Store::live_work_sessions_for_key` matches bare `ref_key` links with
///    no item), so that `item_id` test is load-bearing there, not a
///    tautology: without it a bare-ref link's working session lifted the
///    handover while every other surface said `todo` (the final
///    whole-branch review's finding 2).
///
/// Changing one without the others is exactly the drift this macro exists
/// to avoid; `store::rows::tests::the_sql_macro_and_the_rust_function_agree_arm_by_arm`,
/// `service::work::view_tests`'s equivalent scenarios and
/// `handover::tests::a_bare_ref_links_working_session_does_not_lift_the_item`
/// are what would catch it landing wrong, but there is no automatic check
/// that the SQL texts themselves stay in lockstep — read all three before
/// editing any.
///
/// SQLite cannot call into Rust, so this duplicates `effective_status`'s
/// logic rather than calling it — `store::rows::tests::
/// effective_status_on_the_session_row` checks the real
/// `Store::get_session` path against every scenario (including
/// `status_set_by = 'derived'`, which nothing else in this crate writes
/// outside `stamp_derived_done`), and
/// `the_sql_macro_and_the_rust_function_agree_arm_by_arm` additionally
/// cross-checks this expression's SQL answer against calling
/// `effective_status` on the very same row's real fields — not two
/// independently hand-written literal expectations that could each be
/// wrong the same way.
///
/// A macro, not a `const`, so `SESSION_COLUMNS` and `primary_work_by_session`
/// can `concat!` it (the same reason `session_org_sql!` is a macro). No
/// parameters: every call site aliases the item `i`, and each use sits in
/// its own correlated subquery, so the inner alias names (`es_l`/`es_p`/
/// `es_s`) never collide across uses.
///
/// **Not fenced by `OrgScope`** (fix round 3, an open item, not silently
/// dropped): the `EXISTS` counts a confirmed working session on `i`
/// regardless of which org it belongs to, so a caller whose scope cannot
/// see that session can still see the lift here — unlike `Graph::load`'s
/// `working_session_items`, which is. Both call sites of this macro
/// (`SESSION_COLUMNS`'s `work`/`work_suggested`, and
/// `Store::primary_work_by_session`) build a `SessionRow` with no
/// `OrgScope` in hand — `list_all_sessions`/`get_session` are used by many
/// callers, scoped afterward by `OrgScope::redact_row_org_only`, which only ever
/// looks at the row's own link/session org, never at another session
/// working the same item. Fencing this properly would need either (a)
/// threading `OrgScope` through every caller of those two methods (well
/// beyond this task), or (b) a third correlated subquery per session row
/// to check the working session's visibility — which the maintainer
/// explicitly asked not to add to this path (`list_sessions` is the
/// hottest read in the app) without discussion first. Left open; a
/// batch-style fix mirroring `Graph::load`'s (fetch
/// `Store::work_items_with_working_session`'s pairs ONCE per list, then
/// filter in `redact_row_org_only` using the already-fetched `SessionRow`s) is the
/// likely shape of a real fix, at the cost of complicating `redact_row_org_only`'s
/// per-row signature into a per-list one.
///
/// `card.rs` does NOT use this macro: it is a single-item lookup, so it
/// calls `Store::work_items_with_working_session` and
/// `service::work::status::effective_status` directly and fences the
/// result itself with `OrgScope::sees_row_org_only` — cheap for one item, unlike
/// the per-row cost the same fence would add here.
#[macro_export]
macro_rules! effective_status_sql {
    () => {
        "CASE WHEN i.id IS NULL THEN NULL \
              WHEN i.status_category = '' THEN NULL \
              WHEN i.status_set_by IN ('person', 'derived', 'task') THEN \
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
    /// Reached through [`Self::stamp_derived_done_for_session`] from two
    /// places, and only those two — see that method for which, and why one
    /// of them is not enough.
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

    /// [`Self::stamp_derived_done`] for the local work `session_id` is on:
    /// its live, confirmed, PRIMARY link's item. How many items it wrote
    /// (0 or 1 today — the schema allows one live primary link per
    /// participant, and the return stays a count so a second one could not
    /// go unnoticed).
    ///
    /// **The one definition of "which item a merged PR delivered."** Both
    /// stamp sites call this, so neither can drift from the other:
    ///
    /// * [`Self::set_pr_signals`] — where the merged fact is WRITTEN. This
    ///   is the site that makes the feature work: the background reconcile
    ///   pass reaches it on every probe, `sessions.pr_signals` is deleted
    ///   with its session, and nothing else in the background stamps.
    ///   `service::work::tidy::auto_tidy` returns before `Snapshot::take`
    ///   unless `work.auto_tidy` is on (default off, D2), so before this
    ///   site existed a merged PR followed by a `kill_session` lost `done`
    ///   permanently unless a person happened to open Tidy-up first.
    /// * `Store::tidy_sessions` — kept because it is free (the pass already
    ///   reads every session's `pr_signals`) and because a person opening
    ///   Tidy-up should see delivered work as `done` in that same answer,
    ///   not one probe later. Idempotent, so a second site is harmless.
    ///
    /// The PRIMARY link only, deliberately: a session may hold confirmed
    /// secondary links (an epic, a ticket it also touched), and "my PR
    /// merged" says less about those than about the work the session is on.
    /// It NARROWS the blast radius, it does not eliminate it — an epic that
    /// is itself the session's primary work is stamped like any other item,
    /// which is correct as far as this method can tell: the only thing it
    /// knows is "this session's work", and that is what a person chose.
    pub fn stamp_derived_done_for_session(&self, session_id: i64) -> Result<usize, IpcError> {
        let items: Vec<i64> = {
            let mut stmt = self.conn.prepare(
                "SELECT l.item_id FROM work_links l \
                 JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
                 WHERE p.session_id = ?1 AND l.ended_at IS NULL AND l.is_primary = 1 \
                   AND l.state = 'confirmed' AND l.item_id IS NOT NULL",
            )?;
            let rows = stmt.query_map(rusqlite::params![session_id], |r| r.get::<_, i64>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut wrote = 0;
        for item_id in items {
            if self.stamp_derived_done(item_id)? {
                wrote += 1;
            }
        }
        Ok(wrote)
    }
}

#[cfg(test)]
mod tests;
