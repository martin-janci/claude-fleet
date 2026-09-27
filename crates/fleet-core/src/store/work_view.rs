//! Work view storage (work graph M14, migration 063): what the Work view
//! reads in one pass (every item, every link with its live session), and the
//! three things a person keeps there — placements, placement rules and saved
//! views — each with a `version` a write must name to replace it.
//!
//! Nothing here decides visibility: `service::work::view` filters by the
//! caller's `OrgScope`, and `service::work::structure` gates the writes.

use super::work::{link_columns_prefixed, map_item, map_link, ITEM_COLUMNS, LINK_COLUMN_COUNT};
use super::{now_unix, ItemMeta, Store, WorkItemRow, WorkLinkRow};
use crate::events::{EventBus as _, RowChange, WorkChanged};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// One work item as the Work view reads it: the row, its meta (assignee,
/// description), the tracker's containers (project / team keys, Asana
/// project gids) and a local item's own org (063).
#[derive(Debug, Clone)]
pub struct ViewItem {
    pub item: WorkItemRow,
    pub meta: ItemMeta,
    pub containers: Vec<String>,
    /// `work_items.org_id`: a LOCAL item's own org; never read for a
    /// tracker item (its org is its tracker's).
    pub own_org: Option<i64>,
}

/// One link as the Work view reads it: the row (its `org_id` still the
/// snapshot's org, `snap_org_id`, as `map_link` leaves it), the live session
/// its participant is on, and the 063 columns.
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

/// `E_CONFLICT` for a write that named a version the row no longer has.
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

fn json_list(raw: Option<String>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
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
    })
}

const RULE_COLUMNS: &str = "id, name, enabled, tracker_id, container, key_prefix, title_contains, \
     repo, group_label, version, created_at, updated_at";

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
    /// Emit `work:changed` (ids only).
    pub fn emit_work_changed(&self, change: WorkChanged) {
        self.bus.emit(&RowChange::WorkChanged(change));
    }

    // --- the one read pass --------------------------------------------------

    /// Every work item the Work view may show: local items, items of a
    /// tracker that still exists, and a removed tracker's items that some
    /// link still names (kept for that link's history).
    pub fn work_view_items(&self) -> Result<Vec<ViewItem>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ITEM_COLUMNS}, meta, containers, org_id FROM work_items w \
             WHERE w.tracker_id IS NULL OR w.tracker_id IN (SELECT id FROM trackers) \
                OR EXISTS (SELECT 1 FROM work_links l WHERE l.item_id = w.id)"
        ))?;
        let rows = stmt.query_map([], |r| {
            let meta: Option<String> = r.get(23)?;
            Ok(ViewItem {
                item: map_item(r)?,
                meta: ItemMeta::parse(meta.as_deref()),
                containers: json_list(r.get(24)?),
                own_org: r.get(25)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every link, with the live session its participant is on.
    pub fn work_view_links(&self) -> Result<Vec<ViewLink>, IpcError> {
        self.view_links_where("1 = 1", rusqlite::params![])
    }

    /// The links of one session's participant (live, suggested, rejected
    /// and ended), newest first.
    pub fn work_view_session_links(&self, session_id: i64) -> Result<Vec<ViewLink>, IpcError> {
        self.view_links_where(
            "l.participant_id = (SELECT id FROM participants \
                                  WHERE session_id = ?1 AND retired_at IS NULL)",
            rusqlite::params![session_id],
        )
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

    /// A link's current `version` (migration 063), `None` when it is gone.
    pub fn work_link_version(&self, link_id: i64) -> Result<Option<i64>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT version FROM work_links WHERE id = ?1",
                rusqlite::params![link_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Refuse with `E_CONFLICT` when link `link_id` is no longer at
    /// `expected` (a person decided on a state someone else changed since).
    /// `None` expects nothing (an older client): no check.
    pub fn check_link_version(&self, link_id: i64, expected: Option<i64>) -> Result<(), IpcError> {
        let Some(expected) = expected else {
            return Ok(());
        };
        let row: Option<(i64, String, i64, Option<i64>)> = self
            .conn
            .query_row(
                "SELECT version, state, is_primary, ended_at FROM work_links WHERE id = ?1",
                rusqlite::params![link_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        match row {
            Some((v, state, primary, ended)) if v != expected => Err(version_conflict(
                &format!("work link {link_id}"),
                v,
                serde_json::json!({
                    "link_id": link_id, "version": v, "state": state,
                    "primary": primary != 0, "ended": ended.is_some(),
                }),
            )),
            _ => Ok(()),
        }
    }

    /// Set a local item's own org (063). Refused for a tracker item, whose
    /// org is its tracker's. Emits the rows of every live session linked to
    /// it (their `work.org_id` moved) and `work:changed { org }`.
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
            None => {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("work item {item_id} not found"),
                ))
            }
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
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("org {o} not found"),
                ));
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

    // --- placements ---------------------------------------------------------

    pub fn work_placements(&self) -> Result<Vec<Placement>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id, group_label, note, version, updated_at, updated_by \
             FROM work_placements",
        )?;
        let rows = stmt.query_map([], map_placement)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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
    /// none). A compare-and-set: a concurrent placement from another device
    /// answers `E_CONFLICT` instead of being overwritten. Emits
    /// `work:changed { placement }`. Returns the placement, if one remains.
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
        let now = now_unix();
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
                rusqlite::params![task_id, group, note, now, by],
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
    /// `expected`. The caller validated the fields.
    pub fn save_work_rule(
        &self,
        id: Option<i64>,
        name: &str,
        enabled: bool,
        c: &RuleConditions,
        group: &str,
        expected: Option<i64>,
    ) -> Result<WorkRule, IpcError> {
        let now = now_unix();
        let id = match id {
            None => {
                self.conn.execute(
                    "INSERT INTO work_rules (name, enabled, tracker_id, container, key_prefix, \
                       title_contains, repo, group_label, version, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?9)",
                    rusqlite::params![
                        name,
                        enabled as i64,
                        c.tracker_id,
                        c.container,
                        c.key_prefix,
                        c.title_contains,
                        c.repo,
                        group,
                        now
                    ],
                )?;
                self.conn.last_insert_rowid()
            }
            Some(id) => {
                let tx = self.conn.unchecked_transaction()?;
                let cur = self.work_rule(id)?.ok_or_else(|| {
                    IpcError::new(codes::E_NOTFOUND, format!("rule {id} not found"))
                })?;
                if let Some(e) = expected {
                    if e != cur.version {
                        return Err(version_conflict(
                            &format!("rule {id}"),
                            cur.version,
                            serde_json::json!({ "rule_id": id, "version": cur.version }),
                        ));
                    }
                }
                self.conn.execute(
                    "UPDATE work_rules SET name = ?2, enabled = ?3, tracker_id = ?4, container = ?5, \
                       key_prefix = ?6, title_contains = ?7, repo = ?8, group_label = ?9, \
                       version = version + 1, updated_at = ?10 WHERE id = ?1",
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
                        now
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
        let cur = self
            .work_rule(id)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("rule {id} not found")))?;
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

    /// Create or replace a saved view. Replacing needs the view to be at
    /// `expected` when given; a name another view of the same owner has is
    /// refused (`E_EXISTS`).
    pub fn save_work_view(
        &self,
        id: Option<i64>,
        name: &str,
        filters: &serde_json::Value,
        owner_org: Option<i64>,
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
                        rusqlite::params![name, raw, owner_org, now],
                    )
                    .map_err(dup)?;
                self.conn.last_insert_rowid()
            }
            Some(id) => {
                let cur = self
                    .work_view(id)?
                    .filter(|v| v.owner_org == owner_org)
                    .ok_or_else(|| {
                        IpcError::new(codes::E_NOTFOUND, format!("view {id} not found"))
                    })?;
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

    /// Delete a saved view of `owner_org`.
    pub fn delete_work_view(&self, id: i64, owner_org: Option<i64>) -> Result<(), IpcError> {
        let n = self.conn.execute(
            "DELETE FROM work_views WHERE id = ?1 AND owner_org IS ?2",
            rusqlite::params![id, owner_org],
        )?;
        if n == 0 {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("view {id} not found"),
            ));
        }
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
