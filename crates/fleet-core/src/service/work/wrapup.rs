//! "Finish task": a wrap-up request that asks a live session for its task result.
//!
//! `work_link { action: wrap_up, session_id }` (design 2026-10-13 D2). The
//! request is a `tasks` row of role `wrapup` with the session as its own
//! worker, so the Stop hook resolves it exactly as it resolves a run
//! (`service::tasks::handle_stop_for_worker`): the done marker closes it and
//! the report block beside it becomes the item's result.
//!
//! * **Only an idle REPL**, and never the operator's own session: the rules
//!   of an agent-written handover ([`super::agent_handover`]), reused.
//! * **One open request per session**: `E_EXISTS` until the Stop hook, a
//!   cancel or a failed delivery closes it — or until the session's turn
//!   ended without the marker (a Stop after the request, more than
//!   [`STOP_GRACE_SECS`] ago): then a new request replaces the old one,
//!   which is cancelled, so a reply that left out the block (or an Esc)
//!   does not block "Finish task" until the task's age limit.
//! * **Not an attempt.** The role is outside `run::RUN_ROLES`.

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::orgs::{self, OrgScope};
use crate::service::tasks;
use crate::ssh::SshClient;
use crate::store::{Store, TaskRow};
use std::sync::{Arc, Mutex};

/// Not in `run::RUN_ROLES`: `work_link { run }` cannot ask for it, and
/// attempts count within `(item, role)`, so it never reads as an attempt.
pub const WRAPUP_ROLE: &str = "wrapup";

/// How long after the Stop that ended a request's turn the request still
/// blocks a new one: the Stop hook reads the marker in the background, so
/// a request whose result is about to land is not replaced.
pub const STOP_GRACE_SECS: i64 = 30;

/// PURE: what the session is asked, before the report instruction.
pub fn wrapup_prompt(key: &str) -> String {
    format!(
        "Wrap up your work on {key}. Do not start new work. Commit what is \
         finished, then report what you did and what is left."
    )
}

/// Write the request under the lock. `E_EXISTS` while one is open and its
/// turn has not ended; one whose turn ended without the marker is
/// cancelled and replaced.
pub fn record_wrapup(s: &Store, session_id: i64, item_id: i64) -> Result<TaskRow, IpcError> {
    let open: Vec<TaskRow> = s
        .open_tasks_for_worker(session_id)?
        .into_iter()
        .filter(|t| t.role.as_deref() == Some(WRAPUP_ROLE))
        .collect();
    if !open.is_empty() {
        // A Stop after the request means its turn ended; with no result
        // by now, none is coming from that turn.
        let last_stop = s
            .get_session_by_id(session_id)?
            .and_then(|r| r.last_stop_at);
        let now = crate::store::now_unix();
        let ended = |t: &TaskRow| {
            last_stop.is_some_and(|at| {
                at > t.started_at.unwrap_or(t.created_at) && now - at >= STOP_GRACE_SECS
            })
        };
        if !open.iter().all(ended) {
            return Err(IpcError::new(
                codes::E_EXISTS,
                "a result is already requested from this session",
            ));
        }
        for t in &open {
            tasks::cancel_task(
                s,
                t.id,
                "its turn ended without a result; a new request replaced it",
            )?;
        }
    }
    let key = s
        .get_work_item(item_id)?
        .map(|i| i.key.unwrap_or(i.title))
        .unwrap_or_else(|| format!("item {item_id}"));
    let task = tasks::create_task(s, None, Some(session_id), &wrapup_prompt(&key))?;
    let attempt = s.item_tasks(item_id, WRAPUP_ROLE)?.len() as i64 + 1;
    s.set_task_run(task.id, item_id, attempt, WRAPUP_ROLE)?;
    let task = s
        .get_task(task.id)?
        .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "task vanished after insert"))?;
    tasks::start_task(s, &task)
}

/// `work_link { action: wrap_up, session_id }`: ask the session for its
/// task result. Returns the request's task. A per-host token asks only its
/// own host's sessions, and only about work inside its org.
pub async fn request(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    session_id: i64,
    scope: &OrgScope,
) -> Result<TaskRow, IpcError> {
    let (row, task) = {
        let s = lock(store)?;
        let mut row = s
            .get_session_by_id(session_id)?
            .ok_or_else(|| orgs::not_found("session", session_id))?;
        // Both arms are the ORG half. The person half is
        // `resolve_row_person_gated(.., Reach::Drive, ..)` in
        // `work_link { action: wrap_up }` — drive, because a wrap-up
        // request types into the session's pane.
        if let Some(h) = scope.host() {
            // Org half; `resolve_row_person_gated` is the person half.
            if row.host_alias != h || !scope.sees_row_org_only(&row) {
                return Err(orgs::not_found("session", session_id));
            }
            // Org half; `resolve_row_person_gated` is the person half.
        } else if !scope.sees_row_org_only(&row) {
            // A client bound to another org (M14): an unknown session.
            return Err(orgs::not_found("session", session_id));
        }
        crate::service::operator::refuse_if_operator(
            &s,
            &row.host_alias,
            &row.tmux_name,
            "wrap up",
        )?;
        // The item is read off the redacted row, so work outside the
        // caller's org answers as no work at all.
        scope.redact_row_org_only(&mut row);
        let item_id = row.work.as_ref().and_then(|w| w.item_id).ok_or_else(|| {
            IpcError::new(
                codes::E_INVALID,
                format!(
                    "{} is not linked to any work item; link it first, so the result has a task",
                    row.tmux_name
                ),
            )
        })?;
        let why = match super::agent_handover::refusal(&row) {
            Some(why) => Some(why),
            None => super::agent_handover::conversation_refusal(&s, &row)?,
        };
        if let Some(why) = why {
            return Err(IpcError::new(codes::E_NOT_ALIVE, why));
        }
        let task = record_wrapup(&s, row.id, item_id)?;
        (row, task)
    };
    let prompt = crate::service::work::report::with_run_instruction(
        task.prompt.as_deref().unwrap_or_default(),
        &task.nonce,
    );
    if let Err(e) = crate::service::sessions::send_system_prompt(
        &row.host_alias,
        &row.tmux_name,
        &prompt,
        true,
        store,
        ssh,
    )
    .await
    {
        if let Ok(s) = store.lock() {
            let _ = tasks::fail_task(&s, task.id, &format!("not delivered: {}", e.message));
        }
        return Err(e);
    }
    Ok(task)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    /// A session linked (primary, confirmed) to a local item.
    fn linked(s: &Store) -> (i64, i64) {
        let sid = crate::service::tasks::tests_support::seed(s, "h1", "dev-a");
        let item = s
            .create_native_item(&crate::store::NativeItem {
                title: "Add gate",
                ..Default::default()
            })
            .unwrap();
        s.link_session_work_as(
            sid,
            crate::store::WorkTarget::Item(item.id),
            "manual",
            true,
            None,
        )
        .unwrap();
        (sid, item.id)
    }

    fn set_status(s: &Store, sid: i64, status: &str) {
        s.conn_for_test()
            .execute(
                "UPDATE sessions SET claude_status = ?1 WHERE id = ?2",
                rusqlite::params![status, sid],
            )
            .unwrap();
    }

    #[test]
    fn a_wrapup_is_a_running_task_of_role_wrapup_on_the_sessions_item() {
        let s = Store::open_in_memory().unwrap();
        let (sid, item) = linked(&s);
        let t = record_wrapup(&s, sid, item).unwrap();
        assert_eq!(t.state, "running");
        assert_eq!(t.role.as_deref(), Some(WRAPUP_ROLE));
        assert_eq!(t.work_item_id, Some(item));
        assert_eq!(t.attempt, Some(1));
        assert_eq!(t.worker_session_id, Some(sid));
        assert_eq!(t.requester_session_id, None);
    }

    #[test]
    fn a_wrapup_is_not_an_implement_attempt() {
        let s = Store::open_in_memory().unwrap();
        let (sid, item) = linked(&s);
        record_wrapup(&s, sid, item).unwrap();
        assert!(s.latest_item_task(item, "implement").unwrap().is_none());
        assert!(s.item_tasks(item, "implement").unwrap().is_empty());
        assert!(!crate::service::work::run::RUN_ROLES.contains(&WRAPUP_ROLE));
    }

    #[test]
    fn a_second_wrapup_while_one_is_open_is_refused() {
        let s = Store::open_in_memory().unwrap();
        let (sid, item) = linked(&s);
        let first = record_wrapup(&s, sid, item).unwrap();
        let e = record_wrapup(&s, sid, item).unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_EXISTS);
        // Once it is closed the next one is the item's second.
        tasks::fail_task(&s, first.id, "not delivered").unwrap();
        let second = record_wrapup(&s, sid, item).unwrap();
        assert_eq!(second.attempt, Some(2));
    }

    #[test]
    fn a_wrapup_whose_turn_ended_without_a_result_is_replaced() {
        let s = Store::open_in_memory().unwrap();
        let (sid, item) = linked(&s);
        let first = record_wrapup(&s, sid, item).unwrap();
        let started = first.started_at.unwrap_or(first.created_at);
        let stop_at = |at: i64| {
            s.conn_for_test()
                .execute(
                    "UPDATE sessions SET last_stop_at = ?1 WHERE id = ?2",
                    rusqlite::params![at, sid],
                )
                .unwrap();
        };
        // A Stop from before the request says nothing about it.
        stop_at(started - 5);
        let e = record_wrapup(&s, sid, item).unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_EXISTS);
        // A Stop just now: the hook may still be reading the marker.
        stop_at(crate::store::now_unix());
        let e = record_wrapup(&s, sid, item).unwrap_err();
        assert_eq!(e.code, crate::ipc_error::codes::E_EXISTS);
        // The turn ended a while ago with no marker: replaced.
        s.conn_for_test()
            .execute(
                "UPDATE tasks SET started_at = ?1, created_at = ?1 WHERE id = ?2",
                rusqlite::params![started - 100, first.id],
            )
            .unwrap();
        stop_at(started - 60);
        let second = record_wrapup(&s, sid, item).unwrap();
        assert_eq!(second.attempt, Some(2));
        assert_eq!(s.get_task(first.id).unwrap().unwrap().state, "cancelled");
    }

    #[test]
    fn the_prompt_asks_for_no_new_work() {
        let p = wrapup_prompt("CF-12");
        assert!(p.contains("CF-12"));
        assert!(p.contains("Do not start new work"));
    }

    #[tokio::test]
    async fn a_request_needs_a_work_item_and_an_idle_repl() {
        let s = Store::open_in_memory().unwrap();
        let bare = crate::service::tasks::tests_support::seed(&s, "h1", "dev-b");
        let (sid, _item) = linked(&s);
        s.set_claude_session_id(sid, "conv-1").unwrap();
        set_status(&s, bare, "idle");
        set_status(&s, sid, "working");
        let st = Mutex::new(s);
        let ssh = Arc::new(SshClient::new());

        let err = request(&st, &ssh, bare, &OrgScope::All).await.unwrap_err();
        assert_eq!(err.code, codes::E_INVALID, "no work: {}", err.message);
        let err = request(&st, &ssh, sid, &OrgScope::All).await.unwrap_err();
        assert_eq!(err.code, codes::E_NOT_ALIVE, "{}", err.message);
        // An unknown session and another host's token answer alike.
        let err = request(&st, &ssh, sid + 100, &OrgScope::All)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
        let other = OrgScope::Host {
            alias: "h2".into(),
            org: None,
            isolated: Default::default(),
        };
        let err = request(&st, &ssh, sid, &other).await.unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
        // A refusal leaves no request behind.
        let s = st.lock().unwrap();
        assert!(s.open_tasks_for_worker(sid).unwrap().is_empty());
        assert!(s.open_tasks_for_worker(bare).unwrap().is_empty());
    }
}
