use crate::service::orgs::OrgScope;
use crate::service::trackers::sync::{self as tracker_sync, SyncMetrics};
use crate::service::usage;
use crate::store::{HostRow, SessionRow, Store, TrackerRow, UsageTotals};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// Deliberately **not** `#[serde(default)]`, unlike the list-row types the
/// hub client deserialises.
///
/// Those need it because `ok_json_compact` strips null keys recursively, so
/// absent is the normal encoding of `None` on the wire. `Health`'s own
/// fields have no `Option` — nor do its nested `UsageTotals` / `DayUsage`;
/// the one nested type that does, [`TrackersHealth`] (work graph M12.4),
/// carries a per-field default on each `Option` instead. A
/// container default would buy nothing and cost the loudness: `{}`
/// would parse into a perfectly zeroed health panel (`stuck: 0`, `ghosts: 0`,
/// `context_red: 0`, `hosts_total: 0`) rather than failing, so a renamed
/// field, a wrapper object or an older hub would read as a *healthy* fleet.
/// [`tests::a_partial_health_is_rejected_rather_than_zeroed`] pins that.
///
/// `Default` went with it: nothing derived it (`unready_health`, the poisoned-lock
/// case, writes every field out), and leaving it would invite the attribute
/// back.
#[derive(Serialize, Deserialize)]
pub struct Health {
    pub version: String,
    pub db_ready: bool,
    pub schema_version: i64,
    // Fleet roll-up (from cached reconcile state — no network).
    pub hosts_reachable: u32,
    pub hosts_total: u32,
    pub sessions_total: u32,
    /// Session counts keyed by `claude_status`; a null/None status falls into
    /// the "unknown" bucket.
    pub by_status: BTreeMap<String, u32>,
    /// Sessions whose lifecycle `status` is `"ghost"` (the tmux session
    /// vanished from a reachable host). Ghost is a `status` value, never a
    /// `claude_status` one, so this must not be derived from `by_status`.
    pub ghosts: u32,
    /// Sessions whose `context_pct >= context_red_pct`.
    pub context_red: u32,
    /// The percent `context_red` counts from (`health.context_red_pct`), so a
    /// client draws its context chip at the hub's line, not its own. Per-field
    /// default: an older hub omits it (reads `0`; a client then keeps its own).
    #[serde(default)]
    pub context_red_pct: u32,
    /// Sessions with a `stuck_kind` set.
    pub stuck: u32,
    /// Estimated token usage and cost (micro-USD) per host, summed over the
    /// sessions currently in the store. Hosts with nothing counted are
    /// omitted.
    pub usage_by_host: BTreeMap<String, UsageTotals>,
    /// Per-host reverse-tunnel health, when a supervisor is wired in (the
    /// desktop app and the hub; empty otherwise). Populated via
    /// [`Health::set_tunnels`], not by the pure roll-up.
    #[serde(default)]
    pub tunnels: BTreeMap<String, crate::service::tunnel::TunnelHealth>,
    /// How many of `tunnels` are supervised but crash-looping. The one number
    /// worth alerting on: a flapping tunnel means the Control API is not
    /// reachable from that host, however healthy everything else looks.
    #[serde(default)]
    pub tunnels_flapping: u32,
    /// Estimated usage per UTC day (`day` = `YYYY-MM-DD`), all hosts, over
    /// the last `usage::HEALTH_DAYS` days — from the durable daily roll-up,
    /// so killed sessions still count.
    pub usage_by_day: Vec<usage::DayUsage>,
    /// Live hub↔hub links outside `connected` (retrying, refused,
    /// incompatible). Per-field default: an older hub omits it.
    #[serde(default)]
    pub peer_links_down: u32,
    /// The tracker roll-up (work graph M12.4): per tracker, and the
    /// detection backlog. From the sync's in-memory metrics and the store —
    /// never a call to a tracker or a host. A per-host token's covers its
    /// org's trackers and its own host's sessions. Per-field default: an
    /// older hub omits it.
    #[serde(default)]
    pub trackers: TrackersHealth,
    /// This process's uptime and last reconcile pass (perf-logs §5).
    /// Per-field default: an older hub omits it.
    #[serde(default)]
    pub hub: Option<HubHealth>,
    /// [`TUNNELS_MODE_NONE`] on a public hub, [`TUNNELS_MODE_REVERSE`]
    /// otherwise; `None` from an older hub.
    #[serde(default)]
    pub tunnels_mode: Option<String>,
    /// Configured hub↔hub links (revoked excluded), so `peer_links_down: 0`
    /// can be told from "nothing configured".
    #[serde(default)]
    pub peer_links_total: u32,
    /// Per-host health (host identity & health, task 2): disk, versions,
    /// agent and hook liveness, judged against `health.*` settings.
    /// Per-field default: an older hub omits it.
    #[serde(default)]
    pub hosts: Vec<HostHealthRow>,
    /// The fleet's own software updates (update design §9): the channel's
    /// state and the targets that need a person. Filled by `fleet_health`
    /// on a hub (`service::update::health`); `None` elsewhere and from an
    /// older hub. Not sent when `None`: no desktop reads it yet, so it is
    /// not in the desktop's wire contract (`hub_contract.golden.json`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updates: Option<crate::service::update::UpdatesHealth>,
    /// The decision envelope (Jev): its live calls over the last hour and
    /// whether they are failing (`degraded`, test map §7). `None` when it
    /// is off, in a window onto a hub, and for a scoped caller. Per-field
    /// default: an older hub omits it.
    #[serde(default)]
    pub decide: Option<crate::service::decide::DecideHealth>,
    /// Org administration phase C: the orgs at or over a budget
    /// (`service::org_spend::alerts`). Only for a caller that sees every
    /// session — the master, the desktop, a one-person fleet's owner — since
    /// an org's spend sums other people's private sessions; empty (and not
    /// sent) otherwise and from an older hub.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub org_budgets: Vec<crate::service::org_spend::OrgBudgetAlert>,
    /// Every background loop's last run, next run and result
    /// (`service::loops`, redesign 8.1), in this process. Not sent when
    /// empty (an unready store, an older hub); no client reads it yet, so it
    /// is not in the desktop's wire contract.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loops: Vec<crate::service::loops::LoopHealth>,
    /// `automation.paused`: the pausable loops are standing still. Not sent
    /// while false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub automation_paused: bool,
}

/// `fleet_health.hub`: this process's uptime and its last reconcile pass.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubHealth {
    #[serde(default)]
    pub started_at: i64,
    #[serde(default)]
    pub uptime_secs: i64,
    #[serde(default)]
    pub reconcile: crate::service::tick::ReconcileStats,
}

/// `Health::tunnels_mode` on a hub with a public URL: hooks post to it
/// directly and there is nothing to supervise, so an empty `tunnels` map
/// is "not applicable", not "all down".
pub const TUNNELS_MODE_NONE: &str = "none";
/// `Health::tunnels_mode` without a public URL: reverse SSH tunnels carry
/// the hooks and `tunnels` is their supervisor's view.
pub const TUNNELS_MODE_REVERSE: &str = "reverse";

/// Whether this fleet's hooks ride reverse tunnels, from `hub.public_url`.
pub fn tunnels_mode(s: &Store) -> String {
    let public = s
        .get_setting(crate::service::hub::SETTING_PUBLIC_URL)
        .ok()
        .flatten()
        .is_some_and(|u| !u.trim().is_empty());
    if public {
        TUNNELS_MODE_NONE
    } else {
        TUNNELS_MODE_REVERSE
    }
    .to_string()
}

fn hub_health() -> HubHealth {
    let t = crate::service::tick::tick_stats();
    HubHealth {
        started_at: t.started_at(),
        uptime_secs: t.uptime_secs(),
        reconcile: t.reconcile(),
    }
}

/// One host in [`Health::hosts`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct HostHealthRow {
    pub alias: String,
    pub reachable: bool,
    pub transport: String,
    pub claude_version: Option<String>,
    pub claude_version_at: Option<i64>,
    /// From the live registry when connected, else the stored hello.
    pub agent_version: Option<String>,
    /// Used percent of `$HOME`'s filesystem, when sampled.
    pub disk_home_pct: Option<u8>,
    pub disk_low: bool,
    /// More than `health.claude_max_behind` patch releases behind the
    /// fleet's newest FRESH version.
    pub claude_behind: bool,
    /// An agent host whose agent is older than the hub (not merely different).
    pub agent_behind: bool,
    /// Reachable, has a live non-external session, and no hook from its
    /// token within `hooks_silent_secs`.
    pub hooks_silent: bool,
    /// What the last provisioning warned about, when it delivered the content
    /// but degraded part way (migration 092) — the `ag` launcher did not
    /// install, say. `None` is a clean last run. Before this the reason was a
    /// single `tracing::warn!` on the unattended path, so a fleet could carry
    /// a degraded host indefinitely with nothing saying why.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provision_warning: Option<String>,
    /// Credential variables on the host that outrank its `/login`, by name
    /// (`ANTHROPIC_API_KEY`, `CLAUDE_CODE_USE_BEDROCK`, …): sessions there
    /// bill that credential, not the account fleet shows. Empty when none
    /// are set or the host could not tell.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auth_overrides: Vec<String>,
}

/// The `health.*` thresholds [`hosts_health`] judges against.
pub struct HostHealthThresholds {
    pub disk_low_pct: u8,
    pub claude_max_behind: i64,
    pub hooks_silent_secs: i64,
}

/// A version stamp young enough to compare: the same 24 h the desktop's
/// `health.version_max_age_secs` defaults to.
const VERSION_FRESH_SECS: i64 = 86_400;

/// A health sample (`HostRow::health_at`) young enough to judge `disk_low`
/// on. The sample is rewritten on every reconcile pass (~20 s) of a
/// reachable host, so one an hour old means the host stopped answering and
/// its disk reading says nothing about now. The desktop's health line uses
/// the same value (`HEALTH_SAMPLE_FRESH_SECS` in `src/lib/hosts_view.ts`);
/// keep the two equal.
pub const HEALTH_SAMPLE_FRESH_SECS: i64 = 3600;

/// `2.1.282` → `[2, 1, 282]`; `None` when not a dotted number.
fn version_parts(v: &str) -> Option<Vec<i64>> {
    let first = v.split_whitespace().next()?;
    first.split('.').map(|p| p.parse::<i64>().ok()).collect()
}

/// Patch releases `older` is behind `newest`, when both share major.minor;
/// a different major.minor is "very far" behind (or not behind at all).
fn patch_behind(older: &str, newest: &str) -> Option<i64> {
    let (o, n) = (version_parts(older)?, version_parts(newest)?);
    if o.len() < 3 || n.len() < 3 || o[0] != n[0] || o[1] != n[1] {
        return Some(if o < n { i64::MAX } else { 0 });
    }
    Some((n[2] - o[2]).max(0))
}

/// Pure: the per-host roll-up. `agents` are `(alias, agent_version)` for
/// the agents connected right now; `hub_version` is this build's.
pub fn hosts_health(
    hosts: &[HostRow],
    sessions: &[SessionRow],
    t: &HostHealthThresholds,
    agents: &[(String, String)],
    hub_version: &str,
    now: i64,
) -> Vec<HostHealthRow> {
    let fresh = |h: &HostRow| {
        h.claude_version_at
            .is_some_and(|at| now - at <= VERSION_FRESH_SECS)
    };
    let newest = hosts
        .iter()
        .filter(|h| fresh(h))
        .filter_map(|h| h.claude_version.clone())
        .filter(|v| version_parts(v).is_some())
        .max_by(|a, b| version_parts(a).cmp(&version_parts(b)));
    hosts
        .iter()
        .filter(|h| !h.hidden)
        .map(|h| {
            let disk_home_pct = match (h.disk_home_free_kb, h.disk_home_total_kb) {
                (Some(free), Some(total)) if total > 0 => {
                    // Rounded, like the desktop meter: 97.6 % reads as 98.
                    Some((((total - free).max(0) * 100 + total / 2) / total).min(100) as u8)
                }
                _ => None,
            };
            // A stale sample keeps its percent but raises no flag.
            let sample_fresh = h
                .health_at
                .is_some_and(|at| now - at <= HEALTH_SAMPLE_FRESH_SECS);
            let live = agents
                .iter()
                .find(|(a, _)| a == &h.alias)
                .map(|(_, v)| v.clone());
            let agent_version = live.or_else(|| h.agent_version.clone());
            let has_live_session = sessions
                .iter()
                .any(|s| s.host_alias == h.alias && s.status != "ghost" && s.kind != "external");
            HostHealthRow {
                alias: h.alias.clone(),
                reachable: h.reachable,
                transport: h.transport.clone(),
                claude_version: h.claude_version.clone(),
                claude_version_at: h.claude_version_at,
                agent_version: agent_version.clone(),
                disk_home_pct,
                disk_low: sample_fresh && disk_home_pct.is_some_and(|p| p >= t.disk_low_pct),
                claude_behind: match (&newest, &h.claude_version) {
                    (Some(n), Some(v)) if fresh(h) => {
                        patch_behind(v, n).is_some_and(|b| b > t.claude_max_behind)
                    }
                    _ => false,
                },
                agent_behind: agent_behind(&h.transport, agent_version.as_deref(), hub_version),
                hooks_silent: h.reachable
                    && has_live_session
                    && h.last_hook_at
                        .is_none_or(|at| now - at > t.hooks_silent_secs),
                provision_warning: h.provision_warning.clone(),
                auth_overrides: h.auth_overrides.clone().unwrap_or_default(),
            }
        })
        .collect()
}

/// An agent host whose agent is OLDER than the hub. An agent ahead of the
/// hub (after a hub rollback) is not behind, and a version that is not a
/// dotted number flags nothing — as the desktop's `compareVersions`.
fn agent_behind(transport: &str, agent_version: Option<&str>, hub_version: &str) -> bool {
    transport == "agent"
        && agent_version.is_some_and(|v| {
            matches!(
                (version_parts(v), version_parts(hub_version)),
                (Some(a), Some(b)) if a < b
            )
        })
}

/// Overlay the agents connected right now onto [`Health::hosts`]: the live
/// registry's version outranks the stored hello, and `agent_behind` is
/// judged again against `hub_version`.
pub fn overlay_agents(rows: &mut [HostHealthRow], agents: &[(String, String)], hub_version: &str) {
    for row in rows.iter_mut() {
        if let Some((_, v)) = agents.iter().find(|(a, _)| a == &row.alias) {
            row.agent_version = Some(v.clone());
            row.agent_behind = agent_behind(&row.transport, Some(v), hub_version);
        }
    }
}

/// The `health.*` host thresholds in force.
pub fn host_thresholds(s: &Store) -> HostHealthThresholds {
    use crate::service::settings::{
        get_string, HEALTH_CLAUDE_MAX_BEHIND, HEALTH_DISK_LOW_PCT, HEALTH_HOOKS_SILENT_SECS,
    };
    HostHealthThresholds {
        disk_low_pct: get_string(s, HEALTH_DISK_LOW_PCT).parse().unwrap_or(90),
        claude_max_behind: get_string(s, HEALTH_CLAUDE_MAX_BEHIND)
            .parse()
            .unwrap_or(30),
        hooks_silent_secs: get_string(s, HEALTH_HOOKS_SILENT_SECS)
            .parse()
            .unwrap_or(3600),
    }
}

/// The context threshold in force (`health.context_red_pct`; the
/// registry default is [`crate::service::attention::DEFAULT_CONTEXT_RED_PCT`]).
/// `context_red` here, `needs_attention`'s `context_full` and — exported on
/// [`Health::context_red_pct`] — the desktop's chip all read this one number.
pub fn context_red_pct(s: &Store) -> f64 {
    crate::service::settings::get_string(s, crate::service::settings::HEALTH_CONTEXT_RED_PCT)
        .parse::<f64>()
        .unwrap_or(crate::service::attention::DEFAULT_CONTEXT_RED_PCT)
}

/// Consecutive failed sync passes — or, decision D25, consecutive passes
/// that skipped items — at which a tracker reads as `failing` rather than
/// `degraded`.
pub const TRACKER_FAILING_AFTER: u32 = 3;
/// [`TrackerHealth::reason`]: a person has to act on the credential
/// (`auth_failed`, `captcha`, `unconfigured`) — "Reconnect".
pub const TRACKER_REASON_CREDENTIAL: &str = "credential";
/// [`TrackerHealth::reason`]: passes fail, or the tracker is rate limited
/// or unreachable.
pub const TRACKER_REASON_SYNC_FAILED: &str = "sync_failed";
/// [`TrackerHealth::reason`]: the sync runs, but skips items it cannot
/// store (work graph M13.1) — not a credential problem.
pub const TRACKER_REASON_ITEMS_SKIPPED: &str = "items_skipped";
/// A suggestion (`work_links.state = 'suggested'`) nobody has decided for
/// this many days counts toward [`TrackersHealth::detection_backlog`].
pub const DETECTION_BACKLOG_DAYS: u32 = 7;
/// Longest [`TrackerHealth::last_error`], in characters (before an MCP
/// caller's fence).
pub const TRACKER_ERROR_MAX_CHARS: usize = tracker_sync::METRIC_ERROR_MAX_CHARS;
/// Who the fence around a tracker's error names (see
/// [`TrackersHealth::fence_errors`]).
pub const TRACKER_ERROR_FROM: &str = "a tracker's sync error";

/// `fleet_health.trackers` (work graph M12.4).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackersHealth {
    /// Every tracker the caller may see, by id.
    #[serde(default)]
    pub trackers: Vec<TrackerHealth>,
    /// How many of `trackers` are `failing` / `degraded`.
    #[serde(default)]
    pub failing: u32,
    #[serde(default)]
    pub degraded: u32,
    /// Live suggestions older than `detection_backlog_days` still awaiting a
    /// person's decision, fleet-wide (a per-host token: its own host's).
    #[serde(default)]
    pub detection_backlog: u32,
    #[serde(default)]
    pub detection_backlog_days: u32,
}

/// One tracker's health.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerHealth {
    pub tracker_id: i64,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub org_id: Option<i64>,
    #[serde(default)]
    pub org_name: Option<String>,
    /// `ok` | `degraded` | `failing` (see [`tracker_health_level`]).
    #[serde(default)]
    pub health: String,
    /// The tracker's stored state (`ok`, `auth_failed`, `rate_limited`, …).
    #[serde(default)]
    pub state: String,
    /// Sync passes in a row that failed, since this process started.
    #[serde(default)]
    pub consecutive_failures: u32,
    /// Why it is not ok, as data (work graph M13.1): `credential`,
    /// `sync_failed` or `items_skipped` (see [`tracker_health_reason`]);
    /// empty while `health` is `ok`. A caller picks its wording from this,
    /// never from `last_error`'s text. An older hub omits it.
    #[serde(default)]
    pub reason: String,
    /// Items the last pass skipped (M13.1).
    #[serde(default)]
    pub items_failed: u64,
    /// Passes in a row that skipped items (M13.1).
    #[serde(default)]
    pub consecutive_partial: u32,
    /// Why it is not ok: redacted, defused, one line, capped at
    /// [`TRACKER_ERROR_MAX_CHARS`]; fenced as untrusted for an MCP caller.
    /// `None` while `health` is `ok`.
    #[serde(default)]
    pub last_error: Option<String>,
    /// The last sync pass that ended ok (unix seconds, stored).
    #[serde(default)]
    pub last_success_at: Option<i64>,
    /// The last sync pass that ran, ok or not (unix seconds, in memory).
    #[serde(default)]
    pub last_pass_at: Option<i64>,
    /// Writes to this tracker fleet gave up on (work graph M13.4e: a PR
    /// remote link refused or failing past its retries). Reads are
    /// unaffected, so it never changes `health`.
    #[serde(default)]
    pub write_failures: u64,
}

/// A tracker's health level from its stored state and the sync's counts:
/// failed passes in a row, items the last pass skipped, and passes in a row
/// that skipped some. Pure.
///
/// - `failing`: a person has to act — the credential was refused or is
///   missing, or a captcha stands in the way (the sync stops polling those,
///   so no count would ever grow) — or [`TRACKER_FAILING_AFTER`] passes in a
///   row failed, or skipped items (D25: the same item is stuck);
/// - `degraded`: a transient state (rate limited, unreachable), an unknown
///   one (a newer hub's), fewer failed passes than that, or a last pass
///   that skipped items (M13.1);
/// - `ok`: state `ok`, and the last pass neither failed nor skipped an item.
pub fn tracker_health_level(
    state: &str,
    consecutive_failures: u32,
    items_failed: u64,
    consecutive_partial: u32,
) -> &'static str {
    match state {
        "auth_failed" | "captcha" | "unconfigured" => "failing",
        _ if consecutive_failures >= TRACKER_FAILING_AFTER => "failing",
        _ if consecutive_partial >= TRACKER_FAILING_AFTER => "failing",
        "ok" if consecutive_failures == 0 && items_failed == 0 => "ok",
        _ => "degraded",
    }
}

/// Why a tracker is not ok, as data for [`TrackerHealth::reason`] (empty
/// when it is). Pure. A credential state wins; then a transient or unknown
/// state; then skipped items — which also covers a pass that failed only
/// because every item it tried failed (#318), since that is no more a
/// credential problem than one poison item is; then failed passes.
pub fn tracker_health_reason(
    state: &str,
    consecutive_failures: u32,
    items_failed: u64,
) -> &'static str {
    match state {
        "auth_failed" | "captcha" | "unconfigured" => TRACKER_REASON_CREDENTIAL,
        "ok" if items_failed > 0 => TRACKER_REASON_ITEMS_SKIPPED,
        "ok" if consecutive_failures == 0 => "",
        _ => TRACKER_REASON_SYNC_FAILED,
    }
}

/// One tracker's row, from its stored row and its sync metrics. Pure.
pub fn tracker_health(
    t: &TrackerRow,
    m: Option<&SyncMetrics>,
    org_name: Option<String>,
) -> TrackerHealth {
    let consecutive_failures = m.map_or(0, |m| m.consecutive_failures);
    let items_failed = m.map_or(0, |m| m.items_failed);
    let consecutive_partial = m.map_or(0, |m| m.consecutive_partial);
    let health = tracker_health_level(
        &t.state,
        consecutive_failures,
        items_failed,
        consecutive_partial,
    );
    let reason = if health == "ok" {
        ""
    } else {
        tracker_health_reason(&t.state, consecutive_failures, items_failed)
    };
    // The sync's copies are already sanitised; the stored one (a `test`, a
    // `lookup`, or a pass before this process started) is sanitised here.
    // A pass's own error first, then the last skipped item's (M13.1).
    let last_error = (health != "ok")
        .then(|| {
            m.and_then(|m| m.last_error.clone().or_else(|| m.last_item_error.clone()))
                .or_else(|| t.last_error.as_deref().map(tracker_sync::metric_error))
        })
        .flatten()
        .map(|e| e.chars().take(TRACKER_ERROR_MAX_CHARS).collect());
    TrackerHealth {
        tracker_id: t.id,
        provider: t.provider.clone(),
        name: t.name.clone(),
        org_id: t.org_id,
        org_name,
        health: health.to_string(),
        state: t.state.clone(),
        consecutive_failures,
        reason: reason.to_string(),
        items_failed,
        consecutive_partial,
        last_error,
        last_success_at: t.last_sync_at,
        last_pass_at: m.and_then(|m| m.last_pass_at),
        write_failures: 0,
    }
}

/// The trackers `scope` may see in `fleet_health`: every one for the master,
/// a paired client and the desktop; for a per-host token, only its own org's
/// (an unassigned host: only unassigned trackers). Stricter than
/// [`OrgScope::sees_org`], which also shows a host unassigned work: a
/// tracker's name and error describe another team's setup, not work.
pub fn scope_sees_tracker(scope: &OrgScope, tracker_org: Option<i64>) -> bool {
    match scope {
        // This is the org boundary, not a privacy fence: a tracker belongs
        // to a COMPANY, and `TrackerHealth` carries its name, its site and
        // its last sync error — never a session. The person dimension has
        // nothing to say about whose Jira this is.
        OrgScope::All => true,
        OrgScope::Host { org, .. } => tracker_org == *org,
        // A bound client (work graph M14): its own org's trackers only.
        OrgScope::Org { org, .. } => tracker_org == Some(*org),
    }
}

/// The roll-up for `scope`, reading `metrics` (the sync's in-memory table)
/// and the store only. `now` is unix seconds.
pub fn trackers_from_store(
    s: &Store,
    scope: &OrgScope,
    metrics: &dyn Fn(&[i64]) -> Vec<SyncMetrics>,
    now: i64,
) -> TrackersHealth {
    let org_hosts: Vec<String> = match scope {
        OrgScope::Org { .. } => hosts_in_scope(s, scope)
            .into_iter()
            .map(|h| h.alias)
            .collect(),
        _ => Vec::new(),
    };
    trackers_scoped(s, scope, metrics, now, &org_hosts)
}

/// [`trackers_from_store`] with an org-bound client's visible hosts already
/// in hand (`org_hosts`, read only for [`OrgScope::Org`]): one grouped count
/// of failed writes and one backlog count over those hosts, not one query
/// per tracker and per host.
fn trackers_scoped(
    s: &Store,
    scope: &OrgScope,
    metrics: &dyn Fn(&[i64]) -> Vec<SyncMetrics>,
    now: i64,
    org_hosts: &[String],
) -> TrackersHealth {
    let rows: Vec<TrackerRow> = s
        .list_trackers()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| scope_sees_tracker(scope, t.org_id))
        .collect();
    let ids: Vec<i64> = rows.iter().map(|t| t.id).collect();
    let by_id: std::collections::HashMap<i64, SyncMetrics> = metrics(&ids)
        .into_iter()
        .map(|m| (m.tracker_id, m))
        .collect();
    let orgs: std::collections::HashMap<i64, String> = s
        .list_orgs()
        .unwrap_or_default()
        .into_iter()
        .map(|o| (o.id, o.name))
        .collect();
    let failures = if rows.is_empty() {
        Default::default()
    } else {
        s.tracker_write_failures_by_tracker().unwrap_or_default()
    };
    let trackers: Vec<TrackerHealth> = rows
        .iter()
        .map(|t| {
            let org = t.org_id.and_then(|o| orgs.get(&o).cloned());
            TrackerHealth {
                write_failures: failures.get(&t.id).copied().unwrap_or(0),
                ..tracker_health(t, by_id.get(&t.id), org)
            }
        })
        .collect();
    let count = |level: &str| trackers.iter().filter(|t| t.health == level).count() as u32;
    let before = now.saturating_sub(i64::from(DETECTION_BACKLOG_DAYS) * 86_400);
    TrackersHealth {
        failing: count("failing"),
        degraded: count("degraded"),
        detection_backlog: match scope {
            // A bound client (M14): the hosts it sees, not the fleet.
            OrgScope::Org { .. } => s.detection_backlog_on(before, org_hosts).unwrap_or(0),
            _ => s.detection_backlog(before, scope.host()).unwrap_or(0),
        },
        detection_backlog_days: DETECTION_BACKLOG_DAYS,
        trackers,
    }
}

impl TrackersHealth {
    /// Fence every error as untrusted third-party text for a caller that is
    /// an agent (`fleet_health` over MCP): a tracker's error echoes what the
    /// tracker answered. The desktop strips the fence to show it.
    pub fn fence_errors(&mut self) {
        for t in &mut self.trackers {
            if let Some(e) = t.last_error.take() {
                t.last_error = Some(crate::mcp::guard::fence_untrusted(
                    &e,
                    TRACKER_ERROR_FROM,
                    TRACKER_ERROR_MAX_CHARS,
                ));
            }
        }
    }
}

/// Pure fleet aggregates derived from cached session + host rows.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FleetSummary {
    pub hosts_reachable: u32,
    pub hosts_total: u32,
    pub sessions_total: u32,
    pub by_status: BTreeMap<String, u32>,
    pub ghosts: u32,
    pub context_red: u32,
    pub stuck: u32,
    pub usage_by_host: BTreeMap<String, UsageTotals>,
}

impl Health {
    /// Attach the tunnel supervisor's view and recompute `tunnels_flapping`.
    pub fn set_tunnels(
        &mut self,
        tunnels: std::collections::HashMap<String, crate::service::tunnel::TunnelHealth>,
    ) {
        self.tunnels_flapping = tunnels.values().filter(|t| t.is_flapping()).count() as u32;
        self.tunnels = tunnels.into_iter().collect();
    }
}

/// Roll cached session + host rows into fleet aggregates. Pure: no I/O.
///
/// `kind='external'` rows (interactive Claude sessions running outside fleet,
/// which fleet only observes) are left out of every session count — they are
/// not fleet work and must not raise its blocked / stuck roll-ups. Usage
/// still sums every row: it is real spend on that host. `kind='shell'` rows
/// (a plain shell in tmux) are left out the same way: they have no Claude
/// status, and a pane heuristic that reads their prompt as `idle` would
/// count them (F8).
pub fn summarize(sessions: &[SessionRow], hosts: &[HostRow], context_red_pct: f64) -> FleetSummary {
    let mut summary = FleetSummary {
        hosts_total: hosts.len() as u32,
        ..Default::default()
    };

    for host in hosts {
        if host.reachable {
            summary.hosts_reachable += 1;
        }
    }

    for s in sessions
        .iter()
        .filter(|s| s.kind != "external" && s.kind != "shell")
    {
        summary.sessions_total += 1;
        let status = s.claude_status.as_deref().unwrap_or("unknown");
        *summary.by_status.entry(status.to_string()).or_insert(0) += 1;

        if s.status == "ghost" {
            summary.ghosts += 1;
        }
        if s.context_pct.is_some_and(|p| p >= context_red_pct) {
            summary.context_red += 1;
        }
        if s.stuck_kind.is_some() {
            summary.stuck += 1;
        }
    }
    summary.usage_by_host = usage::per_host_totals(sessions);

    summary
}

/// Whose `fleet_health` is being built, and so what its roll-ups count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthView {
    /// The master, an unbound client, the desktop: the whole fleet.
    Fleet,
    /// A per-host token: the session and host counts stay fleet-wide; the
    /// per-host telemetry (`hosts[]`) and the spend are its own host's (same
    /// scoping as `usage_report`); the trackers are its org's, the backlog
    /// its host's (work graph M12.4) — none when `trackers` is `None` (the
    /// scope could not be read).
    Host {
        alias: String,
        trackers: Option<OrgScope>,
    },
    /// An org-bound client (work graph M14): every roll-up that sums across
    /// hosts is over the hosts it sees ([`hosts_in_scope`]) — host counts,
    /// the tunnels and `hosts[]`; the daily spend over those hosts'
    /// `usage_daily` rows; its org's trackers. `peer_links_down` is the
    /// hub's own links, not a sum over hosts, and stays.
    ///
    /// **It carries the WHOLE [`ViewScope`], not its org half** (multi-user
    /// M1, T10). An org-bound client is still somebody's DEVICE: `view_scope`
    /// reads its `person_id` exactly as it does for an unbound one, and this
    /// arm's session roll-ups — `sessions_total`, `by_status`, `ghosts`,
    /// `context_red`, `stuck`, `usage_by_host`, and `hosts[]`'
    /// `hooks_silent` — were summed over every session in the org, i.e. over
    /// a colleague's private sessions, polled once a second. Rule 2 (privacy
    /// holds against the org admin, with no override) does not stop at the
    /// org boundary, and M1's own [`Self::Person`] arm already did this one
    /// predicate better. The org half is still asked, by name
    /// ([`ViewScope::org`]), for the three questions that ARE org questions:
    /// which hosts this client sees, which trackers, and which hosts'
    /// `usage_daily` rows.
    ///
    /// [`ViewScope`]: crate::service::view_scope::ViewScope
    /// [`ViewScope::org`]: crate::service::view_scope::ViewScope::org
    Org(crate::service::view_scope::ViewScope),
    /// A PERSON's own device — a paired client bound to no org (multi-user
    /// M1, T8d).
    ///
    /// The arm this enum was missing. Such a caller used to fall through to
    /// [`Self::Fleet`], because the handler's only other test was
    /// `caller.is_scoped()`, which `mcp/auth.rs` documents as **not** "is this
    /// caller restricted at all": it is false for the master and for every
    /// paired client bound to no org alike. So a second person's phone was
    /// told `sessions_total`, `by_status`, `ghosts`, `context_red`, `stuck`
    /// and `usage_by_host` summed over everybody's sessions — how many
    /// sessions a colleague is running, how many are blocked or stuck, and the
    /// dollars their work cost.
    ///
    /// These are counts and sums rather than rows, so it was never a row leak;
    /// what makes it worth an arm is that the whole-fleet answer was reached
    /// through exactly the predicate spec §3.6 forbids using as a fence, and
    /// the next field somebody adds to [`FleetSummary`] will not be a count.
    ///
    /// What it narrows and what it does not: the SESSION roll-ups and
    /// `usage_by_host` are summed over the rows
    /// [`crate::service::view_scope::ViewScope::sees_session_row`] keeps —
    /// the same split `service::hosts::list_hosts` already makes for
    /// `unclaimed_sessions`. `usage_by_day` is withheld entirely, because
    /// `usage_daily` is keyed by `(day, host_alias, backfill)` and has no
    /// session to fence by: a per-host total is money attributable to other
    /// people's work. Hosts, tunnels, `hosts[]` and the trackers are NOT
    /// narrowed — a person's device is unbound, so its org half is `All`, and
    /// "is this host reachable, is its disk full" is fleet operations rather
    /// than anybody's session. `decide` stays [`Self::Fleet`]-only.
    ///
    /// The master token keeps [`Self::Fleet`]: §4.5 puts the hub OPERATOR
    /// deliberately out of scope, and narrowing the operator's own roll-up
    /// would be a different decision from this one.
    Person(crate::service::view_scope::ViewScope),
    /// A scoped caller whose scope could not be read: it is told nothing
    /// rather than the whole fleet ([`blank_rollups`]).
    Blank,
}

/// `usage_by_day` for a person's own device (multi-user M1, T8d).
///
/// `usage_daily` is keyed by `(day, host_alias, backfill)`: there is no
/// session on a row of it, so it cannot be fenced the way `usage_by_host`
/// can — it is summed from the session rows themselves. So the rule is all or
/// nothing, and the condition is exactly "does this caller see every session
/// row there is". That keeps a one-person install whole (rule 7: the upgrade
/// must not narrow it) and withholds the series the moment a second person's
/// work is inside it.
fn person_usage_by_day(s: &Store, now: i64, sees_every_session: bool) -> Vec<usage::DayUsage> {
    if sees_every_session {
        usage::recent_days(s, now, usage::HEALTH_DAYS, None)
    } else {
        Vec::new()
    }
}

pub fn health_from_store(s: &Store) -> Health {
    health_for(s, &HealthView::Fleet, Default::default())
}

/// The health roll-up for `view`, built in ONE pass over the cached rows:
/// one session read, one host read, the trackers once — for a scoped caller
/// too (it used to be built fleet-wide and then re-derived). `tunnels` is
/// the supervisor's view, narrowed like the hosts.
pub fn health_for(
    s: &Store,
    view: &HealthView,
    tunnels: std::collections::HashMap<String, crate::service::tunnel::TunnelHealth>,
) -> Health {
    health_at(s, view, tunnels, now_unix())
}

/// [`health_for`] at a given clock, so two roll-ups compared in a test read
/// the same `now` (a second boundary between them changed ages and windows).
fn health_at(
    s: &Store,
    view: &HealthView,
    tunnels: std::collections::HashMap<String, crate::service::tunnel::TunnelHealth>,
    now: i64,
) -> Health {
    // TODO(T3): once IpcError exists, surface the failure reason here
    // instead of silently falling back to schema_version=0 / db_ready=false.
    let schema_version = s.schema_version().unwrap_or(0);
    // Cached reconcile state only — no network / reconcile here. On a read
    // error, fall back to empty slices so health still reports core fields.
    let (sessions, hosts) = if matches!(view, HealthView::Blank) {
        (Vec::new(), Vec::new())
    } else {
        (
            s.list_all_sessions().unwrap_or_default(),
            s.list_hosts().unwrap_or_default(),
        )
    };
    let red = context_red_pct(s);
    // The live agents are overlaid by `fleet_health`, which holds the
    // registry; from the store alone the stored hello is what there is.
    // Judged across every host (the newest Claude is the fleet's), then
    // narrowed to the ones the caller sees.
    let mut host_rows = hosts_health(
        &hosts,
        &sessions,
        &host_thresholds(s),
        &[],
        crate::app_version::get(),
        now,
    );
    let metrics = &tracker_sync::metrics_for;
    let (summary, usage_by_day, trackers, tunnels) = match view {
        HealthView::Fleet => (
            summarize(&sessions, &hosts, red),
            usage::recent_days(s, now, usage::HEALTH_DAYS, None),
            trackers_scoped(s, &OrgScope::All, metrics, now, &[]),
            tunnels,
        ),
        HealthView::Host { alias, trackers } => {
            host_rows.retain(|r| &r.alias == alias);
            let mut summary = summarize(&sessions, &hosts, red);
            summary.usage_by_host.retain(|k, _| k == alias);
            (
                summary,
                usage::recent_days(s, now, usage::HEALTH_DAYS, Some(alias.as_str())),
                trackers
                    .as_ref()
                    .map(|scope| trackers_scoped(s, scope, metrics, now, &[]))
                    .unwrap_or_default(),
                tunnels,
            )
        }
        HealthView::Org(view) => {
            // This is the org boundary, not a privacy fence: which HOSTS an
            // org-bound client sees — [`hosts_in_scope`] over the host list
            // already read. The SESSION roll-ups below take the whole
            // `view` (multi-user M1, T10).
            let scope = &view.org;
            let all_hosts = hosts;
            let hosts: Vec<HostRow> = all_hosts
                .iter()
                .filter(|h| scope.sees_org(h.org_id))
                .cloned()
                .collect();
            let visible: std::collections::BTreeSet<String> =
                hosts.iter().map(|x| x.alias.clone()).collect();
            // WHOSE sessions, not only which org's (multi-user M1, T10):
            // `sees_session_row` composes the org answer this arm used to
            // ask alone with ownership, grants and §4.4's host clauses.
            let sessions: Vec<SessionRow> = sessions
                .into_iter()
                .filter(|r| view.sees_session_row(r).is_visible())
                .collect();
            // `hooks_silent` is computed from `has_live_session` — "does
            // ANYBODY have a live session on this host" — so the rows are
            // rebuilt over the sessions this caller may see, exactly as the
            // `Person` arm does. Rebuilt over EVERY host (`all_hosts`) and
            // narrowed afterwards, which is the order the pass at the top
            // of this function uses and the reason it gives: the newest
            // Claude is the FLEET's, so `claude_behind` must not be judged
            // against one org's hosts alone.
            host_rows = hosts_health(
                &all_hosts,
                &sessions,
                &host_thresholds(s),
                &[],
                crate::app_version::get(),
                now,
            );
            host_rows.retain(|r| visible.contains(&r.alias));
            let mut summary = summarize(&sessions, &hosts, red);
            summary
                .usage_by_host
                .retain(|host, _| visible.contains(host));
            // `usage_by_day` stays the ORG's figure, over the hosts this
            // client sees, and is deliberately NOT narrowed to the person the
            // way `Person`'s `person_usage_by_day` is: `usage_daily` has no
            // session on it to fence by, and a company's daily spend on its
            // own hosts is an org figure. It is the same residual recorded as
            // an owner decision at `usage::report_on`'s `by_day`
            // (`scope_guard_tests::OPEN_QUESTIONS`); if the answer there is
            // "fence it", this line follows it.
            let usage_by_day =
                usage::recent_days_on(s, now, usage::HEALTH_DAYS, &|host| visible.contains(host));
            let aliases: Vec<String> = visible.iter().cloned().collect();
            let trackers = trackers_scoped(s, scope, metrics, now, &aliases);
            let tunnels: std::collections::HashMap<_, _> = tunnels
                .into_iter()
                .filter(|(host, _)| visible.contains(host))
                .collect();
            (summary, usage_by_day, trackers, tunnels)
        }
        HealthView::Person(person) => {
            let total = sessions.len();
            let sessions: Vec<SessionRow> = sessions
                .into_iter()
                .filter(|r| person.sees_session_row(r).is_visible())
                .collect();
            // `hosts[]` is a fleet operation and is not narrowed — but
            // `hooks_silent` is computed from `has_live_session`, i.e. "does
            // ANYBODY have a live session on this host", which is one bit
            // wider than rule 6's per-host count of `unclaimed` rows. Rebuild
            // the rows over the sessions this person may see (multi-user M1,
            // T9b); every other field of a `HostHealthRow` comes from the
            // host row itself.
            host_rows = hosts_health(
                &hosts,
                &sessions,
                &host_thresholds(s),
                &[],
                crate::app_version::get(),
                now,
            );
            (
                summarize(&sessions, &hosts, red),
                person_usage_by_day(s, now, sessions.len() == total),
                trackers_scoped(s, &OrgScope::All, metrics, now, &[]),
                tunnels,
            )
        }
        HealthView::Blank => (
            FleetSummary::default(),
            Vec::new(),
            TrackersHealth::default(),
            Default::default(),
        ),
    };
    let mut h = Health {
        version: crate::app_version::get().to_string(),
        tunnels: Default::default(),
        tunnels_flapping: 0,
        db_ready: schema_version >= 1,
        schema_version,
        hosts_reachable: summary.hosts_reachable,
        hosts_total: summary.hosts_total,
        sessions_total: summary.sessions_total,
        by_status: summary.by_status,
        ghosts: summary.ghosts,
        context_red: summary.context_red,
        context_red_pct: red as u32,
        stuck: summary.stuck,
        usage_by_host: summary.usage_by_host,
        usage_by_day,
        // G14c: a listener link never goes anywhere near `LINK_CONNECTED` on
        // its own (it has no retry loop of its own to write a different
        // `state`), so a filter on `state` alone — the old computation here —
        // never counted a stalled listener as down. `Store::peer_links_down`
        // also watches a listener's client token and its last served
        // exchange.
        peer_links_down: s.peer_links_down(now).unwrap_or_default(),
        trackers,
        hub: Some(hub_health()),
        tunnels_mode: Some(tunnels_mode(s)),
        peer_links_total: s.peer_links_total().unwrap_or_default(),
        updates: None,
        hosts: host_rows,
        // The decision envelope is the hub's own business: a scoped view
        // gets none of it (`fleet_health` also drops it for those callers).
        // The decision envelope is the HUB's own health, not anybody's
        // session: the master and a person's device read it, a per-host token
        // and an org-bound client do not.
        decide: if matches!(view, HealthView::Fleet | HealthView::Person(_)) {
            crate::service::decide::health(s, now)
        } else {
            None
        },
        org_budgets: match view {
            HealthView::Fleet => crate::service::org_spend::alerts(s, now),
            HealthView::Person(person) if crate::service::org_spend::sees_all_spend(s, person) => {
                crate::service::org_spend::alerts(s, now)
            }
            _ => Vec::new(),
        },
        loops: crate::service::loops::registry().snapshot(),
        automation_paused: crate::service::loops::paused(s),
    };
    h.set_tunnels(tunnels);
    if matches!(view, HealthView::Blank) {
        blank_rollups(&mut h);
    }
    h
}

/// The hosts an org-bound client sees (work graph M14): its org's, and
/// unassigned ones while the org's `bound_sees_unassigned` is on (D31) —
/// [`OrgScope::sees_org`] over the host's org. Empty on a read error, and a
/// host the store no longer has is never in it (fail closed).
pub fn hosts_in_scope(s: &Store, scope: &OrgScope) -> Vec<HostRow> {
    s.list_hosts()
        .unwrap_or_default()
        .into_iter()
        .filter(|h| scope.sees_org(h.org_id))
        .collect()
}

/// Every roll-up blanked, for an org-bound client whose scope could not be
/// read: it is told nothing rather than the whole fleet.
pub fn blank_rollups(h: &mut Health) {
    h.hosts_reachable = 0;
    h.hosts_total = 0;
    h.sessions_total = 0;
    h.by_status.clear();
    h.ghosts = 0;
    h.context_red = 0;
    h.stuck = 0;
    h.usage_by_host.clear();
    h.usage_by_day.clear();
    h.tunnels.clear();
    h.tunnels_flapping = 0;
    h.trackers = Default::default();
    h.hosts.clear();
    h.decide = None;
    h.org_budgets.clear();
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn health_check(store: &Mutex<Store>) -> Health {
    // A poisoned store mutex IS an unhealthy state — report db_ready=false
    // rather than panicking the command (which the old `.expect` did).
    match store.lock() {
        Ok(s) => health_from_store(&s),
        Err(_) => unready_health(),
    }
}

/// The health of a store whose lock is poisoned: `db_ready: false`, every
/// roll-up empty.
pub fn unready_health() -> Health {
    Health {
        version: crate::app_version::get().to_string(),
        tunnels: Default::default(),
        tunnels_flapping: 0,
        db_ready: false,
        schema_version: 0,
        hosts_reachable: 0,
        hosts_total: 0,
        sessions_total: 0,
        by_status: BTreeMap::new(),
        ghosts: 0,
        context_red: 0,
        context_red_pct: crate::service::attention::DEFAULT_CONTEXT_RED_PCT as u32,
        stuck: 0,
        usage_by_host: BTreeMap::new(),
        usage_by_day: Vec::new(),
        peer_links_down: 0,
        trackers: TrackersHealth::default(),
        hub: None,
        tunnels_mode: None,
        peer_links_total: 0,
        updates: None,
        hosts: Vec::new(),
        decide: None,
        org_budgets: Vec::new(),
        loops: Vec::new(),
        automation_paused: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::tunnel::TunnelHealth;
    use std::collections::HashMap;
    use std::sync::Mutex;

    fn session(
        claude_status: Option<&str>,
        context_pct: Option<f64>,
        stuck_kind: Option<&str>,
    ) -> SessionRow {
        SessionRow {
            id: 0,
            row_version: 0,
            prompt_submit_seq: 0,
            tmux_name: "t".to_string(),
            host_alias: "alpha".to_string(),
            project_id: None,
            worktree_id: None,
            created_at: 0,
            last_activity_at: 0,
            status: "running".to_string(),
            notes: None,
            account_uuid: None,
            kind: "fg".to_string(),
            reviews_session_id: None,
            worktree_key: None,
            lost_at: None,
            lost_reason: None,
            claude_session_id: None,
            claude_status: claude_status.map(str::to_string),
            effort_level: None,
            pr_url: None,
            current_activity: None,
            context_pct,
            stuck_kind: stuck_kind.map(str::to_string),
            friendly_name: None,
            safe_kill_state: None,
            safe_kill_nonce: None,
            safe_kill_detail: None,
            safe_kill_requested_at: None,
            idle_since: None,
            stuck_since: None,
            last_playbook_at: None,
            last_prompt: None,
            started_at: None,
            last_turn_at: None,
            ci_status: None,
            turn_seq: 0,
            last_stop_at: None,
            stale_working_at: None,
            stale_demoted_at: None,
            work_rev: 0,
            pr_evidence: None,
            pr_checked_at: None,
            owner_person_id: None,
            visibility: crate::store::VISIBILITY_UNCLAIMED.into(),
            claude_profile: None,
            agent: crate::store::AGENT_CLAUDE.into(),
            origin: None,
            origin_ref: None,
            last_viewed_at: None,
            turn_outcome: None,
            proposals: Vec::new(),
            pending_form: None,
            form_draft: None,
            parent_session_id: None,
            tags: Vec::new(),
            usage: Default::default(),
            context: Default::default(),
            pending_input: None,
            work: None,
            work_rejected: vec![],
            work_suggested: None,
            org_id: None,
        }
    }

    fn ghost(mut row: SessionRow) -> SessionRow {
        row.status = "ghost".to_string();
        row
    }

    fn host(alias: &str, reachable: bool) -> HostRow {
        HostRow {
            alias: alias.to_string(),
            ssh_alias: None,
            reachable,
            claude_version: None,
            tmux_version: None,
            hidden: false,
            last_pinged_at: None,
            account_uuid: None,
            provisioned: false,
            transport: "ssh".to_string(),
            org_id: None,
            claude_version_at: None,
            disk_home_free_kb: None,
            disk_home_total_kb: None,
            disk_tmp_free_kb: None,
            load_1m: None,
            mem_avail_kb: None,
            uptime_secs: None,
            health_at: None,
            last_hook_at: None,
            agent_version: None,
            provisioned_at: None,
            provision_stale: false,
            unclaimed_sessions: None,
            provision_warning: None,
            auth_overrides: None,
            claude_profiles: None,
            cpu_count: None,
            mem_total_kb: None,
            boot_at: None,
            latency_ms: None,
            worktree_kb: None,
            worktree_at: None,
            agents_on_path: None,
            last_reachable_at: None,
            last_probe_error_code: None,
            last_probe_error: None,
            harnesses: None,
        }
    }

    fn session_on(host: &str, status: Option<&str>) -> SessionRow {
        let mut s = session(status, None, None);
        s.host_alias = host.to_string();
        s
    }

    #[test]
    fn hosts_health_flags_disk_claude_agent_and_silent_hooks() {
        let now = 1_700_000_000;
        let mut full = host("full", true);
        full.disk_home_free_kb = Some(3_600_000);
        full.disk_home_total_kb = Some(150_000_000);
        full.claude_version = Some("2.1.214".into());
        full.claude_version_at = Some(now - 60);
        full.transport = "agent".into();
        full.agent_version = Some("0.2.26".into());
        full.last_hook_at = Some(now - 7200);
        full.health_at = Some(now - 60);
        let mut fine = host("fine", true);
        fine.disk_home_free_kb = Some(72_000_000);
        fine.disk_home_total_kb = Some(96_000_000);
        fine.claude_version = Some("2.1.282".into());
        fine.claude_version_at = Some(now - 60);
        fine.last_hook_at = Some(now - 60);
        fine.health_at = Some(now - 60);
        let sessions = vec![
            session_on("full", Some("working")),
            session_on("fine", Some("idle")),
        ];
        let rows = hosts_health(
            &[full, fine],
            &sessions,
            &HostHealthThresholds {
                disk_low_pct: 90,
                claude_max_behind: 30,
                hooks_silent_secs: 3600,
            },
            &[("full".to_string(), "0.2.26".to_string())],
            "0.3.1",
            now,
        );
        let full = rows.iter().find(|r| r.alias == "full").unwrap();
        assert_eq!(full.disk_home_pct, Some(98));
        assert!(full.disk_low);
        assert!(
            full.claude_behind,
            "2.1.214 is 68 patch releases behind 2.1.282"
        );
        assert!(full.agent_behind, "0.2.26 on a 0.3.1 hub");
        assert!(full.hooks_silent, "a working session and no hook for 2 h");
        assert_eq!(full.agent_version.as_deref(), Some("0.2.26"));
        let fine = rows.iter().find(|r| r.alias == "fine").unwrap();
        assert_eq!(fine.disk_home_pct, Some(25));
        assert!(!fine.disk_low && !fine.claude_behind && !fine.agent_behind && !fine.hooks_silent);
    }

    /// The live registry outranks the stored hello: an agent that
    /// reconnected on the hub's version is no longer behind.
    #[test]
    fn overlay_agents_takes_the_live_version_over_the_stored_hello() {
        let mut rows = vec![HostHealthRow {
            alias: "trn".into(),
            transport: "agent".into(),
            agent_version: Some("0.2.26".into()),
            agent_behind: true,
            ..Default::default()
        }];
        overlay_agents(&mut rows, &[("trn".into(), "0.3.1".into())], "0.3.1");
        assert_eq!(rows[0].agent_version.as_deref(), Some("0.3.1"));
        assert!(!rows[0].agent_behind);
        // A live agent AHEAD of the hub (a hub rollback) is not behind.
        rows[0].agent_behind = true;
        overlay_agents(&mut rows, &[("trn".into(), "0.4.0".into())], "0.3.1");
        assert_eq!(rows[0].agent_version.as_deref(), Some("0.4.0"));
        assert!(!rows[0].agent_behind);
    }

    /// `agent_behind` is "older than the hub", not "different from it".
    #[test]
    fn agent_behind_only_when_older() {
        assert!(agent_behind("agent", Some("0.2.26"), "0.3.1"));
        assert!(!agent_behind("agent", Some("0.4.0"), "0.3.1"));
        assert!(!agent_behind("agent", Some("0.3.1"), "0.3.1"));
        assert!(!agent_behind("agent", Some("garbage"), "0.3.1"));
        assert!(!agent_behind("agent", None, "0.3.1"));
        assert!(!agent_behind("ssh", Some("0.2.26"), "0.3.1"));
    }

    /// A disk sample from a host that stopped answering keeps its percent
    /// but no longer raises `disk_low`; neither does a host never sampled.
    #[test]
    fn hosts_health_ignores_a_stale_disk_sample() {
        let now = 1_700_000_000;
        let t = HostHealthThresholds {
            disk_low_pct: 90,
            claude_max_behind: 30,
            hooks_silent_secs: 3600,
        };
        let mut stale = host("stale", true);
        stale.disk_home_free_kb = Some(3_000_000);
        stale.disk_home_total_kb = Some(150_000_000);
        stale.health_at = Some(now - 2 * 3600);
        let mut unstamped = stale.clone();
        unstamped.alias = "unstamped".into();
        unstamped.health_at = None;
        let rows = hosts_health(&[stale, unstamped], &[], &t, &[], "0.3.1", now);
        for r in &rows {
            assert_eq!(r.disk_home_pct, Some(98), "{}", r.alias);
            assert!(!r.disk_low, "{}", r.alias);
        }
    }

    /// The code `health_for` replaced, kept as the reference: the whole
    /// fleet's health, then each scope re-derived over second reads — one
    /// failure count per tracker and, for an org-bound client, one backlog
    /// count per host.
    fn two_pass_reference(
        s: &Store,
        view: &HealthView,
        tunnels: HashMap<String, TunnelHealth>,
        now: i64,
    ) -> Health {
        let metrics = &tracker_sync::metrics_for;
        let mut h = health_at(s, &HealthView::Fleet, Default::default(), now);
        h.set_tunnels(tunnels);
        match view {
            HealthView::Fleet => {}
            HealthView::Host { alias, trackers } => {
                h.hosts.retain(|r| &r.alias == alias);
                h.usage_by_host.retain(|k, _| k == alias);
                h.usage_by_day =
                    usage::recent_days(s, now, usage::HEALTH_DAYS, Some(alias.as_str()));
                h.trackers = match trackers {
                    Some(scope) => trackers_from_store(s, scope, metrics, now),
                    None => Default::default(),
                };
            }
            HealthView::Org(view) => {
                let scope = &view.org;
                let hosts = hosts_in_scope(s, scope);
                let visible: std::collections::BTreeSet<String> =
                    hosts.iter().map(|x| x.alias.clone()).collect();
                let sessions: Vec<SessionRow> = s
                    .list_all_sessions()
                    .unwrap()
                    .into_iter()
                    .filter(|r| view.sees_session_row(r).is_visible())
                    .collect();
                // The person fence narrows `hooks_silent` too (multi-user
                // M1, T10): rebuilt over every host, then narrowed to the
                // ones this client sees.
                h.hosts = hosts_health(
                    &s.list_hosts().unwrap(),
                    &sessions,
                    &host_thresholds(s),
                    &[],
                    crate::app_version::get(),
                    now,
                );
                h.hosts.retain(|r| visible.contains(&r.alias));
                let summary = summarize(&sessions, &hosts, context_red_pct(s));
                h.hosts_reachable = summary.hosts_reachable;
                h.hosts_total = summary.hosts_total;
                h.sessions_total = summary.sessions_total;
                h.by_status = summary.by_status;
                h.ghosts = summary.ghosts;
                h.context_red = summary.context_red;
                h.stuck = summary.stuck;
                h.usage_by_host = summary.usage_by_host;
                h.usage_by_host.retain(|host, _| visible.contains(host));
                h.usage_by_day = usage::recent_days_on(s, now, usage::HEALTH_DAYS, &|host| {
                    visible.contains(host)
                });
                let tunnels = std::mem::take(&mut h.tunnels);
                h.set_tunnels(
                    tunnels
                        .into_iter()
                        .filter(|(host, _)| visible.contains(host))
                        .collect(),
                );
                h.trackers = trackers_from_store(s, scope, metrics, now);
                let before = now.saturating_sub(i64::from(DETECTION_BACKLOG_DAYS) * 86_400);
                h.trackers.detection_backlog = hosts
                    .iter()
                    .map(|x| s.detection_backlog(before, Some(&x.alias)).unwrap())
                    .sum();
            }
            HealthView::Person(person) => {
                let hosts = s.list_hosts().unwrap();
                let sessions: Vec<SessionRow> = s
                    .list_all_sessions()
                    .unwrap()
                    .into_iter()
                    .filter(|r| person.sees_session_row(r).is_visible())
                    .collect();
                let summary = summarize(&sessions, &hosts, context_red_pct(s));
                h.sessions_total = summary.sessions_total;
                h.by_status = summary.by_status;
                h.ghosts = summary.ghosts;
                h.context_red = summary.context_red;
                h.stuck = summary.stuck;
                h.usage_by_host = summary.usage_by_host;
                h.usage_by_day = person_usage_by_day(
                    s,
                    now,
                    sessions.len() == s.list_all_sessions().unwrap().len(),
                );
            }
            HealthView::Blank => blank_rollups(&mut h),
        }
        for t in &mut h.trackers.trackers {
            t.write_failures = s.tracker_write_failures(t.tracker_id).unwrap();
        }
        h
    }

    /// The JSON a caller receives, minus the hub's uptime (a clock).
    fn comparable(h: &Health) -> serde_json::Value {
        let mut v = serde_json::to_value(h).unwrap();
        v.as_object_mut().unwrap().remove("hub");
        v
    }

    /// `fleet_health` is built in one pass for the caller's scope; for every
    /// scope it must equal the two-pass code it replaced. Two orgs, three
    /// hosts (one unassigned), sessions of every kind, usage on each host,
    /// a tracker per org and an unassigned one, failed writes, a detection
    /// backlog on each host, and a tunnel per host.
    #[test]
    fn single_pass_health_equals_the_two_pass_code_for_every_scope() {
        let s = Store::open_in_memory().unwrap();
        for h in ["h-a", "h-b", "h-none"] {
            s.upsert_host(h).unwrap();
        }
        s.update_host_probe("h-b", false, None, None, 0).unwrap();
        let a = s.add_org("Company A", None, false).unwrap();
        let b = s.add_org("Company B", None, false).unwrap();
        s.set_host_org("h-a", Some(a.id)).unwrap();
        s.set_host_org("h-b", Some(b.id)).unwrap();
        let now = now_unix();
        let mut n = 0;
        let mut add = |host: &str, kind: &str, status: Option<&str>| {
            n += 1;
            let id = s
                .upsert_session(&format!("s{n}"), host, None, None, 1, 1, "running", None)
                .unwrap();
            s.conn_ref()
                .execute(
                    "UPDATE sessions SET kind=?1, claude_status=?2 WHERE id=?3",
                    rusqlite::params![kind, status, id],
                )
                .unwrap();
            s.apply_usage(
                id,
                host,
                &crate::store::UsageDelta {
                    reset: false,
                    totals: UsageTotals {
                        input_tokens: 10 * n,
                        cost_micros: 100 * n,
                        ..Default::default()
                    },
                    model: Some("claude-opus-5".into()),
                    offset: 10,
                    source: format!("s{n}.jsonl"),
                    last_msg_id: None,
                    last_msg_usage: None,
                    now,
                    by_day: Vec::new(),
                    backfill_until: None,
                },
            )
            .unwrap();
            id
        };
        let mut backlog = Vec::new();
        for host in ["h-a", "h-b", "h-none"] {
            backlog.push(add(host, "work", Some("working")));
            add(host, "work", None);
            add(host, "external", Some("idle"));
            add(host, "shell", Some("idle"));
        }
        for (i, sid) in backlog.into_iter().enumerate() {
            let key = format!("ABC-{}", i + 1);
            let l = s
                .link_session_work(sid, crate::store::WorkTarget::Key(&key), "manual")
                .unwrap();
            s.set_work_link_state_for_test(l.id, "suggested", 0)
                .unwrap();
        }
        let mut trackers = Vec::new();
        for (name, org) in [("alpha", Some(a.id)), ("beta", Some(b.id)), ("gamma", None)] {
            let t = s
                .add_tracker("jira", name, &format!("https://{name}.atlassian.net"))
                .unwrap();
            s.set_tracker_org(t.id, org).unwrap();
            trackers.push(t.id);
        }
        for (k, t) in [trackers[0], trackers[0], trackers[1], trackers[2]]
            .into_iter()
            .enumerate()
        {
            let url = format!("https://github.com/o/r/pull/{k}");
            s.enqueue_tracker_write(&crate::store::NewTrackerWrite {
                tracker_id: t,
                item_key: "ABC-1",
                op: crate::store::WRITE_OP_PR_REMOTE_LINK,
                url: &url,
                title: "PR",
                link_id: None,
                claude_session_id: None,
                session_org_id: None,
            })
            .unwrap();
            let id = s
                .due_tracker_writes(t, i64::MAX, 10)
                .unwrap()
                .into_iter()
                .find(|w| w.url == url)
                .unwrap()
                .id;
            s.retry_tracker_write(id, "forbidden", None, true).unwrap();
        }
        let tunnels = || -> HashMap<String, TunnelHealth> {
            ["h-a", "h-b", "h-none"]
                .into_iter()
                .map(|h| {
                    (
                        h.to_string(),
                        TunnelHealth {
                            supervised: true,
                            consecutive_failures: if h == "h-b" { 9 } else { 0 },
                            ..Default::default()
                        },
                    )
                })
                .collect()
        };

        // A person who owns none of the fixture's rows, as an ORG-BOUND
        // device: the T10 Org arm narrows to zero sessions for them, and the
        // reference must agree. Built through `Caller::view_scope`, which is
        // the one constructor (`view_scope_tests::only_caller_view_scope_constructs_a_view_scope`).
        let nobodys_person = s.create_person("nobody", None).unwrap().id;
        let nobodys_scope = crate::mcp::auth::Caller {
            api: None,
            host_alias: None,
            client: Some(crate::mcp::auth::ClientRef {
                id: 99,
                name: "nobodys-bound-phone".into(),
                trusted: false,
                org_id: Some(a.id),
                person_id: Some(nobodys_person),
            }),
            mode: crate::mcp::auth::TokenMode::Full,
            pane: None,
            is_personal_owner: false,
        }
        .view_scope(&s)
        .unwrap();
        let views = [
            HealthView::Fleet,
            HealthView::Host {
                alias: "h-a".into(),
                trackers: Some(OrgScope::for_host(&s, "h-a").unwrap()),
            },
            HealthView::Host {
                alias: "h-none".into(),
                trackers: Some(OrgScope::for_host(&s, "h-none").unwrap()),
            },
            HealthView::Host {
                alias: "h-b".into(),
                trackers: None,
            },
            // `org_only_view` is internal + this org, so these two cases
            // prove the arm's PLUMBING against the reference exactly as the
            // `Person` case below does; the case after them narrows BY
            // PERSON as well, so the T10 fence is in the parity too.
            HealthView::Org(crate::service::view_scope::org_only_view(&OrgScope::Org {
                org: a.id,
                sees_unassigned: true,
            })),
            HealthView::Org(crate::service::view_scope::org_only_view(&OrgScope::Org {
                org: b.id,
                sees_unassigned: false,
            })),
            HealthView::Org(nobodys_scope.clone()),
            HealthView::Blank,
            // Multi-user M1 (T8d). `internal()` sees every row, so this case
            // proves the arm's PLUMBING is equivalent to the reference; the
            // person fence itself is pinned behaviourally in
            // `mcp::tools::tests`.
            HealthView::Person(crate::service::view_scope::ViewScope::internal()),
        ];
        for view in &views {
            // One clock for both: a second ticking between them is not a
            // difference between the two codes.
            let at = now_unix();
            let one = health_at(&s, view, tunnels(), at);
            let two = two_pass_reference(&s, view, tunnels(), at);
            assert_eq!(comparable(&one), comparable(&two), "{view:?}");
        }

        // The fixture exercises what it claims to.
        let fleet = health_for(&s, &HealthView::Fleet, tunnels());
        assert_eq!(fleet.sessions_total, 6, "external and shell rows left out");
        assert_eq!(fleet.trackers.detection_backlog, 3);
        assert_eq!(fleet.trackers.trackers[0].write_failures, 2);
        let org_a = health_for(
            &s,
            &HealthView::Org(crate::service::view_scope::org_only_view(&OrgScope::Org {
                org: a.id,
                sees_unassigned: true,
            })),
            tunnels(),
        );
        assert_eq!(
            org_a.hosts_total, 2,
            "its org's host and the unassigned one"
        );
        assert_eq!(org_a.trackers.detection_backlog, 2);
        assert_eq!(org_a.tunnels.len(), 2);
        assert!(org_a.usage_by_host.keys().all(|h| h != "h-b"));
    }

    /// An org-bound client whose scope could not be read is told nothing,
    /// per-host telemetry included.
    #[test]
    fn blank_rollups_clears_the_host_rows() {
        let mut h = health_check(&Mutex::new(Store::open_in_memory().unwrap()));
        h.hosts = vec![HostHealthRow {
            alias: "h-b".into(),
            disk_home_pct: Some(98),
            ..Default::default()
        }];
        blank_rollups(&mut h);
        assert!(h.hosts.is_empty());
    }

    /// F8: two `-term` shells were `by_status.unknown = 2`, a third said
    /// `idle`. A shell is not a Claude session; it leaves every roll-up.
    #[test]
    fn summarize_skips_shell_rows() {
        let mut sh = session(Some("idle"), Some(99.0), Some("press_enter"));
        sh.kind = "shell".to_string();
        let mut sh2 = session(None, None, None);
        sh2.kind = "shell".to_string();
        let s = summarize(&[sh, sh2, session(Some("working"), None, None)], &[], 85.0);
        assert_eq!(s.sessions_total, 1);
        assert_eq!(s.by_status.get("unknown"), None);
        assert_eq!(s.by_status.get("idle"), None);
        assert_eq!(s.context_red, 0);
        assert_eq!(s.stuck, 0);
    }

    #[test]
    fn summarize_rolls_up_statuses_ghosts_context_and_stuck() {
        let sessions = vec![
            // null status → "unknown" bucket
            session(None, None, None),
            // working
            session(Some("working"), Some(10.0), None),
            // a ghost: lifecycle status, with its last known claude_status
            ghost(session(Some("idle"), None, None)),
            // context red (>= 85)
            session(Some("working"), Some(90.0), None),
            // stuck
            session(Some("idle"), None, Some("press_enter")),
        ];
        let hosts = vec![
            host("alpha", true),
            host("beta", false),
            host("gamma", true),
        ];

        let s = summarize(&sessions, &hosts, 85.0);

        assert_eq!(s.hosts_total, 3);
        assert_eq!(s.hosts_reachable, 2);
        assert_eq!(s.sessions_total, 5);

        // null → unknown bucket; "working" counted twice.
        assert_eq!(s.by_status.get("unknown"), Some(&1));
        assert_eq!(s.by_status.get("working"), Some(&2));
        assert_eq!(s.by_status.get("idle"), Some(&2));

        assert_eq!(s.ghosts, 1);
        assert_eq!(s.context_red, 1);
        assert_eq!(s.stuck, 1);
    }

    /// Regression (R1/D15): `ghosts` used to count `claude_status ==
    /// "ghost"`, a value that column never holds, so it was always 0.
    #[test]
    fn ghosts_are_counted_by_lifecycle_status_not_claude_status() {
        let sessions = vec![
            ghost(session(None, None, None)),
            ghost(session(Some("working"), None, None)),
            session(Some("idle"), None, None),
            // A stray "ghost" in claude_status is not a ghost session.
            session(Some("ghost"), None, None),
        ];
        let s = summarize(&sessions, &[], 85.0);
        assert_eq!(s.ghosts, 2);
        assert_eq!(s.sessions_total, 4);
    }

    #[test]
    fn health_from_store_counts_a_ghosted_session() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        store
            .upsert_session("live", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store
            .upsert_session("gone", "alpha", None, None, 1, 1, "ghost", None)
            .unwrap();
        let h = health_from_store(&store);
        assert_eq!(h.sessions_total, 2);
        assert_eq!(h.ghosts, 1);
        assert_eq!(serde_json::to_value(&h).unwrap()["ghosts"], 1);
    }

    #[test]
    fn health_from_store_skips_external_rows() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        // An interactive Claude session running outside fleet, blocked.
        store
            .upsert_bg_session(
                "alpha",
                "bg:ext-1",
                None,
                "ext-1",
                Some("blocked"),
                1,
                "external",
                1,
            )
            .unwrap();
        // A fleet tmux session, blocked.
        let id = store
            .upsert_session("dev", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        store.set_claude_session_id(id, "tmux-1").unwrap();
        store
            .set_claude_status_by_session_id("tmux-1", "blocked")
            .unwrap();

        let h = health_from_store(&store);
        assert_eq!(h.sessions_total, 1);
        assert_eq!(h.by_status.get("blocked"), Some(&1));
        assert_eq!(h.by_status.values().sum::<u32>(), 1);
    }

    #[test]
    fn summarize_skips_external_rows() {
        let mut ext = session(Some("blocked"), Some(99.0), Some("press_enter"));
        ext.kind = "external".to_string();
        let sessions = vec![ext, session(Some("blocked"), None, None)];
        let s = summarize(&sessions, &[], 85.0);
        assert_eq!(s.sessions_total, 1);
        assert_eq!(s.by_status.get("blocked"), Some(&1));
        assert_eq!(s.context_red, 0);
        assert_eq!(s.stuck, 0);
    }

    #[test]
    fn summarize_threshold_is_inclusive_at_85() {
        let sessions = vec![
            session(Some("working"), Some(84.9), None),
            session(Some("working"), Some(85.0), None),
        ];
        let s = summarize(&sessions, &[], 85.0);
        assert_eq!(s.context_red, 1);
        assert_eq!(s.hosts_total, 0);
        assert_eq!(s.hosts_reachable, 0);
        assert_eq!(
            summarize(&sessions, &[], 90.0).context_red,
            0,
            "one threshold, the setting's"
        );
    }

    #[test]
    fn summarize_empty_is_all_zero() {
        let s = summarize(&[], &[], 85.0);
        assert_eq!(s, FleetSummary::default());
    }

    #[test]
    fn summarize_rolls_up_usage_per_host_and_skips_hosts_without_usage() {
        let mut a = session(Some("working"), None, None);
        a.usage.usage_input_tokens = 10;
        a.usage.usage_cost_micros = 50;
        let mut b = session(Some("idle"), None, None);
        b.usage.usage_output_tokens = 4;
        b.usage.usage_cost_micros = 100;
        let mut c = session(None, None, None);
        c.host_alias = "beta".into();
        let s = summarize(&[a, b, c], &[], 85.0);
        assert_eq!(s.usage_by_host.len(), 1, "beta counted nothing");
        let alpha = s.usage_by_host["alpha"];
        assert_eq!(alpha.input_tokens, 10);
        assert_eq!(alpha.output_tokens, 4);
        assert_eq!(alpha.cost_micros, 150);
    }

    #[test]
    fn health_from_store_reports_usage_by_host_and_day() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_host("alpha").unwrap();
        let id = store
            .upsert_session("t", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        // One clock read: a UTC midnight between seeding and asserting
        // must not move the expected day.
        let now = now_unix();
        store
            .apply_usage(
                id,
                "alpha",
                &crate::store::UsageDelta {
                    reset: false,
                    totals: UsageTotals {
                        input_tokens: 7,
                        cost_micros: 35,
                        ..Default::default()
                    },
                    model: Some("claude-opus-5".into()),
                    offset: 10,
                    source: "x.jsonl".into(),
                    last_msg_id: None,
                    last_msg_usage: None,
                    now,
                    by_day: Vec::new(),
                    backfill_until: None,
                },
            )
            .unwrap();
        let h = health_from_store(&store);
        assert_eq!(h.usage_by_host["alpha"].cost_micros, 35);
        assert_eq!(h.usage_by_day.len(), 1);
        assert_eq!(h.usage_by_day[0].totals.input_tokens, 7);
        assert_eq!(
            h.usage_by_day[0].day,
            usage::day_string(now.div_euclid(86_400))
        );
        let v = serde_json::to_value(&h).unwrap();
        assert_eq!(v["usage_by_day"][0]["cost_micros"], 35);

        // Scoped to its own host, a per-host caller keeps alpha's usage…
        let host_view = |alias: &str| HealthView::Host {
            alias: alias.into(),
            trackers: None,
        };
        let own = health_for(&store, &host_view("alpha"), Default::default());
        assert_eq!(own.usage_by_host.len(), 1);
        assert_eq!(own.usage_by_day.len(), 1);
        // …and another host's caller sees none of it.
        let other = health_for(&store, &host_view("beta"), Default::default());
        assert!(other.usage_by_host.is_empty());
        assert!(other.usage_by_day.is_empty());
        assert_eq!(other.sessions_total, 1, "counts stay fleet-wide");
    }

    #[test]
    fn health_from_store_reports_version_db_ready_and_schema() {
        let store = Mutex::new(Store::open_in_memory().expect("in-memory store"));
        let s = store.lock().unwrap();
        let h = health_from_store(&s);
        assert_eq!(h.version, crate::app_version::get());
        assert!(h.db_ready);
        // Track the authoritative constant, not a literal: a literal here
        // rots every time a new migration lands (it did, at migration 043).
        assert_eq!(h.schema_version, crate::store::LATEST_SCHEMA_VERSION);
        // Empty store → empty roll-up.
        assert_eq!(h.sessions_total, 0);
        assert_eq!(h.hosts_total, 0);
        assert!(h.by_status.is_empty());
    }

    /// The hub client deserialises `fleet_health`'s answer into this type. A
    /// `Health` that arrives short of a field must be an ERROR, not a
    /// silently zeroed fleet: `stuck: 0, ghosts: 0, context_red: 0` is the
    /// most reassuring thing this app can say, and it must never be said on
    /// the strength of a field that was not there.
    ///
    /// This is the pin on the container `#[serde(default)]` that used to sit
    /// on `Health`. Put it back and every case below parses.
    #[test]
    fn a_partial_health_is_rejected_rather_than_zeroed() {
        for body in [
            // The whole answer missing — a wrapper object, a 200 with `{}`.
            "{}",
            // One renamed field. Everything else is present and right, which
            // is what makes a default so quiet here.
            r#"{"version":"1","db_ready":true,"schema_version":32,
                "hosts_reachable":1,"hosts_total":1,"sessions_total":3,
                "by_status":{},"ghosts":0,"context_red":0,
                "stuckCount":2,
                "usage_by_host":{},"usage_by_day":[]}"#,
        ] {
            let parsed = serde_json::from_str::<Health>(body);
            assert!(
                parsed.is_err(),
                "an incomplete Health must fail to parse, not read as a \
                 healthy fleet; {body} parsed"
            );
        }
        // And a complete one still parses, so the assertion above is about
        // the missing field and not about the shape in general.
        let whole = serde_json::to_string(&Health {
            version: "1".into(),
            tunnels: Default::default(),
            tunnels_flapping: 0,
            db_ready: true,
            schema_version: 32,
            hosts_reachable: 1,
            hosts_total: 2,
            sessions_total: 3,
            by_status: BTreeMap::new(),
            ghosts: 4,
            context_red: 5,
            context_red_pct: 85,
            stuck: 6,
            usage_by_host: BTreeMap::new(),
            usage_by_day: Vec::new(),
            peer_links_down: 0,
            trackers: TrackersHealth::default(),
            hub: None,
            tunnels_mode: None,
            peer_links_total: 0,
            updates: None,
            hosts: Vec::new(),
            decide: None,
            org_budgets: Vec::new(),
            loops: Vec::new(),
            automation_paused: false,
        })
        .expect("Health serialises");
        let back: Health = serde_json::from_str(&whole).expect("a whole Health parses");
        assert_eq!(back.stuck, 6);
    }

    /// A link outside `connected` (retrying, refused, incompatible) is a hub
    /// federation problem an operator wants surfaced the same way a stuck
    /// tunnel is — `peer_links_down` counts them, ignoring `connected` links
    /// and revoked ones (neither is "down": one is fine, the other is gone).
    #[test]
    fn health_from_store_counts_peer_links_not_connected() {
        let store = Store::open_in_memory().unwrap();
        let ok = store.insert_dialer_link("https://b.example", "t1").unwrap();
        store.adopt_dialer_fleet(ok, "fleet-b").unwrap();
        store
            .set_peer_link_state(ok, crate::store::LINK_CONNECTED, None, 1)
            .unwrap();
        let bad = store.insert_dialer_link("https://c.example", "t2").unwrap();
        store.adopt_dialer_fleet(bad, "fleet-c").unwrap();
        store
            .set_peer_link_state(bad, crate::store::LINK_REFUSED, Some("401"), 1)
            .unwrap();
        let h = health_from_store(&store);
        assert_eq!(h.peer_links_down, 1);
    }

    /// G14c: a LISTENER link has no `state` of its own that goes stale (it
    /// only ever holds `LINK_CONNECTED`), so the old `state != connected`
    /// filter above never counted a stalled listener as down at all. It now
    /// counts as down when it has never served an exchange (or the last one
    /// is stale) OR its client token has been revoked — a fresh exchange on
    /// a live token is the only way it reads as up.
    #[test]
    fn health_from_store_counts_a_revoked_or_stale_listener_link_as_down() {
        let store = Store::open_in_memory().unwrap();
        let c = store
            .insert_client_token("hub-a", &format!("{:0>64}", "hub-a".len()), "peer")
            .unwrap()
            .id;
        let link = store.ensure_listener_link(c, "fleet-a").unwrap();
        assert_eq!(
            health_from_store(&store).peer_links_down,
            1,
            "never exchanged yet"
        );

        store
            .set_peer_link_state(link.id, crate::store::LINK_CONNECTED, None, now_unix())
            .unwrap();
        assert_eq!(
            health_from_store(&store).peer_links_down,
            0,
            "a fresh served exchange on a live token is up"
        );

        store.revoke_client_token("hub-a").unwrap();
        assert_eq!(
            health_from_store(&store).peer_links_down,
            1,
            "a revoked token is down even with a fresh last_exchange_at"
        );
    }

    /// The pin on `#[serde(default)]` for this ONE field (unlike the rest of
    /// `Health`, deliberately not container-default — see the struct doc): an
    /// older hub's JSON, minted before this field existed, must still parse,
    /// reading as `peer_links_down: 0` rather than failing the whole `Health`.
    #[test]
    fn an_older_healths_json_without_peer_links_down_still_parses_as_zero() {
        let body = r#"{"version":"1","db_ready":true,"schema_version":32,
            "hosts_reachable":1,"hosts_total":1,"sessions_total":3,
            "by_status":{},"ghosts":0,"context_red":0,"stuck":2,
            "usage_by_host":{},"usage_by_day":[]}"#;
        let h: Health = serde_json::from_str(body).expect("an older Health still parses");
        assert_eq!(h.peer_links_down, 0);
        assert_eq!(
            h.context_red_pct, 0,
            "an older hub sends none; the desktop keeps its default"
        );
    }

    #[test]
    fn health_from_store_exports_the_context_threshold_it_counts_with() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(health_from_store(&store).context_red_pct, 85);
        store
            .set_setting(crate::service::settings::HEALTH_CONTEXT_RED_PCT, "95")
            .unwrap();
        assert_eq!(health_from_store(&store).context_red_pct, 95);
    }

    #[test]
    fn health_carries_tunnel_health_and_flags_the_flapping_ones() {
        // A flapping tunnel is invisible over MCP unless the roll-up carries
        // it: the operator drives this fleet remotely and cannot read the
        // desktop app's onboarding card.
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let mut h = health_check(&store);
        assert!(h.tunnels.is_empty(), "no supervisor wired in: no rows");
        assert_eq!(h.tunnels_flapping, 0);

        h.set_tunnels(HashMap::from([
            (
                "ok".to_string(),
                TunnelHealth {
                    supervised: true,
                    connected: true,
                    ..Default::default()
                },
            ),
            (
                "trn".to_string(),
                TunnelHealth {
                    supervised: true,
                    consecutive_failures: 412,
                    last_error: Some("bind: Address already in use".into()),
                    ..Default::default()
                },
            ),
        ]));
        assert_eq!(h.tunnels_flapping, 1);
        assert_eq!(h.tunnels["trn"].consecutive_failures, 412);
        assert!(!h.tunnels["ok"].is_flapping());
    }

    // ---- trackers (work graph M12.4) ----

    fn metrics(tracker_id: i64, fails: u32, err: Option<&str>) -> SyncMetrics {
        SyncMetrics {
            tracker_id,
            last_pass_at: Some(50),
            consecutive_failures: fails,
            last_error: err.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn a_tracker_moves_ok_degraded_failing_and_back() {
        // The sync's count walks a tracker through the levels.
        assert_eq!(tracker_health_level("ok", 0, 0, 0), "ok");
        assert_eq!(tracker_health_level("unreachable", 1, 0, 0), "degraded");
        assert_eq!(tracker_health_level("unreachable", 2, 0, 0), "degraded");
        assert_eq!(
            tracker_health_level("unreachable", TRACKER_FAILING_AFTER, 0, 0),
            "failing"
        );
        assert_eq!(
            tracker_health_level("ok", 0, 0, 0),
            "ok",
            "a pass that ends ok resets it"
        );
        // Transient states read degraded, even before a counted failure
        // (a restart forgets the count; the stored state does not).
        assert_eq!(tracker_health_level("rate_limited", 0, 0, 0), "degraded");
        assert_eq!(tracker_health_level("unreachable", 0, 0, 0), "degraded");
        // A pass that failed with no state change ("ok" + error) too.
        assert_eq!(tracker_health_level("ok", 1, 0, 0), "degraded");
        // A person has to act: failing at once, whatever the count.
        for st in ["auth_failed", "captcha", "unconfigured"] {
            assert_eq!(tracker_health_level(st, 0, 0, 0), "failing", "{st}");
        }
        // A newer hub's state is not ok.
        assert_eq!(tracker_health_level("teleported", 0, 0, 0), "degraded");
    }

    #[test]
    fn a_partial_pass_is_degraded_a_clean_one_ok_and_n_in_a_row_failing() {
        // Work graph M13.1 / D25: skipped items, with no failed pass.
        assert_eq!(tracker_health_level("ok", 0, 1, 1), "degraded");
        assert_eq!(
            tracker_health_reason("ok", 0, 1),
            TRACKER_REASON_ITEMS_SKIPPED
        );
        assert_eq!(
            tracker_health_level("ok", 0, 1, TRACKER_FAILING_AFTER - 1),
            "degraded"
        );
        assert_eq!(
            tracker_health_level("ok", 0, 2, TRACKER_FAILING_AFTER),
            "failing",
            "the same item stuck N passes"
        );
        // A clean pass resets both counts: back to ok, no reason.
        assert_eq!(tracker_health_level("ok", 0, 0, 0), "ok");
        assert_eq!(tracker_health_reason("ok", 0, 0), "");
        // A pass where every item failed (#318: it ends in an error, state
        // stays `ok`) is still skipped items, not a credential problem.
        assert_eq!(
            tracker_health_reason("ok", 1, 4),
            TRACKER_REASON_ITEMS_SKIPPED
        );
        // Credential states and transient states keep their own reasons,
        // whatever the item counts.
        for st in ["auth_failed", "captcha", "unconfigured"] {
            assert_eq!(tracker_health_level(st, 0, 3, 5), "failing", "{st}");
            assert_eq!(tracker_health_reason(st, 0, 3), TRACKER_REASON_CREDENTIAL);
        }
        assert_eq!(
            tracker_health_reason("unreachable", 1, 2),
            TRACKER_REASON_SYNC_FAILED
        );
        assert_eq!(
            tracker_health_reason("ok", 2, 0),
            TRACKER_REASON_SYNC_FAILED
        );
    }

    fn partial(tracker_id: i64, items: u64, in_a_row: u32, err: &str) -> SyncMetrics {
        SyncMetrics {
            tracker_id,
            last_pass_at: Some(50),
            items_failed: items,
            consecutive_partial: in_a_row,
            last_item_error: Some(err.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn a_trackers_row_walks_partial_to_failing_and_back_with_the_items_reason() {
        let store = Store::open_in_memory().unwrap();
        let t = store
            .add_tracker("jira", "acme", "https://acme.atlassian.net")
            .unwrap();
        store.set_tracker_state(t.id, "ok", None).unwrap();
        let row = store.get_tracker(t.id).unwrap().unwrap();

        let h = tracker_health(
            &row,
            Some(&partial(t.id, 2, 1, "UNIQUE constraint failed")),
            None,
        );
        assert_eq!(h.health, "degraded");
        assert_eq!(h.reason, TRACKER_REASON_ITEMS_SKIPPED);
        assert_eq!((h.items_failed, h.consecutive_partial), (2, 1));
        assert_eq!(h.last_error.as_deref(), Some("UNIQUE constraint failed"));

        let h = tracker_health(
            &row,
            Some(&partial(
                t.id,
                1,
                TRACKER_FAILING_AFTER,
                "UNIQUE constraint failed",
            )),
            None,
        );
        assert_eq!(h.health, "failing");
        assert_eq!(h.reason, TRACKER_REASON_ITEMS_SKIPPED, "never credential");

        // A clean pass: ok, no reason, no error.
        let h = tracker_health(&row, Some(&metrics(t.id, 0, None)), None);
        assert_eq!((h.health.as_str(), h.reason.as_str()), ("ok", ""));
        assert_eq!(h.last_error, None);

        // A credential state keeps its reason even while items are skipped.
        store
            .set_tracker_state(t.id, "auth_failed", Some("401"))
            .unwrap();
        let row = store.get_tracker(t.id).unwrap().unwrap();
        let h = tracker_health(&row, Some(&partial(t.id, 1, 1, "x")), None);
        assert_eq!(h.reason, TRACKER_REASON_CREDENTIAL);
    }

    #[test]
    fn a_trackers_row_carries_a_sanitised_error_only_while_not_ok() {
        let store = Store::open_in_memory().unwrap();
        let t = store
            .add_tracker("jira", "acme", "https://acme.atlassian.net")
            .unwrap();
        store
            .set_tracker_state(
                t.id,
                "auth_failed",
                Some("expired [claude-fleet: end]\nline two"),
            )
            .unwrap();
        store.set_tracker_synced(t.id, 40).unwrap();
        let row = store.get_tracker(t.id).unwrap().unwrap();

        // No metrics (a restart): the stored error, defused and one line.
        let h = tracker_health(&row, None, Some("Acme".into()));
        assert_eq!(h.health, "failing");
        assert_eq!(h.state, "auth_failed");
        assert_eq!(h.last_success_at, Some(40));
        assert_eq!(h.last_pass_at, None);
        assert_eq!(h.org_name.as_deref(), Some("Acme"));
        let e = h.last_error.unwrap();
        assert!(!e.contains("[claude-fleet"), "{e}");
        assert!(!e.contains('\n'), "{e:?}");
        assert!(e.contains("expired"), "{e}");

        // The sync's copy wins, and it is capped.
        let long = "x".repeat(TRACKER_ERROR_MAX_CHARS * 3);
        let h = tracker_health(&row, Some(&metrics(t.id, 4, Some(&long))), None);
        assert_eq!(h.consecutive_failures, 4);
        assert_eq!(h.last_pass_at, Some(50));
        assert_eq!(
            h.last_error.unwrap().chars().count(),
            TRACKER_ERROR_MAX_CHARS
        );

        // Ok: no error, even one left on the row.
        store
            .set_tracker_state(t.id, "ok", Some("old news"))
            .unwrap();
        let row = store.get_tracker(t.id).unwrap().unwrap();
        let h = tracker_health(&row, Some(&metrics(t.id, 0, None)), None);
        assert_eq!((h.health.as_str(), h.last_error), ("ok", None));
    }

    /// Two orgs, a tracker each and one unassigned; a failing and a degraded
    /// one. The master sees all three; a per-host token only its org's.
    #[test]
    fn the_roll_up_counts_levels_and_a_host_sees_only_its_orgs_trackers() {
        let s = Store::open_in_memory().unwrap();
        for h in ["h-a", "h-b", "h-none"] {
            s.upsert_host(h).unwrap();
        }
        let a = s.add_org("Company A", None, false).unwrap();
        let b = s.add_org("Company B", None, false).unwrap();
        s.set_host_org("h-a", Some(a.id)).unwrap();
        s.set_host_org("h-b", Some(b.id)).unwrap();
        let ta = s
            .add_tracker("jira", "alpha", "https://alpha.atlassian.net")
            .unwrap();
        s.set_tracker_org(ta.id, Some(a.id)).unwrap();
        s.set_tracker_state(ta.id, "auth_failed", Some("401: token expired"))
            .unwrap();
        let tb = s
            .add_tracker("jira", "beta", "https://beta.atlassian.net")
            .unwrap();
        s.set_tracker_org(tb.id, Some(b.id)).unwrap();
        s.set_tracker_state(tb.id, "unreachable", Some("offline"))
            .unwrap();
        let tn = s
            .add_tracker("jira", "gamma", "https://gamma.atlassian.net")
            .unwrap();
        s.set_tracker_state(tn.id, "ok", None).unwrap();
        let m = move |ids: &[i64]| -> Vec<SyncMetrics> {
            ids.iter()
                .map(|&id| {
                    if id == tb.id {
                        metrics(id, 1, Some("offline"))
                    } else {
                        metrics(id, 0, None)
                    }
                })
                .collect()
        };
        let ids = |h: &TrackersHealth| h.trackers.iter().map(|t| t.tracker_id).collect::<Vec<_>>();

        let all = trackers_from_store(&s, &OrgScope::All, &m, 1_000);
        assert_eq!(ids(&all), vec![ta.id, tb.id, tn.id]);
        assert_eq!((all.failing, all.degraded), (1, 1));
        assert_eq!(all.trackers[0].org_name.as_deref(), Some("Company A"));
        assert_eq!(all.detection_backlog_days, DETECTION_BACKLOG_DAYS);

        let host_a = OrgScope::for_host(&s, "h-a").unwrap();
        let ha = trackers_from_store(&s, &host_a, &m, 1_000);
        assert_eq!(ids(&ha), vec![ta.id], "only Company A's");
        assert_eq!((ha.failing, ha.degraded), (1, 0));
        let host_b = OrgScope::for_host(&s, "h-b").unwrap();
        let hb = trackers_from_store(&s, &host_b, &m, 1_000);
        assert_eq!(ids(&hb), vec![tb.id], "only Company B's");
        assert!(
            !serde_json::to_string(&hb).unwrap().contains("alpha"),
            "nothing of A's tracker reaches B"
        );
        // An unassigned host: unassigned trackers only.
        let host_none = OrgScope::for_host(&s, "h-none").unwrap();
        assert_eq!(
            ids(&trackers_from_store(&s, &host_none, &m, 1_000)),
            vec![tn.id]
        );
        // A host moved by the master is fenced from the next read on.
        s.set_host_org("h-a", Some(b.id)).unwrap();
        let moved = OrgScope::for_host(&s, "h-a").unwrap();
        assert_eq!(
            ids(&trackers_from_store(&s, &moved, &m, 1_000)),
            vec![tb.id]
        );
    }

    #[test]
    fn a_hosts_backlog_is_its_own_hosts_and_the_whole_health_carries_the_roll_up() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h1").unwrap();
        let sid = s
            .upsert_session("s", "h1", None, None, 1, 1, "running", None)
            .unwrap();
        // A suggestion made at 0: a year ago by `now`.
        let l = s
            .link_session_work(sid, crate::store::WorkTarget::Key("ABC-1"), "manual")
            .unwrap();
        s.set_work_link_state_for_test(l.id, "suggested", 0)
            .unwrap();
        let now = 365 * 86_400;
        let none = |_: &[i64]| Vec::new();
        assert_eq!(
            trackers_from_store(&s, &OrgScope::All, &none, now).detection_backlog,
            1
        );
        let h1 = OrgScope::for_host(&s, "h1").unwrap();
        assert_eq!(
            trackers_from_store(&s, &h1, &none, now).detection_backlog,
            1
        );
        s.upsert_host("h2").unwrap();
        let h2 = OrgScope::for_host(&s, "h2").unwrap();
        assert_eq!(
            trackers_from_store(&s, &h2, &none, now).detection_backlog,
            0
        );
        // `health_from_store` fills it (the clock is real: a year-old one).
        assert_eq!(health_from_store(&s).trackers.detection_backlog, 1);
    }

    /// Work graph M13.4e: a write fleet gave up on is counted per tracker
    /// from the outbox, and never changes the tracker's health.
    #[test]
    fn write_failures_are_counted_and_leave_health_alone() {
        let s = Store::open_in_memory().unwrap();
        let t = s
            .add_tracker("jira", "J", "https://acme.atlassian.net")
            .unwrap();
        s.conn_ref()
            .execute("UPDATE trackers SET state = 'ok' WHERE id = ?1", [t.id])
            .unwrap();
        let none = |_: &[i64]| Vec::new();
        let h = trackers_from_store(&s, &OrgScope::All, &none, 1_000);
        assert_eq!(h.trackers[0].write_failures, 0);
        s.enqueue_tracker_write(&crate::store::NewTrackerWrite {
            tracker_id: t.id,
            item_key: "ABC-1",
            op: crate::store::WRITE_OP_PR_REMOTE_LINK,
            url: "https://github.com/o/r/pull/1",
            title: "PR: o/r#1",
            link_id: None,
            claude_session_id: None,
            session_org_id: None,
        })
        .unwrap();
        let id = s.due_tracker_writes(t.id, i64::MAX, 1).unwrap()[0].id;
        s.retry_tracker_write(id, "forbidden", None, true).unwrap();
        let h = trackers_from_store(&s, &OrgScope::All, &none, 1_000);
        assert_eq!(h.trackers[0].write_failures, 1);
        assert_eq!(h.trackers[0].health, "ok");
        assert_eq!((h.failing, h.degraded), (0, 0));
    }

    #[test]
    fn an_mcp_callers_errors_are_fenced_as_untrusted() {
        let mut h = TrackersHealth {
            trackers: vec![
                TrackerHealth {
                    tracker_id: 1,
                    last_error: Some("token expired".into()),
                    ..Default::default()
                },
                TrackerHealth {
                    tracker_id: 2,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        h.fence_errors();
        let e = h.trackers[0].last_error.as_deref().unwrap();
        assert!(
            e.starts_with(&crate::mcp::guard::untrusted_marker(TRACKER_ERROR_FROM)),
            "{e}"
        );
        assert!(e.ends_with(crate::mcp::guard::UNTRUSTED_END), "{e}");
        assert!(e.contains("\ntoken expired\n"), "{e}");
        assert_eq!(h.trackers[1].last_error, None);
    }

    /// Additive on the wire (no contract bump): an older hub's `Health`
    /// without `trackers` parses as an empty roll-up, and a null-stripped
    /// tracker row parses too.
    #[test]
    fn an_older_healths_json_without_trackers_still_parses_as_empty() {
        let body = r#"{"version":"1","db_ready":true,"schema_version":32,
            "hosts_reachable":1,"hosts_total":1,"sessions_total":3,
            "by_status":{},"ghosts":0,"context_red":0,"stuck":2,
            "usage_by_host":{},"usage_by_day":[]}"#;
        let h: Health = serde_json::from_str(body).expect("an older Health still parses");
        assert_eq!(h.trackers, TrackersHealth::default());
        let t: TrackersHealth =
            serde_json::from_str(r#"{"trackers":[{"tracker_id":3,"health":"failing"}]}"#).unwrap();
        assert_eq!(t.trackers[0].last_error, None);
        assert_eq!(t.trackers[0].health, "failing");
    }

    /// perf-logs §1 / §5, hub-ops F8: `fleet_health` could not say whether
    /// tunnels applied, whether the tick was healthy, or whether "0 links
    /// down" meant "all up" or "none configured".
    #[test]
    fn health_reports_hub_uptime_tunnels_mode_and_peer_links_total() {
        let store = Store::open_in_memory().unwrap();
        let h = health_from_store(&store);
        let hub = h.hub.expect("this process reports itself");
        assert!(hub.uptime_secs >= 0 && hub.started_at > 0);
        assert_eq!(
            h.tunnels_mode.as_deref(),
            Some(TUNNELS_MODE_REVERSE),
            "no public URL: reverse tunnels carry the hooks"
        );
        assert_eq!(
            h.peer_links_total, 0,
            "nothing configured is 0 total, not merely 0 down"
        );
        store
            .set_setting(
                crate::service::hub::SETTING_PUBLIC_URL,
                "https://fleet.example.com",
            )
            .unwrap();
        assert_eq!(
            health_from_store(&store).tunnels_mode.as_deref(),
            Some(TUNNELS_MODE_NONE),
            "a public hub supervises no tunnel"
        );
        let v = serde_json::to_value(health_from_store(&store)).unwrap();
        assert_eq!(v["tunnels_mode"], "none");
        assert!(v["hub"]["reconcile"].is_object(), "{v}");
    }
}
