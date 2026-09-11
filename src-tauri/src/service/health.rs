use crate::service::usage;
use crate::store::{HostRow, SessionRow, Store, UsageTotals};
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Serialize)]
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
    /// Sessions with `claude_status == "ghost"`.
    pub ghosts: u32,
    /// Sessions whose `context_pct >= 85.0`.
    pub context_red: u32,
    /// Sessions with a `stuck_kind` set.
    pub stuck: u32,
    /// Estimated token usage and cost (micro-USD) per host, summed over the
    /// sessions currently in the store. Hosts with nothing counted are
    /// omitted.
    pub usage_by_host: BTreeMap<String, UsageTotals>,
    /// Estimated usage per UTC day (`day` = `YYYY-MM-DD`), all hosts, over
    /// the last `usage::HEALTH_DAYS` days — from the durable daily roll-up,
    /// so killed sessions still count.
    pub usage_by_day: Vec<usage::DayUsage>,
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

/// Threshold (percent) at or above which a session's context window counts as
/// "red".
const CONTEXT_RED_THRESHOLD: f64 = 85.0;

/// Roll cached session + host rows into fleet aggregates. Pure: no I/O.
pub fn summarize(sessions: &[SessionRow], hosts: &[HostRow]) -> FleetSummary {
    let mut summary = FleetSummary {
        hosts_total: hosts.len() as u32,
        sessions_total: sessions.len() as u32,
        ..Default::default()
    };

    for host in hosts {
        if host.reachable {
            summary.hosts_reachable += 1;
        }
    }

    for s in sessions {
        let status = s.claude_status.as_deref().unwrap_or("unknown");
        *summary.by_status.entry(status.to_string()).or_insert(0) += 1;

        if status == "ghost" {
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
        version: env!("CARGO_PKG_VERSION").to_string(),
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
            version: env!("CARGO_PKG_VERSION").to_string(),
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
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn session(
        claude_status: Option<&str>,
        context_pct: Option<f64>,
        stuck_kind: Option<&str>,
    ) -> SessionRow {
        SessionRow {
            id: 0,
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
        }
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
        }
    }

    #[test]
    fn summarize_rolls_up_statuses_ghosts_context_and_stuck() {
        let sessions = vec![
            // null status → "unknown" bucket
            session(None, None, None),
            // working
            session(Some("working"), Some(10.0), None),
            // a ghost
            session(Some("ghost"), None, None),
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
        assert_eq!(s.by_status.get("ghost"), Some(&1));
        assert_eq!(s.by_status.get("idle"), Some(&1));

        assert_eq!(s.ghosts, 1);
        assert_eq!(s.context_red, 1);
        assert_eq!(s.stuck, 1);
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
        assert_eq!(h.version, env!("CARGO_PKG_VERSION"));
        assert!(h.db_ready);
        assert_eq!(h.schema_version, 25);
        // Empty store → empty roll-up.
        assert_eq!(h.sessions_total, 0);
        assert_eq!(h.hosts_total, 0);
        assert!(h.by_status.is_empty());
    }
}
