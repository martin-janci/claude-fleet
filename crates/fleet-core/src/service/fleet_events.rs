//! Fleet events: what happened to the fleet rather than to one session,
//! written as rows (`fleet_events`, migration 201) that event routines fire
//! on (`service::routines::FLEET_EVENTS`).
//!
//! **Who writes them.** This module's watcher, once per reconcile tick: it
//! reads the standing conditions — hosts as `fleet_health` judges them
//! (unreachable, low on disk or memory) and accounts as the operator feed
//! does (at a limit, a login signed out) — and writes one row when one
//! starts and, for a host down and an account at its limit, one when it
//! ends. `Store::set_mission_state` writes `mission_done` and
//! `mission_failed` itself.
//!
//! **Never a storm.** A row is written on a change, never per pass: the
//! watcher keeps what it last told ([`WatchState`]), and a condition that
//! flaps is told again only after [`FLAP_SECS`]; a change inside that
//! window waits, and is told once it outlasts it. The first pass on a hub
//! (no state yet) learns what is true and writes nothing, so an upgrade
//! does not fire every routine on every host already down. The routines
//! add their own rate, overlap and budgets on top.
//!
//! The state lives in one setting, [`WATCH_STATE_KEY`]: no migration of
//! its own, and a restart picks up where it stopped.

use crate::service::operator_feed::{host_conditions, Condition};
use crate::store::fleet_events::{
    ACCOUNT_AVAILABLE, ACCOUNT_LIMITED, HOST_LOW, HOST_OFFLINE, HOST_ONLINE, LOGIN_SIGNED_OUT,
};
use crate::store::{HostRow, NewFleetEvent, Store};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// The setting holding [`WatchState`] as JSON. Internal bookkeeping, not a
/// person's setting: no `SPECS` row, never shown.
pub const WATCH_STATE_KEY: &str = "automation.fleet_event_state";
/// A condition told is not told again (either way) for this long.
pub const FLAP_SECS: i64 = 600;
/// A cleared condition's memory is dropped after this long.
const FORGET_SECS: i64 = 24 * 3600;

/// What the watcher last told of one condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Told {
    /// Raised (true) or cleared.
    pub raised: bool,
    pub at: i64,
}

/// The watcher's memory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchState {
    /// Whether a first pass ran: before it, nothing is told.
    #[serde(default)]
    pub seeded: bool,
    #[serde(default)]
    pub told: BTreeMap<String, Told>,
}

impl WatchState {
    fn load(s: &Store) -> Self {
        s.get_setting(WATCH_STATE_KEY)
            .ok()
            .flatten()
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default()
    }

    fn save(&self, s: &Store) {
        let j = serde_json::to_string(self).unwrap_or_default();
        if let Err(e) = s.set_setting(WATCH_STATE_KEY, &j) {
            tracing::debug!(error = %e, "[fleet-events] state not saved");
        }
    }
}

/// One condition as the watcher reads it: the operator feed's key and
/// words, and what it is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watched {
    pub cond: Condition,
    pub host: Option<String>,
    pub org: Option<i64>,
}

/// PURE: the event a condition's key writes when it is raised (`true`) or
/// cleared, and the subject a routine's rate counts. `None`: nothing.
pub fn event_of(key: &str, raised: bool) -> Option<(&'static str, String)> {
    let parts: Vec<&str> = key.split(':').collect();
    match (parts.as_slice(), raised) {
        (["h", host, "down"], true) => Some((HOST_OFFLINE, format!("host:{host}"))),
        (["h", host, "down"], false) => Some((HOST_ONLINE, format!("host:{host}"))),
        (["h", host, "disk" | "mem"], true) => Some((HOST_LOW, format!("host:{host}"))),
        (["a", uuid, "limit"], true) => Some((ACCOUNT_LIMITED, format!("account:{uuid}"))),
        (["a", uuid, "limit"], false) => Some((ACCOUNT_AVAILABLE, format!("account:{uuid}"))),
        (["l", host, profile, "out"], true) => {
            Some((LOGIN_SIGNED_OUT, format!("login:{host}:{profile}")))
        }
        _ => None,
    }
}

/// PURE: whether the subject of a condition's `key` is still watched: its
/// host is among the active `hosts`, its account on one of them. A key
/// whose subject left (a host hidden, removed, or `local` turned off) did
/// not clear; it is forgotten without an event.
pub fn key_watched(key: &str, hosts: &[String], accounts: &[String]) -> bool {
    let parts: Vec<&str> = key.split(':').collect();
    match parts.as_slice() {
        ["h" | "l", host, ..] => hosts.iter().any(|h| h == host),
        ["a", uuid, ..] => accounts.iter().any(|a| a == uuid),
        _ => true,
    }
}

/// PURE: the change one pass tells, and the state after it. `now_raised`
/// is every condition true now; a key not in it is cleared, unless
/// `watching` says its subject is no longer watched: then it is forgotten.
pub fn diff<'a>(
    state: &mut WatchState,
    now_raised: &'a [Watched],
    watching: &dyn Fn(&str) -> bool,
    now: i64,
) -> Vec<(&'static str, String, Option<&'a Watched>, String)> {
    let mut out = Vec::new();
    if !state.seeded {
        state.seeded = true;
        for w in now_raised {
            state.told.insert(
                w.cond.key.clone(),
                Told {
                    raised: true,
                    at: now,
                },
            );
        }
        return out;
    }
    let mut keys: Vec<String> = state.told.keys().cloned().collect();
    keys.extend(now_raised.iter().map(|w| w.cond.key.clone()));
    keys.sort();
    keys.dedup();
    for key in keys {
        let current = now_raised.iter().find(|w| w.cond.key == key);
        if current.is_none() && !watching(&key) {
            state.told.remove(&key);
            continue;
        }
        let want = current.is_some();
        let last = state.told.get(&key).copied();
        let differs = match last {
            Some(t) => t.raised != want,
            None => want,
        };
        if !differs || last.is_some_and(|t| now - t.at < FLAP_SECS) {
            continue;
        }
        state.told.insert(
            key.clone(),
            Told {
                raised: want,
                at: now,
            },
        );
        if let Some((kind, subject)) = event_of(&key, want) {
            let words = match current {
                Some(w) => w.cond.text.clone(),
                None => cleared_words(kind, &subject),
            };
            out.push((kind, subject, current, words));
        }
    }
    state
        .told
        .retain(|_, t| t.raised || now - t.at < FORGET_SECS);
    out
}

/// PURE: the words of a condition that ended.
fn cleared_words(kind: &str, subject: &str) -> String {
    let what = subject.split_once(':').map(|(_, w)| w).unwrap_or(subject);
    match kind {
        HOST_ONLINE => format!("host {what} is reachable again"),
        ACCOUNT_AVAILABLE => format!("account {what} is under its limit again"),
        _ => format!("{} ended", kind.replace('_', " ")),
    }
}

/// The org of an account: the one org of every active host that logs in
/// with it, `None` when they are in several or none.
fn account_org(s: &Store, hosts: &[HostRow], uuid: &str) -> Option<i64> {
    let mut orgs: Vec<Option<i64>> = hosts
        .iter()
        .filter(|h| {
            h.account_uuid.as_deref() == Some(uuid)
                || h.claude_profiles
                    .iter()
                    .flatten()
                    .any(|p| p.account_uuid.as_deref() == Some(uuid))
        })
        .map(|h| s.host_org(&h.alias).ok().flatten())
        .collect();
    orgs.sort();
    orgs.dedup();
    match orgs.as_slice() {
        [one] => *one,
        _ => None,
    }
}

/// What one pass reads: every watched condition now, over these active hosts.
struct Pass {
    raised: Vec<Watched>,
    hosts: Vec<HostRow>,
}

/// Every watched condition now, or the store's error: a pass that could not
/// read a source must not read its conditions as cleared.
fn watched(s: &Store, now: i64) -> Result<Pass, rusqlite::Error> {
    use crate::service::health;
    let local = crate::service::hub::local_host_enabled();
    let hosts = crate::service::hosts::active_hosts(s.list_hosts()?, local);
    let t = health::host_thresholds(s);
    let mut out = Vec::new();
    for h in &hosts {
        let rows = health::hosts_health(std::slice::from_ref(h), &[], &t, &[], "", now);
        let disk = rows
            .first()
            .filter(|r| r.disk_low)
            .and_then(|r| r.disk_home_pct);
        let mem = health::mem_avail_pct(h, now).filter(|_| health::mem_low(h, t.mem_low_pct, now));
        let org = s.host_org(&h.alias).ok().flatten();
        for cond in host_conditions(&h.alias, h.reachable, disk, mem) {
            out.push(Watched {
                cond,
                host: Some(h.alias.clone()),
                org,
            });
        }
    }
    for cond in crate::service::account_feed::try_conditions(s, &hosts, now)? {
        let parts: Vec<&str> = cond.key.split(':').collect();
        let (host, org) = match parts.as_slice() {
            ["a", uuid, "limit"] => (None, account_org(s, &hosts, uuid)),
            ["l", host, ..] => (Some(host.to_string()), s.host_org(host).ok().flatten()),
            _ => (None, None),
        };
        out.push(Watched { cond, host, org });
    }
    Ok(Pass { raised: out, hosts })
}

/// One pass: read the conditions, write a row for each change. Returns how
/// many rows it wrote.
pub fn run_once(s: &Store, now: i64) -> usize {
    let mut state = WatchState::load(s);
    let pass = match watched(s, now) {
        Ok(p) => p,
        Err(e) => {
            // Skip the pass: diffing a partial read would tell every
            // condition it missed as cleared.
            tracing::warn!(error = %e, "[fleet-events] conditions not read; pass skipped");
            return 0;
        }
    };
    let aliases: Vec<String> = pass.hosts.iter().map(|h| h.alias.clone()).collect();
    let accounts = crate::service::account_usage_poll::distinct_account_uuids(&pass.hosts);
    let watching = |key: &str| key_watched(key, &aliases, &accounts);
    let changes = diff(&mut state, &pass.raised, &watching, now);
    let mut wrote = 0;
    for (kind, subject, w, words) in &changes {
        // A cleared condition has no row now: its host (or account) is in
        // its subject, and its org is worked out as when it was raised.
        let host = w
            .and_then(|w| w.host.clone())
            .or_else(|| subject.strip_prefix("host:").map(str::to_string));
        let org = match (w, subject.strip_prefix("account:")) {
            (Some(w), _) => w.org,
            (None, Some(uuid)) => account_org(s, &pass.hosts, uuid),
            (None, None) => host.as_deref().and_then(|h| s.host_org(h).ok().flatten()),
        };
        let e = NewFleetEvent {
            kind,
            subject,
            detail: Some(words),
            host_alias: host.as_deref(),
            org_id: org,
            person_id: None,
        };
        match s.insert_fleet_event_at(&e, now) {
            Ok(_) => wrote += 1,
            Err(e) => tracing::warn!(error = %e.message, "[fleet-events] not written"),
        }
    }
    state.save(s);
    wrote
}

/// The tick's entry: one cheap pass under the store's lock (reads only
/// rows already in the store; no SSH).
pub fn maybe_run(store: &Mutex<Store>) {
    let Ok(s) = store.lock() else {
        return;
    };
    let n = run_once(&s, crate::store::now_unix());
    if n > 0 {
        tracing::info!("[fleet-events] {n} fleet event(s) written");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: fn(&str) -> bool = |_| true;

    fn w(key: &str) -> Watched {
        Watched {
            cond: Condition {
                key: key.into(),
                text: format!("text of {key}"),
            },
            host: None,
            org: None,
        }
    }

    #[test]
    fn the_first_pass_learns_and_tells_nothing() {
        let mut st = WatchState::default();
        assert!(diff(&mut st, &[w("h:mac:down")], &ALL, 100).is_empty());
        assert!(st.seeded);
        // Still down: nothing.
        assert!(diff(&mut st, &[w("h:mac:down")], &ALL, 100 + FLAP_SECS).is_empty());
        // Back up: host_online.
        let got = diff(&mut st, &[], &ALL, 100 + FLAP_SECS);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, HOST_ONLINE);
        assert_eq!(got[0].1, "host:mac");
        assert_eq!(got[0].3, "host mac is reachable again");
    }

    #[test]
    fn a_change_is_told_once_and_a_flap_waits_out_its_window() {
        let mut st = WatchState {
            seeded: true,
            ..Default::default()
        };
        let down = [w("h:nas:down")];
        let got = diff(&mut st, &down, &ALL, 1000);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, HOST_OFFLINE);
        assert_eq!(got[0].3, "text of h:nas:down");
        assert!(diff(&mut st, &[w("h:nas:down")], &ALL, 1020).is_empty());
        // Back within the window: held.
        assert!(diff(&mut st, &[], &ALL, 1040).is_empty());
        // Down again before the window ends: nothing to tell at all.
        assert!(diff(&mut st, &[w("h:nas:down")], &ALL, 1060).is_empty());
        // Up, and still up after the window: told once.
        assert!(diff(&mut st, &[], &ALL, 1100).is_empty());
        let got = diff(&mut st, &[], &ALL, 1000 + FLAP_SECS);
        assert_eq!(
            got.iter().map(|g| g.0).collect::<Vec<_>>(),
            vec![HOST_ONLINE]
        );
        assert!(diff(&mut st, &[], &ALL, 1000 + 2 * FLAP_SECS).is_empty());
    }

    #[test]
    fn keys_map_to_their_events() {
        assert_eq!(
            event_of("h:mac:disk", true),
            Some((HOST_LOW, "host:mac".into()))
        );
        assert_eq!(event_of("h:mac:disk", false), None);
        assert_eq!(
            event_of("a:u-1:limit", true),
            Some((ACCOUNT_LIMITED, "account:u-1".into()))
        );
        assert_eq!(
            event_of("a:u-1:limit", false),
            Some((ACCOUNT_AVAILABLE, "account:u-1".into()))
        );
        assert_eq!(
            event_of("l:mac:-:out", true),
            Some((LOGIN_SIGNED_OUT, "login:mac:-".into()))
        );
        assert_eq!(event_of("m4:question", true), None);
    }

    #[test]
    fn a_pass_writes_rows_for_a_host_that_went_down() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("mac").unwrap();
        let reach = |up: bool| {
            s.conn_ref()
                .execute("UPDATE hosts SET reachable = ?1 WHERE alias = 'mac'", [up])
                .unwrap();
        };
        reach(true);
        // Seed: the host is up.
        assert_eq!(run_once(&s, 1000), 0);
        reach(false);
        assert_eq!(run_once(&s, 1010), 1);
        let rows = s.fleet_events_of_kind_after(0, HOST_OFFLINE, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].subject, "host:mac");
        assert_eq!(rows[0].host_alias.as_deref(), Some("mac"));
        // Still down: nothing more.
        assert_eq!(run_once(&s, 1020 + FLAP_SECS), 0);
        reach(true);
        assert_eq!(run_once(&s, 1030 + FLAP_SECS), 1);
        let rows = s.fleet_events_of_kind_after(0, HOST_ONLINE, 10).unwrap();
        assert_eq!(rows[0].host_alias.as_deref(), Some("mac"));
    }

    #[test]
    fn a_hidden_down_host_is_forgotten_without_a_host_online_row() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("mac").unwrap();
        s.conn_ref()
            .execute("UPDATE hosts SET reachable = 1 WHERE alias = 'mac'", [])
            .unwrap();
        assert_eq!(run_once(&s, 1000), 0);
        s.conn_ref()
            .execute("UPDATE hosts SET reachable = 0 WHERE alias = 'mac'", [])
            .unwrap();
        assert_eq!(run_once(&s, 1010), 1);
        // The person hides the dead box: it did not come back.
        s.set_host_hidden("mac", true).unwrap();
        assert_eq!(run_once(&s, 1020 + FLAP_SECS), 0);
        assert!(s
            .fleet_events_of_kind_after(0, HOST_ONLINE, 10)
            .unwrap()
            .is_empty());
        assert!(!WatchState::load(&s).told.contains_key("h:mac:down"));
    }

    #[test]
    fn a_pass_that_cannot_read_the_store_writes_nothing() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("mac").unwrap();
        s.conn_ref()
            .execute("UPDATE hosts SET reachable = 0 WHERE alias = 'mac'", [])
            .unwrap();
        assert_eq!(run_once(&s, 1000), 0);
        let before = WatchState::load(&s);
        assert!(before.told.contains_key("h:mac:down"));
        s.conn_ref()
            .execute("UPDATE hosts SET reachable = 1 WHERE alias = 'mac'", [])
            .unwrap();
        s.conn_ref()
            .execute_batch("DROP TABLE account_usage_snapshots")
            .unwrap();
        assert_eq!(run_once(&s, 1000 + 2 * FLAP_SECS), 0);
        assert_eq!(WatchState::load(&s), before, "the pass is skipped");
    }

    #[test]
    fn an_account_available_row_carries_the_accounts_org() {
        use crate::service::account_usage::{AccountUsage, Window};
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("mac").unwrap();
        s.conn_ref()
            .execute("UPDATE hosts SET reachable = 1 WHERE alias = 'mac'", [])
            .unwrap();
        s.upsert_account(&crate::store::AccountRow {
            uuid: "acct-1".into(),
            ..Default::default()
        })
        .unwrap();
        s.set_host_account("mac", Some("acct-1")).unwrap();
        let org = s.add_org("acme", None, false).unwrap();
        s.set_host_org("mac", Some(org.id)).unwrap();
        assert_eq!(run_once(&s, 1000), 0);
        let full = |u: f64| {
            Some(Window {
                utilization: u,
                resets_at: Some(1000 + 3600),
            })
        };
        s.insert_usage_snapshot(&crate::store::UsageSnapshotRow {
            account_uuid: "acct-1".into(),
            fetched_at: 1005,
            usage: AccountUsage {
                five_hour: full(100.0),
                seven_day: full(20.0),
                seven_day_opus: None,
                seven_day_sonnet: None,
            },
            subscription: None,
            source_host: None,
        })
        .unwrap();
        assert_eq!(run_once(&s, 1010), 1);
        let limited = s
            .fleet_events_of_kind_after(0, ACCOUNT_LIMITED, 10)
            .unwrap();
        assert_eq!(limited[0].org_id, Some(org.id));
        // The window resets: under its limit again, in the same org.
        assert_eq!(run_once(&s, 1000 + 3600 + FLAP_SECS), 1);
        let available = s
            .fleet_events_of_kind_after(0, ACCOUNT_AVAILABLE, 10)
            .unwrap();
        assert_eq!(available.len(), 1);
        assert_eq!(available[0].subject, "account:acct-1");
        assert_eq!(available[0].org_id, Some(org.id));
    }

    #[test]
    fn a_key_is_watched_while_its_host_or_account_is() {
        let hosts = vec!["mac".to_string()];
        let accounts = vec!["u-1".to_string()];
        assert!(key_watched("h:mac:down", &hosts, &accounts));
        assert!(!key_watched("h:nas:down", &hosts, &accounts));
        assert!(key_watched("a:u-1:limit", &hosts, &accounts));
        assert!(!key_watched("a:u-2:limit", &hosts, &accounts));
        assert!(!key_watched("l:nas:-:out", &hosts, &accounts));
        let mut st = WatchState {
            seeded: true,
            ..Default::default()
        };
        assert_eq!(diff(&mut st, &[w("h:nas:down")], &ALL, 1000).len(), 1);
        let gone = |k: &str| key_watched(k, &hosts, &accounts);
        assert!(diff(&mut st, &[], &gone, 1000 + FLAP_SECS).is_empty());
        assert!(st.told.is_empty());
    }
}
