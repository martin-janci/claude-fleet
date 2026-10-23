//! The planner at a usage limit (TASK-291, plan
//! `docs/superpowers/plans/2026-10-19-planner-limit-wait.md`).
//!
//! An answer that is Claude Code saying its account's usage limit stopped
//! the run is no refused answer: the planner records a `limit_wait` event
//! for that login and goes to the next login with headroom ([`choose`]: the
//! grant's or the host's own, then the host's other profiles, then the next
//! host the planner may use). With none left, the mission waits until the
//! earliest reset, and the loop asks again once [`choose`] finds a login
//! ([`waiting`]); it never asks into the limit before that.

use super::{event, planner_hosts};
use crate::ipc_error::IpcError;
use crate::service::account_limits;
use crate::service::claude_print::UsageLimit;
use crate::service::routines::backoff::limit_resume_at;
use crate::store::{GrantRow, MissionEventRow, MissionRow, Store};
use serde::{Deserialize, Serialize};

/// The mission event a wait is.
pub const LIMIT_WAIT_EVENT: &str = "limit_wait";
/// The mission event that ends the ask's waits: a later login of the same
/// ask answered (review r8), so the planner's newest word is no wait.
pub const LIMIT_CLEARED_EVENT: &str = "limit_cleared";
/// The planner's words on a mission: its runs, its waits, and an answer
/// after a wait. Only these are read, so a busy log never hides a wait.
const WORD_KINDS: &[&str] = &["planned", LIMIT_WAIT_EVENT, LIMIT_CLEARED_EVENT];
/// Waits are at most this far ahead: Claude's weekly limit and a day.
/// A reset further out (an answer the prompt's text steered, review r8) is
/// not taken as known.
pub const LIMIT_WAIT_MAX_SECS: i64 = 8 * 24 * 3600;
/// Planner runs one ask makes, one login after another, before it waits.
pub const LIMIT_ATTEMPTS: usize = 3;
/// How long a limit is waited out when nothing says when it resets.
pub const LIMIT_WAIT_GUESS_SECS: i64 = 3600;
/// The mission events read for its waits.
const EVENTS_READ: usize = 200;

/// One login on one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Login {
    pub host: String,
    /// `None` = the host's own login; else the credential profile's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

impl Login {
    /// "profile work on mac", "mac's own login".
    pub fn words(&self) -> String {
        match &self.profile {
            Some(p) => format!("profile {p} on {}", self.host),
            None => format!("{}'s own login", self.host),
        }
    }
}

/// The planner waiting on a usage limit: one login's, or (no `login`) the
/// mission's, when every login it may use is at its limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitWait {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<Login>,
    /// When the planner may be asked again (Unix seconds).
    pub resets_at: i64,
    /// Whether `resets_at` is the limit's own (Claude's answer or the
    /// account's usage reading) rather than fleet's guess.
    pub reset_known: bool,
    /// How Claude Code said when it resets ("resets 3pm (Europe/Bratislava)").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
    pub why: String,
}

/// The logins the planner may use, in the order it tries them: on each of
/// [`planner_hosts`], the grant's login when that host has it (else the
/// host's own), then the host's own, then its other signed-in profiles. A
/// login `login_health` holds signed out is left out, the grant's and the
/// host's own as much as the rest (review r8).
pub fn candidates(
    s: &Store,
    m: &MissionRow,
    grant: Option<&GrantRow>,
) -> Result<Vec<Login>, IpcError> {
    let preferred = grant.and_then(|g| g.profile.clone());
    let mut out: Vec<Login> = Vec::new();
    let mut push = |l: Login| {
        if !out.contains(&l) {
            out.push(l);
        }
    };
    for host in planner_hosts(s, m, grant)? {
        let row = s.get_host_row(&host)?;
        let profiles: Vec<&crate::store::HostProfileRow> = row
            .iter()
            .flat_map(|r| r.claude_profiles.iter().flatten())
            .collect();
        let at = |p: Option<&String>| Login {
            host: host.clone(),
            profile: p.cloned(),
        };
        let signed_out = |p: Option<&str>| {
            row.as_ref()
                .is_some_and(|r| crate::service::login_health::is_signed_out(r, p))
        };
        if let Some(p) = preferred
            .as_ref()
            .filter(|p| profiles.iter().any(|x| &x.name == *p))
            .filter(|p| !signed_out(Some(p)))
        {
            push(at(Some(p)));
        }
        if !signed_out(None) {
            push(at(None));
        }
        for p in &profiles {
            if p.account_uuid.is_some() && !signed_out(Some(&p.name)) {
                push(at(Some(&p.name)));
            }
        }
    }
    Ok(out)
}

/// The planner's words on a mission (its runs and waits, newest first),
/// read once for everything [`waiting`], [`open_waits`], [`wake_at`] and
/// [`choose`] tell (review r8: a tick read the page four times).
pub struct Words(Vec<MissionEventRow>);

impl Words {
    pub fn read(s: &Store, mission_id: i64) -> Result<Self, IpcError> {
        Ok(Self(s.mission_events_of_kinds(
            mission_id,
            WORD_KINDS,
            EVENTS_READ,
        )?))
    }

    /// The waits whose reset is still ahead.
    pub fn open_waits(&self, now: i64) -> Vec<LimitWait> {
        self.0
            .iter()
            .filter(|e| e.kind == LIMIT_WAIT_EVENT)
            .filter_map(|e| serde_json::from_value::<LimitWait>(e.payload.clone()?).ok())
            .filter(|w| w.resets_at > now)
            .collect()
    }

    /// Whether the planner's newest word is a wait: its last ask ended at a
    /// limit, and no later login of that ask answered.
    pub fn waiting(&self) -> bool {
        self.0.first().is_some_and(|e| e.kind == LIMIT_WAIT_EVENT)
    }

    /// When the planner was last asked.
    pub fn last_planned_at(&self) -> Option<i64> {
        self.0.iter().find(|e| e.kind == "planned").map(|e| e.at)
    }

    /// The earliest reset of the open waits, when the planner is waiting.
    pub fn wake_at(&self, now: i64) -> Option<i64> {
        if !self.waiting() {
            return None;
        }
        self.open_waits(now).iter().map(|w| w.resets_at).min()
    }
}

/// The waits the mission's log holds whose reset is still ahead.
pub fn open_waits(s: &Store, mission_id: i64, now: i64) -> Result<Vec<LimitWait>, IpcError> {
    Ok(Words::read(s, mission_id)?.open_waits(now))
}

/// Whether the planner's newest word on the mission (of its runs and its
/// waits) is a wait: its last ask ended at a limit.
pub fn waiting(s: &Store, mission_id: i64) -> Result<bool, IpcError> {
    Ok(Words::read(s, mission_id)?.waiting())
}

/// When the loop should look at a waiting mission again: the earliest reset
/// of its open waits. `None` when the planner is not waiting.
pub fn wake_at(s: &Store, mission_id: i64, now: i64) -> Result<Option<i64>, IpcError> {
    Ok(Words::read(s, mission_id)?.wake_at(now))
}

/// The first of [`candidates`] that is neither at `accounts.pause_at`
/// ([`account_limits::over_limit`]) nor in an open wait, or (`Err`) the
/// mission's wait until the earliest of their resets.
pub fn choose(
    s: &Store,
    m: &MissionRow,
    grant: Option<&GrantRow>,
    now: i64,
) -> Result<Result<Login, LimitWait>, IpcError> {
    choose_in(s, m, grant, now, &Words::read(s, m.id)?, &[])
}

/// [`choose`] over `words` already read, passing over `skip` too (the
/// logins this ask found signed out).
pub fn choose_in(
    s: &Store,
    m: &MissionRow,
    grant: Option<&GrantRow>,
    now: i64,
    words: &Words,
    skip: &[Login],
) -> Result<Result<Login, LimitWait>, IpcError> {
    let waits = words.open_waits(now);
    let mut resets: Vec<(i64, bool)> = Vec::new();
    let mut first_why: Option<String> = None;
    for l in candidates(s, m, grant)? {
        if skip.contains(&l) {
            continue;
        }
        if let Some(w) = waits.iter().find(|w| w.login.as_ref() == Some(&l)) {
            resets.push((w.resets_at, w.reset_known));
            first_why.get_or_insert_with(|| w.why.clone());
            continue;
        }
        if let Some(over) = account_limits::over_limit(s, &l.host, l.profile.as_deref(), now)? {
            resets.push(
                limit_resume_at(s, &l.host, l.profile.as_deref(), now)
                    .filter(|t| *t > now)
                    .map_or((now + LIMIT_WAIT_GUESS_SECS, false), |t| (t, true)),
            );
            first_why.get_or_insert_with(|| over.reason());
            continue;
        }
        return Ok(Ok(l));
    }
    let (resets_at, reset_known) = resets
        .into_iter()
        .min_by_key(|r| r.0)
        .unwrap_or((now + LIMIT_WAIT_GUESS_SECS, false));
    Ok(Err(LimitWait {
        login: None,
        resets_at,
        reset_known,
        said: None,
        why: format!(
            "every login the planner may use is at its usage limit ({})",
            first_why.unwrap_or_else(|| "no login".into())
        ),
    }))
}

/// The wait for `login`, whose run answered at its usage limit: until the
/// reset the answer names, else the one its account's reading names, else
/// [`LIMIT_WAIT_GUESS_SECS`]. A reset more than [`LIMIT_WAIT_MAX_SECS`]
/// ahead is no reset.
pub fn answered(s: &Store, login: &Login, limit: &UsageLimit, now: i64) -> LimitWait {
    let (resets_at, reset_known) = limit
        .reset_at
        .filter(|t| *t > now && *t <= now + LIMIT_WAIT_MAX_SECS)
        .or_else(|| {
            limit_resume_at(s, &login.host, login.profile.as_deref(), now).filter(|t| *t > now)
        })
        .map_or((now + LIMIT_WAIT_GUESS_SECS, false), |t| (t, true));
    LimitWait {
        login: Some(login.clone()),
        resets_at,
        reset_known,
        said: limit.said.clone(),
        why: format!(
            "{} is at its usage limit{}",
            login.words(),
            limit
                .said
                .as_deref()
                .map(|w| format!(" ({w})"))
                .unwrap_or_default()
        ),
    }
}

/// Write to the mission's log that `login` answered after this ask's wait
/// on another login, so the planner's newest word is no longer a wait.
pub fn cleared(s: &Store, mission_id: i64, login: &Login) {
    event(
        s,
        mission_id,
        LIMIT_CLEARED_EVENT,
        "planner",
        None,
        serde_json::json!({ "host": login.host, "profile": login.profile }),
    );
}

/// Write `wait` to the mission's log.
pub fn record(s: &Store, mission_id: i64, wait: &LimitWait) {
    event(
        s,
        mission_id,
        LIMIT_WAIT_EVENT,
        "planner",
        None,
        serde_json::to_value(wait).unwrap_or_default(),
    );
}
