//! Opt-in automatic workspace repair on the background reconcile tick.
//!
//! When `repair.auto_on_tick` is on (default off), at most once every
//! `repair.tick_interval_secs` (default 600) the tick looks for running
//! sessions whose worktree directory has vanished and runs the AUTOMATIC
//! repair policy on them ([`repair::ensure_session_workspace`] with
//! [`TICK_ENTRY`]): it may only `git worktree add` from an existing local or
//! remote-tracking branch into a target that is absent on disk and not
//! registered. It never creates or respawns tmux, never unregisters, adopts,
//! rebranches or runs `git worktree repair` — those stay explicit-only.
//!
//! **Signal.** The reconcile pass reads each pane's cwd but never checks that
//! it exists, and does not persist it. So the tick adds ONE batched,
//! read-only `test -d` script per reachable host (every path `quote`d) over
//! the candidates' expected directories, resolved by the same spec builder
//! the repair itself uses (`repair::spec_for_session`, no ssh round trip:
//! local paths come from the store, the remote `$HOME` is cached).
//!
//! **Bounds.** Hosts are handled one at a time, at most
//! [`MAX_REPAIRS_PER_TICK`] repairs per tick, and the whole run is detached
//! from the reconcile loop so a slow `git worktree add` never delays a pass.
//! Never touched: the registered controller, a session with a safe-kill in
//! flight, review sessions (they share their source's worktree), `bg`
//! sessions, ghosts / lost rows, and sessions on unreachable or hidden hosts.
//!
//! **Record + backoff.** Every attempt leaves exactly one
//! `workspace_repaired` / `workspace_repair_failed` event (the repair writes
//! most of them; the tick fills the gaps, e.g. `E_REPAIR_REQUIRED`). A
//! non-transient refusal stamps the row (`repair_backoff_sig`, migration 020)
//! with a signature of its workspace; the tick skips the row until that
//! signature changes, and clears the stamp once the directory is back.

use crate::ipc_error::{codes, IpcError};
use crate::service::repair::{self, Entry, RepairReport, WorkspaceSpec};
use crate::service::settings;
use crate::shell::quote;
use crate::ssh::{SshClient, SshExec};
use crate::store::{SessionRow, Store};
use async_trait::async_trait;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// At most this many repair attempts per tick, across all hosts.
pub const MAX_REPAIRS_PER_TICK: usize = 5;

/// The entry point the tick repairs as: `Policy::Auto { create_dead_tmux:
/// false }` — create-only, tmux untouched (same policy as restart/recreate).
pub const TICK_ENTRY: Entry = Entry::Restart;

/// Wall clock for one host's batched directory check.
const DIR_CHECK_TIMEOUT: Duration = Duration::from_secs(15);

/// Printed once per missing candidate (`<prefix><index>`).
const MISSING_PREFIX: &str = "@@fleet_missing=";
/// Printed last: a check whose output lacks it is ignored (no action).
const DONE_MARKER: &str = "@@fleet_dircheck_done";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepairTickConfig {
    pub enabled: bool,
    pub interval_secs: u64,
}

impl RepairTickConfig {
    pub fn from_store(s: &Store) -> Self {
        Self {
            enabled: settings::get_bool(s, settings::REPAIR_AUTO_ON_TICK),
            interval_secs: settings::get_secs(s, settings::REPAIR_TICK_INTERVAL_SECS),
        }
    }
}

/// Where a candidate's worktree should be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Directories any one of which counts as "present". A convention-derived
    /// (guessed) path also accepts the project's `.worktrees/<name>` layout,
    /// exactly as the repair probe does.
    pub dirs: Vec<String>,
    /// Branch the worktree must carry (part of the backoff signature).
    pub branch: String,
}

/// The worktree [`Target`] of a spec; `None` for a main-checkout session (a
/// missing project root is never recreated automatically).
pub fn target_from_spec(spec: &WorkspaceSpec) -> Option<Target> {
    let w = spec.worktree.as_ref()?;
    let mut dirs = vec![w.path.clone()];
    if w.path_is_guess {
        let alt = format!("{}/.worktrees/{}", spec.project_root, w.name);
        if alt != w.path {
            dirs.push(alt);
        }
    }
    Some(Target {
        dirs,
        branch: w.branch.clone(),
    })
}

/// Side effects of one tick run, injected for tests.
#[async_trait]
pub trait RepairTickExec: Send + Sync {
    /// Expected worktree location of a session (no ssh round trip).
    async fn target(&self, session_id: i64) -> Result<Option<Target>, IpcError>;
    /// ONE read-only check on `host`: the indices of `targets` none of whose
    /// directories exist.
    async fn missing(
        &self,
        host: &str,
        targets: &[Vec<String>],
    ) -> Result<HashSet<usize>, IpcError>;
    /// The automatic repair of one session.
    async fn repair(&self, session_id: i64) -> Result<RepairReport, IpcError>;
}

/// Production executor over the real repair service.
pub struct RealRepairTickExec {
    pub store: Arc<Mutex<Store>>,
    pub ssh: Arc<SshClient>,
}

#[async_trait]
impl RepairTickExec for RealRepairTickExec {
    async fn target(&self, session_id: i64) -> Result<Option<Target>, IpcError> {
        let (spec, _siblings) =
            repair::spec_for_session(&self.store, &self.ssh, session_id).await?;
        Ok(target_from_spec(&spec))
    }

    async fn missing(
        &self,
        host: &str,
        targets: &[Vec<String>],
    ) -> Result<HashSet<usize>, IpcError> {
        check_missing(&*self.ssh, host, targets).await
    }

    async fn repair(&self, session_id: i64) -> Result<RepairReport, IpcError> {
        repair::ensure_session_workspace(session_id, TICK_ENTRY, &self.store, &self.ssh).await
    }
}

/// The batched, read-only directory check. Pure so the quoting is testable.
pub fn dir_check_script(targets: &[Vec<String>]) -> String {
    let mut s = String::new();
    for (i, dirs) in targets.iter().enumerate() {
        let test = dirs
            .iter()
            .map(|d| format!("[ -d {} ]", quote(d)))
            .collect::<Vec<_>>()
            .join(" || ");
        s.push_str(&format!(
            "if {test}; then :; else printf '%s\\n' '{MISSING_PREFIX}{i}'; fi\n"
        ));
    }
    s.push_str(&format!("printf '%s\\n' '{DONE_MARKER}'\n"));
    s
}

/// Parse [`dir_check_script`] output. Output without the done marker (a
/// failed or truncated run) is an error: the tick then does nothing there.
pub fn parse_dir_check(stdout: &str, n: usize) -> Result<HashSet<usize>, IpcError> {
    if !stdout.lines().any(|l| l.trim() == DONE_MARKER) {
        return Err(IpcError::new(
            codes::E_SHELL,
            "directory check produced no completion marker",
        ));
    }
    Ok(stdout
        .lines()
        .filter_map(|l| l.trim().strip_prefix(MISSING_PREFIX))
        .filter_map(|i| i.parse::<usize>().ok())
        .filter(|i| *i < n)
        .collect())
}

/// Run the directory check on `host`: local `bash -lc`, else one ssh call.
pub async fn check_missing(
    ssh: &dyn SshExec,
    host: &str,
    targets: &[Vec<String>],
) -> Result<HashSet<usize>, IpcError> {
    crate::validate::host_alias(host)?;
    if targets.is_empty() {
        return Ok(HashSet::new());
    }
    let script = dir_check_script(targets);
    let out = if host == "local" {
        let child = tokio::process::Command::new("bash")
            .args(["-lc", &script])
            .kill_on_drop(true)
            .output();
        tokio::time::timeout(DIR_CHECK_TIMEOUT, child)
            .await
            .map_err(|_| IpcError::new(codes::E_TIMEOUT, "local directory check timed out"))?
            .map_err(|e| IpcError::new(codes::E_SHELL, format!("spawn bash: {e}")))?
    } else {
        let out = ssh
            .run(host, &["bash", "-lc", &quote(&script)], DIR_CHECK_TIMEOUT)
            .await?;
        // `SshExec` contract: an unreachable host is exit 255, not `Err`.
        if out.status.code() == Some(255) {
            return Err(IpcError::new(
                codes::E_SSH,
                format!(
                    "ssh {host} failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            ));
        }
        out
    };
    if !out.status.success() {
        return Err(IpcError::new(
            codes::E_SHELL,
            format!(
                "directory check on {host} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }
    parse_dir_check(&String::from_utf8_lossy(&out.stdout), targets.len())
}

/// Pure: the rows the tick may consider at all. See the module doc for the
/// exclusions. Only sessions with a linked worktree qualify.
pub fn eligible<'a>(
    rows: &'a [SessionRow],
    controller: Option<&(String, String)>,
    usable_hosts: &HashSet<String>,
) -> Vec<&'a SessionRow> {
    rows.iter()
        .filter(|r| r.status == "running")
        .filter(|r| !matches!(r.kind.as_str(), "bg" | "review"))
        .filter(|r| r.safe_kill_state.is_none())
        .filter(|r| {
            !controller
                .map(|(h, t)| h == &r.host_alias && t == &r.tmux_name)
                .unwrap_or(false)
        })
        .filter(|r| usable_hosts.contains(&r.host_alias))
        .filter(|r| r.project_id.is_some())
        .filter(|r| {
            r.worktree_id.is_some() || r.worktree_key.as_deref().is_some_and(|k| k != "main")
        })
        .collect()
}

/// Signature of the workspace a refusal was stamped against: any change to
/// the row's host, project, worktree link / key, branch or expected path
/// lifts the backoff.
pub fn backoff_sig(row: &SessionRow, t: &Target) -> String {
    format!(
        "{}|{:?}|{:?}|{:?}|{}|{}",
        row.host_alias,
        row.project_id,
        row.worktree_id,
        row.worktree_key,
        t.branch,
        t.dirs.join("\u{1f}")
    )
}

/// Errors worth retrying at the next interval: the host or transport, not
/// the workspace. Everything else is a refusal that needs a human (or a row
/// change), so it is stamped and backed off.
pub fn is_transient(code: &str) -> bool {
    matches!(
        code,
        codes::E_HOST_OFFLINE
            | codes::E_SSH
            | codes::E_SSH_TIMEOUT
            | codes::E_TIMEOUT
            | codes::E_LOCK
            | codes::E_SHELL
    )
}

/// The backoff stamp of a session: `(signature, unix secs)`.
pub fn backoff_of(s: &Store, session_id: i64) -> Option<(String, i64)> {
    s.conn_ref()
        .query_row(
            "SELECT repair_backoff_sig, repair_backoff_at FROM sessions
             WHERE id=?1 AND repair_backoff_sig IS NOT NULL",
            rusqlite::params![session_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()
}

fn load_backoffs(s: &Store) -> HashMap<i64, String> {
    let Ok(mut stmt) = s.conn_ref().prepare(
        "SELECT id, repair_backoff_sig FROM sessions WHERE repair_backoff_sig IS NOT NULL",
    ) else {
        return HashMap::new();
    };
    stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map(|it| it.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

fn set_backoff(store: &Mutex<Store>, session_id: i64, sig: Option<&str>, now: i64) {
    let Ok(s) = store.lock() else {
        return;
    };
    let at = sig.map(|_| now);
    if let Err(e) = s.conn_ref().execute(
        "UPDATE sessions SET repair_backoff_sig=?1, repair_backoff_at=?2 WHERE id=?3",
        rusqlite::params![sig, at, session_id],
    ) {
        tracing::warn!("[repair-tick] backoff update failed for session {session_id}: {e}");
    }
}

/// Highest repair-event id on the session (0 when none), to tell whether an
/// attempt already recorded its own event.
fn last_repair_event_id(store: &Mutex<Store>, session_id: i64) -> i64 {
    let Ok(s) = store.lock() else {
        return 0;
    };
    s.conn_ref()
        .query_row(
            "SELECT COALESCE(MAX(id), 0) FROM session_events
             WHERE session_id=?1 AND kind IN (?2, ?3)",
            rusqlite::params![
                session_id,
                repair::EVENT_REPAIRED,
                repair::EVENT_REPAIR_FAILED
            ],
            |r| r.get(0),
        )
        .unwrap_or(0)
}

fn record(store: &Mutex<Store>, session_id: i64, kind: &str, detail: &str) {
    if let Ok(s) = store.lock() {
        if let Err(e) = s.insert_session_event(session_id, kind, Some(detail)) {
            tracing::warn!("[repair-tick] event insert failed for session {session_id}: {e}");
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RepairTickReport {
    pub hosts_checked: usize,
    pub missing: usize,
    pub attempted: usize,
    pub repaired: usize,
    pub failed: usize,
    pub backed_off: usize,
    /// Missing directories left for a later tick by [`MAX_REPAIRS_PER_TICK`].
    pub over_cap: usize,
}

/// One run: snapshot under one brief lock, then per host (sequentially)
/// resolve targets, check directories in one call, repair what is missing.
pub async fn run_with(
    store: &Mutex<Store>,
    exec: &dyn RepairTickExec,
    cfg: &RepairTickConfig,
    now: i64,
) -> RepairTickReport {
    let mut report = RepairTickReport::default();
    if !cfg.enabled {
        return report;
    }
    let (rows, controller, usable, backoffs) = {
        let Ok(s) = store.lock() else {
            return report;
        };
        let rows = s.list_all_sessions().unwrap_or_default();
        let controller = s.get_controller().ok().flatten();
        let usable: HashSet<String> = s
            .list_hosts()
            .unwrap_or_default()
            .into_iter()
            .filter(|h| h.reachable && !h.hidden)
            .map(|h| h.alias)
            .collect();
        (rows, controller, usable, load_backoffs(&s))
    };
    let mut by_host: BTreeMap<String, Vec<SessionRow>> = BTreeMap::new();
    for r in eligible(&rows, controller.as_ref(), &usable) {
        by_host
            .entry(r.host_alias.clone())
            .or_default()
            .push(r.clone());
    }
    for (host, mut rows) in by_host {
        if report.attempted >= MAX_REPAIRS_PER_TICK {
            break;
        }
        rows.sort_by_key(|r| r.id);
        let mut targets: Vec<(SessionRow, Target)> = Vec::new();
        for r in rows {
            match exec.target(r.id).await {
                Ok(Some(t)) => targets.push((r, t)),
                Ok(None) => {}
                Err(e) => tracing::debug!("[repair-tick] no target for session {}: {e}", r.id),
            }
        }
        if targets.is_empty() {
            continue;
        }
        let dirs: Vec<Vec<String>> = targets.iter().map(|(_, t)| t.dirs.clone()).collect();
        let missing = match exec.missing(&host, &dirs).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("[repair-tick] directory check on {host} failed: {e}");
                continue;
            }
        };
        report.hosts_checked += 1;
        for (i, (row, t)) in targets.iter().enumerate() {
            let stamped = backoffs.get(&row.id);
            if !missing.contains(&i) {
                if stamped.is_some() {
                    set_backoff(store, row.id, None, now);
                }
                continue;
            }
            report.missing += 1;
            let sig = backoff_sig(row, t);
            if stamped == Some(&sig) {
                report.backed_off += 1;
                continue;
            }
            if report.attempted >= MAX_REPAIRS_PER_TICK {
                report.over_cap += 1;
                continue;
            }
            report.attempted += 1;
            let before = last_repair_event_id(store, row.id);
            let result = exec
                .repair(row.id)
                .await
                .and_then(repair::require_no_explicit);
            let recorded = last_repair_event_id(store, row.id) != before;
            match result {
                Ok(rep) => {
                    if !rep.actions.is_empty() {
                        report.repaired += 1;
                        if !recorded {
                            record(
                                store,
                                row.id,
                                repair::EVENT_REPAIRED,
                                &repair::event_detail(&rep),
                            );
                        }
                    }
                    if stamped.is_some() {
                        set_backoff(store, row.id, None, now);
                    }
                    tracing::info!(
                        "[repair-tick] {host}/{}: {}",
                        row.tmux_name,
                        if rep.actions.is_empty() {
                            "already healthy".to_string()
                        } else {
                            rep.actions.join("; ")
                        }
                    );
                }
                Err(e) => {
                    report.failed += 1;
                    if !recorded {
                        record(
                            store,
                            row.id,
                            repair::EVENT_REPAIR_FAILED,
                            &format!("{}: {}", e.code, e.message),
                        );
                    }
                    if !is_transient(&e.code) {
                        set_backoff(store, row.id, Some(&sig), now);
                    }
                    tracing::warn!("[repair-tick] {host}/{}: {e}", row.tmux_name);
                }
            }
        }
    }
    report
}

/// Pure: is a run due? `None` (never ran) is always due.
pub fn due(last: Option<std::time::Instant>, interval_secs: u64) -> bool {
    last.map(|t| t.elapsed().as_secs() >= interval_secs)
        .unwrap_or(true)
}

/// Resets the in-flight flag even if the run panics.
struct InFlight(&'static AtomicBool);
impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Tick entry point: when enabled, due, and no earlier run is still going,
/// start one run in the background and return `true`. Cheap when disabled
/// (one settings read). Detached so repairs never delay the reconcile loop.
pub fn maybe_run(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) -> bool {
    static LAST: once_cell::sync::Lazy<Mutex<Option<std::time::Instant>>> =
        once_cell::sync::Lazy::new(|| Mutex::new(None));
    static RUNNING: AtomicBool = AtomicBool::new(false);
    let cfg = {
        let Ok(s) = store.lock() else {
            return false;
        };
        RepairTickConfig::from_store(&s)
    };
    if !cfg.enabled {
        return false;
    }
    {
        let Ok(mut last) = LAST.lock() else {
            return false;
        };
        if !due(*last, cfg.interval_secs) {
            return false;
        }
        if RUNNING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return false;
        }
        *last = Some(std::time::Instant::now());
    }
    let guard = InFlight(&RUNNING);
    let store = Arc::clone(store);
    let ssh = Arc::clone(ssh);
    tokio::spawn(async move {
        let _guard = guard;
        let exec = RealRepairTickExec {
            store: Arc::clone(&store),
            ssh,
        };
        let report = run_with(&store, &exec, &cfg, now_unix()).await;
        if report.attempted > 0 {
            tracing::info!(
                "[repair-tick] repaired={} failed={} backed_off={} over_cap={}",
                report.repaired,
                report.failed,
                report.backed_off,
                report.over_cap
            );
        }
    });
    true
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
    use crate::service::repair::{
        plan, policy_for, BranchSource, Policy, Probe, RegisteredWorktree, Step, WorktreeSpec,
    };

    const ON: RepairTickConfig = RepairTickConfig {
        enabled: true,
        interval_secs: 600,
    };

    fn report_with(session_id: i64, actions: &[&str]) -> RepairReport {
        RepairReport {
            session_id: Some(session_id),
            host_alias: "local".into(),
            tmux_name: format!("s{session_id}"),
            project_root: "/repo".into(),
            cwd: format!("/wt/{session_id}"),
            cwd_physical: None,
            healthy: actions.is_empty(),
            actions: actions.iter().map(|a| a.to_string()).collect(),
            warnings: Vec::new(),
            needs_explicit_repair: false,
            deferred: Vec::new(),
            branch_source: None,
            tmux: None,
            tmux_alive: true,
            tmux_dead: false,
            tmux_cwd_stale: true,
            worktree_row_updated: false,
            sibling_session_ids: Vec::new(),
        }
    }

    /// Scripted executor. Directories are `/wt/<session id>`; `missing`
    /// holds the ids whose directory is gone. Repairs default to a
    /// successful create that records its event like the real one does.
    struct Fake {
        store: Arc<Mutex<Store>>,
        missing: Mutex<HashSet<i64>>,
        results: Mutex<HashMap<i64, Result<RepairReport, IpcError>>>,
        /// When set, a failing repair records its own failed event (as
        /// `ensure_workspace`'s refusals do).
        records_failures: bool,
        calls: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(store: &Arc<Mutex<Store>>, missing: &[i64]) -> Self {
            Self {
                store: Arc::clone(store),
                missing: Mutex::new(missing.iter().copied().collect()),
                results: Mutex::new(HashMap::new()),
                records_failures: false,
                calls: Mutex::new(Vec::new()),
            }
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
        fn repairs(&self) -> Vec<String> {
            self.calls()
                .into_iter()
                .filter(|c| c.starts_with("repair:"))
                .collect()
        }
    }

    #[async_trait]
    impl RepairTickExec for Fake {
        async fn target(&self, id: i64) -> Result<Option<Target>, IpcError> {
            self.calls.lock().unwrap().push(format!("target:{id}"));
            Ok(Some(Target {
                dirs: vec![format!("/wt/{id}")],
                branch: format!("b{id}"),
            }))
        }
        async fn missing(
            &self,
            host: &str,
            targets: &[Vec<String>],
        ) -> Result<HashSet<usize>, IpcError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("missing:{host}:{}", targets.len()));
            let gone = self.missing.lock().unwrap().clone();
            Ok(targets
                .iter()
                .enumerate()
                .filter(|(_, d)| {
                    d[0].strip_prefix("/wt/")
                        .and_then(|i| i.parse::<i64>().ok())
                        .is_some_and(|id| gone.contains(&id))
                })
                .map(|(i, _)| i)
                .collect())
        }
        async fn repair(&self, id: i64) -> Result<RepairReport, IpcError> {
            self.calls.lock().unwrap().push(format!("repair:{id}"));
            let res = self
                .results
                .lock()
                .unwrap()
                .remove(&id)
                .unwrap_or_else(|| Ok(report_with(id, &["git worktree add -- /wt b"])));
            let s = self.store.lock().unwrap();
            match &res {
                Ok(rep) if !rep.actions.is_empty() => {
                    s.insert_session_event(
                        id,
                        repair::EVENT_REPAIRED,
                        Some(&repair::event_detail(rep)),
                    )
                    .unwrap();
                }
                Err(e) if self.records_failures => {
                    s.insert_session_event(id, repair::EVENT_REPAIR_FAILED, Some(&e.message))
                        .unwrap();
                }
                _ => {}
            }
            res
        }
    }

    /// Store with reachable `local` + `remote` hosts, one project, and one
    /// running work session per `(host, tmux name)` with its own worktree
    /// row. Returns the session ids in order.
    fn seed(sessions: &[(&str, &str)]) -> (Arc<Mutex<Store>>, Vec<i64>) {
        let s = Store::open_in_memory().unwrap();
        for h in ["local", "remote"] {
            s.upsert_host(h).unwrap();
            s.update_host_probe(h, true, None, None, 1).unwrap();
        }
        let pid = s.upsert_project("o", "r", "/repo").unwrap();
        let mut ids = Vec::new();
        for (host, name) in sessions {
            let wid = s
                .upsert_worktree(pid, name, &format!("/repo/.worktrees/{name}"), Some(name))
                .unwrap();
            let id = s
                .upsert_session(name, host, Some(pid), Some(wid), 1, 1, "running", None)
                .unwrap();
            s.set_worktree_key(id, Some(name)).unwrap();
            ids.push(id);
        }
        (Arc::new(Mutex::new(s)), ids)
    }

    fn events(store: &Mutex<Store>, id: i64, kind: &str) -> usize {
        store
            .lock()
            .unwrap()
            .list_session_events(id, 50)
            .unwrap()
            .iter()
            .filter(|e| e.kind == kind)
            .count()
    }

    #[tokio::test]
    async fn setting_off_makes_no_calls() {
        let (store, ids) = seed(&[("local", "a")]);
        let fake = Fake::new(&store, &ids);
        let off = RepairTickConfig {
            enabled: false,
            ..ON
        };
        assert_eq!(
            run_with(&store, &fake, &off, 100).await,
            RepairTickReport::default()
        );
        assert!(fake.calls().is_empty(), "{:?}", fake.calls());
        // And the stored default is off.
        assert!(!RepairTickConfig::from_store(&store.lock().unwrap()).enabled);
    }

    #[tokio::test]
    async fn on_repairs_a_vanished_dir_with_one_check_per_host() {
        let (store, ids) = seed(&[("local", "a"), ("local", "b"), ("remote", "c")]);
        let fake = Fake::new(&store, &[ids[1]]);
        let rep = run_with(&store, &fake, &ON, 100).await;
        assert_eq!(rep.hosts_checked, 2);
        assert_eq!(rep.missing, 1);
        assert_eq!(rep.attempted, 1);
        assert_eq!(rep.repaired, 1);
        assert_eq!(fake.repairs(), vec![format!("repair:{}", ids[1])]);
        let checks: Vec<_> = fake
            .calls()
            .into_iter()
            .filter(|c| c.starts_with("missing:"))
            .collect();
        assert_eq!(checks, vec!["missing:local:2", "missing:remote:1"]);
        // The repair's own event is not duplicated by the tick.
        assert_eq!(events(&store, ids[1], repair::EVENT_REPAIRED), 1);
        assert_eq!(events(&store, ids[0], repair::EVENT_REPAIRED), 0);
    }

    #[tokio::test]
    async fn controller_safe_kill_review_bg_and_offline_rows_are_skipped() {
        let (store, ids) = seed(&[
            ("local", "ctl"),
            ("local", "sk"),
            ("local", "rev"),
            ("local", "bgs"),
            ("remote", "off"),
            ("local", "ok"),
        ]);
        {
            let s = store.lock().unwrap();
            s.set_setting("controller.host", "local").unwrap();
            s.set_setting("controller.tmux", "ctl").unwrap();
            let c = s.conn_ref();
            c.execute(
                "UPDATE sessions SET safe_kill_state='requested' WHERE id=?1",
                [ids[1]],
            )
            .unwrap();
            c.execute("UPDATE sessions SET kind='review' WHERE id=?1", [ids[2]])
                .unwrap();
            c.execute("UPDATE sessions SET kind='bg' WHERE id=?1", [ids[3]])
                .unwrap();
            s.update_host_probe("remote", false, None, None, 2).unwrap();
        }
        let fake = Fake::new(&store, &ids);
        let rep = run_with(&store, &fake, &ON, 100).await;
        assert_eq!(fake.repairs(), vec![format!("repair:{}", ids[5])]);
        assert_eq!(rep.attempted, 1);
        // Skipped rows are never even resolved or checked.
        for id in &ids[..5] {
            assert!(
                !fake.calls().contains(&format!("target:{id}")),
                "{id}: {:?}",
                fake.calls()
            );
        }
    }

    #[tokio::test]
    async fn per_tick_cap_holds_and_stops_further_hosts() {
        let mut spec: Vec<(&str, String)> = (0..7).map(|i| ("local", format!("l{i}"))).collect();
        spec.push(("remote", "r0".to_string()));
        let refs: Vec<(&str, &str)> = spec.iter().map(|(h, n)| (*h, n.as_str())).collect();
        let (store, ids) = seed(&refs);
        let fake = Fake::new(&store, &ids);
        let rep = run_with(&store, &fake, &ON, 100).await;
        assert_eq!(rep.attempted, MAX_REPAIRS_PER_TICK);
        assert_eq!(fake.repairs().len(), MAX_REPAIRS_PER_TICK);
        assert_eq!(rep.over_cap, 2);
        assert!(
            !fake.calls().iter().any(|c| c.starts_with("missing:remote")),
            "no further host is probed once the cap is reached: {:?}",
            fake.calls()
        );
        // The next tick picks up the rest.
        let rep2 = run_with(&store, &fake, &ON, 200).await;
        assert!(rep2.attempted >= 2, "{rep2:?}");
    }

    #[tokio::test]
    async fn repair_required_is_recorded_stamped_and_backed_off_until_the_row_changes() {
        let (store, ids) = seed(&[("local", "a")]);
        let id = ids[0];
        let fake = Fake::new(&store, &[id]);
        fake.results.lock().unwrap().insert(
            id,
            Err(IpcError::new(
                codes::E_REPAIR_REQUIRED,
                "the workspace at /wt needs an explicit repair",
            )),
        );

        let rep = run_with(&store, &fake, &ON, 100).await;
        assert_eq!((rep.attempted, rep.failed), (1, 1));
        // The repair wrote no event for E_REPAIR_REQUIRED: the tick does.
        assert_eq!(events(&store, id, repair::EVENT_REPAIR_FAILED), 1);
        let (sig, at) = backoff_of(&store.lock().unwrap(), id).expect("stamped");
        assert_eq!(at, 100);
        assert!(sig.starts_with("local|"), "{sig}");

        // Later ticks: same row ⇒ no attempt, no new event.
        for now in [700, 1300] {
            let rep = run_with(&store, &fake, &ON, now).await;
            assert_eq!((rep.attempted, rep.backed_off), (0, 1));
        }
        assert_eq!(fake.repairs().len(), 1);
        assert_eq!(events(&store, id, repair::EVENT_REPAIR_FAILED), 1);

        // The row changes (worktree key re-stamped) ⇒ retried.
        store
            .lock()
            .unwrap()
            .set_worktree_key(id, Some("a2"))
            .unwrap();
        let rep = run_with(&store, &fake, &ON, 1900).await;
        assert_eq!(rep.attempted, 1);
        assert_eq!(fake.repairs().len(), 2);
        assert!(backoff_of(&store.lock().unwrap(), id).is_none(), "repaired");
    }

    #[tokio::test]
    async fn backoff_clears_once_the_directory_is_back() {
        let (store, ids) = seed(&[("local", "a")]);
        let id = ids[0];
        let fake = Fake::new(&store, &[id]);
        fake.results.lock().unwrap().insert(
            id,
            Err(IpcError::new(codes::E_BRANCH_CHECKED_OUT, "in main")),
        );
        run_with(&store, &fake, &ON, 100).await;
        assert!(backoff_of(&store.lock().unwrap(), id).is_some());
        fake.missing.lock().unwrap().clear();
        run_with(&store, &fake, &ON, 700).await;
        assert!(backoff_of(&store.lock().unwrap(), id).is_none());
        assert_eq!(fake.repairs().len(), 1);
    }

    #[tokio::test]
    async fn transient_failures_are_retried_and_never_double_recorded() {
        let (store, ids) = seed(&[("local", "a")]);
        let id = ids[0];
        let mut fake = Fake::new(&store, &[id]);
        fake.records_failures = true;
        fake.results
            .lock()
            .unwrap()
            .insert(id, Err(IpcError::new(codes::E_HOST_OFFLINE, "down")));
        run_with(&store, &fake, &ON, 100).await;
        assert!(backoff_of(&store.lock().unwrap(), id).is_none());
        assert_eq!(
            events(&store, id, repair::EVENT_REPAIR_FAILED),
            1,
            "the repair's own event is not duplicated"
        );
        run_with(&store, &fake, &ON, 700).await;
        assert_eq!(fake.repairs().len(), 2, "retried at the next interval");
    }

    #[test]
    fn transient_codes_are_transport_only() {
        for c in [codes::E_HOST_OFFLINE, codes::E_SSH, codes::E_TIMEOUT] {
            assert!(is_transient(c), "{c}");
        }
        for c in [
            codes::E_REPAIR_REQUIRED,
            codes::E_REPO_MISSING,
            codes::E_BRANCH_CHECKED_OUT,
            codes::E_WORKSPACE_LOCKED,
            codes::E_REPAIR_FAILED,
        ] {
            assert!(!is_transient(c), "{c}");
        }
    }

    #[test]
    fn due_respects_the_interval() {
        assert!(due(None, 600));
        assert!(!due(Some(std::time::Instant::now()), 600));
        assert!(due(Some(std::time::Instant::now()), 0));
    }

    // ── the policy the tick runs never plans a destructive step ─────────

    const WT: &str = "/repo/.claude/worktrees/feat";

    fn tick_spec() -> WorkspaceSpec {
        WorkspaceSpec {
            host_alias: "local".into(),
            tmux_name: "dev-x".into(),
            project_root: "/repo".into(),
            worktree: Some(WorktreeSpec {
                name: "feat".into(),
                path: WT.into(),
                branch: "feat".into(),
                path_is_guess: false,
                row_is_local: true,
            }),
            base_branch: None,
            pane_cmd: "claude".into(),
            session_id: Some(1),
            project_id: Some(1),
        }
    }

    fn main_entry() -> RegisteredWorktree {
        RegisteredWorktree {
            path: "/repo".into(),
            branch: Some("main".into()),
            ..Default::default()
        }
    }

    /// Worktree dir gone, not registered, branch exists locally, the live
    /// pane's cwd reported missing.
    fn gone() -> Probe {
        Probe {
            root_exists: true,
            root_git: true,
            root_gitdir_ok: true,
            branch_local: true,
            default_branch: Some("main".into()),
            tmux_alive: true,
            tmux_cwd: Some(WT.into()),
            tmux_cwd_exists: false,
            worktrees: vec![main_entry()],
            ..Default::default()
        }
    }

    #[test]
    fn tick_policy_is_create_only_without_tmux() {
        assert_eq!(
            policy_for(TICK_ENTRY),
            Policy::Auto {
                create_dead_tmux: false
            }
        );
    }

    #[test]
    fn tick_policy_never_plans_a_destructive_step() {
        let policy = policy_for(TICK_ENTRY);
        let spec = tick_spec();
        let registered_gone = Probe {
            worktrees: vec![
                main_entry(),
                RegisteredWorktree {
                    path: WT.into(),
                    branch: Some("feat".into()),
                    prunable: true,
                    ..Default::default()
                },
            ],
            ..gone()
        };
        let no_branch = Probe {
            branch_local: false,
            branch_remote: false,
            ..gone()
        };
        let tmux_dead = Probe {
            tmux_alive: false,
            tmux_dead: true,
            tmux_cwd: None,
            ..gone()
        };
        let elsewhere = Probe {
            worktrees: vec![
                main_entry(),
                RegisteredWorktree {
                    path: "/elsewhere/feat".into(),
                    branch: Some("feat".into()),
                    ..Default::default()
                },
            ],
            ..gone()
        };
        let stale_link = Probe {
            wt_exists: true,
            wt_git: true,
            ..gone()
        };
        let empty_leftover = Probe {
            wt_exists: true,
            wt_empty: true,
            ..gone()
        };
        let remote_branch = Probe {
            branch_local: false,
            branch_remote: true,
            ..gone()
        };
        let cases = [
            ("gone", gone(), true),
            ("remote_branch", remote_branch, true),
            ("tmux_dead", tmux_dead, true),
            ("registered_gone", registered_gone, false),
            ("no_branch", no_branch, false),
            ("elsewhere", elsewhere, false),
            ("stale_link", stale_link, false),
            ("empty_leftover", empty_leftover, false),
        ];
        for (name, probe, creates) in cases {
            let p = plan(&spec, &probe, policy).unwrap_or_else(|e| panic!("{name}: {e}"));
            for s in &p.steps {
                assert!(
                    matches!(
                        s,
                        Step::AddWorktree {
                            from: BranchSource::Local | BranchSource::Remote,
                            ..
                        }
                    ),
                    "{name}: destructive or tmux step planned: {s:?}"
                );
            }
            assert_eq!(!p.steps.is_empty(), creates, "{name}: {:?}", p.steps);
            assert_eq!(p.needs_explicit_repair, !creates, "{name}");
        }
    }

    // ── the batched directory check ─────────────────────────────────────

    #[test]
    fn dir_check_script_quotes_every_path_and_parses_indices() {
        let targets = vec![
            vec!["/h/a".to_string()],
            vec!["/h/b b'x".to_string(), "/h/.worktrees/b".to_string()],
        ];
        let s = dir_check_script(&targets);
        assert!(s.contains(&format!("[ -d {} ]", quote("/h/b b'x"))), "{s}");
        assert!(s.contains(" || "), "{s}");
        assert_eq!(
            parse_dir_check(
                "@@fleet_missing=1\n@@fleet_missing=9\n@@fleet_dircheck_done\n",
                2
            )
            .unwrap(),
            HashSet::from([1])
        );
        assert!(parse_dir_check("@@fleet_missing=0\n", 2).is_err());
    }

    #[tokio::test]
    async fn check_missing_goes_over_ssh_as_one_quoted_script() {
        use crate::ssh_fake::{FakeSsh, Match, Reply};
        let fake = FakeSsh::new();
        fake.on_host(
            "mefistos",
            Match::script_contains(DONE_MARKER),
            Reply::ok("@@fleet_missing=1\n@@fleet_dircheck_done\n"),
        );
        let targets = vec![vec!["/h/a".to_string()], vec!["/h/b".to_string()]];
        let got = check_missing(&fake, "mefistos", &targets).await.unwrap();
        assert_eq!(got, HashSet::from([1]));
        let calls = fake.calls_for("mefistos");
        assert_eq!(calls.len(), 1, "one call per host: {:?}", fake.commands());
        assert_eq!(calls[0].args[0], "bash");
        assert_eq!(
            calls[0].script().as_deref(),
            Some(dir_check_script(&targets).as_str())
        );

        fake.unreachable("down");
        assert!(check_missing(&fake, "down", &targets).await.is_err());
    }

    #[tokio::test]
    async fn check_missing_runs_locally_against_real_directories() {
        let base = std::env::temp_dir().join(format!("fleet-dircheck-{}", std::process::id()));
        let present = base.join("has space");
        std::fs::create_dir_all(&present).unwrap();
        let targets = vec![
            vec![present.to_string_lossy().into_owned()],
            vec![base.join("gone").to_string_lossy().into_owned()],
            vec![
                base.join("gone2").to_string_lossy().into_owned(),
                present.to_string_lossy().into_owned(),
            ],
        ];
        let fake = crate::ssh_fake::FakeSsh::new();
        let got = check_missing(&fake, "local", &targets).await.unwrap();
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(got, HashSet::from([1]));
        assert!(fake.calls().is_empty(), "local never goes over ssh");
    }

    #[test]
    fn guessed_remote_paths_also_accept_the_dot_worktrees_layout() {
        let mut spec = tick_spec();
        let w = spec.worktree.as_mut().unwrap();
        w.path_is_guess = true;
        let t = target_from_spec(&spec).unwrap();
        assert_eq!(
            t.dirs,
            vec![WT.to_string(), "/repo/.worktrees/feat".to_string()]
        );
        spec.worktree = None;
        assert!(target_from_spec(&spec).is_none(), "main checkout: never");
    }

    // ── after a real reconcile pass (ReconcileDeps fake exec) ───────────

    struct ListTmux {
        sessions: Vec<crate::tmux::TmuxSession>,
        hang: bool,
    }

    #[async_trait]
    impl crate::tmux::TmuxExec for ListTmux {
        async fn list_sessions(&self) -> Result<Vec<crate::tmux::TmuxSession>, IpcError> {
            if self.hang {
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
            Ok(self.sessions.clone())
        }
        async fn new_session(&self, _: &str, _: &std::path::Path, _: &str) -> Result<(), IpcError> {
            panic!("the tick never creates tmux sessions")
        }
        async fn kill_session(&self, _: &str) -> Result<(), IpcError> {
            panic!("the tick never kills tmux sessions")
        }
        async fn rename_session(&self, _: &str, _: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn restart_session(&self, _: &str, _: &str) -> Result<(), IpcError> {
            panic!("the tick never restarts panes")
        }
        async fn capture_pane(&self, _: &str) -> Result<String, IpcError> {
            Ok(String::new())
        }
        async fn capture_pane_scrollback(&self, _: &str, _: u32) -> Result<String, IpcError> {
            Ok(String::new())
        }
        async fn list_claude_agents(&self) -> Vec<crate::claude_agents::ClaudeAgentRow> {
            Vec::new()
        }
    }

    #[tokio::test]
    async fn tick_follows_the_reachability_the_reconcile_pass_stamped() {
        let (store, ids) = seed(&[("local", "a"), ("remote", "b")]);
        let live = crate::tmux::TmuxSession {
            name: "a".into(),
            created: 1,
            last_activity: 1,
            attached: false,
            path: "/repo/.worktrees/a".into(),
        };
        let deps = crate::service::sessions::ReconcileDeps::fake(
            move |alias| {
                Box::new(ListTmux {
                    sessions: if alias == "local" {
                        vec![live.clone()]
                    } else {
                        Vec::new()
                    },
                    hang: alias != "local",
                })
            },
            Duration::from_millis(150),
        );
        crate::service::sessions::reconcile_sessions_with(&store, &deps)
            .await
            .expect("pass completes");
        let fake = Fake::new(&store, &ids);
        run_with(&store, &fake, &ON, 100).await;
        assert_eq!(
            fake.repairs(),
            vec![format!("repair:{}", ids[0])],
            "the host whose probe timed out is skipped: {:?}",
            fake.calls()
        );
    }
}
