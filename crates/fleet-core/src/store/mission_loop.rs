//! What a mission's loop keeps between ticks (orchestration O4–O6, design
//! 2026-10-07 §5.2, §7): its lease and next wake, the cards of its confirm
//! queue, a person's grants, and the counts its brakes read.
//!
//! The loop's state is the store, never a model: a restart loses nothing,
//! because every tick starts from these rows (§5.2 "Obnova po reštarte").

use super::{now_unix, Store};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// How long one tick may hold a mission.
pub const MISSION_LEASE_SECS: i64 = 120;
/// Card kinds: what a planner command asks for (§5.5).
pub const CARD_KINDS: [&str; 10] = [
    "create",
    "add_dep",
    "remove_dep",
    "run",
    "retry",
    "cancel",
    "hold",
    "ask",
    "complete",
    "note",
];
/// Card states: `open` waits for a person (or a grant); the rest are final.
pub const CARD_STATES: [&str; 5] = ["open", "applied", "dismissed", "refused", "stale"];
/// Open cards one mission may hold; a planner run past it is refused.
pub const CARDS_OPEN_CAP: i64 = 40;
/// A card's payload, in bytes.
pub const CARD_PAYLOAD_MAX: usize = 4096;
/// The longest a grant runs (seven days).
pub const GRANT_MAX_SECS: i64 = 7 * 24 * 3600;

/// One card of a mission's confirm queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardRow {
    pub id: i64,
    pub mission_id: i64,
    pub decision_id: String,
    /// `planner` | `loop`.
    pub source: String,
    /// One of [`CARD_KINDS`].
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_item_id: Option<i64>,
    /// The command's own fields (title, notes, role, question, options…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    pub state: String,
    /// Why it was refused or what applying it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
}

/// A new card.
#[derive(Debug, Clone, Default)]
pub struct NewCard<'a> {
    pub decision_id: &'a str,
    pub source: &'a str,
    pub kind: &'a str,
    pub work_item_id: Option<i64>,
    pub payload: Option<serde_json::Value>,
}

/// One grant a person signed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantRow {
    pub id: i64,
    pub mission_id: i64,
    pub plan_version: i64,
    pub level: i64,
    pub granted_by: String,
    /// `None`: any host the mission's owner may drive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hosts: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_micros: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallel: Option<i64>,
    /// The login its runs bill (migration 140): a credential profile on
    /// the run's host; `None` = the host's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<i64>,
}

/// A new grant.
#[derive(Debug, Clone, Default)]
pub struct NewGrant<'a> {
    pub level: i64,
    pub granted_by: &'a str,
    pub hosts: Option<Vec<String>>,
    pub budget_micros: Option<i64>,
    pub max_parallel: Option<i64>,
    pub profile: Option<String>,
    pub expires_at: i64,
}

/// The attempts a mission's members took, counted for its brakes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MissionTaskCounts {
    /// Every attempt (any role, any state).
    pub total: i64,
    /// Queued or running now.
    pub open: i64,
    /// The newest start or finish of any attempt.
    pub last_activity_at: Option<i64>,
}

const CARD_COLUMNS: &str = "id, orchestration_project_id, decision_id, source, kind, \
     work_item_id, payload, state, note, created_at, decided_at, decided_by";

fn map_card(r: &rusqlite::Row<'_>) -> rusqlite::Result<CardRow> {
    Ok(CardRow {
        id: r.get(0)?,
        mission_id: r.get(1)?,
        decision_id: r.get(2)?,
        source: r.get(3)?,
        kind: r.get(4)?,
        work_item_id: r.get(5)?,
        payload: r
            .get::<_, Option<String>>(6)?
            .and_then(|j| serde_json::from_str(&j).ok()),
        state: r.get(7)?,
        note: r.get(8)?,
        created_at: r.get(9)?,
        decided_at: r.get(10)?,
        decided_by: r.get(11)?,
    })
}

const GRANT_COLUMNS: &str = "id, orchestration_project_id, plan_version, level, granted_by, \
     hosts, budget_micros, max_parallel, created_at, expires_at, revoked_at, profile";

fn map_grant(r: &rusqlite::Row<'_>) -> rusqlite::Result<GrantRow> {
    Ok(GrantRow {
        id: r.get(0)?,
        mission_id: r.get(1)?,
        plan_version: r.get(2)?,
        level: r.get(3)?,
        granted_by: r.get(4)?,
        hosts: r
            .get::<_, Option<String>>(5)?
            .and_then(|j| serde_json::from_str(&j).ok()),
        budget_micros: r.get(6)?,
        max_parallel: r.get(7)?,
        created_at: r.get(8)?,
        expires_at: r.get(9)?,
        revoked_at: r.get(10)?,
        profile: r.get(11)?,
    })
}

impl Store {
    /// Ask the loop to look at `mission_id` now. A mission that is not
    /// active is left alone: activating it wakes it.
    pub fn wake_mission(&self, mission_id: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE orchestration_projects SET next_wake_at = ?1 \
             WHERE id = ?2 AND state = 'active' \
               AND (next_wake_at IS NULL OR next_wake_at > ?1)",
            rusqlite::params![now_unix(), mission_id],
        )?;
        Ok(())
    }

    /// Wake the mission `item_id` belongs to, if any (best-effort callers:
    /// a finished run, a decided proposal).
    pub fn wake_item_mission(&self, item_id: i64) -> Result<(), IpcError> {
        if let Some(m) = self.item_mission(item_id)? {
            self.wake_mission(m)?;
        }
        Ok(())
    }

    /// Wake every mission whose member `session_id` works on — as a run's
    /// worker or through a confirmed link (a best-effort caller: its PR's
    /// checks moved).
    pub fn wake_session_missions(&self, session_id: i64) -> Result<usize, IpcError> {
        Ok(wake_session_missions_in_tx(&self.conn, session_id)?)
    }

    /// Whether `session_id` is a worker of a live mission: a run of one of
    /// its members, while the mission is active or paused (the worker
    /// guard's scope, orchestration §7.2).
    pub fn is_mission_worker(&self, session_id: i64) -> Result<bool, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM tasks t \
                 JOIN work_items i ON i.id = t.work_item_id \
                 JOIN orchestration_projects p ON p.id = i.orchestration_project_id \
                 WHERE t.worker_session_id = ?1 AND p.state IN ('active', 'paused') LIMIT 1",
                [session_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// The branch `session_id` is on, as the PR probe last read it, else
    /// its worktree's.
    pub fn session_branch(&self, session_id: i64) -> Result<Option<String>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT COALESCE(s.current_branch, w.branch) FROM sessions s \
                 LEFT JOIN worktrees w ON w.id = s.worktree_id WHERE s.id = ?1",
                [session_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    /// The active missions whose wake is due at `now`, or that have none
    /// yet (just activated, or the store was restarted), oldest wake first.
    pub fn missions_due(&self, now: i64) -> Result<Vec<i64>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM orchestration_projects WHERE state = 'active' \
               AND (next_wake_at IS NULL OR next_wake_at <= ?1) \
               AND (lease_until IS NULL OR lease_until < ?1) \
             ORDER BY COALESCE(next_wake_at, 0), id",
        )?;
        let rows = stmt.query_map([now], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<i64>>>()?)
    }

    /// Take the mission for one tick, until `now + MISSION_LEASE_SECS`.
    /// `false`: another tick (the hub's or a desktop's on the same store)
    /// holds it.
    pub fn take_mission_lease(&self, mission_id: i64, now: i64) -> Result<bool, IpcError> {
        Ok(self.conn.execute(
            "UPDATE orchestration_projects SET lease_until = ?1 \
             WHERE id = ?2 AND (lease_until IS NULL OR lease_until < ?3)",
            rusqlite::params![now + MISSION_LEASE_SECS, mission_id, now],
        )? == 1)
    }

    /// Give the lease back and say when to look next (`None`: only when
    /// something wakes it).
    pub fn release_mission_lease(
        &self,
        mission_id: i64,
        next_wake_at: Option<i64>,
        snapshot_at: Option<i64>,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE orchestration_projects SET lease_until = NULL, next_wake_at = ?1, \
               last_snapshot_at = COALESCE(?2, last_snapshot_at) \
             WHERE id = ?3",
            rusqlite::params![next_wake_at, snapshot_at, mission_id],
        )?;
        Ok(())
    }

    /// The attempts at the mission's members, counted.
    pub fn mission_task_counts(&self, mission_id: i64) -> Result<MissionTaskCounts, IpcError> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*), \
               COALESCE(SUM(t.state IN ('queued','running')), 0), \
               MAX(COALESCE(t.finished_at, t.started_at, t.created_at)) \
             FROM tasks t JOIN work_items i ON i.id = t.work_item_id \
             WHERE i.orchestration_project_id = ?1",
            [mission_id],
            |r| {
                Ok(MissionTaskCounts {
                    total: r.get(0)?,
                    open: r.get(1)?,
                    last_activity_at: r.get(2)?,
                })
            },
        )?)
    }

    /// What the mission has spent, in micro-USD: the usage of every session
    /// that ran an attempt at one of its members, plus its planner's
    /// `claude -p` runs (`aux_usage`, redesign 8.2).
    pub fn mission_cost_micros(&self, mission_id: i64) -> Result<i64, IpcError> {
        Ok(self.conn.query_row(
            "SELECT (SELECT COALESCE(SUM(s.usage_cost_micros), 0) FROM sessions s \
                     WHERE s.id IN (SELECT t.worker_session_id FROM tasks t \
                                    JOIN work_items i ON i.id = t.work_item_id \
                                    WHERE i.orchestration_project_id = ?1)) \
                  + (SELECT COALESCE(SUM(cost_micros), 0) FROM aux_usage \
                     WHERE mission_id = ?1)",
            [mission_id],
            |r| r.get(0),
        )?)
    }

    /// Planner calls logged since `since` (events of kind `planned`).
    pub fn mission_planner_runs_since(&self, mission_id: i64, since: i64) -> Result<i64, IpcError> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM orchestration_events \
             WHERE orchestration_project_id = ?1 AND kind = 'planned' AND at >= ?2",
            rusqlite::params![mission_id, since],
            |r| r.get(0),
        )?)
    }

    /// Add a card. A `decision_id` already carded answers `Ok(None)`: the
    /// planner's command becomes one card however often it repeats it.
    pub fn add_card(&self, mission_id: i64, c: &NewCard<'_>) -> Result<Option<CardRow>, IpcError> {
        if !CARD_KINDS.contains(&c.kind) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("card kind {:?}; one of {}", c.kind, CARD_KINDS.join(", ")),
            ));
        }
        let open: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM orchestration_cards \
             WHERE orchestration_project_id = ?1 AND state = 'open'",
            [mission_id],
            |r| r.get(0),
        )?;
        if open >= CARDS_OPEN_CAP {
            return Err(IpcError::new(
                codes::E_LIMIT,
                format!("the mission holds {CARDS_OPEN_CAP} open cards; decide some first"),
            ));
        }
        let payload = match &c.payload {
            Some(p) => {
                let t = p.to_string();
                if t.len() > CARD_PAYLOAD_MAX {
                    return Err(IpcError::new(
                        codes::E_LIMIT,
                        format!("a card's payload holds at most {CARD_PAYLOAD_MAX} bytes"),
                    ));
                }
                Some(t)
            }
            None => None,
        };
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO orchestration_cards (orchestration_project_id, decision_id, \
               source, kind, work_item_id, payload, state, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'open', ?7)",
            rusqlite::params![
                mission_id,
                c.decision_id,
                c.source,
                c.kind,
                c.work_item_id,
                payload,
                now_unix()
            ],
        )?;
        if n == 0 {
            return Ok(None);
        }
        self.get_card(self.conn.last_insert_rowid())
    }

    pub fn get_card(&self, id: i64) -> Result<Option<CardRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {CARD_COLUMNS} FROM orchestration_cards WHERE id = ?1"),
                [id],
                map_card,
            )
            .optional()?)
    }

    /// A mission's cards: the open ones, then the newest decided, at most
    /// `limit` in all.
    pub fn mission_cards(&self, mission_id: i64, limit: i64) -> Result<Vec<CardRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {CARD_COLUMNS} FROM orchestration_cards \
             WHERE orchestration_project_id = ?1 \
             ORDER BY state = 'open' DESC, id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(rusqlite::params![mission_id, limit], map_card)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Close an open card: `applied`, `dismissed`, `refused` or `stale`.
    /// `Ok(false)`: it was not open (decided meanwhile), and nothing changed.
    pub fn decide_card(
        &self,
        id: i64,
        state: &str,
        by: &str,
        note: Option<&str>,
    ) -> Result<bool, IpcError> {
        if state == "open" || !CARD_STATES.contains(&state) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("a card is closed as applied, dismissed, refused or stale, not {state}"),
            ));
        }
        let note: Option<String> = note.map(|n| n.chars().take(500).collect());
        Ok(self.conn.execute(
            "UPDATE orchestration_cards SET state = ?1, decided_by = ?2, decided_at = ?3, \
               note = COALESCE(?4, note) \
             WHERE id = ?5 AND state = 'open'",
            rusqlite::params![state, by, now_unix(), note, id],
        )? == 1)
    }

    /// Sign a grant for the mission's current plan.
    pub fn add_grant(&self, mission_id: i64, g: &NewGrant<'_>) -> Result<GrantRow, IpcError> {
        let plan_version: i64 = self
            .conn
            .query_row(
                "SELECT plan_version FROM orchestration_projects WHERE id = ?1",
                [mission_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("mission {mission_id}")))?;
        let hosts = g
            .hosts
            .as_ref()
            .map(|h| serde_json::to_string(h).unwrap_or_else(|_| "[]".into()));
        self.conn.execute(
            "INSERT INTO orchestration_grants (orchestration_project_id, plan_version, level, \
               granted_by, hosts, budget_micros, max_parallel, created_at, expires_at, profile) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                mission_id,
                plan_version,
                g.level,
                g.granted_by,
                hosts,
                g.budget_micros,
                g.max_parallel,
                now_unix(),
                g.expires_at,
                g.profile
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.conn
            .query_row(
                &format!("SELECT {GRANT_COLUMNS} FROM orchestration_grants WHERE id = ?1"),
                [id],
                map_grant,
            )
            .map_err(Into::into)
    }

    /// The newest grant that still holds at `now`: not revoked, not
    /// expired, and signed for the plan the mission has now.
    pub fn live_mission_grant(
        &self,
        mission_id: i64,
        now: i64,
    ) -> Result<Option<GrantRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {GRANT_COLUMNS} FROM orchestration_grants g \
                     WHERE orchestration_project_id = ?1 AND revoked_at IS NULL \
                       AND expires_at > ?2 \
                       AND plan_version = (SELECT plan_version FROM orchestration_projects \
                                           WHERE id = g.orchestration_project_id) \
                     ORDER BY id DESC LIMIT 1"
                ),
                rusqlite::params![mission_id, now],
                map_grant,
            )
            .optional()?)
    }

    /// Revoke every live grant of the mission. Answers how many.
    pub fn revoke_grants(&self, mission_id: i64) -> Result<usize, IpcError> {
        Ok(self.conn.execute(
            "UPDATE orchestration_grants SET revoked_at = ?1 \
             WHERE orchestration_project_id = ?2 AND revoked_at IS NULL",
            rusqlite::params![now_unix(), mission_id],
        )?)
    }

    /// Bump the mission's plan version: a change to what it may do (a repo
    /// added, the policy raised) voids the grants signed for the old plan.
    pub fn bump_plan_version(&self, mission_id: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE orchestration_projects SET plan_version = plan_version + 1 WHERE id = ?1",
            [mission_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;

/// [`Store::wake_session_missions`] on a caller's connection — reconcile's
/// transaction, where a session's `ci_status` is seen to move.
pub(super) fn wake_session_missions_in_tx(
    conn: &rusqlite::Connection,
    session_id: i64,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE orchestration_projects SET next_wake_at = ?1 \
         WHERE state = 'active' AND (next_wake_at IS NULL OR next_wake_at > ?1) \
           AND id IN ( \
             SELECT i.orchestration_project_id FROM work_items i \
             WHERE i.orchestration_project_id IS NOT NULL AND i.id IN ( \
               SELECT work_item_id FROM tasks WHERE worker_session_id = ?2 \
               UNION SELECT l.item_id FROM work_links l \
                 JOIN participants p ON p.id = l.participant_id \
                 WHERE p.session_id = ?2 AND l.state = 'confirmed' AND l.ended_at IS NULL))",
        rusqlite::params![now_unix(), session_id],
    )
}
