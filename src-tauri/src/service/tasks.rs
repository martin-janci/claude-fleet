//! Task objects and wait primitives (Wave 3 Track E: MCP-1, MCP-5, PROD-6).
//!
//! A *task* is one unit of work a requester session dispatches to a worker
//! session: the prompt is delivered with an appended instruction asking the
//! worker to print `FLEET_TASK_DONE_<nonce>` on its own line followed by a
//! one-paragraph result. The nonce generalises the safe-kill marker pattern
//! (`service::safe_kill`): it makes the assistant's emission distinguishable
//! from the prompt echo still visible in the pane, and unforgeable by a
//! third session. On every Stop hook of a worker with open tasks fleet reads
//! the worker's transcript (falling back to a pane capture), scans for the
//! marker and flips the task to `done` with the paragraph as `result`. A
//! Stop without the marker leaves the task `running`.
//!
//! The wait primitives are bounded long-polls over the store: they lock,
//! read one row, unlock, sleep — never holding the mutex across the sleep.

use crate::ipc_error::IpcError;
use crate::ssh::SshClient;
use crate::store::{SessionRow, Store, TaskRow, IDLE_STATUSES, TASK_TERMINAL_STATES};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Marker prefix the worker is asked to print; the task nonce follows.
pub const DONE_PREFIX: &str = "FLEET_TASK_DONE_";

/// Poll interval of the wait primitives.
pub const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Default / hard cap for `timeout_s` on the wait tools.
pub const DEFAULT_WAIT_SECS: u64 = 120;
pub const MAX_WAIT_SECS: u64 = 600;

/// Cap on the stored `result` paragraph.
const RESULT_MAX_CHARS: usize = 4_000;
/// Pane scrollback depth consulted when the transcript cannot be read.
const PANE_SCAN_LINES: u32 = 3_000;

/// 8 random hex chars — same shape as the safe-kill nonce.
pub fn make_nonce() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 4];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The full marker for a task: `FLEET_TASK_DONE_<nonce>`.
pub fn done_marker(nonce: &str) -> String {
    format!("{DONE_PREFIX}{nonce}")
}

/// The instruction line appended to every dispatched prompt.
pub fn task_instruction(nonce: &str) -> String {
    format!(
        "When finished, print exactly {} on its own line followed by a one-paragraph result.",
        done_marker(nonce)
    )
}

/// The prompt as delivered to the worker: the requester's text plus the
/// instruction, separated by a blank line.
pub fn with_instruction(prompt: &str, nonce: &str) -> String {
    format!("{}\n\n{}", prompt.trim_end(), task_instruction(nonce))
}

/// Strip TUI chrome a captured pane puts in front of assistant text
/// (`⏺ `, `│ `, `> `, bullets, indentation).
fn strip_chrome(line: &str) -> &str {
    line.trim_start_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .trim_end()
}

/// Scan assistant text (a transcript turn or a pane capture) for the task
/// marker. The marker must stand on its own line — nothing but chrome
/// before it and nothing after it — which excludes the prompt echo (`…print
/// exactly FLEET_TASK_DONE_x on its own line…`). The LAST such line wins;
/// the paragraph after it (up to the first blank line once text started,
/// capped at [`RESULT_MAX_CHARS`]) is the result. `None` when the worker has
/// not emitted the marker yet.
pub fn scan_for_done(text: &str, nonce: &str) -> Option<String> {
    let marker = done_marker(nonce);
    let lines: Vec<&str> = text.lines().collect();
    let idx = lines.iter().rposition(|l| strip_chrome(l) == marker)?;
    let mut para: Vec<String> = Vec::new();
    let mut chars = 0usize;
    for l in &lines[idx + 1..] {
        let t = l.trim();
        if t.is_empty() {
            if para.is_empty() {
                continue;
            }
            break;
        }
        let t = strip_chrome(t);
        if t.is_empty() {
            continue;
        }
        chars += t.chars().count() + 1;
        para.push(t.to_string());
        if chars >= RESULT_MAX_CHARS {
            break;
        }
    }
    let joined = para.join(" ");
    Some(joined.chars().take(RESULT_MAX_CHARS).collect())
}

/// True for `done` / `failed` / `cancelled`.
pub fn is_terminal(state: &str) -> bool {
    TASK_TERMINAL_STATES.contains(&state)
}

/// Clamp a caller-supplied `timeout_s` into `[0, MAX_WAIT_SECS]`, defaulting
/// to [`DEFAULT_WAIT_SECS`].
pub fn wait_timeout(timeout_s: Option<u64>) -> Duration {
    Duration::from_secs(timeout_s.unwrap_or(DEFAULT_WAIT_SECS).min(MAX_WAIT_SECS))
}

/// What `wait_for_session` waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitCond {
    /// `claude_status` is one of idle | completed | stopped | failed (no
    /// turn in progress). Note this is also true for a session that never
    /// started a turn — use `TurnGt` after a `send_prompt` for "the reply
    /// to MY prompt is in".
    Idle,
    /// `turn_seq` strictly greater than the given value.
    TurnGt(i64),
}

impl WaitCond {
    /// Parse the tool's `until` / `turn` pair. `E_INVALID` on an unknown
    /// `until` or a missing `turn` for `turn_gt`.
    pub fn parse(until: &str, turn: Option<i64>) -> Result<WaitCond, IpcError> {
        match until {
            "idle" => Ok(WaitCond::Idle),
            "turn_gt" => turn
                .map(WaitCond::TurnGt)
                .ok_or_else(|| IpcError::new("E_INVALID", "until=turn_gt requires `turn`")),
            other => Err(IpcError::new(
                "E_INVALID",
                format!("until must be \"idle\" or \"turn_gt\", got {other:?}"),
            )),
        }
    }
}

/// PURE: does `row` satisfy `cond`?
pub fn session_satisfies(row: &SessionRow, cond: WaitCond) -> bool {
    match cond {
        WaitCond::Idle => row
            .claude_status
            .as_deref()
            .is_some_and(|s| IDLE_STATUSES.contains(&s) || s == "failed"),
        WaitCond::TurnGt(t) => row.turn_seq > t,
    }
}

/// Outcome of a bounded wait.
#[derive(Debug)]
pub struct WaitOutcome<T> {
    pub satisfied: bool,
    pub row: T,
}

/// Bounded long-poll until `cond` holds for `session_id` or `timeout`
/// elapses. `E_NOTFOUND` when the row disappears.
pub async fn wait_for_session(
    store: &Mutex<Store>,
    session_id: i64,
    cond: WaitCond,
    timeout: Duration,
) -> Result<WaitOutcome<SessionRow>, IpcError> {
    wait_for_session_with(store, session_id, cond, timeout, POLL_INTERVAL).await
}

pub async fn wait_for_session_with(
    store: &Mutex<Store>,
    session_id: i64,
    cond: WaitCond,
    timeout: Duration,
    poll: Duration,
) -> Result<WaitOutcome<SessionRow>, IpcError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        // Lock, read one row, unlock — never across the sleep.
        let row = {
            let s = store.lock().map_err(|_| IpcError::lock())?;
            s.get_session_by_id(session_id)?.ok_or_else(|| {
                IpcError::new("E_NOTFOUND", format!("session {session_id} not found"))
            })?
        };
        if session_satisfies(&row, cond) {
            return Ok(WaitOutcome {
                satisfied: true,
                row,
            });
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(WaitOutcome {
                satisfied: false,
                row,
            });
        }
        tokio::time::sleep(poll.min(deadline - now)).await;
    }
}

/// Bounded long-poll until task `task_id` reaches a terminal state or
/// `timeout` elapses. `E_NOTFOUND` for an unknown task.
pub async fn wait_for_task(
    store: &Mutex<Store>,
    task_id: i64,
    timeout: Duration,
) -> Result<WaitOutcome<TaskRow>, IpcError> {
    wait_for_task_with(store, task_id, timeout, POLL_INTERVAL).await
}

pub async fn wait_for_task_with(
    store: &Mutex<Store>,
    task_id: i64,
    timeout: Duration,
    poll: Duration,
) -> Result<WaitOutcome<TaskRow>, IpcError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let row = {
            let s = store.lock().map_err(|_| IpcError::lock())?;
            s.get_task(task_id)?
                .ok_or_else(|| IpcError::new("E_NOTFOUND", format!("task {task_id} not found")))?
        };
        if is_terminal(&row.state) {
            return Ok(WaitOutcome {
                satisfied: true,
                row,
            });
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(WaitOutcome {
                satisfied: false,
                row,
            });
        }
        tokio::time::sleep(poll.min(deadline - now)).await;
    }
}

/// Create a `queued` task. `prompt` is the requester's text (without the
/// instruction line — that is added at delivery so the stored prompt reads
/// as written). Records a `task_dispatched` event on the requester.
pub fn create_task(
    s: &Store,
    requester_session_id: Option<i64>,
    worker_session_id: Option<i64>,
    prompt: &str,
) -> Result<TaskRow, IpcError> {
    if prompt.trim().is_empty() {
        return Err(IpcError::new("E_VALIDATE", "task prompt must be non-empty"));
    }
    let nonce = make_nonce();
    let task = s.insert_task(requester_session_id, worker_session_id, prompt, &nonce)?;
    if let Some(req) = requester_session_id {
        let _ = s.insert_session_event(
            req,
            "task_dispatched",
            Some(&format!("task={} worker={:?}", task.id, worker_session_id)),
        );
    }
    Ok(task)
}

/// Mark a task `running` once its prompt landed in the worker's pane;
/// records `task_started` on the worker.
pub fn start_task(s: &Store, task: &TaskRow) -> Result<TaskRow, IpcError> {
    let row = s
        .mark_task_running(task.id)?
        .ok_or_else(|| IpcError::new("E_NOTFOUND", format!("task {} vanished", task.id)))?;
    if let Some(w) = row.worker_session_id {
        let _ = s.insert_session_event(w, "task_started", Some(&format!("task={}", row.id)));
    }
    Ok(row)
}

/// Fail an open task (delivery error, worker gone). No-op on a terminal one.
pub fn fail_task(s: &Store, task_id: i64, error: &str) -> Result<Option<TaskRow>, IpcError> {
    let (row, changed) = s.finish_task(task_id, "failed", None, Some(error))?;
    if changed {
        if let Some(ref r) = row {
            note_finished(s, r, "task_failed", error);
        }
    }
    Ok(row)
}

/// Cancel an open task. `E_NOTFOUND` for an unknown id, `E_TASK_TERMINAL`
/// when it already finished. The worker session is left running — the
/// caller decides whether to also kill it.
pub fn cancel_task(s: &Store, task_id: i64, reason: &str) -> Result<TaskRow, IpcError> {
    let (row, changed) = s.finish_task(task_id, "cancelled", None, Some(reason))?;
    let row =
        row.ok_or_else(|| IpcError::new("E_NOTFOUND", format!("task {task_id} not found")))?;
    if !changed {
        return Err(IpcError::new(
            "E_TASK_TERMINAL",
            format!("task {task_id} is already {}", row.state),
        ));
    }
    note_finished(s, &row, "task_cancelled", reason);
    Ok(row)
}

/// Complete an open task with the worker's result paragraph. Besides the
/// row and timeline events, the result is delivered to the requester's inbox
/// (kind `task_result`, best-effort) so a requester that is not polling
/// `wait_for_task` still sees it. Returns whether this call did the flip.
pub fn complete_task(s: &Store, task: &TaskRow, result: &str) -> Result<bool, IpcError> {
    let result = if result.trim().is_empty() {
        "(no result text after the marker)"
    } else {
        result
    };
    let (row, changed) = s.finish_task(task.id, "done", Some(result), None)?;
    if !changed {
        return Ok(false);
    }
    if let Some(ref r) = row {
        note_finished(s, r, "task_done", result);
        if let (Some(w), Some(req)) = (r.worker_session_id, r.requester_session_id) {
            if w != req {
                let body = format!("[task #{} done] {result}", r.id);
                let _ = s.insert_message(w, req, &body, "task_result", None);
            }
        }
    }
    Ok(true)
}

fn note_finished(s: &Store, row: &TaskRow, kind: &str, detail: &str) {
    let detail: String = format!("task={} {}", row.id, detail)
        .chars()
        .take(200)
        .collect();
    for sid in [row.requester_session_id, row.worker_session_id]
        .into_iter()
        .flatten()
    {
        let _ = s.insert_session_event(sid, kind, Some(&detail));
    }
}

/// Per-host token scoping (E3): a per-host caller may only see / wait on
/// tasks it requested from its host or that target a worker on its host.
/// PURE over the two endpoint rows.
pub fn task_visible_from_host(
    task: &TaskRow,
    requester: Option<&SessionRow>,
    worker: Option<&SessionRow>,
    host: &str,
) -> bool {
    let on_host = |r: Option<&SessionRow>, id: Option<i64>| {
        id.is_some() && r.is_some_and(|row| row.host_alias == host)
    };
    on_host(requester, task.requester_session_id) || on_host(worker, task.worker_session_id)
}

/// `task_visible_from_host` resolved against the store.
pub fn task_visible_to(s: &Store, task: &TaskRow, host: Option<&str>) -> Result<bool, IpcError> {
    let Some(host) = host else {
        return Ok(true);
    };
    let requester = match task.requester_session_id {
        Some(id) => s.get_session_by_id(id)?,
        None => None,
    };
    let worker = match task.worker_session_id {
        Some(id) => s.get_session_by_id(id)?,
        None => None,
    };
    Ok(task_visible_from_host(
        task,
        requester.as_ref(),
        worker.as_ref(),
        host,
    ))
}

/// Tasks visible to a caller: all of them for the master token, host-scoped
/// for a per-host token.
pub fn list_tasks_for(
    store: &Mutex<Store>,
    requester_session_id: Option<i64>,
    state: Option<&str>,
    limit: i64,
    host: Option<&str>,
) -> Result<Vec<TaskRow>, IpcError> {
    if let Some(st) = state {
        if !crate::store::TASK_STATES.contains(&st) {
            return Err(IpcError::new(
                "E_INVALID",
                format!(
                    "state must be one of {}",
                    crate::store::TASK_STATES.join(" | ")
                ),
            ));
        }
    }
    let s = store.lock().map_err(|_| IpcError::lock())?;
    let rows = s.list_tasks(requester_session_id, state, limit)?;
    let mut out = Vec::with_capacity(rows.len());
    for t in rows {
        if task_visible_to(&s, &t, host)? {
            out.push(t);
        }
    }
    Ok(out)
}

/// Called from the Stop hook (off the HTTP handler, on a background task)
/// for a worker with open tasks: read the worker's last transcript turn
/// (`cwd` from the hook payload locates it; the pane is the fallback) and
/// resolve every open task whose marker appears. Errors are logged and
/// swallowed — the next Stop retries.
pub async fn handle_stop_for_worker(
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    worker: SessionRow,
    cwd: Option<String>,
) {
    let open = match store.lock() {
        Ok(s) => s.open_tasks_for_worker(worker.id).unwrap_or_default(),
        Err(_) => return,
    };
    if open.is_empty() {
        return;
    }
    let text = match read_worker_output(&ssh, &worker, cwd).await {
        Ok(t) => t,
        Err(e) => {
            eprintln!(
                "[tasks] could not read output of worker {} ({}/{}): {}",
                worker.id, worker.host_alias, worker.tmux_name, e.message
            );
            return;
        }
    };
    let Ok(s) = store.lock() else { return };
    for task in open {
        if let Some(result) = scan_for_done(&text, &task.nonce) {
            match complete_task(&s, &task, &result) {
                Ok(true) => eprintln!("[tasks] task {} done (worker {})", task.id, worker.id),
                Ok(false) => {}
                Err(e) => eprintln!("[tasks] completing task {} failed: {}", task.id, e.message),
            }
        }
    }
}

/// How long `wait_for_repl_ready` polls a freshly spawned worker's pane.
const REPL_READY_TIMEOUT: Duration = Duration::from_secs(20);
const REPL_READY_POLL: Duration = Duration::from_secs(1);

/// PURE: does a pane tail show the Claude REPL's input chrome (i.e. it will
/// accept a typed prompt)? The same cues `pane_intel::derive_status` treats
/// as idle.
pub fn pane_shows_repl(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        "? for shortcuts",
        "bypass permissions",
        "% used",
        "shift+tab to cycle",
    ]
    .iter()
    .any(|cue| lower.contains(cue))
}

/// After `new_session` the tmux pane exists but Claude may still be
/// starting; text typed before the REPL is up lands in the wrong process.
/// Poll the pane (bounded) until it shows the REPL chrome. Best-effort: on
/// timeout or capture failure the caller proceeds anyway.
pub async fn wait_for_repl_ready(ssh: &Arc<SshClient>, host_alias: &str, tmux_name: &str) {
    let tmux: Box<dyn crate::tmux::TmuxExec> = if host_alias == "local" {
        Box::new(crate::tmux::LocalTmux)
    } else {
        Box::new(crate::tmux::RemoteTmux {
            client: Arc::clone(ssh),
            host: host_alias.to_string(),
        })
    };
    let deadline = tokio::time::Instant::now() + REPL_READY_TIMEOUT;
    loop {
        if let Ok(text) = tmux.capture_pane(tmux_name).await {
            if pane_shows_repl(&text) {
                return;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            eprintln!("[tasks] REPL of {host_alias}/{tmux_name} not ready after {REPL_READY_TIMEOUT:?}; sending anyway");
            return;
        }
        tokio::time::sleep(REPL_READY_POLL).await;
    }
}

/// The worker's latest output: its last transcript turn when the transcript
/// is readable, else a pane scrollback capture.
async fn read_worker_output(
    ssh: &Arc<SshClient>,
    worker: &SessionRow,
    cwd: Option<String>,
) -> Result<String, IpcError> {
    let is_bg = worker.tmux_name.starts_with("bg:");
    if let Some(cid) = worker.claude_session_id.clone() {
        let args = crate::service::transcript::TranscriptArgs {
            host_alias: worker.host_alias.clone(),
            tmux_name: if is_bg {
                None
            } else {
                Some(worker.tmux_name.clone())
            },
            cwd,
            claude_session_id: cid,
            turns: 1,
            max_chars: crate::service::transcript::MAX_MAX_CHARS,
        };
        match crate::service::transcript::fetch_transcript(args, ssh).await {
            Ok(t) => return Ok(t),
            Err(e) => eprintln!(
                "[tasks] transcript of worker {} unavailable ({}); falling back to pane",
                worker.id, e.message
            ),
        }
    }
    if is_bg {
        return Err(IpcError::new(
            "E_NO_TRANSCRIPT",
            "background worker has no pane and no readable transcript",
        ));
    }
    let tmux: Box<dyn crate::tmux::TmuxExec> = if worker.host_alias == "local" {
        Box::new(crate::tmux::LocalTmux)
    } else {
        Box::new(crate::tmux::RemoteTmux {
            client: Arc::clone(ssh),
            host: worker.host_alias.clone(),
        })
    };
    tmux.capture_pane_scrollback(&worker.tmux_name, PANE_SCAN_LINES)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(s: &Store, host: &str, name: &str) -> i64 {
        s.upsert_host(host).unwrap();
        s.upsert_session(name, host, None, None, 0, 0, "running", None)
            .unwrap()
    }

    // ---- marker ----

    #[test]
    fn nonce_and_instruction_shape() {
        let n = make_nonce();
        assert_eq!(n.len(), 8);
        assert!(n.chars().all(|c| c.is_ascii_hexdigit()));
        let p = with_instruction("do the thing\n", "abcd1234");
        assert_eq!(
            p,
            "do the thing\n\nWhen finished, print exactly FLEET_TASK_DONE_abcd1234 on its own line followed by a one-paragraph result."
        );
    }

    #[test]
    fn scan_ignores_the_prompt_echo_and_finds_the_marker_with_noise_around_it() {
        let pane = "\
╭──────────────────────────╮
│ > fix the flaky test     │
│   When finished, print exactly FLEET_TASK_DONE_abcd1234 on its own line followed by a one-paragraph result.
╰──────────────────────────╯
⏺ Looking at the test…
⏺ Bash(cargo test flaky)
  ⎿  ok
⏺ FLEET_TASK_DONE_abcd1234
  The flake was a race in setup; I added a barrier and the test
  passed 50 times in a row.

  Next unrelated paragraph that is not part of the result.
? for shortcuts";
        let r = scan_for_done(pane, "abcd1234").expect("marker found");
        assert_eq!(
            r,
            "The flake was a race in setup; I added a barrier and the test passed 50 times in a row."
        );
        // Only the echo visible → not yet.
        let echo_only = "> …print exactly FLEET_TASK_DONE_abcd1234 on its own line followed by…";
        assert_eq!(scan_for_done(echo_only, "abcd1234"), None);
        // Wrong nonce → not yet.
        assert_eq!(scan_for_done(pane, "ffffffff"), None);
        // Marker alone with nothing after it → done with an empty result.
        assert_eq!(
            scan_for_done("FLEET_TASK_DONE_abcd1234", "abcd1234"),
            Some(String::new())
        );
        // Last emission wins.
        let twice = "FLEET_TASK_DONE_n1\nfirst\n\nFLEET_TASK_DONE_n1\nsecond";
        assert_eq!(scan_for_done(twice, "n1").as_deref(), Some("second"));
    }

    #[test]
    fn scan_result_is_capped() {
        let text = format!("FLEET_TASK_DONE_n1\n{}", "x".repeat(10_000));
        let r = scan_for_done(&text, "n1").unwrap();
        assert_eq!(r.chars().count(), RESULT_MAX_CHARS);
    }

    // ---- wait conditions ----

    #[test]
    fn wait_cond_parses_and_validates() {
        assert_eq!(WaitCond::parse("idle", None).unwrap(), WaitCond::Idle);
        assert_eq!(
            WaitCond::parse("turn_gt", Some(3)).unwrap(),
            WaitCond::TurnGt(3)
        );
        assert_eq!(
            WaitCond::parse("turn_gt", None).unwrap_err().code,
            "E_INVALID"
        );
        assert_eq!(WaitCond::parse("done", None).unwrap_err().code, "E_INVALID");
        assert_eq!(wait_timeout(None), Duration::from_secs(120));
        assert_eq!(wait_timeout(Some(5)), Duration::from_secs(5));
        assert_eq!(wait_timeout(Some(10_000)), Duration::from_secs(600));
    }

    #[test]
    fn session_satisfies_idle_and_turn_gt() {
        let s = Store::open_in_memory().unwrap();
        let id = seed(&s, "local", "w");
        s.set_claude_session_id(id, "uuid-w").unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert!(
            !session_satisfies(&row, WaitCond::Idle),
            "unknown status is not idle"
        );
        assert!(!session_satisfies(&row, WaitCond::TurnGt(0)));
        s.record_prompt_submit_hook("uuid-w").unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert!(!session_satisfies(&row, WaitCond::Idle));
        s.record_stop_hook("uuid-w").unwrap();
        let row = s.get_session_by_id(id).unwrap().unwrap();
        assert!(session_satisfies(&row, WaitCond::Idle));
        assert!(session_satisfies(&row, WaitCond::TurnGt(0)));
        assert!(!session_satisfies(&row, WaitCond::TurnGt(1)));
    }

    #[tokio::test]
    async fn wait_for_session_times_out_then_is_satisfied_by_a_stop() {
        let s = Store::open_in_memory().unwrap();
        let id = seed(&s, "local", "w");
        s.set_claude_session_id(id, "uuid-w").unwrap();
        let store = Arc::new(Mutex::new(s));
        let fast = Duration::from_millis(5);
        let out = wait_for_session_with(
            &store,
            id,
            WaitCond::TurnGt(0),
            Duration::from_millis(30),
            fast,
        )
        .await
        .unwrap();
        assert!(!out.satisfied);
        assert_eq!(out.row.turn_seq, 0);
        // A Stop that lands mid-wait satisfies it.
        let bg = Arc::clone(&store);
        let stopper = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            bg.lock().unwrap().record_stop_hook("uuid-w").unwrap();
        });
        let out = wait_for_session_with(
            &store,
            id,
            WaitCond::TurnGt(0),
            Duration::from_secs(5),
            fast,
        )
        .await
        .unwrap();
        stopper.await.unwrap();
        assert!(out.satisfied);
        assert_eq!(out.row.turn_seq, 1);
        assert!(out.row.last_stop_at.is_some());
        assert_eq!(
            wait_for_session_with(&store, 9999, WaitCond::Idle, fast, fast)
                .await
                .unwrap_err()
                .code,
            "E_NOTFOUND"
        );
    }

    // ---- task state machine ----

    #[test]
    fn task_state_machine_queued_running_done_and_terminal_is_final() {
        let s = Store::open_in_memory().unwrap();
        let req = seed(&s, "local", "ctl");
        let w = seed(&s, "local", "w");
        let t = create_task(&s, Some(req), Some(w), "do it").unwrap();
        assert_eq!(t.state, "queued");
        assert_eq!(t.nonce.len(), 8);
        assert!(t.started_at.is_none());
        let t = start_task(&s, &t).unwrap();
        assert_eq!(t.state, "running");
        assert!(t.started_at.is_some());
        // start is idempotent on a running task.
        assert_eq!(start_task(&s, &t).unwrap().state, "running");
        assert!(complete_task(&s, &t, "all done").unwrap());
        let t = s.get_task(t.id).unwrap().unwrap();
        assert_eq!(t.state, "done");
        assert_eq!(t.result.as_deref(), Some("all done"));
        assert!(t.finished_at.is_some());
        // Terminal: a second completion / cancel / fail changes nothing.
        assert!(!complete_task(&s, &t, "again").unwrap());
        assert_eq!(
            cancel_task(&s, t.id, "late").unwrap_err().code,
            "E_TASK_TERMINAL"
        );
        assert_eq!(fail_task(&s, t.id, "late").unwrap().unwrap().state, "done");
        // The requester got the result in its inbox.
        let inbox = s.list_inbox(req, true, 10).unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].kind, "task_result");
        assert!(inbox[0].body.contains("all done"));
        assert_eq!(inbox[0].from_session_id, w);
        // Timeline on both ends.
        assert!(s
            .list_session_events(req, 10)
            .unwrap()
            .iter()
            .any(|e| e.kind == "task_dispatched"));
        assert!(s
            .list_session_events(w, 10)
            .unwrap()
            .iter()
            .any(|e| e.kind == "task_done"));
        assert!(s
            .list_session_events(req, 10)
            .unwrap()
            .iter()
            .any(|e| e.kind == "task_done"));

        // cancel / fail from queued and running.
        let c = create_task(&s, Some(req), Some(w), "cancel me").unwrap();
        let c = cancel_task(&s, c.id, "user").unwrap();
        assert_eq!(
            (c.state.as_str(), c.error.as_deref()),
            ("cancelled", Some("user"))
        );
        let f = create_task(&s, None, Some(w), "fail me").unwrap();
        let f = start_task(&s, &f).unwrap();
        let f = fail_task(&s, f.id, "send failed").unwrap().unwrap();
        assert_eq!(
            (f.state.as_str(), f.error.as_deref()),
            ("failed", Some("send failed"))
        );
        assert_eq!(cancel_task(&s, 9999, "x").unwrap_err().code, "E_NOTFOUND");
        assert_eq!(
            create_task(&s, None, None, "  ").unwrap_err().code,
            "E_VALIDATE"
        );
        // An empty result paragraph is stored as a placeholder, not "".
        let e = create_task(&s, None, Some(w), "empty").unwrap();
        assert!(complete_task(&s, &e, "  ").unwrap());
        assert_eq!(
            s.get_task(e.id).unwrap().unwrap().result.as_deref(),
            Some("(no result text after the marker)")
        );
    }

    #[test]
    fn open_tasks_for_worker_lists_only_open_ones_oldest_first() {
        let s = Store::open_in_memory().unwrap();
        let w = seed(&s, "local", "w");
        let a = create_task(&s, None, Some(w), "a").unwrap();
        let b = create_task(&s, None, Some(w), "b").unwrap();
        let _other = create_task(&s, None, Some(w + 100), "c").unwrap();
        cancel_task(&s, a.id, "x").unwrap();
        let open: Vec<i64> = s
            .open_tasks_for_worker(w)
            .unwrap()
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(open, vec![b.id]);
    }

    #[tokio::test]
    async fn wait_for_task_times_out_and_returns_on_terminal() {
        let s = Store::open_in_memory().unwrap();
        let w = seed(&s, "local", "w");
        let t = create_task(&s, None, Some(w), "slow").unwrap();
        let store = Arc::new(Mutex::new(s));
        let fast = Duration::from_millis(5);
        let out = wait_for_task_with(&store, t.id, Duration::from_millis(30), fast)
            .await
            .unwrap();
        assert!(!out.satisfied);
        assert_eq!(out.row.state, "queued");
        let bg = Arc::clone(&store);
        let tid = t.id;
        let finisher = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let s = bg.lock().unwrap();
            let task = s.get_task(tid).unwrap().unwrap();
            complete_task(&s, &task, "finished").unwrap();
        });
        let out = wait_for_task_with(&store, t.id, Duration::from_secs(5), fast)
            .await
            .unwrap();
        finisher.await.unwrap();
        assert!(out.satisfied);
        assert_eq!(out.row.state, "done");
        assert_eq!(
            wait_for_task_with(&store, 9999, fast, fast)
                .await
                .unwrap_err()
                .code,
            "E_NOTFOUND"
        );
    }

    // ---- per-host scoping ----

    #[test]
    fn per_host_scoping_sees_only_own_requests_or_own_workers() {
        let s = Store::open_in_memory().unwrap();
        let ctl_a = seed(&s, "hosta", "ctl-a");
        let w_a = seed(&s, "hosta", "w-a");
        let ctl_b = seed(&s, "hostb", "ctl-b");
        let w_b = seed(&s, "hostb", "w-b");
        let mine = create_task(&s, Some(ctl_a), Some(w_b), "a asks b").unwrap();
        let target = create_task(&s, Some(ctl_b), Some(w_a), "b asks a").unwrap();
        let foreign = create_task(&s, Some(ctl_b), Some(w_b), "b internal").unwrap();
        let orphan = create_task(&s, None, None, "unassigned").unwrap();
        let store = Mutex::new(s);
        let ids = |host: Option<&str>| -> Vec<i64> {
            let mut v: Vec<i64> = list_tasks_for(&store, None, None, 50, host)
                .unwrap()
                .iter()
                .map(|t| t.id)
                .collect();
            v.sort();
            v
        };
        assert_eq!(ids(Some("hosta")), vec![mine.id, target.id]);
        assert_eq!(ids(Some("hostb")), vec![mine.id, target.id, foreign.id]);
        assert_eq!(ids(None), vec![mine.id, target.id, foreign.id, orphan.id]);
        assert!(ids(Some("hostc")).is_empty());
        // Filters compose with scoping.
        let s = store.lock().unwrap();
        assert!(!task_visible_to(&s, &orphan, Some("hosta")).unwrap());
        assert!(task_visible_to(&s, &orphan, None).unwrap());
        drop(s);
        assert_eq!(
            list_tasks_for(&store, Some(ctl_b), None, 50, Some("hosta"))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            list_tasks_for(&store, None, Some("bogus"), 50, None)
                .unwrap_err()
                .code,
            "E_INVALID"
        );
        assert_eq!(
            list_tasks_for(&store, None, Some("queued"), 50, None)
                .unwrap()
                .len(),
            4
        );
    }
}
