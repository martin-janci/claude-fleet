//! Login health (Control review F6): which Claude logins on which hosts are
//! signed out, kept on the host row (`hosts.signed_out_logins`, migration
//! 197) so placement leaves them out, Control's chat shows one card per
//! login and the operator feed tells Control once.
//!
//! **Where the facts come from.** Fleet holds no token and cannot ask a
//! login whether it is signed in without using it, so it records what it
//! sees:
//! * the usage poll (`account_usage_poll::fetch_and_emit`): every source it
//!   tried for an account says either "login expired" / "token rejected"
//!   (signed out) or answered (signed in). [`observations_from_usage`]
//!   reads them off the snapshot.
//! * a headless `claude -p` run refused for want of a login
//!   (`claude_print::signed_out_error`, the planner's "Login expired · Run
//!   /login"): [`note_signed_out`] queues it and the next poll pass records
//!   it ([`drain_noted`]).
//!
//! A usage poll stops at the first source that answers, so a signed-out
//! login ranked after a working one is not looked at again by itself:
//! [`recheck_due`] names one such login per pass, at most every
//! [`RECHECK_EVERY_SECS`], and the poll asks that login alone.
//!
//! **What it never does.** Sign anything in: only a person can, with
//! [`login_command`] on that host.

use std::sync::Mutex;

use crate::ipc_error::IpcError;
use crate::service::account_usage::{parse_source, AccountUsageSnapshot, UsageOutcomeKind};
use crate::store::{HostRow, SignedOutLogin, Store};

/// A signed-out login is looked at again at most this often.
pub const RECHECK_EVERY_SECS: i64 = 3600;
/// Signed-out notes from headless runs kept for the next pass, at most.
const NOTED_MAX: usize = 64;

/// One look at one login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub host_alias: String,
    /// `None` = the host's own login.
    pub profile: Option<String>,
    pub signed_out: bool,
}

/// The note text the usage fetch writes for a source whose login is gone
/// (`UsageOutcomeKind::describe` of `LoginExpired` / `TokenRejected`).
const SIGNED_OUT_NOTES: [&str; 2] = ["login expired", "token rejected"];

/// PURE: what one usage snapshot says about its sources' logins. Every
/// source the fetch skipped is noted `"<source>: <why>"` in `detail`; a
/// "login expired" or "token rejected" there is signed out. The source that
/// answered (`Ok`, or rate limited: the token was accepted) is signed in.
pub fn observations_from_usage(snap: &AccountUsageSnapshot) -> Vec<Observation> {
    let mut out = Vec::new();
    let mut push = |label: &str, signed_out: bool| {
        let (alias, profile) = parse_source(label.trim());
        if alias.is_empty() {
            return;
        }
        let o = Observation {
            host_alias: alias.to_string(),
            profile: profile.map(str::to_string),
            signed_out,
        };
        if !out.contains(&o) {
            out.push(o);
        }
    };
    for note in snap.detail.as_deref().unwrap_or("").split("; ") {
        if let Some((label, why)) = note.rsplit_once(": ") {
            if SIGNED_OUT_NOTES.contains(&why.trim()) {
                push(label, true);
            }
        }
    }
    // A poll where every source failed signs nobody in: its notes already
    // name each source whose login is gone.
    if matches!(
        snap.status,
        UsageOutcomeKind::Ok | UsageOutcomeKind::RateLimited
    ) {
        if let Some(src) = snap.source_host.as_deref() {
            push(src, false);
        }
    }
    out
}

/// PURE: the host's signed-out list after `obs` (this host's only) at
/// `now`: a login seen signed out joins (since now) or stays (since kept,
/// `checked_at` now); one seen signed in leaves.
pub fn apply(current: &[SignedOutLogin], obs: &[Observation], now: i64) -> Vec<SignedOutLogin> {
    let mut out: Vec<SignedOutLogin> = current.to_vec();
    for o in obs {
        let at = out.iter().position(|l| l.profile == o.profile);
        match (o.signed_out, at) {
            (true, Some(i)) => out[i].checked_at = now,
            (true, None) => out.push(SignedOutLogin {
                profile: o.profile.clone(),
                since: now,
                checked_at: now,
            }),
            (false, Some(i)) => {
                out.remove(i);
            }
            (false, None) => {}
        }
    }
    out
}

/// PURE: whether the login `profile` (`None` = the host's own) on `host`
/// is known to be signed out.
pub fn is_signed_out(host: &HostRow, profile: Option<&str>) -> bool {
    host.signed_out_logins
        .iter()
        .any(|l| l.profile.as_deref() == profile)
}

/// PURE: what a person types to sign the login in again.
pub fn login_command(host_alias: &str, profile: Option<&str>) -> String {
    match profile {
        Some(p) => {
            format!("ssh {host_alias} then CLAUDE_CONFIG_DIR=~/.claude-profiles/{p} claude /login")
        }
        None => format!("ssh {host_alias} then claude /login"),
    }
}

/// PURE: the one-line card text: "Claude login expired on htz (since Aug 9)
/// · ssh htz then claude /login". `since` is a calendar day (UTC).
pub fn card_text(host_alias: &str, login: &SignedOutLogin) -> String {
    let whose = match &login.profile {
        Some(p) => format!("{host_alias} (profile {p})"),
        None => host_alias.to_string(),
    };
    format!(
        "Claude login expired on {whose} (since {}) · {}",
        day_label(login.since),
        login_command(host_alias, login.profile.as_deref())
    )
}

/// PURE: "Aug 9" for a unix second (UTC).
pub fn day_label(at: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // Civil-from-days (Howard Hinnant), proleptic Gregorian, UTC.
    let z = at.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{} {d}", MONTHS[(m - 1) as usize])
}

/// Write `obs` into the host rows, one write per host whose list changed.
/// Returns the logins that became signed out (for a log line).
pub fn record(s: &Store, obs: &[Observation], now: i64) -> Result<Vec<Observation>, IpcError> {
    let mut hosts: Vec<&str> = obs.iter().map(|o| o.host_alias.as_str()).collect();
    hosts.sort_unstable();
    hosts.dedup();
    let mut went_out = Vec::new();
    for alias in hosts {
        let Some(row) = s.get_host_row(alias)? else {
            continue;
        };
        let mine: Vec<Observation> = obs
            .iter()
            .filter(|o| o.host_alias == alias)
            .cloned()
            .collect();
        let next = apply(&row.signed_out_logins, &mine, now);
        for o in mine.iter().filter(|o| o.signed_out) {
            if !is_signed_out(&row, o.profile.as_deref()) {
                went_out.push(o.clone());
            }
        }
        s.set_host_signed_out_logins(alias, &next)?;
    }
    for o in &went_out {
        tracing::warn!(
            host = %o.host_alias,
            profile = o.profile.as_deref().unwrap_or(""),
            "[login-health] a Claude login is signed out"
        );
    }
    Ok(went_out)
}

#[cfg(not(test))]
static NOTED: Mutex<Vec<Observation>> = Mutex::new(Vec::new());

/// Run `f` over the queue [`note_signed_out`] fills. Process-wide in the
/// app; per thread under test (libtest runs each test on a thread of its
/// own), so a test that builds a signed-out error never leaks a note into
/// another test's poll.
fn with_noted<R>(f: impl FnOnce(&mut Vec<Observation>) -> R) -> R {
    #[cfg(not(test))]
    {
        f(&mut NOTED.lock().unwrap_or_else(|e| e.into_inner()))
    }
    #[cfg(test)]
    {
        thread_local! {
            static NOTED: Mutex<Vec<Observation>> = const { Mutex::new(Vec::new()) };
        }
        NOTED.with(|q| f(&mut q.lock().unwrap_or_else(|e| e.into_inner())))
    }
}

/// A headless run on `host_alias` under `profile` was refused for want of a
/// login: remember it for the next pass ([`drain_noted`]). No store at hand
/// where that is found, so it waits here; bounded, the oldest dropped.
pub fn note_signed_out(host_alias: &str, profile: Option<&str>) {
    with_noted(|q| {
        if q.len() >= NOTED_MAX {
            q.remove(0);
        }
        q.push(Observation {
            host_alias: host_alias.to_string(),
            profile: profile.map(str::to_string),
            signed_out: true,
        });
    });
}

/// Record what [`note_signed_out`] queued.
pub fn drain_noted(s: &Store, now: i64) {
    let obs: Vec<Observation> = with_noted(std::mem::take);
    if obs.is_empty() {
        return;
    }
    if let Err(e) = record(s, &obs, now) {
        tracing::debug!(error = %e.message, "[login-health] noted logins not recorded");
    }
}

/// Record a recheck of the login `profile` on `host_alias`: what it saw,
/// and — when it saw nothing of that login (an SSH timeout, a parse
/// failure, a missing binary) — that it was looked at all, so the login
/// waits [`RECHECK_EVERY_SECS`] like any other instead of being picked
/// again every poll ahead of the account's other signed-out logins.
pub fn record_recheck(
    s: &Store,
    host_alias: &str,
    profile: Option<&str>,
    obs: &[Observation],
    now: i64,
) -> Result<(), IpcError> {
    record(s, obs, now)?;
    let seen = obs
        .iter()
        .any(|o| o.host_alias == host_alias && o.profile.as_deref() == profile);
    if seen {
        return Ok(());
    }
    let Some(row) = s.get_host_row(host_alias)? else {
        return Ok(());
    };
    let mut next = row.signed_out_logins.clone();
    let Some(l) = next.iter_mut().find(|l| l.profile.as_deref() == profile) else {
        return Ok(());
    };
    l.checked_at = now;
    s.set_host_signed_out_logins(host_alias, &next)?;
    Ok(())
}

/// PURE: the signed-out login on `hosts` to ask again now, if any: one on
/// account `account_uuid`, on a reachable host, not looked at for
/// [`RECHECK_EVERY_SECS`]; the longest unchecked first. Returned as a host
/// row narrowed to that one login, so a usage fetch over it asks that
/// login alone, with the login's profile (`None` = the host's own).
pub fn recheck_due(
    hosts: &[HostRow],
    account_uuid: &str,
    now: i64,
) -> Option<(HostRow, Option<String>)> {
    hosts
        .iter()
        .filter(|h| h.reachable && !h.hidden)
        .flat_map(|h| h.signed_out_logins.iter().map(move |l| (h, l)))
        .filter(|(_, l)| now - l.checked_at >= RECHECK_EVERY_SECS)
        .filter(|(h, l)| login_account(h, l.profile.as_deref()) == Some(account_uuid))
        .min_by_key(|(_, l)| l.checked_at)
        .map(|(h, l)| (narrowed(h, l.profile.as_deref()), l.profile.clone()))
}

/// The account the login `profile` on `h` is on.
fn login_account<'a>(h: &'a HostRow, profile: Option<&str>) -> Option<&'a str> {
    match profile {
        None => h.account_uuid.as_deref(),
        Some(p) => h
            .claude_profiles
            .iter()
            .flatten()
            .find(|x| x.name == p)
            .and_then(|x| x.account_uuid.as_deref()),
    }
}

/// `h` with only the login `profile` left on it.
fn narrowed(h: &HostRow, profile: Option<&str>) -> HostRow {
    let mut out = h.clone();
    match profile {
        None => out.claude_profiles = None,
        Some(p) => {
            out.account_uuid = None;
            out.claude_profiles = h
                .claude_profiles
                .as_ref()
                .map(|v| v.iter().filter(|x| x.name == p).cloned().collect());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(
        status: UsageOutcomeKind,
        source: Option<&str>,
        detail: Option<&str>,
    ) -> AccountUsageSnapshot {
        AccountUsageSnapshot {
            status,
            source_host: source.map(str::to_string),
            detail: detail.map(str::to_string),
            ..AccountUsageSnapshot::never_fetched("A")
        }
    }

    fn obs(host: &str, profile: Option<&str>, out: bool) -> Observation {
        Observation {
            host_alias: host.into(),
            profile: profile.map(str::to_string),
            signed_out: out,
        }
    }

    #[test]
    fn a_skipped_source_with_a_gone_login_is_signed_out_and_the_answer_signed_in() {
        let s = snap(
            UsageOutcomeKind::Ok,
            Some("mac"),
            Some("htz: login expired; mini (work): token rejected; pi: no credentials file"),
        );
        assert_eq!(
            observations_from_usage(&s),
            vec![
                obs("htz", None, true),
                obs("mini", Some("work"), true),
                obs("mac", None, false),
            ]
        );
    }

    #[test]
    fn a_failed_poll_signs_nobody_in() {
        let s = snap(
            UsageOutcomeKind::LoginExpired,
            Some("htz"),
            Some("htz: login expired"),
        );
        assert_eq!(observations_from_usage(&s), vec![obs("htz", None, true)]);
        let s = snap(UsageOutcomeKind::Unavailable, Some("mac"), None);
        assert!(observations_from_usage(&s).is_empty());
    }

    #[test]
    fn a_login_keeps_its_first_sighting_and_leaves_when_seen_signed_in() {
        let first = apply(&[], &[obs("h", None, true)], 100);
        assert_eq!(
            first,
            vec![SignedOutLogin {
                profile: None,
                since: 100,
                checked_at: 100
            }]
        );
        let again = apply(&first, &[obs("h", None, true)], 500);
        assert_eq!((again[0].since, again[0].checked_at), (100, 500));
        assert!(apply(&again, &[obs("h", None, false)], 600).is_empty());
        let other = apply(&again, &[obs("h", Some("w"), false)], 600);
        assert_eq!(other, again, "another login's answer leaves this one");
    }

    #[test]
    fn the_card_names_the_host_the_day_and_the_command() {
        // 2026-08-09 12:00 UTC.
        let l = SignedOutLogin {
            profile: None,
            since: 1_786_276_800,
            checked_at: 0,
        };
        assert_eq!(
            card_text("htz", &l),
            "Claude login expired on htz (since Aug 9) · ssh htz then claude /login"
        );
        let p = SignedOutLogin {
            profile: Some("work".into()),
            ..l
        };
        assert!(card_text("htz", &p)
            .ends_with("ssh htz then CLAUDE_CONFIG_DIR=~/.claude-profiles/work claude /login"));
        assert_eq!(day_label(0), "Jan 1");
    }

    #[test]
    fn a_recheck_asks_one_stale_login_alone() {
        let mut h = crate::service::placement::tests_support::host("htz");
        h.account_uuid = Some("A".into());
        h.claude_profiles = Some(vec![crate::store::HostProfileRow {
            name: "work".into(),
            account_uuid: Some("B".into()),
            email: None,
        }]);
        h.signed_out_logins = vec![SignedOutLogin {
            profile: Some("work".into()),
            since: 0,
            checked_at: 0,
        }];
        assert!(recheck_due(std::slice::from_ref(&h), "A", 10_000).is_none());
        let (n, profile) = recheck_due(std::slice::from_ref(&h), "B", 10_000).expect("due");
        assert_eq!(profile.as_deref(), Some("work"));
        assert_eq!(n.account_uuid, None);
        assert_eq!(n.claude_profiles.as_ref().map(Vec::len), Some(1));
        assert!(
            recheck_due(std::slice::from_ref(&h), "B", 100).is_none(),
            "checked recently"
        );
    }

    /// A recheck that saw nothing of its login still counts as a look, so
    /// the next poll asks the account's other signed-out login.
    #[test]
    fn an_inconclusive_recheck_moves_on_to_the_next_login() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("htz", Some("htz")).unwrap();
        record(
            &s,
            &[obs("htz", None, true), obs("htz", Some("work"), true)],
            0,
        )
        .unwrap();
        record(&s, &[obs("htz", Some("work"), true)], 10).unwrap();
        let mut h = s.get_host_row("htz").unwrap().unwrap();
        h.reachable = true;
        h.hidden = false;
        h.account_uuid = Some("A".into());
        h.claude_profiles = Some(vec![crate::store::HostProfileRow {
            name: "work".into(),
            account_uuid: Some("A".into()),
            email: None,
        }]);
        let now = 10_000;
        let (_, first) = recheck_due(std::slice::from_ref(&h), "A", now).expect("due");
        assert_eq!(first, None, "the longest unchecked first");
        // The fetch failed for another reason: no observation at all.
        record_recheck(&s, "htz", first.as_deref(), &[], now).unwrap();
        h.signed_out_logins = s.get_host_row("htz").unwrap().unwrap().signed_out_logins;
        let own = h
            .signed_out_logins
            .iter()
            .find(|l| l.profile.is_none())
            .unwrap();
        assert_eq!((own.since, own.checked_at), (0, now), "still signed out");
        let (_, next) = recheck_due(std::slice::from_ref(&h), "A", now).expect("due");
        assert_eq!(next.as_deref(), Some("work"));
    }

    /// The F6 path end to end: a headless run refused for want of a login
    /// is on its host row after the next pass drains the notes.
    #[test]
    fn a_refused_headless_run_is_recorded_on_its_host_row() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("lh-refused", Some("lh-refused")).unwrap();
        let e = crate::service::claude_print::signed_out_error("lh-refused", Some("work"));
        assert!(!e.message.is_empty());
        drain_noted(&s, 42);
        let row = s.get_host_row("lh-refused").unwrap().unwrap();
        assert!(is_signed_out(&row, Some("work")));
        assert_eq!(row.signed_out_logins[0].since, 42);
        // Drained: a second pass records nothing new.
        s.set_host_signed_out_logins("lh-refused", &[]).unwrap();
        drain_noted(&s, 50);
        assert!(s
            .get_host_row("lh-refused")
            .unwrap()
            .unwrap()
            .signed_out_logins
            .is_empty());
    }

    #[test]
    fn a_recorded_login_reaches_the_host_row() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("htz", Some("htz")).unwrap();
        let went = record(&s, &[obs("htz", None, true)], 50).unwrap();
        assert_eq!(went.len(), 1);
        let row = s.get_host_row("htz").unwrap().unwrap();
        assert!(is_signed_out(&row, None));
        assert!(record(&s, &[obs("htz", None, true)], 60)
            .unwrap()
            .is_empty());
        record(&s, &[obs("htz", None, false)], 70).unwrap();
        assert!(s
            .get_host_row("htz")
            .unwrap()
            .unwrap()
            .signed_out_logins
            .is_empty());
    }
}
