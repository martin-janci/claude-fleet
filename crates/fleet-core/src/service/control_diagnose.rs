//! `diagnose`: what Control checks before it tells the person a cause, a
//! host's state or a button (review 2026-10-17, F14; the rule is
//! `service::control_rules::SECTION`).
//!
//! One read-only answer to three questions, from what the store already
//! holds (nothing here probes a host or reads a log):
//!
//! 1. **Where does Control run, and is it healthy?** `operator_status`,
//!    its host's last probe (reachable, when it last answered, why the last
//!    probe failed), and the login it bills (`account_limits::login_account`:
//!    percent used against `accounts.pause_at`, a daily limit).
//! 2. **Why can't a host be reached?** The same host check for any alias.
//! 3. **What can the person's client do?** Every paired client with the
//!    version its `X-Fleet-Client` header last named (`update_observed`) and
//!    the Control features that kind of client has ([`client_features`]).
//!
//! Each finding says whether it was `checked` (a fresh reading) or is
//! `inferred` (no reading, or an old one), which is how Control words it.
//!
//! **Beside #933.** The self debug agent's `fleet-diagnostics` server
//! (open PR #933, `service/fleet_agents/diagnostics.rs`) reads logs,
//! journals and error reports for a fleet agent's run. `diagnose` stays a
//! thin summary over rows; when #933 lands, `deeper` points at `/debug`,
//! and its `fleet` tool can back [`host_check`] without changing this
//! answer's shape.

use crate::ipc_error::IpcError;
use crate::service::account_limits::{self, LoginAccount};
use crate::service::operator::{self, OperatorStatus};
use crate::store::{HostRow, Store};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// A host reading older than this is reported, but as `inferred`.
pub const STALE_READING_SECS: i64 = 10 * 60;
/// Clients listed, most recently seen first.
pub const CLIENTS_SHOWN: usize = 10;

#[derive(Debug, Clone, Default, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "DiagnoseParams")]
pub struct DiagnoseArgs {
    /// Also explain this host (why it cannot be reached). Control's own host
    /// is always checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

/// One host as its last probe left it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostCheck {
    pub alias: String,
    /// The fleet has a host by this name.
    pub known: bool,
    pub reachable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_pinged_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reachable_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_version: Option<String>,
    /// The state in one line.
    pub why: String,
    /// The line rests on a fresh probe.
    pub checked: bool,
}

/// One paired client and what it can show.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientCheck {
    pub name: String,
    /// desktop, android or ios; `unknown` before it named its build.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<i64>,
    /// The layout its Control has: `thread_room` on the desktop (the only
    /// layout since Classic was removed), `chat` on a phone.
    pub layout: &'static str,
    /// Control features this kind of client has ([`client_features`]).
    pub has: Vec<&'static str>,
    /// Control features it does not have, so Control never sends the
    /// person to them.
    pub lacks: Vec<&'static str>,
}

/// One line Control may repeat, and whether it rests on a fresh reading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub what: String,
    pub checked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnosis {
    pub checked_at: i64,
    pub control: OperatorStatus,
    pub control_host: HostCheck,
    /// The login Control's session bills: its profile, else the host's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_login: Option<LoginAccount>,
    /// The host asked about, when one was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<HostCheck>,
    pub clients: Vec<ClientCheck>,
    pub findings: Vec<Finding>,
    /// Where to look deeper than rows.
    pub deeper: &'static str,
}

/// Every Control feature a client may have, as `has` / `lacks` name them.
pub const FEATURES: [&str; 9] = [
    "confirm_cards",
    "handoff_chips",
    "threads_column",
    "escalation_cards",
    "autonomy_chip",
    "plus_menu",
    "auto_continue_countdown",
    "fleet_events_folded",
    "local_slash_commands",
];

/// PURE: the Control features of a kind of client, as this build of fleet
/// ships them (an older client may have fewer; its `version` says how old).
pub fn client_features(kind: &str) -> (&'static str, Vec<&'static str>, Vec<&'static str>) {
    let has: &[&str] = match kind {
        "desktop" => &FEATURES,
        "android" | "ios" => &["confirm_cards", "handoff_chips"],
        _ => &[],
    };
    let layout = match kind {
        "desktop" => "thread_room",
        "android" | "ios" => "chat",
        _ => "unknown",
    };
    let lacks = FEATURES
        .iter()
        .copied()
        .filter(|f| !has.contains(f))
        .collect();
    (layout, has.to_vec(), lacks)
}

/// PURE: one host as its row says, at `now`.
pub fn host_check(alias: &str, row: Option<&HostRow>, now: i64) -> HostCheck {
    let Some(h) = row else {
        return HostCheck {
            alias: alias.to_string(),
            known: false,
            reachable: false,
            last_pinged_at: None,
            last_reachable_at: None,
            probe_error_code: None,
            probe_error: None,
            claude_version: None,
            agent_version: None,
            why: format!("this fleet has no host named {alias}"),
            checked: true,
        };
    };
    let fresh = h
        .last_pinged_at
        .is_some_and(|t| now - t <= STALE_READING_SECS);
    let ago = |t: i64| {
        let m = (now - t).max(0) / 60;
        if m < 60 {
            format!("{m} min ago")
        } else {
            format!("{} h ago", m / 60)
        }
    };
    let why = match (h.last_pinged_at, h.reachable) {
        (None, _) => format!("{alias} has not been probed yet"),
        (Some(t), true) => format!("{alias} answered its last probe ({})", ago(t)),
        (Some(t), false) => {
            let since = h
                .last_reachable_at
                .map(|r| format!(", last reachable {}", ago(r)))
                .unwrap_or_default();
            let cause = match (&h.last_probe_error_code, &h.last_probe_error) {
                (Some(c), Some(m)) => format!(": {c}: {m}"),
                (Some(c), None) => format!(": {c}"),
                (None, Some(m)) => format!(": {m}"),
                (None, None) => String::new(),
            };
            format!(
                "{alias} did not answer its last probe ({}){since}{cause}",
                ago(t)
            )
        }
    };
    HostCheck {
        alias: alias.to_string(),
        known: true,
        reachable: h.reachable,
        last_pinged_at: h.last_pinged_at,
        last_reachable_at: h.last_reachable_at,
        probe_error_code: h.last_probe_error_code.clone(),
        probe_error: h.last_probe_error.clone(),
        claude_version: h.claude_version.clone(),
        agent_version: h.agent_version.clone(),
        why,
        checked: fresh,
    }
}

/// PURE: the lines Control may say, each marked checked or inferred.
pub fn findings(
    control: &OperatorStatus,
    host: &HostCheck,
    login: Option<&LoginAccount>,
    pause_at_pct: f64,
    clients: &[ClientCheck],
) -> Vec<Finding> {
    let mut out = Vec::new();
    match control.blocked.as_deref() {
        Some(b) => out.push(Finding {
            what: format!("Control cannot work: {b} (on {})", control.host),
            checked: true,
        }),
        None => out.push(Finding {
            what: format!("Control runs on {}", control.host),
            checked: true,
        }),
    }
    out.push(Finding {
        what: host.why.clone(),
        checked: host.checked,
    });
    match login {
        None => out.push(Finding {
            what: "the login Control bills has no usage reading: its limit is unknown".into(),
            checked: false,
        }),
        Some(l) => {
            let who = l
                .email
                .clone()
                .or_else(|| l.login.profile.clone())
                .unwrap_or_else(|| "the host's own login".into());
            let what = match (&l.daily, l.login.used_pct) {
                (Some(d), _) => format!("Control's login ({who}) {}", d.reason()),
                (None, Some(u)) if l.over => format!(
                    "Control's login ({who}) is at {u:.0}% of its limit, past the {pause_at_pct:.0}% line"
                ),
                (None, Some(u)) => format!("Control's login ({who}) is at {u:.0}% of its limit"),
                (None, None) => format!("Control's login ({who}) has no usage reading yet"),
            };
            out.push(Finding {
                checked: l.login.used_pct.is_some() || l.daily.is_some(),
                what,
            });
        }
    }
    if clients.iter().all(|c| c.version.is_none()) {
        out.push(Finding {
            what: "no client has named its version: which buttons the person has is a guess".into(),
            checked: false,
        });
    } else if clients.iter().any(|c| c.kind == "desktop")
        && clients
            .iter()
            .any(|c| c.kind == "android" || c.kind == "ios")
    {
        out.push(Finding {
            what: "the person uses a desktop and a phone: name the client when you name a button"
                .into(),
            checked: true,
        });
    }
    out
}

fn clients(s: &Store, only: Option<i64>) -> Vec<ClientCheck> {
    let observed = s.update_observed_all().unwrap_or_default();
    let mut out: Vec<ClientCheck> = s
        .list_client_tokens(false)
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.name != operator::OPERATOR_CLIENT_NAME)
        .filter(|t| only.is_none_or(|id| id == t.id))
        .map(|t| {
            let o = observed
                .iter()
                .find(|o| o.target == format!("client:{}", t.id));
            let kind = o.map_or("unknown".to_string(), |o| o.component.clone());
            let (layout, has, lacks) = client_features(&kind);
            ClientCheck {
                name: t.name,
                version: o.map(|o| o.version.clone()),
                last_seen_at: t.last_seen_at,
                kind,
                layout,
                has,
                lacks,
            }
        })
        .collect();
    out.sort_by_key(|c| std::cmp::Reverse(c.last_seen_at.unwrap_or(0)));
    out.truncate(CLIENTS_SHOWN);
    out
}

/// The answer. `only_client` limits the client list to the caller's own
/// (a client that is not Control asks about itself only).
pub fn diagnose(
    store: &Mutex<Store>,
    args: &DiagnoseArgs,
    only_client: Option<i64>,
    now: i64,
) -> Result<Diagnosis, IpcError> {
    let control = operator::operator_status(store)?;
    let s = crate::ipc_error::lock(store)?;
    let control_host = host_check(&control.host, s.get_host_row(&control.host)?.as_ref(), now);
    let profile = control
        .session
        .as_ref()
        .and_then(|r| r.claude_profile.clone());
    let control_login = account_limits::login_account(&s, &control.host, profile.as_deref(), now)?;
    let host = match args
        .host
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
    {
        Some(h) if h != control.host => Some(host_check(h, s.get_host_row(h)?.as_ref(), now)),
        _ => None,
    };
    let clients = clients(&s, only_client);
    let pause_at = account_limits::pause_at_pct(&s);
    let mut findings = findings(
        &control,
        &control_host,
        control_login.as_ref(),
        pause_at,
        &clients,
    );
    if let Some(h) = &host {
        findings.push(Finding {
            what: h.why.clone(),
            checked: h.checked,
        });
    }
    Ok(Diagnosis {
        checked_at: now,
        control,
        control_host,
        control_login,
        host,
        clients,
        findings,
        deeper: "`probe_host` for a fresh reading; `/debug` (the self debug agent) for logs, where installed",
    })
}

#[cfg(test)]
#[path = "control_diagnose_tests.rs"]
mod tests;
