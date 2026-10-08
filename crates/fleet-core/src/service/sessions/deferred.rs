//! Deferred prompts (redesign step 5.10, the Send prompt dialog): "busy
//! sessions get it when they are idle". A prompt for a session that is not
//! between turns is kept in `deferred_prompts` (migration 133) and typed in
//! as a new turn once the session is idle again: from the Stop hook, which
//! is the moment a turn ends, and from the reconcile tick as the backstop for
//! a hook that never came.
//!
//! "Idle" is exactly what an inbox wake pastes into
//! (`messages::wake_action`): a reported `idle`, or a `failed` turn with its
//! REPL back at the input, and never a session waiting on a dialog. Typing
//! into a working REPL would only queue it in Claude Code's own buffer, and
//! Enter into a blocked one would answer the dialog, so neither is done.
//!
//! One prompt goes out per idle moment: the session is working again once it
//! has one, and its next Stop carries the next.

use super::*;
use crate::ipc_error::{codes, lock};
use crate::service::messages::{wake_action, WakeAction};
use crate::store::DeferredPromptRow;

/// How long after a Stop hook the prompt is typed: the hook fires as the
/// turn ends, a moment before the REPL is back at its input.
const AFTER_STOP: std::time::Duration = std::time::Duration::from_millis(1500);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuePromptArgs {
    pub session_id: i64,
    pub prompt: String,
}

/// What [`queue_prompt`] did with one prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuePromptResult {
    pub session_id: i64,
    /// Typed into the pane now: the session was idle.
    pub delivered: bool,
    /// Kept for later: the `deferred_prompts` row, `None` when delivered.
    #[serde(default)]
    pub queued_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedPromptsArgs {
    pub session_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelQueuedPromptArgs {
    pub session_id: i64,
    pub id: i64,
}

/// May a prompt be typed into this row now? PURE.
pub fn idle_for_prompt(row: &SessionRow) -> bool {
    row.status == "running"
        && wake_action(
            false,
            row.claude_status.as_deref(),
            row.stuck_kind.is_some(),
            !crate::store::has_no_pane(&row.kind),
        ) == WakeAction::Paste
}

fn session_row(store: &Mutex<Store>, session_id: i64) -> Result<SessionRow, IpcError> {
    lock(store)?
        .get_session_by_id(session_id)?
        .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("session {session_id} not found")))
}

/// Send a prompt now when the session is idle and nothing is queued ahead of
/// it; otherwise keep it for the session's next idle moment. The body is
/// typed exactly as given (a marker the caller added included).
pub async fn queue_prompt(
    args: QueuePromptArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<QueuePromptResult, IpcError> {
    queue_prompt_with(args, store, |row, body| {
        let ssh = Arc::clone(ssh);
        async move {
            prompt::send_prompt_inner(
                store,
                &ssh,
                &row.host_alias,
                &row.tmux_name,
                &body,
                true,
                prompt::Origin::Person,
            )
            .await
        }
    })
    .await
}

/// [`queue_prompt`] with the typing step handed in, so the decision is
/// testable without a pane.
pub async fn queue_prompt_with<F, Fut>(
    args: QueuePromptArgs,
    store: &Mutex<Store>,
    type_it: F,
) -> Result<QueuePromptResult, IpcError>
where
    F: FnOnce(SessionRow, String) -> Fut,
    Fut: std::future::Future<Output = Result<(), IpcError>>,
{
    let body = normalize_prompt_body(&args.prompt)?;
    if body.trim().is_empty() {
        return Err(IpcError::new(codes::E_VALIDATE, "the prompt is empty"));
    }
    let row = session_row(store, args.session_id)?;
    if crate::store::has_no_pane(&row.kind) || row.kind == "shell" {
        return Err(IpcError::new(
            codes::E_VALIDATE,
            format!("session {} has no agent to prompt", row.id),
        ));
    }
    if matches!(row.status.as_str(), "dead" | "stopped") {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("session {} is not running", row.id),
        ));
    }
    let ahead = lock(store)?.next_deferred_prompt(row.id)?.is_some();
    if idle_for_prompt(&row) && !ahead {
        type_it(row.clone(), body).await?;
        return Ok(QueuePromptResult {
            session_id: row.id,
            delivered: true,
            queued_id: None,
        });
    }
    let id = lock(store)?.insert_deferred_prompt(row.id, &body, now_unix())?;
    Ok(QueuePromptResult {
        session_id: row.id,
        delivered: false,
        queued_id: Some(id),
    })
}

/// Type the session's oldest waiting prompt when it is idle. Answers the row
/// that went out, `None` when nothing did (not idle, nothing waiting, or a
/// racing call took it).
pub async fn deliver_due(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    session_id: i64,
) -> Result<Option<DeferredPromptRow>, IpcError> {
    deliver_due_with(store, session_id, |row, body| {
        let ssh = Arc::clone(ssh);
        async move {
            prompt::send_prompt_inner(
                store,
                &ssh,
                &row.host_alias,
                &row.tmux_name,
                &body,
                true,
                prompt::Origin::Person,
            )
            .await
        }
    })
    .await
}

/// [`deliver_due`] with the typing step handed in.
pub async fn deliver_due_with<F, Fut>(
    store: &Mutex<Store>,
    session_id: i64,
    type_it: F,
) -> Result<Option<DeferredPromptRow>, IpcError>
where
    F: FnOnce(SessionRow, String) -> Fut,
    Fut: std::future::Future<Output = Result<(), IpcError>>,
{
    // Read, decide and claim under one lock; type with it released.
    let (row, next) = {
        let s = lock(store)?;
        let Some(row) = s.get_session_by_id(session_id)? else {
            return Ok(None);
        };
        if !idle_for_prompt(&row) {
            return Ok(None);
        }
        let Some(next) = s.next_deferred_prompt(session_id)? else {
            return Ok(None);
        };
        if !s.claim_deferred_prompt(next.id, now_unix())? {
            return Ok(None);
        }
        (row, next)
    };
    match type_it(row, next.body.clone()).await {
        Ok(()) => Ok(Some(next)),
        Err(e) => {
            let gave_up = lock(store)?.release_deferred_prompt(next.id, &e.message, now_unix())?;
            tracing::warn!(
                session_id,
                prompt = next.id,
                gave_up,
                error = %e.message,
                "[deferred] the queued prompt was not typed"
            );
            Err(e)
        }
    }
}

/// The Stop hook's follow-up: give the REPL a moment, then type the next
/// waiting prompt if the session is still idle. Off the hook handler, like
/// every other Stop follow-up.
pub fn spawn_deliver_after_stop(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>, session_id: i64) {
    let waiting = lock(store)
        .ok()
        .and_then(|s| s.next_deferred_prompt(session_id).ok().flatten())
        .is_some();
    if !waiting {
        return;
    }
    let store = Arc::clone(store);
    let ssh = Arc::clone(ssh);
    crate::rt::spawn(async move {
        tokio::time::sleep(AFTER_STOP).await;
        if let Err(e) = deliver_due(&store, &ssh, session_id).await {
            tracing::debug!(session_id, error = %e.message, "[deferred] delivery after Stop failed");
        }
    });
}

/// The reconcile tick's backstop: every session with a waiting prompt that
/// is idle now gets its next one. Single-flight and off the tick body, since
/// each delivery is an SSH round trip.
pub fn spawn_deliver_all_due(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static RUNNING: AtomicBool = AtomicBool::new(false);
    let ids = match lock(store).and_then(|s| Ok(s.sessions_with_deferred_prompts()?)) {
        Ok(ids) if !ids.is_empty() => ids,
        Ok(_) => return,
        Err(e) => {
            tracing::debug!(error = %e.message, "[deferred] waiting sessions not read");
            return;
        }
    };
    if RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let store = Arc::clone(store);
    let ssh = Arc::clone(ssh);
    crate::rt::spawn(async move {
        for id in ids {
            if let Err(e) = deliver_due(&store, &ssh, id).await {
                tracing::debug!(session_id = id, error = %e.message, "[deferred] delivery failed");
            }
        }
        RUNNING.store(false, Ordering::Release);
    });
}

/// What is waiting (and what failed) for one session.
pub fn queued_prompts(
    args: QueuedPromptsArgs,
    store: &Mutex<Store>,
) -> Result<Vec<DeferredPromptRow>, IpcError> {
    Ok(lock(store)?.list_deferred_prompts(Some(args.session_id))?)
}

/// Take back a prompt of `session_id`'s that has not gone out.
pub fn cancel_queued_prompt(
    args: CancelQueuedPromptArgs,
    store: &Mutex<Store>,
) -> Result<(), IpcError> {
    let s = lock(store)?;
    let mine = s
        .get_deferred_prompt(args.id)?
        .is_some_and(|r| r.session_id == args.session_id);
    if mine && s.cancel_deferred_prompt(args.id, now_unix())? {
        Ok(())
    } else {
        Err(IpcError::new(
            codes::E_NOTFOUND,
            format!(
                "no prompt {} is waiting; it went out, failed or was cancelled",
                args.id
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn store_with(status: &str) -> (Mutex<Store>, i64) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("s", "local", None, None, 0, 0, "running", None)
            .unwrap();
        let store = Mutex::new(s);
        set_status(&store, id, status);
        (store, id)
    }

    fn set_status(store: &Mutex<Store>, id: i64, status: &str) {
        store
            .lock()
            .unwrap()
            .conn_ref()
            .execute(
                "UPDATE sessions SET claude_status = ?2 WHERE id = ?1",
                rusqlite::params![id, status],
            )
            .unwrap();
    }

    fn args(id: i64, p: &str) -> QueuePromptArgs {
        QueuePromptArgs {
            session_id: id,
            prompt: p.into(),
        }
    }

    #[tokio::test]
    async fn an_idle_session_gets_the_prompt_at_once() {
        let (store, id) = store_with("idle");
        let typed = Mutex::new(Vec::<String>::new());
        let r = queue_prompt_with(args(id, "status?"), &store, |_, b| {
            typed.lock().unwrap().push(b);
            async { Ok(()) }
        })
        .await
        .unwrap();
        assert!(r.delivered && r.queued_id.is_none());
        assert_eq!(*typed.lock().unwrap(), vec!["status?".to_string()]);
    }

    #[tokio::test]
    async fn a_queued_prompt_is_delivered_when_the_session_goes_idle() {
        let (store, id) = store_with("working");
        let r = queue_prompt_with(args(id, "rebase on main"), &store, |_, _| async {
            panic!("a working session is never typed into")
        })
        .await
        .unwrap();
        assert!(!r.delivered);
        let qid = r.queued_id.unwrap();

        // Still working: nothing goes out.
        let out = deliver_due_with(&store, id, |_, _| async { panic!("not idle yet") })
            .await
            .unwrap();
        assert!(out.is_none());

        // The turn ends: the prompt is typed, once.
        set_status(&store, id, "idle");
        let typed = Mutex::new(Vec::<(String, String)>::new());
        let out = deliver_due_with(&store, id, |row, b| {
            typed.lock().unwrap().push((row.tmux_name, b));
            async { Ok(()) }
        })
        .await
        .unwrap();
        assert_eq!(out.map(|r| r.id), Some(qid));
        assert_eq!(
            *typed.lock().unwrap(),
            vec![("s".to_string(), "rebase on main".to_string())]
        );
        let again = deliver_due_with(&store, id, |_, _| async { panic!("typed twice") })
            .await
            .unwrap();
        assert!(again.is_none());
        assert!(store
            .lock()
            .unwrap()
            .list_deferred_prompts(Some(id))
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn a_blocked_session_keeps_the_prompt_until_it_is_idle() {
        let (store, id) = store_with("blocked");
        let r = queue_prompt_with(args(id, "continue"), &store, |_, _| async {
            panic!("Enter would answer the dialog")
        })
        .await
        .unwrap();
        assert!(r.queued_id.is_some());
        let out = deliver_due_with(&store, id, |_, _| async { panic!("still blocked") })
            .await
            .unwrap();
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn prompts_go_out_in_order_one_per_idle_moment() {
        let (store, id) = store_with("working");
        for p in ["one", "two"] {
            queue_prompt_with(args(id, p), &store, |_, _| async { panic!("working") })
                .await
                .unwrap();
        }
        // Idle now, but "one" is still waiting: a new prompt queues behind it.
        set_status(&store, id, "idle");
        let r = queue_prompt_with(args(id, "three"), &store, |_, _| async {
            panic!("would jump the queue")
        })
        .await
        .unwrap();
        assert!(!r.delivered);
        let mut sent = Vec::new();
        for _ in 0..3 {
            set_status(&store, id, "idle");
            let row = deliver_due_with(&store, id, |_, _| async { Ok(()) })
                .await
                .unwrap()
                .unwrap();
            sent.push(row.body);
            set_status(&store, id, "working");
            let none = deliver_due_with(&store, id, |_, _| async { panic!("working") })
                .await
                .unwrap();
            assert!(none.is_none());
        }
        assert_eq!(sent, ["one", "two", "three"]);
    }

    #[tokio::test]
    async fn a_failed_typing_hands_the_prompt_back() {
        let (store, id) = store_with("working");
        let qid = queue_prompt_with(args(id, "p"), &store, |_, _| async { panic!() })
            .await
            .unwrap()
            .queued_id
            .unwrap();
        set_status(&store, id, "idle");
        let calls = AtomicUsize::new(0);
        let err = deliver_due_with(&store, id, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(IpcError::new(codes::E_TMUX, "no pane")) }
        })
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_TMUX);
        let row = store
            .lock()
            .unwrap()
            .get_deferred_prompt(qid)
            .unwrap()
            .unwrap();
        assert_eq!((row.delivered_at, row.attempts), (None, 1));
        assert_eq!(
            store
                .lock()
                .unwrap()
                .next_deferred_prompt(id)
                .unwrap()
                .map(|r| r.id),
            Some(qid)
        );
    }

    #[tokio::test]
    async fn a_cancelled_prompt_never_goes_out() {
        let (store, id) = store_with("working");
        let qid = queue_prompt_with(args(id, "p"), &store, |_, _| async { panic!() })
            .await
            .unwrap()
            .queued_id
            .unwrap();
        cancel_queued_prompt(
            CancelQueuedPromptArgs {
                session_id: id,
                id: qid,
            },
            &store,
        )
        .unwrap();
        assert_eq!(
            cancel_queued_prompt(
                CancelQueuedPromptArgs {
                    session_id: id,
                    id: qid
                },
                &store
            )
            .unwrap_err()
            .code,
            codes::E_NOTFOUND
        );
        set_status(&store, id, "idle");
        let out = deliver_due_with(&store, id, |_, _| async { panic!("cancelled") })
            .await
            .unwrap();
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn an_empty_prompt_or_a_shell_is_refused() {
        let (store, id) = store_with("idle");
        let e = queue_prompt_with(args(id, "  \n"), &store, |_, _| async { Ok(()) })
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::E_VALIDATE);
        store
            .lock()
            .unwrap()
            .set_session_kind(id, "shell", None)
            .unwrap();
        let e = queue_prompt_with(args(id, "ls"), &store, |_, _| async { Ok(()) })
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::E_VALIDATE);
    }

    /// The Stop hook is the moment a turn ends; it must hand the session's
    /// waiting prompt on, or a queued prompt waits for the next tick.
    #[test]
    fn the_stop_hook_delivers_waiting_prompts() {
        let src = crate::repo_files::read("crates/fleet-core/src/service/hooks.rs");
        let body = &src[src.find("fn apply_stop_hook(").unwrap()..];
        let body = &body[..body.find("\n}\n").unwrap()];
        assert!(body.contains("deferred::spawn_deliver_after_stop("));
    }
}
