//! Work view storage (work graph M14.1b / M14.1c, migration 066): what the
//! Work view reads in one pass (every item, every link with its live
//! session), and the three things a person keeps there — placements,
//! placement rules and saved views — each with a `version` a write must name
//! to replace it (M14.1c's compare-and-set), plus a local item's own org.
//!
//! Nothing here decides visibility: `service::work::view` filters by the
//! caller's `OrgScope`, and `service::work::structure` gates the writes.

use super::work::{
    link_columns_prefixed, map_item, map_link, ITEM_COLUMNS, ITEM_COLUMN_COUNT, LINK_COLUMN_COUNT,
};
use super::{now_unix, Store, WorkItemRow, WorkLinkRow};
use crate::events::{EventBus as _, RowChange, WorkChanged};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// One work item as the Work view reads it: the row, its assignee, the
/// tracker's containers (project / team keys, Asana project gids) and a
/// local item's own org (066). The rest of `meta` (the description) is not
/// read here: only a task's detail needs it, and it reads that one item's
/// through [`Store::work_item_meta`].
#[derive(Debug, Clone)]
pub struct ViewItem {
    pub item: WorkItemRow,
    /// `meta.assignee_id` (the tracker account the item is assigned to).
    pub assignee_id: Option<String>,
    pub containers: Vec<String>,
    /// `work_items.org_id`: a LOCAL item's own org; never read for a
    /// tracker item (its org is its tracker's).
    pub own_org: Option<i64>,
    /// `meta.iteration_active`: the item's sprint is the tracker's active one.
    pub iteration_active: bool,
}

/// One link as the Work view reads it: the row (its `org_id` still the
/// snapshot's org, `snap_org_id`, as `map_link` leaves it), the live session
/// its participant is on, and the 066 columns.
#[derive(Debug, Clone)]
pub struct ViewLink {
    pub link: WorkLinkRow,
    pub session_id: Option<i64>,
    pub version: i64,
    pub archived_at: Option<i64>,
    pub review_ack_at: Option<i64>,
}

/// A person's placement of one task (navigation only, never a boundary).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub version: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
}

/// What a placement rule matches: every condition it sets must hold.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, rmcp::schemars::JsonSchema,
)]
#[schemars(crate = "rmcp::schemars")]
pub struct RuleConditions {
    /// The task's tracker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker_id: Option<i64>,
    /// A tracker container (project / team key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// Key prefix, e.g. PAY.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_prefix: Option<String>,
    /// Text in the title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_contains: Option<String>,
    /// owner/repo of its sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

impl RuleConditions {
    /// No condition set: a rule that would match everything is refused.
    pub fn is_empty(&self) -> bool {
        self.tracker_id.is_none()
            && self.container.is_none()
            && self.key_prefix.is_none()
            && self.title_contains.is_none()
            && self.repo.is_none()
    }
}

/// A placement rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkRule {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub version: i64,
    pub conditions: RuleConditions,
    pub group: String,
    /// "Its sessions start here" (migration 164): the host a start of a
    /// task it matches lands on when no start rule names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_alias: Option<String>,
    /// The account (credential profile) those sessions bill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A saved view: a name and the Work view's filters, as JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkView {
    pub id: i64,
    pub name: String,
    pub filters: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_org: Option<i64>,
    pub version: i64,
    pub updated_at: i64,
}

/// `E_CONFLICT` for a write that named a version the row no longer has
/// (work graph M14.1c); `details` carry the current value.
pub fn version_conflict(what: &str, current: i64, details: serde_json::Value) -> IpcError {
    IpcError::new(
        codes::E_CONFLICT,
        format!(
            "{what} was changed by someone else meanwhile (now version {current}); \
             reload it and decide again"
        ),
    )
    .with_details(details)
}

fn not_found(what: &str, id: i64) -> IpcError {
    IpcError::new(codes::E_NOTFOUND, format!("{what} {id} not found"))
}

fn json_list(raw: Option<String>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// The columns after `ITEM_COLUMNS` that make a [`ViewItem`].
const VIEW_ITEM_EXTRA: &str = "CASE WHEN json_valid(w.meta) \
          AND json_type(w.meta, '$.assignee_id') = 'text' \
         THEN json_extract(w.meta, '$.assignee_id') END, containers, org_id, \
     CASE WHEN json_valid(w.meta) \
          AND json_extract(w.meta, '$.iteration_active') = 1 \
         THEN 1 ELSE 0 END";

fn map_view_item(r: &rusqlite::Row<'_>) -> rusqlite::Result<ViewItem> {
    Ok(ViewItem {
        item: map_item(r)?,
        assignee_id: r.get(ITEM_COLUMN_COUNT)?,
        containers: json_list(r.get(ITEM_COLUMN_COUNT + 1)?),
        own_org: r.get(ITEM_COLUMN_COUNT + 2)?,
        iteration_active: r.get::<_, i64>(ITEM_COLUMN_COUNT + 3)? == 1,
    })
}

fn map_rule(r: &rusqlite::Row<'_>) -> rusqlite::Result<WorkRule> {
    Ok(WorkRule {
        id: r.get(0)?,
        name: r.get(1)?,
        enabled: r.get::<_, i64>(2)? != 0,
        conditions: RuleConditions {
            tracker_id: r.get(3)?,
            container: r.get(4)?,
            key_prefix: r.get(5)?,
            title_contains: r.get(6)?,
            repo: r.get(7)?,
        },
        group: r.get(8)?,
        version: r.get(9)?,
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
        host_alias: r.get(12)?,
        profile: r.get(13)?,
    })
}

const RULE_COLUMNS: &str = "id, name, enabled, tracker_id, container, key_prefix, title_contains, \
     repo, group_label, version, created_at, updated_at, host_alias, profile";

fn map_view(r: &rusqlite::Row<'_>) -> rusqlite::Result<WorkView> {
    let raw: String = r.get(2)?;
    Ok(WorkView {
        id: r.get(0)?,
        name: r.get(1)?,
        filters: serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null),
        owner_org: r.get(3)?,
        version: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

const VIEW_COLUMNS: &str = "id, name, filters, owner_org, version, updated_at";

impl Store {
    // --- the one read pass --------------------------------------------------

    /// Every work item the Work view may show: local items, items of a
    /// tracker that still exists, and a removed tracker's items that some
    /// link still names (kept for that link's history).
    pub fn work_view_items(&self) -> Result<Vec<ViewItem>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ITEM_COLUMNS}, {VIEW_ITEM_EXTRA} FROM work_items w \
             WHERE w.tracker_id IS NULL OR w.tracker_id IN (SELECT id FROM trackers) \
                OR EXISTS (SELECT 1 FROM work_links l WHERE l.item_id = w.id)"
        ))?;
        let rows = stmt.query_map([], map_view_item)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Item `id` as the Work view reads it: what a placement rule matches
    /// on, for a start of that one task (gap plan G7.1).
    pub fn work_view_item(&self, id: i64) -> Result<Option<ViewItem>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {ITEM_COLUMNS}, {VIEW_ITEM_EXTRA} FROM work_items w WHERE w.id = ?1"
                ),
                [id],
                map_view_item,
            )
            .optional()?)
    }

    /// Every link, with the live session its participant is on.
    pub fn work_view_links(&self) -> Result<Vec<ViewLink>, IpcError> {
        self.view_links_where("1 = 1", rusqlite::params![])
    }

    /// `(item_id, session_id)` for every live confirmed link whose session
    /// is presently working (native item status, design 2026-09-28 §2 rule
    /// 3): what `service::work::status::effective_status` lifts to
    /// `in_progress` (for a local item; the function itself gates on
    /// `source`). One query for the whole page — the shape
    /// `Store::tidy_sessions`'s `in_progress` set already uses, joined
    /// through `sessions` instead of filtered by the item's own stored
    /// category.
    ///
    /// Returns the session too, not just the item id, so the caller can
    /// fence the lift by `OrgScope` (`Graph::load` does: a scope that
    /// cannot see the session must not see its "working" derived either —
    /// fix round 2, a caller-side check this store method does not make).
    ///
    /// `l.item_id IS NOT NULL` is load-bearing, not an optimisation: a
    /// `work_links` row may name a bare `ref_key` with no item yet
    /// (migration 046's CHECK allows either), and `l.item_id` is `NULL` for
    /// one. Without this clause, `r.get::<_, i64>(0)` on that NULL would
    /// **error** — not silently admit a bogus id — failing the whole query
    /// and, with it, every `tree` / `task` / `session_tasks` / `review`
    /// call that loads a `Graph`.
    ///
    /// This `WHERE`, `crate::effective_status_sql!`'s `EXISTS`
    /// (`store/work_status.rs`) and
    /// `service::work::handover::gather_stored`'s `has_working_session` must
    /// read as the same condition — all three answer "is a confirmed,
    /// unended `work_links` row naming this item on a session with
    /// `claude_status = 'working'`" — so a caller sees the same lift
    /// whichever path served it. The macro's doc lists all three; check
    /// them before changing any.
    pub fn work_items_with_working_session(&self) -> Result<Vec<(i64, i64)>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT l.item_id, p.session_id FROM work_links l \
             JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
             JOIN sessions s     ON s.id = p.session_id \
             WHERE l.ended_at IS NULL AND l.state = 'confirmed' \
               AND s.claude_status = 'working' AND l.item_id IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn view_links_where(
        &self,
        cond: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<ViewLink>, IpcError> {
        let cols = link_columns_prefixed("l");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {cols}, p.session_id, l.version, l.archived_at, l.review_ack_at \
             FROM work_links l \
             LEFT JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
             WHERE {cond} \
             ORDER BY COALESCE(l.ended_at, l.decided_at, l.created_at) DESC, l.id DESC"
        ))?;
        let rows = stmt.query_map(params, |r| {
            Ok(ViewLink {
                link: map_link(r)?,
                session_id: r.get(LINK_COLUMN_COUNT)?,
                version: r.get(LINK_COLUMN_COUNT + 1)?,
                archived_at: r.get(LINK_COLUMN_COUNT + 2)?,
                review_ack_at: r.get(LINK_COLUMN_COUNT + 3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Set a local item's own org (work graph M14.1c, `assign_org`).
    /// Refused for a tracker item, whose org is its tracker's. Re-announces
    /// the row of every live session linked to it (their `work.org_id`
    /// moved), so a reader outside the new org stops receiving the task
    /// from its next frame.
    pub fn set_local_item_org(&self, item_id: i64, org: Option<i64>) -> Result<(), IpcError> {
        let tracker: Option<Option<i64>> = self
            .conn
            .query_row(
                "SELECT tracker_id FROM work_items WHERE id = ?1",
                rusqlite::params![item_id],
                |r| r.get(0),
            )
            .optional()?;
        match tracker {
            None => return Err(not_found("work item", item_id)),
            Some(Some(_)) => {
                return Err(IpcError::new(
                    codes::E_FORBIDDEN,
                    format!(
                        "work item {item_id} is a tracker item: its org is its tracker's \
                         (work_admin assign_tracker)"
                    ),
                ))
            }
            Some(None) => {}
        }
        if let Some(o) = org {
            if self.get_org(o)?.is_none() {
                return Err(not_found("org", o));
            }
        }
        let sessions: Vec<i64> = {
            let mut stmt = self.conn.prepare(
                "SELECT DISTINCT p.session_id FROM work_links l \
                 JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
                 WHERE l.item_id = ?1 AND l.ended_at IS NULL AND p.session_id IS NOT NULL",
            )?;
            let rows = stmt.query_map(rusqlite::params![item_id], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "UPDATE work_items SET org_id = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![item_id, org, now_unix()],
        )?;
        for sid in &sessions {
            self.bump_session_for_work(*sid)?;
        }
        tx.commit()?;
        for sid in sessions {
            self.emit_session(sid)?;
        }
        self.emit_work_changed(WorkChanged {
            what: "org".into(),
            task_id: Some(format!("item:{item_id}")),
            rule_id: None,
            view_id: None,
        });
        Ok(())
    }

    /// Emit `work:changed` (work graph M14.1d): ids only, kind `work`, so a
    /// host-bound or org-bound stream never carries it; a client re-reads
    /// what it shows.
    pub fn emit_work_changed(&self, change: WorkChanged) {
        self.bus.emit(&RowChange::WorkChanged(change));
    }

    // --- placements ---------------------------------------------------------

    pub fn work_placements(&self) -> Result<Vec<Placement>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id, group_label, note, version, updated_at, updated_by \
             FROM work_placements",
        )?;
        let rows = stmt.query_map([], map_placement)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Drop the placements whose task no longer exists: an `item:N` whose
    /// item row is gone, a `ref:K` no bare link (`ref_key = K`, no item)
    /// names any more. Run by [`Store::bind_tracker_refs`] after it re-keys
    /// the placements it can; never on the Work view's read path. Returns
    /// how many were removed.
    pub fn sweep_orphan_placements(&self) -> Result<usize, IpcError> {
        let n = self.conn.execute(
            "DELETE FROM work_placements WHERE \
               (task_id LIKE 'item:%' AND NOT EXISTS (SELECT 1 FROM work_items w \
                  WHERE 'item:' || w.id = work_placements.task_id)) \
               OR (task_id LIKE 'ref:%' AND NOT EXISTS (SELECT 1 FROM work_links l \
                  WHERE l.item_id IS NULL AND 'ref:' || l.ref_key = work_placements.task_id))",
            [],
        )?;
        Ok(n)
    }

    pub fn work_placement(&self, task_id: &str) -> Result<Option<Placement>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT task_id, group_label, note, version, updated_at, updated_by \
                 FROM work_placements WHERE task_id = ?1",
                rusqlite::params![task_id],
                map_placement,
            )
            .optional()?)
    }

    /// Place `task_id` in `group` with `note` — both `None` removes the
    /// placement — if its placement is still at `expected` (`0`: there is
    /// none). A compare-and-set (work graph M14.1c): a concurrent placement
    /// from another device answers `E_CONFLICT` instead of being
    /// overwritten. Returns the placement, if one remains.
    pub fn set_work_placement(
        &self,
        task_id: &str,
        group: Option<&str>,
        note: Option<&str>,
        expected: i64,
        by: &str,
    ) -> Result<Option<Placement>, IpcError> {
        let tx = self.conn.unchecked_transaction()?;
        let current = self.work_placement(task_id)?;
        let have = current.as_ref().map_or(0, |p| p.version);
        if have != expected {
            return Err(version_conflict(
                &format!("the placement of {task_id}"),
                have,
                serde_json::json!({
                    "task_id": task_id, "version": have,
                    "group": current.as_ref().and_then(|p| p.group.clone()),
                }),
            ));
        }
        if group.is_none() && note.is_none() {
            self.conn.execute(
                "DELETE FROM work_placements WHERE task_id = ?1",
                rusqlite::params![task_id],
            )?;
        } else {
            self.conn.execute(
                "INSERT INTO work_placements (task_id, group_label, note, version, updated_at, updated_by) \
                 VALUES (?1, ?2, ?3, 1, ?4, ?5) \
                 ON CONFLICT(task_id) DO UPDATE SET group_label = ?2, note = ?3, \
                   version = version + 1, updated_at = ?4, updated_by = ?5",
                rusqlite::params![task_id, group, note, now_unix(), by],
            )?;
        }
        tx.commit()?;
        self.emit_work_changed(WorkChanged {
            what: "placement".into(),
            task_id: Some(task_id.to_string()),
            rule_id: None,
            view_id: None,
        });
        self.work_placement(task_id)
    }

    // --- rules --------------------------------------------------------------

    /// Every placement rule, oldest first (the order they apply in).
    pub fn work_rules(&self) -> Result<Vec<WorkRule>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RULE_COLUMNS} FROM work_rules ORDER BY id"
        ))?;
        let rows = stmt.query_map([], map_rule)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn work_rule(&self, id: i64) -> Result<Option<WorkRule>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {RULE_COLUMNS} FROM work_rules WHERE id = ?1"),
                rusqlite::params![id],
                map_rule,
            )
            .optional()?)
    }

    /// Create a rule (`id` `None`) or replace rule `id` if it is still at
    /// `expected` (`None`: any). The caller validated the fields.
    #[allow(clippy::too_many_arguments)]
    pub fn save_work_rule(
        &self,
        id: Option<i64>,
        name: &str,
        enabled: bool,
        c: &RuleConditions,
        group: &str,
        start: (Option<&str>, Option<&str>),
        expected: Option<i64>,
    ) -> Result<WorkRule, IpcError> {
        let (host, profile) = start;
        let now = now_unix();
        let id = match id {
            None => {
                self.conn.execute(
                    "INSERT INTO work_rules (name, enabled, tracker_id, container, key_prefix, \
                       title_contains, repo, group_label, version, created_at, updated_at, \
                       host_alias, profile) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?9, ?10, ?11)",
                    rusqlite::params![
                        name,
                        enabled as i64,
                        c.tracker_id,
                        c.container,
                        c.key_prefix,
                        c.title_contains,
                        c.repo,
                        group,
                        now,
                        host,
                        profile
                    ],
                )?;
                self.conn.last_insert_rowid()
            }
            Some(id) => {
                let tx = self.conn.unchecked_transaction()?;
                let cur = self.work_rule(id)?.ok_or_else(|| not_found("rule", id))?;
                if expected.is_some_and(|e| e != cur.version) {
                    return Err(version_conflict(
                        &format!("rule {id}"),
                        cur.version,
                        serde_json::json!({ "rule_id": id, "version": cur.version }),
                    ));
                }
                self.conn.execute(
                    "UPDATE work_rules SET name = ?2, enabled = ?3, tracker_id = ?4, container = ?5, \
                       key_prefix = ?6, title_contains = ?7, repo = ?8, group_label = ?9, \
                       version = version + 1, updated_at = ?10, host_alias = ?11, profile = ?12 \
                     WHERE id = ?1",
                    rusqlite::params![
                        id,
                        name,
                        enabled as i64,
                        c.tracker_id,
                        c.container,
                        c.key_prefix,
                        c.title_contains,
                        c.repo,
                        group,
                        now,
                        host,
                        profile
                    ],
                )?;
                tx.commit()?;
                id
            }
        };
        self.emit_work_changed(WorkChanged {
            what: "rule".into(),
            task_id: None,
            rule_id: Some(id),
            view_id: None,
        });
        self.work_rule(id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "rule vanished after write"))
    }

    /// Delete rule `id` if it is still at `expected` (`None`: any).
    pub fn delete_work_rule(&self, id: i64, expected: Option<i64>) -> Result<(), IpcError> {
        let cur = self.work_rule(id)?.ok_or_else(|| not_found("rule", id))?;
        if expected.is_some_and(|e| e != cur.version) {
            return Err(version_conflict(
                &format!("rule {id}"),
                cur.version,
                serde_json::json!({ "rule_id": id, "version": cur.version }),
            ));
        }
        self.conn.execute(
            "DELETE FROM work_rules WHERE id = ?1 AND version = ?2",
            rusqlite::params![id, cur.version],
        )?;
        self.emit_work_changed(WorkChanged {
            what: "rule".into(),
            task_id: None,
            rule_id: Some(id),
            view_id: None,
        });
        Ok(())
    }

    // --- saved views --------------------------------------------------------

    /// The saved views `owner` may list: all of them for `None` (an
    /// unrestricted caller), else only those saved under that org.
    pub fn work_views(&self, owner: Option<Option<i64>>) -> Result<Vec<WorkView>, IpcError> {
        let mut out = Vec::new();
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {VIEW_COLUMNS} FROM work_views ORDER BY name, id"
        ))?;
        for v in stmt.query_map([], map_view)? {
            let v = v?;
            if owner.is_none_or(|o| o == v.owner_org) {
                out.push(v);
            }
        }
        Ok(out)
    }

    pub fn work_view(&self, id: i64) -> Result<Option<WorkView>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {VIEW_COLUMNS} FROM work_views WHERE id = ?1"),
                rusqlite::params![id],
                map_view,
            )
            .optional()?)
    }
}

impl Store {
    /// Create a saved view kept by `owner_org` (`None`: an unrestricted
    /// caller's), or replace view `id` when `owner` may reach it and it is
    /// still at `expected` (`None`: any). `owner` is whose views the caller
    /// keeps (`None`: every view, `Some(o)`: only those with `owner_org`
    /// `o`) — a view out of reach answers as one that does not exist. A
    /// replaced view keeps its owner. A name another view of the same owner
    /// has is refused (`E_EXISTS`).
    pub fn save_work_view(
        &self,
        id: Option<i64>,
        name: &str,
        filters: &serde_json::Value,
        owner: Option<Option<i64>>,
        expected: Option<i64>,
    ) -> Result<WorkView, IpcError> {
        let now = now_unix();
        let raw = filters.to_string();
        let dup = |e: rusqlite::Error| -> IpcError {
            if matches!(
                &e,
                rusqlite::Error::SqliteFailure(f, _)
                    if f.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
            ) {
                IpcError::new(
                    codes::E_EXISTS,
                    format!("a view named {name:?} already exists"),
                )
            } else {
                IpcError::from(e)
            }
        };
        let id = match id {
            None => {
                self.conn
                    .execute(
                        "INSERT INTO work_views (name, filters, owner_org, version, created_at, updated_at) \
                         VALUES (?1, ?2, ?3, 1, ?4, ?4)",
                        rusqlite::params![name, raw, owner.flatten(), now],
                    )
                    .map_err(dup)?;
                self.conn.last_insert_rowid()
            }
            Some(id) => {
                let tx = self.conn.unchecked_transaction()?;
                let cur = self
                    .work_view(id)?
                    .filter(|v| owner.is_none_or(|o| v.owner_org == o))
                    .ok_or_else(|| not_found("view", id))?;
                if expected.is_some_and(|e| e != cur.version) {
                    return Err(version_conflict(
                        &format!("view {id}"),
                        cur.version,
                        serde_json::json!({ "view_id": id, "version": cur.version }),
                    ));
                }
                self.conn
                    .execute(
                        "UPDATE work_views SET name = ?2, filters = ?3, version = version + 1, \
                           updated_at = ?4 WHERE id = ?1",
                        rusqlite::params![id, name, raw, now],
                    )
                    .map_err(dup)?;
                tx.commit()?;
                id
            }
        };
        self.emit_work_changed(WorkChanged {
            what: "view".into(),
            task_id: None,
            rule_id: None,
            view_id: Some(id),
        });
        self.work_view(id)?
            .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "view vanished after write"))
    }

    /// Delete saved view `id` when `owner` may reach it (as in
    /// [`Self::save_work_view`]) and it is still at `expected` (`None`:
    /// any).
    pub fn delete_work_view(
        &self,
        id: i64,
        owner: Option<Option<i64>>,
        expected: Option<i64>,
    ) -> Result<(), IpcError> {
        let cur = self
            .work_view(id)?
            .filter(|v| owner.is_none_or(|o| v.owner_org == o))
            .ok_or_else(|| not_found("view", id))?;
        if expected.is_some_and(|e| e != cur.version) {
            return Err(version_conflict(
                &format!("view {id}"),
                cur.version,
                serde_json::json!({ "view_id": id, "version": cur.version }),
            ));
        }
        self.conn.execute(
            "DELETE FROM work_views WHERE id = ?1 AND version = ?2",
            rusqlite::params![id, cur.version],
        )?;
        self.emit_work_changed(WorkChanged {
            what: "view".into(),
            task_id: None,
            rule_id: None,
            view_id: Some(id),
        });
        Ok(())
    }
}

fn map_placement(r: &rusqlite::Row<'_>) -> rusqlite::Result<Placement> {
    Ok(Placement {
        task_id: r.get(0)?,
        group: r.get(1)?,
        note: r.get(2)?,
        version: r.get(3)?,
        updated_at: r.get(4)?,
        updated_by: r.get(5)?,
    })
}

/// Seeds for the Work view's read tests (work graph M14.1b). The writes a
/// person makes through the hub — placement, rules, views, a local item's
/// org, a secondary link — are M14.1c's; until then the tests write the
/// rows the reads answer directly.
#[cfg(test)]
impl Store {
    pub(crate) fn seed_placement(&self, task_id: &str, group: Option<&str>, note: Option<&str>) {
        self.conn
            .execute(
                "INSERT INTO work_placements (task_id, group_label, note, updated_at, updated_by) \
                 VALUES (?1, ?2, ?3, ?4, 'test') \
                 ON CONFLICT(task_id) DO UPDATE SET group_label = ?2, note = ?3, \
                   version = version + 1, updated_at = ?4",
                rusqlite::params![task_id, group, note, super::now_unix()],
            )
            .unwrap();
    }

    /// Remove a work item row outright (no store path deletes one; the
    /// placement sweep's test needs an `item:N` whose item is gone).
    pub(crate) fn seed_delete_item(&self, id: i64) {
        self.conn
            .execute("DELETE FROM work_items WHERE id = ?1", [id])
            .unwrap();
    }

    pub(crate) fn seed_rule(&self, name: &str, c: &RuleConditions, group: &str) -> i64 {
        self.conn
            .execute(
                "INSERT INTO work_rules (name, tracker_id, container, key_prefix, title_contains, \
                   repo, group_label, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                rusqlite::params![
                    name,
                    c.tracker_id,
                    c.container,
                    c.key_prefix,
                    c.title_contains,
                    c.repo,
                    group,
                    super::now_unix()
                ],
            )
            .unwrap();
        self.conn.last_insert_rowid()
    }

    pub(crate) fn seed_rule_enabled(&self, id: i64, on: bool) {
        self.conn
            .execute(
                "UPDATE work_rules SET enabled = ?2, version = version + 1 WHERE id = ?1",
                rusqlite::params![id, on as i64],
            )
            .unwrap();
    }

    pub(crate) fn seed_view(
        &self,
        name: &str,
        filters: &serde_json::Value,
        owner_org: Option<i64>,
    ) -> i64 {
        self.conn
            .execute(
                "INSERT INTO work_views (name, filters, owner_org, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                rusqlite::params![name, filters.to_string(), owner_org, super::now_unix()],
            )
            .unwrap();
        self.conn.last_insert_rowid()
    }

    pub(crate) fn seed_local_item_org(&self, item_id: i64, org: Option<i64>) {
        self.conn
            .execute(
                "UPDATE work_items SET org_id = ?2 WHERE id = ?1 AND tracker_id IS NULL",
                rusqlite::params![item_id, org],
            )
            .unwrap();
    }

    /// Make live link `link_id` its session's only primary (`true`) or a
    /// secondary (`false`), as M14.1c's `set_primary` / `primary: false` will.
    pub(crate) fn seed_link_primary(&self, link_id: i64, primary: bool) {
        if primary {
            self.conn
                .execute(
                    "UPDATE work_links SET is_primary = 0 WHERE ended_at IS NULL AND id <> ?1 \
                       AND participant_id = (SELECT participant_id FROM work_links WHERE id = ?1)",
                    rusqlite::params![link_id],
                )
                .unwrap();
        }
        self.conn
            .execute(
                "UPDATE work_links SET is_primary = ?2 WHERE id = ?1",
                rusqlite::params![link_id, primary as i64],
            )
            .unwrap();
    }

    /// A conflict a person kept (M14.1c's `ack`).
    pub(crate) fn seed_review_ack(&self, link_id: i64) {
        self.conn
            .execute(
                "UPDATE work_links SET review_ack_at = ?2 WHERE id = ?1",
                rusqlite::params![link_id, super::now_unix()],
            )
            .unwrap();
    }

    /// End live link `link_id` while its session lives on, as a branch
    /// change does (no snapshot: the session is still there).
    pub(crate) fn seed_end_link(&self, link_id: i64, reason: &str) {
        self.conn
            .execute(
                "UPDATE work_links SET ended_at = ?2, end_reason = ?3, is_primary = 0 \
                 WHERE id = ?1",
                rusqlite::params![link_id, super::now_unix(), reason],
            )
            .unwrap();
    }

    /// A second row of link `link_id` — same participant, target, state and
    /// end — as no single store path writes, but as a session's history can
    /// hold (the same task linked, ended, linked again). Returns its id.
    pub(crate) fn seed_duplicate_link(&self, link_id: i64) -> i64 {
        self.conn
            .execute(
                "INSERT INTO work_links (item_id, ref_key, participant_id, state, source, \
                   created_at, decided_at, ended_at, end_reason, claude_session_id, strength, \
                   rule, evidence) \
                 SELECT item_id, ref_key, participant_id, state, source, created_at + 1, \
                   decided_at, ended_at, end_reason, claude_session_id, strength, rule, evidence \
                 FROM work_links WHERE id = ?1",
                rusqlite::params![link_id],
            )
            .unwrap();
        self.conn.last_insert_rowid()
    }
}
