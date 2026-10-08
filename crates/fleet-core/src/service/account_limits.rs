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
//! Usage comes from the same cache the Hosts view and the Accounts page read
//! (`service::account_usage`), so it is local-only like them: a hub client
//! has no cache to read.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::ipc_error::{codes, IpcError};
use crate::service::account_usage::{AccountUsageSnapshot, UsageCache, Window};
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
/// 5-hour and weekly use, counting only a window that has not reset yet.
pub fn used_pct(snap: Option<&AccountUsageSnapshot>, now: i64) -> Option<f64> {
    let usage = snap?.usage.as_ref()?;
    let live = |w: &Option<Window>| {
        w.as_ref()
            .filter(|w| w.utilization.is_finite() && w.resets_at.is_none_or(|at| at > now))
            .map(|w| w.utilization.clamp(0.0, 100.0))
    };
    match (live(&usage.five_hour), live(&usage.seven_day)) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// Every login on `host` that is logged in to an account: its own, then each
/// profile, in the order the host lists them.
pub fn host_logins(host: &HostRow, usage: &[AccountUsageSnapshot], now: i64) -> Vec<HostLogin> {
    let used = |uuid: &str| used_pct(usage.iter().find(|s| s.account_uuid == uuid), now);
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
            .filter(|l| l.used_pct.is_some_and(|u| u < pause_at_pct))
            .min_by(|a, b| a.used_pct.unwrap().total_cmp(&b.used_pct.unwrap()))
            .cloned()
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

#[derive(Debug, Clone, Deserialize)]
pub struct CheckAccountHeadroomArgs {
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
    crate::validate::host_alias(&args.host_alias)?;
    let (host, pause_at, accounts) = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new(codes::E_INTERNAL, "store lock poisoned"))?;
        let host = s.get_host_row(&args.host_alias)?.ok_or_else(|| {
            IpcError::new(
                codes::E_NOTFOUND,
                format!("no host {} to check the accounts of", args.host_alias),
            )
        })?;
        (host, pause_at_pct(&s), s.list_accounts()?)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::account_usage::{AccountUsage, UsageOutcomeKind};
    use crate::store::HostProfileRow;

    const NOW: i64 = 1_000;

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
}
