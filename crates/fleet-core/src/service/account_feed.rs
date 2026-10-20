//! The account half of the operator feed (Control review F9): the standing
//! conditions `operator_feed` reports once when raised and, for these, once
//! more when they clear.
//!
//! * An account at its limit (a 5-hour or weekly window fully used and not
//!   reset): raised with the window and its reset; cleared ("under its
//!   limit again") when the window resets or a reading says so.
//! * A Claude login signed out (`login_health`): raised with the command
//!   that signs it in; cleared once fleet sees it signed in again.
//!
//! Switches done or failed are session events (`account_switch`), read by
//! the feed's own timeline pass.

use crate::service::account_limits::used_pct_of;
use crate::service::account_usage::{FIVE_HOUR_SECS, WEEK_SECS};
use crate::service::login_health;
use crate::service::operator_feed::Condition;
use crate::store::{HostRow, Store, UsageSnapshotRow};

/// Key prefix of an account at its limit: `a:<uuid>:limit`.
const LIMIT_PREFIX: &str = "a:";
/// Key prefix of a signed-out login: `l:<host>:<profile or ->:out`.
const LOGIN_PREFIX: &str = "l:";

/// PURE: the reset of the window that holds `row`'s account at its limit
/// at `now` (`Some(None)` = at the limit, reset unknown), `None` when under
/// it. With both windows full, the later reset.
pub fn limit_reset(row: &UsageSnapshotRow, now: i64) -> Option<Option<i64>> {
    if used_pct_of(&row.usage, Some(row.fetched_at), now).is_none_or(|u| u < 100.0) {
        return None;
    }
    let full = |w: &Option<crate::service::account_usage::Window>, len: i64| {
        w.as_ref()
            .filter(|w| w.utilization >= 100.0 && w.live_at(Some(row.fetched_at), len, now))
            .map(|w| w.resets_at)
    };
    match (
        full(&row.usage.five_hour, FIVE_HOUR_SECS),
        full(&row.usage.seven_day, WEEK_SECS),
    ) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b).or(Some(None)),
    }
}

/// PURE: "14:05 UTC" for a unix second.
fn clock(at: i64) -> String {
    let secs = at.rem_euclid(86_400);
    format!("{:02}:{:02} UTC", secs / 3600, (secs % 3600) / 60)
}

/// PURE: the condition of an account at its limit.
pub fn limit_condition(uuid: &str, label: &str, reset: Option<i64>) -> Condition {
    let when = match reset {
        Some(at) => format!("; it resets at {}", clock(at)),
        None => String::new(),
    };
    Condition {
        key: format!("{LIMIT_PREFIX}{uuid}:limit"),
        text: format!(
            "account {label} hit its usage limit{when}: its sessions wait, and a switch moves them to a login with headroom"
        ),
    }
}

/// PURE: the condition of a signed-out login.
pub fn login_condition(host: &str, login: &crate::store::SignedOutLogin) -> Condition {
    Condition {
        key: format!(
            "{LOGIN_PREFIX}{host}:{}:out",
            login.profile.as_deref().unwrap_or("-")
        ),
        text: login_health::card_text(host, login),
    }
}

/// PURE: the line for a raised `key` that cleared, `label` naming an
/// account by its uuid. `None` for a key that is not this module's.
pub fn cleared_line(key: &str, label: impl Fn(&str) -> String) -> Option<String> {
    if let Some(rest) = key.strip_prefix(LIMIT_PREFIX) {
        let uuid = rest.strip_suffix(":limit")?;
        return Some(format!("account {} is under its limit again", label(uuid)));
    }
    let rest = key.strip_prefix(LOGIN_PREFIX)?.strip_suffix(":out")?;
    let (host, profile) = rest.rsplit_once(':')?;
    Some(match profile {
        "-" => format!("the Claude login on {host} is signed in again"),
        p => format!("the Claude login profile {p} on {host} is signed in again"),
    })
}

/// The account label a line uses: its email, else its uuid.
fn account_label(s: &Store, uuid: &str) -> String {
    s.list_accounts()
        .ok()
        .and_then(|a| a.into_iter().find(|a| a.uuid == uuid))
        .and_then(|a| a.email)
        .unwrap_or_else(|| uuid.to_string())
}

/// Every account condition now, over the active `hosts`.
pub fn conditions(s: &Store, hosts: &[HostRow], now: i64) -> Vec<Condition> {
    conditions_over(
        s,
        hosts,
        now,
        &s.latest_usage_snapshots().unwrap_or_default(),
    )
}

/// [`conditions`], or the store's error when the usage readings could not
/// be read: a caller that tells a missing condition as cleared (the fleet
/// event watcher) must skip the pass rather than read "no account limited".
pub fn try_conditions(
    s: &Store,
    hosts: &[HostRow],
    now: i64,
) -> Result<Vec<Condition>, rusqlite::Error> {
    Ok(conditions_over(s, hosts, now, &s.latest_usage_snapshots()?))
}

fn conditions_over(
    s: &Store,
    hosts: &[HostRow],
    now: i64,
    snaps: &[UsageSnapshotRow],
) -> Vec<Condition> {
    let mut out = Vec::new();
    let accounts: Vec<String> = crate::service::account_usage_poll::distinct_account_uuids(hosts);
    for uuid in &accounts {
        if let Some(reset) = snaps
            .iter()
            .find(|r| r.account_uuid == *uuid)
            .and_then(|r| limit_reset(r, now))
        {
            out.push(limit_condition(uuid, &account_label(s, uuid), reset));
        }
    }
    for h in hosts {
        for l in &h.signed_out_logins {
            out.push(login_condition(&h.alias, l));
        }
    }
    out
}

/// The lines of this module's conditions that were raised and are gone.
pub fn cleared_lines<'a>(
    s: &Store,
    raised: impl IntoIterator<Item = &'a String>,
    now_raised: &[Condition],
) -> Vec<String> {
    raised
        .into_iter()
        .filter(|k| !now_raised.iter().any(|c| &c.key == *k))
        .filter_map(|k| cleared_line(k, |uuid| account_label(s, uuid)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::account_usage::{AccountUsage, Window};

    fn row(five: f64, week: f64, now: i64) -> UsageSnapshotRow {
        let w = |u: f64, at: i64| {
            Some(Window {
                utilization: u,
                resets_at: Some(at),
            })
        };
        UsageSnapshotRow {
            account_uuid: "A".into(),
            fetched_at: now - 60,
            usage: AccountUsage {
                five_hour: w(five, now + 3_600),
                seven_day: w(week, now + 86_400),
                seven_day_opus: None,
                seven_day_sonnet: None,
            },
            subscription: None,
            source_host: None,
        }
    }

    #[test]
    fn a_full_window_is_a_limit_with_its_reset() {
        let now = 1_000_000;
        assert_eq!(limit_reset(&row(50.0, 20.0, now), now), None);
        assert_eq!(
            limit_reset(&row(100.0, 20.0, now), now),
            Some(Some(now + 3_600))
        );
        assert_eq!(
            limit_reset(&row(100.0, 100.0, now), now),
            Some(Some(now + 86_400)),
            "the later reset frees it"
        );
        assert_eq!(
            limit_reset(&row(100.0, 20.0, now), now + 4_000),
            None,
            "reset"
        );
    }

    #[test]
    fn raised_and_cleared_lines_name_the_account_and_the_login() {
        let c = limit_condition("A", "me@x", Some(13 * 3600 + 5 * 60));
        assert_eq!(c.key, "a:A:limit");
        assert!(c
            .text
            .contains("me@x hit its usage limit; it resets at 13:05 UTC"));
        let l = login_condition(
            "htz",
            &crate::store::SignedOutLogin {
                profile: Some("work".into()),
                since: 0,
                checked_at: 0,
            },
        );
        assert_eq!(l.key, "l:htz:work:out");
        assert!(l
            .text
            .starts_with("Claude login expired on htz (profile work)"));
        let label = |u: &str| format!("<{u}>");
        assert_eq!(
            cleared_line("a:A:limit", label).as_deref(),
            Some("account <A> is under its limit again")
        );
        assert_eq!(
            cleared_line("l:htz:-:out", label).as_deref(),
            Some("the Claude login on htz is signed in again")
        );
        assert_eq!(
            cleared_line("l:htz:work:out", label).as_deref(),
            Some("the Claude login profile work on htz is signed in again")
        );
        assert_eq!(cleared_line("h:htz:down", label), None, "not this module's");
    }
}
