//! Lane execution (lanes design 2026-10-23 §3, L3): a mission whose policy
//! says `execution: lane` runs each lane's tasks one after another in one
//! lead session on the lane's branch, instead of a session and a checkout
//! per run.
//!
//! * **Which steps.** [`gate`] keeps one implement run per lane, the first
//!   in graph order, and none while the lane is [`busy_lanes`] (a task of it
//!   has an open run, or its lead is not at an idle prompt). A task in no
//!   lane, and every review, test or integration, runs as before.
//! * **Where.** [`run_in_lane`] hands the task to the lane's live lead as a
//!   new `tasks` row typed into it ([`deliver`]); with no live lead, or one
//!   that has had [`MissionPolicy::lane_relay_after`] tasks, or a retry of a
//!   task that already failed twice on it, it starts a fresh lead in the
//!   lane's own checkout ([`start_lead`]): the relay of §3.5, and the way
//!   back after a lead died.
//! * **What it carries.** Each task records the commit it began on
//!   (`tasks.start_head`), so its evidence is its own commits only
//!   (`report::evidence_script_since`).
//!
//! The lead is the mission's own session (`SessionOrigin::mission`), under
//! the same guard, grant and brakes as any run of it.
//!
//! [`MissionPolicy::lane_relay_after`]: crate::store::MissionPolicy

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use super::steps::{open_impl, Step};
use super::Deps;
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::trackers::tickets::{self, StartArgs};
use crate::service::view_scope::ViewScope;
use crate::service::work::run;
use crate::store::{MissionLaneRow, MissionRow, SessionRow, Store, TaskRow};

/// The wall clock of reading a checkout's HEAD.
const HEAD_READ: Duration = Duration::from_secs(20);

/// PURE: the steps of a mission in lane execution. A lane takes one
/// implement run at a time, the first of its steps in graph order, and
/// none while it is `busy`; every other step passes as it is.
pub fn gate(steps: Vec<Step>, lane_of: &HashMap<i64, i64>, busy: &HashSet<i64>) -> Vec<Step> {
    let mut taken: HashSet<i64> = HashSet::new();
    steps
        .into_iter()
        .filter(|st| {
            let Some(&lane) = st.item_id.and_then(|i| lane_of.get(&i)) else {
                return true;
            };
            !st.is_impl() || (!busy.contains(&lane) && taken.insert(lane))
        })
        .collect()
}

/// The lane's lead when it is still the lane's: a running session of this
/// mission in the lane's checkout. A session id is reused once its row is
/// gone, so the id alone does not say.
fn live_lead(
    s: &Store,
    m: &MissionRow,
    lane: &MissionLaneRow,
) -> Result<Option<SessionRow>, IpcError> {
    let Some(id) = lane.lead_session_id else {
        return Ok(None);
    };
    let own = crate::store::SessionOrigin::mission(m.id);
    Ok(s.get_session_by_id(id)?.filter(|r| {
        r.status == "running"
            && r.origin.as_deref() == Some(own.origin)
            && r.origin_ref == own.origin_ref
            && (lane.worktree_id.is_none() || r.worktree_id == lane.worktree_id)
    }))
}

/// How full a lead's context may be when it is handed a task (§3.5): past
/// this the next task goes to a fresh lead, so compaction never lands in
/// the middle of one.
pub const RELAY_CONTEXT_PCT: f64 = 70.0;

/// PURE: whether a live lead hands over before the next task (§3.5, D4):
/// it has had `relay_after` tasks, its context is [`RELAY_CONTEXT_PCT`]
/// full, or this task already ran on it twice (the first retry stays in
/// its context, the next gets fresh eyes).
pub fn needs_relay(
    lead_runs: i64,
    tries_here: usize,
    relay_after: u32,
    context_pct: Option<f64>,
) -> bool {
    lead_runs >= i64::from(relay_after)
        || tries_here >= 2
        || context_pct.is_some_and(|p| p >= RELAY_CONTEXT_PCT)
}

/// Whether a session waits on a dialog Enter would answer.
fn on_a_dialog(row: &SessionRow) -> bool {
    row.stuck_kind.is_some() || row.claude_status.as_deref() == Some("blocked")
}

/// PURE: the members whose `test` line is still to pass while they are
/// verified (lanes design §3.7, L4): their lane waits, so the test measures
/// that task in the lane's checkout before the lead moves on.
pub fn awaiting_test(graph: &crate::service::work::graph::MissionGraph) -> HashSet<i64> {
    graph
        .nodes
        .iter()
        .filter(|n| n.state == "verifying")
        .filter(|n| {
            n.verification.as_ref().is_some_and(|v| {
                v.checks
                    .iter()
                    .any(|c| c.kind == "test" && c.state != "pass")
            })
        })
        .map(|n| n.item_id)
        .collect()
}

/// The lanes of `m` that cannot take a task now: a task of theirs has an
/// open implement or test run or waits for its test (`testing`), or their
/// live lead is not at an idle prompt.
pub fn busy_lanes(
    s: &Store,
    m: &MissionRow,
    lanes: &[MissionLaneRow],
    attempts: &HashMap<i64, Vec<TaskRow>>,
    testing: &HashSet<i64>,
) -> Result<HashSet<i64>, IpcError> {
    let mut busy = HashSet::new();
    for l in lanes {
        let open = l.item_ids.iter().any(|i| {
            testing.contains(i)
                || attempts.get(i).is_some_and(|ts| {
                    ts.iter().any(|t| {
                        open_impl(t)
                            || (t.role.as_deref() == Some("test")
                                && matches!(t.state.as_str(), "queued" | "running"))
                    })
                })
        });
        let lead_busy = live_lead(s, m, l)?.is_some_and(|r| {
            !crate::service::sessions::deferred::idle_for_prompt(&r) || on_a_dialog(&r)
        });
        if open || lead_busy {
            busy.insert(l.id);
        }
    }
    Ok(busy)
}

/// PURE: the brief a lane's lead starts with (§3.1): the mission's goal,
/// the lane's tasks in order, and how a lane is worked.
pub fn lane_brief(m: &MissionRow, lane: &str, tasks: &[String]) -> String {
    let clip = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    let mut out = format!(
        "You are the lead of lane \"{}\" of mission \"{}\".\nGoal: {}\n",
        clip(lane, 40),
        clip(&m.name, 120),
        clip(&m.goal, 1200)
    );
    if let Some(n) = m.non_goals.as_deref().filter(|n| !n.trim().is_empty()) {
        out.push_str(&format!("Not in scope: {}\n", clip(n, 600)));
    }
    out.push_str("\nFleet hands you this lane's tasks one at a time, in this order:\n");
    for (i, t) in tasks.iter().take(30).enumerate() {
        out.push_str(&format!("{}. {}\n", i + 1, clip(t, 160)));
    }
    out.push_str(
        "\nFor each task: work on that task only, and commit your work on this branch at its \
         end (never amend, rebase or squash the commits of an earlier task); then report it as \
         its prompt asks. Do not start the next task until fleet hands it to you. After your \
         first task, open a draft pull request for this branch if you may push it; never mark \
         it ready or merge it.",
    );
    out.chars()
        .take(crate::service::work::handover::BRIEF_MAX_CHARS)
        .collect()
}

/// PURE: what a lane task's prompt says: the task, the earlier runs' words
/// on a retry, and the subagent note when the setting asks for it.
pub fn task_prompt(
    key: &str,
    title: &str,
    notes: Option<&str>,
    role: &str,
    context: Option<&str>,
    subagents: bool,
) -> String {
    let mut p = format!(
        "Next task in your lane.\n\n{}",
        run::run_prompt(key, title, notes, role, false)
    );
    if let Some(c) = context.map(str::trim).filter(|c| !c.is_empty()) {
        p.push_str(&format!(
            "\n\nWhat the earlier attempts left (their words, quoted):\n> {}",
            c.replace('\n', "\n> ")
        ));
    }
    if let Some(n) = run::subagent_note(role).filter(|_| subagents) {
        p.push_str("\n\n");
        p.push_str(n);
    }
    p
}

/// The commit a checkout stands on, read on its host; `None` when it could
/// not be read (the task's evidence then reads against the merge base).
async fn read_head(deps: &Deps, host: &str, dir: &str) -> Option<String> {
    let script = format!("git -C {} rev-parse HEAD", crate::shell::quote(dir));
    let run = crate::service::catalog::inventory::run_host_script(&deps.ssh, host, &script);
    let out = tokio::time::timeout(HEAD_READ, run).await.ok()?.ok()?;
    let sha = out.trim().to_string();
    crate::store::is_commit_id(&sha).then_some(sha)
}

/// The key and title of `item_id` as its task prompt names them.
fn key_title(s: &Store, item_id: i64) -> Result<(String, String, Option<String>), IpcError> {
    let it = s
        .get_work_item(item_id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("work item {item_id}")))?;
    Ok((
        it.key.clone().unwrap_or_else(|| format!("#{item_id}")),
        it.title.clone(),
        it.notes.clone(),
    ))
}

/// Run `role` on `item_id`, a task of lane `lane_id`, the lane's way: hand
/// it to the live lead, or start a lead. `start` is the run's start as the
/// mission's grant places it.
#[allow(clippy::too_many_arguments)]
pub async fn run_in_lane(
    deps: &Deps,
    m: &MissionRow,
    lane_id: i64,
    item_id: i64,
    role: &str,
    start: StartArgs,
    context: Option<&str>,
    view: &ViewScope,
) -> Result<(String, Option<i64>), IpcError> {
    let (attempt, _claim, lane, lead, relay) = {
        let s = lock(&deps.store)?;
        let attempt = match run::precheck(&s, view, item_id, role)? {
            run::Precheck::Existing(t) => {
                return Ok((format!("a {role} run was already open"), Some(t.id)));
            }
            run::Precheck::Fresh { attempt } => attempt,
        };
        let claim = run::claim_start(&deps.store, item_id, role).ok_or_else(|| {
            IpcError::new(
                codes::E_INVALID_STATE,
                format!("a {role} run on this item is already starting"),
            )
        })?;
        let lane = s
            .get_mission_lane(lane_id)?
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("lane {lane_id}")))?;
        let lead = live_lead(&s, m, &lane)?;
        // §3.5 and D4: a lead hands over after its share of tasks, and a
        // task that already failed twice on it gets fresh eyes.
        let relay = match &lead {
            Some(r) => {
                let tries_here = s
                    .tasks_for_item(item_id)?
                    .iter()
                    .filter(|t| t.worker_session_id == Some(r.id))
                    .count();
                needs_relay(
                    s.lead_run_count(r.id)?,
                    tries_here,
                    m.policy.lane_relay_after,
                    r.context_pct,
                )
            }
            None => false,
        };
        (attempt, claim, lane, lead, relay)
    };
    match lead {
        Some(row) if !relay => deliver(deps, &lane, &row, item_id, role, attempt, context).await,
        old => {
            if let Some(row) = old {
                stop_lead(deps, &row).await;
            }
            start_lead(deps, m, &lane, item_id, role, attempt, start, context, view).await
        }
    }
}

/// End a lead that hands over (its checkout stays for the next one).
async fn stop_lead(deps: &Deps, row: &SessionRow) {
    if let Err(e) = crate::service::sessions::kill_session(
        crate::service::sessions::KillSessionArgs {
            host_alias: row.host_alias.clone(),
            name: row.tmux_name.clone(),
            force: false,
        },
        &deps.store,
        &deps.ssh,
    )
    .await
    {
        tracing::warn!(host = %row.host_alias, session = %row.tmux_name, error = %e,
            "[lanes] a lead that hands over was not stopped");
    }
}

fn subagents_on(s: &Store) -> bool {
    crate::service::settings::get_bool(s, crate::service::settings::ORCHESTRATOR_RUN_SUBAGENTS)
}

/// Hand the task to the live lead `row`: a `tasks` row on it, begun at its
/// checkout's HEAD, typed in as a direct send at its idle prompt.
#[allow(clippy::too_many_arguments)]
async fn deliver(
    deps: &Deps,
    lane: &MissionLaneRow,
    row: &SessionRow,
    item_id: i64,
    role: &str,
    attempt: i64,
    context: Option<&str>,
) -> Result<(String, Option<i64>), IpcError> {
    if !crate::service::sessions::deferred::idle_for_prompt(row) || on_a_dialog(row) {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("lane {}'s lead is not at its prompt yet", lane.name),
        ));
    }
    let (prompt, task) = begin_task(deps, row, item_id, role, attempt, context, "").await?;
    let body = crate::service::work::report::with_run_instruction(&prompt, task.id, &task.nonce);
    let sent = crate::service::sessions::deferred::send_direct(&deps.store, row, &body, |d| {
        let body = body.clone();
        async move {
            if on_a_dialog(&d.row) {
                return Err(IpcError::new(
                    codes::E_INVALID_STATE,
                    "the lead opened a dialog meanwhile",
                ));
            }
            crate::service::sessions::send_prompt(
                crate::service::sessions::SendPromptArgs {
                    host_alias: d.row.host_alias.clone(),
                    tmux_name: d.row.tmux_name.clone(),
                    prompt: body,
                    submit: true,
                    keys: None,
                    expect: None,
                },
                &deps.store,
                &deps.ssh,
            )
            .await
        }
    })
    .await;
    if let Err(e) = sent {
        let s = lock(&deps.store)?;
        let _ = crate::service::tasks::fail_task(
            &s,
            task.id,
            &format!("it was not typed into the lane's lead: {}", e.message),
        );
        return Err(e);
    }
    Ok((
        format!("handed to lane {}'s lead", lane.name),
        Some(task.id),
    ))
}

/// Record attempt `attempt` of `item_id` as a run on `row`, its prompt the
/// task's after `preamble`, stamped with the head its checkout starts at.
async fn begin_task(
    deps: &Deps,
    row: &SessionRow,
    item_id: i64,
    role: &str,
    attempt: i64,
    context: Option<&str>,
    preamble: &str,
) -> Result<(String, TaskRow), IpcError> {
    let dir = {
        let s = lock(&deps.store)?;
        row.worktree_id
            .map(|w| s.worktree_path(w))
            .transpose()?
            .flatten()
    };
    let head = match &dir {
        Some(d) => read_head(deps, &row.host_alias, d).await,
        None => None,
    };
    let s = lock(&deps.store)?;
    let (key, title, notes) = key_title(&s, item_id)?;
    let task_text = task_prompt(
        &key,
        &title,
        notes.as_deref(),
        role,
        context,
        subagents_on(&s),
    );
    let prompt = format!("{preamble}{task_text}");
    let task = run::record_run(&s, row.id, &prompt, item_id, attempt, role)?;
    if let Some(h) = &head {
        s.set_task_start_head(task.id, h)?;
    }
    Ok((prompt, task))
}

/// Start a lead for the lane in the lane's own checkout (made on its first
/// lead, kept for the next): its brief is the lane's, its first prompt the
/// task.
#[allow(clippy::too_many_arguments)]
async fn start_lead(
    deps: &Deps,
    m: &MissionRow,
    lane: &MissionLaneRow,
    item_id: i64,
    role: &str,
    attempt: i64,
    start: StartArgs,
    context: Option<&str>,
    view: &ViewScope,
) -> Result<(String, Option<i64>), IpcError> {
    let (brief, branch) = {
        let s = lock(&deps.store)?;
        let mut tasks = Vec::new();
        for id in &lane.item_ids {
            let (key, title, _) = key_title(&s, *id)?;
            tasks.push(format!("{key} {title}"));
        }
        let branch = lane.branch.clone().unwrap_or_else(|| {
            tickets::branch_slug(&format!("M{}", m.id), &format!("{} {}", m.name, lane.name))
        });
        (lane_brief(m, &lane.name, &tasks), branch)
    };
    let args = StartArgs {
        item_id: Some(item_id),
        reference: None,
        project_id: lane.project_id.or(start.project_id),
        worktree: Some(branch),
        // The lane's checkout may already hold a session on this task (a
        // lead that died, a review beside it): the lead is the lane's.
        parallel: true,
        with_brief: true,
        brief: Some(brief),
        base_branch: None,
        ..start
    };
    let (row, plan, _queued) =
        tickets::start_work_unprompted(&deps.store, &deps.ssh, &deps.reg, &args, view, &deps.net)
            .await?;
    lock(&deps.store)?.set_lane_lead(lane.id, row.id, row.worktree_id, &plan.branch)?;
    let preamble = "Your lane's brief is in your fleet brief: read it first.\n\n";
    let (prompt, task) = begin_task(deps, &row, item_id, role, attempt, context, preamble).await?;
    crate::service::work::resume::spawn_run_prompt(
        std::sync::Arc::clone(&deps.store),
        std::sync::Arc::clone(&deps.ssh),
        &row,
        crate::service::work::report::with_run_instruction(&prompt, task.id, &task.nonce),
        task.id,
    );
    Ok((format!("started lane {}'s lead", lane.name), Some(task.id)))
}

#[cfg(test)]
mod tests;
