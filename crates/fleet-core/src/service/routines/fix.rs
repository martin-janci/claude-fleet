//! The named fix of a failed run (Orbit Fleet M15 step G3.8): its error
//! code (migration 159) read as one thing a person can do about it, "Log
//! in again on mac" rather than a generic Fix. The start's own `E_*` codes
//! come from the session start; the scheduler closes a run with one of the
//! `E_RUN_*`, `E_TURN_*` and `E_SESSION_*` codes below.
//!
//! The action is one the desktop and the phone already have: open the
//! routine's editor, its host, the accounts, its session, or run it again.
//! A run that did not fail has no fix.

use crate::ipc_error::codes;
use crate::store::{RoutineRow, RoutineRunRow};
use serde::{Deserialize, Serialize};

/// A run's session went past the routine's run budget.
pub const E_RUN_BUDGET: &str = "E_RUN_BUDGET";
/// A run went past the routine's time cap.
pub const E_RUN_TIME_CAP: &str = "E_RUN_TIME_CAP";
/// No turn finished in `tick::RUN_STALE_SECS`.
pub const E_RUN_STALE: &str = "E_RUN_STALE";
/// The run's turn ended in an API error that is neither below.
pub const E_TURN_FAILED: &str = "E_TURN_FAILED";
/// The run's turn ended in an authentication error (401/403).
pub const E_TURN_AUTH: &str = "E_TURN_AUTH";
/// The run's turn ended at a rate limit.
pub const E_TURN_RATE_LIMIT: &str = "E_TURN_RATE_LIMIT";
/// The run's session was lost.
pub const E_SESSION_LOST: &str = "E_SESSION_LOST";
/// The run's session was removed before its turn finished.
pub const E_SESSION_REMOVED: &str = "E_SESSION_REMOVED";
/// The run started no session.
pub const E_NO_SESSION: &str = "E_NO_SESSION";
/// The session started but its prompt was not queued.
pub const E_PROMPT_NOT_QUEUED: &str = "E_PROMPT_NOT_QUEUED";

/// What a [`RunFix`] does, as the desktop and the phone act on it.
pub const FIX_ACTIONS: [&str; 5] = ["edit", "host", "accounts", "session", "retry"];

/// The one thing to do about a failed run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunFix {
    pub run_id: i64,
    /// The run's error code; `E_INTERNAL` for a run from before codes.
    pub code: String,
    /// The fix in words: "Log in again on mac".
    pub label: String,
    /// One of [`FIX_ACTIONS`]: `edit` opens the routine's editor, `host`
    /// the host named in `host`, `accounts` the accounts, `session` the
    /// run's session, `retry` runs it again.
    pub action: String,
    /// The host it is about: the one the run started on, else the
    /// routine's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

/// Whether a run failed: its state, or what it came to.
pub fn failed(run: &RoutineRunRow) -> bool {
    run.state == "failed" || run.outcome.as_deref() == Some("failed")
}

/// The named fix of `run`, a run of `r`; `None` for a run that did not
/// fail.
pub fn fix(r: &RoutineRow, run: &RoutineRunRow) -> Option<RunFix> {
    if !failed(run) {
        return None;
    }
    let host = run
        .host_alias
        .clone()
        .unwrap_or_else(|| r.host_alias.clone());
    let code = run
        .error_code
        .clone()
        .unwrap_or_else(|| codes::E_INTERNAL.to_string());
    let (label, action) = match code.as_str() {
        E_TURN_AUTH => (format!("Log in again on {host}"), "host"),
        E_TURN_RATE_LIMIT | codes::E_ACCOUNT_LIMIT => (
            "Switch account or wait for the limit".to_string(),
            "accounts",
        ),
        codes::E_SSH
        | codes::E_SSH_TIMEOUT
        | codes::E_HOST_OFFLINE
        | codes::E_AGENT_OFFLINE
        | codes::E_PROBE => match r.fallback_host.as_deref() {
            Some(_) => (format!("Check {host}"), "host"),
            None => (format!("Check {host}, or give it a fallback host"), "host"),
        },
        codes::E_GIT_SETUP
        | codes::E_GIT
        | codes::E_REPO
        | codes::E_NOREPO
        | codes::E_REPO_MISSING
        | codes::E_WORKSPACE_LOCKED
        | codes::E_REPAIR_REQUIRED => (format!("Repair the project on {host}"), "host"),
        E_RUN_BUDGET => ("Raise its run budget".to_string(), "edit"),
        E_RUN_TIME_CAP => ("Raise its time cap".to_string(), "edit"),
        E_RUN_STALE | E_TURN_FAILED => ("Open its session".to_string(), "session"),
        E_SESSION_LOST => ("Restore its session".to_string(), "session"),
        E_SESSION_REMOVED | E_PROMPT_NOT_QUEUED => ("Run it again".to_string(), "retry"),
        _ => ("Check the routine".to_string(), "edit"),
    };
    Some(RunFix {
        run_id: run.id,
        code,
        label,
        action: action.to_string(),
        host: Some(host),
    })
}

/// Where a failure's code comes from in a `stop_failure` detail: the class
/// `hooks::stop_failure_class` wrote first (`auth (…)`, `rate_limit (…)`).
pub fn turn_code(detail: Option<&str>) -> &'static str {
    let d = detail.unwrap_or("");
    if d.starts_with("auth") {
        E_TURN_AUTH
    } else if d.starts_with("rate_limit") {
        E_TURN_RATE_LIMIT
    } else {
        E_TURN_FAILED
    }
}
