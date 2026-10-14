//! Automatic account switching (docs/accounts.md, *Switch automatically*):
//! a session whose account ran out is resumed under the login on its host
//! with the most headroom, without a person pressing Switch account.
//!
//! **Who it applies to.** Two answers, both needed:
//! * the fleet's, `accounts.auto_switch` (off by default), which the owner
//!   turns on in Settings › Limits;
//! * the session owner's own, `people.auto_switch_account` (migration 187,
//!   on by default), which every person, the owner included, can turn off
//!   for their own sessions on the Accounts page. A session nobody owns
//!   (`unclaimed`) follows the personal owner's answer.
//!
//! **Which sessions.** Exactly the rows the attention model shows as
//! "Paused · limit" (`attention::Reason::AccountLimit`): a live Claude
//! session with a pane, not working, not waiting on a dialog or a form and
//! not stuck, whose account has a 5-hour or weekly window fully used that
//! has not reset. With `accounts.auto_switch_early` on (off by default), the
//! same session moves once its account passes `accounts.pause_at`, so its
//! next turn does not stall at the limit. Never touched: shells, pane-less
//! (`bg` / `external`) sessions, other agents, ghosts and lost rows, the
//! registered controller, sessions on a hidden or unreachable host, and a
//! session switched (or tried) in the last [`RETRY_AFTER_SECS`].
//!
//! **Which login.** [`pick`] over [`candidates`]: another login on the same
//! host, on another account (a second login on the same account shares its
//! limit), with a reading under `accounts.pause_at`, not at a limit and not
//! past its daily spend limit, the least used first. No candidate, no
//! switch: the row stays "Paused · limit" for a person. With two or more,
//! Jev's pick is recorded beside it when `decide.jev.limit_switch` is on
//! (`decide::limit_switch`); the rule's pick is the one used.
//!
//! **What it does.** `restart_session { profile }`, the same call the
//! Switch account button makes: the conversation is resumed under the new
//! login, and the row keeps it. Every attempt leaves one timeline event,
//! [`EVENT_SWITCHED`] or [`EVENT_SWITCH_FAILED`], whose detail names the
//! logins and accounts by profile and uuid only.
//!
//! **Carry on.** A person can also ask for the session to continue
//! (`people.auto_switch_continue`, migration 188, off by default). Then,
//! when the session's last turn was cut off by the limit (its newest turn
//! end is a `stop_failure` of class `rate_limit`, not a `turn_done`), a
//! switch that went through queues [`CONTINUE_PROMPT`] as a deferred
//! prompt, not before [`CONTINUE_AFTER_SECS`] so the resumed pane is up;
//! the deferred-prompt delivery types it once the session is idle. A
//! session that had finished its turn is only resumed.
//!
//! **Bounds.** At most [`MAX_SWITCHES_PER_RUN`] per run, a run at most
//! every [`RUN_EVERY_SECS`], one at a time, detached from the reconcile
//! tick; it stands still while `automation.paused` is on, and only the
//! process that owns the fleet runs it (never a desktop paired to a hub).

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::ipc_error::{lock, IpcError};
use crate::service::account_limits::{self, HostLogin};
use crate::service::settings;
use crate::ssh::SshClient;
use crate::store::{HostRow, SessionRow, Store, UsageSnapshotRow};

/// At most this many switches per run, across all hosts.
pub const MAX_SWITCHES_PER_RUN: usize = 5;
/// A run at most this often.
pub const RUN_EVERY_SECS: u64 = 60;
/// A session switched or tried this recently is left alone: a switch drops
/// the row's account until the next pass reads the new one, and a login
/// that refused once is not hammered.
pub const RETRY_AFTER_SECS: i64 = 30 * 60;
/// What a person who asked to carry on gets typed after a switch.
pub const CONTINUE_PROMPT: &str = "Continue where you left off.";
/// How long after the switch the prompt may go in at the earliest.
pub const CONTINUE_AFTER_SECS: i64 = 30;
/// The timeline event of a switch that went through.
pub const EVENT_SWITCHED: &str = "account_switched";
/// The timeline event of a switch that failed (detail carries the code).
pub const EVENT_SWITCH_FAILED: &str = "account_switch_failed";

/// One switch the pass will make.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Planned {
    pub session_id: i64,
    pub host_alias: String,
    pub tmux_name: String,
    /// The account that ran out.
    pub from_account: String,
    /// The login the session resumes under.
    pub target: HostLogin,
    /// Every login that qualified, `target` among them (what Jev is asked
    /// over).
    pub candidates: Vec<HostLogin>,
    /// Queue [`CONTINUE_PROMPT`] once switched: its owner asked for it and
    /// the limit cut its last turn off.
    pub continue_after: bool,
}

/// Whether the fleet has it on (`accounts.auto_switch`).
pub fn fleet_enabled(s: &Store) -> bool {
    settings::get_bool(s, settings::ACCOUNTS_AUTO_SWITCH)
}

/// PURE: whether `row` is "Paused · limit" on `limited` accounts and a
/// session this pass may switch at all.
pub fn switchable(row: &SessionRow, is_limited: impl Fn(&str) -> bool) -> bool {
    row.kind != "shell"
        && !crate::store::has_no_pane(&row.kind)
        && row.agent == crate::store::AGENT_CLAUDE
        && row.status != "ghost"
        && row.lost_at.is_none()
        && row.claude_status.as_deref() != Some("working")
        // Waiting on a dialog or a form, or stuck: the attention model puts
        // these before "Paused · limit", and a restart would lose the dialog.
        && row.claude_status.as_deref() != Some("blocked")
        && row.pending_form.is_none()
        && row.stuck_kind.is_none()
        && row.account_uuid.as_deref().is_some_and(is_limited)
}

/// PURE: whether the owner of `row` lets it be switched: its owner, or the
/// personal owner for a row nobody owns, has not turned it off.
pub fn owner_allows(
    row: &SessionRow,
    personal_owner: Option<i64>,
    opted_out: &BTreeSet<i64>,
) -> bool {
    match answering_person(row, personal_owner) {
        Some(p) => !opted_out.contains(&p),
        // No owner to ask on a store with no personal owner: nobody said
        // yes, so nothing moves.
        None => false,
    }
}

/// The person whose answers apply to `row`: its owner, or the personal
/// owner for a row nobody owns.
fn answering_person(row: &SessionRow, personal_owner: Option<i64>) -> Option<i64> {
    row.owner_person_id.or(personal_owner)
}

/// PURE: whether a turn-end event says the limit cut the turn off: a
/// `stop_failure` whose class is `rate_limit` (`hooks::stop_failure_class`).
pub fn cut_off_by_limit(kind: &str, detail: Option<&str>) -> bool {
    kind == "stop_failure" && detail.is_some_and(|d| d.starts_with("rate_limit"))
}

/// Whether the session's newest turn end is one the limit cut off.
fn last_turn_cut_off(s: &Store, session_id: i64) -> Result<bool, IpcError> {
    Ok(
        s.newest_session_event_of(session_id, &["turn_done", "stop_failure"])?
            .is_some_and(|e| cut_off_by_limit(&e.kind, e.detail.as_deref())),
    )
}

/// PURE: the logins on `host` a session on `from_account` may move to, the
/// host's own first: another account, with a reading under `pause_at`, not
/// at a limit (`is_limited`) and not past its daily limit (`daily_hit`).
pub fn candidates(
    logins: Vec<HostLogin>,
    from_account: &str,
    pause_at: f64,
    is_limited: impl Fn(&str) -> bool,
    daily_hit: impl Fn(&str) -> bool,
) -> Vec<HostLogin> {
    logins
        .into_iter()
        .filter(|l| l.account_uuid != from_account)
        .filter(|l| l.used_pct.is_some_and(|u| u < pause_at))
        .filter(|l| !is_limited(&l.account_uuid) && !daily_hit(&l.account_uuid))
        .collect()
}

/// PURE: the rule's pick: the least used candidate, the first on a tie (so
/// the host's own login wins one).
pub fn pick(candidates: &[HostLogin]) -> Option<&HostLogin> {
    candidates
        .iter()
        .filter_map(|l| l.used_pct.map(|u| (u, l)))
        .reduce(|best, next| if next.0 < best.0 { next } else { best })
        .map(|(_, l)| l)
}

/// The switches due at `now`, from the store alone (the usage poll writes
/// its readings wherever it runs). Empty while the fleet has it off.
/// `recent` maps a session id to when it was last tried.
pub fn plan(
    s: &Store,
    local_host_enabled: bool,
    recent: &HashMap<i64, i64>,
    now: i64,
) -> Result<Vec<Planned>, IpcError> {
    if !fleet_enabled(s) || !crate::service::decide::owns_the_fleet(s) {
        return Ok(Vec::new());
    }
    let snaps: Vec<UsageSnapshotRow> = s.latest_usage_snapshots()?;
    let used = |uuid: &str| {
        snaps
            .iter()
            .find(|r| r.account_uuid == uuid)
            .and_then(|r| account_limits::used_pct_of(&r.usage, Some(r.fetched_at), now))
    };
    let is_limited = |uuid: &str| used(uuid).is_some_and(|u| u >= 100.0);
    let pause_at = account_limits::pause_at_pct(s);
    // Early (`accounts.auto_switch_early`): an idle session moves once its
    // account passes `accounts.pause_at`, before its next turn stalls.
    let line = if settings::get_bool(s, settings::ACCOUNTS_AUTO_SWITCH_EARLY) {
        pause_at
    } else {
        100.0
    };
    let due = |uuid: &str| used(uuid).is_some_and(|u| u >= line);
    if !snaps.iter().any(|r| due(&r.account_uuid)) {
        return Ok(Vec::new());
    }
    let hosts: Vec<HostRow> =
        crate::service::hosts::active_hosts(s.list_hosts()?, local_host_enabled)
            .into_iter()
            .filter(|h| h.reachable || h.alias == "local")
            .collect();
    let opted_out = s.people_without_auto_switch()?;
    let personal_owner = s.personal_owner_id()?;
    let controller = s.get_controller()?;
    let mut out = Vec::new();
    for row in s.list_all_sessions()? {
        if !switchable(&row, due)
            || !owner_allows(&row, personal_owner, &opted_out)
            || controller
                .as_ref()
                .is_some_and(|(h, t)| *h == row.host_alias && *t == row.tmux_name)
            || recent
                .get(&row.id)
                .is_some_and(|at| now - at < RETRY_AFTER_SECS)
        {
            continue;
        }
        let Some(host) = hosts.iter().find(|h| h.alias == row.host_alias) else {
            continue;
        };
        let from = row.account_uuid.clone().unwrap_or_default();
        let logins = account_limits::host_logins(host, &[], now)
            .into_iter()
            .map(|mut l| {
                l.used_pct = used(&l.account_uuid);
                l
            })
            .collect();
        let cands = candidates(logins, &from, pause_at, is_limited, |uuid| {
            // An unreadable spend is not known to be under its limit.
            account_limits::daily_limit_hit(s, uuid, now).map_or(true, |h| h.is_some())
        });
        let Some(target) = pick(&cands).cloned() else {
            continue;
        };
        let continue_after = match answering_person(&row, personal_owner) {
            Some(p) => s.person_auto_switch_continue(p)? && last_turn_cut_off(s, row.id)?,
            None => false,
        };
        out.push(Planned {
            continue_after,
            session_id: row.id,
            host_alias: row.host_alias.clone(),
            tmux_name: row.tmux_name.clone(),
            from_account: from,
            target,
            candidates: cands,
        });
        if out.len() >= MAX_SWITCHES_PER_RUN {
            break;
        }
    }
    Ok(out)
}

/// The timeline detail of one attempt: logins by profile, accounts by uuid.
pub fn detail(p: &Planned, error_code: Option<&str>, jev: Option<&str>) -> String {
    serde_json::json!({
        "continue": p.continue_after && error_code.is_none(),
        "from_account": p.from_account,
        "to_account": p.target.account_uuid,
        "profile": p.target.profile,
        "used_pct": p.target.used_pct.map(|u| u.round()),
        "jev": jev,
        "error": error_code,
    })
    .to_string()
}

/// The side effect, injected for tests: resume the session under `profile`
/// (`""` = the host's own login).
#[async_trait::async_trait]
pub trait Switcher: Send + Sync {
    async fn switch(
        &self,
        host_alias: &str,
        tmux_name: &str,
        profile: &str,
    ) -> Result<(), IpcError>;
}

/// The real switch: `restart_session { profile }`.
pub struct RestartSwitcher {
    pub store: Arc<Mutex<Store>>,
    pub ssh: Arc<SshClient>,
}

#[async_trait::async_trait]
impl Switcher for RestartSwitcher {
    async fn switch(
        &self,
        host_alias: &str,
        tmux_name: &str,
        profile: &str,
    ) -> Result<(), IpcError> {
        crate::service::sessions::restart_session(
            crate::service::sessions::RestartSessionArgs {
                host_alias: host_alias.to_string(),
                name: tmux_name.to_string(),
                force: false,
                profile: Some(profile.to_string()),
                model: None,
                effort: None,
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map(|_| ())
    }
}

/// What one run did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RunReport {
    pub switched: usize,
    pub failed: usize,
}

/// When each session was last tried, for [`RETRY_AFTER_SECS`].
static RECENT: std::sync::LazyLock<Mutex<HashMap<i64, i64>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// One run: plan, then switch each, recording every attempt. `jev` is asked
/// for its pick (recorded only) when there are two or more candidates.
pub async fn run_with(
    store: &Arc<Mutex<Store>>,
    switcher: &dyn Switcher,
    jev: Option<&crate::service::decide::DecideCtx>,
    recent: &Mutex<HashMap<i64, i64>>,
    local_host_enabled: bool,
    now: i64,
) -> Result<RunReport, IpcError> {
    let planned = {
        let seen = recent.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let s = lock(store)?;
        plan(&s, local_host_enabled, &seen, now)?
    };
    let mut report = RunReport::default();
    for p in planned {
        recent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(p.session_id, now);
        let jev_pick = match jev {
            Some(ctx) => {
                let org = lock(store)?.session_org(p.session_id).ok().flatten();
                crate::service::decide::limit_switch::shadow(
                    ctx,
                    p.session_id,
                    org,
                    &p.candidates,
                    &p.target,
                )
                .await
            }
            None => None,
        };
        let profile = p.target.profile.clone().unwrap_or_default();
        let result = switcher.switch(&p.host_alias, &p.tmux_name, &profile).await;
        let (kind, code) = match &result {
            Ok(()) => {
                report.switched += 1;
                if p.continue_after {
                    queue_continue(store, p.session_id, now);
                }
                (EVENT_SWITCHED, None)
            }
            Err(e) => {
                report.failed += 1;
                tracing::warn!(
                    "[account-switch] {} on {} failed: {e}",
                    p.tmux_name,
                    p.host_alias
                );
                (EVENT_SWITCH_FAILED, Some(e.code.as_str()))
            }
        };
        let d = detail(&p, code, jev_pick.as_deref());
        if let Ok(s) = store.lock() {
            if let Err(e) = s.insert_session_event(p.session_id, kind, Some(&d)) {
                tracing::warn!(
                    "[account-switch] event insert failed for {}: {e}",
                    p.session_id
                );
            }
        }
    }
    // Forget tries the retry window no longer covers.
    recent
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|_, at| now - *at < RETRY_AFTER_SECS);
    Ok(report)
}

/// `auto_switch_account`: read or set the caller's own answer.
#[derive(Debug, Clone, Default, serde::Deserialize, Serialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "AutoSwitchAccountParams")]
pub struct AutoSwitchArgs {
    /// `true` / `false` turns automatic switching on or off for your own
    /// sessions; leave it out to read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// `true` / `false`: after a switch, type "Continue where you left off."
    /// into a session whose turn the limit cut off. Leave it out to keep it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continue_after: Option<bool>,
}

/// What `auto_switch_account` answers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, Serialize)]
pub struct AutoSwitchView {
    /// The caller's own answer (`people.auto_switch_account`).
    pub enabled: bool,
    /// Whether a switched session whose turn the limit cut off is told to
    /// carry on (`people.auto_switch_continue`; off unless turned on).
    pub continue_after: bool,
    /// The fleet's (`accounts.auto_switch`): without it nothing switches,
    /// whatever `enabled` says.
    pub fleet_enabled: bool,
}

/// Read, and with `args.enabled` set, the answer of `person` (the caller,
/// as the MCP layer resolves them; `None` = no person behind the
/// connection, `E_FORBIDDEN`).
pub fn my_auto_switch(
    s: &Store,
    person: Option<i64>,
    args: &AutoSwitchArgs,
) -> Result<AutoSwitchView, IpcError> {
    let Some(person) = person else {
        return Err(IpcError::new(
            crate::ipc_error::codes::E_FORBIDDEN,
            "automatic account switching is a person's own setting; this connection speaks for no person",
        ));
    };
    if let Some(on) = args.enabled {
        s.set_person_auto_switch(person, on)?;
    }
    if let Some(on) = args.continue_after {
        s.set_person_auto_switch_continue(person, on)?;
    }
    Ok(AutoSwitchView {
        enabled: s.person_auto_switch(person)?,
        continue_after: s.person_auto_switch_continue(person)?,
        fleet_enabled: fleet_enabled(s),
    })
}

/// Queue [`CONTINUE_PROMPT`] for a session just switched. Best effort: a
/// failure is logged; the switch stands either way.
fn queue_continue(store: &Mutex<Store>, session_id: i64, now: i64) {
    let timing = crate::store::DeferredTiming {
        not_before: Some(now + CONTINUE_AFTER_SECS),
        // Held, not typed, should the new login be over the line too.
        until_limit_reset: true,
        skip_if_archived: true,
    };
    let queued = lock(store).and_then(|s| {
        s.insert_deferred_prompt_timed(session_id, CONTINUE_PROMPT, timing, now)
            .map_err(IpcError::from)
    });
    if let Err(e) = queued {
        tracing::warn!("[account-switch] queueing continue for {session_id} failed: {e}");
    }
}

static RUNNING: AtomicBool = AtomicBool::new(false);

struct InFlight(&'static AtomicBool);
impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Tick entry point: when the fleet has it on, due, not paused and no run is
/// going, start one in the background and return `true`. One settings read
/// when off.
pub fn maybe_run(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) -> bool {
    static LAST: std::sync::LazyLock<Mutex<Option<std::time::Instant>>> =
        std::sync::LazyLock::new(|| Mutex::new(None));
    let period = std::time::Duration::from_secs(RUN_EVERY_SECS);
    {
        let Ok(s) = store.lock() else {
            return false;
        };
        if !fleet_enabled(&s) {
            return false;
        }
        if !crate::service::loops::gate_in("account_switch", &s, Some(period)) {
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
        if RUNNING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return false;
        }
        *last = Some(std::time::Instant::now());
    }
    let guard = InFlight(&RUNNING);
    let store = Arc::clone(store);
    let ssh = Arc::clone(ssh);
    tokio::spawn(async move {
        let _guard = guard;
        let switcher = RestartSwitcher {
            store: Arc::clone(&store),
            ssh,
        };
        let ctx = crate::service::decide::DecideCtx::jev(Arc::clone(&store));
        let result = run_with(
            &store,
            &switcher,
            Some(&ctx),
            &RECENT,
            crate::service::hub::local_host_enabled(),
            crate::store::now_unix(),
        )
        .await;
        if let Ok(r) = &result {
            if r.switched + r.failed > 0 {
                tracing::info!(
                    "[account-switch] switched={} failed={}",
                    r.switched,
                    r.failed
                );
            }
        }
        crate::service::loops::report("account_switch", result.map(|_| ()), Some(period));
    });
    true
}

#[cfg(test)]
#[path = "account_switch_tests.rs"]
mod tests;
