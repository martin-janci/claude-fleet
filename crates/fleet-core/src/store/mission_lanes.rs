//! A mission's lanes (lanes design 2026-10-23 §2, L2, migration 208): an
//! ordered set of its member tasks in one repository that one lead session
//! works through on one branch (L3). A lane is named once per mission and
//! made the first time a plan, the planner or a person puts a task in it.
//!
//! A task's lane is `work_items.lane_id`, read here and never on
//! [`super::WorkItemRow`]: the lane is the mission's, not the task's. A
//! lane left with no task and no lead session is removed. A change is
//! logged in the mission's event log (`lane_set`).

use super::orchestration::{refuse_final, NewMissionEvent};
use super::{now_unix, Store};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// The longest lane name.
pub const LANE_NAME_MAX_CHARS: usize = 40;

/// The stored lane states; `working` and `waiting` are derived on read.
pub const LANE_STATES: [&str; 3] = ["idle", "held", "done"];

/// One lane, with its tasks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionLaneRow {
    pub id: i64,
    pub mission_id: i64,
    pub name: String,
    /// Its repository; `None` until one is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
    /// The lead session's branch and checkout (L3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_session_id: Option<i64>,
    /// How many lead sessions it has had (L3's relay).
    #[serde(default)]
    pub relay: i64,
    /// One of [`LANE_STATES`].
    pub state: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// Its member tasks, by id.
    #[serde(default)]
    pub item_ids: Vec<i64>,
}

/// PURE: a lane name as stored: trimmed, one line of at most
/// [`LANE_NAME_MAX_CHARS`] characters. `Err` names what is wrong.
pub fn lane_name(raw: &str) -> Result<String, IpcError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(IpcError::new(codes::E_INVALID, "a lane needs a name"));
    }
    if name.chars().count() > LANE_NAME_MAX_CHARS || name.chars().any(char::is_control) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("a lane's name is one line of at most {LANE_NAME_MAX_CHARS} characters"),
        ));
    }
    Ok(name.to_string())
}

const LANE_COLUMNS: &str = "id, orchestration_project_id, name, project_id, branch, \
     worktree_id, lead_session_id, relay, state, created_at, updated_at";

fn map_lane(r: &rusqlite::Row<'_>) -> rusqlite::Result<MissionLaneRow> {
    Ok(MissionLaneRow {
        id: r.get(0)?,
        mission_id: r.get(1)?,
        name: r.get(2)?,
        project_id: r.get(3)?,
        branch: r.get(4)?,
        worktree_id: r.get(5)?,
        lead_session_id: r.get(6)?,
        relay: r.get(7)?,
        state: r.get(8)?,
        created_at: r.get(9)?,
        updated_at: r.get(10)?,
        item_ids: Vec::new(),
    })
}

impl Store {
    /// Mission `id`'s lanes in the order they were made, each with its
    /// member tasks in id order.
    pub fn mission_lanes(&self, id: i64) -> Result<Vec<MissionLaneRow>, IpcError> {
        let mut lanes: Vec<MissionLaneRow> = {
            let mut stmt = self.conn.prepare_cached(&format!(
                "SELECT {LANE_COLUMNS} FROM mission_lanes \
                 WHERE orchestration_project_id = ?1 ORDER BY id"
            ))?;
            let rows = stmt.query_map([id], map_lane)?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT lane_id, id FROM work_items \
             WHERE orchestration_project_id = ?1 AND lane_id IS NOT NULL ORDER BY id",
        )?;
        let pairs = stmt.query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
        for p in pairs {
            let (lane, item) = p?;
            if let Some(l) = lanes.iter_mut().find(|l| l.id == lane) {
                l.item_ids.push(item);
            }
        }
        Ok(lanes)
    }

    /// One lane, with its tasks.
    pub fn get_mission_lane(&self, id: i64) -> Result<Option<MissionLaneRow>, IpcError> {
        let lane = self
            .conn
            .query_row(
                &format!("SELECT {LANE_COLUMNS} FROM mission_lanes WHERE id = ?1"),
                [id],
                map_lane,
            )
            .optional()?;
        let Some(mut lane) = lane else {
            return Ok(None);
        };
        let mut stmt = self
            .conn
            .prepare_cached("SELECT id FROM work_items WHERE lane_id = ?1 ORDER BY id")?;
        lane.item_ids = stmt
            .query_map([id], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Some(lane))
    }

    /// Record a new lead session of lane `id` (lanes design L3): its
    /// session, its checkout and its branch; the relay count goes up by one.
    pub fn set_lane_lead(
        &self,
        id: i64,
        session_id: i64,
        worktree_id: Option<i64>,
        branch: &str,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE mission_lanes SET lead_session_id = ?1, \
               worktree_id = COALESCE(?2, worktree_id), branch = ?3, relay = relay + 1, \
               updated_at = ?4 WHERE id = ?5",
            rusqlite::params![session_id, worktree_id, branch, now_unix(), id],
        )?;
        Ok(())
    }

    /// The implement runs session `lead` was handed, open or not: how far a
    /// lead is into its share of a lane before it hands over (§3.5).
    pub fn lead_run_count(&self, lead: i64) -> Result<i64, IpcError> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM tasks WHERE worker_session_id = ?1 \
               AND work_item_id IS NOT NULL AND COALESCE(role, 'implement') = 'implement'",
            [lead],
            |r| r.get(0),
        )?)
    }

    /// The lane of `item_id`, if it is in one.
    pub fn item_lane(&self, item_id: i64) -> Result<Option<i64>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT lane_id FROM work_items WHERE id = ?1",
                [item_id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten())
    }

    /// Put member `item_id` of mission `id` in the lane named `lane`, made
    /// on first use with `project_id` as its repository (a lane that has
    /// none yet takes it); `None` takes it out of its lane. `Ok(false)`:
    /// it was already there. A lane left empty with no lead is removed.
    pub fn set_item_lane(
        &self,
        id: i64,
        item_id: i64,
        lane: Option<&str>,
        project_id: Option<i64>,
        actor: &str,
    ) -> Result<bool, IpcError> {
        let m = self
            .get_mission(id)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("mission {id} not found")))?;
        refuse_final(&m)?;
        if self.item_mission(item_id)? != Some(id) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("work item {item_id} is not a task of {}", m.name),
            ));
        }
        if m.root_item_id == Some(item_id) && lane.is_some() {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "work item {item_id} is {}'s root, which is in no lane",
                    m.name
                ),
            ));
        }
        let name = lane.map(lane_name).transpose()?;
        let before = self.item_lane(item_id)?;
        let tx = self.conn.unchecked_transaction()?;
        let now = now_unix();
        let after = match &name {
            Some(n) => {
                tx.execute(
                    "INSERT INTO mission_lanes \
                       (orchestration_project_id, name, project_id, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?4) \
                     ON CONFLICT(orchestration_project_id, name) DO UPDATE SET \
                       project_id = COALESCE(mission_lanes.project_id, excluded.project_id)",
                    rusqlite::params![id, n, project_id, now],
                )?;
                Some(tx.query_row(
                    "SELECT id FROM mission_lanes \
                     WHERE orchestration_project_id = ?1 AND name = ?2",
                    rusqlite::params![id, n],
                    |r| r.get::<_, i64>(0),
                )?)
            }
            None => None,
        };
        if after == before {
            tx.commit()?;
            return Ok(false);
        }
        tx.execute(
            "UPDATE work_items SET lane_id = ?1 WHERE id = ?2",
            rusqlite::params![after, item_id],
        )?;
        if let Some(old) = before {
            tx.execute(
                "DELETE FROM mission_lanes WHERE id = ?1 AND lead_session_id IS NULL \
                   AND NOT EXISTS (SELECT 1 FROM work_items WHERE lane_id = ?1)",
                [old],
            )?;
        }
        self.touch_mission(id)?;
        self.append_mission_event(
            id,
            &NewMissionEvent {
                kind: "lane_set",
                actor,
                work_item_id: Some(item_id),
                payload: Some(serde_json::json!({ "lane": name })),
                ..Default::default()
            },
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Take `item_id` out of its lane as it leaves its mission, removing a
    /// lane left empty with no lead. Inside the caller's transaction.
    pub(super) fn clear_item_lane(&self, item_id: i64) -> Result<(), IpcError> {
        let Some(old) = self.item_lane(item_id)? else {
            return Ok(());
        };
        self.conn.execute(
            "UPDATE work_items SET lane_id = NULL WHERE id = ?1",
            [item_id],
        )?;
        self.conn.execute(
            "DELETE FROM mission_lanes WHERE id = ?1 AND lead_session_id IS NULL \
               AND NOT EXISTS (SELECT 1 FROM work_items WHERE lane_id = ?1)",
            [old],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::NewMission;

    /// A mission with two member tasks: (store, mission, a, b).
    fn mission() -> (Store, i64, i64, i64) {
        let s = Store::open_in_memory().unwrap();
        let root = s
            .create_native_item(&crate::store::NativeItem {
                title: "Goal",
                parent_id: None,
                project_id: None,
                notes: None,
            })
            .unwrap();
        let m = s
            .create_mission(
                &NewMission {
                    name: "m",
                    goal: "g",
                    root_item_id: Some(root.id),
                    ..Default::default()
                },
                "t",
            )
            .unwrap();
        let mut ids = Vec::new();
        for t in ["a", "b"] {
            let it = s
                .create_native_item(&crate::store::NativeItem {
                    title: t,
                    parent_id: Some(root.id),
                    project_id: None,
                    notes: None,
                })
                .unwrap();
            s.set_mission_item(m.id, it.id, true, "t").unwrap();
            ids.push(it.id);
        }
        (s, m.id, ids[0], ids[1])
    }

    #[test]
    fn a_lane_is_made_on_first_use_and_holds_its_tasks() {
        let (s, m, a, b) = mission();
        let pid = s.upsert_project("o", "r", "/r").unwrap();
        assert!(s
            .set_item_lane(m, a, Some(" backend "), Some(pid), "t")
            .unwrap());
        assert!(s.set_item_lane(m, b, Some("backend"), None, "t").unwrap());
        assert!(
            !s.set_item_lane(m, b, Some("backend"), None, "t").unwrap(),
            "already there"
        );
        let lanes = s.mission_lanes(m).unwrap();
        assert_eq!(lanes.len(), 1);
        assert_eq!(lanes[0].name, "backend");
        assert_eq!(lanes[0].project_id, Some(pid));
        assert_eq!(lanes[0].state, "idle");
        assert_eq!(lanes[0].item_ids, vec![a, b]);
        let ev = &s.mission_events(m, None, 1).unwrap()[0];
        assert_eq!(ev.kind, "lane_set");
    }

    #[test]
    fn a_lane_left_empty_is_removed() {
        let (s, m, a, b) = mission();
        s.set_item_lane(m, a, Some("one"), None, "t").unwrap();
        s.set_item_lane(m, b, Some("two"), None, "t").unwrap();
        s.set_item_lane(m, a, Some("two"), None, "t").unwrap();
        let lanes = s.mission_lanes(m).unwrap();
        assert_eq!(
            lanes.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
            vec!["two"]
        );
        s.set_item_lane(m, a, None, None, "t").unwrap();
        assert_eq!(s.item_lane(a).unwrap(), None);
        // A task leaving its mission leaves its lane too.
        s.set_mission_item(m, b, false, "t").unwrap();
        assert_eq!(s.item_lane(b).unwrap(), None);
        assert!(s.mission_lanes(m).unwrap().is_empty());
    }

    #[test]
    fn only_a_missions_own_tasks_go_in_its_lanes_and_names_are_one_line() {
        let (s, m, a, _) = mission();
        let root = s.get_mission(m).unwrap().unwrap().root_item_id.unwrap();
        assert!(
            s.set_item_lane(m, root, Some("x"), None, "t").is_err(),
            "the root"
        );
        let outsider = s
            .create_native_item(&crate::store::NativeItem {
                title: "elsewhere",
                parent_id: None,
                project_id: None,
                notes: None,
            })
            .unwrap();
        assert!(s
            .set_item_lane(m, outsider.id, Some("x"), None, "t")
            .is_err());
        assert!(s.set_item_lane(m, a, Some("a\nb"), None, "t").is_err());
        assert!(s
            .set_item_lane(m, a, Some(&"x".repeat(41)), None, "t")
            .is_err());
        assert!(s.set_item_lane(m, a, Some("  "), None, "t").is_err());
    }
}
