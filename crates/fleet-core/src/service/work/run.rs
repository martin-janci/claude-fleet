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

/// Whether a live session is already on the item: a run then starts beside
/// it, in its own `-N` checkout, instead of being refused with "jump to it".
pub fn has_live_work(s: &Store, item_id: i64) -> Result<bool, IpcError> {
    match s.get_work_item(item_id)?.and_then(|i| i.key) {
        Some(key) => Ok(!tickets::live_work_on(s, &key, s.item_org(item_id)?)?.is_empty()),
        None => Ok(false),
    }
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
    let item_id = start
        .item_id
        .ok_or_else(|| IpcError::new(codes::E_INVALID, "run needs item_id"))?;
    let checked = precheck(&*lock(store.as_ref())?, view, item_id, role)?;
    let attempt = match checked {
        Precheck::Existing(task) => {
            return Ok(RunOutcome {
                session_id: task.worker_session_id,
                task: *task,
                existing: true,
            })
        }
        Precheck::Fresh { attempt } => attempt,
    };
    // A run always starts its own worker: a live session already on the
    // item (another role's run, a failed attempt's session that lives on,
    // a person's own) is not a reason to refuse, but a reason for this
    // attempt to get its own `-N` checkout, as a parallel start does.
    let parallel = start.parallel || has_live_work(&*lock(store.as_ref())?, item_id)?;
    let start = StartArgs {
        item_id: Some(item_id),
        reference: None,
        with_brief: true,
        parallel,
        ..start.clone()
    };
    let (row, plan, queued) =
        tickets::start_work_unprompted(store, ssh, reg, &start, view, net).await?;
    let (prompt, task) = {
        let s = lock(store.as_ref())?;
        let notes = s.get_work_item(item_id)?.and_then(|i| i.notes);
        let prompt = run_prompt(&plan.key, &plan.title, notes.as_deref(), role, queued);
        let task = record_run(&s, row.id, &prompt, item_id, attempt, role)?;
        (prompt, task)
    };
    crate::service::work::resume::spawn_start_prompt(
        Arc::clone(store),
        Arc::clone(ssh),
        &row,
        crate::service::work::report::with_run_instruction(&prompt, &task.nonce),
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
}
