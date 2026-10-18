//! Control's own account (Control review F7).
//!
//! Before this Control always ran under its host's default login, and when
//! that account hit its 5-hour limit Control just stalled while
//! `operator_status` still said ready. Now:
//!
//! * **Status.** `operator_status` carries [`OperatorAccount`]: the login
//!   (profile) Control runs under, its account and usage, and a state, with
//!   "Paused · limit" or "signed out" for a person to read.
//! * **Birth.** A Control born on a host keeps the login it had there, and
//!   otherwise takes the best signed-in login with headroom on it
//!   ([`birth_profile`], `service::placement`), not the host's default. A
//!   restart keeps the stored login by itself (`restart_session` with
//!   `profile: None`).
//! * **Switch.** With `control.switch_at_limit` on (the default), a Control
//!   whose account is at its limit, or whose login is signed out, and that
//!   is not mid-turn, is resumed under another signed-in login with
//!   headroom ([`plan_switch`]): on its own host first, by
//!   `restart_session { profile }`, which resumes the same conversation
//!   (login profiles share `projects/`); else on another host, through the
//!   operator move (`operator::move_operator_to`). Either way Control is
//!   told with a `[fleet]` note, and the row's timeline gets
//!   `account_switched` / `account_switch_failed`. A switch is tried at
//!   most every [`RETRY_AFTER_SECS`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::ipc_error::{lock, IpcError};
use crate::service::placement::{Placement, Want};
use crate::service::{account_limits, login_health, settings};
use crate::ssh::SshClient;
use crate::store::{SessionRow, Store};

/// Control's prompt section (wired in `operator::CLAUDE_MD_SECTIONS`).
pub const SECTION: &str = "\
## Your own account

`operator_status` names the login you run under and its usage (`account`).
When your account reaches its limit or your login is signed out, fleet resumes
your conversation under another signed-in login with headroom, on your host if
it can, else on another host, and tells you with a `[fleet]` note: tell the
person in one line, then carry on. While you are at your limit do not retry or
poll; one failed call is enough, then wait for that note.";

/// A switch of Control is tried at most this often.
pub const RETRY_AFTER_SECS: i64 = 30 * 60;
/// A pass at most this often.
pub const RUN_EVERY_SECS: u64 = 60;

/// How Control's account stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountState {
    Ok,
    /// At or past `accounts.pause_at`, not yet at the limit.
    NearLimit,
    /// A window fully used (or a daily spend limit spent): turns fail.
    Limit,
    /// The login is signed out (`login_health`).
    SignedOut,
    /// No reading for the account.
    Unknown,
}

/// Control's account, as `operator_status` reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperatorAccount {
    pub host_alias: String,
    /// The login profile; `null` = the host's own login.
    pub profile: Option<String>,
    pub account_uuid: Option<String>,
    pub email: Option<String>,
    pub used_pct: Option<f64>,
    pub state: AccountState,
    /// "Paused · limit" / "signed out" / "near its limit", for the panel.
    pub label: Option<String>,
}

/// PURE: the state of a login.
pub fn state_of(
    used: Option<f64>,
    signed_out: bool,
    daily_hit: bool,
    pause_at: f64,
) -> AccountState {
    if signed_out {
        return AccountState::SignedOut;
    }
    if daily_hit || used.is_some_and(|u| u >= 100.0) {
        return AccountState::Limit;
    }
    match used {
        Some(u) if u >= pause_at => AccountState::NearLimit,
        Some(_) => AccountState::Ok,
        None => AccountState::Unknown,
    }
}

/// PURE: what the panel says for a state.
pub fn label_of(state: AccountState) -> Option<&'static str> {
    match state {
        AccountState::Limit => Some("Paused · limit"),
        AccountState::SignedOut => Some("signed out"),
        AccountState::NearLimit => Some("near its limit"),
        AccountState::Ok | AccountState::Unknown => None,
    }
}

/// The account Control's session `row` runs under, from the store at `now`.
pub fn account_of(s: &Store, row: &SessionRow, now: i64) -> Option<OperatorAccount> {
    let profile = s.session_launch(row.id).ok().and_then(|(_, _, p)| p);
    let host = s.get_host_row(&row.host_alias).ok().flatten()?;
    let login = account_limits::login_account(s, &row.host_alias, profile.as_deref(), now)
        .ok()
        .flatten();
    let account_uuid = login
        .as_ref()
        .map(|l| l.login.account_uuid.clone())
        .or_else(|| row.account_uuid.clone());
    let used = login.as_ref().and_then(|l| l.login.used_pct);
    let daily = login.as_ref().is_some_and(|l| l.daily.is_some());
    let signed_out = login_health::is_signed_out(&host, profile.as_deref());
    let state = state_of(used, signed_out, daily, account_limits::pause_at_pct(s));
    Some(OperatorAccount {
        host_alias: row.host_alias.clone(),
        profile,
        email: login.as_ref().and_then(|l| l.email.clone()),
        account_uuid,
        used_pct: used,
        state,
        label: label_of(state).map(str::to_string),
    })
}

/// Where a switch takes Control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Switch {
    /// Another login on its own host: a restart under it.
    SameHost { profile: Option<String> },
    /// A login on another host: the operator move.
    OtherHost {
        host_alias: String,
        profile: Option<String>,
    },
}

/// PURE: whether and where to switch Control. Only a Control at its limit
/// or signed out, not mid-turn; `placement` is ranked without its account
/// (at the limit) and leaves signed-out logins out by itself. Its own host
/// first; another host only when `may_home` allows the operator there.
pub fn plan_switch(
    current_host: &str,
    current_profile: Option<&str>,
    state: AccountState,
    working: bool,
    placement: &Placement,
    may_home: impl Fn(&str) -> bool,
) -> Option<Switch> {
    if working || !matches!(state, AccountState::Limit | AccountState::SignedOut) {
        return None;
    }
    let mine = |c: &&crate::service::placement::Candidate| {
        c.host_alias == current_host && c.profile.as_deref() == current_profile
    };
    if let Some(c) = placement
        .candidates
        .iter()
        .filter(|c| !mine(c))
        .find(|c| c.host_alias == current_host)
    {
        return Some(Switch::SameHost {
            profile: c.profile.clone(),
        });
    }
    placement
        .candidates
        .iter()
        .find(|c| c.host_alias != current_host && may_home(&c.host_alias))
        .map(|c| Switch::OtherHost {
            host_alias: c.host_alias.clone(),
            profile: c.profile.clone(),
        })
}

/// PURE: the login a Control born on `host` takes: `previous` (the login
/// it had on that host) while it is a candidate there, else the best
/// candidate on it, else the host's own (`None`).
pub fn pick_birth_profile(
    host: &str,
    previous: Option<Option<&str>>,
    placement: &Placement,
) -> Option<String> {
    let on_host = || placement.candidates.iter().filter(|c| c.host_alias == host);
    if let Some(prev) = previous {
        if on_host().any(|c| c.profile.as_deref() == prev) {
            return prev.map(str::to_string);
        }
    }
    on_host().next().and_then(|c| c.profile.clone())
}

/// The login a Control born on `host` now takes ([`pick_birth_profile`]):
/// the previous operator row's when it was on the same host.
pub fn birth_profile(s: &Store, host: &str, now: i64) -> Option<String> {
    let previous = crate::service::operator::operator_ref(s)
        .filter(|r| r.host_alias == host)
        .and_then(|r| s.get_session(&r.tmux_name, &r.host_alias).ok().flatten())
        .map(|row| s.session_launch(row.id).ok().and_then(|(_, _, p)| p));
    let want = Want {
        hosts: Some(vec![host.to_string()]),
        ..Default::default()
    };
    let placement = crate::service::placement::suggest_with(
        s,
        None,
        &want,
        crate::service::hub::local_host_enabled(),
        now,
    )
    .ok()?;
    pick_birth_profile(host, previous.as_ref().map(|p| p.as_deref()), &placement)
}

/// PURE: the note Control gets after a switch.
pub fn switched_note(
    from: &OperatorAccount,
    to_host: &str,
    to_profile: Option<&str>,
    moved: bool,
) -> String {
    let why = match from.state {
        AccountState::SignedOut => "your login is signed out".to_string(),
        _ => match &from.email {
            Some(e) => format!("your account {e} reached its limit"),
            None => "your account reached its limit".to_string(),
        },
    };
    let login = match to_profile {
        Some(p) => format!("login profile {p}"),
        None => "the host's own login".to_string(),
    };
    if moved {
        format!(
            "[fleet] {why} on {}, and no other login there has headroom, so you were \
             started again on {to_host} under {login}. Your earlier conversation stays on \
             {}. Check what you were following (fleet_status, your missions), tell the \
             person in one line, then carry on.",
            from.host_alias, from.host_alias
        )
    } else {
        format!(
            "[fleet] {why}, so you were resumed on {to_host} under {login}. It is the \
             same conversation: tell the person in one line, then carry on."
        )
    }
}

/// The side effects of a switch, injected for tests.
#[async_trait::async_trait]
pub trait SwitchExec: Send + Sync {
    /// Resume Control under `profile` (`""` = the host's own) on its host.
    async fn restart(&self, host: &str, tmux: &str, profile: &str) -> Result<i64, IpcError>;
    /// Start Control on `host` under `profile`, leaving `from`.
    async fn move_to(
        &self,
        host: &str,
        profile: Option<&str>,
        from: &str,
    ) -> Result<SessionRow, IpcError>;
    /// Queue `body` for the session's next idle moment.
    async fn queue_note(&self, session_id: i64, body: String) -> Result<(), IpcError>;
}

/// Production executor.
pub struct RealSwitchExec {
    pub store: Arc<Mutex<Store>>,
    pub ssh: Arc<SshClient>,
}

#[async_trait::async_trait]
impl SwitchExec for RealSwitchExec {
    async fn restart(&self, host: &str, tmux: &str, profile: &str) -> Result<i64, IpcError> {
        crate::service::sessions::restart_session(
            crate::service::sessions::RestartSessionArgs {
                host_alias: host.to_string(),
                name: tmux.to_string(),
                force: false,
                profile: Some(profile.to_string()),
                model: None,
                effort: None,
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map(|r| r.id)
    }

    async fn move_to(
        &self,
        host: &str,
        profile: Option<&str>,
        from: &str,
    ) -> Result<SessionRow, IpcError> {
        crate::service::operator::move_operator_to(&self.store, &self.ssh, host, profile, from)
            .await
    }

    async fn queue_note(&self, session_id: i64, body: String) -> Result<(), IpcError> {
        crate::service::sessions::queue_prompt(
            crate::service::sessions::QueuePromptArgs {
                session_id,
                prompt: body,
                ..Default::default()
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map(|_| ())
    }
}

/// What a pass found to do, read off the store.
struct Planned {
    row: SessionRow,
    account: OperatorAccount,
    switch: Switch,
}

fn plan(s: &Store, now: i64) -> Option<Planned> {
    if !settings::get_bool(s, settings::CONTROL_SWITCH_AT_LIMIT)
        || !crate::service::decide::owns_the_fleet(s)
        || crate::service::loops::paused(s)
    {
        return None;
    }
    let r = crate::service::operator::operator_ref(s)?;
    let row = s.get_session(&r.tmux_name, &r.host_alias).ok().flatten()?;
    if row.lost_at.is_some() || row.status != "running" {
        return None;
    }
    let account = account_of(s, &row, now)?;
    if !matches!(account.state, AccountState::Limit | AccountState::SignedOut) {
        return None;
    }
    let want = Want {
        not_account: (account.state == AccountState::Limit)
            .then(|| account.account_uuid.clone())
            .flatten(),
        ..Default::default()
    };
    let local = crate::service::hub::local_host_enabled();
    let placement = crate::service::placement::suggest_with(s, None, &want, local, now).ok()?;
    let hosts = s.list_hosts().ok()?;
    let switch = plan_switch(
        &row.host_alias,
        account.profile.as_deref(),
        account.state,
        row.claude_status.as_deref() == Some("working"),
        &placement,
        |h| {
            hosts
                .iter()
                .find(|x| x.alias == h)
                .is_some_and(|x| x.org_id.is_none())
        },
    )?;
    Some(Planned {
        row,
        account,
        switch,
    })
}

/// When the last switch was tried (unix seconds).
static LAST_TRY: Mutex<Option<i64>> = Mutex::new(None);

/// One pass. Returns whether a switch was made.
pub async fn run_with(
    store: &Mutex<Store>,
    exec: &dyn SwitchExec,
    last_try: &Mutex<Option<i64>>,
    now: i64,
) -> Result<bool, IpcError> {
    if last_try
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some_and(|t| now - t < RETRY_AFTER_SECS)
    {
        return Ok(false);
    }
    let Some(p) = plan(&*lock(store)?, now) else {
        return Ok(false);
    };
    *last_try.lock().unwrap_or_else(|e| e.into_inner()) = Some(now);
    let (result, to_host, to_profile, moved) = match &p.switch {
        Switch::SameHost { profile } => (
            exec.restart(
                &p.row.host_alias,
                &p.row.tmux_name,
                profile.as_deref().unwrap_or(""),
            )
            .await,
            p.row.host_alias.clone(),
            profile.clone(),
            false,
        ),
        Switch::OtherHost {
            host_alias,
            profile,
        } => (
            exec.move_to(host_alias, profile.as_deref(), &p.row.host_alias)
                .await
                .map(|r| r.id),
            host_alias.clone(),
            profile.clone(),
            true,
        ),
    };
    let detail = serde_json::json!({
        "operator": true,
        "from_account": p.account.account_uuid,
        "from_profile": p.account.profile,
        "to_host": to_host,
        "profile": to_profile,
        "moved": moved,
        "error": result.as_ref().err().map(|e| e.code.as_str()),
    })
    .to_string();
    let kind = if result.is_ok() {
        crate::service::account_switch::EVENT_SWITCHED
    } else {
        crate::service::account_switch::EVENT_SWITCH_FAILED
    };
    if let Ok(s) = store.lock() {
        let _ = s.insert_session_event(p.row.id, kind, Some(&detail));
    }
    match result {
        Ok(id) => {
            let note = switched_note(&p.account, &to_host, to_profile.as_deref(), moved);
            if let Err(e) = exec.queue_note(id, note).await {
                tracing::warn!(error = %e.message, "[operator-account] note not queued");
            }
            tracing::info!(to = %to_host, moved, "[operator-account] Control switched login");
            Ok(true)
        }
        Err(e) => {
            tracing::warn!(error = %e.message, "[operator-account] switching Control failed");
            Ok(false)
        }
    }
}

static RUNNING: AtomicBool = AtomicBool::new(false);

/// The tick's entry: detached, single-flight, at most every
/// [`RUN_EVERY_SECS`], and one settings read when off. Returns whether a
/// pass started.
pub fn maybe_run(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) -> bool {
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let period = std::time::Duration::from_secs(RUN_EVERY_SECS);
    {
        let Ok(s) = store.lock() else { return false };
        if !settings::get_bool(&s, settings::CONTROL_SWITCH_AT_LIMIT) {
            return false;
        }
        if !crate::service::loops::gate_in("operator_account", &s, Some(period)) {
            return false;
        }
    }
    {
        let Ok(mut last) = LAST.lock() else {
            return false;
        };
        if !crate::service::repair_tick::due(*last, RUN_EVERY_SECS) {
            return false;
        }
        if RUNNING.swap(true, Ordering::AcqRel) {
            return false;
        }
        *last = Some(std::time::Instant::now());
    }
    let store = Arc::clone(store);
    let ssh = Arc::clone(ssh);
    crate::rt::spawn(async move {
        let _flight = crate::rt::ClearOnDrop::of_static(&RUNNING);
        let exec = RealSwitchExec {
            store: Arc::clone(&store),
            ssh,
        };
        let r = run_with(&store, &exec, &LAST_TRY, crate::store::now_unix()).await;
        crate::service::loops::report("operator_account", r.map(|_| ()), Some(period));
    });
    true
}

#[cfg(test)]
#[path = "operator_account_tests.rs"]
mod tests;
