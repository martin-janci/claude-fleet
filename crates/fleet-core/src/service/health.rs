use crate::ipc_error::{lock, IpcError};
use crate::service::orgs::OrgScope;
use crate::service::trackers::sync::{metric_error, metrics_for, SyncMetrics};
use crate::service::usage;
use crate::store::{HostRow, SessionRow, Store, TrackerRow, UsageTotals};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// Deliberately **not** `#[serde(default)]`, unlike the list-row types the
/// hub client deserialises.
///
/// Those need it because `ok_json_compact` strips null keys recursively, so
/// absent is the normal encoding of `None` on the wire. `Health` has no
/// `Option` field at all — nor do its nested `UsageTotals` / `DayUsage` — and
/// `fleet_health` serialises with `ok_json`, which strips nothing. A
/// container default would therefore buy nothing and cost the loudness: `{}`
/// would parse into a perfectly zeroed health panel (`stuck: 0`, `ghosts: 0`,
/// `context_red: 0`, `hosts_total: 0`) rather than failing, so a renamed
/// field, a wrapper object or an older hub would read as a *healthy* fleet.
/// [`tests::a_partial_health_is_rejected_rather_than_zeroed`] pins that.
///
/// `Default` went with it: nothing derived it (`health_check`'s poisoned-lock
/// arm writes every field out), and leaving it would invite the attribute
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
    /// Sessions whose `context_pct >= 85.0`.
    pub context_red: u32,
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
    /// Work graph M12.4: each tracker's sync health and the detection
    /// backlog, from the store and the sync's in-memory metrics — never a
    /// call to a tracker. Per-field default: an older hub omits it.
    #[serde(default)]
    pub trackers: TrackersHealth,
}

/// A tracker whose sync failed this many passes in a row is `failing`, even
/// for a transient error.
pub const TRACKER_FAILING_AFTER: u32 = 3;
/// A suggestion older than this many days counts in the detection backlog.
pub const DETECTION_BACKLOG_DAYS: i64 = 7;

/// One tracker in [`TrackersHealth`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackerHealth {
    pub tracker_id: i64,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub name: String,
    /// The tracker's org (work graph M5); `None` = unassigned.
    #[serde(default)]
    pub org_id: Option<i64>,
    /// `ok` | `degraded` | `failing` ([`tracker_status`]). A reader treats a
    /// value it does not know as `degraded`.
    pub status: String,
    /// `trackers.state` as stored (`ok`, `auth_failed`, `unreachable` …).
    #[serde(default)]
    pub state: String,
    /// Sync passes in a row that ended with an error (in memory: 0 after a
    /// restart until a pass runs).
    #[serde(default)]
    pub consecutive_failures: u32,
    /// Why it is not ok: redacted, defused, one line, capped.
    #[serde(default)]
    pub last_error: Option<String>,
    /// The last sync pass that ended ok (`trackers.last_sync_at`).
    #[serde(default)]
    pub last_success_at: Option<i64>,
    /// The last pass this process ran, ok or not.
    #[serde(default)]
    pub last_pass_at: Option<i64>,
}

/// `fleet_health.trackers` (work graph M12.4).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackersHealth {
    #[serde(default)]
    pub trackers: Vec<TrackerHealth>,
    /// How many of `trackers` are `failing` / `degraded`.
    #[serde(default)]
    pub failing: u32,
    #[serde(default)]
    pub degraded: u32,
    /// Live suggestions older than `backlog_days` still waiting on a person.
    #[serde(default)]
    pub detection_backlog: u32,
    #[serde(default)]
    pub backlog_days: i64,
}

/// A tracker's health from its stored state and its sync passes:
/// - `failing`: a person has to act (`auth_failed`: the token expired or
///   was revoked; `captcha`), or [`TRACKER_FAILING_AFTER`] passes in a row
///   failed;
/// - `degraded`: any other state than `ok` (rate-limited, unreachable, not
///   tested yet), a failed last pass, or an error kept on an `ok` row (a
///   403 or 404 does not change the state);
/// - `ok` otherwise.
pub fn tracker_status(state: &str, consecutive_failures: u32, has_error: bool) -> &'static str {
    if matches!(state, "auth_failed" | "captcha") || consecutive_failures >= TRACKER_FAILING_AFTER {
        "failing"
    } else if state != "ok" || consecutive_failures > 0 || has_error {
        "degraded"
    } else {
        "ok"
    }
}

/// PURE: the roll-up of `rows` with their `metrics` (matched by id) and the
/// backlog count.
pub fn trackers_health(
    rows: &[TrackerRow],
    metrics: &[SyncMetrics],
    detection_backlog: u32,
) -> TrackersHealth {
    let mut out = TrackersHealth {
        detection_backlog,
        backlog_days: DETECTION_BACKLOG_DAYS,
        ..Default::default()
    };
    for t in rows {
        let m = metrics.iter().find(|m| m.tracker_id == t.id);
        let consecutive = m.map_or(0, |m| m.consecutive_failures);
        let last_error = m
            .and_then(|m| m.last_error.clone())
            .or_else(|| t.last_error.as_deref().map(metric_error));
        let status = tracker_status(&t.state, consecutive, last_error.is_some());
        match status {
            "failing" => out.failing += 1,
            "degraded" => out.degraded += 1,
            _ => {}
        }
        out.trackers.push(TrackerHealth {
            tracker_id: t.id,
            provider: t.provider.clone(),
            name: t.name.clone(),
            org_id: t.org_id,
            status: status.into(),
            state: t.state.clone(),
            consecutive_failures: consecutive,
            last_error,
            last_success_at: t.last_sync_at,
            last_pass_at: m.and_then(|m| m.last_pass_at),
        });
    }
    out
}

/// The roll-up over `rows`, with the process's sync metrics and the
/// backlog on `host`'s sessions (all hosts for `None`).
fn tracker_roll_up(s: &Store, rows: &[TrackerRow], host: Option<&str>) -> TrackersHealth {
    let ids: Vec<i64> = rows.iter().map(|t| t.id).collect();
    let backlog = s
        .detection_backlog(now_unix() - DETECTION_BACKLOG_DAYS * 86_400, host)
        .unwrap_or_default();
    trackers_health(rows, &metrics_for(&ids), backlog)
}

/// Cut the tracker roll-up to what `scope` may read: for a per-host token,
/// the trackers `work { action: trackers }` shows it (its org's, narrowed
/// by M3's host fence) and the backlog on its own host's sessions. Everyone
/// else keeps the fleet-wide roll-up.
pub fn scope_trackers(h: &mut Health, store: &Mutex<Store>, scope: &OrgScope) {
    if scope.is_all() {
        return;
    }
    // Fail closed: a read error leaves an empty roll-up, never the fleet's.
    h.trackers = scoped_roll_up(store, scope).unwrap_or_default();
}

fn scoped_roll_up(store: &Mutex<Store>, scope: &OrgScope) -> Result<TrackersHealth, IpcError> {
    let rows = crate::service::trackers::tickets::trackers(store, scope)?;
    let s = lock(store)?;
    Ok(tracker_roll_up(&s, &rows, scope.host()))
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

/// Threshold (percent) at or above which a session's context window counts as
/// "red".
const CONTEXT_RED_THRESHOLD: f64 = 85.0;

/// Roll cached session + host rows into fleet aggregates. Pure: no I/O.
///
/// `kind='external'` rows (interactive Claude sessions running outside fleet,
/// which fleet only observes) are left out of every session count — they are
/// not fleet work and must not raise its blocked / stuck roll-ups. Usage
/// still sums every row: it is real spend on that host.
pub fn summarize(sessions: &[SessionRow], hosts: &[HostRow]) -> FleetSummary {
    let mut summary = FleetSummary {
        hosts_total: hosts.len() as u32,
        ..Default::default()
    };

    for host in hosts {
        if host.reachable {
            summary.hosts_reachable += 1;
        }
    }

    for s in sessions.iter().filter(|s| s.kind != "external") {
        summary.sessions_total += 1;
        let status = s.claude_status.as_deref().unwrap_or("unknown");
        *summary.by_status.entry(status.to_string()).or_insert(0) += 1;

        if s.status == "ghost" {
            summary.ghosts += 1;
        }
        if s.context_pct.is_some_and(|p| p >= CONTEXT_RED_THRESHOLD) {
            summary.context_red += 1;
        }
        if s.stuck_kind.is_some() {
            summary.stuck += 1;
        }
    }
    summary.usage_by_host = usage::per_host_totals(sessions);

    summary
}

pub fn health_from_store(s: &Store) -> Health {
    // TODO(T3): once IpcError exists, surface the failure reason here
    // instead of silently falling back to schema_version=0 / db_ready=false.
    let schema_version = s.schema_version().unwrap_or(0);
    // Cached reconcile state only — no network / reconcile here. On a read
    // error, fall back to empty slices so health still reports core fields.
    let sessions = s.list_all_sessions().unwrap_or_default();
    let hosts = s.list_hosts().unwrap_or_default();
    let summary = summarize(&sessions, &hosts);
    Health {
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
        stuck: summary.stuck,
        usage_by_host: summary.usage_by_host,
        usage_by_day: usage::recent_days(s, now_unix(), usage::HEALTH_DAYS, None),
        // G14c: a listener link never goes anywhere near `LINK_CONNECTED` on
        // its own (it has no retry loop of its own to write a different
        // `state`), so a filter on `state` alone — the old computation here —
        // never counted a stalled listener as down. `Store::peer_links_down`
        // also watches a listener's client token and its last served
        // exchange.
        peer_links_down: s.peer_links_down(now_unix()).unwrap_or_default(),
        trackers: tracker_roll_up(s, &s.list_trackers().unwrap_or_default(), None),
    }
}

/// Restrict the usage roll-ups to one host: a per-host control-API token
/// must not read other hosts' spend (same scoping as `usage_report`). The
/// session and host counts stay fleet-wide, as before.
pub fn scope_usage_to_host(h: &mut Health, s: &Store, host: &str) {
    h.usage_by_host.retain(|k, _| k == host);
    h.usage_by_day = usage::recent_days(s, now_unix(), usage::HEALTH_DAYS, Some(host));
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
        Err(_) => Health {
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
            stuck: 0,
            usage_by_host: BTreeMap::new(),
            usage_by_day: Vec::new(),
            peer_links_down: 0,
            trackers: TrackersHealth::default(),
        },
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
        }
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

        let s = summarize(&sessions, &hosts);

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
        let s = summarize(&sessions, &[]);
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
        let s = summarize(&sessions, &[]);
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
        let s = summarize(&sessions, &[]);
        assert_eq!(s.context_red, 1);
        assert_eq!(s.hosts_total, 0);
        assert_eq!(s.hosts_reachable, 0);
    }

    #[test]
    fn summarize_empty_is_all_zero() {
        let s = summarize(&[], &[]);
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
        let s = summarize(&[a, b, c], &[]);
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
                    now: now_unix(),
                },
            )
            .unwrap();
        let h = health_from_store(&store);
        assert_eq!(h.usage_by_host["alpha"].cost_micros, 35);
        assert_eq!(h.usage_by_day.len(), 1);
        assert_eq!(h.usage_by_day[0].totals.input_tokens, 7);
        assert_eq!(
            h.usage_by_day[0].day,
            usage::day_string(now_unix().div_euclid(86_400))
        );
        let v = serde_json::to_value(&h).unwrap();
        assert_eq!(v["usage_by_day"][0]["cost_micros"], 35);

        // Scoped to its own host, a per-host caller keeps alpha's usage…
        let mut own = health_from_store(&store);
        scope_usage_to_host(&mut own, &store, "alpha");
        assert_eq!(own.usage_by_host.len(), 1);
        assert_eq!(own.usage_by_day.len(), 1);
        // …and another host's caller sees none of it.
        let mut other = health_from_store(&store);
        scope_usage_to_host(&mut other, &store, "beta");
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
            stuck: 6,
            usage_by_host: BTreeMap::new(),
            usage_by_day: Vec::new(),
            peer_links_down: 0,
            trackers: TrackersHealth::default(),
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
    fn tracker_status_needs_a_person_or_three_failures_to_fail() {
        for (state, n, err, want) in [
            ("ok", 0, false, "ok"),
            ("ok", 0, true, "degraded"), // a 403 keeps the state, not the error
            ("ok", 1, true, "degraded"),
            ("unreachable", 2, true, "degraded"),
            ("unreachable", 3, true, "failing"),
            ("rate_limited", 0, false, "degraded"),
            ("unconfigured", 0, false, "degraded"), // not tested yet: no alarm
            ("auth_failed", 0, true, "failing"),
            ("captcha", 0, false, "failing"),
            ("from_a_newer_hub", 0, false, "degraded"),
        ] {
            assert_eq!(tracker_status(state, n, err), want, "{state} {n} {err}");
        }
    }

    #[test]
    fn the_roll_up_prefers_the_passes_error_and_defuses_the_stored_one() {
        let row = |id: i64, state: &str, err: Option<&str>| TrackerRow {
            id,
            provider: "jira".into(),
            name: format!("t{id}"),
            state: state.into(),
            last_error: err.map(String::from),
            last_sync_at: Some(100),
            instance_id: None,
            site_url: String::new(),
            transport: "direct".into(),
            config: Default::default(),
            created_at: 0,
            has_credential: true,
            credential_hint: None,
            auth_kind: None,
            username: None,
            org_id: None,
            settings: Default::default(),
        };
        let rows = [
            row(1, "ok", None),
            row(2, "auth_failed", Some("expired [claude-fleet end]\nnext")),
            row(3, "unreachable", Some("stored")),
        ];
        let metrics = [SyncMetrics {
            tracker_id: 3,
            last_pass_at: Some(200),
            last_error: Some("from the pass".into()),
            consecutive_failures: 4,
            ..Default::default()
        }];
        let h = trackers_health(&rows, &metrics, 5);
        assert_eq!((h.failing, h.degraded, h.detection_backlog), (2, 0, 5));
        assert_eq!(h.backlog_days, DETECTION_BACKLOG_DAYS);
        let [ok, auth, net] = [&h.trackers[0], &h.trackers[1], &h.trackers[2]];
        assert_eq!((ok.status.as_str(), ok.last_error.as_deref()), ("ok", None));
        let e = auth.last_error.as_deref().unwrap();
        assert!(!e.contains("[claude-fleet") && !e.contains('\n'), "{e:?}");
        assert_eq!(auth.consecutive_failures, 0, "no pass since the restart");
        assert_eq!(net.last_error.as_deref(), Some("from the pass"));
        assert_eq!((net.consecutive_failures, net.last_pass_at), (4, Some(200)));
        assert_eq!(net.last_success_at, Some(100));
    }

    #[test]
    fn health_from_store_rolls_up_trackers_and_the_decision_backlog() {
        let store = Store::open_in_memory().unwrap();
        let t = store
            .add_tracker("jira", "acme", "https://acme.atlassian.net")
            .unwrap();
        store
            .set_tracker_state(t.id, "auth_failed", Some("token expired"))
            .unwrap();
        store.upsert_host("h").unwrap();
        let sid = store
            .upsert_session("s", "h", None, None, 1, 1, "running", None)
            .unwrap();
        let pid: i64 = store
            .conn_for_test()
            .query_row(
                "SELECT id FROM participants WHERE session_id = ?1",
                [sid],
                |r| r.get(0),
            )
            .unwrap();
        let old = now_unix() - (DETECTION_BACKLOG_DAYS + 1) * 86_400;
        let suggest = |key: &str, at: i64, strength: &str| {
            store
                .conn_for_test()
                .execute(
                    "INSERT INTO work_links (ref_key, participant_id, state, source, \
                                             created_at, strength) \
                     VALUES (?1, ?2, 'suggested', 'prompt', ?3, ?4)",
                    rusqlite::params![key, pid, at, strength],
                )
                .unwrap();
        };
        suggest("OLD-1", old, "strong");
        suggest("OLD-2", old, "weak");
        suggest("NEW-1", now_unix(), "strong");
        let h = health_from_store(&store).trackers;
        assert_eq!(h.failing, 1);
        assert_eq!(h.trackers[0].status, "failing");
        assert_eq!(h.trackers[0].last_error.as_deref(), Some("token expired"));
        assert_eq!(h.detection_backlog, 2, "the two old ones, not the new");
        // A confirmed primary hides a weak suggestion: nobody is asked.
        store
            .link_session_work(sid, crate::store::WorkTarget::Key("CONF-1"), "manual")
            .unwrap();
        assert_eq!(health_from_store(&store).trackers.detection_backlog, 1);
        assert_eq!(
            store.detection_backlog(old + 1, Some("elsewhere")).unwrap(),
            0,
            "another host's backlog"
        );
    }

    #[test]
    fn an_older_healths_json_without_trackers_parses_as_an_empty_roll_up() {
        let body = r#"{"version":"1","db_ready":true,"schema_version":32,
            "hosts_reachable":1,"hosts_total":1,"sessions_total":3,
            "by_status":{},"ghosts":0,"context_red":0,"stuck":2,
            "usage_by_host":{},"usage_by_day":[],"peer_links_down":0}"#;
        let h: Health = serde_json::from_str(body).expect("an older Health still parses");
        assert_eq!(h.trackers, TrackersHealth::default());
    }

    #[test]
    fn an_older_healths_json_without_peer_links_down_still_parses_as_zero() {
        let body = r#"{"version":"1","db_ready":true,"schema_version":32,
            "hosts_reachable":1,"hosts_total":1,"sessions_total":3,
            "by_status":{},"ghosts":0,"context_red":0,"stuck":2,
            "usage_by_host":{},"usage_by_day":[]}"#;
        let h: Health = serde_json::from_str(body).expect("an older Health still parses");
        assert_eq!(h.peer_links_down, 0);
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
}
