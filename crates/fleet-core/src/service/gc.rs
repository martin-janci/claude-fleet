//! Session GC (PROD-1): a Settings-driven sweeper that kills sessions idle
//! longer than their kind's TTL. Runs from the background tick every
//! `gc.sweep_interval_secs`. The idle-session killer itself is opt-in via
//! `gc.enabled` (default off); the retired-participant retention sweep (see
//! `Store::sweep_retired_participants`) and the orphan read-cursor sweep
//! (see `Store::sweep_orphan_read_cursors`) are not gated on it and run
//! every tick regardless, so tombstoned participants and their mail, and
//! cursors whose reader or target session is gone, never pile up on an
//! install that has never turned GC on. (A session delete already drops
//! its cursors at once, through migration 044's trigger; the cursor sweep
//! is the backstop for rows naming an id no session ever had.)
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub mod tidy;

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
/// safe-kill in flight, the registered controller, hosts that are not
/// reachable (a kill would fail and an offline host's idle stamps are stale),
/// and work sessions with no tracked worktree (main-checkout / orphan rows:
/// `inspect_safe_kill` cannot see their tree, so nothing may kill them
/// unattended). Review sessions share their source's worktree and are only
/// ever plain-killed — a safe-remove would arm deletion of a tree another
/// session is using.
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
        // Fleet does not own an `external` session's process (it runs
        // outside tmux, wherever the user started it): never collected.
        if r.status != "running" || r.safe_kill_state.is_some() || r.kind == "external" {
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
        if !matches!(r.kind.as_str(), "bg" | "shell") && r.worktree_id.is_none() {
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
            "bg" | "shell" | "review" => GcAction::Kill,
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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GcReport {
    pub killed: usize,
    pub safe_kill_requested: usize,
    pub failed: usize,
    /// Retired participants whose retention window elapsed this sweep (see
    /// `Store::sweep_retired_participants`). `#[serde(default)]`: this
    /// report crosses the hub wire, and a new field without a default is a
    /// shipped outage against an older hub.
    #[serde(default)]
    pub swept_participants: usize,
    /// Read cursors swept because their reader, or a non-NULL target
    /// session, no longer exists (`Store::sweep_orphan_read_cursors`), this
    /// sweep. Same not-gated-on-`gc.enabled` reasoning as
    /// `swept_participants` above: an orphaned cursor has no one left to
    /// read it and no retention window, so it is cleaned on every install.
    /// `#[serde(default)]` for the same reason as `swept_participants`: this
    /// report crosses the hub wire, and a new field without a default is a
    /// shipped outage against an older hub.
    #[serde(default)]
    pub swept_read_cursors: usize,
    /// Work-journal rows past `work.retention.journal_days` swept this
    /// sweep (`service::work::retention`). Not gated on `gc.enabled`, like
    /// the two sweeps above. `#[serde(default)]` for the same wire reason.
    #[serde(default)]
    pub swept_journal: usize,
    /// Done, unlinked tracker items past `work.retention.tracker_items_days`.
    #[serde(default)]
    pub swept_tracker_items: usize,
    /// Work timeline events past `work.retention.timeline_work_events_days`.
    #[serde(default)]
    pub swept_work_events: usize,
    /// Sessions auto-tidy acted on this sweep (work graph M7: only with
    /// `work.auto_tidy` on; safe kill or archive of the allowed reasons).
    /// `#[serde(default)]` for the same wire reason.
    #[serde(default)]
    pub tidied: usize,
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
    // Sessions the idle killer acted on: auto-tidy (below) leaves them be.
    let mut acted: HashSet<i64> = HashSet::new();
    // The session-idle killer stays opt-in (`cfg.enabled`, default off): it
    // is a destructive action against a live session. The mail-retention
    // sweep below is NOT gated on it — it only ever touches participants
    // retired more than 7 days ago, so it must run on every install or
    // tombstoned participants and their mail accumulate forever (the same
    // shape as the 22k-row / 88MB precedent this module's docs cite for
    // unbounded bg rows) and `message_undeliverable` never fires.
    if cfg.enabled {
        let (rows, controller, reachable) = {
            if let Ok(s) = store.lock() {
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
            } else {
                (Vec::new(), None, HashSet::new())
            }
        };
        for p in plan(&rows, cfg, controller.as_ref(), &reachable, now) {
            acted.insert(p.session_id);
            let via_claude = match p.action {
                GcAction::Kill => false,
                GcAction::InspectThenKill => {
                    match exec.inspect(&p.host_alias, &p.tmux_name).await {
                        Ok(insp) => needs_safe_remove(&insp),
                        Err(e) => {
                            tracing::warn!(
                                host = %p.host_alias,
                                session = %p.tmux_name,
                                error = %e,
                                "[gc] inspect failed; using safe-remove"
                            );
                            true
                        }
                    }
                }
            };
            let detail = format!(
                "{}:{}:idle_{}s",
                p.kind,
                if via_claude { "safe_kill" } else { "kill" },
                p.idle_secs
            );
            if let Ok(s) = store.lock() {
                if let Err(e) = s.insert_session_event(p.session_id, "gc_killed", Some(&detail)) {
                    tracing::warn!(
                        session_id = p.session_id,
                        error = %e,
                        "[gc] session_event insert failed"
                    );
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
                    tracing::warn!(
                        host = %p.host_alias,
                        session = %p.tmux_name,
                        action = %detail,
                        error = %e,
                        "[gc] action failed"
                    );
                    if let Ok(s) = store.lock() {
                        let _ = s.insert_session_event(p.session_id, "gc_failed", Some(&e.message));
                    }
                }
            }
        }
    }
    // Auto-tidy (work graph M7): its own opt-in, `work.auto_tidy` (off by
    // default); with it off this reads one setting and does nothing, so the
    // idle killer above behaves exactly as before.
    report.tidied = crate::service::work::tidy::auto_tidy(store, exec, &acted, now).await;
    // `sweep_retired_participants` returns `Result<usize, IpcError>`, but
    // this function returns a plain `GcReport` (not a `Result`), so a
    // failed sweep contributes 0 and the pass still completes — the same
    // best-effort pattern the rest of this function already uses for its
    // other store reads/writes; a GC sweep must never abort the whole pass.
    report.swept_participants = match store.lock() {
        Ok(s) => s
            .sweep_retired_participants(now, crate::store::RETIRED_RETENTION_SECS)
            .unwrap_or(0),
        Err(_) => 0,
    };
    // Same best-effort, not-gated-on-`cfg.enabled` pattern as the retention
    // sweep just above: a cursor whose reader (or non-NULL target) is gone
    // has no one left to serve a delta to, so it is cleaned on every
    // install regardless of whether the destructive idle-session killer is
    // turned on. A BACKSTOP: every session delete already drops that
    // session's cursors at once (migration 044's trigger), so this finds
    // only rows written with an id no session ever had.
    report.swept_read_cursors = match store.lock() {
        Ok(s) => s.sweep_orphan_read_cursors().unwrap_or(0),
        Err(_) => 0,
    };
    // Hub↔hub outbox: a message a peer never took within 7 days, or one
    // queued on a removed link, fails back to its sender. Ungated, like the
    // two sweeps above: it is bookkeeping, not the idle killer.
    if let Ok(s) = store.lock() {
        if let Err(e) = s.sweep_peer_outbox(now, crate::store::PEER_PENDING_MAX_SECS) {
            // G24: this used to be `let _ =`, silently dropping both the
            // error and the count — an operator had no way to learn the
            // outbox sweep stopped running.
            tracing::warn!(error = %e, "[gc] peer outbox sweep failed");
        }
    }
    // Work graph retention (M12.3): the journal, done tracker items and
    // work timeline events, by `work.retention.*` (0 = forever). Ungated
    // like the sweeps above; bounded per tick, one batch per lock.
    let r = crate::service::work::retention::sweep(store, now);
    report.swept_journal = r.journal;
    report.swept_tracker_items = r.tracker_items;
    report.swept_work_events = r.timeline_work_events;
    report
}

/// Single-flight guard mirroring `service::usage`'s: at most one sweep pass
/// runs at a time. The interval gate in `maybe_sweep` below already makes
/// two back-to-back calls a no-op in the common case (the second finds
/// itself not due, since `LAST` is stamped before the sweep runs) — but a
/// `gc.sweep_interval_secs` of `0` would otherwise let two ticks race into
/// overlapping sweeps, so this guard is unconditional, not just interval-based.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Held while a sweep runs; clears [`RUNNING`] on drop (also on panic).
struct Flight;

impl Drop for Flight {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

fn begin_flight() -> Option<Flight> {
    RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .ok()
        .map(|_| Flight)
}

/// Reconcile-tick hook: spawn [`maybe_sweep`] as its own task so a slow GC
/// sweep (it does SSH work — safe-kill inspections, kills) never stretches
/// the tick body past its period. Mirrors `service::usage::spawn_collect`
/// exactly: unconditional spawn, single-flight enforced inside the spawned
/// call, so a second spawn while one is still running is a no-op. Must be
/// called from inside the tokio runtime.
pub fn spawn_sweep(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) {
    let (store, ssh) = (Arc::clone(store), Arc::clone(ssh));
    tokio::spawn(async move {
        let _ = maybe_sweep(&store, &ssh).await;
    });
}

/// Tick entry point: sweeps once the sweep interval has elapsed since the
/// last tick, regardless of `gc.enabled` — that flag only gates the
/// destructive idle-session killer inside `sweep_with`, which the
/// mail-retention pass is not, and which must run on every install (see the
/// module doc). Not "cheap when disabled": every due tick does a real
/// retention sweep, not just a settings read.
pub async fn maybe_sweep(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) -> Option<GcReport> {
    static LAST: std::sync::LazyLock<Mutex<Option<std::time::Instant>>> =
        std::sync::LazyLock::new(|| Mutex::new(None));
    let _flight = begin_flight()?;
    let cfg = {
        let s = store.lock().ok()?;
        GcConfig::from_store(&s)
    };
    // Deliberately NOT gated on `cfg.enabled` here: that flag opts a fleet
    // into the destructive session-idle killer, but `sweep_with` also runs
    // the mail-retention sweep (participants retired > 7 days), which is not
    // that killer and must run on every install regardless. Bailing out here
    // on a disabled default would mean `sweep_with` is never even called, so
    // tombstoned participants and their mail would accumulate forever. The
    // interval gating below is shared with the retention pass rather than
    // given its own scheduler.
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
        // Only when the sweep acted: a no-op sweep stays silent.
        tracing::info!(
            killed = report.killed,
            safe_kill_requested = report.safe_kill_requested,
            failed = report.failed,
            swept_participants = report.swept_participants,
            swept_read_cursors = report.swept_read_cursors,
            swept_journal = report.swept_journal,
            swept_tracker_items = report.swept_tracker_items,
            swept_work_events = report.swept_work_events,
            tidied = report.tidied,
            "[gc] sweep"
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

    /// Task 1(c): the tick no longer awaits `maybe_sweep` inline — it is
    /// spawned single-flight (`spawn_sweep`), mirroring
    /// `service::usage`'s `only_one_collection_pass_is_in_flight`. A second
    /// spawn while one sweep is still running must do nothing.
    #[test]
    fn only_one_sweep_pass_is_in_flight() {
        let first = begin_flight().expect("no pass running");
        assert!(begin_flight().is_none(), "a second pass is refused");
        drop(first);
        assert!(begin_flight().is_some(), "released on drop");
    }

    pub(super) fn row(
        id: i64,
        kind: &str,
        idle_since: Option<i64>,
        last_activity_at: i64,
    ) -> SessionRow {
        SessionRow {
            id,
            row_version: 0,
            prompt_submit_seq: 0,
            tmux_name: format!("s{id}"),
            host_alias: "local".into(),
            project_id: None,
            // Work/review rows get a tracked worktree by default; the
            // no-worktree case is exercised explicitly below.
            worktree_id: if matches!(kind, "bg" | "shell") {
                None
            } else {
                Some(1)
            },
            created_at: 0,
            last_activity_at,
            status: "running".into(),
            notes: None,
            account_uuid: None,
            kind: kind.into(),
            reviews_session_id: None,
            worktree_key: None,
            lost_at: None,
            lost_reason: None,
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
    fn external_rows_are_never_collected() {
        // Fleet does not own an interactive session running outside tmux:
        // no action of any kind, however idle, even with a worktree linked.
        let mut ext = row(1, "external", Some(0), 0);
        ext.worktree_id = Some(1);
        let mut no_status = row(2, "external", None, 0);
        no_status.claude_status = None;
        assert!(plan(&[ext, no_status], &CFG, None, &local(), 1_000_000).is_empty());
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
    fn work_rows_without_a_tracked_worktree_are_never_collected() {
        let mut orphan = row(1, "work", Some(0), 0);
        orphan.worktree_id = None;
        assert!(plan(&[orphan], &CFG, None, &local(), 10_000).is_empty());
        // bg / shell rows never have a worktree and are still eligible.
        assert_eq!(
            plan(&[row(2, "bg", Some(0), 0)], &CFG, None, &local(), 10_000).len(),
            1
        );
    }

    #[test]
    fn review_rows_are_plain_killed_never_safe_removed() {
        let review = row(3, "review", Some(0), 0);
        let planned = plan(&[review], &CFG, None, &local(), 10_000);
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].action, GcAction::Kill);
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
        let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
        let wid = s
            .upsert_worktree(pid, "idle", "/p/o/r/.worktrees/idle", Some("idle"))
            .unwrap();
        let id = s
            .upsert_session(
                "dev-idle",
                "local",
                Some(pid),
                Some(wid),
                1,
                1,
                "running",
                None,
            )
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
                failed: 0,
                swept_participants: 0,
                swept_read_cursors: 0,
                swept_journal: 0,
                swept_tracker_items: 0,
                swept_work_events: 0,
                tidied: 0,
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
                failed: 0,
                swept_participants: 0,
                swept_read_cursors: 0,
                swept_journal: 0,
                swept_tracker_items: 0,
                swept_work_events: 0,
                tidied: 0,
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

    /// The mail-retention sweep runs alongside the session-idle one, on the
    /// SAME `sweep_with` call, using the caller-supplied `now` rather than
    /// the wall clock — so this test can age a tombstone with plain
    /// arithmetic instead of a real 7-day wait.
    #[tokio::test]
    async fn sweep_also_reaps_retired_participants_past_their_retention_window() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let (sender, msg) = {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            let a = s
                .upsert_session("alpha", "local", None, None, 0, 0, "running", None)
                .unwrap();
            let b = s
                .upsert_session("beta", "local", None, None, 0, 0, "running", None)
                .unwrap();
            let m = s.insert_message(a, b, "pending", "message", None).unwrap();
            s.delete_session(b).unwrap();
            // Age the tombstone past the retention window.
            s.conn_ref()
                .execute(
                    "UPDATE participants SET retired_at = retired_at - ?1 \
                     WHERE retired_at IS NOT NULL",
                    rusqlite::params![crate::store::RETIRED_RETENTION_SECS + 60],
                )
                .unwrap();
            (a, m)
        };
        let exec = fake(false);
        // `retired_at` is stamped from the real wall clock (`delete_session`
        // -> `Store::retire_participant` both use `now_unix()`), so the
        // sweep must be driven by a `now` in the same frame — unlike the
        // idle-session sweep above, which never touches wall time and is
        // free to use a small synthetic clock.
        let report = sweep_with(&store, &exec, &CFG, now_unix()).await;
        assert_eq!(
            report,
            GcReport {
                killed: 0,
                safe_kill_requested: 0,
                failed: 0,
                swept_participants: 1,
                swept_read_cursors: 0,
                swept_journal: 0,
                swept_tracker_items: 0,
                swept_work_events: 0,
                tidied: 0,
            }
        );
        let s = store.lock().unwrap();
        assert!(s.get_message(msg).unwrap().is_none(), "the mail is gone");
        let ev = s.list_session_events(sender, 10).unwrap();
        assert!(
            ev.iter().any(|e| e.kind == "message_undeliverable"),
            "the sender must learn its message was never read: {ev:?}"
        );
    }

    /// Fix round 1, Important 1: the mail-retention sweep is not the opt-in
    /// session killer and must run on a DEFAULT (`gc.enabled = false`)
    /// install, or tombstoned participants and their mail accumulate
    /// forever and `message_undeliverable` never fires for anyone. Same
    /// setup as `sweep_also_reaps_retired_participants_past_their_retention_window`,
    /// but with `enabled: false` and asserting the session-idle side (`exec`)
    /// is never touched.
    #[tokio::test]
    async fn sweep_reaps_retired_participants_even_when_gc_is_disabled() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let (sender, msg) = {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            let a = s
                .upsert_session("alpha", "local", None, None, 0, 0, "running", None)
                .unwrap();
            let b = s
                .upsert_session("beta", "local", None, None, 0, 0, "running", None)
                .unwrap();
            let m = s.insert_message(a, b, "pending", "message", None).unwrap();
            s.delete_session(b).unwrap();
            s.conn_ref()
                .execute(
                    "UPDATE participants SET retired_at = retired_at - ?1 \
                     WHERE retired_at IS NOT NULL",
                    rusqlite::params![crate::store::RETIRED_RETENTION_SECS + 60],
                )
                .unwrap();
            (a, m)
        };
        let disabled = GcConfig {
            enabled: false,
            ..CFG
        };
        let exec = fake(false);
        let report = sweep_with(&store, &exec, &disabled, now_unix()).await;
        assert_eq!(
            report,
            GcReport {
                killed: 0,
                safe_kill_requested: 0,
                failed: 0,
                swept_participants: 1,
                swept_read_cursors: 0,
                swept_journal: 0,
                swept_tracker_items: 0,
                swept_work_events: 0,
                tidied: 0,
            },
            "the retention sweep must run regardless of gc.enabled"
        );
        assert_eq!(
            exec.inspects.load(Ordering::SeqCst),
            0,
            "the session-idle killer stays off"
        );
        let s = store.lock().unwrap();
        assert!(s.get_message(msg).unwrap().is_none(), "the mail is gone");
        let ev = s.list_session_events(sender, 10).unwrap();
        assert!(
            ev.iter().any(|e| e.kind == "message_undeliverable"),
            "the sender must learn its message was never read even on a default install: {ev:?}"
        );
    }

    /// Task 8: orphan read-cursor retention (`Store::sweep_orphan_read_cursors`)
    /// must run on the SAME `sweep_with` call as the mail-retention sweep,
    /// and — like that sweep — must NOT be gated on `gc.enabled`. Since
    /// Ruling 16 a session delete drops its cursors at once (migration 044's
    /// trigger), so a deleted reader leaves the sweep nothing; the sweep is
    /// the backstop for a cursor naming a reader id no session ever had,
    /// which it removes. A cursor whose reader is alive is kept; the
    /// idle-session killer stays off (asserted via `exec.inspects == 0`,
    /// the same proof the mail-retention disabled-gc test above uses).
    #[tokio::test]
    async fn sweep_reaps_orphan_read_cursors_even_when_gc_is_disabled() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let keep = {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            let gone = s
                .upsert_session("gone-reader", "local", None, None, 0, 0, "running", None)
                .unwrap();
            let keep = s
                .upsert_session("keep-reader", "local", None, None, 0, 0, "running", None)
                .unwrap();
            s.put_stream_cursor(gone, "session_history", "1", None, 5, None, None)
                .unwrap();
            s.put_stream_cursor(keep, "session_history", "2", None, 5, None, None)
                .unwrap();
            s.delete_session(gone).unwrap();
            assert!(
                s.get_read_cursor(gone, "session_history", "1")
                    .unwrap()
                    .is_none(),
                "the delete itself dropped the gone reader's cursor (trigger)"
            );
            // Dangling: a reader id no session row ever had — no delete
            // fired for it, so only the sweep can find it.
            s.put_stream_cursor(9_001, "session_history", "3", None, 5, None, None)
                .unwrap();
            keep
        };
        let disabled = GcConfig {
            enabled: false,
            ..CFG
        };
        let exec = fake(false);
        let report = sweep_with(&store, &exec, &disabled, now_unix()).await;
        assert_eq!(
            report.swept_read_cursors, 1,
            "exactly the dangling-reader cursor is swept"
        );
        assert_eq!(
            exec.inspects.load(Ordering::SeqCst),
            0,
            "the session-idle killer stays off"
        );
        let s = store.lock().unwrap();
        assert!(
            s.get_read_cursor(keep, "session_history", "2")
                .unwrap()
                .is_some(),
            "the live reader's cursor is kept"
        );
    }

    /// Task 9: the hub↔hub outbox sweep runs inside `sweep_with`, same
    /// not-gated-on-`gc.enabled` shape as the mail-retention and orphan
    /// read-cursor sweeps above — a week-old pending row to a peer fails back
    /// to its sender even on a default (`enabled: false`) install.
    #[tokio::test]
    async fn sweep_fails_a_week_old_peer_outbox_row_even_when_gc_is_disabled() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let (sender, link) = {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            let a1 = s
                .upsert_session("a1", "local", None, None, 0, 0, "running", None)
                .unwrap();
            let link = s.insert_dialer_link("https://b.example", "t").unwrap();
            let to = s
                .ensure_remote_participant(link, "fleet-b/session/h/b1")
                .unwrap();
            let m = s
                .insert_outbound_remote(
                    a1,
                    "fleet-a/session/local/a1",
                    to,
                    "x",
                    "message",
                    None,
                    false,
                )
                .unwrap();
            s.conn_ref()
                .execute("UPDATE session_messages SET sent_at = 0 WHERE id = ?1", [m])
                .unwrap();
            (a1, link)
        };
        let disabled = GcConfig {
            enabled: false,
            ..CFG
        };
        let exec = fake(false);
        let now = crate::store::PEER_PENDING_MAX_SECS + 1;
        sweep_with(&store, &exec, &disabled, now).await;
        assert_eq!(
            exec.inspects.load(Ordering::SeqCst),
            0,
            "the session-idle killer stays off"
        );
        let s = store.lock().unwrap();
        assert!(
            s.pending_outbox(link, 0, 50).unwrap().is_empty(),
            "the stale row is off the outbox"
        );
        let ev = s.list_session_events(sender, 10).unwrap();
        assert!(
            ev.iter().any(|e| e.kind == "message_undeliverable"),
            "the sender must learn its message was never taken by the peer: {ev:?}"
        );
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
