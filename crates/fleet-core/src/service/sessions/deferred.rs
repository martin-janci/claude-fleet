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
//!
//! Send later's time choices (M15 step G1.8, migration 155) narrow "idle" per
//! prompt: `not_before` holds it until a time ("In 1 hour", "Tomorrow
//! 09:00", "At…"), `until_limit_reset` while the session's account is at or
//! past `accounts.pause_at`, and `skip_if_archived` drops it instead once the
//! session is archived. A prompt that is not due yet does not hold back one
//! queued after it that is. The reconcile tick is the clock: a timed prompt
//! goes in on the first tick at which its time has come and the session is
//! idle.

use super::*;
use crate::ipc_error::{codes, lock};
use crate::service::messages::{wake_action, WakeAction};
use crate::store::{DeferredPromptRow, DeferredTiming};

/// How long after a Stop hook the prompt is typed: the hook fires as the
/// turn ends, a moment before the REPL is back at its input.
const AFTER_STOP: std::time::Duration = std::time::Duration::from_millis(1500);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueuePromptArgs {
    pub session_id: i64,
    pub prompt: String,
    /// Send later: not typed before this unix second. Absent: the next idle
    /// moment, as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before: Option<i64>,
    /// Send later "When the limit resets": held while the session's account
    /// is at or past `accounts.pause_at`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub until_limit_reset: bool,
    /// "Skip it if the session is archived first".
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skip_if_archived: bool,
}

impl QueuePromptArgs {
    fn timing(&self) -> DeferredTiming {
        DeferredTiming {
            not_before: self.not_before,
            until_limit_reset: self.until_limit_reset,
            skip_if_archived: self.skip_if_archived,
        }
    }
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

/// Whether the row's work link is archived (collapsed into Done).
fn archived(row: &SessionRow) -> bool {
    row.work.as_ref().is_some_and(|w| w.archived_at.is_some())
}

/// Whether the session's account is known to be at or past the line now.
fn account_over(s: &Store, row: &SessionRow, now: i64) -> Result<bool, IpcError> {
    match row.account_uuid.as_deref() {
        Some(uuid) => crate::service::account_limits::account_over_line(s, uuid, now),
        None => Ok(false),
    }
}

/// The prompt among `due` (oldest first, their time come) to type now: the
/// first that does not wait for a limit still hit.
fn first_ready(
    s: &Store,
    row: &SessionRow,
    due: Vec<DeferredPromptRow>,
    now: i64,
) -> Result<Option<DeferredPromptRow>, IpcError> {
    let mut over: Option<bool> = None;
    for p in due {
        if p.until_limit_reset {
            let hit = match over {
                Some(h) => h,
                None => *over.insert(account_over(s, row, now)?),
            };
            if hit {
                continue;
            }
        }
        return Ok(Some(p));
    }
    Ok(None)
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
    queue_prompt_with(args, store, now_unix(), |row, body| {
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
    now: i64,
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
    // A row is `running` or `ghost` (lost or killed): a prompt queued for a
    // ghost would wait forever, neither typed nor failed.
    if row.status != "running" {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("session {} is not running", row.id),
        ));
    }
    let timing = args.timing();
    if timing.skip_if_archived && archived(&row) {
        return Err(IpcError::new(
            codes::E_INVALID_STATE,
            format!("session {} is already archived", row.id),
        ));
    }
    let send_now = {
        let s = lock(store)?;
        // Only a prompt that could go out now holds this one back.
        let due = s.due_deferred_prompts(row.id, now)?;
        let ahead = first_ready(&s, &row, due, now)?.is_some();
        let held = timing.not_before.is_some_and(|t| t > now)
            || (timing.until_limit_reset && account_over(&s, &row, now)?);
        idle_for_prompt(&row) && !ahead && !held
    };
    if send_now {
        type_it(row.clone(), body).await?;
        return Ok(QueuePromptResult {
            session_id: row.id,
            delivered: true,
            queued_id: None,
        });
    }
    let id = lock(store)?.insert_deferred_prompt_timed(row.id, &body, timing, now)?;
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
    deliver_due_with(store, session_id, now_unix(), |row, body| {
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
    now: i64,
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
        // Archived first: what was to be skipped then never goes out, idle
        // or not.
        if archived(&row) {
            s.skip_archived_deferred_prompts(session_id, now)?;
        }
        if !idle_for_prompt(&row) {
            return Ok(None);
        }
        let due = s.due_deferred_prompts(session_id, now)?;
        let Some(next) = first_ready(&s, &row, due, now)? else {
            return Ok(None);
        };
        // One prompt per idle moment, whichever path (Stop hook or the
        // reconcile backstop) gets there first.
        if !s.claim_deferred_prompt_in(next.id, now, row.idle_since)? {
            return Ok(None);
        }
        (row, next)
    };
    match type_it(row, next.body.clone()).await {
        Ok(()) => Ok(Some(next)),
        Err(e) => {
            let gave_up = lock(store)?.release_deferred_prompt(next.id, &e.message, now)?;
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
        let _flight = crate::rt::ClearOnDrop::of_static(&RUNNING);
        for id in ids {
            if let Err(e) = deliver_due(&store, &ssh, id).await {
                tracing::debug!(session_id = id, error = %e.message, "[deferred] delivery failed");
            }
        }
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
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn an_idle_session_gets_the_prompt_at_once() {
        let (store, id) = store_with("idle");
        let typed = Mutex::new(Vec::<String>::new());
        let r = queue_prompt_with(args(id, "status?"), &store, now_unix(), |_, b| {
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
        let r = queue_prompt_with(
            args(id, "rebase on main"),
            &store,
            now_unix(),
            |_, _| async { panic!("a working session is never typed into") },
        )
        .await
        .unwrap();
        assert!(!r.delivered);
        let qid = r.queued_id.unwrap();

        // Still working: nothing goes out.
        let out = deliver_due_with(&store, id, now_unix(), |_, _| async {
            panic!("not idle yet")
        })
        .await
        .unwrap();
        assert!(out.is_none());

        // The turn ends: the prompt is typed, once.
        set_status(&store, id, "idle");
        let typed = Mutex::new(Vec::<(String, String)>::new());
        let out = deliver_due_with(&store, id, now_unix(), |row, b| {
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
        let again = deliver_due_with(&store, id, now_unix(), |_, _| async {
            panic!("typed twice")
        })
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
        let r = queue_prompt_with(args(id, "continue"), &store, now_unix(), |_, _| async {
            panic!("Enter would answer the dialog")
        })
        .await
        .unwrap();
        assert!(r.queued_id.is_some());
        let out = deliver_due_with(&store, id, now_unix(), |_, _| async {
            panic!("still blocked")
        })
        .await
        .unwrap();
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn prompts_go_out_in_order_one_per_idle_moment() {
        let (store, id) = store_with("working");
        for p in ["one", "two"] {
            queue_prompt_with(args(id, p), &store, now_unix(), |_, _| async {
                panic!("working")
            })
            .await
            .unwrap();
        }
        // Idle now, but "one" is still waiting: a new prompt queues behind it.
        set_status(&store, id, "idle");
        let r = queue_prompt_with(args(id, "three"), &store, now_unix(), |_, _| async {
            panic!("would jump the queue")
        })
        .await
        .unwrap();
        assert!(!r.delivered);
        let mut sent = Vec::new();
        for _ in 0..3 {
            set_status(&store, id, "idle");
            let row = deliver_due_with(&store, id, now_unix(), |_, _| async { Ok(()) })
                .await
                .unwrap()
                .unwrap();
            sent.push(row.body);
            set_status(&store, id, "working");
            let none = deliver_due_with(&store, id, now_unix(), |_, _| async { panic!("working") })
                .await
                .unwrap();
            assert!(none.is_none());
        }
        assert_eq!(sent, ["one", "two", "three"]);
    }

    /// Review r06: the Stop hook's delivery and the reconcile backstop both
    /// see the same idle moment; only the first types a prompt in it.
    #[tokio::test]
    async fn one_idle_moment_takes_one_prompt_whichever_path_comes_first() {
        let (store, id) = store_with("working");
        for p in ["one", "two"] {
            queue_prompt_with(args(id, p), &store, now_unix(), |_, _| async {
                panic!("working")
            })
            .await
            .unwrap();
        }
        let idle_at = |at: i64| {
            set_status(&store, id, "idle");
            store
                .lock()
                .unwrap()
                .conn_ref()
                .execute(
                    "UPDATE sessions SET idle_since = ?2 WHERE id = ?1",
                    rusqlite::params![id, at],
                )
                .unwrap();
        };
        idle_at(now_unix() - 5);
        let first = deliver_due_with(&store, id, now_unix(), |_, _| async { Ok(()) })
            .await
            .unwrap();
        assert_eq!(first.map(|r| r.body).as_deref(), Some("one"));
        // The other path, same moment (the status still reads idle).
        let second = deliver_due_with(&store, id, now_unix(), |_, _| async {
            panic!("typed twice")
        })
        .await
        .unwrap();
        assert!(second.is_none());
        // The next idle moment carries the next prompt.
        idle_at(now_unix() + 5);
        let next = deliver_due_with(&store, id, now_unix(), |_, _| async { Ok(()) })
            .await
            .unwrap();
        assert_eq!(next.map(|r| r.body).as_deref(), Some("two"));
    }

    #[tokio::test]
    async fn a_failed_typing_hands_the_prompt_back() {
        let (store, id) = store_with("working");
        let qid = queue_prompt_with(args(id, "p"), &store, now_unix(), |_, _| async { panic!() })
            .await
            .unwrap()
            .queued_id
            .unwrap();
        set_status(&store, id, "idle");
        let calls = AtomicUsize::new(0);
        let err = deliver_due_with(&store, id, now_unix(), |_, _| {
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
        let qid = queue_prompt_with(args(id, "p"), &store, now_unix(), |_, _| async { panic!() })
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
        let out = deliver_due_with(&store, id, now_unix(), |_, _| async { panic!("cancelled") })
            .await
            .unwrap();
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn a_prompt_for_a_lost_or_killed_session_is_refused_not_queued() {
        let (store, id) = store_with("working");
        store
            .lock()
            .unwrap()
            .conn_ref()
            .execute("UPDATE sessions SET status = 'ghost' WHERE id = ?1", [id])
            .unwrap();
        let e = queue_prompt_with(args(id, "later"), &store, now_unix(), |_, _| async {
            Ok(())
        })
        .await
        .unwrap_err();
        assert_eq!(e.code, codes::E_INVALID_STATE);
        assert!(store
            .lock()
            .unwrap()
            .next_deferred_prompt(id)
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn an_empty_prompt_or_a_shell_is_refused() {
        let (store, id) = store_with("idle");
        let e = queue_prompt_with(args(id, "  \n"), &store, now_unix(), |_, _| async {
            Ok(())
        })
        .await
        .unwrap_err();
        assert_eq!(e.code, codes::E_VALIDATE);
        store
            .lock()
            .unwrap()
            .set_session_kind(id, "shell", None)
            .unwrap();
        let e = queue_prompt_with(args(id, "ls"), &store, now_unix(), |_, _| async { Ok(()) })
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::E_VALIDATE);
    }

    // --- Send later's time choices (M15 step G1.8), on a clock the test
    // holds: `T` is "now" when the prompt is sent.
    const T: i64 = 1_800_000_000;

    fn timed(id: i64, p: &str, f: impl FnOnce(&mut QueuePromptArgs)) -> QueuePromptArgs {
        let mut a = args(id, p);
        f(&mut a);
        a
    }

    async fn deliver_at(store: &Mutex<Store>, id: i64, now: i64) -> Option<String> {
        deliver_due_with(store, id, now, |_, _| async { Ok(()) })
            .await
            .unwrap()
            .map(|r| r.body)
    }

    #[tokio::test]
    async fn a_prompt_for_later_waits_for_its_time_even_when_idle() {
        let (store, id) = store_with("idle");
        let r = queue_prompt_with(
            timed(id, "in an hour", |a| a.not_before = Some(T + 3_600)),
            &store,
            T,
            |_, _| async { panic!("not before its time") },
        )
        .await
        .unwrap();
        assert!(!r.delivered);
        let row = store
            .lock()
            .unwrap()
            .get_deferred_prompt(r.queued_id.unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(row.not_before, Some(T + 3_600));
        assert_eq!(deliver_at(&store, id, T + 3_599).await, None);
        // Its time has come but the session is busy: still waits.
        set_status(&store, id, "working");
        assert_eq!(deliver_at(&store, id, T + 3_600).await, None);
        set_status(&store, id, "idle");
        assert_eq!(
            deliver_at(&store, id, T + 3_700).await.as_deref(),
            Some("in an hour")
        );
    }

    #[tokio::test]
    async fn a_prompt_not_due_yet_holds_back_nothing_behind_it() {
        let (store, id) = store_with("idle");
        queue_prompt_with(
            timed(id, "tomorrow", |a| a.not_before = Some(T + 86_400)),
            &store,
            T,
            |_, _| async { panic!("not yet") },
        )
        .await
        .unwrap();
        let typed = Mutex::new(Vec::<String>::new());
        let r = queue_prompt_with(args(id, "now"), &store, T, |_, b| {
            typed.lock().unwrap().push(b);
            async { Ok(()) }
        })
        .await
        .unwrap();
        assert!(r.delivered, "the timed one is not ahead of it");
        assert_eq!(*typed.lock().unwrap(), vec!["now".to_string()]);
        // A time in the past is the next idle moment.
        let r = queue_prompt_with(
            timed(id, "late", |a| a.not_before = Some(T - 60)),
            &store,
            T,
            |_, _| async { Ok(()) },
        )
        .await
        .unwrap();
        assert!(r.delivered);
    }

    fn set_account_use(store: &Mutex<Store>, id: i64, pct: f64, resets_at: i64) {
        use crate::service::account_usage::{AccountUsage, Window};
        let s = store.lock().unwrap();
        s.upsert_account(&crate::store::AccountRow {
            uuid: "acct".into(),
            ..Default::default()
        })
        .unwrap();
        s.conn_ref()
            .execute(
                "UPDATE sessions SET account_uuid = 'acct' WHERE id = ?1",
                [id],
            )
            .unwrap();
        s.insert_usage_snapshot(&crate::store::UsageSnapshotRow {
            account_uuid: "acct".into(),
            fetched_at: T,
            usage: AccountUsage {
                five_hour: Some(Window {
                    utilization: pct,
                    resets_at: Some(resets_at),
                }),
                ..Default::default()
            },
            subscription: None,
            source_host: None,
        })
        .unwrap();
    }

    #[tokio::test]
    async fn a_prompt_for_after_the_limit_waits_until_the_window_resets() {
        let (store, id) = store_with("idle");
        set_account_use(&store, id, 100.0, T + 1_000);
        let r = queue_prompt_with(
            timed(id, "after the reset", |a| a.until_limit_reset = true),
            &store,
            T,
            |_, _| async { panic!("the account is at its limit") },
        )
        .await
        .unwrap();
        assert!(!r.delivered);
        // A plain prompt is not held by the limit wait of the one before it.
        let r = queue_prompt_with(args(id, "plain"), &store, T, |_, _| async { Ok(()) })
            .await
            .unwrap();
        assert!(r.delivered);
        assert_eq!(deliver_at(&store, id, T + 999).await, None);
        assert_eq!(
            deliver_at(&store, id, T + 1_000).await.as_deref(),
            Some("after the reset"),
            "the window reset: the reading no longer counts"
        );
    }

    #[tokio::test]
    async fn the_limit_wait_sends_at_once_when_the_account_has_headroom() {
        let (store, id) = store_with("idle");
        set_account_use(&store, id, 20.0, T + 1_000);
        let r = queue_prompt_with(
            timed(id, "go", |a| a.until_limit_reset = true),
            &store,
            T,
            |_, _| async { Ok(()) },
        )
        .await
        .unwrap();
        assert!(r.delivered);
    }

    fn archive(store: &Mutex<Store>, id: i64) {
        let s = store.lock().unwrap();
        s.link_session_work(id, crate::store::WorkTarget::Key("PD-1"), "manual")
            .unwrap();
        s.archive_session_work(id).unwrap();
    }

    #[tokio::test]
    async fn a_prompt_marked_skip_is_dropped_when_the_session_is_archived_first() {
        let (store, id) = store_with("working");
        let skip = queue_prompt_with(
            timed(id, "skip me", |a| a.skip_if_archived = true),
            &store,
            T,
            |_, _| async { panic!("working") },
        )
        .await
        .unwrap()
        .queued_id
        .unwrap();
        queue_prompt_with(args(id, "keep me"), &store, T, |_, _| async {
            panic!("working")
        })
        .await
        .unwrap();
        archive(&store, id);
        // Still working: the archived skip goes anyway, the other waits.
        assert_eq!(deliver_at(&store, id, T + 1).await, None);
        let row = store
            .lock()
            .unwrap()
            .get_deferred_prompt(skip)
            .unwrap()
            .unwrap();
        assert_eq!(row.skipped_at, Some(T + 1));
        set_status(&store, id, "idle");
        assert_eq!(
            deliver_at(&store, id, T + 2).await.as_deref(),
            Some("keep me")
        );
        // Sent to an already archived session with skip on: refused.
        let e = queue_prompt_with(
            timed(id, "too late", |a| a.skip_if_archived = true),
            &store,
            T,
            |_, _| async { Ok(()) },
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, codes::E_INVALID_STATE);
    }

    /// The desktop routes `queue_prompt` to a hub as the args it was given:
    /// the new fields are left out when unset, so an older hub reads the same
    /// call it always did.
    #[test]
    fn unset_time_choices_stay_off_the_wire() {
        let v = serde_json::to_value(args(3, "p")).unwrap();
        assert_eq!(v, serde_json::json!({ "session_id": 3, "prompt": "p" }));
        let v = serde_json::to_value(timed(3, "p", |a| {
            a.not_before = Some(9);
            a.until_limit_reset = true;
            a.skip_if_archived = true;
        }))
        .unwrap();
        assert_eq!(v["not_before"], 9);
        assert_eq!(v["until_limit_reset"], true);
        assert_eq!(v["skip_if_archived"], true);
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
