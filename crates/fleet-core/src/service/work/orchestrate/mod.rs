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

pub mod drafts;
pub mod guard;
pub mod integrate;
pub mod planner;
pub mod steps;
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
    Store, TaskRow, WorkItemRow, GRANT_MAX_SECS,
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

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

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
    let effective = m.level.min(ceiling).min(granted);
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

/// Fill a listed mission's spend: what it cost and its live grant's budget.
pub fn fill_spend(s: &Store, m: &mut MissionRow, now: i64) -> Result<(), IpcError> {
    m.cost_micros = Some(s.mission_cost_micros(m.id)?);
    m.budget_micros = s
        .live_mission_grant(m.id, now)?
        .and_then(|g| g.budget_micros);
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
    Ok(plan_steps(&StepInput {
        mission: m,
        graph: &graph,
        items: &items,
        attempts: &attempts,
        counts: s.mission_task_counts(m.id)?,
        max_parallel: max_parallel(m, a),
    }))
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
                    "{} is in a repo outside {}'s allow-list",
                    item.key.as_deref().unwrap_or(&item.title),
                    m.name
                ),
            ))
        }
        (None, [only]) => Some(*only),
        (None, _) => None,
    };
    Ok(StartArgs {
        item_id: Some(item_id),
        project_id,
        host_alias: grant
            .and_then(|g| g.hosts.as_ref())
            .and_then(|h| h.first().cloned()),
        with_brief: true,
        decider: match actor {
            Actor::Person(_) => Decider::Person,
            Actor::Loop => Decider::Agent,
        },
        owner: m.owner_person_id,
        // The run's session is the mission's, whoever pressed Go
        // (migration 124): the origin chip names the mission.
        origin: Some(crate::store::SessionOrigin::mission(m.id)),
        // The login the grant names (redesign 8.7); none, the host's own.
        profile: grant.and_then(|g| g.profile.clone()),
        ..Default::default()
    })
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
    let (ok, detail, task_id) = match res {
        Ok((d, t)) => (true, d, t),
        Err(e) => (false, e.message, None),
    };
    if let Ok(s) = deps.store.lock() {
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
                }
                (start_for(&s, m, item_id, actor, a.grant.as_ref())?, a.grant)
            };
            let _ = grant;
            let role = step.role.as_deref().unwrap_or("implement");
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
            let s = lock(&deps.store)?;
            let made = s.propose_tree(root, &tree, "planner")?;
            for (i, row) in made.iter().enumerate() {
                if let Some(l) = lines.get(i).filter(|l| !l.is_empty()) {
                    s.set_item_done_when(row.id, l, &actor.name())?;
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
            let s = lock(&deps.store)?;
            let mut n = 0;
            for t in s.tasks_for_item(it)? {
                if matches!(t.state.as_str(), "queued" | "running") {
                    crate::service::tasks::cancel_task(&s, t.id, "the mission cancelled it")?;
                    n += 1;
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
    s.decide_card(id, state, &who, note.as_deref())?;
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
    let named = m
        .policy
        .planner_host
        .as_ref()
        .map(|h| (h, "policy.planner_host"));
    let granted = grant
        .and_then(|g| g.hosts.as_ref())
        .and_then(|h| h.first())
        .map(|h| (h, "the grant's first host"));
    if let Some((h, from)) = named.or(granted) {
        require_host_sees_org(s, h, m.org_id, from)?;
        return Ok(h.clone());
    }
    let mut passed_over = None;
    for i in s.mission_items(m.id)? {
        for t in s.tasks_for_item(i.id)? {
            if let Some(w) = t
                .worker_session_id
                .and_then(|w| s.get_session_by_id(w).ok().flatten())
            {
                if host_sees_org(s, &w.host_alias, m.org_id)? {
                    return Ok(w.host_alias);
                }
                passed_over.get_or_insert(w.host_alias);
            }
        }
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

/// Ask the planner about `m` now, and card its answer.
pub async fn run_planner(
    deps: &Deps,
    m: &MissionRow,
    why: &str,
    actor: &Actor,
) -> Result<PlanOutcome, IpcError> {
    let _slot = PlannerSlot::take(deps, m)?;
    let now = now_unix();
    let (host, prompt, model, auto_level, accept_created) = {
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
        let host = planner_host(&s, m, a.grant.as_ref())?;
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
            serde_json::json!({ "why": why, "host": host }),
        );
        (
            host,
            format!("{}\n\n{snap}", planner::PLANNER_PROMPT),
            m.policy
                .planner_model
                .clone()
                .unwrap_or_else(|| planner::PLANNER_DEFAULT_MODEL.into()),
            a.effective,
            a.effective >= 3 && m.policy.task_creation == "auto",
        )
    };
    let script = planner::planner_script(&model, &prompt);
    let out = crate::ssh::run_shell_bounded(
        deps.ssh.as_ref(),
        &host,
        &script,
        Duration::from_secs(10),
        Duration::from_secs(planner::PLANNER_HOST_TIMEOUT_SECS + 20),
    )
    .await?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let answer = match planner::parse_planner_output(&stdout) {
        PlannerOutput::Ran(ran) => {
            let (answer, usage) = planner::planner_answer(ran);
            let s = lock(&deps.store)?;
            book_planner_run(&s, m, &host, &model, usage.as_ref(), now_unix());
            // Claude Code's own "Login expired" is not a malformed answer:
            // say whose login it is and how to sign it in again.
            if claude_print::run_signed_out(usage.as_ref(), &answer, &out.stderr) {
                return Err(claude_print::signed_out_error(&host, None));
            }
            answer
        }
        PlannerOutput::NoClaude => {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!("claude is not on {host}'s PATH"),
            ))
        }
        PlannerOutput::Nothing if claude_print::run_signed_out(None, "", &out.stderr) => {
            return Err(claude_print::signed_out_error(&host, None))
        }
        PlannerOutput::Nothing => {
            return Err(IpcError::new(
                codes::E_SHELL,
                format!("the planner on {host} gave no answer"),
            ))
        }
    };
    let commands = match planner::parse_commands(&answer) {
        Ok(c) => c,
        Err(mut why) => {
            if answer.trim().is_empty() && !out.stderr.is_empty() {
                // `claude` printed nothing: what it said went to stderr.
                why.push_str(&format!(
                    "; claude said: {}",
                    crate::service::work::summary::last_error_line(&out.stderr)
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
            });
        }
    };
    let cards = card_commands(deps, m, &commands, now)?;
    // What the autonomy covers is applied now; the rest waits for a person.
    let mut shown = Vec::with_capacity(cards.len());
    for c in cards {
        if loop_applies_card(&c.kind, auto_level) {
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
    Ok(PlanOutcome {
        mission_id: m.id,
        cards: shown,
        refused: None,
    })
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

/// `work_link { action: mission_grant, mission_id, level, hours, budget_cents?,
/// hosts?, max_parallel?, profile? }`: a person signs what the loop may do by itself,
/// for this plan, until it expires. Never an agent's: the scope must be a
/// person's own, and the mission theirs to change.
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
    let who = super::graph::actor(scope);
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
            expires_at: now_unix() + hours * 3600,
        },
    )?;
    event(
        &s,
        id,
        "grant",
        &who,
        None,
        serde_json::json!({ "level": level, "hours": hours, "budget_cents": args.budget_cents, "profile": profile }),
    );
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
fn planner_wanted(
    s: &Store,
    m: &MissionRow,
    steps: &[Step],
    counts: &MissionTaskCounts,
) -> Result<Option<&'static str>, IpcError> {
    if s.mission_cards(m.id, CARDS_SHOWN)?
        .iter()
        .any(|c| c.state == "open" && c.source == "planner")
    {
        return Ok(None);
    }
    let last_plan = s
        .mission_events(m.id, None, 200)?
        .into_iter()
        .find(|e| e.kind == "planned")
        .map(|e| e.at);
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

/// The account a loop run of `step` would bill, when it is at or past
/// `accounts.pause_at` (redesign 8.7): on the grant's first host, else where
/// the start would land (`tickets::seen_place` / `seen_host`, the start's
/// own choice: a start rule, the key's history, the project's last host),
/// under the grant's login. `None` when the run's host cannot be told yet:
/// the start then decides, as before.
fn run_over_limit(
    s: &Store,
    m: &MissionRow,
    step: &Step,
    grant: Option<&GrantRow>,
    now: i64,
) -> Result<Option<crate::service::account_limits::OverLimit>, IpcError> {
    let Some(item_id) = step.item_id else {
        return Ok(None);
    };
    let Ok(start) = start_for(s, m, item_id, &Actor::Loop, grant) else {
        return Ok(None);
    };
    let host = match start.host_alias {
        Some(h) => Some(h),
        None => {
            // A work item with no key cannot be started: nothing to hold.
            let Some(key) = s.get_work_item(item_id)?.and_then(|i| i.key) else {
                return Ok(None);
            };
            let item_org = s.item_org(item_id)?;
            let (_, seen, _) =
                crate::service::trackers::tickets::seen_place(s, &key, item_org, start.project_id)?;
            match start.project_id.or(seen.as_ref().map(|p| p.0)) {
                Some(p) => crate::service::trackers::tickets::seen_host(s, seen, p)?,
                None => None,
            }
        }
    };
    let Some(host) = host else {
        return Ok(None);
    };
    crate::service::account_limits::over_limit(s, &host, start.profile.as_deref(), now)
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
                why.get_or_insert_with(|| over.reason());
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
        if counts.open > 0
            && counts
                .last_activity_at
                .is_some_and(|at| now - at > m.policy.no_progress_secs as i64)
        {
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
            planner_wanted(&s, &m, &steps, &counts)?
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
    s.release_mission_lease(id, Some(next_wake(&m, &counts, now)), Some(now))?;
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
                return;
            }
            if !settings::get_bool(&s, settings::ORCHESTRATOR_ENABLED) {
                crate::service::loops::report("missions", Ok::<_, String>(()), next);
                return;
            }
            s.missions_due(now).unwrap_or_default()
        }
        Err(e) => {
            crate::service::loops::report("missions", Err(e.to_string()), next);
            return;
        }
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
