//! Session GC (PROD-1): a Settings-driven sweeper that kills sessions idle
//! longer than their kind's TTL. Runs from the background tick every
//! `gc.sweep_interval_secs`; opt-in via `gc.enabled` (default off).
//!
//! Idle reference per kind (see migration 019 `idle_since`):
//! - `bg`: `idle_since` (claude_status ∈ idle/completed/stopped), falling
//!   back to tmux/agent `last_activity_at` when the agent never reported a
//!   status
//! - `shell`: `last_activity_at` (tmux session_activity)
//! - `work`/`review`: `idle_since` only — a session Claude is still driving
//!   is never touched
//!
//! A work session past its TTL whose worktree is dirty (or whose inspection
//! failed) goes through the existing safe-remove flow (Claude is asked to
//! commit + push and the Stop hook finishes the job); a clean one is plainly
//! killed. Every action writes a `gc_killed` timeline event first; the kill
//! paths emit the row events. The planner is pure and the executor injectable.

use crate::ipc_error::IpcError;
use crate::service::safe_kill::SafeKillInspection;
use crate::service::settings;
use crate::ssh::SshClient;
use crate::store::{SessionRow, Store};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GcConfig {
    pub enabled: bool,
    pub bg_idle_secs: u64,
    pub shell_idle_secs: u64,
    pub work_idle_secs: u64,
    pub sweep_interval_secs: u64,
}

impl GcConfig {
    pub fn from_store(s: &Store) -> Self {
        Self {
            enabled: settings::get_bool(s, settings::GC_ENABLED),
            bg_idle_secs: settings::get_secs(s, settings::GC_BG_IDLE_SECS),
            shell_idle_secs: settings::get_secs(s, settings::GC_SHELL_IDLE_SECS),
            work_idle_secs: settings::get_secs(s, settings::GC_WORK_IDLE_SECS),
            sweep_interval_secs: settings::get_secs(s, settings::GC_SWEEP_INTERVAL_SECS),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcAction {
    /// `kill_session` (tmux kill or `claude stop` for bg rows).
    Kill,
    /// Inspect the worktree; dirty ⇒ safe-remove via Claude, clean ⇒ kill.
    InspectThenKill,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub session_id: i64,
    pub host_alias: String,
    pub tmux_name: String,
    pub kind: String,
    pub idle_secs: i64,
    pub action: GcAction,
}

/// The TTL (seconds) that applies to a row's kind; `0` ⇒ never collected.
fn ttl_for(kind: &str, cfg: &GcConfig) -> u64 {
    match kind {
        "bg" => cfg.bg_idle_secs,
        "shell" => cfg.shell_idle_secs,
        _ => cfg.work_idle_secs,
    }
}

/// The unix second the row has been idle since, per the kind rules above.
fn idle_reference(row: &SessionRow) -> Option<i64> {
    match row.kind.as_str() {
        "shell" => Some(row.last_activity_at),
        "bg" => row.idle_since.or(if row.claude_status.is_none() {
            Some(row.last_activity_at)
        } else {
            None
        }),
        _ => row.idle_since,
    }
}

/// Pure: which rows are past their TTL this sweep. Skips ghosts, rows with a
/// safe-kill in flight, the registered controller, and hosts that are not
/// reachable (a kill would fail and an offline host's idle stamps are stale).
pub fn plan(
    rows: &[SessionRow],
    cfg: &GcConfig,
    controller: Option<&(String, String)>,
    reachable_hosts: &HashSet<String>,
    now: i64,
) -> Vec<Planned> {
    if !cfg.enabled {
        return Vec::new();
    }
    let mut out = Vec::new();
    for r in rows {
        if r.status != "running" || r.safe_kill_state.is_some() {
            continue;
        }
        if !reachable_hosts.contains(&r.host_alias) {
            continue;
        }
        if controller
            .map(|(h, t)| h == &r.host_alias && t == &r.tmux_name)
            .unwrap_or(false)
        {
            continue;
        }
        let ttl = ttl_for(&r.kind, cfg);
        if ttl == 0 {
            continue;
        }
        let Some(since) = idle_reference(r) else {
            continue;
        };
        let idle_secs = now - since;
        if idle_secs < ttl as i64 {
            continue;
        }
        let action = match r.kind.as_str() {
            "bg" | "shell" => GcAction::Kill,
            _ => GcAction::InspectThenKill,
        };
        out.push(Planned {
            session_id: r.id,
            host_alias: r.host_alias.clone(),
            tmux_name: r.tmux_name.clone(),
            kind: r.kind.clone(),
            idle_secs,
            action,
        });
    }
    out
}

/// Side effects the sweeper performs, injected for tests.
#[async_trait::async_trait]
pub trait GcExec: Send + Sync {
    async fn inspect(
        &self,
        host_alias: &str,
        tmux_name: &str,
    ) -> Result<SafeKillInspection, IpcError>;
    async fn safe_kill(&self, host_alias: &str, tmux_name: &str) -> Result<(), IpcError>;
    async fn kill(&self, host_alias: &str, tmux_name: &str) -> Result<(), IpcError>;
}

/// Production executor over the existing service paths.
pub struct RealGcExec {
    pub store: Arc<Mutex<Store>>,
    pub ssh: Arc<SshClient>,
}

#[async_trait::async_trait]
impl GcExec for RealGcExec {
    async fn inspect(
        &self,
        host_alias: &str,
        tmux_name: &str,
    ) -> Result<SafeKillInspection, IpcError> {
        crate::service::safe_kill::inspect_safe_kill(
            crate::service::safe_kill::InspectSafeKillArgs {
                host_alias: host_alias.to_string(),
                tmux_name: tmux_name.to_string(),
            },
            &self.store,
            &self.ssh,
        )
        .await
    }

    async fn safe_kill(&self, host_alias: &str, tmux_name: &str) -> Result<(), IpcError> {
        crate::service::safe_kill::safe_kill_session(
            crate::service::safe_kill::SafeKillSessionArgs {
                host_alias: host_alias.to_string(),
                tmux_name: tmux_name.to_string(),
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map(|_| ())
    }

    async fn kill(&self, host_alias: &str, tmux_name: &str) -> Result<(), IpcError> {
        crate::service::sessions::kill_session(
            crate::service::sessions::KillSessionArgs {
                host_alias: host_alias.to_string(),
                name: tmux_name.to_string(),
                force: false,
            },
            &self.store,
            &self.ssh,
        )
        .await
        .map(|_| ())
    }
}

/// Pure: is the inspected worktree something we must not discard silently?
pub fn needs_safe_remove(insp: &SafeKillInspection) -> bool {
    insp.error.is_some()
        || !insp.dirty_files.is_empty()
        || insp.unpushed_commits != 0
        || (insp.has_worktree && insp.upstream.is_none())
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GcReport {
    pub killed: usize,
    pub safe_kill_requested: usize,
    pub failed: usize,
}

/// Run one sweep against `exec`. Reads rows/hosts/controller under one brief
/// lock, then acts off-lock. The `gc_killed` timeline entry is written before
/// each action (a plain kill reaps the row, and its events, moments later).
pub async fn sweep_with(
    store: &Mutex<Store>,
    exec: &dyn GcExec,
    cfg: &GcConfig,
    now: i64,
) -> GcReport {
    let mut report = GcReport::default();
    if !cfg.enabled {
        return report;
    }
    let (rows, controller, reachable) = {
        let Ok(s) = store.lock() else {
            return report;
        };
        let rows = s.list_all_sessions().unwrap_or_default();
        let controller = s.get_controller().ok().flatten();
        let reachable: HashSet<String> = s
            .list_hosts()
            .unwrap_or_default()
            .into_iter()
            .filter(|h| h.reachable)
            .map(|h| h.alias)
            .collect();
        (rows, controller, reachable)
    };
    for p in plan(&rows, cfg, controller.as_ref(), &reachable, now) {
        let via_claude = match p.action {
            GcAction::Kill => false,
            GcAction::InspectThenKill => match exec.inspect(&p.host_alias, &p.tmux_name).await {
                Ok(insp) => needs_safe_remove(&insp),
                Err(e) => {
                    eprintln!(
                        "[gc] inspect {}/{} failed ({e}); using safe-remove",
                        p.host_alias, p.tmux_name
                    );
                    true
                }
            },
        };
        let detail = format!(
            "{}:{}:idle_{}s",
            p.kind,
            if via_claude { "safe_kill" } else { "kill" },
            p.idle_secs
        );
        if let Ok(s) = store.lock() {
            if let Err(e) = s.insert_session_event(p.session_id, "gc_killed", Some(&detail)) {
                eprintln!("[gc] session_event insert failed for {}: {e}", p.session_id);
            }
        }
        let result = if via_claude {
            exec.safe_kill(&p.host_alias, &p.tmux_name).await
        } else {
            exec.kill(&p.host_alias, &p.tmux_name).await
        };
        match result {
            Ok(()) if via_claude => report.safe_kill_requested += 1,
            Ok(()) => report.killed += 1,
            Err(e) => {
                report.failed += 1;
                eprintln!(
                    "[gc] {} of {}/{} failed: {e}",
                    detail, p.host_alias, p.tmux_name
                );
                if let Ok(s) = store.lock() {
                    let _ = s.insert_session_event(p.session_id, "gc_failed", Some(&e.message));
                }
            }
        }
    }
    report
}

/// Tick entry point: sweep when `gc.enabled` and the sweep interval has
/// elapsed since the last sweep. Cheap when disabled (one settings read).
pub async fn maybe_sweep(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) -> Option<GcReport> {
    static LAST: once_cell::sync::Lazy<Mutex<Option<std::time::Instant>>> =
        once_cell::sync::Lazy::new(|| Mutex::new(None));
    let cfg = {
        let s = store.lock().ok()?;
        GcConfig::from_store(&s)
    };
    if !cfg.enabled {
        return None;
    }
    {
        let mut last = LAST.lock().ok()?;
        let due = last
            .map(|t| t.elapsed().as_secs() >= cfg.sweep_interval_secs)
            .unwrap_or(true);
        if !due {
            return None;
        }
        *last = Some(std::time::Instant::now());
    }
    let exec = RealGcExec {
        store: Arc::clone(store),
        ssh: Arc::clone(ssh),
    };
    let report = sweep_with(store, &exec, &cfg, now_unix()).await;
    if report != GcReport::default() {
        eprintln!(
            "[gc] sweep: killed={} safe_kill_requested={} failed={}",
            report.killed, report.safe_kill_requested, report.failed
        );
    }
    Some(report)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn row(id: i64, kind: &str, idle_since: Option<i64>, last_activity_at: i64) -> SessionRow {
        SessionRow {
            id,
            tmux_name: format!("s{id}"),
            host_alias: "local".into(),
            project_id: None,
            worktree_id: None,
            created_at: 0,
            last_activity_at,
            status: "running".into(),
            notes: None,
            account_uuid: None,
            kind: kind.into(),
            reviews_session_id: None,
            worktree_key: None,
            lost_at: None,
            claude_session_id: None,
            claude_status: idle_since.map(|_| "idle".to_string()),
            effort_level: None,
            pr_url: None,
            current_activity: None,
            context_pct: None,
            stuck_kind: None,
            friendly_name: None,
            safe_kill_state: None,
            safe_kill_nonce: None,
            safe_kill_detail: None,
            safe_kill_requested_at: None,
            idle_since,
            stuck_since: None,
            last_playbook_at: None,
            last_prompt: None,
            started_at: None,
            last_turn_at: None,
            ci_status: None,
        }
    }

    const CFG: GcConfig = GcConfig {
        enabled: true,
        bg_idle_secs: 100,
        shell_idle_secs: 1000,
        work_idle_secs: 50,
        sweep_interval_secs: 1,
    };

    fn local() -> HashSet<String> {
        HashSet::from(["local".to_string()])
    }

    #[test]
    fn disabled_config_plans_nothing() {
        let cfg = GcConfig {
            enabled: false,
            ..CFG
        };
        let rows = vec![row(1, "bg", Some(0), 0)];
        assert!(plan(&rows, &cfg, None, &local(), 10_000).is_empty());
    }

    #[test]
    fn ttl_applies_per_kind_and_zero_means_never() {
        let rows = vec![
            row(1, "bg", Some(0), 0),     // idle 500 ≥ 100 ⇒ kill
            row(2, "bg", Some(450), 450), // idle 50 < 100 ⇒ keep
            row(3, "shell", None, 0),     // last activity 500 < 1000 ⇒ keep
            row(4, "shell", None, -600),  // 1100 ≥ 1000 ⇒ kill
            row(5, "work", Some(400), 0), // idle 100 ≥ 50 ⇒ inspect
            row(6, "work", None, 0),      // no idle stamp ⇒ never
        ];
        let planned = plan(&rows, &CFG, None, &local(), 500);
        let got: Vec<_> = planned.iter().map(|p| (p.session_id, p.action)).collect();
        assert_eq!(
            got,
            vec![
                (1, GcAction::Kill),
                (4, GcAction::Kill),
                (5, GcAction::InspectThenKill)
            ]
        );
        let never = GcConfig {
            work_idle_secs: 0,
            ..CFG
        };
        assert!(plan(
            &[row(7, "work", Some(0), 0)],
            &never,
            None,
            &local(),
            10_000
        )
        .is_empty());
    }

    #[test]
    fn bg_without_agent_status_falls_back_to_last_activity() {
        let mut r = row(1, "bg", None, 0);
        r.claude_status = None;
        assert_eq!(plan(&[r.clone()], &CFG, None, &local(), 500).len(), 1);
        // With a status but no idle stamp (still working) it is never touched.
        r.claude_status = Some("working".into());
        assert!(plan(&[r], &CFG, None, &local(), 500).is_empty());
    }

    #[test]
    fn plan_skips_ghosts_in_flight_safe_kills_controller_and_offline_hosts() {
        let mut ghost = row(1, "bg", Some(0), 0);
        ghost.status = "ghost".into();
        let mut in_flight = row(2, "bg", Some(0), 0);
        in_flight.safe_kill_state = Some("requested".into());
        let ctl = row(3, "bg", Some(0), 0);
        let mut offline = row(4, "bg", Some(0), 0);
        offline.host_alias = "remote".into();
        let controller = ("local".to_string(), "s3".to_string());
        let planned = plan(
            &[ghost, in_flight, ctl, offline],
            &CFG,
            Some(&controller),
            &local(),
            10_000,
        );
        assert!(planned.is_empty());
    }

    fn inspection(dirty: usize, unpushed: i32, upstream: bool) -> SafeKillInspection {
        SafeKillInspection {
            has_worktree: true,
            worktree_path: Some("/w".into()),
            branch: Some("b".into()),
            upstream: upstream.then(|| "origin/b".to_string()),
            dirty_files: (0..dirty)
                .map(|i| crate::service::safe_kill::DirtyFile {
                    status: " M".into(),
                    path: format!("f{i}"),
                })
                .collect(),
            unpushed_commits: unpushed,
            safe_to_remove: dirty == 0 && unpushed == 0 && upstream,
            error: None,
        }
    }

    #[test]
    fn needs_safe_remove_only_when_work_could_be_lost() {
        assert!(!needs_safe_remove(&inspection(0, 0, true)));
        assert!(needs_safe_remove(&inspection(1, 0, true)));
        assert!(needs_safe_remove(&inspection(0, 2, true)));
        assert!(needs_safe_remove(&inspection(0, 0, false)));
        let mut errored = inspection(0, 0, true);
        errored.error = Some("git failed".into());
        assert!(needs_safe_remove(&errored));
        // No worktree at all: nothing to lose, plain kill.
        let mut none = inspection(0, 0, false);
        none.has_worktree = false;
        assert!(!needs_safe_remove(&none));
    }

    struct FakeExec {
        dirty: bool,
        kills: AtomicUsize,
        safe_kills: AtomicUsize,
        inspects: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl GcExec for FakeExec {
        async fn inspect(&self, _h: &str, _t: &str) -> Result<SafeKillInspection, IpcError> {
            self.inspects.fetch_add(1, Ordering::SeqCst);
            Ok(inspection(if self.dirty { 1 } else { 0 }, 0, true))
        }
        async fn safe_kill(&self, _h: &str, _t: &str) -> Result<(), IpcError> {
            self.safe_kills.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn kill(&self, _h: &str, _t: &str) -> Result<(), IpcError> {
            self.kills.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn fake(dirty: bool) -> FakeExec {
        FakeExec {
            dirty,
            kills: AtomicUsize::new(0),
            safe_kills: AtomicUsize::new(0),
            inspects: AtomicUsize::new(0),
        }
    }

    /// In-memory store with a reachable local host and one idle work session
    /// (idle since `idle_since`). Returns the session id.
    fn seed_idle_work(store: &Mutex<Store>, idle_since: i64) -> i64 {
        let s = store.lock().unwrap();
        s.upsert_host("local").unwrap();
        s.update_host_probe("local", true, None, None, 1).unwrap();
        let id = s
            .upsert_session("dev-idle", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(id, "uuid-idle").unwrap();
        s.set_claude_status_by_session_id("uuid-idle", "idle")
            .unwrap();
        s.conn_ref()
            .execute(
                "UPDATE sessions SET idle_since=?1 WHERE id=?2",
                rusqlite::params![idle_since, id],
            )
            .unwrap();
        id
    }

    #[tokio::test]
    async fn sweep_kills_a_clean_idle_work_session_and_records_the_event() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let id = seed_idle_work(&store, 0);
        let exec = fake(false);
        let report = sweep_with(&store, &exec, &CFG, 10_000).await;
        assert_eq!(
            report,
            GcReport {
                killed: 1,
                safe_kill_requested: 0,
                failed: 0
            }
        );
        assert_eq!(exec.inspects.load(Ordering::SeqCst), 1);
        let s = store.lock().unwrap();
        let events = s.list_session_events(id, 10).unwrap();
        let gc: Vec<_> = events.iter().filter(|e| e.kind == "gc_killed").collect();
        assert_eq!(gc.len(), 1);
        assert_eq!(gc[0].detail.as_deref(), Some("work:kill:idle_10000s"));
    }

    #[tokio::test]
    async fn sweep_routes_a_dirty_work_session_through_safe_kill() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let id = seed_idle_work(&store, 0);
        let exec = fake(true);
        let report = sweep_with(&store, &exec, &CFG, 10_000).await;
        assert_eq!(
            report,
            GcReport {
                killed: 0,
                safe_kill_requested: 1,
                failed: 0
            }
        );
        assert_eq!(exec.kills.load(Ordering::SeqCst), 0);
        let s = store.lock().unwrap();
        let events = s.list_session_events(id, 10).unwrap();
        assert!(events
            .iter()
            .any(|e| e.kind == "gc_killed"
                && e.detail.as_deref() == Some("work:safe_kill:idle_10000s")));
    }

    #[tokio::test]
    async fn sweep_is_a_noop_before_the_ttl_or_when_disabled() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        seed_idle_work(&store, 9_990);
        let exec = fake(false);
        assert_eq!(
            sweep_with(&store, &exec, &CFG, 10_000).await,
            GcReport::default()
        );
        let off = GcConfig {
            enabled: false,
            ..CFG
        };
        assert_eq!(
            sweep_with(&store, &exec, &off, 1_000_000).await,
            GcReport::default()
        );
        assert_eq!(exec.inspects.load(Ordering::SeqCst), 0);
    }
}
