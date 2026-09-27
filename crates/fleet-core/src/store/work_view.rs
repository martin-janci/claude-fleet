//! Work view storage (work graph M14.1b, migration 065): what the Work view
//! reads in one pass (every item, every link with its live session), and the
//! three things a person keeps there — placements, placement rules and saved
//! views — as they are read. Writing them is M14.1c's.
//!
//! Nothing here decides visibility: `service::work::view` and
//! `service::work::structure` filter by the caller's `OrgScope`.

use super::work::{link_columns_prefixed, map_item, map_link, ITEM_COLUMNS, LINK_COLUMN_COUNT};
use super::{ItemMeta, Store, WorkItemRow, WorkLinkRow};
use crate::ipc_error::IpcError;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// One work item as the Work view reads it: the row, its meta (assignee,
/// description), the tracker's containers (project / team keys, Asana
/// project gids) and a local item's own org (065).
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
/// its participant is on, and the 065 columns.
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
}
