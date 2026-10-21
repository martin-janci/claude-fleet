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
    /// How the call was let through (migration 196): `person` — a person
    /// approved its card and fleet ran it (review 2026-10-17, F1);
    /// `pre_approved` — the autonomy position let it through without a card
    /// (F2). Absent when no card was needed, or on an older receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<String>,
}

/// [`ControlHandoffRow::approval`]: a person approved the card (F1).
pub const APPROVAL_PERSON: &str = "person";
/// [`ControlHandoffRow::approval`]: the autonomy position let it through (F2).
pub const APPROVAL_PRE_APPROVED: &str = "pre_approved";

/// One timeline event of a session Control answers for, with the names the
/// operator feed's digest shows ([`Store::operator_feed_events`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedEventRow {
    pub id: i64,
    pub session_id: i64,
    pub at: i64,
    pub kind: String,
    pub detail: Option<String>,
    pub tmux_name: String,
    pub host_alias: String,
    /// Control started it (`origin = 'operator'`), rather than only handed
    /// it work.
    pub started_by_control: bool,
    /// The task it works on now: its live primary link's key, so the digest
    /// can say which task a finished turn or a failure belongs to.
    pub task_key: Option<String>,
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
        self.insert_control_handoff_as(h, now, None)
    }

    /// [`Self::insert_control_handoff`], saying how the call was let through
    /// ([`APPROVAL_PERSON`] / [`APPROVAL_PRE_APPROVED`]).
    pub fn insert_control_handoff_as(
        &self,
        h: &NewHandoff,
        now: i64,
        approval: Option<&str>,
    ) -> Result<i64> {
        let ids = if h.item_ids.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&h.item_ids).unwrap_or_else(|_| "[]".into()))
        };
        self.conn
            .prepare_cached(
                "INSERT INTO control_handoffs \
                 (at, kind, tool, session_id, task_id, mission_id, item_id, item_ids, preview, \
                  approval) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
                approval,
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
                    m.name, m.state, h.item_id, h.item_ids, h.preview, h.approval \
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
                        approval: r.get(12)?,
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

    /// Timeline events of the sessions Control answers for, strictly after
    /// `after_id`, OLDEST first by id: the operator feed's cursor read
    /// (`service::operator_feed`). A session counts when Control started it
    /// (`origin = 'operator'`) or handed it work (a `control_handoffs`
    /// receipt names it); `exclude` (the operator's own row) never does, so
    /// the feed cannot report Control to itself. Only `kinds` are read, and
    /// paging by id is what keeps a `limit` from skipping a row, as
    /// [`Store::session_events_after`] explains.
    pub fn operator_feed_events(
        &self,
        after_id: i64,
        exclude: Option<i64>,
        kinds: &[&str],
        limit: i64,
    ) -> Result<Vec<FeedEventRow>> {
        if kinds.is_empty() {
            return Ok(Vec::new());
        }
        // Numbered from ?4: a bare `?` would take the number after the largest
        // seen so far (?2 here), colliding with the ?3 that follows.
        let marks = (4..4 + kinds.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT e.id, e.session_id, e.at, e.kind, e.detail, s.tmux_name, s.host_alias, \
                    s.origin = 'operator', \
                    (SELECT COALESCE(i.key, l.ref_key) FROM work_links l \
                       JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
                       LEFT JOIN work_items i ON i.id = l.item_id \
                      WHERE p.session_id = s.id AND l.ended_at IS NULL \
                        AND l.state = 'confirmed' \
                      ORDER BY l.is_primary DESC, l.id DESC LIMIT 1) \
             FROM session_events e JOIN sessions s ON s.id = e.session_id \
             WHERE e.id > ?1 AND e.session_id IS NOT ?2 AND e.kind IN ({marks}) \
               AND (s.origin = 'operator' OR e.session_id IN \
                    (SELECT session_id FROM control_handoffs WHERE session_id IS NOT NULL)) \
             ORDER BY e.id ASC LIMIT ?3"
        );
        let mut params: Vec<rusqlite::types::Value> =
            vec![after_id.into(), exclude.into(), limit.into()];
        params.extend(
            kinds
                .iter()
                .map(|k| rusqlite::types::Value::from(k.to_string())),
        );
        let mut st = self.conn.prepare_cached(&sql)?;
        let rows = st
            .query_map(rusqlite::params_from_iter(params), |r| {
                Ok(FeedEventRow {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    at: r.get(2)?,
                    kind: r.get(3)?,
                    detail: r.get(4)?,
                    tmux_name: r.get(5)?,
                    host_alias: r.get(6)?,
                    started_by_control: r.get::<_, Option<bool>>(7)?.unwrap_or(false),
                    task_key: r.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The newest timeline event id fleet-wide (`0` for none): where the
    /// operator feed's cursor starts, so turning it on does not replay the
    /// past.
    pub fn newest_session_event_id(&self) -> Result<i64> {
        self.conn
            .prepare_cached("SELECT COALESCE(MAX(id), 0) FROM session_events")?
            .query_row([], |r| r.get(0))
    }

    /// Whether `session_id` is one of Control's OPEN handoffs (review
    /// 2026-10-17, F2): a live session (`running`, not lost) that Control
    /// started (`origin = 'operator'`) or briefed (a receipt names it). The
    /// operator guard pre-approves a prompt or task to such a session at
    /// "Within your brief" and "Autopilot" (`mcp::guard::operator_must_confirm`)
    /// when Control also still follows it (`service::control_follow::
    /// session_followed`, review w04-4).
    pub fn is_open_control_handoff(&self, session_id: i64) -> Result<bool> {
        self.conn
            .prepare_cached(
                "SELECT EXISTS (SELECT 1 FROM sessions s \
                  WHERE s.id = ?1 AND s.status = 'running' AND s.lost_at IS NULL \
                    AND (s.origin = 'operator' OR EXISTS \
                         (SELECT 1 FROM control_handoffs h WHERE h.session_id = s.id)))",
            )?
            .query_row([session_id], |r| r.get(0))
    }

    /// Whether `session_id` is a live session Control itself started
    /// (`origin = 'operator'`): what session care may restart or retire
    /// without a card. A session Control only briefed (a `send_prompt`
    /// receipt) is an open handoff for prompts, but it is still the
    /// person's, so care never retires it.
    pub fn is_control_started_session(&self, session_id: i64) -> Result<bool> {
        self.conn
            .prepare_cached(
                "SELECT EXISTS (SELECT 1 FROM sessions s \
                  WHERE s.id = ?1 AND s.status = 'running' AND s.lost_at IS NULL \
                    AND s.origin = 'operator')",
            )?
            .query_row([session_id], |r| r.get(0))
    }

    /// The missions Control handed off (a receipt names them), newest
    /// receipt first, without repeats.
    pub fn handed_off_mission_ids(&self) -> Result<Vec<i64>> {
        self.conn
            .prepare_cached(
                "SELECT mission_id FROM control_handoffs WHERE mission_id IS NOT NULL \
                 GROUP BY mission_id ORDER BY MAX(id) DESC",
            )?
            .query_map([], |r| r.get(0))?
            .collect()
    }

    /// The sessions Control answers for, as [`Store::operator_feed_events`]
    /// counts them (started by it, or named by a receipt), live ones only
    /// and never `exclude` (the operator's own row): what
    /// `service::control_sweep` looks at.
    pub fn handed_off_session_ids(&self, exclude: Option<i64>) -> Result<Vec<i64>> {
        self.conn
            .prepare_cached(
                "SELECT s.id FROM sessions s \
                 WHERE s.id IS NOT ?1 AND s.status = 'running' \
                   AND (s.origin = 'operator' OR s.id IN \
                        (SELECT session_id FROM control_handoffs WHERE session_id IS NOT NULL)) \
                 ORDER BY s.id",
            )?
            .query_map([exclude], |r| r.get(0))?
            .collect()
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

    /// F1/F2: how a call was let through reads back, and a receipt from
    /// before says nothing.
    #[test]
    fn a_receipt_keeps_how_it_was_approved() {
        let s = Store::open_in_memory().unwrap();
        s.insert_control_handoff(&NewHandoff::default(), 1).unwrap();
        s.insert_control_handoff_as(&NewHandoff::default(), 2, Some(APPROVAL_PRE_APPROVED))
            .unwrap();
        s.insert_control_handoff_as(&NewHandoff::default(), 3, Some(APPROVAL_PERSON))
            .unwrap();
        let rows = s.list_control_handoffs(10).unwrap();
        let got: Vec<Option<&str>> = rows.iter().map(|r| r.approval.as_deref()).collect();
        assert_eq!(got, vec![Some("person"), Some("pre_approved"), None]);
        assert!(s
            .insert_control_handoff_as(&NewHandoff::default(), 4, Some("whatever"))
            .is_err());
    }

    /// F2: an open handoff is a live session Control started or briefed.
    #[test]
    fn an_open_handoff_is_a_live_session_control_started_or_briefed() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let mk = |name: &str| {
            s.upsert_session(name, "h", None, None, 1, 1, "running", None)
                .unwrap()
        };
        let started = mk("started");
        let briefed = mk("briefed");
        let stranger = mk("stranger");
        let gone = mk("gone");
        s.conn
            .execute(
                "UPDATE sessions SET origin = 'operator' WHERE id IN (?1, ?2)",
                [started, gone],
            )
            .unwrap();
        s.insert_control_handoff(
            &NewHandoff {
                session_id: Some(briefed),
                ..Default::default()
            },
            1,
        )
        .unwrap();
        s.conn
            .execute(
                "UPDATE sessions SET status = 'killed' WHERE id = ?1",
                [gone],
            )
            .unwrap();
        assert!(s.is_open_control_handoff(started).unwrap());
        assert!(s.is_open_control_handoff(briefed).unwrap());
        assert!(!s.is_open_control_handoff(stranger).unwrap());
        assert!(
            !s.is_open_control_handoff(gone).unwrap(),
            "a dead session is closed"
        );
        assert!(!s.is_open_control_handoff(9_999).unwrap());
        // Session care's narrower set: started, not merely briefed.
        assert!(s.is_control_started_session(started).unwrap());
        assert!(!s.is_control_started_session(briefed).unwrap());
        assert!(!s.is_control_started_session(stranger).unwrap());
        assert!(!s.is_control_started_session(gone).unwrap());
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
