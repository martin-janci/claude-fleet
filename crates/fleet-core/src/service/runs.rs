//! `runs { list }` (Orbit Fleet redesign step 8.3): every run on the
//! fleet's behalf — dispatched tasks, a mission's actions and brakes, Jev's
//! decisions, fleet's own `claude -p` runs and routine fires — in one
//! newest-first list, each linked to the sessions it ran in. The union and
//! its column mappings are `store::runs`; this file decides who sees which
//! row.
//!
//! **Reach** (`store::RunsReach`), from the caller's `ViewScope`:
//!
//! - the hub's own unnarrowed reader (the standalone desktop) sees all;
//! - everyone else sees a task when they see every session it names
//!   (`service::tasks::task_visible_in_scope_pure`, less its pane-proof
//!   clause, so narrower and never wider), a mission's actions, brakes and
//!   planner runs when they may read the mission
//!   (`work::missions::sees_mission`), a routine's fires when they may read
//!   the routine (`routines::sees_routine`), a Jev run or a summary when
//!   they see the session it was about, and the runs that belong to no session or
//!   mission only when they are served whole-fleet spend
//!   (`org_spend::sees_all_spend`, and no org fence).

use crate::ipc_error::{lock, IpcError};
use crate::service::view_scope::ViewScope;
use crate::store::{RunRow, RunsFilter, RunsReach, Store, RUNS_DEFAULT_LIMIT};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// `runs { list }`'s filters, as a caller sends them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunsArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routine_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
}

/// What `runs { list }` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunsPage {
    pub runs: Vec<RunRow>,
    /// How many match the filters in all (the page is `limit` of them).
    pub total: i64,
}

/// The reach of `scope` over runs. See the module doc.
pub fn reach(s: &Store, scope: &ViewScope) -> Result<RunsReach, IpcError> {
    if scope.is_unrestricted() {
        return Ok(RunsReach::All);
    }
    let all = s.list_all_sessions()?;
    let sessions: Vec<i64> = all
        .iter()
        .filter(|r| scope.sees_session_row(r).is_visible())
        .map(|r| r.id)
        .collect();
    // Whole-fleet spend (`org_spend::sees_all_spend`'s condition) and no org
    // narrowing. This is the org boundary, not a privacy fence: a reader
    // bound to an org never sees the runs that belong to no session or
    // mission. The person half is the session count beside it — every
    // session there is passed `sees_session_row` above.
    let spend = scope.org.is_all() && sessions.len() == all.len();
    let mut missions = Vec::new();
    for m in s.list_missions()? {
        if crate::service::work::missions::sees_mission(s, scope, &m)? {
            missions.push(m.id);
        }
    }
    let mut routines = Vec::new();
    for r in s.list_routines()? {
        if crate::service::routines::sees_routine(s, scope, &r)? {
            routines.push(r.id);
        }
    }
    Ok(RunsReach::Scoped {
        sessions,
        missions,
        routines,
        spend,
    })
}

/// One page of the runs `scope` may see.
pub fn list(
    store: &Mutex<Store>,
    scope: &ViewScope,
    args: &RunsArgs,
) -> Result<RunsPage, IpcError> {
    let s = lock(store)?;
    list_in(&s, scope, args)
}

/// [`list`] on a store the caller already holds.
pub fn list_in(s: &Store, scope: &ViewScope, args: &RunsArgs) -> Result<RunsPage, IpcError> {
    let filter = RunsFilter {
        since: args.since,
        until: args.until,
        kind: args.kind.clone().filter(|k| !k.is_empty()),
        outcome: args.outcome.clone().filter(|o| !o.is_empty()),
        org_id: args.org_id,
        mission_id: args.mission_id,
        session_id: args.session_id,
        routine_id: args.routine_id,
        limit: args.limit.unwrap_or(RUNS_DEFAULT_LIMIT),
        offset: args.offset.unwrap_or(0),
        reach: RunsReach::All,
    };
    // Refuse a bad filter before the reach is worked out.
    filter.validate()?;
    let filter = RunsFilter {
        reach: reach(s, scope)?,
        ..filter
    };
    let (runs, total) = s.runs_list(&filter)?;
    Ok(RunsPage { runs, total })
}

#[cfg(test)]
mod tests;
