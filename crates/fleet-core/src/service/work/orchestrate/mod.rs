//! A mission's loop (orchestration O4–O8, design 2026-10-07 §5, §7):
//! observe → decide → validate → apply, with the store as its only state.
//!
//! - [`steps`] decides the mechanical next steps without a model (§5.3).
//! - [`planner`] asks a locked `claude -p` for judgment (§5.1), and turns
//!   its answer into cards a person decides (the confirm queue, §7.4).
//! - [`autonomy`] is what the loop may do by itself: the least of the
//!   fleet's ceiling (`orchestrator.max_level`), the mission's asked level
//!   and a person's live grant (§7.1). Below [`steps::AUTO_LEVEL`] it only
//!   keeps the cards current; a person presses every step.
//! - [`tick_once`] is one pass of the loop over the missions that are due,
//!   each under its lease, so a hub and a desktop on one store never both
//!   act on one mission.
//!
//! Every step taken, refused or braked is a mission event, so the timeline
//! says what the machine did and why it did not.

pub mod access;
pub mod checks;
pub mod drafts;
pub mod guard;
pub mod integrate;
pub mod lanes;
pub mod limit;
pub mod planner;
pub mod steps;
pub mod touches;
pub mod triage;

use self::planner::{Command, PlannerOutput};
use self::steps::{plan_steps, Step, StepInput, AUTO_LEVEL};
use super::missions::{changeable, mission_id};
use super::WorkLinkArgs;
use crate::cancel::CancellationRegistry;
use crate::ipc_error::{codes, lock, IpcError};
use crate::service::claude_print;
use crate::service::settings;
use crate::service::trackers::tickets::StartArgs;
use crate::service::trackers::TrackerNet;
use crate::service::view_scope::ViewScope;
use crate::ssh::SshClient;
use crate::store::{
    CardRow, Decider, GrantRow, MissionRow, MissionTaskCounts, NewCard, NewGrant, NewMissionEvent,
    Store, TaskRow, WorkItemRow, GRANT_MAX_SECS, GRANT_UNTIL_DONE_AT,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often the loop looks for due missions.
pub const TICK_EVERY: Duration = Duration::from_secs(20);
/// A mission with runs open is looked at again within this, for its brakes.
pub const RUNNING_RECHECK_SECS: i64 = 300;
/// An idle finite mission is looked at again within this.
pub const IDLE_RECHECK_SECS: i64 = 3600;
/// The soonest a mission whose planner waits on a usage limit is looked at
/// again, however near the reset.
pub const RESET_RECHECK_SECS: i64 = 60;
/// A continuous mission's timer when its policy names none (O8).
pub const CONTINUOUS_DEFAULT_WAKE_SECS: u64 = 1800;
/// Cards a mission's detail shows.
pub const CARDS_SHOWN: i64 = 30;

/// What the loop needs to act.
#[derive(Clone)]
pub struct Deps {
    pub store: Arc<Mutex<Store>>,
    pub ssh: Arc<SshClient>,
    pub reg: Arc<CancellationRegistry>,
    pub net: TrackerNet,
}

use crate::store::now_unix;

/// What the loop may do on its own, and why not more.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Autonomy {
    /// The mission's own `level`.
    pub asked: i64,
    /// `orchestrator.max_level`.
    pub ceiling: i64,
    /// What applies.
    pub effective: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<GrantRow>,
    /// The limit that decided, in words.
    pub why: String,
    /// `orchestrator.enabled`.
    pub enabled: bool,
}

/// PURE over the store: the mission's autonomy at `now`.
pub fn autonomy(s: &Store, m: &MissionRow, now: i64) -> Result<Autonomy, IpcError> {
    let enabled = settings::get_bool(s, settings::ORCHESTRATOR_ENABLED);
    let ceiling = settings::get_secs(s, settings::ORCHESTRATOR_MAX_LEVEL).min(3) as i64;
    let grant = s.live_mission_grant(m.id, now)?;
    let granted = grant.as_ref().map(|g| g.level).unwrap_or(1);
    // A live grant lifts a lower ask to its level while it holds (TASK-293):
    // a mission made in the room (L0) runs under its L2 grant, and drops back
    // to its own ask when the grant ends, never past it (review r8).
    let asked = grant.as_ref().map_or(m.level, |g| m.level.max(g.level));
    let effective = asked.min(ceiling).min(granted);
    let why = if !enabled {
        "the mission loop is off (orchestrator.enabled)".to_string()
    } else if effective == m.level {
        format!("L{} as the mission asks", m.level)
    } else if effective == ceiling {
        format!("L{ceiling}, the fleet's ceiling (orchestrator.max_level)")
    } else if grant.is_none() {
        "L1 without a grant: a person presses every step".to_string()
    } else {
        format!("L{granted}, what the grant signs")
    };
    Ok(Autonomy {
        asked: m.level,
        ceiling,
        effective: if enabled { effective } else { 0 },
        grant,
        why,
        enabled,
    })
}

/// A mission's loop, as its detail shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionPlan {
    /// The deterministic next steps (Ready cards).
    #[serde(default)]
    pub steps: Vec<Step>,
    /// The confirm queue: open cards first.
    #[serde(default)]
    pub cards: Vec<CardRow>,
    pub autonomy: Autonomy,
    /// What its workers spent, in micro-USD.
    pub cost_micros: i64,
    pub counts: MissionTaskCounts,
    /// What the next run will likely cost, for the run cards and the spend
    /// ask: this mission's average, else every mission's; absent with no
    /// history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_estimate: Option<RunEstimate>,
}

/// An average run's cost (`Store::run_cost_estimate`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEstimate {
    /// Micro-USD per run.
    pub micros: i64,
    /// How many finished runs it averages.
    pub runs: i64,
    /// `mission` (its own runs) | `fleet` (every mission's, when it has none).
    pub basis: String,
}

/// [`RunEstimate`] for mission `id`.
pub fn run_estimate(s: &Store, id: i64) -> Result<Option<RunEstimate>, IpcError> {
    let own = s.run_cost_estimate(Some(id))?.map(|e| (e, "mission"));
    let any = match own {
        Some(o) => Some(o),
        None => s.run_cost_estimate(None)?.map(|e| (e, "fleet")),
    };
    Ok(any.map(|((micros, runs), basis)| RunEstimate {
        micros,
        runs,
        basis: basis.into(),
    }))
}

/// Fill a listed mission's spend (what it cost, its live grant's budget)
/// and whether it waits on a person (`waiting_on`, G1.6).
pub fn fill_spend(s: &Store, m: &mut MissionRow, now: i64) -> Result<(), IpcError> {
    m.cost_micros = Some(s.mission_cost_micros(m.id)?);
    let grant = s.live_mission_grant(m.id, now)?;
    m.budget_micros = grant.as_ref().and_then(|g| g.budget_micros);
    // Whether it waits on a person (G1.6): only an active mission does, so
    // the others skip the card read.
    m.waiting_on = if m.state == "active" {
        let cards = s.mission_cards(m.id, crate::store::CARDS_OPEN_CAP)?;
        crate::service::attention::mission_waiting(m, grant.is_some(), &cards)
    } else {
        None
    };
    Ok(())
}

/// Every member's attempts, newest first.
fn attempts_of(s: &Store, items: &[WorkItemRow]) -> Result<HashMap<i64, Vec<TaskRow>>, IpcError> {
    let mut out = HashMap::new();
    for i in items {
        out.insert(i.id, s.tasks_for_item(i.id)?);
    }
    Ok(out)
}

/// The parallelism that applies: the policy's, or a grant's when lower.
fn max_parallel(m: &MissionRow, a: &Autonomy) -> u32 {
    let g = a
        .grant
        .as_ref()
        .and_then(|g| g.max_parallel)
        .map(|p| p.max(1) as u32)
        .unwrap_or(u32::MAX);
    m.policy.max_parallel.min(g)
}

/// The mission's steps over every member (the loop's own view: no caller's
/// scope narrows what it plans for).
fn steps_for(s: &Store, m: &MissionRow, a: &Autonomy) -> Result<Vec<Step>, IpcError> {
    let items = s.mission_items(m.id)?;
    let graph = super::graph::build(s, &ViewScope::internal(), &items)?;
    let attempts = attempts_of(s, &items)?;
    let steps = plan_steps(&StepInput {
        mission: m,
        graph: &graph,
        items: &items,
        attempts: &attempts,
        counts: s.mission_task_counts(m.id)?,
        max_parallel: max_parallel(m, a),
    });
    // Lanes L5: what each task says it changes, in which repository (a
    // mission with one repo names it for a task that names none).
    let sole_repo = match m.repos.as_slice() {
        [only] => Some(only.project_id),
        _ => None,
    };
    let said = s.mission_touches(m.id)?;
    let known: HashMap<i64, touches::ItemTouch> = items
        .iter()
        .map(|i| {
            (
                i.id,
                touches::ItemTouch {
                    project: i.project_id.or(sole_repo),
                    touches: said.get(&i.id).cloned().unwrap_or_default(),
                },
            )
        })
        .collect();
    let running: Vec<i64> = attempts
        .iter()
        .filter(|(_, ts)| ts.iter().any(steps::open_impl))
        .map(|(i, _)| *i)
        .collect();
    if m.policy.execution != "lane" {
        return Ok(touches::gate_items(steps, &known, &running, None));
    }
    // Lanes L3: a lane takes one implement run at a time, when it is free.
    let lanes = s.mission_lanes(m.id)?;
    let lane_of: HashMap<i64, i64> = lanes
        .iter()
        .flat_map(|l| l.item_ids.iter().map(move |i| (*i, l.id)))
        .collect();
    let busy = lanes::busy_lanes(s, m, &lanes, &attempts, &lanes::awaiting_test(&graph))?;
    let steps = lanes::gate(steps, &lane_of, &busy);
    // Lanes L5: a lane starts beside another of its repository only when
    // both say all they change and it does not meet.
    let done = |i: &i64| {
        items
            .iter()
            .any(|it| it.id == *i && it.status_category == "done")
    };
    let units: Vec<touches::LaneTouch> = lanes
        .iter()
        .map(|l| {
            let mut paths: Vec<String> = Vec::new();
            for i in &l.item_ids {
                for p in said.get(i).into_iter().flatten() {
                    if !paths.contains(p) {
                        paths.push(p.clone());
                    }
                }
            }
            touches::LaneTouch {
                id: l.id,
                project: l.project_id.or_else(|| {
                    l.item_ids
                        .iter()
                        .find_map(|i| known.get(i).and_then(|k| k.project))
                }),
                touches: paths,
                complete: l.item_ids.iter().all(|i| said.contains_key(i)),
                under_way: (l.lead_session_id.is_some()
                    || l.item_ids
                        .iter()
                        .any(|i| attempts.get(i).is_some_and(|ts| !ts.is_empty())))
                    && !l.item_ids.iter().all(done),
            }
        })
        .collect();
    let steps = touches::gate_lane_starts(steps, &lane_of, &units);
    let laneless: std::collections::HashSet<i64> = items
        .iter()
        .map(|i| i.id)
        .filter(|i| !lane_of.contains_key(i))
        .collect();
    Ok(touches::gate_items(
        steps,
        &known,
        &running,
        Some(&laneless),
    ))
}

/// The plan a mission's detail carries: `None` for a draft or a finished
/// one.
pub fn plan_for(s: &Store, m: &MissionRow) -> Result<Option<MissionPlan>, IpcError> {
    if !matches!(m.state.as_str(), "active" | "paused") || !crate::store::mode_runs_loop(&m.mode) {
        return Ok(None);
    }
    let a = autonomy(s, m, now_unix())?;
    Ok(Some(MissionPlan {
        steps: steps_for(s, m, &a)?,
        cards: s.mission_cards(m.id, CARDS_SHOWN)?,
        cost_micros: s.mission_cost_micros(m.id)?,
        counts: s.mission_task_counts(m.id)?,
        run_estimate: run_estimate(s, m.id)?,
        autonomy: a,
    }))
}

/// Refuse the loop's own acts (Start wave, the planner, a grant) on a
/// `plan` mission: people and their own sessions work a plan through.
fn not_a_plan(m: &MissionRow) -> Result<(), IpcError> {
    if crate::store::mode_runs_loop(&m.mode) {
        return Ok(());
    }
    Err(IpcError::new(
        codes::E_INVALID_STATE,
        format!(
            "{} is a plan: fleet tracks it but does not run it; make it finite to run it",
            m.name
        ),
    ))
}

/// Who takes a step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    /// A person, as `person:<id>` (or `fleet` for the desktop's own).
    Person(String),
    /// The loop, under a grant.
    Loop,
}

impl Actor {
    fn name(&self) -> String {
        match self {
            Actor::Person(p) => p.clone(),
            Actor::Loop => "loop".into(),
        }
    }
}

fn event(
    s: &Store,
    m: i64,
    kind: &str,
    actor: &str,
    item: Option<i64>,
    payload: serde_json::Value,
) {
    let e = NewMissionEvent {
        kind,
        actor,
        work_item_id: item,
        payload: Some(payload),
        ..Default::default()
    };
    if let Err(err) = s.record_mission_event(m, &e) {
        tracing::debug!(mission = m, error = %err.message, "[orchestrate] event not recorded");
    }
}

/// The start a mission's run takes: its item, inside the mission's repo
/// allow-list, on a host its grant names, in its owner's name.
fn start_for(
    s: &Store,
    m: &MissionRow,
    item_id: i64,
    actor: &Actor,
    grant: Option<&GrantRow>,
) -> Result<StartArgs, IpcError> {
    let item = s
        .get_work_item(item_id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("work item {item_id}")))?;
    let allowed: Vec<i64> = m.repos.iter().map(|r| r.project_id).collect();
    let project_id = match (item.project_id, allowed.as_slice()) {
        (Some(p), []) => Some(p),
        (Some(p), list) if list.contains(&p) => Some(p),
        (Some(_), _) => {
            return Err(IpcError::new(
                codes::E_FORBIDDEN,
                format!(
                    "{} is in a repo outside {}'s allow-list: add its repo to the \
                     mission's repos, or take the task out of the mission",
                    item.key.as_deref().unwrap_or(&item.title),
                    m.name
                ),
            ))
        }
        (None, [only]) => Some(*only),
        (None, _) => None,
    };
    let (host_alias, profile) = grant_placement(s, grant, project_id);
    // A run that builds on earlier work (a retry, a review, a test) goes
    // where that work is when the grant allows that host and it is up
    // (lanes design P2, `run::prior_work`), billing the grant's own login.
    let (host_alias, profile) = match super::run::prior_work(s, item_id)? {
        Some(p)
            if p.host_up
                && host_alias.as_deref() != Some(p.host_alias.as_str())
                && grant
                    .and_then(|g| g.hosts.as_ref())
                    .is_some_and(|h| h.contains(&p.host_alias)) =>
        {
            (Some(p.host_alias), grant.and_then(|g| g.profile.clone()))
        }
        _ => (host_alias, profile),
    };
    Ok(StartArgs {
        item_id: Some(item_id),
        project_id,
        host_alias,
        with_brief: true,
        decider: match actor {
            Actor::Person(_) => Decider::Person,
            Actor::Loop => Decider::Agent,
        },
        owner: m.owner_person_id,
        // The run's session is the mission's, whoever pressed Go
        // (migration 124): the origin chip names the mission.
        origin: Some(crate::store::SessionOrigin::mission(m.id)),
        // The login the grant names (redesign 8.7); else placement's pick
        // on the chosen host; none, the host's own.
        profile,
        ..Default::default()
    })
}

/// Where a run inside `grant` goes (control review F5): the best host and
/// login among the grant's hosts (`placement::best_in_grant`), its own
/// login when it names one. Nothing in the grant qualifies, or no grant
/// hosts: its first host (or none, for the tracker's) and its login, so the
/// limit hold still says why.
fn grant_placement(
    s: &Store,
    grant: Option<&GrantRow>,
    project_id: Option<i64>,
) -> (Option<String>, Option<String>) {
    let named = grant.and_then(|g| g.profile.clone());
    let Some(hosts) = grant
        .and_then(|g| g.hosts.as_ref())
        .filter(|h| !h.is_empty())
    else {
        return (None, named);
    };
    match crate::service::placement::best_in_grant(
        s,
        project_id,
        hosts,
        named.as_deref(),
        crate::store::now_unix(),
    ) {
        Some(c) => (Some(c.host_alias), named.or(c.profile)),
        None => (hosts.first().cloned(), named),
    }
}

/// What taking one step did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepResult {
    pub step: Step,
    pub ok: bool,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<i64>,
}

/// Take one step of `m`. A run goes through the start path under `view`
/// (a person's own scope, or the loop's internal one).
pub async fn apply_step(
    deps: &Deps,
    m: &MissionRow,
    step: &Step,
    actor: &Actor,
    view: &ViewScope,
) -> StepResult {
    let res = apply_step_inner(deps, m, step, actor, view).await;
    // A start the loop cannot make by itself (outside the repo allow-list,
    // another organisation's host, a host the grant does not reach) waits on
    // a person: ask, rather than only log a refusal on every tick.
    let ask = match (&res, actor) {
        (Err(e), Actor::Loop) if e.code == codes::E_FORBIDDEN => Some(e.message.clone()),
        _ => None,
    };
    let (ok, detail, task_id) = match res {
        Ok((d, t)) => (true, d, t),
        Err(e) => (false, e.message, None),
    };
    if let Ok(s) = deps.store.lock() {
        if let Some(why) = ask {
            let _ = ask_card(
                &s,
                m.id,
                &Step {
                    kind: "ask".into(),
                    item_id: step.item_id,
                    role: None,
                    reason: format!("The loop cannot start {}: {why}", step.kind),
                    context: None,
                    auto: false,
                },
            );
        }
        event(
            &s,
            m.id,
            if ok { "step" } else { "refused" },
            &actor.name(),
            step.item_id,
            serde_json::json!({ "step": step.kind, "role": step.role, "detail": detail, "task_id": task_id }),
        );
    }
    StepResult {
        step: step.clone(),
        ok,
        detail,
        task_id,
    }
}

async fn apply_step_inner(
    deps: &Deps,
    m: &MissionRow,
    step: &Step,
    actor: &Actor,
    view: &ViewScope,
) -> Result<(String, Option<i64>), IpcError> {
    match step.kind.as_str() {
        "run" | "retry" | "review" | "test" | "integrate" => {
            let item_id = step
                .item_id
                .ok_or_else(|| IpcError::new(codes::E_INVALID, "a run step names its item"))?;
            let (start, grant) = {
                let s = lock(&deps.store)?;
                let a = autonomy(&s, m, now_unix())?;
                if *actor == Actor::Loop {
                    // The pass planned its steps once and each start is SSH:
                    // Pause all, a paused mission, the loop switched off or a
                    // lowered level stops the rest of the pass, not the next
                    // one (review r06 F5).
                    let current = s.get_mission(m.id)?;
                    let live = match &current {
                        Some(cur) => autonomy(&s, cur, now_unix())?.effective >= AUTO_LEVEL,
                        None => false,
                    };
                    if crate::service::loops::paused(&s)
                        || current.is_none_or(|cur| cur.state != "active")
                        || !live
                    {
                        return Err(IpcError::new(
                            codes::E_INVALID_STATE,
                            "the mission loop stood down during its pass",
                        ));
                    }
                    if let Some(budget) = a.grant.as_ref().and_then(|g| g.budget_micros) {
                        if s.mission_cost_micros(m.id)? >= budget {
                            return Err(IpcError::new(
                                codes::E_LIMIT,
                                "the grant's budget is spent",
                            ));
                        }
                    }
                    // `plan_steps` counts slots and attempts for its own
                    // steps; a planner card the loop applies did not, so
                    // the same limits hold here.
                    if step.kind != "integrate" {
                        let counts = s.mission_task_counts(m.id)?;
                        let cap = max_parallel(m, &a);
                        if counts.open >= i64::from(cap) {
                            return Err(IpcError::new(
                                codes::E_LIMIT,
                                format!("the mission already runs {cap} at once"),
                            ));
                        }
                        if counts.total >= i64::from(m.policy.max_tasks) {
                            return Err(IpcError::new(
                                codes::E_LIMIT,
                                format!("the mission used its {} attempts", m.policy.max_tasks),
                            ));
                        }
                    }
                    if s.get_work_item(item_id)?
                        .is_some_and(|i| i.held_at.is_some())
                    {
                        return Err(IpcError::new(
                            codes::E_INVALID_STATE,
                            "a person holds this item",
                        ));
                    }
                }
                (start_for(&s, m, item_id, actor, a.grant.as_ref())?, a.grant)
            };
            let role = step.role.as_deref().unwrap_or("implement");
            // Lanes L3: a lane's task runs in the lane's lead session.
            if role == "implement" && m.policy.execution == "lane" {
                let lane = lock(&deps.store)?.item_lane(item_id)?;
                if let Some(lane) = lane {
                    return lanes::run_in_lane(
                        deps,
                        m,
                        lane,
                        item_id,
                        role,
                        start,
                        step.context.as_deref(),
                        view,
                    )
                    .await;
                }
            }
            // F3: a review or a test, headless in the implementer's
            // checkout, when the setting is on and the checkout allows.
            if let Some(started) = checks::try_start(
                deps,
                m,
                item_id,
                role,
                start.profile.as_deref(),
                grant.as_ref().and_then(|g| g.hosts.as_deref()),
                view,
                step.context.as_deref(),
            )
            .await?
            {
                return Ok((started.0, Some(started.1)));
            }
            let out = super::run::run_item_with(
                &deps.store,
                &deps.ssh,
                &deps.reg,
                &start,
                role,
                view,
                &deps.net,
                step.context.as_deref(),
            )
            .await?;
            Ok((
                if out.existing {
                    format!("a {role} run was already open")
                } else {
                    format!("started a {role} run")
                },
                Some(out.task.id),
            ))
        }
        "close" => {
            let item_id = step
                .item_id
                .ok_or_else(|| IpcError::new(codes::E_INVALID, "a close step names its item"))?;
            let s = lock(&deps.store)?;
            // The completion gate's evidence: the loop never closes an item
            // whose implement run shows no commits or files. A person's
            // press is never refused (they see it on the card).
            if *actor == Actor::Loop {
                if let Some(e) =
                    crate::service::work::gate::item_run_evidence(&s, item_id, now_unix())?
                        .filter(|e| e.state == "missing" || e.state == "pending")
                {
                    return Err(IpcError::new(
                        codes::E_INVALID_STATE,
                        format!("no run evidence: {}", e.detail),
                    ));
                }
            }
            s.set_item_status(item_id, "done")?;
            s.wake_mission(m.id)?;
            Ok(("closed".into(), None))
        }
        "complete" => {
            // A person completes a mission, never the loop (F19).
            if matches!(actor, Actor::Loop) {
                return Err(IpcError::new(
                    codes::E_FORBIDDEN,
                    "a person completes a mission; the loop only shows it is due",
                ));
            }
            let s = lock(&deps.store)?;
            // Red CI on its pull requests holds the step; only the
            // mission's own Complete takes a person's override.
            if let Some(e) =
                super::missions::ci_step_refusal(&super::missions::red_prs(&s, view, m.id)?)
            {
                return Err(e);
            }
            if let Some(root) = m.root_item_id {
                s.set_item_status(root, "done")?;
            }
            s.set_mission_state(m.id, None, "completed", &actor.name())?;
            Ok(("the mission is complete".into(), None))
        }
        "ask" => {
            let s = lock(&deps.store)?;
            ask_card(&s, m.id, step)?;
            Ok(("asked a person".into(), None))
        }
        other => Err(IpcError::new(
            codes::E_INVALID,
            format!("no step {other:?}"),
        )),
    }
}

/// The card an `ask` step raises: one per item and reason, however often
/// the loop sees it.
fn ask_card(s: &Store, m: i64, step: &Step) -> Result<Option<CardRow>, IpcError> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    step.reason.hash(&mut h);
    let decision = format!("loop:ask:{}:{:x}", step.item_id.unwrap_or(0), h.finish());
    s.add_card(
        m,
        &NewCard {
            decision_id: &decision,
            source: "loop",
            kind: "ask",
            work_item_id: step.item_id,
            payload: Some(serde_json::json!({ "question": step.reason, "context": step.context })),
        },
    )
}

/// What `mission_start` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartOutcome {
    pub mission_id: i64,
    #[serde(default)]
    pub results: Vec<StepResult>,
}

/// `work_link { action: mission_start, mission_id, step? }`: a person takes
/// the mission's current steps (*Start wave*), or the one `step` names
/// (`run:12`, `close:12`, `complete:0`). Asks are not steps a press takes.
pub async fn start(
    args: &WorkLinkArgs,
    deps: &Deps,
    scope: &ViewScope,
) -> Result<StartOutcome, IpcError> {
    let id = mission_id(args)?;
    let (m, steps) = {
        let s = lock(&deps.store)?;
        let m = changeable(&s, scope, id)?;
        not_a_plan(&m)?;
        if m.state != "active" {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!("{} is {}; start it first", m.name, m.state),
            ));
        }
        let a = autonomy(&s, &m, now_unix())?;
        (m.clone(), steps_for(&s, &m, &a)?)
    };
    let chosen: Vec<Step> = match args.step.as_deref() {
        Some(k) => {
            let st = steps.into_iter().find(|s| s.key() == k).ok_or_else(|| {
                IpcError::new(
                    codes::E_INVALID_STATE,
                    format!("{k} is not one of the mission's next steps now"),
                )
            })?;
            vec![st]
        }
        None => steps.into_iter().filter(|s| s.kind != "ask").collect(),
    };
    let actor = Actor::Person(super::graph::actor(scope));
    let mut results = Vec::with_capacity(chosen.len());
    for st in &chosen {
        results.push(apply_step(deps, &m, st, &actor, scope).await);
    }
    Ok(StartOutcome {
        mission_id: id,
        results,
    })
}

/// `work_link { action: retry, item_id, note? }`: a person runs an item's
/// implementation again, with its last failure (and their note) in the
/// prompt. A person's retry is not held to the policy's retry count.
pub async fn retry(
    args: &WorkLinkArgs,
    deps: &Deps,
    scope: &ViewScope,
) -> Result<StepResult, IpcError> {
    let item_id = args
        .item_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "retry needs item_id"))?;
    let (m, context) = {
        let s = lock(&deps.store)?;
        super::graph::visible_item(&s, scope, item_id)?;
        let mid = s.item_mission(item_id)?.ok_or_else(|| {
            IpcError::new(
                codes::E_INVALID_STATE,
                format!("work item {item_id} is in no mission; run it instead"),
            )
        })?;
        let m = changeable(&s, scope, mid)?;
        let last = s
            .latest_item_task(item_id, "implement")?
            .map(|t| {
                [
                    t.error.clone(),
                    t.report.as_ref().map(|r| r.summary.clone()),
                ]
                .into_iter()
                .flatten()
                .filter(|x| !x.trim().is_empty())
                .collect::<Vec<_>>()
                .join(" — ")
            })
            .unwrap_or_default();
        let note = args.note.as_deref().map(str::trim).unwrap_or("");
        let context = match (last.is_empty(), note.is_empty()) {
            (true, true) => String::new(),
            (false, true) => format!("The last attempt: {last}"),
            (true, false) => format!("A person's note: {note}"),
            (false, false) => format!("The last attempt: {last}\nA person's note: {note}"),
        };
        (m, context)
    };
    let step = Step {
        kind: "retry".into(),
        item_id: Some(item_id),
        role: Some("implement".into()),
        reason: "a person asked for another attempt".into(),
        context: (!context.is_empty()).then_some(context),
        auto: false,
    };
    let actor = Actor::Person(super::graph::actor(scope));
    Ok(apply_step(deps, &m, &step, &actor, scope).await)
}

/// Apply one card. `Ok(detail)`: what it did.
async fn apply_card(
    deps: &Deps,
    m: &MissionRow,
    card: &CardRow,
    actor: &Actor,
    view: &ViewScope,
    answer: Option<&str>,
    accept_created: bool,
) -> Result<String, IpcError> {
    let p = card.payload.clone().unwrap_or(serde_json::Value::Null);
    let item = card.work_item_id;
    let need_item = || {
        item.ok_or_else(|| {
            IpcError::new(
                codes::E_INVALID,
                format!("a {} card names its item", card.kind),
            )
        })
    };
    match card.kind.as_str() {
        "create" => {
            let root = m.root_item_id.ok_or_else(|| {
                IpcError::new(
                    codes::E_INVALID_STATE,
                    "the mission has no root to plan under",
                )
            })?;
            let tree: Vec<crate::store::TreeEntry> =
                serde_json::from_value(p.get("tree").cloned().unwrap_or_default())
                    .map_err(|e| IpcError::new(codes::E_INVALID, e.to_string()))?;
            let lines: Vec<Vec<String>> =
                serde_json::from_value(p.get("done_when").cloned().unwrap_or_default())
                    .unwrap_or_default();
            let lanes: Vec<Option<String>> =
                serde_json::from_value(p.get("lanes").cloned().unwrap_or_default())
                    .unwrap_or_default();
            let touches: Vec<Vec<String>> =
                serde_json::from_value(p.get("touches").cloned().unwrap_or_default())
                    .unwrap_or_default();
            let s = lock(&deps.store)?;
            let made = s.propose_tree(root, &tree, "planner")?;
            for (i, row) in made.iter().enumerate() {
                if let Some(l) = lines.get(i).filter(|l| !l.is_empty()) {
                    s.set_item_done_when(row.id, l, &actor.name())?;
                }
                // Lanes design L2: the lane the planner named.
                if let Some(l) = lanes.get(i).and_then(Option::as_deref) {
                    s.set_item_lane(m.id, row.id, Some(l), row.project_id, &actor.name())?;
                }
                // Lanes design L5: what it said the task changes.
                if let Some(t) = touches.get(i).filter(|t| !t.is_empty()) {
                    s.set_item_touches(row.id, t)?;
                }
            }
            if accept_created {
                let ids: Vec<i64> = made.iter().map(|r| r.id).collect();
                s.accept_proposals(&ids)?;
                Ok(format!("created {} tasks", made.len()))
            } else {
                drop(s);
                // K4: whether each repeats an existing task (off by default).
                crate::service::decide::duplicate::spawn_ask(
                    &deps.store,
                    made.iter().map(|r| r.id).collect(),
                );
                Ok(format!("proposed {} tasks", made.len()))
            }
        }
        "add_dep" | "remove_dep" => {
            let on = p
                .get("depends_on")
                .and_then(|x| x.as_i64())
                .ok_or_else(|| IpcError::new(codes::E_INVALID, "a dep card names depends_on"))?;
            let s = lock(&deps.store)?;
            let it = need_item()?;
            if card.kind == "add_dep" {
                s.add_item_dep(it, on, "planner", &actor.name())?;
            } else {
                s.remove_item_dep(it, on, &actor.name())?;
            }
            Ok(format!("{} {it} → {on}", card.kind))
        }
        "run" | "retry" => {
            let role = p
                .get("role")
                .and_then(|x| x.as_str())
                .unwrap_or("implement");
            let step = Step {
                kind: card.kind.clone(),
                item_id: Some(need_item()?),
                role: Some(role.into()),
                reason: "the planner".into(),
                context: p.get("note").and_then(|x| x.as_str()).map(str::to_string),
                auto: true,
            };
            let r = apply_step(deps, m, &step, actor, view).await;
            if r.ok {
                Ok(r.detail)
            } else {
                Err(IpcError::new(codes::E_INVALID_STATE, r.detail))
            }
        }
        "cancel" => {
            let it = need_item()?;
            let (n, workers) = {
                let s = lock(&deps.store)?;
                let mut n = 0;
                let mut workers = Vec::new();
                for t in s.tasks_for_item(it)? {
                    if matches!(t.state.as_str(), "queued" | "running") {
                        crate::service::tasks::cancel_task(&s, t.id, "the mission cancelled it")?;
                        n += 1;
                        // Only the mission's own worker: a cancelled row
                        // whose session kept editing would race the retry.
                        let own = crate::store::SessionOrigin::mission(m.id);
                        if let Some(w) = t
                            .worker_session_id
                            .map(|id| s.get_session_by_id(id))
                            .transpose()?
                            .flatten()
                            .filter(|w| {
                                w.status == "running"
                                    && w.origin.as_deref() == Some(own.origin)
                                    && w.origin_ref == own.origin_ref
                            })
                        {
                            workers.push((w.host_alias, w.tmux_name));
                        }
                    }
                }
                (n, workers)
            };
            for (host_alias, name) in workers {
                if let Err(e) = crate::service::sessions::kill_session(
                    crate::service::sessions::KillSessionArgs {
                        host_alias: host_alias.clone(),
                        name: name.clone(),
                        force: false,
                    },
                    &deps.store,
                    &deps.ssh,
                )
                .await
                {
                    tracing::warn!(host = %host_alias, session = %name, error = %e,
                        "[mission] a cancelled run's worker was not stopped");
                }
            }
            Ok(format!("cancelled {n} runs"))
        }
        "hold" => {
            let s = lock(&deps.store)?;
            s.set_item_hold(need_item()?, true, &actor.name())?;
            Ok("held".into())
        }
        "ask" => {
            let a = answer
                .map(str::trim)
                .filter(|a| !a.is_empty())
                .ok_or_else(|| {
                    IpcError::new(codes::E_INVALID, "a question is answered with a note")
                })?;
            let s = lock(&deps.store)?;
            s.wake_mission(m.id)?;
            Ok(format!(
                "answered: {}",
                a.chars().take(200).collect::<String>()
            ))
        }
        "complete" => {
            let ok = {
                let s = lock(&deps.store)?;
                let a = autonomy(&s, m, now_unix())?;
                steps_for(&s, m, &a)?.iter().any(|st| st.kind == "complete")
            };
            if !ok {
                return Err(IpcError::new(
                    codes::E_INVALID_STATE,
                    "not every task is done and verified",
                ));
            }
            let step = Step {
                kind: "complete".into(),
                item_id: m.root_item_id,
                role: None,
                reason: "the planner".into(),
                context: None,
                auto: true,
            };
            let r = apply_step(deps, m, &step, actor, view).await;
            if r.ok {
                Ok(r.detail)
            } else {
                Err(IpcError::new(codes::E_INVALID_STATE, r.detail))
            }
        }
        other => Err(IpcError::new(
            codes::E_INVALID,
            format!("no card {other:?}"),
        )),
    }
}

/// A card being decided right now, released when the decision is written
/// (or fails). Process-wide: one hub decides each card once.
#[derive(Debug)]
struct CardClaim(i64);

static CARDS_IN_FLIGHT: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<i64>>> =
    std::sync::LazyLock::new(Default::default);

impl CardClaim {
    fn take(id: i64) -> Result<Self, IpcError> {
        let mut held = CARDS_IN_FLIGHT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !held.insert(id) {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!("card {id} is being decided already"),
            ));
        }
        Ok(Self(id))
    }
}

impl Drop for CardClaim {
    fn drop(&mut self) {
        CARDS_IN_FLIGHT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.0);
    }
}

/// `work_link { action: card_decide, card_id, ok, note? }`: a person
/// applies a card (`ok: true`) or dismisses it. A question is answered by
/// applying it with the answer as `note`.
pub async fn decide_card(
    args: &WorkLinkArgs,
    deps: &Deps,
    scope: &ViewScope,
) -> Result<CardRow, IpcError> {
    super::graph::person_decides(scope)?;
    let id = args
        .card_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "card_decide needs card_id"))?;
    let ok = args
        .ok
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "card_decide needs ok (true or false)"))?;
    let (m, card) = {
        let s = lock(&deps.store)?;
        let card = s
            .get_card(id)?
            .ok_or_else(|| crate::service::orgs::not_found("card", id))?;
        let m = match changeable(&s, scope, card.mission_id) {
            Ok(m) => m,
            Err(e) if e.code == codes::E_NOTFOUND => {
                return Err(crate::service::orgs::not_found("card", id))
            }
            Err(e) => return Err(e),
        };
        if card.state != "open" {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!("card {id} is {} already", card.state),
            ));
        }
        (m, card)
    };
    // Claimed before applying: `apply_card` runs with the store unlocked
    // (it may dispatch a run or propose a tree), so the desktop and the
    // phone pressing Apply together both passed the `open` check above and
    // both applied the card. The second now hears it is being decided.
    let _claim = CardClaim::take(id)?;
    let who = super::graph::actor(scope);
    let (state, note) = if ok {
        match apply_card(
            deps,
            &m,
            &card,
            &Actor::Person(who.clone()),
            scope,
            args.note.as_deref(),
            true,
        )
        .await
        {
            Ok(d) => (
                "applied",
                Some(if card.kind == "ask" {
                    args.note.clone().unwrap_or(d)
                } else {
                    d
                }),
            ),
            Err(e) => {
                if e.code == codes::E_INVALID && card.kind == "ask" {
                    return Err(e);
                }
                ("refused", Some(e.message))
            }
        }
    } else {
        ("dismissed", args.note.clone())
    };
    let s = lock(&deps.store)?;
    if !s.decide_card(id, state, &who, note.as_deref())? {
        // Closed meanwhile by something other than a person's decision (a
        // stale sweep): say so rather than report a decision not recorded.
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("card {id} was closed while it was being decided"),
        ));
    }
    event(
        &s,
        m.id,
        "card",
        &who,
        card.work_item_id,
        serde_json::json!({ "card_id": id, "kind": card.kind, "state": state }),
    );
    s.wake_mission(m.id)?;
    s.get_card(id)?
        .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "card vanished"))
}

/// What a planner call did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanOutcome {
    pub mission_id: i64,
    #[serde(default)]
    pub cards: Vec<CardRow>,
    /// Why the answer was refused, when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    /// The usage limit the ask ended on (TASK-291): the mission's wait when
    /// every login the planner may use is at one, else the last login's
    /// when its tries ran out. Not a refused answer; the loop asks again
    /// once a login has headroom.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting: Option<limit::LimitWait>,
}

/// The project whose checkout the planner reads (`orchestrator.planner_reads`,
/// lanes design L1): the root item's, else the mission's first repo. `None`
/// when the setting is off or the mission names no repository.
fn planner_reads_project(s: &Store, m: &MissionRow) -> Result<Option<i64>, IpcError> {
    if !crate::service::settings::get_bool(s, crate::service::settings::ORCHESTRATOR_PLANNER_READS)
    {
        return Ok(None);
    }
    let root = match m.root_item_id {
        Some(r) => s.get_work_item(r)?.and_then(|i| i.project_id),
        None => None,
    };
    Ok(root.or_else(|| m.repos.first().map(|r| r.project_id)))
}

/// The host the planner runs on: the policy's, the grant's first, else the
/// host of the mission's latest worker that may see the mission's org.
///
/// Every LLM run of a mission (the planner, its release note, its triage
/// card) is sent the mission's text, so the host must be one of the
/// mission's org (transition plan, risk "Org text leaves the org"): a policy
/// or grant host that is not refuses with `E_FORBIDDEN`, and a worker on
/// such a host is passed over.
fn planner_host(s: &Store, m: &MissionRow, grant: Option<&GrantRow>) -> Result<String, IpcError> {
    Ok(planner_hosts_up_to(s, m, grant, 1)?.swap_remove(0))
}

/// Every host the planner may run on, [`planner_host`]'s first (TASK-291:
/// the rest are where it goes when that one's logins are at their usage
/// limit): the policy's host alone, else the grant's hosts of the
/// mission's org, else the hosts of the mission's workers that see it, by
/// item and task. Never empty.
fn planner_hosts(
    s: &Store,
    m: &MissionRow,
    grant: Option<&GrantRow>,
) -> Result<Vec<String>, IpcError> {
    planner_hosts_up_to(s, m, grant, usize::MAX)
}

/// [`planner_hosts`], the first `max` of them: the walk of the mission's
/// workers stops there, and looks each worker session and host up once
/// (review r8: it ran under the store lock on every limit check).
fn planner_hosts_up_to(
    s: &Store,
    m: &MissionRow,
    grant: Option<&GrantRow>,
    max: usize,
) -> Result<Vec<String>, IpcError> {
    if let Some(h) = &m.policy.planner_host {
        require_host_sees_org(s, h, m.org_id, "policy.planner_host")?;
        return Ok(vec![h.clone()]);
    }
    if let Some(hosts) = grant
        .and_then(|g| g.hosts.as_ref())
        .filter(|h| !h.is_empty())
    {
        require_host_sees_org(s, &hosts[0], m.org_id, "the grant's first host")?;
        let mut out = vec![hosts[0].clone()];
        for h in &hosts[1..] {
            if !out.contains(h) && host_sees_org(s, h, m.org_id)? {
                out.push(h.clone());
            }
        }
        return Ok(out);
    }
    let mut out: Vec<String> = Vec::new();
    let mut passed_over = None;
    let mut seen_sessions = std::collections::HashSet::new();
    let mut seen_hosts = std::collections::HashSet::new();
    'walk: for i in s.mission_items(m.id)? {
        for t in s.tasks_for_item(i.id)? {
            let Some(sid) = t.worker_session_id.filter(|w| seen_sessions.insert(*w)) else {
                continue;
            };
            let Some(w) = s.get_session_by_id(sid).ok().flatten() else {
                continue;
            };
            if !seen_hosts.insert(w.host_alias.clone()) {
                continue;
            }
            if host_sees_org(s, &w.host_alias, m.org_id)? {
                out.push(w.host_alias);
                if out.len() >= max {
                    break 'walk;
                }
            } else {
                passed_over.get_or_insert(w.host_alias);
            }
        }
    }
    if !out.is_empty() {
        return Ok(out);
    }
    if let Some(h) = passed_over {
        return Err(other_org_host(&h, "its workers' host"));
    }
    Err(IpcError::new(
        codes::E_INVALID_STATE,
        "no host for the planner: set the mission's policy.planner_host",
    ))
}

/// Whether `host` may be sent text of `org`: the host's own org scope sees
/// it (the rule the ticket brief keeps, `tickets::brief_visible_on`).
fn host_sees_org(s: &Store, host: &str, org: Option<i64>) -> Result<bool, IpcError> {
    Ok(crate::service::orgs::OrgScope::for_host(s, host)?.sees_org(org))
}

/// [`host_sees_org`], or `E_FORBIDDEN` naming the host and where it came
/// from.
fn require_host_sees_org(
    s: &Store,
    host: &str,
    org: Option<i64>,
    from: &str,
) -> Result<(), IpcError> {
    if host_sees_org(s, host, org)? {
        Ok(())
    } else {
        Err(other_org_host(host, from))
    }
}

fn other_org_host(host: &str, from: &str) -> IpcError {
    IpcError::new(
        codes::E_FORBIDDEN,
        format!(
            "{host} ({from}) is not a host of this mission's organisation, so the \
             mission's text cannot be sent there; pick a host of the same organisation"
        ),
    )
}

/// Book a planner run's cost on its mission (redesign 8.2), so the budget
/// brake counts it. `usage` is `None` when `claude` reported none: the run
/// is still booked, at 0. A failed write is logged, never the plan's error.
pub fn book_planner_run(
    s: &Store,
    m: &MissionRow,
    host: &str,
    model: &str,
    usage: Option<&crate::service::claude_print::Envelope>,
    now: i64,
) {
    let row = crate::store::NewAuxUsage {
        origin: crate::store::AUX_ORIGIN_PLANNER,
        host_alias: host.to_string(),
        model: model.to_string(),
        mission_id: Some(m.id),
        org_id: m.org_id,
        claude_session_id: None,
        input_tokens: usage.and_then(|u| u.input_tokens),
        output_tokens: usage.and_then(|u| u.output_tokens),
        cost_micros: usage.and_then(|u| u.cost_microusd).unwrap_or(0),
        at: now,
    };
    if let Err(e) = s.insert_aux_usage(&row) {
        tracing::warn!(mission = m.id, error = %e.message, "[orchestrate] planner cost not booked");
    }
}

/// Whether the loop applies a planner card of `kind` itself at
/// `auto_level`. Never `complete` (nor `ask`): those wait for a person at
/// every level ([`steps::PERSON_ONLY_STEPS`]).
fn loop_applies_card(kind: &str, auto_level: i64) -> bool {
    match kind {
        "create" => auto_level >= 1,
        "run" | "retry" | "add_dep" | "remove_dep" | "hold" | "cancel" => auto_level >= AUTO_LEVEL,
        _ => false,
    }
}

/// The missions with a planner call in flight in this process, keyed by the
/// store they live in as well as their id: two stores in one process (the
/// test suite's, run in parallel) hand out the same mission ids, and one
/// test's slot must not refuse another's planner.
static PLANNING: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<(usize, i64)>>> =
    std::sync::LazyLock::new(Default::default);

/// Holds a mission's planner slot while its call runs (review r01): a
/// person's `mission_plan` and the loop's own call, or two presses, do not
/// ask the planner twice at once and card its answer twice.
struct PlannerSlot((usize, i64));

impl PlannerSlot {
    fn take(deps: &Deps, m: &MissionRow) -> Result<Self, IpcError> {
        let key = (Arc::as_ptr(&deps.store) as usize, m.id);
        let mut running = PLANNING.lock().unwrap_or_else(|e| e.into_inner());
        if !running.insert(key) {
            return Err(IpcError::new(
                codes::E_EXISTS,
                format!(
                    "the planner is already running for {}; try again when it ends",
                    m.name
                ),
            ));
        }
        Ok(Self(key))
    }
}

impl Drop for PlannerSlot {
    fn drop(&mut self) {
        PLANNING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// The planner's cards the loop applies itself at `auto_level`, in order;
/// the rest are returned as they are, for a person.
async fn loop_apply_cards(
    deps: &Deps,
    m: &MissionRow,
    cards: Vec<CardRow>,
    auto_level: i64,
    accept_created: bool,
) -> Result<Vec<CardRow>, IpcError> {
    let mut shown = Vec::with_capacity(cards.len());
    for c in cards {
        if loop_applies_card(&c.kind, auto_level) {
            // Each earlier card's apply may take an SSH start, so this one
            // sat `open` meanwhile, where a person can dismiss or apply it.
            // Claimed as a person's decision is, and re-read: a card decided
            // in that window is theirs, never applied a second time.
            let Ok(_claim) = CardClaim::take(c.id) else {
                shown.push(c);
                continue;
            };
            let current = lock(&deps.store)?.get_card(c.id)?;
            match current {
                Some(card) if card.state == "open" => {}
                other => {
                    shown.push(other.unwrap_or(c));
                    continue;
                }
            }
            let res = apply_card(
                deps,
                m,
                &c,
                &Actor::Loop,
                &ViewScope::internal(),
                None,
                accept_created,
            )
            .await;
            let s = lock(&deps.store)?;
            let (state, note) = match res {
                Ok(d) => ("applied", d),
                Err(e) => ("refused", e.message),
            };
            s.decide_card(c.id, state, "loop", Some(&note))?;
            shown.push(s.get_card(c.id)?.unwrap_or(c));
        } else {
            shown.push(c);
        }
    }
    Ok(shown)
}

/// Ask the planner about `m` now, and card its answer.
pub async fn run_planner(
    deps: &Deps,
    m: &MissionRow,
    why: &str,
    actor: &Actor,
) -> Result<PlanOutcome, IpcError> {
    let _slot = PlannerSlot::take(deps, m)?;
    let now = now_unix();
    let (mut login, grant, snap, reads, model, auto_level, accept_created, plan_nodes) = {
        let s = lock(&deps.store)?;
        let a = autonomy(&s, m, now)?;
        let runs = s.mission_planner_runs_since(m.id, now - 3600)?;
        if runs >= m.policy.max_planner_runs_per_hour as i64 {
            return Err(IpcError::new(
                codes::E_LIMIT,
                format!(
                    "the planner ran {runs} times in the last hour (policy.max_planner_runs_per_hour)"
                ),
            ));
        }
        // TASK-291: a login at its usage limit is passed over; with none
        // left the mission waits for the reset, and nothing is asked.
        // Review r10: every login signed out is said on the log, so the
        // mission shows which to sign in again and the loop backs off.
        let words = limit::Words::read(&s, m.id)?;
        let login = match limit::choose_in(&s, m, a.grant.as_ref(), now, &words, &[]) {
            Ok(Ok(l)) => l,
            Ok(Err(wait)) => return Ok(waiting_outcome(&s, m, wait)),
            Err(e) => {
                if limit::is_signed_out_refusal(&e) {
                    limit::say_signed_out(&s, m.id, &words, &e, &actor.name());
                }
                return Err(e);
            }
        };
        let items = s.mission_items(m.id)?;
        let graph = super::graph::build(&s, &ViewScope::internal(), &items)?;
        let steps = steps_for(&s, m, &a)?;
        let cards = s.mission_cards(m.id, CARDS_SHOWN)?;
        let events = s.mission_events(m.id, None, 40)?;
        let snap = planner::snapshot(&planner::SnapshotInput {
            mission: m,
            items: &items,
            graph: &graph,
            steps: &steps,
            cards: &cards,
            events: &events,
            why,
        });
        event(
            &s,
            m.id,
            "planned",
            &actor.name(),
            None,
            serde_json::json!({ "why": why, "host": login.host, "profile": login.profile }),
        );
        (
            login,
            a.grant.clone(),
            snap,
            planner_reads_project(&s, m)?,
            m.policy
                .planner_model
                .clone()
                .unwrap_or_else(|| planner::PLANNER_DEFAULT_MODEL.into()),
            a.effective,
            a.effective >= 3 && m.policy.task_creation == "auto",
            planner::plan_nodes(&items, &graph, m.root_item_id),
        )
    };
    let mut attempts = 0;
    // Review r8: a signed-out login is passed over for the next one, like a
    // login at its limit; the first one's refusal is the answer when none
    // is left.
    let mut signed_out: Vec<limit::Login> = Vec::new();
    let mut first_signed_out: Option<IpcError> = None;
    // Whether this ask recorded a wait, which a later login's answer clears.
    let mut waited = false;
    let (answer, stderr) = loop {
        attempts += 1;
        let (host, profile) = (login.host.as_str(), login.profile.as_deref());
        // Lanes design L1: the planner reads the repository's checkout on
        // its host when `orchestrator.planner_reads` is on. A checkout it
        // cannot have is said on the log, and it plans without reading.
        let checkout = match reads {
            Some(project_id) => {
                match crate::service::sessions::project_checkout_on(
                    &deps.store,
                    &deps.ssh,
                    host,
                    project_id,
                )
                .await
                {
                    Ok(dir) => Some(dir),
                    Err(e) => {
                        let s = lock(&deps.store)?;
                        event(
                            &s,
                            m.id,
                            "note",
                            "fleet",
                            None,
                            serde_json::json!({ "text": format!(
                                "the planner did not read the repository on {host}: {}",
                                e.message
                            ) }),
                        );
                        None
                    }
                }
            }
            None => None,
        };
        let prompt = format!("{}\n\n{snap}", planner::planner_prompt(checkout.is_some()));
        let script = planner::planner_script_as(&model, &prompt, profile, checkout.as_deref())
            .map_err(|e| IpcError::new(codes::E_INVALID, e))?;
        let secs = planner::planner_timeout_secs(checkout.is_some());
        let out = crate::ssh::run_shell_bounded(
            deps.ssh.as_ref(),
            host,
            &script,
            Duration::from_secs(10),
            Duration::from_secs(secs + 20),
        )
        .await?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        // `None`: Claude Code's own "Login expired", no malformed answer.
        let (answer, usage) = match planner::parse_planner_output(&stdout) {
            PlannerOutput::Ran(ran) => {
                let (answer, usage) = planner::planner_answer(ran);
                let s = lock(&deps.store)?;
                book_planner_run(&s, m, host, &model, usage.as_ref(), now_unix());
                if claude_print::run_signed_out(usage.as_ref(), &answer, &out.stderr) {
                    (None, None)
                } else {
                    (Some(answer), usage)
                }
            }
            PlannerOutput::NoClaude => {
                return Err(IpcError::new(
                    codes::E_INVALID_STATE,
                    format!("claude is not on {host}'s PATH"),
                ))
            }
            PlannerOutput::Nothing if claude_print::run_signed_out(None, "", &out.stderr) => {
                (None, None)
            }
            // The limit said on stderr alone is read below.
            PlannerOutput::Nothing
                if claude_print::run_usage_limit(None, "", &out.stderr).is_some() =>
            {
                (Some(String::new()), None)
            }
            PlannerOutput::Nothing => {
                return Err(IpcError::new(
                    codes::E_SHELL,
                    format!("the planner on {host} gave no answer"),
                ))
            }
        };
        let Some(answer) = answer else {
            // Say whose login it is and how to sign it in again, unless
            // another login answers.
            let refusal = claude_print::signed_out_error(host, profile);
            let first = first_signed_out.take().unwrap_or(refusal);
            signed_out.push(login.clone());
            let s = lock(&deps.store)?;
            let words = limit::Words::read(&s, m.id)?;
            // This ask's own refusal, not the chooser's, when none is left.
            match limit::choose_in(&s, m, grant.as_ref(), now_unix(), &words, &signed_out) {
                Ok(Ok(next)) if attempts < limit::LIMIT_ATTEMPTS => {
                    first_signed_out = Some(first);
                    login = next;
                    continue;
                }
                _ => return Err(first),
            }
        };
        // A command array never opens with Claude's limit words, so only an
        // answer that is not one is read for them; and only Claude Code's
        // own error, not the model's text (review r8).
        let hit = match planner::parse_commands(&answer) {
            Ok(_) => None,
            Err(_) => claude_print::run_usage_limit(usage.as_ref(), &answer, &out.stderr),
        };
        let Some(hit) = hit else {
            if waited {
                limit::cleared(&*lock(&deps.store)?, m.id, &login);
            }
            break (answer, out.stderr);
        };
        let s = lock(&deps.store)?;
        let now = now_unix();
        let wait = limit::answered(&s, &login, &hit, now);
        limit::record(&s, m.id, &wait);
        waited = true;
        let words = limit::Words::read(&s, m.id)?;
        match limit::choose_in(&s, m, grant.as_ref(), now, &words, &signed_out)? {
            Ok(next) if attempts < limit::LIMIT_ATTEMPTS => login = next,
            // Another login has headroom; the loop asks there next. This
            // login's wait is said, not the mission's (review r8).
            Ok(_) => {
                return Ok(PlanOutcome {
                    mission_id: m.id,
                    cards: vec![],
                    refused: None,
                    waiting: Some(wait),
                })
            }
            Err(wait) => return Ok(waiting_outcome(&s, m, wait)),
        }
    };
    let commands = match planner::parse_commands(&answer)
        .and_then(|c| planner::check_merge_waves(&c, &plan_nodes).map(|()| c))
    {
        Ok(c) => c,
        Err(mut why) => {
            if answer.trim().is_empty() && !stderr.is_empty() {
                // `claude` printed nothing: what it said went to stderr.
                why.push_str(&format!(
                    "; claude said: {}",
                    crate::service::work::summary::last_error_line(&stderr)
                ));
            }
            let s = lock(&deps.store)?;
            event(
                &s,
                m.id,
                "refused",
                "planner",
                None,
                serde_json::json!({ "why": why }),
            );
            return Ok(PlanOutcome {
                mission_id: m.id,
                cards: vec![],
                refused: Some(why),
                waiting: None,
            });
        }
    };
    let cards = card_commands(deps, m, &commands, now)?;
    // What the autonomy covers is applied now; the rest waits for a person.
    let shown = loop_apply_cards(deps, m, cards, auto_level, accept_created).await?;
    Ok(PlanOutcome {
        mission_id: m.id,
        cards: shown,
        refused: None,
        waiting: None,
    })
}

/// Record that the mission waits on `wait` and say so as the planner's
/// outcome.
fn waiting_outcome(s: &Store, m: &MissionRow, wait: limit::LimitWait) -> PlanOutcome {
    limit::record(s, m.id, &wait);
    PlanOutcome {
        mission_id: m.id,
        cards: vec![],
        refused: None,
        waiting: Some(wait),
    }
}

/// The planner's commands as cards: every `create_item` of the answer in
/// one `create` card (they may wait for each other), a `note` straight to
/// the log, the rest one card each.
fn card_commands(
    deps: &Deps,
    m: &MissionRow,
    commands: &[Command],
    now: i64,
) -> Result<Vec<CardRow>, IpcError> {
    let s = lock(&deps.store)?;
    // One run's cards share a prefix no other run has, even one in the
    // same second (review r01): a repeated id would be dropped as a repeat.
    let run = format!(
        "plan:{now}:{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    );
    let mut out = Vec::new();
    let tree = planner::tree_of(commands);
    if !tree.is_empty() {
        let decision = format!("{run}:create");
        if let Some(c) = s.add_card(
            m.id,
            &NewCard {
                decision_id: &decision,
                source: "planner",
                kind: "create",
                work_item_id: None,
                payload: Some(serde_json::json!({
                    "tree": tree,
                    "done_when": planner::done_when_of(commands),
                    "lanes": planner::lanes_of(commands),
                    "touches": planner::touches_of(commands),
                })),
            },
        )? {
            out.push(c);
        }
    }
    for (n, c) in commands.iter().enumerate() {
        let decision = format!("{run}:{n}");
        let (kind, item, payload) = match c {
            Command::CreateItem { .. } => continue,
            Command::Note { text } => {
                event(
                    &s,
                    m.id,
                    "note",
                    "planner",
                    None,
                    serde_json::json!({ "text": text }),
                );
                continue;
            }
            Command::AddDep {
                item_id,
                depends_on,
            } => (
                "add_dep",
                Some(*item_id),
                serde_json::json!({ "depends_on": depends_on }),
            ),
            Command::RemoveDep {
                item_id,
                depends_on,
            } => (
                "remove_dep",
                Some(*item_id),
                serde_json::json!({ "depends_on": depends_on }),
            ),
            Command::Run { item_id, role } => {
                ("run", Some(*item_id), serde_json::json!({ "role": role }))
            }
            Command::Retry { item_id, note } => {
                ("retry", Some(*item_id), serde_json::json!({ "note": note }))
            }
            Command::Cancel { item_id } => ("cancel", Some(*item_id), serde_json::Value::Null),
            Command::Hold { item_id } => ("hold", Some(*item_id), serde_json::Value::Null),
            Command::Ask { question, options } => (
                "ask",
                None,
                serde_json::json!({ "question": question, "options": options }),
            ),
            Command::Complete { evidence } => (
                "complete",
                None,
                serde_json::json!({ "evidence": evidence }),
            ),
        };
        // An item outside the mission is not the planner's to touch.
        if let Some(i) = item {
            if s.item_mission(i)? != Some(m.id) {
                event(
                    &s,
                    m.id,
                    "refused",
                    "planner",
                    None,
                    serde_json::json!({ "why": format!("{kind} on item {i}, which is not in the mission") }),
                );
                continue;
            }
        }
        if let Some(card) = s.add_card(
            m.id,
            &NewCard {
                decision_id: &decision,
                source: "planner",
                kind,
                work_item_id: item,
                payload: (!payload.is_null()).then_some(payload),
            },
        )? {
            out.push(card);
        }
    }
    Ok(out)
}

/// `work_link { action: mission_plan, mission_id }`: ask the planner now.
pub async fn plan_now(
    args: &WorkLinkArgs,
    deps: &Deps,
    scope: &ViewScope,
) -> Result<PlanOutcome, IpcError> {
    let id = mission_id(args)?;
    let m = {
        let s = lock(&deps.store)?;
        changeable(&s, scope, id)?
    };
    not_a_plan(&m)?;
    if !matches!(m.state.as_str(), "draft" | "active" | "paused") {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("{} is {}", m.name, m.state),
        ));
    }
    run_planner(
        deps,
        &m,
        "a person asked",
        &Actor::Person(super::graph::actor(scope)),
    )
    .await
}

/// `work_link { action: mission_grant, mission_id, level, hours?, budget_cents?,
/// until_done?, hosts?, max_parallel?, profile? }`: a person signs what the
/// loop may do by itself, for this plan, until it expires: after `hours`, or
/// with `until_done` when the mission completes (a finite mission only, and
/// only with a budget, which is then its one bound). Never an agent's: the
/// scope must be a person's own, and the mission theirs to change.
///
/// A grant above the level the mission asks lifts the ask to it while the
/// grant holds (TASK-293, [`autonomy`]): the loop runs at the least of the
/// ask, the fleet's ceiling and the grant, so a mission made in the room (L0)
/// never ran a step under an L2 grant. The mission's own level is left as it
/// is, so the grant's autonomy ends with the grant (review r8).
pub fn grant(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &ViewScope,
) -> Result<GrantRow, IpcError> {
    super::graph::person_decides(scope)?;
    let id = mission_id(args)?;
    let level = args
        .level
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "mission_grant needs level (1 to 3)"))?;
    if !(1..=3).contains(&level) {
        return Err(IpcError::new(codes::E_INVALID, "a grant's level is 1 to 3"));
    }
    let until_done = args.until_done.unwrap_or(false);
    if until_done && args.hours.is_some() {
        return Err(IpcError::new(
            codes::E_INVALID,
            "a grant lasts some hours or until the mission completes, not both",
        ));
    }
    if until_done && args.budget_cents.is_none() {
        return Err(IpcError::new(
            codes::E_INVALID,
            "a grant until the mission completes needs a budget (budget_cents)",
        ));
    }
    let hours = args.hours.unwrap_or(8) as i64;
    if !(1..=GRANT_MAX_SECS / 3600).contains(&hours) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("a grant runs 1 to {} hours", GRANT_MAX_SECS / 3600),
        ));
    }
    if args.budget_cents.is_some_and(|b| b <= 0) {
        return Err(IpcError::new(codes::E_INVALID, "a budget is more than 0"));
    }
    let hosts = args.hosts.clone().map(|h| {
        h.into_iter()
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .take(20)
            .collect::<Vec<_>>()
    });
    let profile = args
        .profile
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| crate::validate::claude_profile(p).map(|()| p.to_string()))
        .transpose()?;
    let s = lock(store)?;
    let m = changeable(&s, scope, id)?;
    not_a_plan(&m)?;
    if crate::store::MISSION_FINAL_STATES.contains(&m.state.as_str()) {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("{} is {}", m.name, m.state),
        ));
    }
    if until_done && m.mode != "finite" {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "{} is {}: it never completes, so its grant runs some hours",
                m.name, m.mode
            ),
        ));
    }
    let who = super::graph::actor(scope);
    let lifted_from = (m.level < level).then_some(m.level);
    s.revoke_grants(id)?;
    let g = s.add_grant(
        id,
        &NewGrant {
            level,
            granted_by: &who,
            hosts,
            budget_micros: args.budget_cents.map(|c| c * 10_000),
            max_parallel: args.max_parallel.map(|p| p as i64),
            profile: profile.clone(),
            expires_at: if until_done {
                GRANT_UNTIL_DONE_AT
            } else {
                now_unix() + hours * 3600
            },
        },
    )?;
    event(
        &s,
        id,
        "grant",
        &who,
        None,
        serde_json::json!({
            "level": level,
            "hours": (!until_done).then_some(hours),
            "until_done": until_done,
            "budget_cents": args.budget_cents,
            "profile": profile,
            "lifted_from": lifted_from,
        }),
    );
    // The grant's hosts against the tasks' organisations (access review
    // F4): a run on a host of another organisation is refused at its
    // start, so say it now. Hosts placement picks are checked by the loop.
    if let Some(hosts) = g.hosts.as_ref().filter(|h| !h.is_empty()) {
        for why in access::org_conflicts(&s, &m, hosts)? {
            ask_card(
                &s,
                id,
                &Step {
                    kind: "ask".into(),
                    item_id: None,
                    role: None,
                    reason: why,
                    context: None,
                    auto: false,
                },
            )?;
        }
    }
    s.wake_mission(id)?;
    Ok(g)
}

/// `work_link { action: mission_revoke, mission_id }`: end the mission's
/// grants now. The loop drops to L1 at once.
pub fn revoke(
    args: &WorkLinkArgs,
    store: &Mutex<Store>,
    scope: &ViewScope,
) -> Result<usize, IpcError> {
    let id = mission_id(args)?;
    let s = lock(store)?;
    changeable(&s, scope, id)?;
    let n = s.revoke_grants(id)?;
    event(
        &s,
        id,
        "revoked",
        &super::graph::actor(scope),
        None,
        serde_json::json!({ "grants": n }),
    );
    Ok(n)
}

/// `work_link { action: missions_pause_all }`: pause every active mission
/// the caller may change, and revoke their grants (Pause all, §7.5).
pub fn pause_all(store: &Mutex<Store>, scope: &ViewScope) -> Result<Vec<i64>, IpcError> {
    let s = lock(store)?;
    let who = super::graph::actor(scope);
    let mut paused = Vec::new();
    for m in s.list_missions()? {
        if m.state != "active" || !super::missions::may_change_mission(&s, scope, &m)? {
            continue;
        }
        s.set_mission_state(m.id, None, "paused", &who)?;
        s.revoke_grants(m.id)?;
        paused.push(m.id);
    }
    Ok(paused)
}

/// Pause `m` because a brake fired, with a question for a person.
fn brake(s: &Store, m: &MissionRow, kind: &str, why: String) -> Result<(), IpcError> {
    s.set_mission_state(m.id, None, "paused", "loop")?;
    event(
        s,
        m.id,
        kind,
        "loop",
        None,
        serde_json::json!({ "why": why }),
    );
    ask_card(
        s,
        m.id,
        &Step {
            kind: "ask".into(),
            item_id: None,
            role: None,
            reason: format!("paused: {why}"),
            context: None,
            auto: false,
        },
    )?;
    Ok(())
}

/// When to look at `m` next.
fn next_wake(m: &MissionRow, counts: &MissionTaskCounts, now: i64) -> i64 {
    if m.mode == "continuous" {
        let every = m
            .policy
            .wake_every_secs
            .unwrap_or(CONTINUOUS_DEFAULT_WAKE_SECS) as i64;
        return now
            + every.min(if counts.open > 0 {
                RUNNING_RECHECK_SECS
            } else {
                every
            });
    }
    now + if counts.open > 0 {
        RUNNING_RECHECK_SECS
    } else {
        IDLE_RECHECK_SECS
    }
}

/// Whether the planner should be asked now, and why. Only for judgment:
/// a mission with nothing planned under its root, or one that needs a
/// person's answer since its last call, and only when something happened
/// since that call.
///
/// A planner whose last ask ended at a usage limit (TASK-291) is asked again
/// as soon as a login it may use has headroom (the reset passed, or another
/// login is under the line), and never before.
fn planner_wanted(
    s: &Store,
    m: &MissionRow,
    steps: &[Step],
    counts: &MissionTaskCounts,
    grant: Option<&GrantRow>,
    now: i64,
) -> Result<Option<&'static str>, IpcError> {
    if s.mission_cards(m.id, CARDS_SHOWN)?
        .iter()
        .any(|c| c.state == "open" && c.source == "planner")
    {
        return Ok(None);
    }
    let words = limit::Words::read(s, m.id)?;
    if words.waiting() {
        let free = matches!(limit::choose_in(s, m, grant, now, &words, &[]), Ok(Ok(_)));
        return Ok(free.then_some("the usage limit it waited on is over"));
    }
    // Refused for signed-out logins (review r10): not asked again until a
    // login it may use is signed in, or something else answers.
    if words.signed_out().is_some() && limit::choose_in(s, m, grant, now, &words, &[]).is_err() {
        return Ok(None);
    }
    let last_plan = words.last_planned_at();
    let items = s.mission_items(m.id)?;
    if items.len() <= 1 && counts.open == 0 && last_plan.is_none() {
        return Ok(Some("break the goal down into tasks"));
    }
    let something_new = match (last_plan, counts.last_activity_at) {
        (None, _) => true,
        (Some(p), Some(a)) => a > p,
        (Some(_), None) => false,
    };
    if something_new
        && steps
            .iter()
            .any(|st| st.kind == "ask" && st.item_id.is_some())
    {
        return Ok(Some("a task needs a decision after its failure"));
    }
    Ok(None)
}

/// The steps that start a worker session.
const RUN_STEPS: &[&str] = &["run", "retry", "review", "test", "integrate"];

/// Why a loop run of `step` waits, when the login it would bill is at or
/// past `accounts.pause_at` (redesign 8.7) or signed out (review w02-6): on
/// the grant's first host, else where the start would land
/// (`tickets::seen_place` / `seen_host` / `placed_host`, the start's own
/// choice: the key's history, the project's last host, placement's login
/// there), under the grant's login. `None` when the run's host cannot be
/// told yet: the start then decides, as before.
fn run_over_limit(
    s: &Store,
    m: &MissionRow,
    step: &Step,
    grant: Option<&GrantRow>,
    now: i64,
) -> Result<Option<String>, IpcError> {
    let Some(item_id) = step.item_id else {
        return Ok(None);
    };
    let Ok(start) = start_for(s, m, item_id, &Actor::Loop, grant) else {
        return Ok(None);
    };
    let (host, placed_profile) = match start.host_alias {
        Some(h) => (Some(h), None),
        None => {
            // A work item with no key cannot be started: nothing to hold.
            let Some(key) = s.get_work_item(item_id)?.and_then(|i| i.key) else {
                return Ok(None);
            };
            let item_org = s.item_org(item_id)?;
            let (_, seen, _) =
                crate::service::trackers::tickets::seen_place(s, &key, item_org, start.project_id)?;
            match start.project_id.or(seen.as_ref().map(|p| p.0)) {
                // Where the start itself lands (`plan_resolved`): the placed
                // host and its login, not only the host last seen (review
                // w02-1), so the hold looks at what the start would bill.
                Some(p) => {
                    let seen = crate::service::trackers::tickets::seen_host(s, seen, p)?;
                    match crate::service::trackers::tickets::placed_host(s, p, seen) {
                        Some((h, profile)) => (Some(h), profile),
                        None => (None, None),
                    }
                }
                None => (None, None),
            }
        }
    };
    let Some(host) = host else {
        return Ok(None);
    };
    let profile = start.profile.or(placed_profile);
    if let Some(over) =
        crate::service::account_limits::over_limit(s, &host, profile.as_deref(), now)?
    {
        return Ok(Some(over.reason()));
    }
    // A login fleet knows is signed out answers a run with `/login` (review
    // w02-6): the run waits for it as for a spent one.
    crate::service::login_health::signed_out_reason(s, &host, profile.as_deref())
}

/// Drop the loop's run steps whose account is over the line (redesign 8.7):
/// they wait, the mission stays active, and the next pass looks again. One
/// `account_limit` event says why, not one per pass.
fn hold_runs_over_limit(
    s: &Store,
    m: &MissionRow,
    grant: Option<&GrantRow>,
    steps: &mut Vec<Step>,
    now: i64,
) -> Result<(), IpcError> {
    let mut why: Option<String> = None;
    let mut kept = Vec::with_capacity(steps.len());
    for st in steps.drain(..) {
        if st.auto && RUN_STEPS.contains(&st.kind.as_str()) {
            if let Some(over) = run_over_limit(s, m, &st, grant, now)? {
                why.get_or_insert(over);
                continue;
            }
        }
        kept.push(st);
    }
    *steps = kept;
    let Some(why) = why else {
        return Ok(());
    };
    if !hold_episode_open(s, m.id)? {
        event(
            s,
            m.id,
            "account_limit",
            "loop",
            None,
            serde_json::json!({ "why": why }),
        );
    }
    Ok(())
}

/// Whether the loop's runs are already in a held episode the log has said
/// (review r01): the newest `account_limit` event comes after the last run
/// step the loop took or had refused. Other events in between (a card, a
/// planner note) do not end the episode; a run that goes (or is tried and
/// refused) does, so the next hold is said again.
fn hold_episode_open(s: &Store, mission_id: i64) -> Result<bool, IpcError> {
    const PAGE: usize = 100;
    let mut before = None;
    loop {
        let page = s.mission_events(mission_id, before, PAGE)?;
        for e in &page {
            if e.kind == "account_limit" {
                return Ok(true);
            }
            let run = matches!(e.kind.as_str(), "step" | "refused")
                && e.payload
                    .as_ref()
                    .and_then(|p| p.get("step"))
                    .and_then(|k| k.as_str())
                    .is_some_and(|k| RUN_STEPS.contains(&k));
            if run {
                return Ok(false);
            }
        }
        match page.last() {
            Some(e) if page.len() == PAGE => before = Some(e.id),
            _ => return Ok(false),
        }
    }
}

/// One pass over one mission, under its lease.
pub async fn tick_mission(deps: &Deps, id: i64, now: i64) -> Result<(), IpcError> {
    let (m, a, steps, counts) = {
        let s = lock(&deps.store)?;
        // A fresh clock (review r06): a mission reached late in a long
        // pass does not get a lease that is already near its end.
        if !s.take_mission_lease(id, crate::service::tick::lease_clock(now))? {
            return Ok(());
        }
        let Some(m) = s.get_mission(id)? else {
            return Ok(());
        };
        let a = autonomy(&s, &m, now)?;
        let counts = s.mission_task_counts(id)?;
        if !a.enabled || m.state != "active" || !crate::store::mode_runs_loop(&m.mode) {
            s.release_mission_lease(id, Some(now + IDLE_RECHECK_SECS), Some(now))?;
            return Ok(());
        }
        // The brakes (§7.5): the grant's budget, and no progress at all.
        if let Some(budget) = a.grant.as_ref().and_then(|g| g.budget_micros) {
            let cost = s.mission_cost_micros(id)?;
            if cost >= budget {
                brake(
                    &s,
                    &m,
                    "budget",
                    format!(
                        "the workers spent ${:.2} of the grant's ${:.2}",
                        cost as f64 / 1e6,
                        budget as f64 / 1e6
                    ),
                )?;
                s.release_mission_lease(id, Some(now + IDLE_RECHECK_SECS), Some(now))?;
                return Ok(());
            }
        }
        // A person's resume restarts the window: braking on the same stale
        // run that paused it would pause the mission again before this tick
        // ever tried to start a step.
        let since = counts.last_activity_at.max(s.mission_resumed_at(id)?);
        if counts.open > 0 && since.is_some_and(|at| now - at > m.policy.no_progress_secs as i64) {
            brake(
                &s,
                &m,
                "no_progress",
                format!(
                    "no run started or finished for {} minutes",
                    m.policy.no_progress_secs / 60
                ),
            )?;
            s.release_mission_lease(id, Some(now + IDLE_RECHECK_SECS), Some(now))?;
            return Ok(());
        }
        let mut steps = steps_for(&s, &m, &a)?;
        for st in steps.iter().filter(|st| st.kind == "ask") {
            ask_card(&s, id, st)?;
        }
        if a.effective >= AUTO_LEVEL {
            hold_runs_over_limit(&s, &m, a.grant.as_ref(), &mut steps, now)?;
        }
        (m, a, steps, counts)
    };
    // Under a grant, what its runs can reach is checked once, before they
    // need it (access review F6).
    if let Some(g) = a.grant.as_ref().filter(|_| a.effective >= AUTO_LEVEL) {
        access::check_once(deps, &m, g, now).await;
    }
    // Under a grant, the mechanical steps are the loop's own.
    if a.effective >= AUTO_LEVEL {
        for st in steps.iter().filter(|st| st.auto) {
            let r = apply_step(deps, &m, st, &Actor::Loop, &ViewScope::internal()).await;
            if !r.ok {
                tracing::debug!(mission = id, step = %st.key(), detail = %r.detail, "[orchestrate] step refused");
            }
        }
    }
    let want = {
        let s = lock(&deps.store)?;
        if a.effective >= 1 {
            planner_wanted(&s, &m, &steps, &counts, a.grant.as_ref(), now)?
        } else {
            None
        }
    };
    if let Some(why) = want {
        if let Err(e) = run_planner(deps, &m, why, &Actor::Loop).await {
            tracing::debug!(mission = id, error = %e.message, "[orchestrate] planner not run");
        }
    }
    // A finite mission's members may need their branches checked against
    // each other before it integrates (O7).
    if a.effective >= 1 {
        integrate::check(deps, &m).await;
    }
    let s = lock(&deps.store)?;
    let counts = s.mission_task_counts(id)?;
    let m = s.get_mission(id)?.unwrap_or(m);
    // A planner waiting on a usage limit is asked again at its reset.
    let wake = match limit::wake_at(&s, id, now)? {
        Some(reset) => next_wake(&m, &counts, now).min(reset.max(now + RESET_RECHECK_SECS)),
        None => next_wake(&m, &counts, now),
    };
    s.release_mission_lease(id, Some(wake), Some(now))?;
    Ok(())
}

/// One pass of the loop: every due mission, one after the other.
pub async fn tick_once(deps: &Deps, now: i64) {
    let next = Some(TICK_EVERY);
    let due = match deps.store.lock() {
        Ok(s) => {
            // Pause all (redesign 8.1) before the loop's own switch, so
            // health says why nothing moves.
            if !crate::service::loops::gate_in("missions", &s, next) {
                None
            // With the loop off no mission is ticked, but a failed git
            // read is still read again below.
            } else if settings::get_bool(&s, settings::ORCHESTRATOR_ENABLED) {
                Some(s.missions_due(now).unwrap_or_default())
            } else {
                Some(Vec::new())
            }
        }
        Err(e) => {
            crate::service::loops::report("missions", Err(e.to_string()), next);
            return;
        }
    };
    let Some(due) = due else {
        // The evidence retry only reads git and writes evidence, so it runs
        // under Pause all too (review r10): a pause longer than
        // EVIDENCE_RETRY_SECS would leave a transient failed read `missing`
        // for good.
        crate::service::work::report::retry_unread_evidence(&deps.store, &deps.ssh, now).await;
        return;
    };
    let mut failed: Option<String> = None;
    for id in due {
        if let Err(e) = tick_mission(deps, id, now).await {
            tracing::warn!(mission = id, error = %e.message, "[orchestrate] tick failed");
            failed = Some(format!("mission {id}: {}", e.message));
            if let Ok(s) = deps.store.lock() {
                let _ = s.release_mission_lease(id, Some(now + RUNNING_RECHECK_SECS), None);
            }
        }
    }
    // A run whose git read failed (a host that dropped for a moment) is read
    // again, in a mission or not and with the orchestrator off too: the gate
    // holds it `pending` meanwhile. After the missions, outside every
    // mission's lease, and bounded per pass.
    crate::service::work::report::retry_unread_evidence(&deps.store, &deps.ssh, now).await;
    crate::service::loops::report("missions", failed.map_or(Ok(()), Err), next);
}

/// The loop's periodic task, on the hub and on a standalone desktop. A
/// restart loses nothing: every active mission is due again at once, and a
/// lease a crashed tick held expires.
pub fn spawn_mission_tick(
    deps: Deps,
    token: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    crate::rt::spawn(async move {
        let mut every = tokio::time::interval(TICK_EVERY);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // A pass that panics is logged and the next tick runs (review r06 F7).
        crate::service::tick::run_cancellable_tick(every, token, || tick_once(&deps, now_unix()))
            .await;
    })
}

#[cfg(test)]
mod tests;
