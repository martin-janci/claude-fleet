//! Missions (orchestration O1, design 2026-10-07 §4.1, §4.2, §4.6):
//! `orchestration_projects`, the repos each may run in, its member items
//! and its event log.
//!
//! * **The lifecycle is stored, the loop's phase is not** (§4.5): `state`
//!   moves only along [`MISSION_TRANSITIONS`], and each move is an event.
//! * **Membership is a flat column** (`work_items.orchestration_project_id`):
//!   an item belongs to at most one mission, and the refusal names the one
//!   that holds it. A mission holds at most [`MISSION_ITEM_CAP`] items
//!   (decision O10); a continuous one counts only the open ones.
//! * **The event log is capped** at [`MISSION_EVENT_CAP`] rows per mission;
//!   the oldest fold into one `digest` row that counts them.
//!
//! Visibility is the caller's: these methods know no scope, and
//! `service::work::missions` fences every read and write by the mission's
//! org and owner.

use super::work::{map_item, ITEM_COLUMNS};
use super::{now_unix, Store, WorkItemRow};
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// A mission's name, in characters.
pub const MISSION_NAME_MAX_CHARS: usize = 120;
/// A mission's goal and non-goals, in characters each.
pub const MISSION_TEXT_MAX_CHARS: usize = 4000;
/// `done_when` rows, and characters per row.
pub const MISSION_DONE_WHEN_MAX: usize = 20;
pub const MISSION_DONE_WHEN_ROW_MAX_CHARS: usize = 500;
/// A repo's role label, in characters.
pub const MISSION_ROLE_MAX_CHARS: usize = 40;
/// Member items per mission (decision O10).
pub const MISSION_ITEM_CAP: i64 = 30;
/// Member items of a `plan` mission, which the loop never runs: a plan
/// people and their own agents work through, tracked here as a graph.
pub const PLAN_MISSION_ITEM_CAP: i64 = 200;
/// Event rows kept per mission before the oldest fold into a digest.
pub const MISSION_EVENT_CAP: i64 = 5000;
/// An event's JSON payload, in bytes.
pub const MISSION_EVENT_PAYLOAD_MAX: usize = 4096;
/// The autonomy a mission may ask for: L0..=L3.
pub const MISSION_LEVEL_MAX: i64 = 3;

/// `finite` ends when its done_when holds; `continuous` keeps running (O8);
/// `plan` tracks a plan the loop never runs (no steps, cards, planner or
/// grant), so it may hold [`PLAN_MISSION_ITEM_CAP`] items.
pub const MISSION_MODES: [&str; 3] = ["finite", "continuous", "plan"];

/// Whether the loop (orchestration O4–O8) runs `mode` at all.
pub fn mode_runs_loop(mode: &str) -> bool {
    mode != "plan"
}

/// How many members a mission in `mode` may hold.
pub fn mission_item_cap(mode: &str) -> i64 {
    if mode_runs_loop(mode) {
        MISSION_ITEM_CAP
    } else {
        PLAN_MISSION_ITEM_CAP
    }
}
/// The stored lifecycle, in order.
pub const MISSION_STATES: [&str; 6] = [
    "draft",
    "active",
    "paused",
    "completed",
    "failed",
    "cancelled",
];
/// The states that end a mission. Nothing changes a finished mission; the
/// one way out is a person's Reopen, back to `paused` (Orbit Fleet G3.7).
pub const MISSION_FINAL_STATES: [&str; 3] = ["completed", "failed", "cancelled"];
/// Every move the lifecycle allows, `(from, to)`.
pub const MISSION_TRANSITIONS: &[(&str, &str)] = &[
    ("draft", "active"),
    ("draft", "cancelled"),
    ("active", "paused"),
    ("paused", "active"),
    ("active", "completed"),
    ("active", "failed"),
    ("active", "cancelled"),
    ("paused", "completed"),
    ("paused", "failed"),
    ("paused", "cancelled"),
    // Reopen (G3.7): a finished mission comes back paused, so its loop
    // takes nothing until a person resumes it.
    ("completed", "paused"),
    ("failed", "paused"),
    ("cancelled", "paused"),
];

/// What a mission asks of its loop (`orchestration_projects.policy_json`,
/// design §7.1): the limits it runs within. It is what the mission ASKS for;
/// what applies is capped by a person's grant (O6) and the fleet-wide
/// ceiling (`orchestrator.max_level`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MissionPolicy {
    /// Runs open at once.
    pub max_parallel: u32,
    /// Retries of one item after its first attempt.
    pub max_retries: u32,
    /// Every implemented item gets a review run before it closes.
    pub require_review: bool,
    /// `propose`: the planner's new items are proposals a person accepts;
    /// `auto`: at L3 under a grant they are created as tasks.
    pub task_creation: String,
    /// Attempts (runs of any role) the whole mission may take.
    pub max_tasks: u32,
    /// Planner calls per hour, a brake on replan → fail → replan.
    pub max_planner_runs_per_hour: u32,
    /// With runs open and nothing finishing for this long, the mission
    /// pauses and asks a person.
    pub no_progress_secs: u64,
    /// The planner's model (`claude -p --model`); empty for the default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub planner_model: Option<String>,
    /// The host the planner runs on; empty: the root's or a member's host.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub planner_host: Option<String>,
    /// A continuous mission's timer (O8): wake at least this often.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake_every_secs: Option<u64>,
}

impl Default for MissionPolicy {
    fn default() -> Self {
        MissionPolicy {
            max_parallel: 2,
            max_retries: 1,
            require_review: false,
            task_creation: "propose".into(),
            max_tasks: 60,
            max_planner_runs_per_hour: 6,
            no_progress_secs: 3600,
            planner_model: None,
            planner_host: None,
            wake_every_secs: None,
        }
    }
}

/// The policy's bounds.
pub const POLICY_MAX_PARALLEL: u32 = 8;
pub const POLICY_MAX_RETRIES: u32 = 5;
pub const POLICY_MAX_TASKS: u32 = 300;
pub const POLICY_MAX_PLANNER_RUNS: u32 = 30;
/// A continuous mission's timer is at least this.
pub const POLICY_MIN_WAKE_SECS: u64 = 300;

/// PURE: a policy within its bounds, or `E_INVALID` naming the field.
pub fn check_policy(p: &MissionPolicy) -> Result<MissionPolicy, IpcError> {
    let bad = |f: &str, why: String| invalid(format!("policy.{f}: {why}"));
    if !(1..=POLICY_MAX_PARALLEL).contains(&p.max_parallel) {
        return Err(bad("max_parallel", format!("1 to {POLICY_MAX_PARALLEL}")));
    }
    if p.max_retries > POLICY_MAX_RETRIES {
        return Err(bad("max_retries", format!("0 to {POLICY_MAX_RETRIES}")));
    }
    if !(1..=POLICY_MAX_TASKS).contains(&p.max_tasks) {
        return Err(bad("max_tasks", format!("1 to {POLICY_MAX_TASKS}")));
    }
    if p.max_planner_runs_per_hour > POLICY_MAX_PLANNER_RUNS {
        return Err(bad(
            "max_planner_runs_per_hour",
            format!("0 to {POLICY_MAX_PLANNER_RUNS}"),
        ));
    }
    if p.no_progress_secs < 300 {
        return Err(bad("no_progress_secs", "at least 300".into()));
    }
    if !matches!(p.task_creation.as_str(), "propose" | "auto") {
        return Err(bad("task_creation", "propose or auto".into()));
    }
    if p.wake_every_secs.is_some_and(|w| w < POLICY_MIN_WAKE_SECS) {
        return Err(bad(
            "wake_every_secs",
            format!("at least {POLICY_MIN_WAKE_SECS}"),
        ));
    }
    let short = |v: &Option<String>, f: &str| -> Result<Option<String>, IpcError> {
        match v.as_deref().map(str::trim).filter(|x| !x.is_empty()) {
            None => Ok(None),
            Some(x)
                if x.len() <= 80
                    && x.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-._@:".contains(c)) =>
            {
                Ok(Some(x.to_string()))
            }
            Some(_) => Err(bad(f, "a short name of letters, digits and -._@:".into())),
        }
    };
    Ok(MissionPolicy {
        planner_model: short(&p.planner_model, "planner_model")?,
        planner_host: short(&p.planner_host, "planner_host")?,
        ..p.clone()
    })
}

/// One mission, with its members' roll-up and its repos.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionRow {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    /// Who owns it and signs its grants (O6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_person_id: Option<i64>,
    /// The epic or ticket the mission is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_item_id: Option<i64>,
    pub name: String,
    pub goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub non_goals: Option<String>,
    /// One condition per row; typed in O3.
    #[serde(default)]
    pub done_when: Vec<String>,
    /// finite | continuous
    pub mode: String,
    /// draft | active | paused | completed | failed | cancelled
    pub state: String,
    /// The autonomy asked for, 0..=3. What applies is what a grant covers.
    pub level: i64,
    pub plan_version: i64,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    /// The `expected_version` guard.
    pub version: i64,
    /// Member items.
    #[serde(default)]
    pub total: i64,
    /// Member items whose stored status is `done`.
    #[serde(default)]
    pub done: i64,
    #[serde(default)]
    pub repos: Vec<MissionRepoRow>,
    /// What it asks of its loop (O4).
    #[serde(default)]
    pub policy: MissionPolicy,
    /// When the loop looks at it next (O4); `None`: when something wakes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_wake_at: Option<i64>,
    /// What it has spent, in micro-USD (`Store::mission_cost_micros`). Filled
    /// by `work { action: missions }` and the detail; absent elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_micros: Option<i64>,
    /// The live grant's budget, in micro-USD, when it sets one: the list's
    /// "spent of budget" meter. Filled where `cost_micros` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_micros: Option<i64>,
    /// Whether it waits on a person, and why (gap plan G1.6,
    /// `service::attention::mission_waiting`; hub contract 15). Filled
    /// where `cost_micros` is; absent when it waits on nobody.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_on: Option<crate::service::attention::MissionWait>,
}

/// A repo a mission may run in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionRepoRow {
    pub project_id: i64,
    /// `owner/repo`, for display.
    pub name: String,
    /// A free label: primary, mobile, backend…
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub created_at: i64,
}

/// One row of a mission's log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionEventRow {
    pub id: i64,
    pub at: i64,
    pub kind: String,
    /// person:<id> | planner:<run> | fleet | worker:<task>
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_item_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

/// What `create_mission` writes.
#[derive(Debug, Clone, Default)]
pub struct NewMission<'a> {
    pub org_id: Option<i64>,
    pub owner_person_id: Option<i64>,
    pub root_item_id: Option<i64>,
    pub name: &'a str,
    pub goal: &'a str,
    pub non_goals: Option<&'a str>,
    pub done_when: &'a [String],
    /// `None`: finite.
    pub mode: Option<&'a str>,
    pub level: Option<i64>,
}

/// What `update_mission` changes. `None` leaves a field; for the optional
/// text `Some(None)` clears it.
#[derive(Debug, Clone, Default)]
pub struct MissionPatch {
    pub name: Option<String>,
    pub goal: Option<String>,
    pub non_goals: Option<Option<String>>,
    pub done_when: Option<Vec<String>>,
    pub mode: Option<String>,
    pub level: Option<i64>,
    pub policy: Option<MissionPolicy>,
}

/// A [`NewMission`]'s fields, checked.
struct CheckedMission {
    name: String,
    goal: String,
    non_goals: Option<String>,
    done_when: Option<String>,
    mode: &'static str,
    level: i64,
}

/// One event to append.
#[derive(Debug, Clone, Default)]
pub struct NewMissionEvent<'a> {
    pub kind: &'a str,
    pub actor: &'a str,
    pub work_item_id: Option<i64>,
    pub task_id: Option<i64>,
    pub decision_id: Option<&'a str>,
    pub payload: Option<serde_json::Value>,
}

const MISSION_COLUMNS: &str = "m.id, m.org_id, m.owner_person_id, m.root_item_id, m.name, \
     m.goal, m.non_goals, m.done_when, m.mode, m.state, m.level, m.plan_version, \
     m.created_at, m.updated_at, m.started_at, m.finished_at, m.version, \
     (SELECT COUNT(*) FROM work_items i WHERE i.orchestration_project_id = m.id), \
     (SELECT COUNT(*) FROM work_items i \
       WHERE i.orchestration_project_id = m.id AND i.status_category = 'done'), \
     m.policy_json, m.next_wake_at";

fn map_mission(r: &rusqlite::Row<'_>) -> rusqlite::Result<MissionRow> {
    let done_when: Option<String> = r.get(7)?;
    Ok(MissionRow {
        id: r.get(0)?,
        org_id: r.get(1)?,
        owner_person_id: r.get(2)?,
        root_item_id: r.get(3)?,
        name: r.get(4)?,
        goal: r.get(5)?,
        non_goals: r.get(6)?,
        done_when: done_when
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default(),
        mode: r.get(8)?,
        state: r.get(9)?,
        level: r.get(10)?,
        plan_version: r.get(11)?,
        created_at: r.get(12)?,
        updated_at: r.get(13)?,
        started_at: r.get(14)?,
        finished_at: r.get(15)?,
        version: r.get(16)?,
        total: r.get(17)?,
        done: r.get(18)?,
        repos: Vec::new(),
        policy: r
            .get::<_, Option<String>>(19)?
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default(),
        next_wake_at: r.get(20)?,
        cost_micros: None,
        budget_micros: None,
        waiting_on: None,
    })
}

fn invalid(msg: impl Into<String>) -> IpcError {
    IpcError::new(codes::E_INVALID, msg)
}

fn not_found(id: i64) -> IpcError {
    IpcError::new(codes::E_NOTFOUND, format!("mission {id} not found"))
}

fn check_name(name: &str) -> Result<String, IpcError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("a mission needs a name"));
    }
    if name.chars().count() > MISSION_NAME_MAX_CHARS {
        return Err(invalid(format!(
            "a mission name is at most {MISSION_NAME_MAX_CHARS} characters"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(invalid("a mission name is one line of text"));
    }
    Ok(name.to_string())
}

fn check_goal(goal: &str) -> Result<String, IpcError> {
    match check_text("a goal", Some(goal))? {
        Some(g) => Ok(g),
        None => Err(invalid(
            "a mission needs a goal: what is true when it is done",
        )),
    }
}

fn check_text(what: &str, v: Option<&str>) -> Result<Option<String>, IpcError> {
    match v.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) if v.chars().count() > MISSION_TEXT_MAX_CHARS => Err(invalid(format!(
            "{what} is at most {MISSION_TEXT_MAX_CHARS} characters"
        ))),
        v => Ok(v.map(str::to_string)),
    }
}

/// The rows, trimmed, empty ones dropped, as the stored JSON (`None` for
/// none).
fn check_done_when(rows: &[String]) -> Result<Option<String>, IpcError> {
    let rows: Vec<&str> = rows
        .iter()
        .map(|r| r.trim())
        .filter(|r| !r.is_empty())
        .collect();
    if rows.len() > MISSION_DONE_WHEN_MAX {
        return Err(invalid(format!(
            "done_when is at most {MISSION_DONE_WHEN_MAX} conditions"
        )));
    }
    if let Some(r) = rows
        .iter()
        .find(|r| r.chars().count() > MISSION_DONE_WHEN_ROW_MAX_CHARS || r.contains('\n'))
    {
        return Err(invalid(format!(
            "a done_when condition is one line of at most \
             {MISSION_DONE_WHEN_ROW_MAX_CHARS} characters: {:?}…",
            r.chars().take(40).collect::<String>()
        )));
    }
    if rows.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::to_string(&rows).map_err(|e| {
        IpcError::new(codes::E_SERIALIZE, e.to_string())
    })?))
}

fn check_mode(mode: &str) -> Result<&'static str, IpcError> {
    MISSION_MODES
        .iter()
        .find(|m| **m == mode)
        .copied()
        .ok_or_else(|| {
            invalid(format!(
                "a mission's mode is {}; {mode:?} is neither",
                MISSION_MODES.join(" or ")
            ))
        })
}

fn check_level(level: i64) -> Result<i64, IpcError> {
    if (0..=MISSION_LEVEL_MAX).contains(&level) {
        Ok(level)
    } else {
        Err(invalid(format!(
            "a mission's level is 0 to {MISSION_LEVEL_MAX} (L0 asks a person for everything)"
        )))
    }
}

fn check_version(m: &MissionRow, expected: Option<i64>) -> Result<(), IpcError> {
    match expected {
        Some(v) if v != 0 && v != m.version => Err(IpcError::new(
            codes::E_CONFLICT,
            format!(
                "{} changed meanwhile (version {} now, {v} expected); reload it",
                m.name, m.version
            ),
        )
        .with_details(serde_json::json!({ "version": m.version }))),
        _ => Ok(()),
    }
}

/// Whether the lifecycle allows `from → to`.
pub fn mission_transition_allowed(from: &str, to: &str) -> bool {
    MISSION_TRANSITIONS.contains(&(from, to))
}

/// A UNIQUE violation on `ux_orch_events_decision`.
fn is_unique_violation(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _)
        if f.code == rusqlite::ErrorCode::ConstraintViolation)
}

impl Store {
    /// Every refusal [`Self::create_mission`] can answer, without writing.
    pub fn check_new_mission(&self, m: &NewMission<'_>) -> Result<(), IpcError> {
        self.checked_new_mission(m).map(|_| ())
    }

    fn checked_new_mission(&self, m: &NewMission<'_>) -> Result<CheckedMission, IpcError> {
        let checked = CheckedMission {
            name: check_name(m.name)?,
            goal: check_goal(m.goal)?,
            non_goals: check_text("non_goals", m.non_goals)?,
            done_when: check_done_when(m.done_when)?,
            mode: check_mode(m.mode.unwrap_or("finite"))?,
            level: check_level(m.level.unwrap_or(0))?,
        };
        if let Some(org) = m.org_id {
            let known: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM orgs WHERE id = ?1)",
                [org],
                |r| r.get(0),
            )?;
            if !known {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("organisation {org} not found"),
                ));
            }
        }
        if let Some(root) = m.root_item_id {
            self.get_work_item(root)?.ok_or_else(|| {
                IpcError::new(codes::E_NOTFOUND, format!("work item {root} not found"))
            })?;
            if let Some(holder) = self.mission_rooted_at(root)? {
                return Err(IpcError::new(
                    codes::E_EXISTS,
                    format!(
                        "work item {root} is already the root of mission {} ({})",
                        holder.0, holder.1
                    ),
                ));
            }
            if let Some(other) = self.item_mission(root)? {
                return Err(IpcError::new(
                    codes::E_EXISTS,
                    format!("work item {root} already belongs to mission {other}"),
                )
                .with_details(serde_json::json!({ "mission_id": other })));
            }
        }
        Ok(checked)
    }

    /// Create a mission, `draft`, and log `created`.
    pub fn create_mission(&self, m: &NewMission<'_>, actor: &str) -> Result<MissionRow, IpcError> {
        let CheckedMission {
            name,
            goal,
            non_goals,
            done_when,
            mode,
            level,
        } = self.checked_new_mission(m)?;
        let now = now_unix();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO orchestration_projects (org_id, owner_person_id, root_item_id, name, \
               goal, non_goals, done_when, mode, state, level, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'draft', ?9, ?10, ?10)",
            rusqlite::params![
                m.org_id,
                m.owner_person_id,
                m.root_item_id,
                name,
                goal,
                non_goals,
                done_when,
                mode,
                level,
                now
            ],
        )?;
        let id = tx.last_insert_rowid();
        // The root is a member too: its status is the mission's headline.
        if let Some(root) = m.root_item_id {
            tx.execute(
                "UPDATE work_items SET orchestration_project_id = ?1 \
                 WHERE id = ?2 AND orchestration_project_id IS NULL",
                rusqlite::params![id, root],
            )?;
        }
        self.insert_mission_event(
            id,
            &NewMissionEvent {
                kind: "created",
                actor,
                work_item_id: m.root_item_id,
                ..Default::default()
            },
        )?;
        tx.commit()?;
        self.require_mission(id)
    }

    /// The mission (id, name) whose root is `item`, if any.
    fn mission_rooted_at(&self, item: i64) -> Result<Option<(i64, String)>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, name FROM orchestration_projects WHERE root_item_id = ?1 LIMIT 1",
                [item],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    /// One mission with its roll-up and repos.
    pub fn get_mission(&self, id: i64) -> Result<Option<MissionRow>, IpcError> {
        let row = self
            .conn
            .query_row(
                &format!("SELECT {MISSION_COLUMNS} FROM orchestration_projects m WHERE m.id = ?1"),
                [id],
                map_mission,
            )
            .optional()?;
        let Some(mut row) = row else {
            return Ok(None);
        };
        row.repos = self.mission_repos(id)?;
        Ok(Some(row))
    }

    fn require_mission(&self, id: i64) -> Result<MissionRow, IpcError> {
        self.get_mission(id)?.ok_or_else(|| not_found(id))
    }

    fn mission_repos(&self, id: i64) -> Result<Vec<MissionRepoRow>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT r.project_id, p.owner || '/' || p.repo, r.role, r.created_at \
             FROM orchestration_project_repos r JOIN projects p ON p.id = r.project_id \
             WHERE r.orchestration_project_id = ?1 ORDER BY r.created_at, r.project_id",
        )?;
        let rows = stmt.query_map([id], |r| {
            Ok(MissionRepoRow {
                project_id: r.get(0)?,
                name: r.get(1)?,
                role: r.get(2)?,
                created_at: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every work item that belongs to a mission, as `(item id, mission
    /// id)`: the Work view's group by mission (redesign step 6.2). The
    /// caller fences which missions it may name.
    pub fn mission_membership(&self) -> Result<Vec<(i64, i64)>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, orchestration_project_id FROM work_items \
             WHERE orchestration_project_id IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Every mission: live ones (draft, active, paused) first, then by last
    /// change. The caller fences by org and owner.
    pub fn list_missions(&self) -> Result<Vec<MissionRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {MISSION_COLUMNS} FROM orchestration_projects m \
             ORDER BY m.state IN ('completed', 'failed', 'cancelled') ASC, \
                      m.updated_at DESC, m.id DESC"
        ))?;
        let rows = stmt.query_map([], map_mission)?;
        let mut out = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        for m in &mut out {
            m.repos = self.mission_repos(m.id)?;
        }
        Ok(out)
    }

    /// Change a mission's fields. A finished mission is read-only. With
    /// `expected_version`, a change made meanwhile answers `E_CONFLICT`
    /// and writes nothing.
    pub fn update_mission(
        &self,
        id: i64,
        expected_version: Option<i64>,
        p: &MissionPatch,
        actor: &str,
    ) -> Result<MissionRow, IpcError> {
        let before = self.require_mission(id)?;
        check_version(&before, expected_version)?;
        refuse_final(&before)?;
        let name = match &p.name {
            Some(n) => check_name(n)?,
            None => before.name.clone(),
        };
        let goal = match &p.goal {
            Some(g) => check_goal(g)?,
            None => before.goal.clone(),
        };
        let non_goals = match &p.non_goals {
            Some(n) => check_text("non_goals", n.as_deref())?,
            None => before.non_goals.clone(),
        };
        let done_when = match &p.done_when {
            Some(d) => check_done_when(d)?,
            None => check_done_when(&before.done_when)?,
        };
        let mode = match &p.mode {
            Some(m) => check_mode(m)?.to_string(),
            None => before.mode.clone(),
        };
        if mode != before.mode {
            // A plan may hold more than the loop runs: leaving `plan` keeps
            // the loop's cap (O10).
            let members: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM work_items WHERE orchestration_project_id = ?1 \
                   AND (?2 <> 'continuous' OR status_category <> 'done')",
                rusqlite::params![id, mode],
                |r| r.get(0),
            )?;
            let cap = mission_item_cap(&mode);
            if members > cap {
                return Err(invalid(format!(
                    "{} holds {members} items; a {mode} mission holds at most {cap}",
                    before.name
                )));
            }
        }
        let level = match p.level {
            Some(l) => check_level(l)?,
            None => before.level,
        };
        let policy = match &p.policy {
            Some(pol) => check_policy(pol)?,
            None => before.policy.clone(),
        };
        let policy_json = serde_json::to_string(&policy)
            .map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE orchestration_projects SET name = ?1, goal = ?2, non_goals = ?3, \
               done_when = ?4, mode = ?5, level = ?6, policy_json = ?7, updated_at = ?8, \
               version = version + 1 \
             WHERE id = ?9",
            rusqlite::params![
                name,
                goal,
                non_goals,
                done_when,
                mode,
                level,
                policy_json,
                now_unix(),
                id
            ],
        )?;
        let mut changed = Vec::new();
        if name != before.name {
            changed.push("name");
        }
        if goal != before.goal {
            changed.push("goal");
        }
        if non_goals != before.non_goals {
            changed.push("non_goals");
        }
        if p.done_when.is_some() {
            changed.push("done_when");
        }
        if mode != before.mode {
            changed.push("mode");
        }
        if level != before.level {
            changed.push("level");
        }
        if policy != before.policy {
            changed.push("policy");
        }
        self.insert_mission_event(
            id,
            &NewMissionEvent {
                kind: "updated",
                actor,
                payload: Some(serde_json::json!({ "fields": changed })),
                ..Default::default()
            },
        )?;
        tx.commit()?;
        self.require_mission(id)
    }

    /// Move a mission along its lifecycle ([`MISSION_TRANSITIONS`]). The
    /// first `active` stamps `started_at`, a final state `finished_at`.
    /// Becoming `active` clears `next_wake_at`, so a resumed mission is due
    /// at once rather than after the wake its last tick (or brake) set.
    pub fn set_mission_state(
        &self,
        id: i64,
        expected_version: Option<i64>,
        to: &str,
        actor: &str,
    ) -> Result<MissionRow, IpcError> {
        let before = self.require_mission(id)?;
        check_version(&before, expected_version)?;
        if !MISSION_STATES.contains(&to) {
            return Err(invalid(format!(
                "a mission's state is one of {}",
                MISSION_STATES.join(", ")
            )));
        }
        if before.state == to {
            return Ok(before);
        }
        if !mission_transition_allowed(&before.state, to) {
            return Err(invalid(format!(
                "a mission does not go from {} to {to}",
                before.state
            )));
        }
        let now = now_unix();
        let started_at = before
            .started_at
            .or_else(|| (to == "active").then_some(now));
        let finished_at = MISSION_FINAL_STATES.contains(&to).then_some(now);
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE orchestration_projects SET state = ?1, started_at = ?2, finished_at = ?3, \
               updated_at = ?4, version = version + 1, \
               next_wake_at = CASE WHEN ?1 = 'active' THEN NULL ELSE next_wake_at END \
             WHERE id = ?5",
            rusqlite::params![to, started_at, finished_at, now, id],
        )?;
        self.insert_mission_event(
            id,
            &NewMissionEvent {
                kind: "state",
                actor,
                payload: Some(serde_json::json!({ "from": before.state, "to": to })),
                ..Default::default()
            },
        )?;
        tx.commit()?;
        self.require_mission(id)
    }

    /// Delete a mission that is not running: a draft, or a finished one.
    /// Its items stay and leave it; its repos and events go with it.
    pub fn delete_mission(&self, id: i64) -> Result<bool, IpcError> {
        let Some(m) = self.get_mission(id)? else {
            return Ok(false);
        };
        if matches!(m.state.as_str(), "active" | "paused") {
            return Err(invalid(format!(
                "{} is {}; cancel it before deleting it",
                m.name, m.state
            )));
        }
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE work_items SET orchestration_project_id = NULL \
             WHERE orchestration_project_id = ?1",
            [id],
        )?;
        let n = tx.execute("DELETE FROM orchestration_projects WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(n > 0)
    }

    /// The live sessions working on a mission's member items (a confirmed,
    /// unended link), each with the member item it serves: what a finished
    /// mission offers to archive (G3.7). Oldest session first.
    pub fn mission_live_sessions(&self, id: i64) -> Result<Vec<(i64, i64)>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT p.session_id, MIN(l.item_id) FROM work_links l \
             JOIN work_items i ON i.id = l.item_id AND i.orchestration_project_id = ?1 \
             JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
             WHERE l.state = 'confirmed' AND l.ended_at IS NULL AND p.session_id IS NOT NULL \
             GROUP BY p.session_id ORDER BY p.session_id",
        )?;
        let rows = stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The pull request URLs a mission's work has had: a live session's
    /// current one and the one each ended link recorded, so a PR stays on
    /// the mission after its session is archived (G3.7).
    pub fn mission_pr_urls(&self, id: i64) -> Result<Vec<String>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT s.pr_url FROM work_links l \
             JOIN work_items i ON i.id = l.item_id AND i.orchestration_project_id = ?1 \
             JOIN participants p ON p.id = l.participant_id AND p.retired_at IS NULL \
             JOIN sessions s ON s.id = p.session_id \
             WHERE l.state = 'confirmed' AND l.ended_at IS NULL AND s.pr_url IS NOT NULL \
             UNION \
             SELECT l.snap_pr_url FROM work_links l \
             JOIN work_items i ON i.id = l.item_id AND i.orchestration_project_id = ?1 \
             WHERE l.state = 'confirmed' AND l.snap_pr_url IS NOT NULL",
        )?;
        let rows = stmt.query_map([id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Add a repo to a mission's allow-list (or change its role), or remove
    /// it.
    pub fn set_mission_repo(
        &self,
        id: i64,
        project_id: i64,
        role: Option<&str>,
        on: bool,
        actor: &str,
    ) -> Result<MissionRow, IpcError> {
        let m = self.require_mission(id)?;
        refuse_final(&m)?;
        let role = role.map(str::trim).filter(|r| !r.is_empty());
        if let Some(r) = role {
            if r.chars().count() > MISSION_ROLE_MAX_CHARS || r.chars().any(char::is_control) {
                return Err(invalid(format!(
                    "a repo's role is one line of at most {MISSION_ROLE_MAX_CHARS} characters"
                )));
            }
        }
        let tx = self.conn.unchecked_transaction()?;
        let changed = if on {
            if self.get_project(project_id)?.is_none() {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("project {project_id} not found"),
                ));
            }
            tx.execute(
                "INSERT INTO orchestration_project_repos \
                   (orchestration_project_id, project_id, role, created_at) \
                 VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT (orchestration_project_id, project_id) DO UPDATE SET role = ?3",
                rusqlite::params![id, project_id, role, now_unix()],
            )?
        } else {
            tx.execute(
                "DELETE FROM orchestration_project_repos \
                 WHERE orchestration_project_id = ?1 AND project_id = ?2",
                rusqlite::params![id, project_id],
            )?
        };
        if changed > 0 {
            self.touch_mission(id)?;
            self.insert_mission_event(
                id,
                &NewMissionEvent {
                    kind: if on { "repo_added" } else { "repo_removed" },
                    actor,
                    payload: Some(serde_json::json!({ "project_id": project_id, "role": role })),
                    ..Default::default()
                },
            )?;
        }
        tx.commit()?;
        self.require_mission(id)
    }

    /// Make an item a member of a mission, or take it out. An item is in at
    /// most one mission; the root never leaves its own.
    pub fn set_mission_item(
        &self,
        id: i64,
        item_id: i64,
        on: bool,
        actor: &str,
    ) -> Result<MissionRow, IpcError> {
        let m = self.require_mission(id)?;
        refuse_final(&m)?;
        let current: Option<Option<i64>> = self
            .conn
            .query_row(
                "SELECT orchestration_project_id FROM work_items WHERE id = ?1",
                [item_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(current) = current else {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("work item {item_id} not found"),
            ));
        };
        let tx = self.conn.unchecked_transaction()?;
        let changed = if on {
            match current {
                Some(c) if c == id => false,
                Some(c) => {
                    let name = self
                        .get_mission(c)?
                        .map(|o| o.name)
                        .unwrap_or_else(|| format!("#{c}"));
                    return Err(IpcError::new(
                        codes::E_EXISTS,
                        format!("work item {item_id} already belongs to mission {name}"),
                    )
                    .with_details(serde_json::json!({ "mission_id": c })));
                }
                None => {
                    self.check_mission_room(id, 1)?;
                    tx.execute(
                        "UPDATE work_items SET orchestration_project_id = ?1 WHERE id = ?2",
                        rusqlite::params![id, item_id],
                    )?;
                    true
                }
            }
        } else {
            if m.root_item_id == Some(item_id) {
                return Err(invalid(format!(
                    "work item {item_id} is {}'s root; it stays in its mission",
                    m.name
                )));
            }
            tx.execute(
                "UPDATE work_items SET orchestration_project_id = NULL \
                 WHERE id = ?1 AND orchestration_project_id = ?2",
                rusqlite::params![item_id, id],
            )? > 0
        };
        if changed {
            self.touch_mission(id)?;
            self.insert_mission_event(
                id,
                &NewMissionEvent {
                    kind: if on { "item_added" } else { "item_removed" },
                    actor,
                    work_item_id: Some(item_id),
                    ..Default::default()
                },
            )?;
        }
        tx.commit()?;
        self.require_mission(id)
    }

    /// Refuse `n` more members for mission `id`: a finished mission takes
    /// none, and no mission holds more than [`MISSION_ITEM_CAP`] (a
    /// continuous one counts its open members only; a plan may hold
    /// [`PLAN_MISSION_ITEM_CAP`]).
    pub(crate) fn check_mission_room(&self, id: i64, n: usize) -> Result<(), IpcError> {
        let m = self.require_mission(id)?;
        refuse_final(&m)?;
        let counted: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM work_items WHERE orchestration_project_id = ?1 \
               AND (?2 <> 'continuous' OR status_category <> 'done')",
            rusqlite::params![id, m.mode],
            |r| r.get(0),
        )?;
        let cap = mission_item_cap(&m.mode);
        if counted + n as i64 > cap {
            return Err(invalid(format!(
                "{} already holds {counted} of {cap} items; split it into \
                 another mission",
                m.name
            )));
        }
        Ok(())
    }

    /// Place a just-made item in mission `id` and log it, inside the
    /// caller's transaction (a proposal joining its parent's mission).
    pub(super) fn join_mission(&self, id: i64, item_id: i64, actor: &str) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE work_items SET orchestration_project_id = ?1 \
             WHERE id = ?2 AND orchestration_project_id IS NULL",
            rusqlite::params![id, item_id],
        )?;
        self.touch_mission(id)?;
        self.insert_mission_event(
            id,
            &NewMissionEvent {
                kind: "item_added",
                actor,
                work_item_id: Some(item_id),
                ..Default::default()
            },
        )?;
        Ok(())
    }

    fn touch_mission(&self, id: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE orchestration_projects SET updated_at = ?1, version = version + 1 \
             WHERE id = ?2",
            rusqlite::params![now_unix(), id],
        )?;
        Ok(())
    }

    /// A mission's member items, root first, then by key or title.
    pub fn mission_items(&self, id: i64) -> Result<Vec<WorkItemRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ITEM_COLUMNS} FROM work_items \
             WHERE orchestration_project_id = ?1 \
             ORDER BY id = (SELECT root_item_id FROM orchestration_projects WHERE id = ?1) DESC, \
                      id"
        ))?;
        let rows = stmt.query_map([id], map_item)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The mission an item belongs to, if any.
    pub fn item_mission(&self, item_id: i64) -> Result<Option<i64>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT orchestration_project_id FROM work_items WHERE id = ?1",
                [item_id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten())
    }

    /// Append one event. A `decision_id` already logged is not logged
    /// again: `Ok(None)` says so, which is what makes one planner command
    /// execute once.
    pub fn record_mission_event(
        &self,
        id: i64,
        e: &NewMissionEvent<'_>,
    ) -> Result<Option<i64>, IpcError> {
        self.require_mission(id)?;
        let tx = self.conn.unchecked_transaction()?;
        let row = self.insert_mission_event(id, e)?;
        tx.commit()?;
        Ok(row)
    }

    /// [`Self::record_mission_event`] inside the caller's transaction, for
    /// the store's other writers (`item_deps`).
    pub(super) fn append_mission_event(
        &self,
        id: i64,
        e: &NewMissionEvent<'_>,
    ) -> Result<Option<i64>, IpcError> {
        self.insert_mission_event(id, e)
    }

    /// [`Self::record_mission_event`] inside the caller's transaction.
    fn insert_mission_event(
        &self,
        id: i64,
        e: &NewMissionEvent<'_>,
    ) -> Result<Option<i64>, IpcError> {
        let payload = match &e.payload {
            Some(p) => {
                let text = p.to_string();
                if text.len() > MISSION_EVENT_PAYLOAD_MAX {
                    Some(serde_json::json!({ "truncated": true, "bytes": text.len() }).to_string())
                } else {
                    Some(text)
                }
            }
            None => None,
        };
        let inserted = self.conn.execute(
            "INSERT INTO orchestration_events (orchestration_project_id, at, kind, actor, \
               work_item_id, task_id, decision_id, payload) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                id,
                now_unix(),
                e.kind,
                e.actor,
                e.work_item_id,
                e.task_id,
                e.decision_id,
                payload
            ],
        );
        match inserted {
            Ok(_) => {}
            Err(err) if e.decision_id.is_some() && is_unique_violation(&err) => return Ok(None),
            Err(err) => return Err(err.into()),
        }
        let row = self.conn.last_insert_rowid();
        self.fold_mission_events(id)?;
        Ok(Some(row))
    }

    /// Keep at most [`MISSION_EVENT_CAP`] rows: the oldest fold into the
    /// mission's one `digest` row, which counts them by kind.
    fn fold_mission_events(&self, id: i64) -> Result<(), IpcError> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM orchestration_events \
             WHERE orchestration_project_id = ?1 AND kind <> 'digest'",
            [id],
            |r| r.get(0),
        )?;
        let over = n - MISSION_EVENT_CAP;
        if over <= 0 {
            return Ok(());
        }
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, at FROM orchestration_events \
             WHERE orchestration_project_id = ?1 AND kind <> 'digest' \
             ORDER BY at, id LIMIT ?2",
        )?;
        let oldest = stmt
            .query_map(rusqlite::params![id, over], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let digest: Option<(i64, Option<String>)> = self
            .conn
            .query_row(
                "SELECT id, payload FROM orchestration_events \
                 WHERE orchestration_project_id = ?1 AND kind = 'digest'",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let mut counts: serde_json::Map<String, serde_json::Value> = digest
            .as_ref()
            .and_then(|(_, p)| p.as_deref())
            .and_then(|p| serde_json::from_str::<serde_json::Value>(p).ok())
            .and_then(|v| v.get("kinds").and_then(|k| k.as_object().cloned()))
            .unwrap_or_default();
        let mut until = 0;
        for (row, kind, at) in &oldest {
            let c = counts.get(kind).and_then(|v| v.as_i64()).unwrap_or(0);
            counts.insert(kind.clone(), (c + 1).into());
            until = until.max(*at);
            self.conn
                .execute("DELETE FROM orchestration_events WHERE id = ?1", [row])?;
        }
        let payload = serde_json::json!({ "kinds": counts, "until": until }).to_string();
        match digest {
            Some((row, _)) => {
                self.conn.execute(
                    "UPDATE orchestration_events SET payload = ?1, at = ?2 WHERE id = ?3",
                    rusqlite::params![payload, until, row],
                )?;
            }
            None => {
                self.conn.execute(
                    "INSERT INTO orchestration_events \
                       (orchestration_project_id, at, kind, actor, payload) \
                     VALUES (?1, ?2, 'digest', 'fleet', ?3)",
                    rusqlite::params![id, until, payload],
                )?;
            }
        }
        Ok(())
    }

    /// A mission's log, newest first, `limit` rows before `before_id`.
    pub fn mission_events(
        &self,
        id: i64,
        before_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<MissionEventRow>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, at, kind, actor, work_item_id, task_id, decision_id, payload \
             FROM orchestration_events \
             WHERE orchestration_project_id = ?1 AND (?2 IS NULL OR id < ?2) \
             ORDER BY at DESC, id DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(rusqlite::params![id, before_id, limit as i64], |r| {
            let payload: Option<String> = r.get(7)?;
            Ok(MissionEventRow {
                id: r.get(0)?,
                at: r.get(1)?,
                kind: r.get(2)?,
                actor: r.get(3)?,
                work_item_id: r.get(4)?,
                task_id: r.get(5)?,
                decision_id: r.get(6)?,
                payload: payload.and_then(|p| serde_json::from_str(&p).ok()),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

fn refuse_final(m: &MissionRow) -> Result<(), IpcError> {
    if MISSION_FINAL_STATES.contains(&m.state.as_str()) {
        Err(invalid(format!(
            "{} is {}; a finished mission does not change",
            m.name, m.state
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
