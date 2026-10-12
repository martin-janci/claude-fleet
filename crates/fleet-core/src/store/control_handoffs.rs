//! Control's handoff receipts (`control_handoffs`, migration 140, redesign
//! steps 9.3 and 9.6): what the operator session sent where. The decision of
//! which calls are handoffs is `service::control_handoffs`'; this is the
//! table and the read that joins each receipt to its target's live state.

use super::Store;
use rusqlite::{OptionalExtension, Result};
use serde::{Deserialize, Serialize};

/// Receipts kept: older rows are dropped as new ones arrive. Control shows
/// the recent ones; the history is in the transcript.
pub const HANDOFFS_KEEP: i64 = 500;

/// A preview is cut to this many characters.
pub const HANDOFF_PREVIEW_MAX: usize = 160;

/// One receipt to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewHandoff {
    /// `session` | `mission` | `task` | `tree`.
    pub kind: &'static str,
    pub tool: String,
    pub session_id: Option<i64>,
    pub task_id: Option<i64>,
    pub mission_id: Option<i64>,
    pub item_id: Option<i64>,
    pub item_ids: Vec<i64>,
    pub preview: Option<String>,
}

/// A kind the table's `CHECK` accepts (review r02: a derived `Default` left
/// `kind` empty, which the insert refuses). Every caller still names its own
/// kind; this only keeps `..Default::default()` from building a row that
/// cannot be written.
impl Default for NewHandoff {
    fn default() -> Self {
        NewHandoff {
            kind: "session",
            tool: String::new(),
            session_id: None,
            task_id: None,
            mission_id: None,
            item_id: None,
            item_ids: Vec::new(),
            preview: None,
        }
    }
}

/// A work item a receipt points at, as it is now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffItem {
    pub id: i64,
    pub title: String,
    /// The item's status category (`todo`, `in_progress`, `done`, …).
    pub status: String,
    /// `proposed` | `accepted` | `rejected`, or none for an item nobody
    /// proposed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_state: Option<String>,
    /// An accepted item's last change, unix seconds: the clock the card's
    /// Undo runs out on (`ACCEPT_UNDO_SECS` after it, as `undo_accept`
    /// counts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_at: Option<i64>,
    /// The item's done-when lines (`work_items.done_when`: `ci:<check>`,
    /// `review`, `test:<command>`, `person`): the plan card's "finishes
    /// when" (gap plan G3.11).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub done_when: Vec<String>,
    /// The items it waits for (`work_item_deps`): the plan card draws the
    /// tree's items by wave from them (gap plan G3.11).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<i64>,
}

/// One receipt with its target's live state, as `control_handoffs` lists it.
/// A session's state is not here: the desktop has the live row already.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlHandoffRow {
    pub id: i64,
    pub at: i64,
    pub kind: String,
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission_id: Option<i64>,
    /// The mission's name and state now; absent once it is deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission_state: Option<String>,
    /// kind `task`: the created item; kind `tree`: the parent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<HandoffItem>,
    /// kind `tree`: the proposed items as they are now, in proposal order;
    /// a deleted one is left out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<HandoffItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// The first non-empty line of `text`, cut to [`HANDOFF_PREVIEW_MAX`]
/// characters with an ellipsis; `None` for blank text.
pub fn handoff_preview(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    if line.chars().count() <= HANDOFF_PREVIEW_MAX {
        return Some(line.to_string());
    }
    let cut: String = line.chars().take(HANDOFF_PREVIEW_MAX - 1).collect();
    Some(format!("{}…", cut.trim_end()))
}

impl Store {
    /// Write one receipt and drop the ones past [`HANDOFFS_KEEP`]; answers
    /// the row id.
    pub fn insert_control_handoff(&self, h: &NewHandoff, now: i64) -> Result<i64> {
        let ids = if h.item_ids.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&h.item_ids).unwrap_or_else(|_| "[]".into()))
        };
        self.conn
            .prepare_cached(
                "INSERT INTO control_handoffs \
                 (at, kind, tool, session_id, task_id, mission_id, item_id, item_ids, preview) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?
            .execute(rusqlite::params![
                now,
                h.kind,
                h.tool,
                h.session_id,
                h.task_id,
                h.mission_id,
                h.item_id,
                ids,
                h.preview,
            ])?;
        let id = self.conn.last_insert_rowid();
        self.conn
            .prepare_cached("DELETE FROM control_handoffs WHERE id <= ?1")?
            .execute([id - HANDOFFS_KEEP])?;
        Ok(id)
    }

    /// The newest `limit` receipts, newest first, each joined to its
    /// target's state now.
    pub fn list_control_handoffs(&self, limit: i64) -> Result<Vec<ControlHandoffRow>> {
        let mut st = self.conn.prepare_cached(
            "SELECT h.id, h.at, h.kind, h.tool, h.session_id, h.task_id, h.mission_id, \
                    m.name, m.state, h.item_id, h.item_ids, h.preview \
             FROM control_handoffs h \
             LEFT JOIN orchestration_projects m ON m.id = h.mission_id \
             ORDER BY h.id DESC LIMIT ?1",
        )?;
        let raw = st
            .query_map([limit.clamp(1, HANDOFFS_KEEP)], |r| {
                Ok((
                    ControlHandoffRow {
                        id: r.get(0)?,
                        at: r.get(1)?,
                        kind: r.get(2)?,
                        tool: r.get(3)?,
                        session_id: r.get(4)?,
                        task_id: r.get(5)?,
                        mission_id: r.get(6)?,
                        mission_name: r.get(7)?,
                        mission_state: r.get(8)?,
                        item: None,
                        items: Vec::new(),
                        preview: r.get(11)?,
                    },
                    r.get::<_, Option<i64>>(9)?,
                    r.get::<_, Option<String>>(10)?,
                ))
            })?
            .collect::<Result<Vec<_>>>()?;
        let mut out = Vec::with_capacity(raw.len());
        for (mut row, item_id, item_ids) in raw {
            if let Some(id) = item_id {
                row.item = self.handoff_item(id)?;
            }
            let ids: Vec<i64> = item_ids
                .as_deref()
                .and_then(|j| serde_json::from_str(j).ok())
                .unwrap_or_default();
            for id in ids {
                if let Some(it) = self.handoff_item(id)? {
                    row.items.push(it);
                }
            }
            out.push(row);
        }
        Ok(out)
    }

    fn handoff_item(&self, id: i64) -> Result<Option<HandoffItem>> {
        let item = self
            .conn
            .prepare_cached(
                "SELECT id, title, status_category, proposal_state, \
                        CASE WHEN proposal_state = 'accepted' THEN updated_at END, done_when \
                 FROM work_items WHERE id = ?1",
            )?
            .query_row([id], |r| {
                let done_when: Option<String> = r.get(5)?;
                Ok(HandoffItem {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    status: r.get(2)?,
                    proposal_state: r.get(3)?,
                    accepted_at: r.get(4)?,
                    done_when: done_when
                        .as_deref()
                        .and_then(|j| serde_json::from_str(j).ok())
                        .unwrap_or_default(),
                    depends_on: Vec::new(),
                })
            })
            .optional()?;
        let Some(mut item) = item else {
            return Ok(None);
        };
        item.depends_on = self
            .conn
            .prepare_cached(
                "SELECT depends_on FROM work_item_deps WHERE item_id = ?1 ORDER BY depends_on",
            )?
            .query_map([id], |r| r.get(0))?
            .collect::<Result<Vec<i64>>>()?;
        Ok(Some(item))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review r02: a default receipt is one the table accepts.
    #[test]
    fn a_default_receipt_can_be_written() {
        let s = Store::open_in_memory().unwrap();
        s.insert_control_handoff(&NewHandoff::default(), 100)
            .unwrap();
    }

    #[test]
    fn a_preview_is_the_first_line_cut_short() {
        assert_eq!(
            handoff_preview("\n  fix the build \nmore"),
            Some("fix the build".into())
        );
        assert_eq!(handoff_preview("  \n "), None);
        let long = "x".repeat(400);
        let p = handoff_preview(&long).unwrap();
        assert_eq!(p.chars().count(), HANDOFF_PREVIEW_MAX);
        assert!(p.ends_with('…'));
    }

    #[test]
    fn a_receipt_reads_back_newest_first_and_old_ones_are_dropped() {
        let s = Store::open_in_memory().unwrap();
        for i in 0..(HANDOFFS_KEEP + 3) {
            s.insert_control_handoff(
                &NewHandoff {
                    kind: "session",
                    tool: "send_prompt".into(),
                    session_id: Some(i),
                    preview: Some(format!("p{i}")),
                    ..Default::default()
                },
                1_000 + i,
            )
            .unwrap();
        }
        let rows = s.list_control_handoffs(HANDOFFS_KEEP * 2).unwrap();
        assert_eq!(rows.len() as i64, HANDOFFS_KEEP);
        assert_eq!(rows[0].session_id, Some(HANDOFFS_KEEP + 2));
        assert_eq!(
            rows[0].preview.as_deref(),
            Some(&*format!("p{}", HANDOFFS_KEEP + 2))
        );
        assert_eq!(s.list_control_handoffs(2).unwrap().len(), 2);
    }

    #[test]
    fn a_tree_receipt_carries_its_items_as_they_are_now() {
        let s = Store::open_in_memory().unwrap();
        let parent = s.create_local_work_item(None, "Ship 9.6").unwrap();
        let kids = s
            .propose_tree(
                parent.id,
                &[
                    crate::store::TreeEntry {
                        title: "first".into(),
                        ..Default::default()
                    },
                    crate::store::TreeEntry {
                        title: "second".into(),
                        ..Default::default()
                    },
                ],
                "client:ux-agent",
            )
            .unwrap();
        s.insert_control_handoff(
            &NewHandoff {
                kind: "tree",
                tool: "work_link".into(),
                item_id: Some(parent.id),
                item_ids: kids.iter().map(|k| k.id).collect(),
                ..Default::default()
            },
            1_000,
        )
        .unwrap();
        s.decide_proposal(kids[0].id, true).unwrap();
        let row = s.list_control_handoffs(10).unwrap().remove(0);
        assert_eq!(row.kind, "tree");
        assert_eq!(row.item.as_ref().map(|i| i.id), Some(parent.id));
        assert_eq!(row.items.len(), 2);
        assert_eq!(row.items[0].proposal_state.as_deref(), Some("accepted"));
        assert!(row.items[0].accepted_at.is_some());
        assert_eq!(row.items[1].proposal_state.as_deref(), Some("proposed"));
        assert_eq!(row.items[1].accepted_at, None);
    }

    /// Gap plan G3.11: the plan card's waves and "finishes when" come from
    /// each item's edges and done-when lines.
    #[test]
    fn a_tree_item_carries_its_edges_and_done_when() {
        let s = Store::open_in_memory().unwrap();
        let parent = s.create_local_work_item(None, "Ship G3.11").unwrap();
        let kids = s
            .propose_tree(
                parent.id,
                &[
                    crate::store::TreeEntry {
                        title: "schema".into(),
                        ..Default::default()
                    },
                    crate::store::TreeEntry {
                        title: "ui".into(),
                        depends_on: vec![crate::store::TreeRef::Entry(0)],
                        ..Default::default()
                    },
                ],
                "client:ux-agent",
            )
            .unwrap();
        s.conn_ref()
            .execute(
                "UPDATE work_items SET done_when = ?1 WHERE id = ?2",
                rusqlite::params![r#"["ci:test","review"]"#, kids[1].id],
            )
            .unwrap();
        s.insert_control_handoff(
            &NewHandoff {
                kind: "tree",
                tool: "work_link".into(),
                item_id: Some(parent.id),
                item_ids: kids.iter().map(|k| k.id).collect(),
                ..Default::default()
            },
            1_000,
        )
        .unwrap();
        let row = s.list_control_handoffs(10).unwrap().remove(0);
        assert!(row.items[0].depends_on.is_empty());
        assert!(row.items[0].done_when.is_empty());
        assert_eq!(row.items[1].depends_on, vec![kids[0].id]);
        assert_eq!(row.items[1].done_when, vec!["ci:test", "review"]);
        let json = serde_json::to_value(&row.items[0]).unwrap();
        assert!(json.get("depends_on").is_none() && json.get("done_when").is_none());
    }
}
