//! Limit handling (Orbit Fleet redesign step 4.4): how close a host login's
//! Claude account is to its usage limit, and which other login on the same
//! host has headroom.
//!
//! A session runs under one login on its host: the host's own (`profile:
//! None`) or a credential profile (`~/.claude-profiles/<name>`), each logged
//! in to one account. Starting a session on an account already past
//! `accounts.pause_at` would stall at the limit within the hour, so the New
//! session dialog asks first and offers the login with the most headroom;
//! a "Paused · limit" row's Switch account restarts under that login
//! (`restart_session { profile }`). This module only answers the question;
//! nothing here refuses a start or moves a session.
//!
//! On the desktop, usage comes from the same cache the Hosts view and the
//! Accounts page read (`service::account_usage`). On a hub it comes from what
//! the bus followed, the answers `account_usage` serves
//! ([`served_check_account_headroom`], hub contract 14), so a phone and a
//! paired desktop ask the hub.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::ipc_error::{codes, IpcError};
use crate::service::account_usage::{AccountUsage, AccountUsageSnapshot, UsageCache, Window};
use crate::service::settings;
use crate::store::{HostRow, Store};

/// `accounts.pause_at` when unset: past 90% used, a start asks first.
pub const DEFAULT_PAUSE_AT_PCT: f64 = 90.0;

/// The setting, as a percent used.
pub fn pause_at_pct(s: &Store) -> f64 {
    settings::get_string(s, settings::ACCOUNTS_PAUSE_AT)
        .parse::<f64>()
        .unwrap_or(DEFAULT_PAUSE_AT_PCT)
}

/// One login on a host and how much of its account's tighter window is used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostLogin {
    /// `None` = the host's own login; else the credential profile's name.
    pub profile: Option<String>,
    pub account_uuid: String,
    /// Percent used of the account's tighter window that has not reset yet;
    /// `None` without a reading.
    pub used_pct: Option<f64>,
}

/// The answer for one start or switch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Headroom {
    pub pause_at_pct: f64,
    /// The login asked about; `None` when it is not logged in to a known
    /// account (a fresh profile asks for `/login` in the pane).
    pub chosen: Option<HostLogin>,
    /// Whether the chosen login's account is at or past `pause_at_pct`.
    pub over: bool,
    /// The other login on the host with the most headroom, when the chosen
    /// one is over and one is under the line.
    pub suggestion: Option<HostLogin>,
    /// Every login on the host, the host's own first, for a picker.
    pub logins: Vec<HostLogin>,
}

/// Percent used of the account's tighter window at `now`: the larger of the
/// 5-hour and weekly use, counting only a window that has not reset yet
/// ([`Window::live_at`]).
pub fn used_pct(snap: Option<&AccountUsageSnapshot>, now: i64) -> Option<f64> {
    let snap = snap?;
    used_pct_of(snap.usage.as_ref()?, snap.fetched_at, now)
}

/// [`used_pct`] of one reading, fetched at `fetched_at`.
pub fn used_pct_of(usage: &AccountUsage, fetched_at: Option<i64>, now: i64) -> Option<f64> {
    let live = |w: &Option<Window>, len: i64| {
        w.as_ref()
            .filter(|w| w.utilization.is_finite() && w.live_at(fetched_at, len, now))
            .map(|w| w.utilization.clamp(0.0, 100.0))
    };
    use crate::service::account_usage::{FIVE_HOUR_SECS, WEEK_SECS};
    match (
        live(&usage.five_hour, FIVE_HOUR_SECS),
        live(&usage.seven_day, WEEK_SECS),
    ) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// Every login on `host` that is logged in to an account: its own, then each
/// profile, in the order the host lists them.
pub fn host_logins(host: &HostRow, usage: &[AccountUsageSnapshot], now: i64) -> Vec<HostLogin> {
    logins_with(host, |uuid| {
        used_pct(usage.iter().find(|s| s.account_uuid == uuid), now)
    })
}

fn logins_with(host: &HostRow, used: impl Fn(&str) -> Option<f64>) -> Vec<HostLogin> {
    let own = host.account_uuid.as_deref().map(|uuid| HostLogin {
        profile: None,
        account_uuid: uuid.to_string(),
        used_pct: used(uuid),
    });
    let profiles = host.claude_profiles.iter().flatten().filter_map(|p| {
        p.account_uuid.as_deref().map(|uuid| HostLogin {
            profile: Some(p.name.clone()),
            account_uuid: uuid.to_string(),
            used_pct: used(uuid),
        })
    });
    own.into_iter().chain(profiles).collect()
}

/// Whether starting under `profile` on `host` crosses `pause_at_pct`, and the
/// login with the most headroom to offer instead. A suggestion is on another
/// account (another login on the same account shares its limit) and has a
/// reading under the line; with none, the caller can only ask.
pub fn headroom(
    host: &HostRow,
    profile: Option<&str>,
    usage: &[AccountUsageSnapshot],
    pause_at_pct: f64,
    now: i64,
) -> Headroom {
    let logins = host_logins(host, usage, now);
    let profile = profile.map(str::trim).filter(|p| !p.is_empty());
    let chosen = logins
        .iter()
        .find(|l| l.profile.as_deref() == profile)
        .cloned();
    let over = chosen
        .as_ref()
        .and_then(|c| c.used_pct)
        .is_some_and(|u| u >= pause_at_pct);
    let suggestion = if over {
        let current = chosen.as_ref().map(|c| c.account_uuid.as_str());
        logins
            .iter()
            .filter(|l| Some(l.account_uuid.as_str()) != current)
            .filter_map(|l| l.used_pct.filter(|u| *u < pause_at_pct).map(|u| (u, l)))
            .min_by(|(a, _), (b, _)| a.total_cmp(b))
            .map(|(_, l)| l.clone())
    } else {
        None
    };
    Headroom {
        pause_at_pct,
        chosen,
        over,
        suggestion,
        logins,
    }
}

/// A login automation found at or past `accounts.pause_at` (redesign 8.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverLimit {
    pub host_alias: String,
    pub login: HostLogin,
    pub pause_at_pct: f64,
}

impl OverLimit {
    /// Why a run was skipped, for a run row or a mission event.
    pub fn reason(&self) -> String {
        let who = match &self.login.profile {
            Some(p) => format!("profile {p} on {}", self.host_alias),
            None => format!("{}'s own login", self.host_alias),
        };
        format!(
            "the account of {who} is at {:.0}%, over accounts.pause_at ({:.0}%)",
            self.login.used_pct.unwrap_or_default(),
            self.pause_at_pct
        )
    }
}

/// The account a login bills, for automation that names it ("runs as … on
/// mac", redesign 8.7): from the store's newest usage snapshots, which the
/// usage poll writes wherever it runs (the hub included), so a loop needs no
/// cache.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoginAccount {
    pub host_alias: String,
    #[serde(flatten)]
    pub login: HostLogin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// At or past `accounts.pause_at`: automation leaves it alone.
    pub over: bool,
}

/// [`LoginAccount`] of the login `profile` on `host_alias` (`None` = the
/// host's own). `None` for an unknown host or a login not on a known
/// account.
pub fn login_account(
    s: &Store,
    host_alias: &str,
    profile: Option<&str>,
    now: i64,
) -> Result<Option<LoginAccount>, IpcError> {
    let Some(host) = s.get_host_row(host_alias)? else {
        return Ok(None);
    };
    let snaps = s.latest_usage_snapshots()?;
    let pause_at = pause_at_pct(s);
    let profile = profile.map(str::trim).filter(|p| !p.is_empty());
    let Some(login) = logins_with(&host, |uuid| {
        snaps
            .iter()
            .find(|r| r.account_uuid == uuid)
            .and_then(|r| used_pct_of(&r.usage, Some(r.fetched_at), now))
    })
    .into_iter()
    .find(|l| l.profile.as_deref() == profile) else {
        return Ok(None);
    };
    let email = s
        .list_accounts()?
        .into_iter()
        .find(|a| a.uuid == login.account_uuid)
        .and_then(|a| a.email);
    Ok(Some(LoginAccount {
        host_alias: host_alias.to_string(),
        over: login.used_pct.is_some_and(|u| u >= pause_at),
        login,
        email,
    }))
}

/// Whether automation (a routine, the mission loop) should leave the login
/// `profile` on `host_alias` alone: its account is at or past
/// `accounts.pause_at` ([`login_account`]). `None` when under the line,
/// without a reading, for a login that is not on a known account, or for an
/// unknown host: nothing here refuses what it cannot measure.
pub fn over_limit(
    s: &Store,
    host_alias: &str,
    profile: Option<&str>,
    now: i64,
) -> Result<Option<OverLimit>, IpcError> {
    Ok(login_account(s, host_alias, profile, now)?
        .filter(|a| a.over)
        .map(|a| OverLimit {
            host_alias: a.host_alias,
            login: a.login,
            pause_at_pct: pause_at_pct(s),
        }))
}

/// Step 4.4 on a start that did not come through the desktop's own dialog
/// (an MCP or hub `new_session`): refuse a login at or past
/// `accounts.pause_at` with `E_ACCOUNT_LIMIT`, naming its account and the
/// login on the host with the most headroom, unless the caller says the
/// person chose to start anyway (`over_limit_ok`). From the store's newest
/// usage readings, as [`over_limit`]: a login without a reading, or not on
/// a known account, is never refused.
pub fn refuse_over_limit(
    s: &Store,
    host_alias: &str,
    profile: Option<&str>,
    now: i64,
) -> Result<(), IpcError> {
    let Some(over) = over_limit(s, host_alias, profile, now)? else {
        return Ok(());
    };
    let Some(host) = s.get_host_row(host_alias)? else {
        return Ok(());
    };
    let snaps = s.latest_usage_snapshots()?;
    let pause_at = over.pause_at_pct;
    let suggestion = logins_with(&host, |uuid| {
        snaps
            .iter()
            .find(|r| r.account_uuid == uuid)
            .and_then(|r| used_pct_of(&r.usage, Some(r.fetched_at), now))
    })
    .into_iter()
    .filter(|l| l.account_uuid != over.login.account_uuid)
    .filter_map(|l| l.used_pct.filter(|u| *u < pause_at).map(|u| (u, l)))
    .min_by(|(a, _), (b, _)| a.total_cmp(b))
    .map(|(_, l)| l);
    let email = |uuid: &str| {
        s.list_accounts()
            .ok()
            .and_then(|a| a.into_iter().find(|a| a.uuid == uuid))
            .and_then(|a| a.email)
            .unwrap_or_else(|| uuid.to_string())
    };
    let account = email(&over.login.account_uuid);
    let instead = match &suggestion {
        Some(l) => format!(
            "; {} has headroom ({:.0}% used): start with profile {}",
            email(&l.account_uuid),
            l.used_pct.unwrap_or_default(),
            match &l.profile {
                Some(p) => format!("\"{p}\""),
                None => "unset (the host's own login)".to_string(),
            }
        ),
        None => "; no other login on this host has headroom".to_string(),
    };
    Err(IpcError::new(
        codes::E_ACCOUNT_LIMIT,
        format!(
            "{account}: {}{instead}, or pass over_limit_ok: true once the person chose to start anyway",
            over.reason()
        ),
    )
    .with_details(serde_json::json!({
            "account_uuid": over.login.account_uuid,
            "used_pct": over.login.used_pct,
            "pause_at_pct": pause_at,
            "suggested_profile": suggestion.as_ref().map(|l| l.profile.clone().unwrap_or_default()),
            "suggested_account_uuid": suggestion.as_ref().map(|l| l.account_uuid.clone()),
    })))
}

#[derive(Debug, Clone, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "CheckAccountHeadroomParams")]
pub struct CheckAccountHeadroomArgs {
    /// The host the session runs or will run on.
    pub host_alias: String,
    /// The login to start or switch to: a profile name, or `None` / `""`
    /// for the host's own.
    #[serde(default)]
    pub profile: Option<String>,
}

/// `check_account_headroom`: [`headroom`] for a host in the store, from the
/// usage cache, at the stored `accounts.pause_at`. `E_NOTFOUND` for an
/// unknown host.
pub fn check_account_headroom(
    args: &CheckAccountHeadroomArgs,
    store: &Mutex<Store>,
    cache: &Mutex<UsageCache>,
    now: i64,
) -> Result<Headroom, IpcError> {
    let (host, pause_at, accounts) = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new(codes::E_INTERNAL, "store lock poisoned"))?;
        host_and_line(&s, args)?
    };
    let usage: Vec<AccountUsageSnapshot> = {
        let c = cache.lock().unwrap_or_else(|e| e.into_inner());
        accounts.iter().map(|a| c.snapshot(&a.uuid)).collect()
    };
    Ok(headroom(
        &host,
        args.profile.as_deref(),
        &usage,
        pause_at,
        now,
    ))
}

/// The hub's `check_account_headroom` (contract 14): [`headroom`] from what
/// the hub's bus followed, the same answers its `account_usage` tool serves,
/// so a phone can offer Switch to the login with headroom. An account the bus
/// has no answer for reads as unknown use, as an unfetched one does on the
/// desktop. `E_NOTFOUND` for an unknown host.
pub fn served_check_account_headroom(
    args: &CheckAccountHeadroomArgs,
    store: &Mutex<Store>,
    now: i64,
) -> Result<Headroom, IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new(codes::E_INTERNAL, "store lock poisoned"))?;
    let (host, pause_at, _) = host_and_line(&s, args)?;
    let usage = s.bus_account_usage();
    Ok(headroom(
        &host,
        args.profile.as_deref(),
        &usage,
        pause_at,
        now,
    ))
}

fn host_and_line(
    s: &Store,
    args: &CheckAccountHeadroomArgs,
) -> Result<(HostRow, f64, Vec<crate::store::AccountRow>), IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    let host = s.get_host_row(&args.host_alias)?.ok_or_else(|| {
        IpcError::new(
            codes::E_NOTFOUND,
            format!("no host {} to check the accounts of", args.host_alias),
        )
    })?;
    Ok((host, pause_at_pct(s), s.list_accounts()?))
}

/// Test seam: put `host_alias`'s login `profile` (its own for `None`) on
/// account `uuid`, with a stored reading of `pct` used in both windows.
#[cfg(test)]
pub(crate) fn seed_usage(
    s: &Store,
    host_alias: &str,
    profile: Option<&str>,
    uuid: &str,
    pct: f64,
    now: i64,
) {
    use crate::service::account_usage::AccountUsage;
    use crate::store::{AccountRow, UsageSnapshotRow};
    let _ = s.insert_host(host_alias, None);
    s.upsert_account(&AccountRow {
        uuid: uuid.into(),
        ..Default::default()
    })
    .unwrap();
    match profile {
        None => s.set_host_account(host_alias, Some(uuid)).unwrap(),
        Some(p) => {
            let mut list = s
                .get_host_row(host_alias)
                .unwrap()
                .and_then(|h| h.claude_profiles)
                .unwrap_or_default();
            list.retain(|x| x.name != p);
            list.push(crate::store::HostProfileRow {
                name: p.into(),
                account_uuid: Some(uuid.into()),
                email: None,
            });
            s.set_host_profiles(host_alias, &list).unwrap();
        }
    }
    let w = Some(Window {
        utilization: pct,
        resets_at: Some(now + 3_600),
    });
    s.insert_usage_snapshot(&UsageSnapshotRow {
        account_uuid: uuid.into(),
        fetched_at: now - 60,
        usage: AccountUsage {
            five_hour: w.clone(),
            seven_day: w,
            seven_day_opus: None,
            seven_day_sonnet: None,
        },
        subscription: None,
        source_host: None,
    })
    .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::account_usage::{AccountUsage, UsageOutcomeKind};
    use crate::store::HostProfileRow;

    const NOW: i64 = 1_000;

    /// Step 4.4 server-side: a login past the line is refused with the
    /// account named and the login with headroom offered; under the line,
    /// without a reading, or confirmed by the caller, a start goes ahead.
    #[test]
    fn a_start_past_pause_at_is_refused_and_names_the_login_with_headroom() {
        let s = Store::open_in_memory().unwrap();
        seed_usage(&s, "mac", None, "acct-full", 95.0, NOW);
        seed_usage(&s, "mac", Some("work"), "acct-free", 20.0, NOW);
        let err = refuse_over_limit(&s, "mac", None, NOW).unwrap_err();
        assert_eq!(err.code, codes::E_ACCOUNT_LIMIT);
        assert!(err.message.contains("acct-full"), "{}", err.message);
        assert!(err.message.contains("profile \"work\""), "{}", err.message);
        assert!(err.message.contains("over_limit_ok"), "{}", err.message);
        let details = err.details.clone().unwrap();
        assert_eq!(details["suggested_profile"], "work");
        assert_eq!(details["suggested_account_uuid"], "acct-free");
        // The login with headroom starts, and an unknown login is not refused.
        assert!(refuse_over_limit(&s, "mac", Some("work"), NOW).is_ok());
        assert!(refuse_over_limit(&s, "mac", Some("fresh"), NOW).is_ok());
        assert!(refuse_over_limit(&s, "nowhere", None, NOW).is_ok());
        // Nothing with headroom: still refused, and it says so.
        seed_usage(&s, "mac", Some("work"), "acct-busy", 97.0, NOW);
        let err = refuse_over_limit(&s, "mac", None, NOW).unwrap_err();
        assert!(err.message.contains("no other login"), "{}", err.message);
    }

    fn snap(uuid: &str, five: f64, week: f64) -> AccountUsageSnapshot {
        AccountUsageSnapshot {
            account_uuid: uuid.into(),
            usage: Some(AccountUsage {
                five_hour: Some(Window {
                    utilization: five,
                    resets_at: Some(NOW + 3_600),
                }),
                seven_day: Some(Window {
                    utilization: week,
                    resets_at: Some(NOW + 86_400),
                }),
                seven_day_opus: None,
                seven_day_sonnet: None,
            }),
            subscription: None,
            fetched_at: Some(NOW - 60),
            source_host: None,
            status: UsageOutcomeKind::Ok,
            detail: None,
            next_try_at: 0,
        }
    }

    fn profile(name: &str, uuid: Option<&str>) -> HostProfileRow {
        HostProfileRow {
            name: name.into(),
            account_uuid: uuid.map(Into::into),
            email: None,
        }
    }

    fn mac() -> HostRow {
        let mut h: HostRow = serde_json::from_value(serde_json::json!({
            "alias": "mac", "ssh_alias": null, "reachable": true, "claude_version": null,
            "tmux_version": null, "hidden": false, "last_pinged_at": 1,
            "account_uuid": "own", "provisioned": true, "transport": "ssh",
        }))
        .unwrap();
        h.claude_profiles = Some(vec![
            profile("work", Some("work")),
            profile("spare", Some("spare")),
            profile("fresh", None),
            profile("twin", Some("own")),
        ]);
        h
    }

    /// The plan's check for 4.4: a start on an account past the line is
    /// refused (asked about) and offered the login with the most headroom.
    #[test]
    fn a_start_over_the_line_is_offered_the_login_with_the_most_headroom() {
        let usage = [
            snap("own", 20.0, 95.0),
            snap("work", 70.0, 40.0),
            snap("spare", 10.0, 30.0),
        ];
        let h = headroom(&mac(), None, &usage, 90.0, NOW);
        assert!(h.over);
        assert_eq!(
            h.chosen.as_ref().unwrap().used_pct,
            Some(95.0),
            "the tighter window"
        );
        let s = h.suggestion.unwrap();
        assert_eq!(s.profile.as_deref(), Some("spare"));
        assert_eq!(s.used_pct, Some(30.0));
        assert_eq!(
            h.logins
                .iter()
                .map(|l| l.profile.as_deref())
                .collect::<Vec<_>>(),
            [None, Some("work"), Some("spare"), Some("twin")],
            "the host's own first; a profile with no login is not a choice"
        );
    }

    #[test]
    fn under_the_line_there_is_nothing_to_ask() {
        let usage = [snap("own", 20.0, 89.0), snap("spare", 1.0, 1.0)];
        let h = headroom(&mac(), Some(""), &usage, 90.0, NOW);
        assert!(!h.over);
        assert_eq!(h.suggestion, None);
        let h = headroom(&mac(), Some("work"), &usage, 90.0, NOW);
        assert!(!h.over, "no reading is not over");
    }

    #[test]
    fn a_suggestion_is_on_another_account_with_a_reading_under_the_line() {
        // `twin` is another login on the over account: it shares the limit.
        let usage = [snap("own", 100.0, 50.0), snap("work", 95.0, 10.0)];
        let h = headroom(&mac(), Some("twin"), &usage, 90.0, NOW);
        assert!(h.over);
        assert_eq!(h.suggestion, None, "work is over too; spare has no reading");
    }

    /// Review r05 F7: a window with no reset time is live only while the
    /// reading is younger than the window: an 8-day-old weekly reading (and
    /// a 6-hour-old 5-hour one) says nothing about now.
    #[test]
    fn a_window_with_no_reset_time_lapses_with_its_length() {
        let mut s = snap("own", 95.0, 97.0);
        let u = s.usage.as_mut().unwrap();
        u.five_hour.as_mut().unwrap().resets_at = None;
        u.seven_day.as_mut().unwrap().resets_at = None;
        assert_eq!(used_pct(Some(&s), NOW), Some(97.0), "a fresh reading");
        s.fetched_at = Some(NOW - 6 * 3_600);
        assert_eq!(used_pct(Some(&s), NOW), Some(97.0), "the week still runs");
        s.usage
            .as_mut()
            .unwrap()
            .seven_day
            .as_mut()
            .unwrap()
            .utilization = 10.0;
        assert_eq!(used_pct(Some(&s), NOW), Some(10.0), "the 5 hours lapsed");
        s.fetched_at = Some(NOW - 8 * 86_400);
        assert_eq!(used_pct(Some(&s), NOW), None, "8 days old");
        s.fetched_at = None;
        assert_eq!(used_pct(Some(&s), NOW), None, "no time, no claim");
    }

    #[test]
    fn a_window_that_already_reset_does_not_count() {
        let mut s = snap("own", 100.0, 10.0);
        s.usage
            .as_mut()
            .unwrap()
            .five_hour
            .as_mut()
            .unwrap()
            .resets_at = Some(NOW);
        assert_eq!(used_pct(Some(&s), NOW), Some(10.0));
        assert_eq!(used_pct(None, NOW), None);
    }

    #[test]
    fn the_command_reads_the_setting_and_the_cache() {
        let store = Store::open_in_memory().unwrap();
        settings::set(&store, settings::ACCOUNTS_PAUSE_AT, "50").unwrap();
        let store = Mutex::new(store);
        let cache = Mutex::new(UsageCache::new());
        let err = check_account_headroom(
            &CheckAccountHeadroomArgs {
                host_alias: "nowhere".into(),
                profile: None,
            },
            &store,
            &cache,
            NOW,
        )
        .unwrap_err();
        assert_eq!(err.code, codes::E_NOTFOUND);
        assert_eq!(pause_at_pct(&store.lock().unwrap()), 50.0);
    }

    /// Redesign 8.7: automation reads the stored readings, per login.
    #[test]
    fn over_limit_reads_the_stored_reading_of_the_login_asked_about() {
        let s = Store::open_in_memory().unwrap();
        seed_usage(&s, "mac", None, "own", 95.0, NOW);
        seed_usage(&s, "mac", Some("work"), "work", 40.0, NOW);
        let over = over_limit(&s, "mac", None, NOW).unwrap().unwrap();
        assert_eq!(over.login.used_pct, Some(95.0));
        assert!(over.reason().contains("95%"), "{}", over.reason());
        assert!(over.reason().contains("90%"), "{}", over.reason());
        assert_eq!(over_limit(&s, "mac", Some("work"), NOW).unwrap(), None);
        // A window that reset says nothing; an unknown host or login neither.
        assert_eq!(over_limit(&s, "mac", None, NOW + 7_200).unwrap(), None);
        assert_eq!(over_limit(&s, "nowhere", None, NOW).unwrap(), None);
        assert_eq!(over_limit(&s, "mac", Some("fresh"), NOW).unwrap(), None);
        // The line is the setting.
        settings::set(&s, settings::ACCOUNTS_PAUSE_AT, "99").unwrap();
        assert_eq!(over_limit(&s, "mac", None, NOW).unwrap(), None);
    }

    /// Contract 14: the hub answers from what its bus followed, so a phone
    /// asking about a host whose own login is over the line is offered the
    /// profile with headroom; an account the bus has no answer for is
    /// unknown use, never over.
    #[test]
    fn the_hub_answers_headroom_from_the_usage_its_bus_followed() {
        use crate::events::EventBus;
        let bus = std::sync::Arc::new(crate::events::BroadcastEventBus::new(4));
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        s.insert_host("mac", None).unwrap();
        for uuid in ["acct-own", "acct-work"] {
            s.upsert_account(&crate::store::AccountRow {
                uuid: uuid.into(),
                ..Default::default()
            })
            .unwrap();
        }
        s.set_host_account("mac", Some("acct-own")).unwrap();
        s.set_host_profiles(
            "mac",
            &[HostProfileRow {
                name: "work".into(),
                account_uuid: Some("acct-work".into()),
                email: None,
            }],
        )
        .unwrap();
        let store = Mutex::new(s);
        let args = CheckAccountHeadroomArgs {
            host_alias: "mac".into(),
            profile: None,
        };
        let now = crate::store::now_unix();
        let unknown = served_check_account_headroom(&args, &store, now).unwrap();
        assert!(!unknown.over, "no answer is not over the line");

        let mut over = snap("acct-own", 97.0, 40.0);
        let mut under = snap("acct-work", 10.0, 20.0);
        for w in [&mut over, &mut under] {
            let u = w.usage.as_mut().unwrap();
            u.five_hour.as_mut().unwrap().resets_at = Some(now + 3_600);
            u.seven_day.as_mut().unwrap().resets_at = Some(now + 86_400);
        }
        bus.emit(&crate::events::RowChange::AccountUsageUpdated(over));
        bus.emit(&crate::events::RowChange::AccountUsageUpdated(under));
        let h = served_check_account_headroom(&args, &store, now).unwrap();
        assert!(h.over);
        assert_eq!(h.suggestion.unwrap().profile.as_deref(), Some("work"));
        assert_eq!(h.logins.len(), 2);

        let missing = CheckAccountHeadroomArgs {
            host_alias: "gone".into(),
            profile: None,
        };
        let e = served_check_account_headroom(&missing, &store, now).unwrap_err();
        assert_eq!(e.code, codes::E_NOTFOUND);
    }

    /// Review r05 F4: `LoginAccount` is FLAT on the wire (its `HostLogin`
    /// is flattened in), which `RoutineAccount` in `src/lib/routines.ts`
    /// reads; a nested `login` object left the routine header saying
    /// "its account".
    #[test]
    fn a_login_account_is_flat_on_the_wire() {
        let a = LoginAccount {
            host_alias: "mac".into(),
            login: HostLogin {
                profile: Some("work".into()),
                account_uuid: "u1".into(),
                used_pct: Some(12.0),
            },
            email: Some("me@x.com".into()),
            over: false,
        };
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "host_alias": "mac",
                "profile": "work",
                "account_uuid": "u1",
                "used_pct": 12.0,
                "email": "me@x.com",
                "over": false,
            })
        );
        assert_eq!(serde_json::from_value::<LoginAccount>(v).unwrap(), a);
        let ts = crate::repo_files::read("src/lib/routines.ts");
        let start = ts.find("export interface RoutineAccount {").unwrap();
        let body = &ts[start..start + ts[start..].find("\n}").unwrap()];
        for field in ["host_alias", "profile?", "account_uuid", "email?", "over"] {
            assert!(body.contains(&format!("  {field}:")), "{field} in {body}");
        }
        assert!(!body.contains("  login"), "{body}");
    }
}
