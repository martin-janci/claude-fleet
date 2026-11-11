//! The account rules every Claude launch path holds to (docs/accounts.md,
//! *Keep an account off a project*, *Daily limit*, *Signed out*), in one
//! place, for the paths that do not go through `sessions::new_session`:
//! a review session (`sessions::spawn_review`), a background session
//! (`new_bg_session`), a mission's headless review or test
//! (`work::orchestrate::checks`), the AI check on done
//! (`work::ticket_check`), the mission planner and its release-note and
//! triage drafts (`work::orchestrate::limit`), and the PR-reviewer,
//! support and fleet-agent runs (`agents`, `fleet_agents`).
//!
//! A login `profile` (`None` = the host's own) may not take Claude work in
//! a project when:
//! * the project, or its org, forbids its account
//!   (`project_accounts::refuse_forbidden`, `E_ACCOUNT_FORBIDDEN`);
//! * its account spent its daily limit today
//!   (`account_limits::refuse_over_daily_limit`, `E_ACCOUNT_LIMIT`);
//! * fleet knows it is signed out (`login_health::signed_out_reason`,
//!   `E_CLAUDE_CLI`, the code a run that met `/login` answers).
//!
//! Unattended work ([`unattended_hold`]) also waits while the login is at
//! or past `accounts.pause_at`, as a routine does.
//!
//! `run_login_tests::every_claude_launch_path_holds_to_the_account_rules`
//! lists the launch paths and checks each one asks.

use crate::ipc_error::{codes, IpcError};
use crate::service::{account_limits, login_health, project_accounts};
use crate::store::Store;

/// Why a login may not take a Claude run.
#[derive(Debug)]
pub enum Stop {
    /// The project forbids its account.
    Forbidden(IpcError),
    /// Its account spent its daily limit today.
    DailyLimit(IpcError),
    /// Fleet knows it is signed out: the reason, with the command that
    /// signs it in again.
    SignedOut(String),
}

impl Stop {
    /// The refusal a caller answers with.
    pub fn into_error(self) -> IpcError {
        match self {
            Stop::Forbidden(e) | Stop::DailyLimit(e) => e,
            Stop::SignedOut(why) => IpcError::new(codes::E_CLAUDE_CLI, why),
        }
    }

    /// The refusal's words, for automation that skips or holds.
    pub fn reason(&self) -> String {
        match self {
            Stop::Forbidden(e) | Stop::DailyLimit(e) => e.message.clone(),
            Stop::SignedOut(why) => why.clone(),
        }
    }
}

fn norm(profile: Option<&str>) -> Option<&str> {
    profile.map(str::trim).filter(|p| !p.is_empty())
}

/// Why the login `profile` on `host` may not take Claude work in
/// `project_id` (`None`: no project, so no account is forbidden). `None`
/// when it may.
pub fn stop(
    s: &Store,
    project_id: Option<i64>,
    host: &str,
    profile: Option<&str>,
    now: i64,
) -> Result<Option<Stop>, IpcError> {
    let profile = norm(profile);
    match project_accounts::refuse_forbidden(s, project_id, host, profile, now) {
        Ok(()) => {}
        Err(e) if e.code == codes::E_ACCOUNT_FORBIDDEN => return Ok(Some(Stop::Forbidden(e))),
        Err(e) => return Err(e),
    }
    match account_limits::refuse_over_daily_limit(s, host, profile, now) {
        Ok(()) => {}
        Err(e) if e.code == codes::E_ACCOUNT_LIMIT => return Ok(Some(Stop::DailyLimit(e))),
        Err(e) => return Err(e),
    }
    Ok(login_health::signed_out_reason(s, host, profile)?.map(Stop::SignedOut))
}

/// [`stop`] as a refusal: `Ok(())` when the login may take the work.
pub fn refuse(
    s: &Store,
    project_id: Option<i64>,
    host: &str,
    profile: Option<&str>,
    now: i64,
) -> Result<(), IpcError> {
    match stop(s, project_id, host, profile, now)? {
        Some(st) => Err(st.into_error()),
        None => Ok(()),
    }
}

/// The login a Claude run in `project_id` on `host` takes when it would run
/// under `profile` (`None` = the host's own) and nobody chose otherwise:
/// `account_limits::login_with_headroom_for`, which steps off a forbidden
/// login and off one past `accounts.pause_at` when the host has another,
/// then [`refuse`]d. `Err` when the login it lands on may not take it.
pub fn choose(
    s: &Store,
    project_id: Option<i64>,
    host: &str,
    profile: Option<&str>,
    now: i64,
) -> Result<Option<String>, IpcError> {
    let picked = account_limits::login_with_headroom_for(s, project_id, host, norm(profile), now)?;
    refuse(s, project_id, host, picked.as_deref(), now)?;
    Ok(picked)
}

/// Why an unattended run (an agent's job, a held mission step) waits: a
/// [`stop`], or the login at or past `accounts.pause_at`. With the time the
/// line clears, when the reading says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hold {
    pub reason: String,
    /// When a login past the pause line gets under it again (unix seconds);
    /// `None` for a stop that waits on a person, or with no reading.
    pub until: Option<i64>,
}

/// [`stop`], or past the pause line, for a run nobody is waiting on.
pub fn unattended_hold(
    s: &Store,
    project_id: Option<i64>,
    host: &str,
    profile: Option<&str>,
    now: i64,
) -> Result<Option<Hold>, IpcError> {
    let profile = norm(profile);
    if let Some(st) = stop(s, project_id, host, profile, now)? {
        return Ok(Some(Hold {
            reason: st.reason(),
            until: None,
        }));
    }
    Ok(
        account_limits::over_limit(s, host, profile, now)?.map(|o| Hold {
            reason: o.reason(),
            until: crate::service::routines::backoff::limit_resume_at(s, host, profile, now)
                .or(o.resets_at),
        }),
    )
}

#[cfg(test)]
#[path = "run_login_tests.rs"]
mod tests;
