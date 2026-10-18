//! Placement (Control review F5): fleet, not the model, chooses the host and
//! the Claude login a new piece of work runs on.
//!
//! One rule, used by Control (`suggest_placement`, required by its prompt
//! through [`SECTION`]), by a mission's runs inside their grant
//! (`work::orchestrate::start_for`), by a start that names no host
//! (`trackers::tickets::plan_resolved`) and by the over-limit refusal's
//! suggestion (`account_limits::refuse_over_limit`):
//!
//! **Filter.** A host that answers (reachable, not hidden), has `claude`,
//! and is under the recovery thresholds (`auto_restore::host_load`: low
//! memory, memory pressure, CPU saturated); when a project is named, a host
//! that has run it (when any host has). On it, a login that is on a known
//! account, not signed out (`login_health`), under `accounts.pause_at` and
//! not past its daily spend limit.
//!
//! **Rank.** By headroom: the account's unused share first, then the host's
//! idle CPU and free memory. A preferred host (the one the work last ran
//! on) gets a small bonus, so it wins a near tie and loses to a clearly
//! better one. Jev's placement (`decide.jev.host_placement`) is not asked
//! here; it stays its own, off-by-default proposal.
//!
//! [`rank`] is PURE over [`HostFacts`]; [`suggest`] reads the store into
//! them. Nothing here starts, moves or refuses anything.

use serde::{Deserialize, Serialize};

use crate::ipc_error::IpcError;
use crate::service::{account_limits, auto_restore, health, login_health};
use crate::store::{HostRow, Store};

/// Control's prompt section (wired in `operator::CLAUDE_MD_SECTIONS`).
pub const SECTION: &str = "\
## Hosts and logins

Fleet chooses where work runs, not you. Before `new_session`, `start_work` or
a mission grant names a host or a login, call `suggest_placement` (with the
`project_id` when there is one) and take its first candidate: its host and its
`profile` (none = the host's own login). It already leaves out hosts that are
down or loaded, logins that are signed out and accounts at their limit, and
says why for each. Name another candidate only when the person asked for that
host, and say so in a line. When it returns no candidate, tell the person why
(its `excluded` reasons) instead of guessing a host.

A refusal with `E_ACCOUNT_LIMIT` names the login to use instead; start there
rather than passing `over_limit_ok`, which is the person's call. A login fleet
reports as signed out needs the person: tell them the command fleet gives
(`ssh <host>` then `claude /login`) once, and do not start work on it.";

/// Bonus a preferred host's candidates get, in score points (a point is a
/// percent of account headroom).
pub const PREFERRED_BONUS: f64 = 10.0;
/// Headroom assumed for a login with no usage reading: neither the best nor
/// the worst.
pub const UNKNOWN_HEADROOM: f64 = 50.0;
/// Score points a login loses per live session already on its account: the
/// usage reading lags the sessions placement just started, so without it
/// every run a mission starts in one pass lands on the same top login.
pub const SESSION_PENALTY: f64 = 5.0;

/// One login on a host, as placement sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoginFacts {
    /// `None` = the host's own login.
    pub profile: Option<String>,
    pub account_uuid: String,
    pub email: Option<String>,
    /// Percent used of the account's tighter live window.
    pub used_pct: Option<f64>,
    /// Its API-key account spent its daily limit today.
    pub daily_limit_hit: bool,
    /// When fleet first saw it signed out; `None` = signed in.
    pub signed_out_since: Option<i64>,
    /// Live sessions running on its account, on any host.
    #[serde(default)]
    pub live_sessions: u32,
}

/// One host, as placement sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostFacts {
    pub alias: String,
    /// Active and answering.
    pub up: bool,
    /// A probe saw `claude` there.
    pub has_claude: bool,
    /// Why it reads as loaded (`auto_restore::DEFER_*`), `None` when calm.
    pub loaded: Option<String>,
    pub cpu_pct: Option<f64>,
    pub mem_avail_pct: Option<f64>,
    /// Whether it has run the project asked about; `None` when no project
    /// was named or no host has run it yet.
    pub has_repo: Option<bool>,
    pub logins: Vec<LoginFacts>,
}

/// What the work needs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Want {
    /// Only these hosts (a mission's grant); `None` = any.
    pub hosts: Option<Vec<String>>,
    /// Only this login: `Some(None)` = each host's own, `Some(Some(p))` =
    /// profile `p`; `None` = any.
    pub profile: Option<Option<String>>,
    /// Not on this account (the one that ran out).
    pub not_account: Option<String>,
    /// Not on this host.
    pub not_host: Option<String>,
    /// Won on a near tie ([`PREFERRED_BONUS`]).
    pub prefer_host: Option<String>,
}

/// A place the work can run, best first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct Candidate {
    pub host_alias: String,
    /// The login: a profile name, `null` for the host's own.
    pub profile: Option<String>,
    pub account_uuid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_pct: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_pct: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_avail_pct: Option<f64>,
    /// Higher is better.
    pub score: f64,
    /// Why it ranks where it does, in words.
    pub reasons: Vec<String>,
}

/// A host or login left out, and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct Excluded {
    pub host_alias: String,
    /// The login, when it is one login that was left out; `null` with
    /// `host: true` for the whole host.
    pub profile: Option<String>,
    /// The whole host was left out.
    pub host: bool,
    pub why: String,
}

/// The answer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct Placement {
    pub pause_at_pct: f64,
    pub candidates: Vec<Candidate>,
    pub excluded: Vec<Excluded>,
    /// Something the caller should know (no host has run the project yet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Placement {
    /// The best place, if any.
    pub fn best(&self) -> Option<&Candidate> {
        self.candidates.first()
    }
}

/// PURE: why a whole host is left out, or `None`.
fn host_out(h: &HostFacts, want: &Want) -> Option<String> {
    if want.not_host.as_deref() == Some(h.alias.as_str()) {
        return Some("the host the work is leaving".into());
    }
    if !h.up {
        return Some("host is down or hidden".into());
    }
    if !h.has_claude {
        return Some("no claude seen on the host".into());
    }
    if let Some(why) = &h.loaded {
        return Some(format!("host is loaded ({})", why.replace('_', " ")));
    }
    if h.has_repo == Some(false) {
        return Some("the project has never run on this host".into());
    }
    if h.logins.is_empty() {
        return Some("no Claude login known on the host".into());
    }
    None
}

/// PURE: why one login is left out, or `None`.
fn login_out(l: &LoginFacts, want: &Want, pause_at: f64) -> Option<String> {
    if want.not_account.as_deref() == Some(l.account_uuid.as_str()) {
        return Some("the account that ran out".into());
    }
    if let Some(since) = l.signed_out_since {
        return Some(format!(
            "login signed out (since {})",
            login_health::day_label(since)
        ));
    }
    if l.daily_limit_hit {
        return Some("its account spent its daily limit".into());
    }
    if let Some(u) = l.used_pct.filter(|u| *u >= pause_at) {
        return Some(format!(
            "its account is at {u:.0}%, over accounts.pause_at ({pause_at:.0}%)"
        ));
    }
    None
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// PURE: every place the work can run, best first, and what was left out.
pub fn rank(hosts: &[HostFacts], want: &Want, pause_at: f64) -> Placement {
    let mut out = Placement {
        pause_at_pct: pause_at,
        ..Default::default()
    };
    // (score, host order, login order) so a tie keeps the listed order.
    let mut scored: Vec<(f64, usize, usize, Candidate)> = Vec::new();
    for (hi, h) in hosts.iter().enumerate() {
        if want.hosts.as_ref().is_some_and(|l| !l.contains(&h.alias)) {
            continue;
        }
        if let Some(why) = host_out(h, want) {
            out.excluded.push(Excluded {
                host_alias: h.alias.clone(),
                profile: None,
                host: true,
                why,
            });
            continue;
        }
        for (li, l) in h.logins.iter().enumerate() {
            if want.profile.as_ref().is_some_and(|p| *p != l.profile) {
                continue;
            }
            if let Some(why) = login_out(l, want, pause_at) {
                out.excluded.push(Excluded {
                    host_alias: h.alias.clone(),
                    profile: l.profile.clone(),
                    host: false,
                    why,
                });
                continue;
            }
            let mut reasons = Vec::new();
            let headroom = match l.used_pct {
                Some(u) => {
                    reasons.push(format!("account {u:.0}% used"));
                    100.0 - u
                }
                None => {
                    reasons.push("no usage reading for the account".into());
                    UNKNOWN_HEADROOM
                }
            };
            let cpu = h.cpu_pct.unwrap_or(50.0);
            let mem_used = 100.0 - h.mem_avail_pct.unwrap_or(50.0);
            match (h.cpu_pct, h.mem_avail_pct) {
                (Some(c), Some(m)) => reasons.push(format!("CPU {c:.0}%, {m:.0}% memory free")),
                (Some(c), None) => reasons.push(format!("CPU {c:.0}%")),
                (None, Some(m)) => reasons.push(format!("{m:.0}% memory free")),
                (None, None) => reasons.push("no load reading for the host".into()),
            }
            let mut score = headroom - 0.25 * cpu - 0.15 * mem_used;
            if l.live_sessions > 0 {
                score -= SESSION_PENALTY * f64::from(l.live_sessions);
                reasons.push(match l.live_sessions {
                    1 => "1 session already on the account".into(),
                    n => format!("{n} sessions already on the account"),
                });
            }
            if want.prefer_host.as_deref() == Some(h.alias.as_str()) {
                score += PREFERRED_BONUS;
                reasons.push("where this work ran before".into());
            }
            if h.has_repo == Some(true) {
                reasons.push("has run the project".into());
            }
            scored.push((
                round1(score),
                hi,
                li,
                Candidate {
                    host_alias: h.alias.clone(),
                    profile: l.profile.clone(),
                    account_uuid: l.account_uuid.clone(),
                    email: l.email.clone(),
                    used_pct: l.used_pct,
                    cpu_pct: h.cpu_pct,
                    mem_avail_pct: h.mem_avail_pct,
                    score: round1(score),
                    reasons,
                },
            ));
        }
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    out.candidates = scored.into_iter().map(|(_, _, _, c)| c).collect();
    out
}

/// What `suggest_placement` takes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "SuggestPlacementParams")]
pub struct SuggestArgs {
    /// The project the work is in: hosts that have run it rank, the others
    /// are left out (when any host has run it).
    #[serde(default)]
    pub project_id: Option<i64>,
    /// Only these hosts (a mission's grant).
    #[serde(default)]
    pub hosts: Option<Vec<String>>,
    /// Only this login: a profile name, or `""` for each host's own.
    #[serde(default)]
    pub profile: Option<String>,
}

/// The facts of every active host, from the store at `now`. `project_id`
/// fills [`HostFacts::has_repo`]; the second value says no host has run it.
pub fn facts(
    s: &Store,
    project_id: Option<i64>,
    local_enabled: bool,
    now: i64,
) -> Result<(Vec<HostFacts>, bool), IpcError> {
    let hosts = crate::service::hosts::active_hosts(s.list_hosts()?, local_enabled);
    let snaps = s.latest_usage_snapshots()?;
    let accounts = s.list_accounts()?;
    let mut live = s.live_sessions_per_account()?;
    // A run started moments ago may not carry its account yet (a profile
    // start drops the link until reconcile fills it): charge it to the
    // account its login holds, so the next pick of the same pass sees it.
    let unlinked = s.live_unlinked_sessions_per_login()?;
    if !unlinked.is_empty() {
        for h in s.list_hosts()? {
            for l in account_limits::host_logins(&h, &[], now) {
                if let Some(n) = unlinked.get(&(h.alias.clone(), l.profile.clone())) {
                    *live.entry(l.account_uuid).or_default() += n;
                }
            }
        }
    }
    let cfg = auto_restore::AutoRestoreConfig::from_store(s);
    let with_repo: Option<Vec<String>> = match project_id {
        Some(p) => Some(s.hosts_with_project(p)?),
        None => None,
    };
    let unseen = with_repo.as_ref().is_some_and(Vec::is_empty);
    let mut out = Vec::with_capacity(hosts.len());
    for h in &hosts {
        let mut logins = Vec::new();
        for l in account_limits::host_logins(h, &[], now) {
            let used = snaps
                .iter()
                .find(|r| r.account_uuid == l.account_uuid)
                .and_then(|r| account_limits::used_pct_of(&r.usage, Some(r.fetched_at), now));
            let daily =
                account_limits::daily_limit_hit(s, &l.account_uuid, now).is_ok_and(|d| d.is_some());
            logins.push(LoginFacts {
                signed_out_since: h
                    .signed_out_logins
                    .iter()
                    .find(|x| x.profile == l.profile)
                    .map(|x| x.since),
                email: accounts
                    .iter()
                    .find(|a| a.uuid == l.account_uuid)
                    .and_then(|a| a.email.clone()),
                used_pct: used,
                daily_limit_hit: daily,
                live_sessions: live.get(&l.account_uuid).copied().unwrap_or(0),
                profile: l.profile,
                account_uuid: l.account_uuid,
            });
        }
        out.push(host_facts(
            h,
            &cfg,
            with_repo.as_deref(),
            unseen,
            logins,
            now,
        ));
    }
    Ok((out, unseen))
}

/// PURE (given the row): one host's facts.
fn host_facts(
    h: &HostRow,
    cfg: &auto_restore::AutoRestoreConfig,
    with_repo: Option<&[String]>,
    unseen: bool,
    logins: Vec<LoginFacts>,
    now: i64,
) -> HostFacts {
    HostFacts {
        alias: h.alias.clone(),
        up: h.reachable || h.alias == crate::service::projects::LOCAL_HOST,
        // `local` is the machine serving fleet: a desktop or hub that runs
        // sessions there has claude, whatever an old probe stored.
        has_claude: h.claude_version.is_some() || h.alias == crate::service::projects::LOCAL_HOST,
        loaded: auto_restore::host_load(h, cfg, 0, now).map(str::to_string),
        cpu_pct: health::cpu_busy_pct(h, now),
        mem_avail_pct: health::mem_avail_pct(h, now),
        has_repo: match with_repo {
            Some(list) if !unseen => Some(list.contains(&h.alias)),
            _ => None,
        },
        logins,
    }
}

/// [`rank`] over the store's facts for `want`, at `now`.
pub fn suggest_with(
    s: &Store,
    project_id: Option<i64>,
    want: &Want,
    local_enabled: bool,
    now: i64,
) -> Result<Placement, IpcError> {
    let (hosts, unseen) = facts(s, project_id, local_enabled, now)?;
    let mut p = rank(&hosts, want, account_limits::pause_at_pct(s));
    if unseen {
        p.note = Some("no host has run this project yet; any host can take it".into());
    }
    Ok(p)
}

/// `suggest_placement`: [`suggest_with`] for the tool's arguments.
pub fn suggest(s: &Store, args: &SuggestArgs, now: i64) -> Result<Placement, IpcError> {
    if let Some(list) = &args.hosts {
        for h in list {
            crate::validate::host_alias(h)?;
        }
    }
    let profile = match args.profile.as_deref().map(str::trim) {
        None => None,
        Some("") => Some(None),
        Some(p) => {
            crate::validate::claude_profile(p)?;
            Some(Some(p.to_string()))
        }
    };
    let want = Want {
        hosts: args.hosts.clone(),
        profile,
        ..Default::default()
    };
    suggest_with(
        s,
        args.project_id,
        &want,
        crate::service::hub::local_host_enabled(),
        now,
    )
}

/// The best place inside a mission's grant: its hosts, its login when it
/// names one, the project's hosts preferred. `None` when nothing in the
/// grant qualifies (the caller keeps its old choice, and the limit hold
/// says why).
pub fn best_in_grant(
    s: &Store,
    project_id: Option<i64>,
    hosts: &[String],
    profile: Option<&str>,
    now: i64,
) -> Option<Candidate> {
    let want = Want {
        hosts: Some(hosts.to_vec()),
        profile: profile.map(|p| Some(p.to_string())),
        ..Default::default()
    };
    suggest_with(
        s,
        project_id,
        &want,
        crate::service::hub::local_host_enabled(),
        now,
    )
    .ok()?
    .candidates
    .into_iter()
    .next()
}

#[cfg(test)]
pub(crate) mod tests_support {
    use crate::store::HostRow;

    /// A reachable, provisioned host with nothing else known.
    pub(crate) fn host(alias: &str) -> HostRow {
        serde_json::from_value(serde_json::json!({
            "alias": alias,
            "reachable": true,
            "hidden": false,
            "provisioned": true,
            "transport": "ssh",
        }))
        .expect("a minimal host row")
    }
}

#[cfg(test)]
#[path = "placement_tests.rs"]
mod tests;
