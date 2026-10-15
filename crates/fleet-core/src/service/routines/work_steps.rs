//! A routine run's work in its trace (`service::run_trace`): each tool its
//! session finished, read from the session's transcript tail while the run
//! is open, so a run's timeline shows what Claude did between its prompt
//! and its settle. The tool running right now is the session row's
//! `current_activity`, which every client already has; the trace keeps only
//! finished calls, each once (by its `tool_use` id).

use super::trace;
use crate::ipc_error::{lock, IpcError};
use crate::service::run_trace::{routine_ref, Step};
use crate::service::transcript::{ConvItem, Conversation};
use crate::ssh::SshClient;
use crate::store::{SessionRow, Store};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Turns of transcript one read takes: a run's session is fresh, so its
/// work is its last turn or two.
pub const WORK_TURNS: usize = 3;
/// Characters of transcript one read keeps.
pub const WORK_MAX_CHARS: usize = 64_000;
/// The longest tool summary a step keeps, in characters.
pub const TOOL_SUMMARY_MAX: usize = 200;

/// One finished tool call in a session's transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    /// The `tool_use` id.
    pub id: String,
    /// `"Bash"`, `"Edit"`, `"Task"`, `"mcp__x__y"`, …
    pub name: String,
    /// The transcript's one-liner for it.
    pub summary: String,
    pub target: Option<String>,
    pub error: bool,
    /// Unix seconds of its `tool_use` and its `tool_result`.
    pub at: Option<i64>,
    pub ended_at: Option<i64>,
}

/// Reads the tools a run's session finished: the session's transcript in
/// production, a fake in tests.
#[async_trait::async_trait]
pub trait ToolReader: Send + Sync {
    async fn tools(&self, row: &SessionRow) -> Result<Vec<ToolCall>, IpcError>;
}

/// [`ToolReader`] through the session's transcript, locally or over SSH.
pub struct TranscriptTools {
    pub store: Arc<Mutex<Store>>,
    pub ssh: Arc<SshClient>,
}

#[async_trait::async_trait]
impl ToolReader for TranscriptTools {
    async fn tools(&self, row: &SessionRow) -> Result<Vec<ToolCall>, IpcError> {
        let conv = crate::service::transcript::fetch_conversation_for_row(
            &self.store,
            &self.ssh,
            row,
            None,
            WORK_TURNS,
            WORK_MAX_CHARS,
            0,
        )
        .await?;
        Ok(tool_calls(&conv))
    }
}

fn unix(iso: Option<&str>) -> Option<i64> {
    iso.and_then(crate::service::account_usage::parse_rfc3339)
}

fn clip(s: &str) -> String {
    match s.char_indices().nth(TOOL_SUMMARY_MAX) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// PURE: the finished tool calls in `conv`, oldest first. A call still
/// waiting for its result, or one without an id, is left out.
pub fn tool_calls(conv: &Conversation) -> Vec<ToolCall> {
    let mut out = Vec::new();
    for item in conv.turns.iter().flat_map(|t| &t.items) {
        match item {
            ConvItem::Tool {
                summary,
                error,
                id: Some(id),
                name,
                target,
                at,
                ended_at,
                done: true,
            } => out.push(ToolCall {
                id: id.clone(),
                name: name.clone(),
                summary: clip(summary),
                target: target.clone(),
                error: *error,
                at: unix(at.as_deref()),
                ended_at: unix(ended_at.as_deref()),
            }),
            ConvItem::Subagent {
                id: Some(id),
                name,
                agent_type,
                description,
                error,
                at,
                ended_at,
                done: true,
                ..
            } => {
                let what = description.as_deref().unwrap_or("a subagent");
                let summary = match agent_type {
                    Some(t) => format!("{name} ({t}): {what}"),
                    None => format!("{name}: {what}"),
                };
                out.push(ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    summary: clip(&summary),
                    target: None,
                    error: *error,
                    at: unix(at.as_deref()),
                    ended_at: unix(ended_at.as_deref()),
                });
            }
            _ => {}
        }
    }
    out
}

/// PURE: the step that records `call`, at `now` when it carries no time.
pub fn tool_step(call: &ToolCall, now: i64) -> Step {
    let at = call.ended_at.or(call.at).unwrap_or(now);
    let kind = if call.error { "error" } else { "tool_call" };
    let summary = if call.error {
        format!("{} failed", call.summary)
    } else {
        call.summary.clone()
    };
    let mut step = Step::new(at, "working", kind, "claude", summary).data(serde_json::json!({
        "tool_use_id": call.id,
        "tool": call.name,
        "target": call.target,
        "error": call.error,
        "started_at": call.at,
    }));
    if let (Some(a), Some(b)) = (call.at, call.ended_at) {
        step = step.ms((b - a).max(0) * 1000);
    }
    step
}

/// Record on run `run_id` each of `calls` its trace does not have yet and
/// that began at or after `started_at`. Answers how many were recorded.
pub fn record_new(
    s: &Store,
    run_id: i64,
    started_at: i64,
    calls: &[ToolCall],
    now: i64,
) -> Result<usize, IpcError> {
    let seen: HashSet<String> = s.run_trace_tool_ids(&routine_ref(run_id))?;
    let mut n = 0;
    for call in calls {
        if seen.contains(&call.id) || call.at.is_some_and(|a| a < started_at) {
            continue;
        }
        trace::write_one(s, run_id, tool_step(call, now));
        n += 1;
    }
    Ok(n)
}

/// The open runs with a live session: `(run id, started_at, session)`.
fn open_runs(s: &Store) -> Result<Vec<(i64, i64, SessionRow)>, IpcError> {
    let mut out = Vec::new();
    for run in s.running_routine_runs()? {
        let Some(sid) = run.session_id else { continue };
        if let Some(row) = s.get_session_by_id(sid)?.filter(|r| r.lost_at.is_none()) {
            out.push((run.id, run.started_at, row));
        }
    }
    Ok(out)
}

/// Read each open run's session once and record the tools it finished.
pub async fn read_open(store: &Arc<Mutex<Store>>, reader: &dyn ToolReader, now: i64) {
    let runs = match lock(store).and_then(|s| open_runs(&s)) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e.message, "[routines] open runs not read for their work");
            return;
        }
    };
    for (run_id, started_at, row) in runs {
        let calls = match reader.tools(&row).await {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!(run = run_id, error = %e.message, "[routines] run's transcript not read");
                continue;
            }
        };
        if let Ok(s) = lock(store) {
            if let Err(e) = record_new(&s, run_id, started_at, &calls, now) {
                tracing::warn!(run = run_id, error = %e.message, "[routines] run's work not recorded");
            }
        }
    }
}

static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// [`read_open`] off the scheduler's path, one pass at a time: a pass still
/// reading when the next tick comes is not doubled.
pub fn spawn_pass(store: &Arc<Mutex<Store>>, reader: Arc<dyn ToolReader>, now: i64) {
    let any = lock(store)
        .ok()
        .is_some_and(|s| s.running_routine_runs().is_ok_and(|r| !r.is_empty()));
    if !any || IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return;
    }
    let store = Arc::clone(store);
    let spawned = crate::rt::try_spawn(async move {
        let _flight = crate::rt::ClearOnDrop::of_static(&IN_FLIGHT);
        read_open(&store, reader.as_ref(), now).await;
    });
    if spawned.is_none() {
        IN_FLIGHT.store(false, Ordering::SeqCst);
    }
}
