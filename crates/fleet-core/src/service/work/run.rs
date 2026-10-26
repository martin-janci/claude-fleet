//! `work_link { run, item_id, role? }`: one attempt at a work item, in its
//! own session and worktree, tracked as a task (orchestration O0, design
//! 2026-10-07 §4.4 and task → session spec §4.3).
//!
//! A run is a [`start_work`] — the item's project, a host, a worktree per
//! `branch_slug(key + title)`, the brief through the handover — whose first
//! prompt carries the `FLEET_TASK_DONE_<nonce>` instruction of a `tasks` row,
//! so `wait_for_task`, the Stop-hook marker scan and the liveness sweep work
//! on it unchanged. Unlike `dispatch_task { new_worker }`, which starts its
//! worker in the project's main checkout and mirrors the job as a NEW
//! `agent` item, a run points at the item that already exists
//! (`tasks.work_item_id`, migration 110), so one item can be attempted more
//! than once and by more than one role.
//!
//! Idempotent per (item, role): while an attempt is open, a second run
//! answers it (`existing: true`) instead of starting another.
//!
//! [`start_work`]: crate::service::trackers::tickets::start_work

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs;
use crate::service::tasks;
use crate::service::trackers::tickets::{self, StartArgs};
use crate::service::view_scope::ViewScope;
use crate::store::{Store, TaskRow};

/// The roles a run may take. `implement` is the default.
pub const RUN_ROLES: &[&str] = &["implement", "review", "test", "research", "integrate"];

/// `work_link { run }`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunOutcome {
    /// The attempt, `running` once its session was started.
    pub task: TaskRow,
    /// The session doing it (the task's worker).
    #[serde(default)]
    pub session_id: Option<i64>,
    /// `true`: an attempt was already open, and nothing was started.
    #[serde(default)]
    pub existing: bool,
}

/// What [`precheck`] found: an open attempt to answer, or the number the new
/// one takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Precheck {
    Existing(Box<TaskRow>),
    Fresh { attempt: i64 },
}

/// PURE over the store: may `item_id` be run in `role` by `view`, and is an
/// attempt already open? An item the org half may not see answers exactly as
/// one that does not exist; an open attempt is answered only to a caller who
/// may see that task (its prompt and its worker), and to anybody else is a
/// bare `E_EXISTS` that names nobody.
pub fn precheck(
    s: &Store,
    view: &ViewScope,
    item_id: i64,
    role: &str,
) -> Result<Precheck, IpcError> {
    if !RUN_ROLES.contains(&role) {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("run role {role:?}; one of {}", RUN_ROLES.join(", ")),
        ));
    }
    let item = match s.get_work_item(item_id)? {
        Some(i) if tickets::item_visible(&view.org, s, &i)? => i,
        _ => return Err(orgs::not_found("work item", item_id)),
    };
    // A job's mirror item: its title and notes are the dispatch prompt of
    // someone else's job, which a run would copy into a brief and a task
    // row. The job itself is the run.
    if item.origin.as_deref() == Some("agent") {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("work item {item_id} mirrors a dispatched job; it is not run on its own"),
        ));
    }
    if let Some(open) = s.open_task_for_item(item_id, role)? {
        if tasks::task_visible_in_scope(s, &open, view)? {
            return Ok(Precheck::Existing(Box::new(open)));
        }
        return Err(IpcError::new(
            codes::E_EXISTS,
            format!("work item {item_id} already has a {role} run in progress"),
        ));
    }
    match item.proposal_state.as_deref() {
        Some("proposed") => {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!("work item {item_id} is a proposal a person has not accepted yet"),
            ))
        }
        Some("rejected") => {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!("work item {item_id} is a rejected proposal"),
            ))
        }
        _ => {}
    }
    if item.status_category == "done" {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("work item {item_id} is done; reopen it before running it again"),
        ));
    }
    Ok(Precheck::Fresh {
        attempt: s.next_task_attempt(item_id, role)?,
    })
}

/// The first prompt of a run (the done-marker instruction is appended at
/// delivery, [`tasks::with_instruction`]): the start prompt when a brief was
/// queued, else the item's own words, since there is no brief to point at.
pub fn run_prompt(key: &str, title: &str, notes: Option<&str>, role: &str, queued: bool) -> String {
    let head = if queued {
        tickets::start_prompt(key)
    } else {
        match notes.map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) => format!("Work on {key} {title}.\n\n{n}"),
            None => format!("Work on {key} {title}."),
        }
    };
    match role {
        "implement" => head,
        other => format!("{head}\n\nYour role on this task: {other}."),
    }
}

/// The work a new run of an item builds on: the branch and host of its
/// newest implement attempt that had a checkout of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorWork {
    pub host_alias: String,
    pub branch: String,
    /// The host answered its last probe (`local` always does): a run is
    /// sent there only while it is up, else placement picks and the branch
    /// is found on `origin` if it was pushed.
    pub host_up: bool,
}

/// PURE over the store: [`PriorWork`] for `item_id`, `None` before its first
/// implement run with a checkout. A run beside a live session gets a `-N`
/// checkout of its own, which would start from the default branch: a review
/// then read none of the work and a retry began again from nothing (lanes
/// design 2026-10-23, P2). Starting it from this branch, on this host,
/// gives it the work.
pub fn prior_work(s: &Store, item_id: i64) -> Result<Option<PriorWork>, IpcError> {
    for t in s.tasks_for_item(item_id)? {
        if !matches!(t.role.as_deref(), None | Some("implement")) {
            continue;
        }
        let Some(session) = t
            .worker_session_id
            .map(|id| s.get_session_by_id(id))
            .transpose()?
            .flatten()
        else {
            continue;
        };
        let Some(wt) = session
            .worktree_id
            .map(|id| s.get_worktree_row(id))
            .transpose()?
            .flatten()
        else {
            continue;
        };
        let branch = wt
            .branch
            .filter(|b| !b.trim().is_empty() && b != "HEAD")
            .unwrap_or(wt.name);
        // A worktree row's branch can come from a host's hook report: only
        // a name git would take as a branch goes into the prompt.
        if !crate::service::agents::workspace::branch_ok(&branch) {
            continue;
        }
        let host_up = session.host_alias == crate::service::projects::LOCAL_HOST
            || s.get_host_row(&session.host_alias)?
                .is_some_and(|h| h.reachable);
        return Ok(Some(PriorWork {
            host_alias: session.host_alias,
            branch,
            host_up,
        }));
    }
    Ok(None)
}

/// What a run's prompt says about the work it builds on ([`prior_work`]).
pub fn prior_work_note(role: &str, branch: &str) -> String {
    let what = match role {
        "implement" => format!(
            "The earlier attempts' work is on branch `{branch}` and your checkout starts \
             from it: continue it rather than starting again."
        ),
        other => format!(
            "The work to {other} is on branch `{branch}` and your checkout starts from it: \
             its changes are the commits the default branch lacks."
        ),
    };
    format!("{what} If `git log` does not show them, fetch them with `git fetch origin {branch}`.")
}

/// What a run's prompt asks of a Claude worker about subagents
/// (`orchestrator.run_subagents`, lanes design 2026-10-23 L1), by role;
/// `None` for a role with no use for them. Subagents only read: they share
/// the worker's checkout, so two writing at once would fight over it.
pub fn subagent_note(role: &str) -> Option<&'static str> {
    match role {
        "implement" | "integrate" => Some(
            "Use subagents (the Agent tool) to keep your own context for the work: one or more \
             to explore the code before you edit, and one with a fresh context to review your \
             diff before you report. Subagents only read: never have one edit, since they share \
             your checkout.",
        ),
        "review" | "research" => Some(
            "Use subagents (the Agent tool) to read the parts of the code in parallel; they only \
             read.",
        ),
        _ => None,
    }
}

/// Write the attempt: a `running` task on `worker`, naming the item, its
/// attempt number and role. No requester (nobody's inbox gets the result),
/// and no mirror item: the item it is an attempt at already exists.
pub fn record_run(
    s: &Store,
    worker: i64,
    prompt: &str,
    item_id: i64,
    attempt: i64,
    role: &str,
) -> Result<TaskRow, IpcError> {
    let task = tasks::create_task(s, None, Some(worker), prompt)?;
    s.set_task_run(task.id, item_id, attempt, role)?;
    let task = s
        .get_task(task.id)?
        .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "task vanished after insert"))?;
    tasks::start_task(s, &task)
}

/// Write a headless attempt (orchestration F3, `orchestrate::checks`): a
/// `running` task with no worker session, naming the item, its attempt
/// number and role.
pub fn record_headless_run(
    s: &Store,
    prompt: &str,
    item_id: i64,
    attempt: i64,
    role: &str,
) -> Result<TaskRow, IpcError> {
    let task = tasks::create_task(s, None, None, prompt)?;
    s.set_task_run(task.id, item_id, attempt, role)?;
    let task = s
        .get_task(task.id)?
        .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "task vanished after insert"))?;
    tasks::start_task(s, &task)
}

/// Whether a live session is already on the item: a run then starts beside
/// it, in its own `-N` checkout, instead of being refused with "jump to it".
pub fn has_live_work(s: &Store, item_id: i64) -> Result<bool, IpcError> {
    match s.get_work_item(item_id)?.and_then(|i| i.key) {
        Some(key) => Ok(!tickets::live_work_on(s, &key, s.item_org(item_id)?)?.is_empty()),
        None => Ok(false),
    }
}

/// The runs between their check and their task row, by item and role.
static STARTING: crate::rt::KeySet = std::sync::Mutex::new(None);

/// Claim the start of `role` on `item_id` against a session start racing it
/// ([`run_item_with`]); released on drop. `None`: one is already starting.
pub(crate) fn claim_start(
    store: &Arc<Mutex<Store>>,
    item_id: i64,
    role: &str,
) -> Option<crate::rt::KeyedClaim> {
    crate::rt::claim(
        &STARTING,
        Arc::as_ptr(store) as usize,
        format!("{item_id}:{role}"),
    )
}

/// `work_link { run }` end to end over the real start path.
pub async fn run_item(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<crate::ssh::SshClient>,
    reg: &Arc<crate::cancel::CancellationRegistry>,
    start: &StartArgs,
    role: &str,
    view: &ViewScope,
    net: &crate::service::trackers::TrackerNet,
) -> Result<RunOutcome, IpcError> {
    run_item_with(store, ssh, reg, start, role, view, net, None).await
}

/// [`run_item`] with `context` added to the first prompt: a retry's last
/// error, a reviewer's findings, the command a test run is to run
/// (orchestration O4). It is quoted as what a run left, not an order.
#[allow(clippy::too_many_arguments)]
pub async fn run_item_with(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<crate::ssh::SshClient>,
    reg: &Arc<crate::cancel::CancellationRegistry>,
    start: &StartArgs,
    role: &str,
    view: &ViewScope,
    net: &crate::service::trackers::TrackerNet,
    context: Option<&str>,
) -> Result<RunOutcome, IpcError> {
    let item_id = start
        .item_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "run needs item_id"))?;
    // The task is recorded only once its worker exists: a second start of
    // the same item and role in that gap (the mission loop's step and a
    // person's "start next steps", a double click) would pass `precheck`
    // too and start a second worker. The claim is taken under the same lock
    // as the check (review r06 F2).
    let (attempt, _starting) = {
        let s = lock(store.as_ref())?;
        let attempt = match precheck(&s, view, item_id, role)? {
            Precheck::Existing(task) => {
                return Ok(RunOutcome {
                    session_id: task.worker_session_id,
                    task: *task,
                    existing: true,
                })
            }
            Precheck::Fresh { attempt } => attempt,
        };
        let claim = crate::rt::claim(
            &STARTING,
            Arc::as_ptr(store) as usize,
            format!("{item_id}:{role}"),
        )
        .ok_or_else(|| {
            IpcError::new(
                codes::E_INVALID_STATE,
                format!("a {role} run on this item is already starting"),
            )
        })?;
        (attempt, claim)
    };
    // A run always starts its own worker: a live session already on the
    // item (another role's run, a failed attempt's session that lives on,
    // a person's own) is not a reason to refuse, but a reason for this
    // attempt to get its own `-N` checkout, as a parallel start does.
    let (parallel, prior) = {
        let s = lock(store.as_ref())?;
        (
            start.parallel || has_live_work(&s, item_id)?,
            prior_work(&s, item_id)?,
        )
    };
    let start = StartArgs {
        item_id: Some(item_id),
        reference: None,
        with_brief: true,
        parallel,
        // Where the earlier work is: its host when nobody chose one and it
        // is up, and its branch as the new checkout's start (P2,
        // [`prior_work`]).
        host_alias: start.host_alias.clone().or_else(|| {
            prior
                .as_ref()
                .filter(|p| p.host_up)
                .map(|p| p.host_alias.clone())
        }),
        base_branch: prior.as_ref().map(|p| p.branch.clone()),
        ..start.clone()
    };
    let (row, plan, queued) =
        tickets::start_work_unprompted(store, ssh, reg, &start, view, net).await?;
    let (prompt, task) = {
        let s = lock(store.as_ref())?;
        let notes = s.get_work_item(item_id)?.and_then(|i| i.notes);
        let mut prompt = run_prompt(&plan.key, &plan.title, notes.as_deref(), role, queued);
        if let Some(c) = context.map(str::trim).filter(|c| !c.is_empty()) {
            prompt.push_str(&format!(
                "\n\nWhat the mission's earlier runs left (their words, quoted):\n> {}",
                c.replace('\n', "\n> ")
            ));
        }
        if let Some(p) = &prior {
            prompt.push_str("\n\n");
            prompt.push_str(&prior_work_note(role, &p.branch));
        }
        let claude = plan
            .agent
            .as_deref()
            .is_none_or(|a| a == crate::store::AGENT_CLAUDE);
        if let Some(n) = subagent_note(role).filter(|_| {
            claude
                && crate::service::settings::get_bool(
                    &s,
                    crate::service::settings::ORCHESTRATOR_RUN_SUBAGENTS,
                )
        }) {
            prompt.push_str("\n\n");
            prompt.push_str(n);
        }
        let task = record_run(&s, row.id, &prompt, item_id, attempt, role)?;
        (prompt, task)
    };
    crate::service::work::resume::spawn_run_prompt(
        Arc::clone(store),
        Arc::clone(ssh),
        &row,
        crate::service::work::report::with_run_instruction(&prompt, task.id, &task.nonce),
        task.id,
    );
    Ok(RunOutcome {
        task,
        session_id: Some(row.id),
        existing: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store with a host, one session (the worker) and a native task;
    /// answers the worker's and the task's ids.
    fn store_with_item() -> (Store, i64, i64) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        let worker = s
            .upsert_session("dev-o-r--w", "h", None, None, 1, 1, "running", None)
            .unwrap();
        let item = s
            .create_native_item(&crate::store::NativeItem {
                title: "Queue schema",
                parent_id: None,
                project_id: None,
                notes: None,
            })
            .unwrap();
        (s, worker, item.id)
    }

    /// A worker session on `host` in a checkout of `branch`.
    fn worker_in(s: &Store, host: &str, branch: &str) -> i64 {
        s.upsert_host(host).unwrap();
        let pid = s.upsert_project("o", "r", "/src/r").unwrap();
        let wt = s
            .upsert_worktree_on(
                host,
                pid,
                branch,
                &format!("/src/r/.worktrees/{branch}"),
                Some(branch),
            )
            .unwrap();
        s.upsert_session(
            &format!("dev-{branch}"),
            host,
            Some(pid),
            Some(wt),
            1,
            1,
            "running",
            None,
        )
        .unwrap()
    }

    /// P2 (lanes design 2026-10-23): a run beside the work it continues or
    /// checks starts from the newest implement attempt's branch, on its
    /// host; a review's or a test's checkout is not the work.
    #[test]
    fn prior_work_is_the_newest_implement_attempts_branch() {
        let (s, _, item) = store_with_item();
        assert_eq!(prior_work(&s, item).unwrap(), None, "nothing ran yet");
        let first = worker_in(&s, "h", "task-7-queue-schema");
        let t = record_run(&s, first, "do it", item, 1, "implement").unwrap();
        crate::service::tasks::fail_task(&s, t.id, "tests fail").unwrap();
        let review = worker_in(&s, "k", "task-7-queue-schema-3");
        record_run(&s, review, "check it", item, 1, "review").unwrap();
        assert_eq!(
            prior_work(&s, item).unwrap(),
            Some(PriorWork {
                host_alias: "h".into(),
                branch: "task-7-queue-schema".into(),
                host_up: true,
            })
        );
        // A host whose last probe went unanswered is not where a run goes.
        s.update_host_probe("h", false, None, None, 2).unwrap();
        assert!(!prior_work(&s, item).unwrap().unwrap().host_up);
        s.update_host_probe("h", true, None, None, 3).unwrap();
        let second = worker_in(&s, "h", "task-7-queue-schema-2");
        record_run(&s, second, "again", item, 2, "implement").unwrap();
        assert_eq!(
            prior_work(&s, item).unwrap().map(|p| p.branch),
            Some("task-7-queue-schema-2".into())
        );
    }

    /// A branch a prompt cannot carry as is (a host reported it) is no
    /// prior work; an attempt with no checkout of its own is passed over.
    #[test]
    fn prior_work_skips_odd_branches_and_checkout_less_attempts() {
        let (s, plain, item) = store_with_item();
        let odd = worker_in(&s, "h", "x`; rm -rf ~ #");
        record_run(&s, odd, "do it", item, 1, "implement").unwrap();
        record_run(&s, plain, "do it", item, 2, "implement").unwrap();
        assert_eq!(prior_work(&s, item).unwrap(), None);
        assert!(!crate::service::agents::workspace::branch_ok("-delete"));
        assert!(crate::service::agents::workspace::branch_ok("feat/x_1.2"));
    }

    #[test]
    fn the_subagent_note_is_for_the_roles_that_read_or_write_code() {
        let n = subagent_note("implement").unwrap();
        assert!(
            n.contains("Agent tool") && n.contains("never have one edit"),
            "{n}"
        );
        assert!(subagent_note("review").unwrap().contains("only"));
        assert_eq!(subagent_note("test"), None, "a test runs its commands");
    }

    #[test]
    fn the_prior_work_note_names_the_branch_by_role() {
        let n = prior_work_note("implement", "task-7-x");
        assert!(n.contains("`task-7-x`") && n.contains("continue it"), "{n}");
        let n = prior_work_note("review", "task-7-x");
        assert!(
            n.contains("The work to review is on branch `task-7-x`"),
            "{n}"
        );
        assert!(n.contains("git fetch origin task-7-x"), "{n}");
    }

    /// A worker whose first prompt never went in fails its run, so the
    /// attempt frees its parallel slot and the next one is attempt 2.
    #[test]
    fn a_run_whose_prompt_was_not_typed_fails() {
        let (s, worker, item) = store_with_item();
        let t = record_run(&s, worker, "do it", item, 1, "implement").unwrap();
        let store = Mutex::new(s);
        crate::service::work::resume::run_prompt_undelivered(&store, t.id);
        let s = store.lock().unwrap();
        assert_eq!(s.get_task(t.id).unwrap().unwrap().state, "failed");
        assert_eq!(
            precheck(&s, &ViewScope::internal(), item, "implement").unwrap(),
            Precheck::Fresh { attempt: 2 }
        );
    }

    /// The undelivered run's worker is answered for stopping only when it
    /// is the mission's own, still running, and the task was still open.
    #[test]
    fn an_undelivered_runs_mission_worker_is_answered_for_stopping() {
        let (s, worker, item) = store_with_item();
        let t = record_run(&s, worker, "do it", item, 1, "implement").unwrap();
        let store = Mutex::new(s);
        // Not a mission's session: left alone.
        assert_eq!(
            crate::service::work::resume::run_prompt_undelivered(&store, t.id),
            None
        );
        let t = record_run(
            &store.lock().unwrap(),
            worker,
            "again",
            item,
            2,
            "implement",
        )
        .unwrap();
        store
            .lock()
            .unwrap()
            .set_session_origin(worker, &crate::store::SessionOrigin::mission(7))
            .unwrap();
        assert_eq!(
            crate::service::work::resume::run_prompt_undelivered(&store, t.id),
            Some(("h".to_string(), "dev-o-r--w".to_string()))
        );
        // Already failed: nothing more to stop.
        assert_eq!(
            crate::service::work::resume::run_prompt_undelivered(&store, t.id),
            None
        );
    }

    #[test]
    fn a_first_run_is_attempt_one_and_a_second_is_the_open_one() {
        let (s, worker, item) = store_with_item();
        assert_eq!(
            precheck(&s, &ViewScope::internal(), item, "implement").unwrap(),
            Precheck::Fresh { attempt: 1 }
        );
        let t = record_run(&s, worker, "do it", item, 1, "implement").unwrap();
        assert_eq!(t.state, "running");
        assert_eq!(
            (t.work_item_id, t.attempt, t.role.as_deref()),
            (Some(item), Some(1), Some("implement"))
        );
        // Idempotent while open.
        assert_eq!(
            precheck(&s, &ViewScope::internal(), item, "implement").unwrap(),
            Precheck::Existing(Box::new(t.clone()))
        );
        // Another role is its own lane.
        assert_eq!(
            precheck(&s, &ViewScope::internal(), item, "review").unwrap(),
            Precheck::Fresh { attempt: 1 }
        );
        // Once it ended, the next attempt is number two.
        tasks::fail_task(&s, t.id, "boom").unwrap();
        assert_eq!(
            precheck(&s, &ViewScope::internal(), item, "implement").unwrap(),
            Precheck::Fresh { attempt: 2 }
        );
        assert_eq!(s.tasks_for_item(item).unwrap().len(), 1);
    }

    #[test]
    fn a_run_never_mirrors_its_job_as_another_item() {
        let (s, worker, item) = store_with_item();
        let before: i64 = s
            .conn_ref()
            .query_row("SELECT COUNT(*) FROM work_items", [], |r| r.get(0))
            .unwrap();
        record_run(&s, worker, "do it", item, 1, "implement").unwrap();
        let after: i64 = s
            .conn_ref()
            .query_row("SELECT COUNT(*) FROM work_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, after);
        assert_eq!(s.get_work_item(item).unwrap().unwrap().task_id, None);
    }

    #[test]
    fn a_run_beside_a_live_session_is_planned_parallel() {
        let (s, worker, item) = store_with_item();
        assert!(!has_live_work(&s, item).unwrap());
        s.link_session_work(worker, crate::store::WorkTarget::Item(item), "manual")
            .unwrap();
        assert!(has_live_work(&s, item).unwrap());
    }

    #[test]
    fn a_jobs_mirror_item_is_not_run() {
        let (s, _, item) = store_with_item();
        s.conn_ref()
            .execute(
                "UPDATE work_items SET origin = 'agent' WHERE id = ?1",
                [item],
            )
            .unwrap();
        assert_eq!(
            precheck(&s, &ViewScope::internal(), item, "implement")
                .unwrap_err()
                .code,
            codes::E_INVALID_STATE
        );
    }

    #[test]
    fn unknown_roles_proposals_done_and_missing_items_are_refused() {
        let (s, _, item) = store_with_item();
        let code = |r: Result<Precheck, IpcError>| r.unwrap_err().code;
        assert_eq!(
            code(precheck(&s, &ViewScope::internal(), item, "deploy")),
            codes::E_INVALID
        );
        assert_eq!(
            code(precheck(&s, &ViewScope::internal(), 999_999, "implement")),
            codes::E_NOTFOUND
        );
        s.conn_ref()
            .execute(
                "UPDATE work_items SET proposal_state = 'proposed' WHERE id = ?1",
                [item],
            )
            .unwrap();
        assert_eq!(
            code(precheck(&s, &ViewScope::internal(), item, "implement")),
            codes::E_INVALID_STATE
        );
        s.conn_ref()
            .execute(
                "UPDATE work_items SET proposal_state = NULL, status_category = 'done' \
                 WHERE id = ?1",
                [item],
            )
            .unwrap();
        assert_eq!(
            code(precheck(&s, &ViewScope::internal(), item, "implement")),
            codes::E_INVALID_STATE
        );
    }

    #[test]
    fn the_prompt_points_at_the_brief_only_when_one_was_queued() {
        let with = run_prompt(
            "TASK-7",
            "Queue schema",
            Some("IndexedDB"),
            "implement",
            true,
        );
        assert!(with.contains("fleet brief"), "{with}");
        let without = run_prompt(
            "TASK-7",
            "Queue schema",
            Some("IndexedDB"),
            "implement",
            false,
        );
        assert!(!without.contains("fleet brief"), "{without}");
        assert!(without.contains("IndexedDB"), "{without}");
        let review = run_prompt("TASK-7", "Queue schema", None, "review", true);
        assert!(
            review.ends_with("Your role on this task: review."),
            "{review}"
        );
    }

    /// Review r06 F2: while one start of an item and role is between its
    /// check and its task row, a second start of the same pair is refused
    /// before it reaches a host, rather than starting a second worker.
    #[tokio::test]
    async fn a_second_start_of_a_starting_run_is_refused() {
        let (s, _worker, item) = store_with_item();
        let store = Arc::new(Mutex::new(s));
        let first = crate::rt::claim(
            &STARTING,
            Arc::as_ptr(&store) as usize,
            format!("{item}:implement"),
        )
        .expect("free");
        let start = StartArgs {
            item_id: Some(item),
            ..Default::default()
        };
        let err = run_item_with(
            &store,
            &Arc::new(crate::ssh::SshClient::new()),
            &crate::cancel::CancellationRegistry::new(),
            &start,
            "implement",
            &ViewScope::internal(),
            &crate::service::trackers::TrackerNet::fake(Arc::new(
                crate::net::https::FakeTransport::new(),
            )),
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_INVALID_STATE);
        assert!(err.message.contains("already starting"), "{}", err.message);
        drop(first);
    }
}
