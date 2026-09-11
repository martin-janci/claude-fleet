use crate::cancel::{CancelGuard, CancellationRegistry};
use crate::ipc_error::{codes, IpcError};
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::{HostReconcile, HostRow, ProjectRow, ReconcileSession, SessionRow, Store};
use crate::tmux::{LocalTmux, RemoteTmux, TmuxExec};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// Number of pane lines captured per work session for the reconcile intel
/// probe. Eight lines covers the REPL footer (status bar / context %) plus the
/// last tool line or prompt without dragging in scrollback.
const PANE_TAIL_LINES: u32 = 8;

/// Hard wall-clock cap on a single host's reconcile probe. Without it, a wedged
/// SSH ControlMaster (see `ssh.rs` `mux_opts`) leaves `tmux.list_sessions`
/// awaiting forever — and because the multi-host reconcile awaits EVERY probe
/// before `list_sessions` can return, one hung host would block the whole load
/// path and leave the sidebar empty for ALL hosts, including the healthy
/// `local` one. On elapse we synthesize a probe error, routing the host through
/// the existing "unreachable, keep last-known sessions" branch. Set generously
/// so a healthy host with many sessions (each pane capture is a sequential
/// round-trip) never false-trips; on a real wedge the ssh-layer wall clock
/// (`SshClient::run` → `E_SSH_TIMEOUT`) usually fires first and resets the
/// master.
const HOST_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Default cadence (seconds) for the background reconcile tick, and the
/// freshness window `list_sessions` serves cached rows within when the tick
/// is disabled. Overridden by the `reconcile.interval_secs` setting.
pub const DEFAULT_RECONCILE_INTERVAL_SECS: i64 = 20;

/// Pure: resolve the reconcile interval from the raw `reconcile.interval_secs`
/// setting value. `None`/garbage falls back to the 20s default; an explicit
/// `0` (or negative) is surfaced verbatim — it disables the proactive tick
/// (see `reconcile_tick_interval`). Shared by the tick spawner in `lib.rs`
/// and the `list_sessions` freshness window so the two can never disagree.
pub fn read_reconcile_interval_secs(raw: Option<String>) -> i64 {
    // Same registry the Settings dialog writes through, so the spec's
    // default / validation (non-negative seconds) cannot drift from here.
    crate::service::settings::resolve(
        crate::service::settings::RECONCILE_INTERVAL_SECS,
        raw.as_deref(),
    )
    .parse::<i64>()
    .unwrap_or(DEFAULT_RECONCILE_INTERVAL_SECS)
}

/// Map of `tmux_name` → analyzed pane intel, gathered off-lock during a host
/// probe. A name absent from the map (capture failed) leaves the session's
/// intel fields untouched (COALESCE in the upsert preserves prior values).
type PaneIntelMap = std::collections::HashMap<String, crate::service::pane_intel::PaneIntel>;

/// Map of `tmux_name` → PR probe result, gathered off-lock (see `outcome.rs`).
type PrInfoMap = std::collections::HashMap<String, crate::service::outcome::PrInfo>;

/// Runs one shell script on a host and returns its stdout. Abstracted so the
/// reconcile PR probe (and the playbook / GC helpers) are testable without a
/// real host. The production impl is `RealHostShell`.
#[async_trait::async_trait]
pub(crate) trait HostShell: Send + Sync {
    async fn run_script(&self, host: &str, script: &str) -> Result<String, IpcError>;
}

/// `bash -lc <script>` locally or over ssh, bounded by `timeout`.
pub(crate) struct RealHostShell {
    ssh: Arc<SshClient>,
    timeout: std::time::Duration,
}

#[async_trait::async_trait]
impl HostShell for RealHostShell {
    async fn run_script(&self, host: &str, script: &str) -> Result<String, IpcError> {
        let out = run_host_script(&self.ssh, host, script, self.timeout).await?;
        if !out.status.success() {
            return Err(IpcError::new(
                "E_SHELL",
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// A shell that always fails — the default for test deps that don't exercise
/// the PR probe (a failed probe leaves the stored outcome fields untouched).
#[cfg(test)]
pub(crate) struct NoHostShell;

#[cfg(test)]
#[async_trait::async_trait]
impl HostShell for NoHostShell {
    async fn run_script(&self, _host: &str, _script: &str) -> Result<String, IpcError> {
        Err(IpcError::new("E_SHELL", "no shell in this test"))
    }
}

/// Run `script` through `bash -lc` on `host` (local spawn or ssh), bounded by
/// `timeout`. Every value interpolated into `script` must already be quoted
/// by the caller; the script itself is quoted here for the ssh hop. Shared by
/// the PR probe, the stuck playbooks and the GC sweeper.
pub(crate) async fn run_host_script(
    ssh: &Arc<SshClient>,
    host: &str,
    script: &str,
    timeout: std::time::Duration,
) -> Result<std::process::Output, IpcError> {
    crate::validate::host_alias(host)?;
    if host == "local" {
        let child = tokio::process::Command::new("bash")
            .args(["-lc", script])
            .output();
        match tokio::time::timeout(timeout, child).await {
            Ok(res) => res.map_err(|e| IpcError::new("E_SHELL", format!("spawn bash: {e}"))),
            Err(_) => Err(IpcError::new(
                "E_TIMEOUT",
                format!("local script exceeded {}s", timeout.as_secs()),
            )),
        }
    } else {
        ssh.run(host, &["bash", "-lc", &quote(script)], timeout)
            .await
    }
}

/// Wall clock for one host's PR probe script (one `gh pr view` per due
/// session, sequential). Runs AFTER the reachability probe, outside
/// `HOST_PROBE_TIMEOUT`, so a slow GitHub API can never flip a host to
/// unreachable.
const PR_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Most sessions probed for a PR per host per pass. Bounds the script's
/// worst case (`PR_PROBE_BATCH` sequential `gh` calls); the rest are picked
/// up on later passes as the per-session cache expires.
const PR_PROBE_BATCH: usize = 12;

/// One host's probe result, carried from the off-lock probe task to the
/// under-lock writer.
struct HostProbe {
    host: HostRow,
    result: Result<Vec<crate::tmux::TmuxSession>, IpcError>,
    agent_rows: Vec<crate::claude_agents::ClaudeAgentRow>,
    intel: PaneIntelMap,
    /// `tmux_name → gh pr view` result for the sessions probed THIS pass
    /// (PROD-5). A name absent from the map was not probed (cache still
    /// fresh, host has no `gh`, or the probe failed) and keeps its stored
    /// `pr_url` / `ci_status`.
    pr_info: PrInfoMap,
    /// Unix-epoch second the probe STARTED. Forwarded as
    /// `HostReconcile::probe_started_at` so the writer never ghosts a row that
    /// a newer probe (e.g. `new_session`'s own reconcile) stamped after this
    /// probe listed tmux (BE-3).
    started_at: i64,
}

/// Executor factory + probe budget for the reconcile core. Production uses
/// `exec_for` (local tmux or ssh-wrapped tmux) under `HOST_PROBE_TIMEOUT`;
/// tests inject fakes so the fan-out, the gate and the ghost guard are
/// exercisable without a real host (BE-7).
pub(crate) struct ReconcileDeps {
    exec: ExecFactory,
    probe_timeout: std::time::Duration,
    /// Shell used for the per-host `gh pr view` probe (PROD-5).
    shell: Arc<dyn HostShell>,
    /// Per-session probe throttle; production shares one process-wide cache,
    /// tests get a fresh one per deps.
    pr_cache: Arc<crate::service::outcome::PrProbeCache>,
}

/// `host alias → tmux executor` factory used by `ReconcileDeps`.
type ExecFactory = Box<dyn Fn(&str) -> Box<dyn TmuxExec> + Send + Sync>;

impl ReconcileDeps {
    fn real(ssh: &Arc<SshClient>) -> Arc<Self> {
        let ssh = Arc::clone(ssh);
        let shell = Arc::new(RealHostShell {
            ssh: Arc::clone(&ssh),
            timeout: PR_PROBE_TIMEOUT,
        });
        Arc::new(Self {
            exec: Box::new(move |alias| exec_for(alias, &ssh)),
            probe_timeout: HOST_PROBE_TIMEOUT,
            shell,
            pr_cache: crate::service::outcome::pr_probe_cache(),
        })
    }

    #[cfg(test)]
    pub(crate) fn fake(
        exec: impl Fn(&str) -> Box<dyn TmuxExec> + Send + Sync + 'static,
        probe_timeout: std::time::Duration,
    ) -> Arc<Self> {
        Self::fake_with_shell(exec, probe_timeout, Arc::new(NoHostShell))
    }

    /// Test deps with an injected shell for the PR probe. Each call gets its
    /// own probe cache so tests never share throttle state.
    #[cfg(test)]
    fn fake_with_shell(
        exec: impl Fn(&str) -> Box<dyn TmuxExec> + Send + Sync + 'static,
        probe_timeout: std::time::Duration,
        shell: Arc<dyn HostShell>,
    ) -> Arc<Self> {
        Arc::new(Self {
            exec: Box::new(exec),
            probe_timeout,
            shell,
            pr_cache: Arc::new(crate::service::outcome::PrProbeCache::new(
                crate::service::outcome::PR_PROBE_TTL,
            )),
        })
    }
}

/// Shared overlap guard + freshness marker for the fleet-wide reconcile.
///
/// Every entry point that can start a full pass — the background tick, the
/// `list_sessions` Tauri command, the MCP `list_sessions` tool — goes through
/// the same gate, so at most ONE pass runs at a time process-wide (BE-2).
/// A caller that finds a pass already running is served the stored rows
/// instead of stacking a second N-host probe behind it. `last_completed`
/// lets `list_sessions` skip the probe entirely while the last pass is still
/// within the configured interval.
///
/// The gate is a process-wide static (`reconcile_gate()`) rather than Tauri
/// managed state because the MCP tools and the commands only hand the service
/// a `&Mutex<Store>` + `&Arc<SshClient>`; tests construct their own instance.
pub struct ReconcileGate {
    running: tokio::sync::Mutex<()>,
    last_completed: std::sync::Mutex<Option<std::time::Instant>>,
    /// Number of full passes that ran to completion (tests use it to prove a
    /// call caused zero / exactly one pass).
    passes: std::sync::atomic::AtomicU64,
}

/// RAII token for a running pass. Drop without `complete()` (a pass that
/// errored) leaves `last_completed` untouched so the next caller retries.
pub struct ReconcilePass<'a> {
    _guard: tokio::sync::MutexGuard<'a, ()>,
    gate: &'a ReconcileGate,
}

impl ReconcilePass<'_> {
    fn complete(self) {
        if let Ok(mut last) = self.gate.last_completed.lock() {
            *last = Some(std::time::Instant::now());
        }
        self.gate
            .passes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Default for ReconcileGate {
    fn default() -> Self {
        Self::new()
    }
}

impl ReconcileGate {
    pub fn new() -> Self {
        Self {
            running: tokio::sync::Mutex::new(()),
            last_completed: std::sync::Mutex::new(None),
            passes: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Claim the single pass slot without waiting. `None` ⇒ a pass is
    /// already running.
    pub fn try_begin(&self) -> Option<ReconcilePass<'_>> {
        self.running.try_lock().ok().map(|guard| ReconcilePass {
            _guard: guard,
            gate: self,
        })
    }

    /// `true` when a full pass completed less than `window` ago.
    pub fn is_fresh(&self, window: std::time::Duration) -> bool {
        self.last_completed
            .lock()
            .ok()
            .and_then(|l| *l)
            .map(|t| t.elapsed() < window)
            .unwrap_or(false)
    }

    /// Completed full passes so far.
    #[cfg(test)]
    pub fn passes(&self) -> u64 {
        self.passes.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// The process-wide gate every production entry point shares.
pub fn reconcile_gate() -> &'static ReconcileGate {
    static GATE: once_cell::sync::Lazy<ReconcileGate> =
        once_cell::sync::Lazy::new(ReconcileGate::new);
    &GATE
}

/// Capture and analyze the pane tail for every live session on a host. Runs
/// off-lock inside the probe task. A failed capture for one session is skipped
/// (no map entry) rather than aborting — reconcile must be robust to a session
/// whose pane just vanished.
async fn capture_pane_intel(
    tmux: &dyn TmuxExec,
    sessions: &[crate::tmux::TmuxSession],
) -> PaneIntelMap {
    let mut map = PaneIntelMap::new();
    for sess in sessions {
        match tmux
            .capture_pane_scrollback(&sess.name, PANE_TAIL_LINES)
            .await
        {
            Ok(tail) if !tail.is_empty() => {
                map.insert(
                    sess.name.clone(),
                    crate::service::pane_intel::analyze(&tail),
                );
            }
            _ => {}
        }
    }
    map
}

pub(crate) fn exec_for(host: &str, ssh: &Arc<SshClient>) -> Box<dyn TmuxExec> {
    if host == "local" {
        Box::new(LocalTmux)
    } else {
        Box::new(RemoteTmux {
            client: Arc::clone(ssh),
            host: host.to_string(),
        })
    }
}

/// Apply one host's probe result to the store. Extracted from the reconcile
/// loop so a per-host write failure can be isolated (logged) without `?`
/// aborting the whole multi-host reconcile. The write itself goes through the
/// transactional `Store::apply_host_reconcile` (one fsync, emit-after-commit).
fn reconcile_write_one_host(
    s: &mut Store,
    probe: &HostProbe,
    projects: &[ProjectRow],
) -> Result<(), IpcError> {
    let host = &probe.host;
    let paths = HostPaths::for_host(s, &host.alias);
    let agent_rows = &probe.agent_rows;
    let intel = &probe.intel;
    match &probe.result {
        Ok(live) => {
            let mut keep: Vec<String> = Vec::with_capacity(live.len());
            let mut sessions: Vec<ReconcileSession> = Vec::with_capacity(live.len());
            // ── Task G: reconcile transition-detection (event timeline) ──
            // The PRIOR stored `(claude_status, stuck_kind)` of every already
            // known session, read before the write. Events are derived after
            // the write from the STORED values, never from this pass's
            // candidates: the upsert COALESCEs a missing status onto the
            // stored one and the hook guard keeps a hook-stamped status, so a
            // candidate that differs from the prior row does not mean the row
            // changed. `s` is the store guard held for this whole function, so
            // no other writer lands between this read, the write and the
            // read-back.
            let mut priors: Vec<(String, Option<String>, Option<String>)> = Vec::new();
            for sess in live {
                keep.push(sess.name.clone());
                let project_id =
                    find_project_id_for_path(projects, &host.alias, &sess.path, &paths);
                // Preservation invariant: if the session already has an
                // account_uuid in the DB, keep it; only capture the host's
                // current account for newly-discovered sessions.
                let account_uuid = s
                    .get_session_account(&host.alias, &sess.name)?
                    .or_else(|| host.account_uuid.clone());
                let worktree_key = worktree_key_for_host(&sess.path.to_string_lossy(), &paths);
                // Match the running Claude agent by name (sessions launched
                // with `--name <tmux_name>`) or, for older sessions without a
                // name, by a unique cwd — so `recreate`/`restart` can resume
                // the exact conversation instead of "most recent for the cwd".
                let agent = crate::claude_agents::find_for_session(
                    agent_rows,
                    &sess.name,
                    &sess.path.to_string_lossy(),
                    host.alias == "local",
                );
                // Pane-tail intel from the off-lock probe (may be absent if the
                // capture failed — then all four intel fields stay None and the
                // upsert's COALESCE preserves the session's prior values).
                let pane = intel.get(&sess.name);
                // PR probe result for this pass (PROD-5). `pr_observed`
                // makes the values authoritative so a closed PR's stale
                // link clears; an unprobed session keeps its stored fields.
                let pr = probe.pr_info.get(&sess.name);
                let agent_status =
                    known_agent_status(&sess.name, agent.and_then(|a| a.status.as_deref()));
                // Prefer the authoritative `claude agents` status; fall back to
                // the status derived from the pane tail only when it is absent
                // (or outside the documented vocabulary).
                let claude_status = agent_status
                    .or_else(|| pane.and_then(|p| p.derived_status).map(|s| s.to_string()));
                let stuck_kind = pane.and_then(|p| p.stuck.map(|k| k.as_str().to_string()));
                // Transition-detection: remember the PRIOR stored values (the
                // upsert below overwrites them). A read failure or a first
                // sighting just skips detection for this session.
                if let Ok(Some(prior)) = s.get_session(&sess.name, &host.alias) {
                    priors.push((sess.name.clone(), prior.claude_status, prior.stuck_kind));
                }
                sessions.push(ReconcileSession {
                    tmux_name: &sess.name,
                    project_id,
                    created_at: sess.created,
                    last_activity_at: sess.last_activity,
                    account_uuid,
                    worktree_key,
                    claude_session_id: agent.and_then(|a| a.session_id.clone()),
                    claude_status,
                    effort_level: None, // not in claude agents --json; reserved for future
                    pr_url: pr.and_then(|p| p.pr_url.clone()),
                    current_activity: pane.and_then(|p| p.activity.clone()),
                    context_pct: pane.and_then(|p| p.context_pct),
                    stuck_kind,
                    // Pane captured this pass ⇒ stuck_kind is authoritative and a
                    // None clears any stale flag; a failed capture (pane absent)
                    // leaves intel_observed false so the prior flag is preserved.
                    intel_observed: pane.is_some(),
                    ci_status: pr.and_then(|p| p.ci_status.clone()),
                    pr_observed: pr.is_some(),
                });
            }
            s.apply_host_reconcile(HostReconcile {
                alias: &host.alias,
                reachable: true,
                claude_version: host.claude_version.as_deref(),
                tmux_version: host.tmux_version.as_deref(),
                last_pinged_at: now_unix(),
                probe_started_at: probe.started_at,
                sessions: &sessions,
                keep: &keep,
            })?;
            // Task G: the write has committed — read each known row back and
            // record a transition only where the STORED value changed.
            // Append-only and best-effort — a failed insert is logged and
            // skipped, never blocking reconcile.
            for (tmux_name, old_status, old_stuck) in &priors {
                let Ok(Some(row)) = s.get_session(tmux_name, &host.alias) else {
                    continue;
                };
                let mut events: Vec<(&str, Option<&str>)> = Vec::new();
                if row.claude_status != *old_status {
                    events.push(("status_change", row.claude_status.as_deref()));
                }
                // A newly-set (or changed) stuck_kind is the alert-worthy
                // event; clearing it back to None is not recorded.
                if row.stuck_kind.is_some() && row.stuck_kind != *old_stuck {
                    events.push(("stuck", row.stuck_kind.as_deref()));
                }
                for (kind, detail) in events {
                    if let Err(e) = s.insert_session_event(row.id, kind, detail) {
                        eprintln!(
                            "[reconcile] session_event insert failed for {}/{tmux_name}: {e}",
                            host.alias
                        );
                    }
                }
            }
            // SECOND pass: background (`claude --bg`) agents that matched NO tmux
            // session are never in `keep` and would otherwise be invisible.
            // Surface each as a synthetic `kind='bg'` SessionRow so it appears in
            // `list_sessions`. These rows are exempt from the tmux-keyed ghost
            // cleanup (`ghost_and_clean_sessions_in_tx`) — instead they are
            // pruned inside `reconcile_bg_agents` against the current
            // `claude agents --json` result, so dead agents can't accumulate.
            reconcile_bg_agents(s, &host.alias, live, projects, agent_rows)?;
            // Task H: stamp freshness on every session this pass observed live,
            // so a proactive (background) reconcile keeps `last_reconciled_at`
            // current and the UI can dim rows whose host has gone quiet. It is
            // also the BE-3 ghost guard's evidence (`probe_started_at` above).
            // Best-effort: a failure here must not abort reconcile.
            if let Err(e) = s.mark_sessions_reconciled(&host.alias, &keep, now_unix()) {
                eprintln!(
                    "[reconcile] mark_sessions_reconciled failed for {}: {e}",
                    host.alias
                );
            }
        }
        Err(_e) => {
            // Mark host unreachable; surface last-known sessions so the UI
            // can render them dimmed/red. We KEEP them (no delete).
            s.apply_host_reconcile(HostReconcile {
                alias: &host.alias,
                reachable: false,
                claude_version: host.claude_version.as_deref(),
                tmux_version: host.tmux_version.as_deref(),
                last_pinged_at: now_unix(),
                probe_started_at: probe.started_at,
                sessions: &[],
                keep: &[],
            })?;
        }
    }
    Ok(())
}

/// Select the `claude --bg` agents that did NOT correlate to any live tmux
/// session — i.e. real background sessions that have no pane. An agent counts
/// as "matched" if `find_for_session` would resolve some tmux session to it
/// (by name or unique cwd). Agents without a `session_id` are skipped (we can't
/// build a stable sentinel / track them). `is_local` gates the canonical cwd
/// fallback (never for a remote host's paths). Pure so it's unit-testable.
fn unmatched_bg_agents<'a>(
    live: &[crate::tmux::TmuxSession],
    agents: &'a [crate::claude_agents::ClaudeAgentRow],
    is_local: bool,
) -> Vec<&'a crate::claude_agents::ClaudeAgentRow> {
    // Collect the set of agent session_ids that a tmux session resolved to.
    let mut matched: std::collections::HashSet<String> = std::collections::HashSet::new();
    for sess in live {
        if let Some(agent) = crate::claude_agents::find_for_session(
            agents,
            &sess.name,
            &sess.path.to_string_lossy(),
            is_local,
        ) {
            if let Some(id) = agent.session_id.as_deref() {
                matched.insert(id.to_string());
            }
        }
    }
    agents
        .iter()
        .filter(|a| match a.session_id.as_deref() {
            Some(id) => !matched.contains(id),
            None => false,
        })
        .collect()
}

/// Upsert a synthetic `kind='bg'` SessionRow for every background agent that has
/// no tmux session (the reconcile "second pass"), then prune the host's bg rows
/// whose agent is NOT in the current `claude agents --json` result. The prune is
/// two-phase (ghost this pass, hard-delete next pass) via
/// `ghost_and_clean_bg_sessions`, so a transiently-failed agents probe — which
/// comes back as an empty list — only ghosts rows for one cycle instead of
/// deleting them. Per-agent write failures are logged and skipped so one bad
/// row can't abort the others.
fn reconcile_bg_agents(
    s: &Store,
    host_alias: &str,
    live: &[crate::tmux::TmuxSession],
    projects: &[ProjectRow],
    agents: &[crate::claude_agents::ClaudeAgentRow],
) -> Result<(), IpcError> {
    let mut keep: Vec<String> = Vec::new();
    let paths = HostPaths::for_host(s, host_alias);
    for agent in unmatched_bg_agents(live, agents, host_alias == "local") {
        let Some(session_id) = agent.session_id.as_deref() else {
            continue;
        };
        let tmux_name = format!("bg:{session_id}");
        // Keep the sentinel even if the upsert below fails — ghosting an
        // existing row over a transient write error would be wrong.
        keep.push(tmux_name.clone());
        let project_id = agent.cwd.as_deref().and_then(|cwd| {
            find_project_id_for_path(projects, host_alias, std::path::Path::new(cwd), &paths)
        });
        // Same vocabulary filter as tmux rows: an unknown value is logged and
        // dropped (the upsert's COALESCE then keeps the prior status).
        let status = known_agent_status(&tmux_name, agent.status.as_deref());
        if let Err(e) = s.upsert_bg_session(
            host_alias,
            &tmux_name,
            project_id,
            session_id,
            status.as_deref(),
            now_unix(),
        ) {
            eprintln!("[reconcile] bg upsert failed for {host_alias}/{session_id}: {e}");
        }
    }
    if let Err(e) = s.ghost_and_clean_bg_sessions(host_alias, &keep, now_unix()) {
        eprintln!("[reconcile] bg cleanup failed for {host_alias}: {e}");
    }
    Ok(())
}

/// Probe one host off-lock, under a hard wall-clock cap (`deps.probe_timeout`,
/// `HOST_PROBE_TIMEOUT` in production). A wedged SSH ControlMaster can make
/// any of these awaits hang (`ConnectTimeout` does not cover a multiplexed
/// attach onto an existing master; the ssh-layer wall clock is the first line
/// of defence, this cap the second), so the whole probe is bounded. On timeout
/// we return an `Err` probe result, which `reconcile_write_one_host` turns
/// into "host unreachable, keep last-known sessions". Shared by the multi-host
/// reconcile and the single-host refresh so both are bounded identically.
async fn probe_one_host(host: HostRow, paths: HostPaths, deps: &ReconcileDeps) -> HostProbe {
    let tmux = (deps.exec)(&host.alias);
    probe_with_timeout(
        host,
        tmux,
        deps.probe_timeout,
        Some((deps.shell.as_ref(), deps.pr_cache.as_ref(), &paths)),
    )
    .await
}

/// The `gh pr view` probe for one host (PROD-5): pick the live sessions that
/// sit in a github-layout worktree and are due per the cache, run ONE script
/// for all of them, and fold the result into a `tmux_name → PrInfo` map.
/// Best-effort throughout — any failure yields an empty map and the stored
/// outcome fields survive untouched.
async fn probe_pr_info(
    host: &str,
    live: &[crate::tmux::TmuxSession],
    shell: &dyn HostShell,
    cache: &crate::service::outcome::PrProbeCache,
    paths: &HostPaths,
) -> PrInfoMap {
    use crate::service::outcome::{build_pr_probe_script, parse_pr_probe_output, ProbeOutput};
    let live_names: Vec<String> = live.iter().map(|s| s.name.clone()).collect();
    cache.retain_host(host, &live_names);
    let candidates: Vec<(String, String)> = live
        .iter()
        .filter(|s| worktree_key_for_host(&s.path.to_string_lossy(), paths).is_some())
        .map(|s| (s.name.clone(), s.path.to_string_lossy().into_owned()))
        .collect();
    let mut due: Vec<(String, String)> =
        cache.due(host, &candidates).into_iter().cloned().collect();
    if due.is_empty() {
        return PrInfoMap::new();
    }
    due.truncate(PR_PROBE_BATCH);
    let script = build_pr_probe_script(&due);
    let stdout = match shell.run_script(host, &script).await {
        Ok(out) => out,
        Err(e) => {
            eprintln!("[reconcile] pr probe failed on {host}: {e}");
            return PrInfoMap::new();
        }
    };
    match parse_pr_probe_output(&stdout) {
        ProbeOutput::NoGh | ProbeOutput::NoAuth => {
            cache.mark_no_gh(host);
            PrInfoMap::new()
        }
        ProbeOutput::Results(map) => {
            // Every target the script ran for is throttled, observed or not:
            // a transport failure must not be retried on every 20 s pass.
            cache.mark_probed(host, due.iter().map(|(n, _)| n.as_str()));
            map
        }
    }
}

/// Inner probe with an injectable executor + timeout, so the wedged-host path
/// is unit-testable without real ssh. See `probe_one_host` for the rationale.
async fn probe_with_timeout(
    host: HostRow,
    tmux: Box<dyn TmuxExec>,
    timeout: std::time::Duration,
    pr_probe: Option<(
        &dyn HostShell,
        &crate::service::outcome::PrProbeCache,
        &HostPaths,
    )>,
) -> HostProbe {
    // Recorded BEFORE the first await: this is the instant the probe's view of
    // the host stops being current (BE-3 ghost guard).
    let started_at = now_unix();
    let probe = async {
        let tmux_result = tmux.list_sessions().await;
        let agent_rows = tmux.list_claude_agents().await;
        // One pane-tail read per live session, parsed into reconcile intel.
        let intel = match &tmux_result {
            Ok(live) => capture_pane_intel(tmux.as_ref(), live).await,
            Err(_) => PaneIntelMap::new(),
        };
        (tmux_result, agent_rows, intel)
    };
    let mut probe = match tokio::time::timeout(timeout, probe).await {
        Ok((result, agent_rows, intel)) => HostProbe {
            host,
            result,
            agent_rows,
            intel,
            pr_info: PrInfoMap::new(),
            started_at,
        },
        Err(_elapsed) => {
            eprintln!(
                "[reconcile] host {alias} probe exceeded {timeout:?}; marking unreachable (last-known sessions kept)",
                alias = host.alias,
            );
            return HostProbe {
                host,
                result: Err(IpcError::new("E_TIMEOUT", "host probe timed out")),
                agent_rows: Vec::new(),
                intel: PaneIntelMap::new(),
                pr_info: PrInfoMap::new(),
                started_at,
            };
        }
    };
    // The PR probe is its own bounded step AFTER reachability is settled: it
    // talks to GitHub, not to the host, and must never cost the host its
    // "reachable" verdict. On timeout the stored outcome fields survive.
    if let (Ok(live), Some((shell, cache, paths))) = (&probe.result, pr_probe) {
        match tokio::time::timeout(
            PR_PROBE_TIMEOUT,
            probe_pr_info(&probe.host.alias, live, shell, cache, paths),
        )
        .await
        {
            Ok(map) => probe.pr_info = map,
            Err(_elapsed) => eprintln!(
                "[reconcile] pr probe on {} exceeded {PR_PROBE_TIMEOUT:?}; outcome fields kept",
                probe.host.alias
            ),
        }
    }
    probe
}

/// Full fleet pass: probe every non-hidden host in parallel, then apply each
/// host's result under its own short store-lock window. Callers are expected
/// to hold a `ReconcilePass` from the shared gate (see `run_full_reconcile`);
/// this function does not take the gate itself so tests can drive it directly.
pub(crate) async fn reconcile_sessions_with(
    store: &Mutex<Store>,
    deps: &Arc<ReconcileDeps>,
) -> Result<(), IpcError> {
    // 1. Snapshot under lock (brief). Ensure local host exists first.
    let hosts = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        s.upsert_host("local")?;
        s.list_hosts()?
            .into_iter()
            .map(|h| {
                let paths = HostPaths::for_host(&s, &h.alias);
                (h, paths)
            })
            .collect::<Vec<_>>()
    };

    // 2. Fan out probes (off-lock) via JoinSet for parallel execution.
    //    Hidden hosts are skipped here — their last-known sessions are still
    //    surfaced by the final `list_all_sessions` read, without probing.
    //    Each task receives owned data so it satisfies 'static + Send.
    //
    //    Each probe is bounded by `deps.probe_timeout` (see `probe_one_host`):
    //    a single wedged host can no longer stall the collector below and, with
    //    it, the whole load path.
    //
    //    `JoinSet::drop` aborts the futures but does NOT kill spawned ssh
    //    children by itself; the ssh layer's own wall clock
    //    (`SshClient::run_child`) kills and reaps them and resets the master.
    let mut set = tokio::task::JoinSet::new();
    for (host, paths) in hosts.into_iter().filter(|(h, _)| !h.hidden) {
        let deps = Arc::clone(deps);
        set.spawn(async move { probe_one_host(host, paths, &deps).await });
    }

    // Collect per-host probe results. Join errors (task panics) are logged
    // and skipped — they don't abort the rest of reconcile.
    let mut probed: Vec<HostProbe> = Vec::new();
    while let Some(join) = set.join_next().await {
        match join {
            Ok(probe) => probed.push(probe),
            Err(e) => eprintln!("[reconcile] probe task panicked: {e}"),
        }
    }

    // 3. Apply writes, taking the store lock ONCE PER HOST rather than once
    //    for the whole loop (BE-12): a command or the PTY poller waiting on
    //    the store only ever queues behind one host's transaction. Each
    //    host's write-burst goes through `Store::apply_host_reconcile`, which
    //    wraps update_host_probe + upserts + touches + ghosting in ONE
    //    transaction (one fsync) and emits events only AFTER it commits — so
    //    a mid-burst error rolls everything back and emits nothing for that
    //    host.
    //
    //    The project list is identical for every host — fetch it once here
    //    rather than re-querying inside `find_project_id_for_path` per session.
    let projects = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        s.list_projects()?
    };
    for probe in &probed {
        let mut s = store.lock().map_err(|_| IpcError::lock())?;
        // Per-host isolation: one host's DB write failure (e.g. an FK
        // violation on a stale account_uuid) must NOT abort reconcile for
        // every other host. apply_host_reconcile is transactional, so a
        // failed host rolls back cleanly; we log it and carry on.
        if let Err(e) = reconcile_write_one_host(&mut s, probe, &projects) {
            eprintln!("[reconcile] write failed for {}: {e}", probe.host.alias);
        }
    }
    Ok(())
}

/// Claim the gate and run one full pass. Returns `Ok(false)` without probing
/// when another pass is already running (the caller then serves stored rows).
async fn run_full_reconcile(
    store: &Mutex<Store>,
    deps: &Arc<ReconcileDeps>,
    gate: &ReconcileGate,
) -> Result<bool, IpcError> {
    let Some(pass) = gate.try_begin() else {
        return Ok(false);
    };
    reconcile_sessions_with(store, deps).await?;
    pass.complete();
    Ok(true)
}

/// Test access to the private gate entry point, so reconcile tests exercise
/// the real claim → pass → complete sequence.
#[cfg(test)]
pub(crate) async fn run_full_reconcile_for_test(
    store: &Mutex<Store>,
    deps: &Arc<ReconcileDeps>,
    gate: &ReconcileGate,
) -> Result<bool, IpcError> {
    run_full_reconcile(store, deps, gate).await
}

/// Freshness window for `list_sessions`: the configured tick interval, or the
/// default when the tick is disabled (`0`) — pull-only mode still must not
/// re-probe the fleet on every sidebar focus.
fn list_freshness_window(store: &Mutex<Store>) -> std::time::Duration {
    let raw = store
        .lock()
        .ok()
        .and_then(|s| s.get_setting("reconcile.interval_secs").ok().flatten());
    let secs = read_reconcile_interval_secs(raw);
    let secs = if secs <= 0 {
        DEFAULT_RECONCILE_INTERVAL_SECS
    } else {
        secs
    };
    std::time::Duration::from_secs(secs as u64)
}

/// `list_sessions` core with injectable deps/gate/window (tests). See the
/// public `list_sessions` for the policy.
async fn list_sessions_with(
    store: &Mutex<Store>,
    deps: &Arc<ReconcileDeps>,
    gate: &ReconcileGate,
    window: std::time::Duration,
    force: bool,
) -> Result<Vec<SessionRow>, IpcError> {
    if force || !gate.is_fresh(window) {
        // `Ok(false)` ⇒ a pass is in flight; fall through to the stored rows
        // rather than queue a second fleet-wide probe behind it.
        run_full_reconcile(store, deps, gate).await?;
    }
    let s = store.lock().map_err(|_| IpcError::lock())?;
    s.list_all_sessions().map_err(IpcError::from)
}

async fn reconcile_one_host_with(
    store: &Mutex<Store>,
    deps: &ReconcileDeps,
    alias: &str,
) -> Result<(), IpcError> {
    // 1. Snapshot the host under lock (brief).
    let (host, paths) = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        let host = s
            .list_hosts()?
            .into_iter()
            .find(|h| h.alias == alias)
            .ok_or_else(|| IpcError::new("E_NOTFOUND", format!("host {alias} not found")))?;
        let paths = HostPaths::for_host(&s, alias);
        (host, paths)
    };

    // 2. Probe off-lock, under the same hard cap as the multi-host reconcile.
    let probe = probe_one_host(host, paths, deps).await;

    // 3. Apply writes under one brief lock, via the SAME per-host write path
    //    as the multi-host reconcile (single transaction + emit-after-commit).
    let mut s = store.lock().map_err(|_| IpcError::lock())?;
    let projects = s.list_projects()?;
    reconcile_write_one_host(&mut s, &probe, &projects)
}

/// Single-host refresh used after a mutation (`new_session`, `kill`, `rename`,
/// …) so the caller can return the fresh row. Not gated: it is one host, not
/// the fleet, and the BE-3 probe-start guard makes it safe to interleave with
/// a full pass.
pub(crate) async fn reconcile_one_host(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    alias: &str,
) -> Result<(), IpcError> {
    reconcile_one_host_with(store, &ReconcileDeps::real(ssh), alias).await
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Extract `(owner, repo)` from a path that follows the conventional
/// `.../projects/github.com/<owner>/<repo>/...` layout (the same layout
/// `proj-clean` enforces on disk). Remote hosts often store repos under
/// a different prefix (e.g. `/home/mjanci/...` instead of `/Users/...`),
/// but the GitHub portion is stable — so we match into the repo cell
/// regardless of where the path starts.
fn extract_owner_repo(path: &str) -> Option<(String, String)> {
    static RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"/projects/github\.com/([^/]+)/([^/]+)").expect("static regex")
    });
    let caps = RE.captures(path)?;
    Some((
        caps.get(1)?.as_str().to_string(),
        caps.get(2)?.as_str().to_string(),
    ))
}

/// Derive a portable worktree name from a session's cwd. Host-path-independent:
///   - <repo>/.claude/worktrees/<name>[/…]  → Some("<name>")
///   - <repo>/.worktrees/<name>[/…]         → Some("<name>")
///   - <repo> root or any other subdir       → Some("main")
///   - path without a github.com repo segment → None (orphan)
///
/// Both worktree layouts are recognized: `worktree_add_script` and the
/// ecosystem's `proj-clean` use `.worktrees/` *or* `.claude/worktrees/`, and a
/// session living under either must key to its worktree name (not "main"), or
/// recreate/restart would rebuild it at the repo root.
fn worktree_key_for_path(path: &str) -> Option<String> {
    static RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"/projects/github\.com/[^/]+/[^/]+(/.*)?$").expect("static regex")
    });
    let caps = RE.captures(path)?;
    Some(worktree_key_from_remainder(
        caps.get(1).map(|m| m.as_str()).unwrap_or(""),
    ))
}

/// Worktree name from the part of a cwd below the repo directory (`""`,
/// `/src/lib`, `/.worktrees/feat/src`, …).
fn worktree_key_from_remainder(remainder: &str) -> String {
    // Check `.claude/worktrees/` first — it is the more specific marker, and
    // `/.worktrees/` is not a substring of `/.claude/worktrees/`.
    for marker in ["/.claude/worktrees/", "/.worktrees/"] {
        if let Some(idx) = remainder.find(marker) {
            let after = &remainder[idx + marker.len()..];
            if let Some(name) = after.split('/').next() {
                if !name.is_empty() {
                    return name.to_string();
                }
            }
        }
    }
    "main".to_string()
}

/// A host's projects root and layout (the `projects.*` settings), captured
/// under the store lock so cwd → project linking also works off-lock (the PR
/// probe). `local` holds the absolute scan root; remote roots stay
/// unexpanded, since `$HOME` is not known here, so a `~` / `~/rest` root is
/// anchored right after the path's home directory ([`below_home`]). The
/// layout only says where a repo sits under the root; worktree subdirs are
/// recognised separately. `worktrees` holds the host's known worktree
/// checkouts as `(path, project_id)`: for `local` the scan's
/// `git worktree list` (which lists linked worktrees wherever they live), for
/// a remote host what its EnterWorktree hooks reported. A cwd in a linked
/// worktree OUTSIDE the root layout, such as a sibling folder of the repo,
/// still resolves to the repo's project through them.
#[derive(Debug, Clone)]
pub(crate) struct HostPaths {
    root: String,
    layout: crate::projects::Layout,
    worktrees: Vec<(String, i64)>,
}

impl HostPaths {
    pub(crate) fn for_host(s: &Store, alias: &str) -> Self {
        use crate::service::projects::{layout, local_projects_root, project_base_for, LOCAL_HOST};
        let root = if alias == LOCAL_HOST {
            local_projects_root(s).to_string_lossy().into_owned()
        } else {
            project_base_for(s, alias)
        };
        let worktrees = s
            .list_worktrees_on_host(alias)
            .unwrap_or_default()
            .into_iter()
            .map(|w| (w.path, w.project_id))
            .collect();
        HostPaths {
            root,
            layout: layout(s),
            worktrees,
        }
    }

    /// The longest known worktree checkout on this host that contains the cwd
    /// in any of its `spellings`, by whole components, as `(project_id,
    /// matched path length)`; the length lets the caller weigh it against a
    /// project-root match.
    fn project_by_worktree(&self, spellings: &[&str]) -> Option<(i64, usize)> {
        self.worktrees
            .iter()
            .filter(|(wt, _)| {
                !wt.is_empty()
                    && spellings
                        .iter()
                        .any(|p| crate::service::projects::strip_root(p, wt).is_some())
            })
            .max_by_key(|(wt, _)| wt.len())
            .map(|(wt, pid)| (*pid, wt.len()))
    }

    /// Path components below the root, when `path` lies under it. Compares
    /// whole components, so root `/data/git` does not match `/data/git-old`.
    fn below_root<'a>(&self, path: &'a str) -> Option<Vec<&'a str>> {
        let root = self.root.trim_end_matches('/');
        let rest = if root == "~" {
            // The home directory itself is the root. Where home ends is only
            // known for the standard layouts; elsewhere it cannot be told
            // apart from the projects below it, so nothing matches.
            below_home(path)?
        } else if let Some(tail) = root.strip_prefix("~/") {
            match below_home(path) {
                // Anchored right after $HOME: root `~/code` never matches a
                // `/code/` run deeper in the path, nor a user named `code`.
                Some(home_rest) => crate::service::projects::strip_root(home_rest, tail)?,
                // Unrecognised home layout: fall back to the first `/tail/`
                // run anywhere in the path. This can mis-anchor when the home
                // path itself contains that run; the github.com regex
                // fallback in the callers has the same limit.
                None => {
                    let needle = format!("/{tail}/");
                    let idx = path.find(&needle)?;
                    &path[idx + needle.len()..]
                }
            }
        } else if root.starts_with('/') {
            crate::service::projects::strip_root(path, root)?
        } else {
            return None;
        };
        Some(rest.split('/').filter(|c| !c.is_empty()).collect())
    }

    /// `(owner, repo, remainder below the repo)` for a cwd under the root;
    /// `owner` is `None` under the flat layout.
    fn locate<'a>(&self, path: &'a str) -> Option<(Option<&'a str>, &'a str, String)> {
        use crate::projects::Layout;
        let comps = self.below_root(path)?;
        let (owner, repo, rest) = match self.layout {
            Layout::Github if comps.len() >= 2 => (Some(comps[0]), comps[1], &comps[2..]),
            Layout::Flat if !comps.is_empty() => (None, comps[0], &comps[1..]),
            _ => return None,
        };
        let remainder = if rest.is_empty() {
            String::new()
        } else {
            format!("/{}", rest.join("/"))
        };
        Some((owner, repo, remainder))
    }
}

/// The part of an absolute path below its home directory, for the standard
/// home layouts `/home/<u>`, `/Users/<u>`, `/var/home/<u>` and `/root`: `""`
/// for the home itself, `None` for any other path. A remote `$HOME` is not
/// known when paths are matched, so this is how `~` roots are anchored.
fn below_home(path: &str) -> Option<&str> {
    let p = path.strip_prefix('/')?;
    let depth = match p.split('/').next()? {
        "root" => 1,
        "home" | "Users" => 2,
        "var" if p.starts_with("var/home/") => 3,
        _ => return None,
    };
    let mut rest = p;
    for i in 0..depth {
        match rest.split_once('/') {
            Some((head, tail)) if !head.is_empty() => rest = tail,
            None if !rest.is_empty() && i + 1 == depth => rest = "",
            _ => return None,
        }
    }
    Some(rest)
}

/// `worktree_key_for_path` for a cwd on a host with a (possibly custom)
/// projects root / layout. The github.com regex stays the fallback.
fn worktree_key_for_host(path: &str, paths: &HostPaths) -> Option<String> {
    match paths.locate(path) {
        Some((_, _, remainder)) => Some(worktree_key_from_remainder(&remainder)),
        None => worktree_key_for_path(path),
    }
}

/// Match a session's cwd to a known project id. `projects` is passed in by the
/// caller (fetched once per reconcile) rather than queried per session.
pub(crate) fn find_project_id_for_path(
    projects: &[ProjectRow],
    host_alias: &str,
    path: &std::path::Path,
    paths: &HostPaths,
) -> Option<i64> {
    let path_str = path.to_string_lossy();
    if host_alias == "local" {
        // Local paths: component-wise prefix match against the scanned
        // base_path (handles worktrees nested under repos; `/b/x` does not
        // capture `/b/x-build`), in the raw AND the canonical spelling of the
        // cwd. The scan stores physical base_paths, while a pane under a
        // symlinked root can report the logical one; without this every
        // local session there was orphaned (E_NOREPO on recreate/restart).
        // Rows stored before the scan canonicalized still match raw, and heal
        // on the next refresh. One canonicalize per session: a few local
        // stat calls, no await, so fine under the store lock.
        let canon = crate::projects::path_identity::canonical(path);
        let canon_str = canon.to_string_lossy();
        let within = |base: &str| {
            crate::service::projects::strip_root(&path_str, base).is_some()
                || crate::service::projects::strip_root(&canon_str, base).is_some()
        };
        // The most specific match wins: a project whose base contains the
        // cwd, or a checkout from `git worktree list` (a linked worktree
        // outside the layout, such as a sibling folder of the repo, belongs
        // to the repo that lists it). See `most_specific`.
        let by_root = projects
            .iter()
            .filter(|p| within(&p.base_path))
            .max_by_key(|p| p.base_path.len())
            .map(|p| (p.id, p.base_path.len()));
        return most_specific(by_root, paths.project_by_worktree(&[&path_str, &canon_str]));
    }
    // Remote paths: the project located under the host's configured root and
    // layout (owner/repo), weighed against the worktree checkouts this host's
    // hooks reported (`most_specific`), then the conventional
    // `.../projects/github.com/<owner>/<repo>/...` regex. `None` (orphan) if
    // nothing matches.
    let by_layout = remote_project_by_layout(projects, &path_str, paths);
    if let Some(pid) = most_specific(by_layout, paths.project_by_worktree(&[&path_str])) {
        return Some(pid);
    }
    let (owner, repo) = extract_owner_repo(&path_str)?;
    projects
        .iter()
        .find(|p| p.owner == owner && p.repo == repo)
        .map(|p| p.id)
}

/// The more specific of a project-root match and a worktree-row match, each
/// `(project_id, matched path length)`: the longer match wins, and a tie goes
/// to the worktree row. A tie means a project whose base IS that checkout,
/// typically a leftover duplicate of a linked worktree (a sibling folder once
/// scanned as its own repo and kept alive by a session). The worktree row
/// names the repo whose `git worktree list` holds that checkout, which must
/// win.
fn most_specific(root: Option<(i64, usize)>, worktree: Option<(i64, usize)>) -> Option<i64> {
    match (root, worktree) {
        (Some((r, root_len)), Some((w, wt_len))) => Some(if wt_len >= root_len { w } else { r }),
        (root, worktree) => root.or(worktree).map(|(id, _)| id),
    }
}

/// The project of a remote cwd located under the host's configured root and
/// layout, matched by owner/repo (github layout) or a unique repo name
/// (flat), as `(project_id, length of the repo directory prefix of the cwd)`.
fn remote_project_by_layout(
    projects: &[ProjectRow],
    path_str: &str,
    paths: &HostPaths,
) -> Option<(i64, usize)> {
    let (owner, repo, remainder) = paths.locate(path_str)?;
    let repo_len = path_str.len().saturating_sub(remainder.len());
    let pid = match owner {
        Some(o) => projects
            .iter()
            .find(|p| p.owner == o && p.repo == repo)
            .map(|p| p.id),
        None => {
            // Flat: repo name only; never guess between same-named repos.
            let mut same = projects.iter().filter(|p| p.repo == repo);
            match (same.next(), same.next()) {
                (Some(p), None) => Some(p.id),
                _ => None,
            }
        }
    }?;
    Some((pid, repo_len))
}

/// List every session in the fleet.
///
/// Served from the store; a full reconcile pass runs first ONLY when the last
/// completed pass is older than the configured interval AND no pass is
/// currently running (BE-2). Called from the Tauri command, the MCP tool and
/// the frontend on window focus — none of which should be able to stack
/// N-host probes on top of the background tick. Use `reconcile_now` for an
/// explicit refresh that ignores freshness.
pub async fn list_sessions(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<Vec<SessionRow>, IpcError> {
    let window = list_freshness_window(store);
    list_sessions_with(
        store,
        &ReconcileDeps::real(ssh),
        reconcile_gate(),
        window,
        false,
    )
    .await
}

/// `list_sessions` for an explicit user refresh: ignores the freshness window
/// (a pass already in flight is still not duplicated — the stored rows it is
/// about to write are returned instead).
pub async fn refresh_sessions(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<Vec<SessionRow>, IpcError> {
    let window = list_freshness_window(store);
    list_sessions_with(
        store,
        &ReconcileDeps::real(ssh),
        reconcile_gate(),
        window,
        true,
    )
    .await
}

/// Headless, forced reconcile entry point: the background tick (Task H) and
/// any "refresh now" caller.
///
/// Reconcile is Tauri-free: it takes only a `&Mutex<Store>` and a
/// `&Arc<SshClient>`, and all row events are emitted through the store's
/// `EventBus` (see `events.rs`) — NOT a Tauri `AppHandle`. So a `tokio::spawn`ed
/// loop can drive it with the same managed `Arc`s the commands use. Ignores
/// the freshness window but still honours the shared gate: `Ok(false)` means a
/// pass was already running and this call did nothing.
pub async fn reconcile_now(store: &Mutex<Store>, ssh: &Arc<SshClient>) -> Result<bool, IpcError> {
    run_full_reconcile(store, &ReconcileDeps::real(ssh), reconcile_gate()).await
}

/// Pure interval-guard decision for the background reconcile tick.
///
/// Returns the tick interval to use when reconcile should run, or `None` when
/// the feature is disabled. A `reconcile.interval_secs` of `0` (or anything
/// non-positive) disables the proactive tick entirely — reconcile then stays
/// pull-only (list_sessions / per-host paths). Kept pure so it is unit-testable
/// without spawning a timer.
pub fn reconcile_tick_interval(interval_secs: i64) -> Option<std::time::Duration> {
    if interval_secs <= 0 {
        None
    } else {
        Some(std::time::Duration::from_secs(interval_secs as u64))
    }
}

#[derive(Deserialize)]
pub struct RelatedSessionsArgs {
    pub session_id: i64,
}

pub fn related_sessions(
    args: RelatedSessionsArgs,
    store: &Mutex<Store>,
) -> Result<Vec<SessionRow>, IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.list_related_sessions(args.session_id)
        .map_err(IpcError::from)
}

#[derive(Deserialize)]
pub struct NewSessionArgs {
    pub host_alias: String,
    pub project_id: i64,
    pub worktree_id: Option<i64>,
    pub name: String,
    pub call_id: Option<u64>,
    pub new_worktree: Option<String>,
    /// Branch to fork a new worktree from. `None` / empty = the repo's default
    /// branch. Only consulted when `new_worktree` is set. Resolution falls back
    /// to the default branch if the named branch isn't found (see
    /// `worktree_add_script`).
    pub base_branch: Option<String>,
    /// Session kind: `"work"` (default) runs Claude Code in the pane;
    /// `"shell"` runs a plain interactive login shell.
    pub kind: Option<String>,
    /// Optional command run once on start for a `"shell"` session, before
    /// the pane drops to an interactive shell. Ignored for `"work"`.
    pub start_command: Option<String>,
    /// Optional user-supplied sidebar label. Empty / missing -> derive
    /// deterministically from the branch via `humanize::humanize_branch` so
    /// the sidebar never shows the raw `dev-<owner>-<repo>--…` slug. The
    /// in-session agent can refine it later via the `set_friendly_name`
    /// MCP tool.
    pub friendly_name: Option<String>,
}

/// Look up `(owner, repo)` for a given project id.
pub(crate) fn fetch_owner_repo(s: &Store, project_id: i64) -> Result<(String, String), IpcError> {
    let mut stmt = s
        .conn_ref()
        .prepare("SELECT owner, repo FROM projects WHERE id=?1")?;
    stmt.query_row(rusqlite::params![project_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })
    .map_err(IpcError::from)
}

/// Look up `(name, branch)` for a worktree id. `branch` may be NULL in the DB.
fn fetch_worktree(s: &Store, worktree_id: i64) -> Result<(String, Option<String>), IpcError> {
    let mut stmt = s
        .conn_ref()
        .prepare("SELECT name, branch FROM worktrees WHERE id=?1")?;
    stmt.query_row(rusqlite::params![worktree_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })
    .map_err(IpcError::from)
}

/// Build the absolute path on the remote host where a project (and optional
/// worktree) should live: `<root>/<owner>/<repo>` (`github` layout) or
/// `<root>/<repo>` (`flat`) for the project root, plus
/// `.claude/worktrees/<wt>` for non-main worktrees (a best-effort guess;
/// `service::repair` resolves the real worktree dir on the host). `root` must
/// already be absolute (see `remote_project_path_for`). Returns just the
/// project root if `wt_name` is None or "main".
pub(crate) fn remote_project_path(
    root: &str,
    layout: crate::projects::Layout,
    owner: &str,
    repo: &str,
    wt_name: Option<&str>,
) -> (String, String) {
    let project_root = layout.project_dir(root, owner, repo);
    let cwd = match wt_name {
        Some(name) if name != "main" => {
            format!("{project_root}/.claude/worktrees/{name}")
        }
        _ => project_root.clone(),
    };
    (project_root, cwd)
}

/// `remote_project_path` with the host's projects root resolved from the
/// `projects.*` settings (default `~/projects/github.com`) and expanded
/// against the remote `$HOME`.
fn remote_project_path_for(
    s: &Store,
    host: &str,
    home: &str,
    owner: &str,
    repo: &str,
    wt_name: Option<&str>,
) -> (String, String) {
    use crate::service::projects::{expand_home, layout, project_base_for};
    let root = expand_home(&project_base_for(s, host), home);
    remote_project_path(&root, layout(s), owner, repo, wt_name)
}

/// Ensure the remote host has the project cloned at `<project_root>` and,
/// optionally, has a worktree at `<project_root>/.claude/worktrees/<wt>`
/// checked out to `<branch>`. Idempotent: if the directory + .git is already
/// there, the clone step is skipped; same for worktree-add. Auto-clones via
/// SSH (`git@github.com:<owner>/<repo>.git`), assuming the remote has SSH
/// github access (the common case for dev machines).
///
/// The `token` parameter allows the caller to cancel the (potentially long-
/// running) `git clone` step. On cancellation the child is killed and
/// `Err(E_CANCELLED)` is returned. Partial clone dirs are NOT cleaned up
/// on cancel — that's a follow-up task.
///
/// Returns Ok(()) on success. Failure surfaces stderr in the IpcError so the
/// user can diagnose (missing SSH key, private-repo auth, etc.).
async fn ensure_remote_project(
    ssh: &Arc<SshClient>,
    host: &str,
    owner: &str,
    repo: &str,
    project_root: &str,
    worktree: Option<(&str, Option<&str>)>, // (name, branch)
    token: CancellationToken,
) -> Result<(), IpcError> {
    // Validate every component that gets interpolated into a remote path or
    // git command. Shell-quoting (below) stops command injection but NOT
    // `..` path traversal — a repo named `../../.ssh` would still be a valid
    // quoted argument that escapes the projects directory.
    crate::validate::path_component("owner", owner)?;
    crate::validate::path_component("repo", repo)?;
    if let Some((wt_name, branch)) = worktree {
        crate::validate::path_component("worktree name", wt_name)?;
        if let Some(b) = branch {
            crate::validate::git_ref(b)?;
        }
    }
    let clone_url = format!("git@github.com:{owner}/{repo}.git");
    // Build a single bash script that:
    //   1. clones the repo if .git is missing
    //   2. creates the worktree if requested and not yet present
    // Both steps are guarded so a re-run on an already-set-up host is a no-op.
    let mut script = String::new();
    script.push_str(&format!(
        "if [ ! -d {root}/.git ]; then mkdir -p $(dirname {root}) && git clone {url} {root}; fi",
        root = quote(project_root),
        url = quote(&clone_url),
    ));
    if let Some((wt_name, branch)) = worktree {
        if wt_name != "main" {
            let wt_rel = format!(".claude/worktrees/{wt_name}");
            let wt_abs = format!("{project_root}/{wt_rel}");
            let branch = branch.unwrap_or(wt_name);
            script.push_str(&format!(
                " && if [ ! -d {abs} ]; then cd {root} && git worktree add {rel} {br}; fi",
                abs = quote(&wt_abs),
                root = quote(project_root),
                rel = quote(&wt_rel),
                br = quote(branch),
            ));
        }
    }
    // Wrap in bash -lc so $PATH (git on Homebrew/Linuxbrew) is sourced. Use
    // the same single-quote-the-whole-script trick as RemoteTmux::remote_bash
    // to avoid the ssh argv-joining bug.
    // Single-quote the WHOLE script so it crosses the ssh argv-join as one
    // word. ssh concatenates the trailing args with spaces and the remote
    // LOGIN shell (often zsh) re-tokenizes them — without quoting, the
    // `if ...; then ...; fi` splits at `;` and orphans `then` ("zsh: parse
    // error near then"). `quote` escapes the inner single-quotes from the path
    // interpolation above.
    let quoted = quote(&script);
    let out = ssh
        .run_cancellable(
            host,
            &["bash", "-lc", &quoted],
            std::time::Duration::from_secs(120),
            token,
        )
        .await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        return Err(IpcError::new(
            "E_GIT_SETUP",
            format!(
                "couldn't ensure {owner}/{repo} on {host}: {}",
                if stderr.trim().is_empty() {
                    stdout.trim().to_string()
                } else {
                    stderr.trim().to_string()
                }
            ),
        ));
    }
    Ok(())
}

/// Build a bash script (run via `bash -lc`) that creates a new worktree for a
/// NEW branch `name` off the repo's default branch, under `.worktrees/` or
/// `.claude/worktrees/` (auto-detected, fallback `.worktrees/`). Idempotent:
/// if the worktree dir already exists it's reused. Git's chatter goes to
/// stderr; the ONLY stdout is the absolute PHYSICAL path of the worktree
/// (`pwd -P`, last line), which the caller uses as the tmux cwd. The logical
/// `pwd` echoed a symlinked root's spelling, a second identity for the same
/// checkout next to the physical one tmux and git report.
fn worktree_add_script(root: &str, name: &str, base: Option<&str>) -> String {
    // Requested base branch, shell-quoted; empty string when unset (= default
    // branch). The shell var is `basebr` to avoid colliding with `base`, which
    // already names the worktree *directory* (.worktrees vs .claude/worktrees).
    let basebr = base
        .map(|b| b.trim())
        .filter(|b| !b.is_empty())
        .map(quote)
        .unwrap_or_else(|| "''".to_string());
    format!(
        "set -e\n\
         cd {root}\n\
         name={name}\n\
         basebr={basebr}\n\
         if [ -d .worktrees ]; then base=.worktrees\n\
         elif [ -d .claude/worktrees ]; then base=.claude/worktrees\n\
         else base=.worktrees\n\
         fi\n\
         wt=\"$base/$name\"\n\
         if [ ! -e \"$wt\" ]; then\n\
         def=\"$(git symbolic-ref --short refs/remotes/origin/HEAD 2>/dev/null | sed 's#^origin/##')\"\n\
         [ -z \"$def\" ] && def=\"$(git rev-parse --abbrev-ref HEAD 2>/dev/null)\"\n\
         if [ -n \"$basebr\" ] && git show-ref --verify --quiet \"refs/heads/$basebr\"; then start=\"$basebr\"\n\
         elif [ -n \"$basebr\" ] && git show-ref --verify --quiet \"refs/remotes/origin/$basebr\"; then start=\"origin/$basebr\"\n\
         else start=\"$def\"\n\
         fi\n\
         git worktree add \"$wt\" -b \"$name\" \"$start\" 1>&2\n\
         fi\n\
         ( cd \"$wt\" && pwd -P )\n",
        root = quote(root),
        name = quote(name),
    )
}

async fn create_worktree_local(
    root: &str,
    name: &str,
    base: Option<&str>,
) -> Result<String, IpcError> {
    let script = worktree_add_script(root, name, base);
    let out = tokio::process::Command::new("bash")
        .args(["-lc", &script])
        .output()
        .await
        .map_err(|e| IpcError::new("E_GIT_SETUP", format!("bash: {e}")))?;
    if !out.status.success() {
        return Err(IpcError::new(
            "E_GIT_SETUP",
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub async fn new_session(
    mut args: NewSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    reg: &Arc<CancellationRegistry>,
) -> Result<SessionRow, IpcError> {
    // Reject hostile input before it reaches ssh / tmux / git.
    crate::validate::host_alias(&args.host_alias)?;
    // An empty name means "pick one for me" (the dialog's dice, an MCP
    // caller that doesn't care): mint it here so every caller shares the
    // same convention and the same collision policy.
    if args.name.trim().is_empty() {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        args.name = fill_session_name(&s, &args)?;
    }
    crate::validate::tmux_name(&args.name)?;
    if let Some(fname) = args.friendly_name.as_deref() {
        crate::validate::friendly_name(fname)?;
    }

    if let Some(name) = args.new_worktree.as_deref() {
        crate::validate::git_ref(name)?;
        if name == "main" || name == "master" {
            return Err(IpcError::new(
                "E_INVALID",
                "worktree name must not be 'main' or 'master'",
            ));
        }
    }

    // Mint / bind a cancellation token for the duration of this command.
    // If a call_id was provided by the frontend, bind under that id so the
    // frontend can cancel via cancel_command(call_id). Otherwise use an
    // anonymous id (internal callers, tests, local sessions).
    let (cancel_id, token) = match args.call_id {
        Some(id) => {
            let token = CancellationToken::new();
            reg.bind(id, token.clone());
            (id, token)
        }
        None => reg.register_anonymous(),
    };
    // RAII guard releases the registry slot on every exit path — including a
    // panic inside new_session_inner, which a manual unregister would miss.
    let _guard = CancelGuard::new(Arc::clone(reg), cancel_id);

    new_session_inner(args, store, ssh, token).await
}

/// Mint a tmux name for a `new_session` call that left `name` empty.
///
/// The deterministic `dev-<owner>-<repo>[--<worktree>][-term]` is used when no
/// session of that name exists on the host (the dialog's own convention).
/// When it is taken — a second session on the same worktree — a memorable
/// `<adjective>-<noun>` pair from `names::generate_name` is appended instead,
/// avoiding every slug already in use on the project (see
/// `project_taken_slugs`), so the result is unique and reads like
/// `dev-owner-repo--blue-sirius` rather than `…--main-2`. `.` and `:` are
/// mapped to `-` so the result always passes `validate::tmux_name`.
pub(crate) fn fill_session_name(s: &Store, args: &NewSessionArgs) -> Result<String, IpcError> {
    use super::names::{generate_name_default, tmux_safe};
    let (owner, repo) = fetch_owner_repo(s, args.project_id)?;
    let base = format!("dev-{owner}-{repo}");
    let term = if args.kind.as_deref() == Some("shell") {
        "-term"
    } else {
        ""
    };
    let wt: Option<String> = if let Some(n) = args
        .new_worktree
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        Some(n.to_string())
    } else if let Some(wid) = args.worktree_id {
        let (name, _) = fetch_worktree(s, wid)?;
        (name != "main").then_some(name)
    } else {
        None
    };
    let deterministic = tmux_safe(&match &wt {
        Some(w) => format!("{base}--{w}{term}"),
        None => format!("{base}{term}"),
    });
    let on_host = s.list_sessions_for_host(&args.host_alias)?;
    if !on_host.iter().any(|r| r.tmux_name == deterministic) {
        return Ok(deterministic);
    }
    let taken = project_taken_slugs(s, args.project_id, &owner, &repo)?;
    let pair = generate_name_default(&taken);
    Ok(tmux_safe(&match &wt {
        Some(w) => format!("{base}--{w}--{pair}{term}"),
        None => format!("{base}--{pair}{term}"),
    }))
}

/// Every slug already in use on a project — worktree names, the suffix of
/// each session's tmux name, and each slugified friendly name — i.e. the set
/// a freshly generated pair must avoid. Mirrors `takenSlugs` in
/// `NewSessionDialog.svelte`.
fn project_taken_slugs(
    s: &Store,
    project_id: i64,
    owner: &str,
    repo: &str,
) -> Result<std::collections::HashSet<String>, IpcError> {
    use super::names::{slugify, tmux_name_suffix};
    let mut taken: std::collections::HashSet<String> = s
        .list_worktrees_for_project(project_id)?
        .into_iter()
        .map(|w| w.name.to_lowercase())
        .collect();
    for r in s.list_all_sessions()? {
        if r.project_id != Some(project_id) {
            continue;
        }
        if let Some(suffix) = tmux_name_suffix(&r.tmux_name, owner, repo) {
            taken.insert(suffix.to_lowercase());
        }
        if let Some(slug) = r.friendly_name.as_deref().map(slugify) {
            if !slug.is_empty() {
                taken.insert(slug);
            }
        }
    }
    Ok(taken)
}

async fn new_session_inner(
    args: NewSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    token: CancellationToken,
) -> Result<SessionRow, IpcError> {
    // Resolve the cwd that tmux will spawn the pane in. For LOCAL the path
    // comes straight from the DB (it was discovered by scanning ~/projects).
    // For REMOTE we can't use the local path — it doesn't exist on the other
    // machine — so we translate to `~/projects/github.com/<owner>/<repo>`
    // (matching proj-clean's convention) and auto-clone if missing.
    let path: PathBuf = if args.host_alias == "local" {
        if let Some(ref name) = args.new_worktree {
            // NEW WORKTREE: create branch + worktree, return the new dir.
            let base_path = {
                let s = store
                    .lock()
                    .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
                let mut stmt = s
                    .conn_ref()
                    .prepare("SELECT base_path FROM projects WHERE id=?1")?;
                let row: String =
                    stmt.query_row(rusqlite::params![args.project_id], |r| r.get(0))?;
                row
            };
            PathBuf::from(
                create_worktree_local(&base_path, name, args.base_branch.as_deref()).await?,
            )
        } else {
            let s = store
                .lock()
                .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
            if let Some(wid) = args.worktree_id {
                let mut stmt = s
                    .conn_ref()
                    .prepare("SELECT path FROM worktrees WHERE id=?1")?;
                let row: String = stmt.query_row(rusqlite::params![wid], |r| r.get(0))?;
                PathBuf::from(row)
            } else {
                let mut stmt = s
                    .conn_ref()
                    .prepare("SELECT base_path FROM projects WHERE id=?1")?;
                let row: String =
                    stmt.query_row(rusqlite::params![args.project_id], |r| r.get(0))?;
                PathBuf::from(row)
            }
        }
    } else {
        // Remote path: derive from owner/repo, then ensure-on-remote.
        if let Some(ref name) = args.new_worktree {
            // NEW WORKTREE on remote: ensure clone exists, then create worktree.
            let (owner, repo) = {
                let s = store
                    .lock()
                    .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
                fetch_owner_repo(&s, args.project_id)?
            };
            let home = ssh.remote_home(&args.host_alias).await?;
            let (project_root, _) = {
                let s = store.lock().map_err(|_| IpcError::lock())?;
                remote_project_path_for(&s, &args.host_alias, &home, &owner, &repo, None)
            };
            ensure_remote_project(
                ssh,
                &args.host_alias,
                &owner,
                &repo,
                &project_root,
                None,
                token.clone(),
            )
            .await?;
            let script = worktree_add_script(&project_root, name, args.base_branch.as_deref());
            // Quote the whole script so it survives the ssh argv-join +
            // remote login-shell re-tokenization (see ensure_remote_project).
            let quoted = quote(&script);
            let out = ssh
                .run_cancellable(
                    &args.host_alias,
                    &["bash", "-lc", &quoted],
                    std::time::Duration::from_secs(60),
                    token,
                )
                .await?;
            if !out.status.success() {
                return Err(IpcError::new(
                    "E_GIT_SETUP",
                    String::from_utf8_lossy(&out.stderr).trim().to_string(),
                ));
            }
            PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            let (owner, repo, wt_info) = {
                let s = store
                    .lock()
                    .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
                let (owner, repo) = fetch_owner_repo(&s, args.project_id)?;
                let wt = if let Some(wid) = args.worktree_id {
                    Some(fetch_worktree(&s, wid)?)
                } else {
                    None
                };
                (owner, repo, wt)
            };
            let home = ssh.remote_home(&args.host_alias).await?;
            let wt_name_str = wt_info.as_ref().map(|(name, _)| name.as_str());
            let (project_root, cwd) = {
                let s = store.lock().map_err(|_| IpcError::lock())?;
                remote_project_path_for(&s, &args.host_alias, &home, &owner, &repo, wt_name_str)
            };
            let worktree_for_clone = wt_info
                .as_ref()
                .map(|(name, branch)| (name.as_str(), branch.as_deref()));
            ensure_remote_project(
                ssh,
                &args.host_alias,
                &owner,
                &repo,
                &project_root,
                worktree_for_clone,
                token,
            )
            .await?;
            PathBuf::from(cwd)
        }
    };
    // A "shell" session runs a plain login shell in the pane instead of
    // Claude Code. Any other value (incl. None) is treated as a "work" session.
    let is_shell = args.kind.as_deref() == Some("shell");
    // Work/review sessions get an app-minted Claude session id so a later
    // recreate/restart resumes THIS conversation, not "most recent for the cwd".
    let claude_id: Option<String> = if is_shell {
        None
    } else {
        Some(uuid::Uuid::new_v4().to_string())
    };
    let pane_cmd: String = if is_shell {
        crate::tmux::shell_pane_command(args.start_command.as_deref())
    } else {
        crate::tmux::pane_command_for(claude_id.as_deref())
    };

    // Automatic self-repair for an EXISTING worktree row / main checkout: the
    // row may point at a directory that was deleted since it was written.
    // Automatic means create-only (re-add a missing worktree from its existing
    // branch); anything more returns E_REPAIR_REQUIRED before tmux starts.
    // A brand-new worktree was just created by `worktree_add_script`.
    let (path, repaired) = if args.new_worktree.is_none() {
        let rep = crate::service::repair::ensure_for_new_session(
            store,
            ssh,
            crate::service::repair::NewSessionWorkspace {
                host_alias: &args.host_alias,
                project_id: args.project_id,
                worktree_id: args.worktree_id,
                tmux_name: &args.name,
                pane_cmd: &pane_cmd,
                cwd: &path.to_string_lossy(),
                base_branch: args.base_branch.as_deref(),
            },
        )
        .await?;
        (
            PathBuf::from(rep.cwd.clone()),
            Some(rep).filter(|r| !r.actions.is_empty()),
        )
    } else {
        (path, None)
    };

    let tmux = exec_for(&args.host_alias, ssh);
    tmux.new_session(&args.name, &path, &pane_cmd).await?;

    reconcile_one_host(store, ssh, &args.host_alias).await?;
    if let Some(rep) = &repaired {
        // Same detail as every other workspace_repaired event (branch_source
        // included), attached now that reconcile created the row.
        record_session_event(
            store,
            &args.host_alias,
            &args.name,
            crate::service::repair::EVENT_REPAIRED,
            Some(crate::service::repair::event_detail(rep)),
        );
    }
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let row = s
        .list_sessions_for_host(&args.host_alias)?
        .into_iter()
        .find(|r| r.tmux_name == args.name)
        .ok_or_else(|| {
            IpcError::new(
                "E_INTERNAL",
                format!(
                    "session {} on {} vanished after creation",
                    args.name, args.host_alias
                ),
            )
        })?;

    // PROD-5: the fleet created this session now. Soft-fail (cosmetic).
    if let Err(e) = s.set_started_at(row.id, now_unix()) {
        eprintln!(
            "new_session: storing started_at for {} failed: {e:?}",
            args.name
        );
    }

    // Deterministic friendly name: trust an explicit user value, otherwise
    // derive from the branch so the sidebar never shows the raw slug. Soft-
    // fail like the claude_session_id write below — a missing label is
    // cosmetic, the session is live.
    let derived_friendly = derive_friendly_name(&s, &args, row.worktree_id)?;
    if let Some(ref value) = derived_friendly {
        if let Err(e) = s.set_friendly_name(&args.host_alias, &args.name, Some(value)) {
            eprintln!(
                "new_session: storing friendly_name for {} failed: {e:?}",
                args.name
            );
        }
    }

    // Reconcile inserts every session as kind="work"; tag shell sessions
    // afterwards. The session upsert preserves `kind` on re-reconcile.
    if is_shell {
        s.set_session_kind(row.id, "shell", None)?;
        return s
            .get_session(&args.name, &args.host_alias)?
            .ok_or_else(|| IpcError::new("E_INTERNAL", "session vanished after kind tag"));
    }
    // Persist the minted Claude session id. Soft-fail: the session is live; a
    // failed write just means a future recreate falls back to `cl --continue`.
    let mut row = row;
    if let Some(ref cid) = claude_id {
        if let Err(e) = s.set_claude_session_id(row.id, cid) {
            eprintln!(
                "new_session: storing claude_session_id for {} failed: {e:?}",
                args.name
            );
        } else {
            row.claude_session_id = Some(cid.clone());
        }
    }
    if derived_friendly.is_some() {
        // The set_friendly_name call above emitted a session_updated row
        // event already; we refresh in-memory so the value returned to the
        // caller matches what the sidebar will display.
        if let Some(refreshed) = s.get_session(&args.name, &args.host_alias)? {
            row.friendly_name = refreshed.friendly_name;
        }
    }
    Ok(row)
}

/// Resolve the friendly name to persist for a freshly created session:
/// trim the user's explicit value, or derive a humanised label from the
/// branch when none was supplied. Returns `None` only when both inputs
/// resolve to empty — in practice this is rare (the humaniser falls back
/// to the repo name), but we treat it as "leave it NULL, let the agent
/// fill it in later" rather than overwriting with an empty string.
fn derive_friendly_name(
    s: &Store,
    args: &NewSessionArgs,
    row_worktree_id: Option<i64>,
) -> Result<Option<String>, IpcError> {
    if let Some(supplied) = args.friendly_name.as_deref() {
        let trimmed = supplied.trim();
        if !trimmed.is_empty() {
            return Ok(Some(trimmed.to_string()));
        }
    }
    let (owner, repo) = fetch_owner_repo(s, args.project_id)?;
    // Pick the most specific branch source available. `new_worktree` is the
    // branch the dialog just created; otherwise the worktree row stored
    // either a real `branch` ref or its display `name`; falling back to the
    // tmux name handles attach-to-bare-main and other oddities so the
    // humaniser always has something to chew on.
    let branch = if let Some(name) = args.new_worktree.as_deref() {
        name.to_string()
    } else if let Some(wid) = row_worktree_id.or(args.worktree_id) {
        let (name, branch) = fetch_worktree(s, wid)?;
        branch.unwrap_or(name)
    } else {
        args.name.clone()
    };
    let derived = crate::humanize::humanize_branch(&branch, &owner, &repo);
    if derived.is_empty() {
        Ok(None)
    } else {
        Ok(Some(derived))
    }
}

/// Session addressing for the control API (MCP-6). Every name-addressed
/// tool accepts EITHER a fleet `session_id` OR the `(host_alias, tmux_name)`
/// pair; this resolves whichever was given to the stored row. Precedence:
/// `session_id` when present (host/name are then ignored), else both parts
/// of the pair are required.
///
/// Errors: `E_INVALID` when neither form is complete, `E_NOTFOUND` when the
/// id / pair matches no row.
pub fn resolve_session_target(
    s: &Store,
    session_id: Option<i64>,
    host_alias: Option<&str>,
    tmux_name: Option<&str>,
) -> Result<SessionRow, IpcError> {
    if let Some(id) = session_id {
        return s
            .get_session_by_id(id)?
            .ok_or_else(|| IpcError::new("E_NOTFOUND", format!("session {id} not found")));
    }
    match (host_alias, tmux_name) {
        (Some(host), Some(name)) if !host.trim().is_empty() && !name.trim().is_empty() => {
            crate::validate::host_alias(host)?;
            crate::validate::tmux_name_lookup(name)?;
            s.get_session(name, host)?.ok_or_else(|| {
                IpcError::new("E_NOTFOUND", format!("session {name} not found on {host}"))
            })
        }
        _ => Err(IpcError::new(
            "E_INVALID",
            "pass session_id, or both host_alias and tmux_name",
        )),
    }
}

/// `whoami` for an in-session agent (MCP-6): the ONE fleet row whose
/// `tmux_name` matches. Default names are project-derived, so the same name
/// can exist on several hosts — that is `E_AMBIGUOUS`, with the candidates'
/// `(session_id, host_alias)` in `details` so the caller can retry with
/// `session_id`.
pub fn find_session_by_tmux_name(s: &Store, tmux_name: &str) -> Result<SessionRow, IpcError> {
    crate::validate::tmux_name_lookup(tmux_name)?;
    let all: Vec<SessionRow> = s
        .list_all_sessions()?
        .into_iter()
        .filter(|r| r.tmux_name == tmux_name)
        .collect();
    // A ghost left behind on another host must not make a live session
    // ambiguous: prefer running rows, fall back to everything.
    let running: Vec<SessionRow> = all
        .iter()
        .filter(|r| r.status == "running")
        .cloned()
        .collect();
    let matches = if running.is_empty() { all } else { running };
    match matches.len() {
        0 => Err(IpcError::new(
            "E_NOTFOUND",
            format!("no session named {tmux_name} on any host"),
        )),
        1 => Ok(matches.into_iter().next().expect("one match")),
        _ => {
            let candidates: Vec<serde_json::Value> = matches
                .iter()
                .map(|r| serde_json::json!({ "session_id": r.id, "host_alias": r.host_alias }))
                .collect();
            Err(IpcError::new(
                "E_AMBIGUOUS",
                format!(
                    "{} sessions are named {tmux_name}; pass session_id or host_alias",
                    matches.len()
                ),
            )
            .with_details(serde_json::json!({ "candidates": candidates })))
        }
    }
}

/// Refuse to operate on the registered controller session unless `force`.
///
/// Returns `Err(E_SELF_TARGET)` when `(host, name)` equals the registered
/// controller `(host, tmux_name)` and `force` is false. Always `Ok` when no
/// controller is registered, the target is a different session, or `force` is
/// set. Pure — the caller reads the controller from the store first.
pub fn guard_not_controller(
    controller: Option<&(String, String)>,
    host: &str,
    name: &str,
    force: bool,
) -> Result<(), IpcError> {
    if force {
        return Ok(());
    }
    if let Some((c_host, c_name)) = controller {
        if c_host == host && c_name == name {
            return Err(IpcError::new(
                "E_SELF_TARGET",
                format!(
                    "{name} on {host} is the registered fleet controller; \
                     pass force=true to target it anyway"
                ),
            ));
        }
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct KillSessionArgs {
    pub host_alias: String,
    pub name: String,
    /// Override the controller self-target guard.
    #[serde(default)]
    pub force: bool,
}

/// Claude session id to `claude stop` for a synthetic `bg:<uuid>` row: the
/// row's stored `claude_session_id` when present, else the uuid embedded in
/// the tmux_name itself. Pure so the fallback order is unit-testable.
fn bg_claude_session_id(tmux_name: &str, row_claude_id: Option<&str>) -> String {
    match row_claude_id {
        Some(id) if !id.trim().is_empty() => id.to_string(),
        _ => tmux_name.trim_start_matches("bg:").to_string(),
    }
}

pub async fn kill_session(
    args: KillSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<i64, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    // Lookup form: synthetic `bg:<uuid>` rows are killable too (via
    // `claude stop`, below) — only real tmux rows go through tmux.
    crate::validate::tmux_name_lookup(&args.name)?;
    // Look up id BEFORE killing so we can return it after. Read the controller
    // under the same lock and refuse to nuke ourselves unless forced.
    let (id, claude_sid) = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        guard_not_controller(
            s.get_controller()?.as_ref(),
            &args.host_alias,
            &args.name,
            args.force,
        )?;
        s.get_session(&args.name, &args.host_alias)?
            .map(|r| (r.id, r.claude_session_id))
            .ok_or_else(|| {
                IpcError::new("E_NOTFOUND", format!("session {} not found", args.name))
            })?
    };
    if args.name.starts_with("bg:") {
        // Background (`claude --bg`) agent — there is no tmux pane to kill.
        // `claude stop` is idempotent (an already-dead job is not an error),
        // so this also clears a stale row whose process died un-noticed: the
        // reconcile below sees the agent gone and prunes the row.
        let sid = bg_claude_session_id(&args.name, claude_sid.as_deref());
        crate::claude_cli::claude_stop(ssh, &args.host_alias, &sid).await?;
        if let Ok(s) = store.lock() {
            if let Err(e) = s.insert_session_event(id, "killed", None) {
                eprintln!("[event] insert killed failed for session {id}: {e}");
            }
        }
        reconcile_one_host(store, ssh, &args.host_alias).await?;
        return Ok(id);
    }
    let tmux = exec_for(&args.host_alias, ssh);
    tmux.kill_session(&args.name).await?;
    // Task G: record the kill before reconcile reaps the row. Best-effort.
    if let Ok(s) = store.lock() {
        if let Err(e) = s.insert_session_event(id, "killed", None) {
            eprintln!("[event] insert killed failed for session {id}: {e}");
        }
    }
    reconcile_one_host(store, ssh, &args.host_alias).await?;
    Ok(id)
}

#[derive(Deserialize)]
pub struct RenameSessionArgs {
    pub host_alias: String,
    pub old_name: String,
    pub new_name: String,
}

pub async fn rename_session(
    args: RenameSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<SessionRow, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    crate::validate::tmux_name_addressable(&args.old_name)?;
    crate::validate::tmux_name(&args.new_name)?;
    let tmux = exec_for(&args.host_alias, ssh);
    tmux.rename_session(&args.old_name, &args.new_name).await?;
    reconcile_one_host(store, ssh, &args.host_alias).await?;
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    // `new_name` is validated verbatim (no padding), so look it up as-is —
    // consistent with kill_session / restart_session.
    s.get_session(&args.new_name, &args.host_alias)?
        .ok_or_else(|| {
            IpcError::new(
                "E_NOTFOUND",
                format!(
                    "renamed session {} on {} did not appear in list",
                    args.new_name, args.host_alias
                ),
            )
        })
}

#[derive(Deserialize)]
pub struct SetFriendlyNameArgs {
    pub host_alias: String,
    pub tmux_name: String,
    /// Empty / whitespace-only value clears the label.
    pub friendly_name: String,
}

/// Set (or clear, on empty/whitespace) the session's display label. The agent
/// running inside a tmux session calls this via MCP after picking up a task.
pub fn set_session_friendly_name(
    args: SetFriendlyNameArgs,
    store: &Mutex<Store>,
) -> Result<SessionRow, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    // Lookup-mode validator: must accept the synthetic `bg:<uuid>` form used
    // by background-agent rows, which the create-mode `tmux_name` validator
    // rejects (tmux forbids `:` when creating a session, but we're only
    // addressing an existing row here, never spawning tmux).
    crate::validate::tmux_name_lookup(&args.tmux_name)?;
    crate::validate::friendly_name(&args.friendly_name)?;
    let trimmed = args.friendly_name.trim();
    let value = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    };
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.set_friendly_name(&args.host_alias, &args.tmux_name, value)?
        .ok_or_else(|| {
            IpcError::new(
                "E_NOTFOUND",
                format!(
                    "session {} not found on {}",
                    args.tmux_name, args.host_alias
                ),
            )
        })
}

#[derive(Deserialize)]
pub struct RestartSessionArgs {
    pub host_alias: String,
    pub name: String,
    /// Override the controller self-target guard.
    #[serde(default)]
    pub force: bool,
}

pub async fn restart_session(
    args: RestartSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<SessionRow, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    crate::validate::tmux_name_addressable(&args.name)?;
    // Respawn the pane with the command matching the session's kind so a
    // restarted shell session comes back as a shell, not a Claude pane. Read
    // the controller under the same lock and refuse to restart ourselves
    // unless forced.
    let (kind, claude_id, session_id) = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        guard_not_controller(
            s.get_controller()?.as_ref(),
            &args.host_alias,
            &args.name,
            args.force,
        )?;
        match s.get_session(&args.name, &args.host_alias)? {
            Some(r) => (r.kind, r.claude_session_id, Some(r.id)),
            None => ("work".to_string(), None, None),
        }
    };
    let pane_cmd: String = recreate_pane_command(&kind, claude_id.as_deref());
    let tmux = exec_for(&args.host_alias, ssh);
    // Automatic self-repair (create-only) before the pane is respawned, then
    // respawn INTO the verified directory (a pane whose cwd was deleted keeps
    // the dead inode until respawned with an explicit `-c`). A dead tmux
    // session is created instead of failing with "can't find session". A
    // workspace that needs more returns E_REPAIR_REQUIRED. Sessions with
    // nothing to repair (orphans, bg rows) keep the plain respawn.
    let repaired = match session_id {
        Some(id) => match crate::service::repair::ensure_session_workspace(
            id,
            crate::service::repair::Entry::Restart,
            store,
            ssh,
        )
        .await
        {
            Ok(rep) => Some(rep),
            Err(e) if e.code == codes::E_NOREPO || e.code == codes::E_BG_SESSION => None,
            Err(e) => return Err(e),
        },
        None => None,
    };
    match repaired {
        Some(rep) if !rep.tmux_alive => {
            tmux.new_session(&args.name, std::path::Path::new(&rep.cwd), &pane_cmd)
                .await?
        }
        Some(rep) => {
            tmux.respawn_pane_in(&args.name, std::path::Path::new(&rep.cwd), &pane_cmd)
                .await?
        }
        None => tmux.restart_session(&args.name, &pane_cmd).await?,
    }
    reconcile_one_host(store, ssh, &args.host_alias).await?;
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.get_session(&args.name, &args.host_alias)?.ok_or_else(|| {
        IpcError::new(
            "E_NOTFOUND",
            format!(
                "restarted session {} on {} did not appear in list",
                args.name, args.host_alias
            ),
        )
    })
}

/// Build the tmux invocations that together send a prompt to a session:
///   1. send-keys -t <name> -l <body>   (literal, no key-name translation;
///      a single trailing newline is stripped so internal newlines stay as
///      soft newlines and a stray trailing one can't pre-submit the body)
///   2. (when `submit`) a short settle so the REPL flushes the literal paste
///   3. (when `submit`) send-keys -t <name> Enter   (one real Enter to submit)
///
/// With `submit = false` the body is staged in the REPL but not submitted.
pub fn build_send_commands(tmux_name: &str, prompt: &str, submit: bool) -> Vec<String> {
    let body = prompt.strip_suffix('\n').unwrap_or(prompt);
    let mut cmds = vec![format!(
        "tmux send-keys -t {} -l {}",
        quote(tmux_name),
        quote(body)
    )];
    if submit {
        // settle so the REPL flushes the literal paste before the submit key
        cmds.push("sleep 0.15".to_string());
        cmds.push(format!("tmux send-keys -t {} Enter", quote(tmux_name)));
    }
    cmds
}

fn default_submit() -> bool {
    true
}

#[derive(Deserialize)]
pub struct SendPromptArgs {
    pub host_alias: String,
    pub tmux_name: String,
    pub prompt: String,
    #[serde(default = "default_submit")]
    pub submit: bool,
}

async fn send_prompt_inner(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    host_alias: &str,
    tmux_name: &str,
    prompt: &str,
    submit: bool,
) -> Result<(), IpcError> {
    crate::validate::host_alias(host_alias)?;
    crate::validate::tmux_name_addressable(tmux_name)?;
    // The send-keys commands run in ONE shell invocation joined with `&&` (so a
    // failed literal-text send doesn't still fire Enter) — one round-trip
    // instead of two.
    let script = build_send_commands(tmux_name, prompt, submit).join(" && ");
    let out = if host_alias == "local" {
        tokio::process::Command::new("bash")
            .args(["-c", &script])
            .output()
            .await
            .map_err(|e| IpcError::new("E_TMUX", format!("spawn bash: {e}")))?
    } else {
        ssh.run(
            host_alias,
            &["bash", "-lc", &quote(&script)],
            std::time::Duration::from_secs(10),
        )
        .await?
    };
    if !out.status.success() {
        return Err(IpcError::new(
            "E_TMUX",
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    // Task G: record the prompt on the session's timeline (detail truncated to
    // ~120 chars). Append-only + best-effort: never fail the send on this.
    record_session_event(store, host_alias, tmux_name, "prompt_sent", {
        let truncated: String = prompt.chars().take(120).collect();
        Some(truncated)
    });
    record_prompt_outcome(store, host_alias, tmux_name, prompt);
    Ok(())
}

/// Derive a default sidebar label from a prompt (PROD-4): the first five
/// words, lowercased, punctuation stripped, capped to the friendly-name
/// limit. `None` when nothing printable is left.
pub fn friendly_name_from_prompt(prompt: &str) -> Option<String> {
    let words: Vec<String> = prompt
        .split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .take(5)
        .collect();
    if words.is_empty() {
        return None;
    }
    let joined = words.join(" ");
    Some(joined.chars().take(80).collect())
}

/// Post-send bookkeeping (PROD-4 / PROD-5): stamp `last_prompt`, and give a
/// still-unnamed session a default friendly name derived from the prompt.
/// Best-effort: every failure is logged and swallowed — the prompt already
/// landed in the pane.
fn record_prompt_outcome(store: &Mutex<Store>, host_alias: &str, tmux_name: &str, prompt: &str) {
    let Ok(s) = store.lock() else {
        eprintln!("[prompt] store mutex poisoned recording outcome for {host_alias}/{tmux_name}");
        return;
    };
    let row = match s.get_session(tmux_name, host_alias) {
        Ok(Some(row)) => row,
        Ok(None) => return,
        Err(e) => {
            eprintln!("[prompt] lookup failed for {host_alias}/{tmux_name}: {e}");
            return;
        }
    };
    if let Err(e) = s.set_last_prompt(row.id, prompt) {
        eprintln!("[prompt] set_last_prompt failed for {host_alias}/{tmux_name}: {e}");
    }
    // The prompt-derived label replaces NO name or the deterministic
    // branch-derived default every fleet-created session starts with; a
    // label a human or the in-session agent chose (set_friendly_name) stays.
    let replaceable = match &row.friendly_name {
        None => true,
        Some(current) => {
            s.default_friendly_name(row.id).ok().flatten().as_deref() == Some(current.as_str())
        }
    };
    if replaceable {
        if let Some(name) = friendly_name_from_prompt(prompt) {
            if let Err(e) = s.set_friendly_name(host_alias, tmux_name, Some(&name)) {
                eprintln!(
                    "[prompt] default friendly_name failed for {host_alias}/{tmux_name}: {e}"
                );
            }
        }
    }
}

/// Append one event to a session's timeline, resolving the row by
/// (tmux_name, host). Best-effort: every failure (lock poisoned, row missing,
/// SQL error) is logged and swallowed so it can never block the mutation that
/// produced the event. Shared by send_prompt / kill / recreate. (Task G;
/// reconcile uses an inlined variant because it already holds the lock.)
fn record_session_event(
    store: &Mutex<Store>,
    host_alias: &str,
    tmux_name: &str,
    kind: &str,
    detail: Option<String>,
) {
    let s = match store.lock() {
        Ok(s) => s,
        Err(_) => {
            eprintln!("[event] store mutex poisoned recording {kind} for {host_alias}/{tmux_name}");
            return;
        }
    };
    match s.get_session(tmux_name, host_alias) {
        Ok(Some(row)) => {
            if let Err(e) = s.insert_session_event(row.id, kind, detail.as_deref()) {
                eprintln!("[event] insert {kind} failed for {host_alias}/{tmux_name}: {e}");
            }
        }
        Ok(None) => {} // no row yet (e.g. brand-new session) — nothing to attach to
        Err(e) => eprintln!("[event] lookup failed for {host_alias}/{tmux_name}: {e}"),
    }
}

pub async fn send_prompt(
    args: SendPromptArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    send_prompt_inner(
        store,
        ssh,
        &args.host_alias,
        &args.tmux_name,
        &args.prompt,
        args.submit,
    )
    .await
}

// --- broadcast_prompt (fan-out to matching work sessions) ------------------

/// Filter narrowing which work sessions a broadcast targets. Any field left
/// `None` is not constrained. `status` compares against a session's
/// `claude_status`.
#[derive(Debug, Default, Clone)]
pub struct BroadcastFilter {
    pub host: Option<String>,
    pub project_id: Option<i64>,
    pub status: Option<String>,
}

/// PURE selector: pick the session ids a broadcast should target.
///
/// Rules:
///   - only `kind == "work"` sessions are eligible;
///   - the host/project_id/status filters are applied only when set
///     (status compares against `claude_status`);
///   - the controller `(host_alias, tmux_name)`, when known, is excluded so a
///     broadcast never fans back into the session driving it.
pub fn select_targets(
    sessions: &[SessionRow],
    f: &BroadcastFilter,
    controller: Option<&(String, String)>,
) -> Vec<i64> {
    sessions
        .iter()
        .filter(|s| s.kind == "work")
        .filter(|s| match &f.host {
            Some(h) => &s.host_alias == h,
            None => true,
        })
        .filter(|s| match f.project_id {
            Some(pid) => s.project_id == Some(pid),
            None => true,
        })
        .filter(|s| match &f.status {
            Some(st) => s.claude_status.as_deref() == Some(st.as_str()),
            None => true,
        })
        .filter(|s| match controller {
            Some((host, tmux)) => !(&s.host_alias == host && &s.tmux_name == tmux),
            None => true,
        })
        .map(|s| s.id)
        .collect()
}

/// Per-session outcome of a broadcast.
#[derive(serde::Serialize)]
pub struct BroadcastResult {
    pub session_id: i64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Serializable summary returned by [`broadcast_prompt`].
#[derive(serde::Serialize)]
pub struct BroadcastSummary {
    pub sent: u32,
    pub failed: u32,
    pub results: Vec<BroadcastResult>,
}

/// Fan the same `prompt` out to every work session matching `filter`,
/// excluding the controller. Resolves targets via [`select_targets`] (reading
/// the controller from the store), then delivers via the existing
/// [`send_prompt`] per target, collecting one result each.
///
/// `submit` mirrors `send_prompt`'s submit semantics (Enter after the literal
/// text). It is threaded through for API parity; the current delivery path
/// always submits, so today it is accepted and ignored when `true` (the
/// default). It is kept in the signature so a future no-submit `send_prompt`
/// can wire straight through without a signature change.
pub async fn broadcast_prompt(
    filter: BroadcastFilter,
    prompt: String,
    submit: bool,
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
) -> Result<BroadcastSummary, IpcError> {
    // Snapshot sessions + resolve the controller while holding the guard, then
    // drop it before any `.await` (never hold the mutex across await).
    let (sessions, controller) = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        let sessions = s.list_all_sessions().map_err(|e| {
            IpcError::new(codes::E_SQLITE, format!("list sessions for broadcast: {e}"))
        })?;
        // The controller concept is resolved from the store when available.
        // Until a controller is recorded, no session is excluded on that basis.
        let controller = resolve_controller(&s);
        (sessions, controller)
    };

    let targets = select_targets(&sessions, &filter, controller.as_ref());

    // Map session id -> (host_alias, tmux_name) for delivery.
    let mut results: Vec<BroadcastResult> = Vec::with_capacity(targets.len());
    let mut sent: u32 = 0;
    let mut failed: u32 = 0;
    for sid in targets {
        let Some(row) = sessions.iter().find(|s| s.id == sid) else {
            continue;
        };
        let res =
            send_prompt_inner(store, ssh, &row.host_alias, &row.tmux_name, &prompt, submit).await;
        match res {
            Ok(()) => {
                sent += 1;
                results.push(BroadcastResult {
                    session_id: sid,
                    ok: true,
                    error: None,
                });
            }
            Err(e) => {
                failed += 1;
                results.push(BroadcastResult {
                    session_id: sid,
                    ok: false,
                    error: Some(format!("{}: {}", e.code, e.message)),
                });
            }
        }
    }

    Ok(BroadcastSummary {
        sent,
        failed,
        results,
    })
}

/// Best-effort controller lookup. The recorded controller `(host_alias,
/// tmux_name)` lives in the store (Task D); broadcast excludes it so a fan-out
/// never steers the controller session into itself. Degrades to "no controller
/// known" (no exclusion) on any store error.
fn resolve_controller(store: &Store) -> Option<(String, String)> {
    store.get_controller().ok().flatten()
}

/// Resolve the cwd a session should (re)open in for a LOCAL host. Order: the
/// session's worktree path (by `worktree_id`) → its project `base_path` →
/// error. Remote hosts must go through [`cwd_source_for_session`] /
/// [`resolve_cwd_source`] instead — the local paths in this table do not
/// exist on the remote machine.
fn resolve_session_cwd(s: &Store, row: &crate::store::SessionRow) -> Result<String, IpcError> {
    if let Some(wt_id) = row.worktree_id {
        if let Some(path) = s.worktree_path(wt_id)? {
            return Ok(path);
        }
    }
    let base = match row.project_id {
        Some(pid) => s.project_base_path(pid)?,
        None => None,
    };
    // Sessions discovered by reconcile carry only `worktree_key` (the worktree
    // dir name, derived from their live cwd), never `worktree_id` — reconcile
    // does not resolve the FK. Honor the key so a session in a worktree is
    // recreated there, not at the repo root.
    if let (Some(pid), Some(key)) = (row.project_id, row.worktree_key.as_deref()) {
        if key != "main" {
            // 1. The worktrees table is authoritative (and handles non-standard
            //    locations) — when it is fresh.
            if let Some(path) = s
                .list_worktrees_for_project(pid)?
                .into_iter()
                .find(|w| w.name == key)
                .map(|w| w.path)
            {
                return Ok(path);
            }
            // 2. The table is only refreshed by `refresh_projects`, so a
            //    just-created worktree (the common recreate case) may be absent.
            //    Reconstruct from the on-disk standard layouts under the base.
            if let Some(ref base) = base {
                if let Some(path) =
                    worktree_path_on_disk(base, key, |p| std::path::Path::new(p).exists())
                {
                    return Ok(path);
                }
            }
        }
    }
    if let Some(base) = base {
        return Ok(base);
    }
    Err(IpcError::new(
        "E_NOREPO",
        "cannot determine a worktree path for this session",
    ))
}

/// Reconstruct a local worktree's path from the project `base` and its dir
/// name `key`, trying the two layouts the ecosystem uses (`.claude/worktrees/`
/// then `.worktrees/`). Returns the first that `exists`. Used as a fallback
/// when the worktrees table has not been refreshed since the worktree was made.
fn worktree_path_on_disk(base: &str, key: &str, exists: impl Fn(&str) -> bool) -> Option<String> {
    for layout in [".claude/worktrees", ".worktrees"] {
        let candidate = format!("{base}/{layout}/{key}");
        if exists(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Per-host cwd resolution input, captured under the store lock before any
/// async work. For `local` we already have the absolute path; for remote we
/// keep the (owner, repo, worktree-name) tuple and only translate to a path
/// once `ssh.remote_home` resolves off-lock.
#[cfg_attr(test, derive(Debug))]
enum CwdSource {
    Local(String),
    Remote {
        /// Host's projects root from the `projects.*` settings, unexpanded
        /// (may start with `~/`; expanded against the remote `$HOME`).
        root: String,
        layout: crate::projects::Layout,
        owner: String,
        repo: String,
        wt_name: Option<String>,
    },
}

/// Pick the cwd-resolution strategy for `row` while holding the store lock.
/// Falls back from worktree → project root (matching the local resolver) when
/// the worktree row is missing.
fn cwd_source_for_session(
    s: &Store,
    row: &crate::store::SessionRow,
) -> Result<CwdSource, IpcError> {
    if row.host_alias == "local" {
        return Ok(CwdSource::Local(resolve_session_cwd(s, row)?));
    }
    let pid = row.project_id.ok_or_else(|| {
        IpcError::new(
            "E_NOREPO",
            "cannot determine a remote path: session has no project",
        )
    })?;
    let (owner, repo) = fetch_owner_repo(s, pid)?;
    // Like the local resolver: reconciled sessions only have `worktree_key`, so
    // fall back to it when the FK is unset. `remote_project_path` maps a
    // non-"main" name to `<repo>/.claude/worktrees/<name>`, matching how
    // `new_session` creates remote worktrees.
    let wt_name = match row.worktree_id {
        Some(wid) => s.worktree_name(wid)?,
        None => row
            .worktree_key
            .as_deref()
            .filter(|k| *k != "main")
            .map(str::to_string),
    };
    Ok(CwdSource::Remote {
        root: crate::service::projects::project_base_for(s, &row.host_alias),
        layout: crate::service::projects::layout(s),
        owner,
        repo,
        wt_name,
    })
}

/// Off-lock half of host-aware cwd resolution: for remote sources, look up
/// the remote `$HOME` and assemble the project (or worktree) path using the
/// same convention `new_session` uses. Local sources pass straight through.
async fn resolve_cwd_source(
    src: CwdSource,
    host_alias: &str,
    ssh: &Arc<SshClient>,
) -> Result<String, IpcError> {
    match src {
        CwdSource::Local(p) => Ok(p),
        CwdSource::Remote {
            root,
            layout,
            owner,
            repo,
            wt_name,
        } => {
            let home = ssh.remote_home(host_alias).await?;
            let root = crate::service::projects::expand_home(&root, &home);
            let (_root, cwd) =
                remote_project_path(&root, layout, &owner, &repo, wt_name.as_deref());
            Ok(cwd)
        }
    }
}

/// Poll the tmux pane until `cl`'s REPL prompt appears, up to ~6s. Returns
/// when ready, or after the timeout (best-effort — a missed prompt just means
/// the user presses Enter / re-sends manually; spawn_review already soft-fails
/// the seed). `cl`'s prompt box draws a border (│) and a `>` prompt; we look
/// for either as a readiness signal.
async fn wait_for_repl_ready(tmux: &dyn TmuxExec, name: &str) {
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        if let Ok(pane) = tmux.capture_pane(name).await {
            if pane.contains('>') || pane.contains('│') {
                return;
            }
        }
    }
}

#[derive(Deserialize)]
pub struct SpawnReviewArgs {
    pub source_session_id: i64,
    pub prompt: String,
    // Reserved for future cancellation wiring. The frontend's
    // invokeCmdAbortable injects a call_id; v1 spawn_review doesn't register a
    // CancellationToken under it (the spawn is short — tmux create + reconcile
    // + ~1.5s seed delay), so an abort is currently a no-op on the backend.
    #[allow(dead_code)]
    pub call_id: Option<u64>,
}

pub async fn spawn_review(
    args: SpawnReviewArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<crate::store::SessionRow, IpcError> {
    // 1. Snapshot source + capture cwd-resolution inputs under a brief lock.
    //    For remote hosts the cwd is finalized off-lock via `ssh.remote_home`.
    let (source, cwd_src) = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        let source = s
            .get_session_by_id(args.source_session_id)?
            .ok_or_else(|| IpcError::new("E_NOTFOUND", "source session not found"))?;
        let cwd_src = cwd_source_for_session(&s, &source)?;
        (source, cwd_src)
    };
    // Automatic workspace check (create-only) in the SOURCE's workspace, and
    // use the directory the probe resolved on the host. The remote guess in
    // `resolve_cwd_source` assumes `.claude/worktrees/`, so on a
    // `.worktrees/`-layout host it named a missing dir and tmux silently fell
    // back to $HOME. Orphans / bg rows keep the plain resolution.
    let cwd = match crate::service::repair::ensure_session_workspace(
        source.id,
        crate::service::repair::Entry::SpawnReview,
        store,
        ssh,
    )
    .await
    {
        Ok(rep) => rep.cwd,
        Err(e) if e.code == codes::E_NOREPO || e.code == codes::E_BG_SESSION => {
            resolve_cwd_source(cwd_src, &source.host_alias, ssh).await?
        }
        Err(e) => return Err(e),
    };

    // 2. Spawn the review tmux session (off-lock).
    //    A review runs Claude Code — same pane command as any "work" session.
    let short = format!("{:x}", now_unix() & 0xfffff);
    let review_name = format!("{}--review-{}", source.tmux_name, short);
    let claude_id = uuid::Uuid::new_v4().to_string();
    let tmux = exec_for(&source.host_alias, ssh);
    tmux.new_session(
        &review_name,
        std::path::Path::new(&cwd),
        &crate::tmux::pane_command_for(Some(&claude_id)),
    )
    .await?;

    // 3. Register via per-host reconcile.
    reconcile_one_host(store, ssh, &source.host_alias).await?;

    // 4. Tag as review + capture id.
    let review_id = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        let row = s
            .list_sessions_for_host(&source.host_alias)?
            .into_iter()
            .find(|r| r.tmux_name == review_name)
            .ok_or_else(|| IpcError::new("E_INTERNAL", "review session vanished after spawn"))?;
        s.set_session_kind(row.id, "review", Some(source.id))?;
        let _ = s.set_claude_session_id(row.id, &claude_id);
        let _ = s.set_started_at(row.id, now_unix());
        row.id
    };

    // 5. Seed the prompt. Wait until cl's TUI is ready before send-keys lands.
    wait_for_repl_ready(tmux.as_ref(), &review_name).await;
    // Soft-fail: the review session is already spawned, registered, and tagged.
    // If seeding the prompt fails (e.g. cl wasn't ready yet), DON'T discard the
    // session — return it anyway so the user can type the review prompt manually
    // in the terminal. Log the failure for diagnostics.
    if let Err(e) = send_prompt_inner(
        store,
        ssh,
        &source.host_alias,
        &review_name,
        &args.prompt,
        true,
    )
    .await
    {
        eprintln!("spawn_review: seeding prompt to {review_name} failed (session is live, seed manually): {e:?}");
    }

    // 6. Return the tagged review row.
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.get_session_by_id(review_id)?
        .ok_or_else(|| IpcError::new("E_INTERNAL", "review row missing after tag"))
}

/// The pane command to relaunch when (re)creating a session. `shell` → a bare
/// shell; otherwise resume the session's own Claude id (or `--continue` for a
/// legacy session with no stored id). A stored id is validated before use so a
/// tampered DB value can't inject shell — an invalid id degrades to `None`.
pub(crate) fn recreate_pane_command(kind: &str, claude_session_id: Option<&str>) -> String {
    if kind == "shell" {
        return crate::tmux::shell_pane_command(None);
    }
    let id = claude_session_id.filter(|id| crate::validate::claude_session_id(id).is_ok());
    crate::tmux::pane_command_for(id)
}

#[derive(Deserialize)]
pub struct RecreateSessionArgs {
    pub session_id: i64,
    /// Override the controller self-target guard.
    #[serde(default)]
    pub force: bool,
}

pub async fn recreate_session(
    args: RecreateSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<SessionRow, IpcError> {
    // Snapshot the session, gate on host reachability, and capture cwd-resolution
    // inputs — all under one brief lock, before any tmux/ssh call. For remote
    // hosts the cwd is finalized off-lock (needs `ssh.remote_home`), because the
    // local DB path is meaningless on the other machine.
    let (sess, cwd_src, pane_cmd) = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        let sess = s
            .get_session_by_id(args.session_id)?
            .ok_or_else(|| IpcError::new("E_NOTFOUND", "session not found"))?;
        // Refuse to nuke-and-rebuild ourselves unless forced.
        guard_not_controller(
            s.get_controller()?.as_ref(),
            &sess.host_alias,
            &sess.tmux_name,
            args.force,
        )?;
        let host = s
            .get_host_row(&sess.host_alias)?
            .ok_or_else(|| IpcError::new("E_NOTFOUND", "host not found"))?;
        if !host.reachable {
            return Err(IpcError::new(
                "E_HOST_OFFLINE",
                format!("host {} is not reachable", host.alias),
            ));
        }
        let cwd_src = cwd_source_for_session(&s, &sess)?;
        let pane_cmd = recreate_pane_command(&sess.kind, sess.claude_session_id.as_deref());
        (sess, cwd_src, pane_cmd)
    };
    // Automatic self-repair (create-only): re-add a deleted worktree from its
    // existing branch and use the verified path; anything more returns
    // E_REPAIR_REQUIRED. Orphans (no project) keep the plain resolution.
    let cwd = match crate::service::repair::ensure_session_workspace(
        sess.id,
        crate::service::repair::Entry::Recreate,
        store,
        ssh,
    )
    .await
    {
        Ok(rep) => rep.cwd,
        Err(e) if e.code == codes::E_NOREPO => {
            resolve_cwd_source(cwd_src, &sess.host_alias, ssh).await?
        }
        Err(e) => return Err(e),
    };

    let tmux = exec_for(&sess.host_alias, ssh);
    // Tear down any live session first (frees the old process tree / wedged
    // session). A ghost has no live session, so tolerate "no such session":
    // we ignore the kill result and rely on new_session below to fail loudly
    // if the old session unexpectedly survived (it would report a duplicate).
    let _ = tmux.kill_session(&sess.tmux_name).await;
    // Rebuild fresh in the worktree with the kind-appropriate command — the
    // same primitive new_session() uses.
    tmux.new_session(&sess.tmux_name, std::path::Path::new(&cwd), &pane_cmd)
        .await?;

    // Mark the row live again and return it.
    let row = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        let row = s
            .restore_session(sess.id)?
            .ok_or_else(|| IpcError::new("E_INTERNAL", "session vanished after restore"))?;
        // Task G: record the recreate on the (preserved) row. Best-effort.
        if let Err(e) = s.insert_session_event(sess.id, "recreated", None) {
            eprintln!(
                "[event] insert recreated failed for session {}: {e}",
                sess.id
            );
        }
        row
    };
    Ok(row)
}

#[derive(Deserialize)]
pub struct DismissGhostSessionArgs {
    pub session_id: i64,
}

pub fn dismiss_ghost_session(
    args: DismissGhostSessionArgs,
    store: &Mutex<Store>,
) -> Result<(), IpcError> {
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let sess = s
        .get_session_by_id(args.session_id)?
        .ok_or_else(|| IpcError::new("E_NOTFOUND", "session not found"))?;
    if sess.status != "ghost" {
        return Err(IpcError::new(
            "E_INVALID_STATE",
            format!(
                "session {} is not a ghost (status={})",
                sess.id, sess.status
            ),
        ));
    }
    s.delete_session(sess.id)?;
    Ok(())
}

/// Capture a session's terminal output. `scrollback_lines = None` returns the
/// visible pane; `Some(n)` includes `n` rows of scrollback history.
pub async fn capture_session_output(
    session_id: i64,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    scrollback_lines: Option<u32>,
) -> Result<String, IpcError> {
    let (host, name) = crate::commands::repo::session_target(store, session_id)?;
    let tmux = exec_for(&host, ssh);
    match scrollback_lines {
        Some(n) => tmux.capture_pane_scrollback(&name, n).await,
        None => tmux.capture_pane(&name).await,
    }
}

/// Accept a `claude agents --json` status only when it is in the documented
/// `ClaudeStatus` vocabulary. An unknown value (a newer CLI, a typo upstream)
/// is logged once per reconcile and dropped so the pane-derived fallback wins
/// instead of the DB silently diverging from what the MCP docs promise.
fn known_agent_status(tmux_name: &str, status: Option<&str>) -> Option<String> {
    let raw = status?;
    match raw.parse::<crate::service::pane_intel::ClaudeStatus>() {
        Ok(st) => Some(st.as_str().to_string()),
        Err(_) => {
            eprintln!(
                "[reconcile] {tmux_name}: dropping unknown claude agents status {raw:?} \
                 (not in vocabulary {})",
                crate::service::pane_intel::ClaudeStatus::vocabulary_doc()
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn known_agent_status_keeps_vocabulary_and_drops_the_rest() {
        for good in [
            "working",
            "blocked",
            "completed",
            "failed",
            "stopped",
            "idle",
        ] {
            assert_eq!(
                known_agent_status("dev", Some(good)).as_deref(),
                Some(good),
                "{good} is in the vocabulary and must be stored verbatim"
            );
        }
        // Unknown CLI values fall through to None so the pane-derived
        // fallback (or the COALESCE-preserved prior value) is used instead.
        assert_eq!(known_agent_status("dev", Some("awaiting_input")), None);
        assert_eq!(known_agent_status("dev", Some("")), None);
        assert_eq!(known_agent_status("dev", None), None);
    }

    #[test]
    fn bg_claude_session_id_prefers_row_id_falls_back_to_name() {
        // Stored claude_session_id wins…
        assert_eq!(
            bg_claude_session_id("bg:aaa-111", Some("bbb-222")),
            "bbb-222"
        );
        // …a missing or blank one falls back to the uuid in the tmux_name.
        assert_eq!(bg_claude_session_id("bg:aaa-111", None), "aaa-111");
        assert_eq!(bg_claude_session_id("bg:aaa-111", Some("  ")), "aaa-111");
    }

    /// Build a `SessionRow` with sensible defaults for selector tests.
    fn row(
        id: i64,
        host: &str,
        tmux: &str,
        kind: &str,
        project_id: Option<i64>,
        claude_status: Option<&str>,
    ) -> SessionRow {
        SessionRow {
            id,
            tmux_name: tmux.into(),
            host_alias: host.into(),
            project_id,
            worktree_id: None,
            created_at: 0,
            last_activity_at: 0,
            status: "running".into(),
            notes: None,
            account_uuid: None,
            kind: kind.into(),
            reviews_session_id: None,
            worktree_key: None,
            lost_at: None,
            claude_session_id: None,
            claude_status: claude_status.map(|s| s.to_string()),
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
        }
    }

    fn sample_sessions() -> Vec<SessionRow> {
        vec![
            row(1, "mac", "work-a", "work", Some(10), Some("idle")),
            row(2, "mac", "work-b", "work", Some(10), Some("running")),
            row(3, "mefistos", "work-c", "work", Some(20), Some("idle")),
            // non-work session must always be excluded
            row(4, "mac", "review-a", "review", Some(10), Some("idle")),
        ]
    }

    #[test]
    fn select_targets_filters_by_host() {
        let s = sample_sessions();
        let f = BroadcastFilter {
            host: Some("mac".into()),
            ..Default::default()
        };
        assert_eq!(select_targets(&s, &f, None), vec![1, 2]);
    }

    #[test]
    fn select_targets_filters_by_status() {
        let s = sample_sessions();
        let f = BroadcastFilter {
            status: Some("idle".into()),
            ..Default::default()
        };
        // session 4 is idle but kind=review, so excluded.
        assert_eq!(select_targets(&s, &f, None), vec![1, 3]);
    }

    #[test]
    fn select_targets_filters_by_project() {
        let s = sample_sessions();
        let f = BroadcastFilter {
            project_id: Some(20),
            ..Default::default()
        };
        assert_eq!(select_targets(&s, &f, None), vec![3]);
    }

    #[test]
    fn select_targets_filters_combined() {
        let s = sample_sessions();
        let f = BroadcastFilter {
            host: Some("mac".into()),
            project_id: Some(10),
            status: Some("running".into()),
        };
        assert_eq!(select_targets(&s, &f, None), vec![2]);
    }

    #[test]
    fn select_targets_excludes_non_work() {
        let s = sample_sessions();
        // No filters: every work session, never the review one (id 4).
        let f = BroadcastFilter::default();
        assert_eq!(select_targets(&s, &f, None), vec![1, 2, 3]);
    }

    #[test]
    fn select_targets_excludes_controller() {
        let s = sample_sessions();
        let f = BroadcastFilter::default();
        let controller = ("mac".to_string(), "work-a".to_string());
        // session 1 is the controller and must be dropped.
        assert_eq!(select_targets(&s, &f, Some(&controller)), vec![2, 3]);
    }

    #[test]
    fn select_targets_controller_only_matches_on_both_host_and_tmux() {
        let s = sample_sessions();
        let f = BroadcastFilter::default();
        // Same tmux name on a different host must NOT be excluded.
        let controller = ("mefistos".to_string(), "work-a".to_string());
        assert_eq!(select_targets(&s, &f, Some(&controller)), vec![1, 2, 3]);
    }

    #[test]
    fn guard_blocks_self_target_without_force() {
        let ctrl = ("mac".to_string(), "dev-fleet".to_string());
        let err =
            guard_not_controller(Some(&ctrl), "mac", "dev-fleet", false).expect_err("should block");
        assert_eq!(err.code, "E_SELF_TARGET");
    }

    #[test]
    fn guard_allows_self_target_with_force() {
        let ctrl = ("mac".to_string(), "dev-fleet".to_string());
        assert!(guard_not_controller(Some(&ctrl), "mac", "dev-fleet", true).is_ok());
    }

    #[test]
    fn guard_allows_non_controller_target() {
        let ctrl = ("mac".to_string(), "dev-fleet".to_string());
        // different name
        assert!(guard_not_controller(Some(&ctrl), "mac", "other", false).is_ok());
        // different host
        assert!(guard_not_controller(Some(&ctrl), "mefistos", "dev-fleet", false).is_ok());
    }

    #[test]
    fn guard_allows_when_no_controller_registered() {
        assert!(guard_not_controller(None, "mac", "dev-fleet", false).is_ok());
    }

    #[test]
    fn extracts_owner_repo_from_macos_path() {
        let r = extract_owner_repo(
            "/Users/martinjanci/projects/github.com/martin-janci/claude-fleet/.claude/worktrees/x",
        );
        assert_eq!(r, Some(("martin-janci".into(), "claude-fleet".into())));
    }

    #[test]
    fn extracts_owner_repo_from_linux_path() {
        let r = extract_owner_repo("/home/mjanci/projects/github.com/martin-janci/sales-twins-app");
        assert_eq!(r, Some(("martin-janci".into(), "sales-twins-app".into())));
    }

    #[test]
    fn extracts_owner_repo_when_followed_by_subdir() {
        let r = extract_owner_repo("/anywhere/projects/github.com/papayapos/pos-frontend/src/lib");
        assert_eq!(r, Some(("papayapos".into(), "pos-frontend".into())));
    }

    #[test]
    fn returns_none_when_not_github_com_layout() {
        assert_eq!(extract_owner_repo("/tmp/random/repo"), None);
        assert_eq!(extract_owner_repo("/home/x/projects/gitlab.com/a/b"), None);
    }

    fn agent(
        session_id: &str,
        name: Option<&str>,
        cwd: Option<&str>,
    ) -> crate::claude_agents::ClaudeAgentRow {
        crate::claude_agents::ClaudeAgentRow {
            session_id: Some(session_id.into()),
            name: name.map(Into::into),
            status: Some("working".into()),
            cwd: cwd.map(Into::into),
        }
    }

    #[test]
    fn unmatched_bg_agents_selects_agents_with_no_tmux_session() {
        // No tmux sessions at all → every agent (with an id) is unmatched.
        let agents = vec![
            agent("bg-1", Some("bg-job-1"), Some("/a")),
            agent("bg-2", None, Some("/b")),
        ];
        let unmatched = unmatched_bg_agents(&[], &agents, true);
        assert_eq!(unmatched.len(), 2);

        // A tmux session whose name matches an agent → that agent is matched
        // (excluded), the other remains unmatched.
        let live = vec![crate::tmux::TmuxSession {
            name: "bg-job-1".into(),
            created: 1,
            last_activity: 1,
            attached: false,
            path: std::path::PathBuf::from("/a"),
        }];
        let unmatched = unmatched_bg_agents(&live, &agents, true);
        let ids: Vec<&str> = unmatched
            .iter()
            .map(|a| a.session_id.as_deref().unwrap())
            .collect();
        assert_eq!(ids, vec!["bg-2"], "only the unmatched agent remains");
    }

    #[test]
    fn unmatched_bg_agents_skips_agents_without_session_id() {
        let agents = vec![crate::claude_agents::ClaudeAgentRow {
            session_id: None,
            name: Some("ghosty".into()),
            status: None,
            cwd: None,
        }];
        assert!(unmatched_bg_agents(&[], &agents, true).is_empty());
    }

    #[test]
    fn reconcile_bg_agents_upserts_bg_session_row() {
        // Feed agent rows + an EMPTY tmux list → expect a `bg` SessionRow.
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let agents = vec![agent("bg-uuid-1", Some("my-bg-job"), Some("/tmp/proj"))];

        reconcile_bg_agents(&s, "local", &[], &[], &agents).unwrap();

        let rows = s.list_sessions_for_host("local").unwrap();
        let bg = rows
            .iter()
            .find(|r| r.tmux_name == "bg:bg-uuid-1")
            .expect("a bg SessionRow must be present");
        assert_eq!(bg.kind, "bg");
        assert_eq!(bg.claude_session_id.as_deref(), Some("bg-uuid-1"));
        assert_eq!(bg.claude_status.as_deref(), Some("working"));
        assert_eq!(bg.status, "running");
    }

    #[test]
    fn reconcile_bg_agents_prunes_vanished_agents_two_phase() {
        // A bg agent that disappears from `claude agents --json` is ghosted on
        // the next reconcile pass and hard-deleted (events included) on the one
        // after — so dead bg rows cannot accumulate.
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let agents = vec![agent("bg-uuid-1", Some("my-bg-job"), Some("/tmp/proj"))];
        reconcile_bg_agents(&s, "local", &[], &[], &agents).unwrap();
        let id = s
            .get_session("bg:bg-uuid-1", "local")
            .unwrap()
            .expect("upserted")
            .id;

        // Pass 2: agent gone (empty listing) → ghosted, still present.
        reconcile_bg_agents(&s, "local", &[], &[], &[]).unwrap();
        let row = s
            .get_session("bg:bg-uuid-1", "local")
            .unwrap()
            .expect("ghosted, not yet deleted");
        assert_eq!(row.status, "ghost");
        assert!(row.lost_at.is_some());

        // Pass 3: still gone → hard-deleted.
        reconcile_bg_agents(&s, "local", &[], &[], &[]).unwrap();
        assert!(
            s.get_session("bg:bg-uuid-1", "local").unwrap().is_none(),
            "dead bg row must be reaped on the second missing pass"
        );
        assert!(s.get_session_by_id(id).unwrap().is_none());
    }

    #[test]
    fn reconcile_bg_agents_resurrects_ghost_when_agent_returns() {
        // A single missing pass (e.g. a transiently failed `claude agents`
        // probe, which comes back as an empty list) must not lose the row.
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let agents = vec![agent("bg-uuid-1", Some("my-bg-job"), Some("/tmp/proj"))];
        reconcile_bg_agents(&s, "local", &[], &[], &agents).unwrap();
        reconcile_bg_agents(&s, "local", &[], &[], &[]).unwrap(); // ghosts it
        reconcile_bg_agents(&s, "local", &[], &[], &agents).unwrap(); // returns

        let row = s
            .get_session("bg:bg-uuid-1", "local")
            .unwrap()
            .expect("row survives a one-pass blip");
        assert_eq!(row.status, "running");
        assert_eq!(row.lost_at, None);

        // And it is NOT deleted on the next pass with the agent still live.
        reconcile_bg_agents(&s, "local", &[], &[], &agents).unwrap();
        assert!(s.get_session("bg:bg-uuid-1", "local").unwrap().is_some());
    }

    #[test]
    fn reconcile_bg_agents_cleanup_spares_other_hosts_and_tmux_rows() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_host("remote").unwrap();
        // A bg row on ANOTHER host and a normal tmux row on this host.
        s.upsert_bg_session("remote", "bg:other", None, "other", Some("working"), 1)
            .unwrap();
        s.upsert_session("work-a", "local", None, None, 1, 1, "running", None)
            .unwrap();

        // Two empty-agent passes on `local` — enough to ghost + delete any
        // bg row this cleanup wrongly considered.
        reconcile_bg_agents(&s, "local", &[], &[], &[]).unwrap();
        reconcile_bg_agents(&s, "local", &[], &[], &[]).unwrap();

        let work = s.get_session("work-a", "local").unwrap().expect("tmux row");
        assert_eq!(work.status, "running", "tmux rows are not the bg pruner's");
        let other = s
            .get_session("bg:other", "remote")
            .unwrap()
            .expect("other host's bg row");
        assert_eq!(other.status, "running");
    }

    #[test]
    fn remote_project_path_returns_project_root_for_main_or_no_worktree() {
        use crate::projects::Layout;
        let root_dir = "/home/mjanci/projects/github.com";
        let (root, cwd) = remote_project_path(
            root_dir,
            Layout::Github,
            "martin-janci",
            "claude-fleet",
            None,
        );
        assert_eq!(
            root,
            "/home/mjanci/projects/github.com/martin-janci/claude-fleet"
        );
        assert_eq!(cwd, root);

        let (root, cwd) = remote_project_path(
            root_dir,
            Layout::Github,
            "papayapos",
            "pos-frontend",
            Some("main"),
        );
        assert_eq!(cwd, root);
    }

    #[test]
    fn remote_project_path_uses_worktree_subdir_for_non_main() {
        let (root, cwd) = remote_project_path(
            "/home/mjanci/projects/github.com",
            crate::projects::Layout::Github,
            "martin-janci",
            "sales-twins-app",
            Some("feature-x"),
        );
        assert_eq!(
            root,
            "/home/mjanci/projects/github.com/martin-janci/sales-twins-app"
        );
        assert_eq!(
            cwd,
            "/home/mjanci/projects/github.com/martin-janci/sales-twins-app/.claude/worktrees/feature-x"
        );
    }

    #[test]
    fn remote_new_session_path_unchanged_without_a_setting() {
        // Existing configs: exactly the pre-setting `{home}/projects/github.com/...`.
        let s = Store::open_in_memory().unwrap();
        let (root, cwd) =
            remote_project_path_for(&s, "mefistos", "/home/mjanci", "o", "r", Some("wt"));
        assert_eq!(root, "/home/mjanci/projects/github.com/o/r");
        assert_eq!(
            cwd,
            "/home/mjanci/projects/github.com/o/r/.claude/worktrees/wt"
        );
    }

    #[test]
    fn remote_new_session_path_follows_the_projects_settings() {
        use crate::service::settings;
        let s = Store::open_in_memory().unwrap();
        settings::set(
            &s,
            settings::PROJECTS_BASE_PATH,
            r#"{"mefistos":"~/code","other":"/data/git"}"#,
        )
        .unwrap();
        // github layout under the host's own root, `~/` expanded remotely
        let (root, _) = remote_project_path_for(&s, "mefistos", "/home/mjanci", "o", "r", None);
        assert_eq!(root, "/home/mjanci/code/o/r");
        // flat layout
        settings::set(&s, settings::PROJECTS_LAYOUT, "flat").unwrap();
        let (root, cwd) =
            remote_project_path_for(&s, "mefistos", "/home/mjanci", "o", "r", Some("feat"));
        assert_eq!(root, "/home/mjanci/code/r");
        assert_eq!(cwd, "/home/mjanci/code/r/.claude/worktrees/feat");
        // absolute per-host root; a host without an entry gets the flat default
        let (root, _) = remote_project_path_for(&s, "other", "/home/x", "o", "r", None);
        assert_eq!(root, "/data/git/r");
        let (root, _) = remote_project_path_for(&s, "third", "/home/x", "o", "r", None);
        assert_eq!(root, "/home/x/projects/r");
    }

    #[test]
    fn upsert_session_preserves_account_uuid_when_passed_existing_value() {
        use crate::store::{AccountRow, Store};
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        s.upsert_account(&AccountRow {
            uuid: "u1".into(),
            email: None,
            display_name: None,
            organization_name: None,
            organization_uuid: None,
            seat_tier: None,
            last_seen_at: None,
        })
        .unwrap();
        // First reconcile captures host's account
        s.upsert_session("dev-a", "h", None, None, 1, 100, "running", Some("u1"))
            .unwrap();
        // Host re-auths into a different account
        s.upsert_account(&AccountRow {
            uuid: "u2".into(),
            email: None,
            display_name: None,
            organization_name: None,
            organization_uuid: None,
            seat_tier: None,
            last_seen_at: None,
        })
        .unwrap();
        // Second reconcile: caller reads existing account before upsert
        let preserved = s.get_session_account("h", "dev-a").unwrap();
        s.upsert_session(
            "dev-a",
            "h",
            None,
            None,
            1,
            200,
            "running",
            preserved.as_deref(), // u1
        )
        .unwrap();
        // Verify session kept the ORIGINAL account
        assert_eq!(
            s.get_session_account("h", "dev-a").unwrap().as_deref(),
            Some("u1")
        );
    }

    #[test]
    fn build_send_commands_emits_literal_text_then_enter() {
        let cmds = build_send_commands("dev-foo", "hello world", true);
        assert_eq!(cmds.len(), 3);
        assert!(cmds[0].starts_with("tmux send-keys -t "));
        assert!(cmds[0].contains(" -l "));
        assert!(cmds[0].contains("'hello world'"));
        assert!(cmds.last().unwrap().ends_with(" Enter"));
    }

    #[test]
    fn build_send_commands_escapes_embedded_quotes() {
        let cmds = build_send_commands("dev-foo", "it's a test", true);
        // quote uses the '\''..  dance for embedded singles.
        assert!(cmds[0].contains("'it'\\''s a test'"));
    }

    #[test]
    fn build_send_commands_quotes_session_name_with_dashes() {
        let cmds = build_send_commands("dev-with-dashes", "x", true);
        assert!(cmds[0].contains("'dev-with-dashes'"));
    }

    #[test]
    fn send_commands_strip_trailing_newline_and_submit_once() {
        let cmds = build_send_commands("dev-x", "line1\nline2\n", true);
        // body preserves the internal newline, trailing newline stripped
        assert!(cmds
            .iter()
            .any(|c| c.contains("-l") && c.contains("line1") && c.contains("line2")));
        // the literal body must not carry the trailing newline
        assert!(!cmds.iter().any(|c| c.contains("line2\n")));
        // exactly one Enter/submit, with a settle before it
        let enters = cmds.iter().filter(|c| c.ends_with("Enter")).count();
        assert_eq!(enters, 1);
        assert!(cmds.iter().any(|c| c.contains("sleep")));
    }

    #[test]
    fn send_commands_no_submit_when_submit_false() {
        let cmds = build_send_commands("dev-x", "stage me", false);
        assert!(cmds.iter().all(|c| !c.ends_with("Enter")));
    }

    #[tokio::test]
    async fn parallel_reconcile_does_not_serialise_on_slow_host() {
        use crate::tmux::TmuxSession;
        use async_trait::async_trait;
        use std::time::Duration;

        struct SleepyTmux {
            sleep_ms: u64,
        }

        #[async_trait]
        impl TmuxExec for SleepyTmux {
            async fn list_sessions(&self) -> Result<Vec<TmuxSession>, IpcError> {
                tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
                Ok(Vec::new())
            }
            async fn new_session(
                &self,
                _name: &str,
                _cwd: &std::path::Path,
                _pane_cmd: &str,
            ) -> Result<(), IpcError> {
                Ok(())
            }
            async fn kill_session(&self, _name: &str) -> Result<(), IpcError> {
                Ok(())
            }
            async fn rename_session(&self, _old: &str, _new: &str) -> Result<(), IpcError> {
                Ok(())
            }
            async fn restart_session(&self, _name: &str, _pane_cmd: &str) -> Result<(), IpcError> {
                Ok(())
            }
            async fn capture_pane(&self, _name: &str) -> Result<String, IpcError> {
                Ok(String::new())
            }
            async fn capture_pane_scrollback(
                &self,
                _name: &str,
                _lines: u32,
            ) -> Result<String, IpcError> {
                Ok(String::new())
            }
            async fn list_claude_agents(&self) -> Vec<crate::claude_agents::ClaudeAgentRow> {
                vec![]
            }
        }

        // Spawn 3 tasks with sleeps 50ms, 500ms, 50ms.
        // Sequential sum ≈ 600ms; parallel max ≈ 500ms.
        let mut set = tokio::task::JoinSet::new();
        let start = std::time::Instant::now();
        for ms in [50u64, 500, 50] {
            set.spawn(async move { SleepyTmux { sleep_ms: ms }.list_sessions().await });
        }
        while set.join_next().await.is_some() {}
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(700),
            "parallel reconcile took {elapsed:?}, expected ≈max not sum",
        );
    }

    #[tokio::test]
    async fn wedged_host_probe_times_out_into_unreachable() {
        // Regression: a host whose probe hangs far past the cap must still
        // resolve — as an Err result (→ "unreachable, keep last-known
        // sessions") — and at ~the cap, not the hang length. Without the
        // timeout this future never completes, which is exactly what left the
        // whole sidebar empty when one host's ssh ControlMaster wedged: the
        // multi-host collector awaits EVERY probe before list_sessions returns.
        use crate::tmux::TmuxSession;
        use async_trait::async_trait;
        use std::time::Duration;

        struct HangingTmux;
        #[async_trait]
        impl TmuxExec for HangingTmux {
            async fn list_sessions(&self) -> Result<Vec<TmuxSession>, IpcError> {
                tokio::time::sleep(Duration::from_secs(3600)).await; // never within the test
                Ok(Vec::new())
            }
            async fn new_session(
                &self,
                _n: &str,
                _c: &std::path::Path,
                _p: &str,
            ) -> Result<(), IpcError> {
                Ok(())
            }
            async fn kill_session(&self, _n: &str) -> Result<(), IpcError> {
                Ok(())
            }
            async fn rename_session(&self, _o: &str, _n: &str) -> Result<(), IpcError> {
                Ok(())
            }
            async fn restart_session(&self, _n: &str, _p: &str) -> Result<(), IpcError> {
                Ok(())
            }
            async fn capture_pane(&self, _n: &str) -> Result<String, IpcError> {
                Ok(String::new())
            }
            async fn capture_pane_scrollback(&self, _n: &str, _l: u32) -> Result<String, IpcError> {
                Ok(String::new())
            }
            async fn list_claude_agents(&self) -> Vec<crate::claude_agents::ClaudeAgentRow> {
                vec![]
            }
        }

        // A real HostRow, without standing up ssh.
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("wedged").unwrap();
        let host = store
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.alias == "wedged")
            .expect("host row");

        let start = std::time::Instant::now();
        let before = now_unix();
        let probe =
            probe_with_timeout(host, Box::new(HangingTmux), Duration::from_millis(80), None).await;
        let elapsed = start.elapsed();

        assert_eq!(
            probe.host.alias, "wedged",
            "host identity preserved for the writer"
        );
        assert!(
            probe.result.is_err(),
            "wedged probe must surface as Err → unreachable"
        );
        assert!(probe.agent_rows.is_empty());
        assert!(probe.intel.is_empty());
        assert!(
            probe.started_at >= before && probe.started_at <= now_unix(),
            "probe start is stamped even on timeout"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "must return at ~the cap, not the 3600s hang; took {elapsed:?}",
        );
    }

    /// Scriptable executor for the reconcile-core tests: returns a fixed
    /// session list after `delay` (or never, when `hang`), and counts how many
    /// probes hit it so a test can prove "zero probes" / "exactly one probe".
    struct ScriptedTmux {
        sessions: Vec<crate::tmux::TmuxSession>,
        delay: std::time::Duration,
        hang: bool,
        probes: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl TmuxExec for ScriptedTmux {
        async fn list_sessions(&self) -> Result<Vec<crate::tmux::TmuxSession>, IpcError> {
            self.probes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.hang {
                std::future::pending::<()>().await;
            }
            tokio::time::sleep(self.delay).await;
            Ok(self.sessions.clone())
        }
        async fn new_session(
            &self,
            _n: &str,
            _c: &std::path::Path,
            _p: &str,
        ) -> Result<(), IpcError> {
            Ok(())
        }
        async fn kill_session(&self, _n: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn rename_session(&self, _o: &str, _n: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn restart_session(&self, _n: &str, _p: &str) -> Result<(), IpcError> {
            Ok(())
        }
        async fn capture_pane(&self, _n: &str) -> Result<String, IpcError> {
            Ok(String::new())
        }
        async fn capture_pane_scrollback(&self, _n: &str, _l: u32) -> Result<String, IpcError> {
            Ok(String::new())
        }
        async fn list_claude_agents(&self) -> Vec<crate::claude_agents::ClaudeAgentRow> {
            vec![]
        }
    }

    fn tmux_session(name: &str) -> crate::tmux::TmuxSession {
        crate::tmux::TmuxSession {
            name: name.to_string(),
            created: 1,
            last_activity: 1,
            attached: false,
            path: PathBuf::from("/tmp"),
        }
    }

    /// Deps whose `local` host answers with `local_sessions` after `delay`
    /// and whose every other host hangs forever. `probes` counts list calls
    /// across all hosts.
    fn scripted_deps(
        local_sessions: Vec<crate::tmux::TmuxSession>,
        delay: std::time::Duration,
        probe_timeout: std::time::Duration,
    ) -> (Arc<ReconcileDeps>, Arc<std::sync::atomic::AtomicUsize>) {
        let probes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let probes_for_exec = Arc::clone(&probes);
        let deps = ReconcileDeps::fake(
            move |alias| {
                Box::new(ScriptedTmux {
                    sessions: if alias == "local" {
                        local_sessions.clone()
                    } else {
                        Vec::new()
                    },
                    delay,
                    hang: alias != "local",
                    probes: Arc::clone(&probes_for_exec),
                })
            },
            probe_timeout,
        );
        (deps, probes)
    }

    #[tokio::test]
    async fn fleet_reconcile_completes_when_one_host_never_answers() {
        // BE-1 (d): the multi-host fan-out must finish — and write the healthy
        // host's rows — when another host's probe never returns. The dead host
        // is treated exactly like an unreachable one (reachable=false,
        // last-known rows kept).
        use std::time::Duration;
        let store = Mutex::new(Store::open_in_memory().expect("store"));
        {
            let s = store.lock().unwrap();
            s.upsert_host("wedged").unwrap();
            s.upsert_session("wedged-old", "wedged", None, None, 1, 1, "running", None)
                .unwrap();
        }
        let (deps, _probes) = scripted_deps(
            vec![tmux_session("local-live")],
            Duration::from_millis(10),
            Duration::from_millis(150),
        );
        let start = std::time::Instant::now();
        reconcile_sessions_with(&store, &deps)
            .await
            .expect("fan-out completes");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "one dead host must not block the pass; took {:?}",
            start.elapsed()
        );
        let s = store.lock().unwrap();
        let wedged = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.alias == "wedged")
            .unwrap();
        assert!(!wedged.reachable, "timed-out host is marked unreachable");
        let kept = s.list_sessions_for_host("wedged").unwrap();
        assert_eq!(kept.len(), 1, "last-known rows kept on the dead host");
        assert_eq!(kept[0].status, "running", "not ghosted by a timeout");
        let local = s.list_sessions_for_host("local").unwrap();
        assert_eq!(local.len(), 1, "healthy host's rows were written");
        assert_eq!(local[0].tmux_name, "local-live");
    }

    #[tokio::test]
    async fn concurrent_list_sessions_share_one_reconcile_pass() {
        // BE-2: two callers racing into `list_sessions` (UI focus + MCP tool,
        // say) must cause ONE fleet probe; the loser is served the stored
        // rows immediately instead of queueing a second pass.
        use std::time::Duration;
        let store = Mutex::new(Store::open_in_memory().expect("store"));
        let gate = ReconcileGate::new();
        let (deps, probes) = scripted_deps(
            vec![tmux_session("s1")],
            Duration::from_millis(200),
            Duration::from_secs(5),
        );
        let window = Duration::from_secs(60);
        let (a, b) = tokio::join!(
            list_sessions_with(&store, &deps, &gate, window, false),
            list_sessions_with(&store, &deps, &gate, window, false),
        );
        a.expect("first caller ok");
        b.expect("second caller ok");
        assert_eq!(
            gate.passes(),
            1,
            "exactly one pass for two concurrent callers"
        );
        assert_eq!(
            probes.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "only the `local` host was probed, once"
        );
        // The winner (whichever it was) got the fresh row; the store now has it.
        let rows = list_sessions_with(&store, &deps, &gate, window, false)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tmux_name, "s1");
    }

    #[tokio::test]
    async fn list_sessions_within_freshness_window_causes_zero_probes() {
        // BE-2: a store that was reconciled within the interval is served as
        // is; `force` (the explicit-refresh path) still probes.
        use std::time::Duration;
        let store = Mutex::new(Store::open_in_memory().expect("store"));
        let gate = ReconcileGate::new();
        let (deps, probes) = scripted_deps(
            vec![tmux_session("s1")],
            Duration::from_millis(1),
            Duration::from_secs(5),
        );
        let window = Duration::from_secs(60);
        // First call: nothing completed yet → one pass.
        list_sessions_with(&store, &deps, &gate, window, false)
            .await
            .unwrap();
        assert_eq!(probes.load(std::sync::atomic::Ordering::SeqCst), 1);
        // Within the window: served from the store, zero new probes.
        for _ in 0..3 {
            let rows = list_sessions_with(&store, &deps, &gate, window, false)
                .await
                .unwrap();
            assert_eq!(rows.len(), 1, "stored rows are returned");
        }
        assert_eq!(
            probes.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "fresh store ⇒ no probe"
        );
        assert_eq!(gate.passes(), 1);
        // Explicit refresh ignores freshness.
        list_sessions_with(&store, &deps, &gate, window, true)
            .await
            .unwrap();
        assert_eq!(probes.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(gate.passes(), 2);
        // A zero-length window means "always stale".
        list_sessions_with(&store, &deps, &gate, Duration::ZERO, false)
            .await
            .unwrap();
        assert_eq!(gate.passes(), 3);
    }

    #[tokio::test]
    async fn failed_pass_leaves_gate_stale_so_next_caller_retries() {
        // A pass that errors must not stamp `last_completed`; the next caller
        // probes again instead of trusting a store that never got written.
        let gate = ReconcileGate::new();
        {
            let pass = gate.try_begin().expect("free gate");
            drop(pass); // errored / aborted: no `complete()`
        }
        assert!(!gate.is_fresh(std::time::Duration::from_secs(60)));
        assert_eq!(gate.passes(), 0);
        let pass = gate.try_begin().expect("released after drop");
        assert!(gate.try_begin().is_none(), "single slot while a pass runs");
        pass.complete();
        assert!(gate.is_fresh(std::time::Duration::from_secs(60)));
        assert_eq!(gate.passes(), 1);
    }

    #[tokio::test]
    async fn stale_probe_write_does_not_ghost_session_created_after_probe_start() {
        // BE-3 end to end through the service writer: a tick's probe starts
        // (its `keep` set is frozen), `new_session` then creates + reconciles
        // a session on the same host, and only afterwards does the tick's
        // write land. The new row must survive; a probe that starts after the
        // create ghosts it as usual.
        let store = Mutex::new(Store::open_in_memory().expect("store"));
        let host = {
            let s = store.lock().unwrap();
            s.upsert_host("h").unwrap();
            s.list_hosts()
                .unwrap()
                .into_iter()
                .find(|h| h.alias == "h")
                .unwrap()
        };
        // 1. The stale tick probe starts: sees zero sessions.
        let stale = HostProbe {
            host: host.clone(),
            result: Ok(Vec::new()),
            agent_rows: Vec::new(),
            intel: PaneIntelMap::new(),
            pr_info: PrInfoMap::new(),
            started_at: now_unix(),
        };
        // 2. `new_session` creates the tmux session and runs its own
        //    single-host reconcile, which upserts + stamps the row.
        let (deps, _) = scripted_deps(
            vec![tmux_session("brand-new")],
            std::time::Duration::from_millis(1),
            std::time::Duration::from_secs(5),
        );
        // Point the fake at host `h` instead of `local`.
        let deps_h = ReconcileDeps::fake(
            move |_alias| (deps.exec)("local"),
            std::time::Duration::from_secs(5),
        );
        reconcile_one_host_with(&store, &deps_h, "h")
            .await
            .expect("create's reconcile");
        {
            let s = store.lock().unwrap();
            let row = s
                .get_session("brand-new", "h")
                .unwrap()
                .expect("row exists");
            assert_eq!(row.status, "running");
        }
        // 3. The stale write lands.
        {
            let mut s = store.lock().unwrap();
            let projects = s.list_projects().unwrap();
            reconcile_write_one_host(&mut s, &stale, &projects).expect("stale write ok");
            let row = s.get_session("brand-new", "h").unwrap().unwrap();
            assert_eq!(
                row.status, "running",
                "row reconciled after the stale probe started must not be ghosted"
            );
            assert!(row.lost_at.is_none());
        }
        // 4. A probe that starts strictly after the create is authoritative.
        let later = HostProbe {
            host,
            result: Ok(Vec::new()),
            agent_rows: Vec::new(),
            intel: PaneIntelMap::new(),
            pr_info: PrInfoMap::new(),
            started_at: now_unix() + 5,
        };
        let mut s = store.lock().unwrap();
        let projects = s.list_projects().unwrap();
        reconcile_write_one_host(&mut s, &later, &projects).unwrap();
        let row = s.get_session("brand-new", "h").unwrap().unwrap();
        assert_eq!(row.status, "ghost", "a later probe ghosts it normally");
    }

    #[tokio::test]
    async fn reconcile_one_host_does_not_touch_other_hosts() {
        // Exercises the Store-level invariant: a write burst targeting host
        // 'alpha' must leave host 'beta's session rows untouched.
        let store = Mutex::new(Store::open_in_memory().expect("store"));
        {
            let s = store.lock().unwrap();
            s.upsert_host("alpha").unwrap();
            s.upsert_host("beta").unwrap();
            s.upsert_session("alpha-s", "alpha", None, None, 1, 1, "running", None)
                .unwrap();
            s.upsert_session("beta-s", "beta", None, None, 1, 1, "running", None)
                .unwrap();
        }
        // Simulate "alpha was probed and has zero sessions" — directly call the
        // delete helper that reconcile_one_host uses internally.
        {
            let s = store.lock().unwrap();
            s.delete_sessions_not_in("alpha", &[]).unwrap();
        }
        let s = store.lock().unwrap();
        let alpha = s.list_sessions_for_host("alpha").unwrap();
        let beta = s.list_sessions_for_host("beta").unwrap();
        assert!(alpha.is_empty(), "alpha cleared");
        assert_eq!(beta.len(), 1, "beta untouched");
        assert_eq!(beta[0].tmux_name, "beta-s");
    }

    #[test]
    fn upsert_session_captures_new_account_for_fresh_row() {
        use crate::store::AccountRow;
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("h").unwrap();
        s.upsert_account(&AccountRow {
            uuid: "u1".into(),
            email: None,
            display_name: None,
            organization_name: None,
            organization_uuid: None,
            seat_tier: None,
            last_seen_at: None,
        })
        .unwrap();
        // Brand new session — no existing row
        assert!(s.get_session_account("h", "dev-new").unwrap().is_none());
        let preserved = s.get_session_account("h", "dev-new").unwrap();
        let account = preserved.or(Some("u1".to_string()));
        s.upsert_session(
            "dev-new",
            "h",
            None,
            None,
            1,
            100,
            "running",
            account.as_deref(),
        )
        .unwrap();
        assert_eq!(
            s.get_session_account("h", "dev-new").unwrap().as_deref(),
            Some("u1")
        );
    }

    #[tokio::test]
    async fn wait_for_repl_ready_returns_once_prompt_appears() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc as StdArc;

        struct FakeTmux {
            calls: StdArc<AtomicU32>,
        }
        #[async_trait::async_trait]
        impl TmuxExec for FakeTmux {
            async fn list_sessions(&self) -> Result<Vec<crate::tmux::TmuxSession>, IpcError> {
                Ok(vec![])
            }
            async fn new_session(
                &self,
                _: &str,
                _: &std::path::Path,
                _: &str,
            ) -> Result<(), IpcError> {
                Ok(())
            }
            async fn kill_session(&self, _: &str) -> Result<(), IpcError> {
                Ok(())
            }
            async fn rename_session(&self, _: &str, _: &str) -> Result<(), IpcError> {
                Ok(())
            }
            async fn restart_session(&self, _: &str, _: &str) -> Result<(), IpcError> {
                Ok(())
            }
            async fn capture_pane(&self, _: &str) -> Result<String, IpcError> {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                // Not ready for the first 2 polls, then the prompt appears.
                if n < 2 {
                    Ok("starting…".into())
                } else {
                    Ok("│ > ".into())
                }
            }
            async fn capture_pane_scrollback(
                &self,
                _name: &str,
                _lines: u32,
            ) -> Result<String, IpcError> {
                Ok(String::new())
            }
            async fn list_claude_agents(&self) -> Vec<crate::claude_agents::ClaudeAgentRow> {
                vec![]
            }
        }

        let calls = StdArc::new(AtomicU32::new(0));
        let tmux = FakeTmux {
            calls: calls.clone(),
        };
        let start = std::time::Instant::now();
        wait_for_repl_ready(&tmux, "x").await;
        // Returned after ~3 polls (~600ms), well under the 6s cap.
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        assert!(calls.load(Ordering::SeqCst) >= 3);
    }

    #[test]
    fn resolve_session_cwd_prefers_worktree_then_project_then_errors() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        // A session with neither worktree nor project → E_NOREPO.
        s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
            .unwrap();
        let row = s.get_session("dev", "local").unwrap().unwrap();
        let err = resolve_session_cwd(&s, &row).unwrap_err();
        assert_eq!(err.code, "E_NOREPO");
    }

    #[test]
    fn resolve_session_cwd_with_worktree_and_project_and_neither() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("alpha").unwrap();
        // Project with a base_path, and a worktree under it.
        let pid = store.upsert_project("o", "r", "/base/r").unwrap();
        let wid = store
            .upsert_worktree(pid, "main", "/base/r/main", None)
            .unwrap();
        // Session with worktree → worktree path wins.
        let s1 = store
            .upsert_session("s1", "alpha", Some(pid), Some(wid), 1, 1, "running", None)
            .unwrap();
        let row1 = store.get_session_by_id(s1).unwrap().unwrap();
        assert_eq!(resolve_session_cwd(&store, &row1).unwrap(), "/base/r/main");
        // Session with project but no worktree → project base.
        let s2 = store
            .upsert_session("s2", "alpha", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        let row2 = store.get_session_by_id(s2).unwrap().unwrap();
        assert_eq!(resolve_session_cwd(&store, &row2).unwrap(), "/base/r");
        // Session with neither → error.
        let s3 = store
            .upsert_session("s3", "alpha", None, None, 1, 1, "running", None)
            .unwrap();
        let row3 = store.get_session_by_id(s3).unwrap().unwrap();
        assert!(resolve_session_cwd(&store, &row3).is_err());
    }

    #[test]
    fn resolve_session_cwd_honors_worktree_key_when_id_missing() {
        // Reproduces the recreate bug: reconcile sets `worktree_key` but never
        // `worktree_id`, so a session in a worktree must still resolve to that
        // worktree's path, not the repo root.
        let store = Store::open_in_memory().unwrap();
        store.upsert_host("local").unwrap();
        let pid = store.upsert_project("o", "r", "/base/r").unwrap();
        store
            .upsert_worktree(pid, "feat-x", "/base/r/.claude/worktrees/feat-x", None)
            .unwrap();

        // worktree_id is None (as after reconcile) but worktree_key points at it.
        let mut r = row(1, "local", "dev", "work", Some(pid), Some("idle"));
        r.worktree_key = Some("feat-x".into());
        assert_eq!(
            resolve_session_cwd(&store, &r).unwrap(),
            "/base/r/.claude/worktrees/feat-x"
        );

        // worktree_key "main" → repo root.
        let mut rm = row(2, "local", "dev2", "work", Some(pid), Some("idle"));
        rm.worktree_key = Some("main".into());
        assert_eq!(resolve_session_cwd(&store, &rm).unwrap(), "/base/r");

        // Unknown key (worktree not in the table) → graceful fallback to root.
        let mut ru = row(3, "local", "dev3", "work", Some(pid), Some("idle"));
        ru.worktree_key = Some("gone".into());
        assert_eq!(resolve_session_cwd(&store, &ru).unwrap(), "/base/r");
    }

    #[test]
    fn worktree_path_on_disk_tries_both_layouts() {
        let base = "/base/r";
        // `.worktrees/<key>` layout (the case the stale-table bug hit).
        let only_dot_worktrees = worktree_path_on_disk(base, "test-worktree", |p| {
            p == "/base/r/.worktrees/test-worktree"
        });
        assert_eq!(
            only_dot_worktrees.as_deref(),
            Some("/base/r/.worktrees/test-worktree")
        );
        // `.claude/worktrees/<key>` is preferred when both exist.
        let both = worktree_path_on_disk(base, "feat", |_| true);
        assert_eq!(both.as_deref(), Some("/base/r/.claude/worktrees/feat"));
        // Neither present → None (caller falls back to the repo root).
        assert_eq!(worktree_path_on_disk(base, "gone", |_| false), None);
    }

    #[test]
    fn cwd_source_remote_honors_worktree_key_when_id_missing() {
        let store = Store::open_in_memory().unwrap();
        store.upsert_host("mefistos").unwrap();
        let pid = store.upsert_project("acme", "repo", "/base/repo").unwrap();

        let mut r = row(1, "mefistos", "dev", "work", Some(pid), Some("idle"));
        r.worktree_key = Some("feat-x".into());
        match cwd_source_for_session(&store, &r).unwrap() {
            CwdSource::Remote {
                owner,
                repo,
                wt_name,
                ..
            } => {
                assert_eq!(owner, "acme");
                assert_eq!(repo, "repo");
                assert_eq!(wt_name, Some("feat-x".to_string()));
            }
            CwdSource::Local(_) => panic!("expected Remote for host=mefistos"),
        }

        // "main" carries no worktree name → project root on the remote.
        let mut rm = row(2, "mefistos", "dev2", "work", Some(pid), Some("idle"));
        rm.worktree_key = Some("main".into());
        match cwd_source_for_session(&store, &rm).unwrap() {
            CwdSource::Remote { wt_name, .. } => assert_eq!(wt_name, None),
            CwdSource::Local(_) => panic!("expected Remote for host=mefistos"),
        }
    }

    #[test]
    fn cwd_source_remote_follows_projects_settings() {
        // recreate/restart derive the remote cwd through `cwd_source_for_session`
        // + `resolve_cwd_source`; the path must follow `projects.*`.
        use crate::projects::Layout;
        use crate::service::settings;
        let store = Store::open_in_memory().unwrap();
        store.upsert_host("mefistos").unwrap();
        let pid = store.upsert_project("acme", "repo", "/base/repo").unwrap();
        let mut r = row(1, "mefistos", "dev", "work", Some(pid), Some("idle"));
        r.worktree_key = Some("feat-x".into());

        // No setting: the historical remote root, unchanged.
        match cwd_source_for_session(&store, &r).unwrap() {
            CwdSource::Remote { root, layout, .. } => {
                assert_eq!(root, "~/projects/github.com");
                assert_eq!(layout, Layout::Github);
            }
            CwdSource::Local(_) => panic!("expected Remote for host=mefistos"),
        }

        settings::set(
            &store,
            settings::PROJECTS_BASE_PATH,
            r#"{"mefistos":"~/code"}"#,
        )
        .unwrap();
        settings::set(&store, settings::PROJECTS_LAYOUT, "flat").unwrap();
        match cwd_source_for_session(&store, &r).unwrap() {
            CwdSource::Remote {
                root,
                layout,
                owner,
                repo,
                wt_name,
            } => {
                let root = crate::service::projects::expand_home(&root, "/home/m");
                let (_, cwd) =
                    remote_project_path(&root, layout, &owner, &repo, wt_name.as_deref());
                assert_eq!(cwd, "/home/m/code/repo/.claude/worktrees/feat-x");
            }
            CwdSource::Local(_) => panic!("expected Remote for host=mefistos"),
        }
    }

    fn host_paths(root: &str, layout: crate::projects::Layout) -> HostPaths {
        HostPaths {
            root: root.into(),
            layout,
            worktrees: Vec::new(),
        }
    }

    #[test]
    fn host_paths_locate_custom_roots_and_layouts() {
        use crate::projects::Layout;
        let def = host_paths("~/projects/github.com", Layout::Github);
        assert_eq!(
            def.locate("/home/u/projects/github.com/o/r/.worktrees/f"),
            Some((Some("o"), "r", "/.worktrees/f".to_string()))
        );
        let code = host_paths("~/code", Layout::Github);
        assert_eq!(
            code.locate("/home/u/code/o/r"),
            Some((Some("o"), "r", String::new()))
        );
        assert_eq!(code.locate("/home/u/code/o"), None, "owner dir, no repo");
        let abs = host_paths("/data/git/", Layout::Flat);
        assert_eq!(
            abs.locate("/data/git/r/src"),
            Some((None, "r", "/src".to_string()))
        );
        assert_eq!(abs.locate("/data/git-old/r"), None, "whole components only");
        assert_eq!(abs.locate("/data/git"), None);
    }

    #[test]
    fn host_paths_tilde_roots_anchor_after_home() {
        use crate::projects::Layout;
        let home = host_paths("~", Layout::Flat);
        assert_eq!(
            home.locate("/home/u/r/src"),
            Some((None, "r", "/src".to_string()))
        );
        assert_eq!(home.locate("/Users/u/r"), Some((None, "r", String::new())));
        assert_eq!(home.locate("/root/r"), Some((None, "r", String::new())));
        assert_eq!(
            home.locate("/var/home/u/r"),
            Some((None, "r", String::new()))
        );
        assert_eq!(home.locate("/home/u"), None, "the home itself is no repo");
        assert_eq!(home.locate("/srv/r"), None, "unknown home layout");
        let gh = host_paths("~/", Layout::Github);
        assert_eq!(
            gh.locate("/home/u/o/r"),
            Some((Some("o"), "r", String::new()))
        );
        let code = host_paths("~/code", Layout::Github);
        assert_eq!(
            code.locate("/home/u/work/code/o/r"),
            None,
            "a `/code/` run deeper in the path is not the root"
        );
        assert_eq!(
            code.locate("/home/code/code/o/r"),
            Some((Some("o"), "r", String::new())),
            "a user named `code` is not mistaken for the root"
        );
        // Unknown home layout: the unanchored fallback still applies.
        assert_eq!(
            code.locate("/data/users/u/code/o/r"),
            Some((Some("o"), "r", String::new()))
        );
    }

    #[test]
    fn below_home_handles_standard_layouts() {
        assert_eq!(below_home("/home/u/a/b"), Some("a/b"));
        assert_eq!(below_home("/home/u/"), Some(""));
        assert_eq!(below_home("/home/u"), Some(""));
        assert_eq!(below_home("/home"), None);
        assert_eq!(below_home("/root"), Some(""));
        assert_eq!(below_home("/Users/u/p"), Some("p"));
        assert_eq!(below_home("/opt/x"), None);
        assert_eq!(below_home("relative/x"), None);
    }

    #[cfg(unix)]
    #[test]
    fn find_project_local_matches_through_a_symlinked_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let real = tmp.path().join("mnt").join("o").join("r");
        std::fs::create_dir_all(real.join(".worktrees").join("f")).unwrap();
        let link = tmp.path().join("projects");
        std::os::unix::fs::symlink(tmp.path().join("mnt"), &link).unwrap();
        let s = Store::open_in_memory().unwrap();
        // refresh_projects stores the physical base_path.
        let base = crate::projects::path_identity::canonical(&real);
        let pid = s.upsert_project("o", "r", &base.to_string_lossy()).unwrap();
        let xb = s
            .upsert_project(
                "o",
                "r-build",
                &base.with_file_name("r-build").to_string_lossy(),
            )
            .unwrap();
        let projects = s.list_projects().unwrap();
        let paths = HostPaths::for_host(&s, "local");
        let find = |p: &std::path::Path| find_project_id_for_path(&projects, "local", p, &paths);
        let logical = link.join("o").join("r");
        assert_eq!(
            find(&logical.join(".worktrees").join("f")),
            Some(pid),
            "a logical pane PWD links to the physical row"
        );
        assert_eq!(find(&base), Some(pid));
        assert_eq!(find(&link.join("o").join("r-build").join("src")), Some(xb));
        assert_eq!(find(&link.join("o").join("rx")), None);
    }

    /// A linked worktree in a SIBLING folder of the repo (outside the base
    /// layout, e.g. `o/stw-fix2` next to `o/sales-twins-app`) links to the
    /// repo's project through the host's worktree rows: locally the scan's
    /// `git worktree list`, on a remote host its EnterWorktree hooks.
    #[test]
    fn find_project_links_sibling_linked_worktrees_through_worktree_rows() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("vps").unwrap();
        let app = s.upsert_project("o", "app", "/b/o/app").unwrap();
        s.upsert_worktree(app, "main", "/b/o/app", None).unwrap();
        s.upsert_worktree(app, "app-wt", "/b/o/app-wt", Some("wt"))
            .unwrap();
        s.upsert_worktree_on(
            "vps",
            app,
            "app-wt",
            "/home/u/projects/github.com/o/app-wt",
            None,
        )
        .unwrap();
        let projects = s.list_projects().unwrap();
        let local = HostPaths::for_host(&s, "local");
        let find_local =
            |p: &str| find_project_id_for_path(&projects, "local", std::path::Path::new(p), &local);
        assert_eq!(find_local("/b/o/app-wt/src"), Some(app), "sibling worktree");
        assert_eq!(find_local("/b/o/app-wt-old"), None, "whole components only");
        assert_eq!(find_local("/b/o/app/.worktrees/f"), Some(app));
        // Another host's rows never leak into this host's linking.
        assert_eq!(find_local("/home/u/projects/github.com/o/app-wt"), None);
        let vps = HostPaths::for_host(&s, "vps");
        let find_vps =
            |p: &str| find_project_id_for_path(&projects, "vps", std::path::Path::new(p), &vps);
        // The layout reads repo `app-wt`, which is no project; the host's
        // worktree row names the real one.
        assert_eq!(
            find_vps("/home/u/projects/github.com/o/app-wt/src"),
            Some(app)
        );
        assert_eq!(find_vps("/b/o/app-wt/src"), None, "local rows stay local");
    }

    /// A leftover duplicate project whose base IS a linked worktree (such as
    /// `stw-fix2`, once scanned as its own repo and kept alive by a session)
    /// must not win over the real repo that lists that checkout. The matches
    /// tie on length and the worktree row wins, locally and remotely. A
    /// project root deeper than the worktree row still wins.
    #[test]
    fn find_project_prefers_the_worktree_row_over_a_duplicate_project() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("vps").unwrap();
        let app = s.upsert_project("o", "app", "/b/o/app").unwrap();
        s.upsert_project("o", "stw-fix2", "/b/o/stw-fix2").unwrap();
        s.upsert_worktree(app, "stw-fix2", "/b/o/stw-fix2", None)
            .unwrap();
        s.upsert_worktree_on(
            "vps",
            app,
            "stw-fix2",
            "/home/u/projects/github.com/o/stw-fix2",
            None,
        )
        .unwrap();
        let nested = s
            .upsert_project("o", "nested", "/b/o/stw-fix2/vendor/nested")
            .unwrap();
        let projects = s.list_projects().unwrap();
        let local = HostPaths::for_host(&s, "local");
        let vps = HostPaths::for_host(&s, "vps");
        let find = |host: &str, paths: &HostPaths, p: &str| {
            find_project_id_for_path(&projects, host, std::path::Path::new(p), paths)
        };
        assert_eq!(
            find("local", &local, "/b/o/stw-fix2/src"),
            Some(app),
            "the duplicate project loses the tie locally"
        );
        assert_eq!(
            find("vps", &vps, "/home/u/projects/github.com/o/stw-fix2/src"),
            Some(app),
            "the duplicate project located by the layout loses the tie remotely"
        );
        assert_eq!(
            find("local", &local, "/b/o/stw-fix2/vendor/nested/x"),
            Some(nested),
            "a deeper project root beats a shorter worktree row"
        );
    }

    #[test]
    fn find_project_local_prefix_is_component_aware() {
        let s = Store::open_in_memory().unwrap();
        let x = s.upsert_project("o", "x", "/b/x").unwrap();
        let xb = s.upsert_project("o", "x-build", "/b/x-build").unwrap();
        let projects = s.list_projects().unwrap();
        let paths = HostPaths::for_host(&s, "local");
        let find =
            |p: &str| find_project_id_for_path(&projects, "local", std::path::Path::new(p), &paths);
        assert_eq!(find("/b/x-build/src"), Some(xb));
        assert_eq!(find("/b/x/.worktrees/f"), Some(x));
        assert_eq!(find("/b/x"), Some(x));
        assert_eq!(find("/b/xy"), None);
    }

    #[test]
    fn find_project_remote_flat_matches_unique_repo_name() {
        let s = Store::open_in_memory().unwrap();
        let a = s.upsert_project("acme", "alpha", "/l/alpha").unwrap();
        s.upsert_project("one", "dup", "/l/dup").unwrap();
        s.upsert_project("two", "dup", "/l2/dup").unwrap();
        let projects = s.list_projects().unwrap();
        let paths = host_paths("~/code", crate::projects::Layout::Flat);
        let find =
            |p: &str| find_project_id_for_path(&projects, "vps", std::path::Path::new(p), &paths);
        assert_eq!(find("/home/u/code/alpha/src"), Some(a));
        assert_eq!(
            find("/home/u/code/dup"),
            None,
            "an ambiguous repo name is not guessed"
        );
        assert_eq!(find("/home/u/elsewhere/alpha"), None);
        // The github.com convention stays the fallback.
        assert_eq!(find("/home/u/projects/github.com/acme/alpha"), Some(a));
    }

    /// Run two reconcile ticks for one remote session at `cwd` under the given
    /// `projects.*` settings; returns (project_id, worktree_key, expected pid).
    fn reconcile_linking(
        base_map: Option<&str>,
        layout: &str,
        cwd: &str,
    ) -> (Option<i64>, Option<String>, i64) {
        use crate::service::settings;
        let mut s = Store::open_in_memory().unwrap();
        s.upsert_host("vps").unwrap();
        let pid = s
            .upsert_project("acme", "repo", "/local/acme/repo")
            .unwrap();
        if let Some(m) = base_map {
            settings::set(&s, settings::PROJECTS_BASE_PATH, m).unwrap();
        }
        settings::set(&s, settings::PROJECTS_LAYOUT, layout).unwrap();
        let host = s
            .list_hosts()
            .unwrap()
            .into_iter()
            .find(|h| h.alias == "vps")
            .unwrap();
        let projects = s.list_projects().unwrap();
        let probe = HostProbe {
            host,
            result: Ok(vec![crate::tmux::TmuxSession {
                name: "dev-a".into(),
                created: 1,
                last_activity: 1,
                attached: false,
                path: PathBuf::from(cwd),
            }]),
            agent_rows: Vec::new(),
            intel: PaneIntelMap::new(),
            pr_info: PrInfoMap::new(),
            started_at: now_unix(),
        };
        // Two ticks: the second must keep the link, not null it.
        reconcile_write_one_host(&mut s, &probe, &projects).unwrap();
        reconcile_write_one_host(&mut s, &probe, &projects).unwrap();
        let row = s.get_session("dev-a", "vps").unwrap().unwrap();
        (row.project_id, row.worktree_key, pid)
    }

    #[test]
    fn reconcile_keeps_links_under_custom_root() {
        let (pid, key, want) = reconcile_linking(
            Some(r#"{"vps":"~/code"}"#),
            "github",
            "/home/u/code/acme/repo/.worktrees/feat",
        );
        assert_eq!(pid, Some(want));
        assert_eq!(key.as_deref(), Some("feat"));
    }

    #[test]
    fn reconcile_keeps_links_under_flat_layout() {
        // No base set: the flat default `~/projects`.
        let (pid, key, want) =
            reconcile_linking(None, "flat", "/home/u/projects/repo/.claude/worktrees/x");
        assert_eq!(pid, Some(want));
        assert_eq!(key.as_deref(), Some("x"));
        // Absolute per-host root.
        let (pid, key, want) =
            reconcile_linking(Some(r#"{"vps":"/srv/git"}"#), "flat", "/srv/git/repo");
        assert_eq!(pid, Some(want));
        assert_eq!(key.as_deref(), Some("main"));
    }

    #[test]
    fn reconcile_default_config_links_exactly_as_before() {
        let (pid, key, want) =
            reconcile_linking(None, "github", "/home/u/projects/github.com/acme/repo/src");
        assert_eq!(pid, Some(want));
        assert_eq!(key.as_deref(), Some("main"));
        // Outside any root and outside the convention: orphan, as before.
        let (pid, key, _) = reconcile_linking(None, "github", "/tmp/elsewhere");
        assert_eq!(pid, None);
        assert_eq!(key, None);
    }

    #[test]
    fn cwd_source_local_uses_db_path_remote_uses_owner_repo() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("local").unwrap();
        store.upsert_host("mefistos").unwrap();
        let pid = store.upsert_project("acme", "repo", "/base/repo").unwrap();
        let wid = store
            .upsert_worktree(pid, "feat-x", "/base/repo/.claude/worktrees/feat-x", None)
            .unwrap();

        // LOCAL: takes the worktree's stored path verbatim.
        let lid = store
            .upsert_session("dev", "local", Some(pid), Some(wid), 1, 1, "running", None)
            .unwrap();
        let local_row = store.get_session_by_id(lid).unwrap().unwrap();
        match cwd_source_for_session(&store, &local_row).unwrap() {
            CwdSource::Local(p) => assert_eq!(p, "/base/repo/.claude/worktrees/feat-x"),
            CwdSource::Remote { .. } => panic!("expected Local for host=local"),
        }

        // REMOTE: captures (owner, repo, wt_name) — the local DB path is
        // unusable on the remote machine and must NOT leak into the cwd.
        let rid = store
            .upsert_session(
                "dev",
                "mefistos",
                Some(pid),
                Some(wid),
                1,
                1,
                "running",
                None,
            )
            .unwrap();
        let remote_row = store.get_session_by_id(rid).unwrap().unwrap();
        match cwd_source_for_session(&store, &remote_row).unwrap() {
            CwdSource::Remote {
                owner,
                repo,
                wt_name,
                ..
            } => {
                assert_eq!(owner, "acme");
                assert_eq!(repo, "repo");
                assert_eq!(wt_name.as_deref(), Some("feat-x"));
            }
            CwdSource::Local(_) => panic!("expected Remote for non-local host"),
        }

        // REMOTE without worktree → wt_name = None, so remote_project_path
        // returns the project root.
        let rid2 = store
            .upsert_session("dev2", "mefistos", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        let remote_row2 = store.get_session_by_id(rid2).unwrap().unwrap();
        match cwd_source_for_session(&store, &remote_row2).unwrap() {
            CwdSource::Remote { wt_name, .. } => assert!(wt_name.is_none()),
            CwdSource::Local(_) => panic!("expected Remote for non-local host"),
        }
    }

    #[test]
    fn cwd_source_remote_without_project_errors() {
        let store = Store::open_in_memory().expect("store");
        store.upsert_host("mefistos").unwrap();
        let id = store
            .upsert_session("orphan", "mefistos", None, None, 1, 1, "running", None)
            .unwrap();
        let row = store.get_session_by_id(id).unwrap().unwrap();
        let err = cwd_source_for_session(&store, &row).unwrap_err();
        assert_eq!(err.code, "E_NOREPO");
    }

    #[test]
    fn worktree_key_root_is_main_local_and_remote() {
        assert_eq!(
            worktree_key_for_path(
                "/Users/martinjanci/projects/github.com/martin-janci/claude-fleet"
            ),
            Some("main".to_string())
        );
        assert_eq!(
            worktree_key_for_path("/home/mjanci/projects/github.com/martin-janci/claude-fleet"),
            Some("main".to_string())
        );
    }

    #[test]
    fn worktree_key_extracts_named_worktree() {
        assert_eq!(
            worktree_key_for_path("/Users/x/projects/github.com/o/r/.claude/worktrees/feat-auth"),
            Some("feat-auth".to_string())
        );
        assert_eq!(
            worktree_key_for_path(
                "/home/mjanci/projects/github.com/o/r/.claude/worktrees/feat-auth/src"
            ),
            Some("feat-auth".to_string())
        );
    }

    #[test]
    fn worktree_key_extracts_dot_worktrees_named_worktree() {
        // The `.worktrees/` layout (no `.claude/` prefix) must also key to the
        // worktree name — otherwise these sessions recreate at the repo root.
        assert_eq!(
            worktree_key_for_path("/Users/x/projects/github.com/o/r/.worktrees/changelog"),
            Some("changelog".to_string())
        );
        assert_eq!(
            worktree_key_for_path("/home/mjanci/projects/github.com/o/r/.worktrees/changelog/src"),
            Some("changelog".to_string())
        );
    }

    #[test]
    fn worktree_key_other_subdir_is_main() {
        assert_eq!(
            worktree_key_for_path("/Users/x/projects/github.com/o/r/src/lib"),
            Some("main".to_string())
        );
    }

    #[test]
    fn worktree_key_non_repo_path_is_none() {
        assert_eq!(worktree_key_for_path("/tmp/whatever"), None);
        assert_eq!(worktree_key_for_path("/Users/x/Documents"), None);
    }

    #[test]
    fn recreate_pane_command_matches_kind_and_id() {
        let id = "550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(
            recreate_pane_command("shell", Some(id)),
            crate::tmux::shell_pane_command(None)
        );
        assert_eq!(
            recreate_pane_command("work", Some(id)),
            crate::tmux::pane_command_for(Some(id))
        );
        assert_eq!(
            recreate_pane_command("work", None),
            crate::tmux::pane_command_for(None)
        );
        // A corrupt/non-UUID stored id must NOT inject — it degrades to the
        // --continue form (same as no id).
        assert_eq!(
            recreate_pane_command("work", Some("not-a-uuid; rm -rf /")),
            crate::tmux::pane_command_for(None)
        );
        // "review" is a non-shell kind → same resume behavior as "work".
        assert_eq!(
            recreate_pane_command("review", Some(id)),
            crate::tmux::pane_command_for(Some(id))
        );
    }

    #[test]
    fn worktree_key_empty_worktree_name_falls_back_to_main() {
        // A trailing `.claude/worktrees/` with no name segment must not yield
        // Some("") — it degrades to the safe "main" fallback.
        assert_eq!(
            worktree_key_for_path("/Users/x/projects/github.com/o/r/.claude/worktrees/"),
            Some("main".to_string())
        );
    }

    #[test]
    fn reconcile_writes_claude_session_id_when_name_matches() {
        use crate::claude_agents::ClaudeAgentRow;
        // Build a fake agent row with name = "my-session"
        let agent_rows = vec![ClaudeAgentRow {
            session_id: Some("abc123".into()),
            name: Some("my-session".into()),
            status: Some("working".into()),
            cwd: None,
        }];
        let hit = crate::claude_agents::find_by_name(&agent_rows, "my-session");
        assert_eq!(hit.unwrap().session_id.as_deref(), Some("abc123"));
        let miss = crate::claude_agents::find_by_name(&agent_rows, "other");
        assert!(miss.is_none());
    }

    // ── worktree_add_script unit tests ────────────────────────────────────────

    #[test]
    fn worktree_add_script_contains_expected_fragments() {
        let script = worktree_add_script("/repo/root", "feat-x", None);
        assert!(script.contains("cd '/repo/root'"), "cd root: {script}");
        assert!(
            script.contains("basebr=''"),
            "empty base when None: {script}"
        );
        assert!(
            script.contains("name='feat-x'"),
            "name assignment: {script}"
        );
        assert!(
            script.contains("git worktree add"),
            "worktree add: {script}"
        );
        assert!(script.contains(" -b "), "branch flag: {script}");
        assert!(script.contains(".worktrees"), ".worktrees dir: {script}");
        assert!(
            script.contains(".claude/worktrees"),
            ".claude/worktrees dir: {script}"
        );
        assert!(
            script.contains("refs/remotes/origin/HEAD"),
            "default branch detection: {script}"
        );
        assert!(
            script.contains("( cd \"$wt\" && pwd -P )"),
            "reports the physical path: {script}"
        );
    }

    #[test]
    fn worktree_add_script_resolves_requested_base_with_default_fallback() {
        let script = worktree_add_script("/repo/root", "feat-x", Some("dev"));
        // Requested base is captured, shell-quoted.
        assert!(script.contains("basebr='dev'"), "base captured: {script}");
        // Resolution: prefer a local branch, then origin/<base>, else fall
        // back to the default branch ($def).
        assert!(
            script.contains("refs/heads/$basebr"),
            "local branch check: {script}"
        );
        assert!(
            script.contains("refs/remotes/origin/$basebr"),
            "origin fallback check: {script}"
        );
        // The worktree is created from the resolved start point, not a literal
        // "$def" — so the default-branch arg must now be the resolved $start.
        assert!(
            script.contains("git worktree add \"$wt\" -b \"$name\" \"$start\""),
            "forks from resolved start point: {script}"
        );
    }

    #[test]
    fn worktree_add_script_blank_base_normalizes_to_default() {
        // Whitespace-only base is treated as "unset" → empty basebr → default.
        let script = worktree_add_script("/repo/root", "feat-x", Some("  "));
        assert!(
            script.contains("basebr=''"),
            "blank base is empty: {script}"
        );
    }

    // ── create_worktree_local integration test ────────────────────────────────

    #[tokio::test]
    async fn create_worktree_local_creates_and_is_idempotent() {
        use std::process::Command;

        // Create a unique temp dir for the bare repo
        let base = std::env::temp_dir().join(format!(
            "cf-wt-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&base).expect("create base");

        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).expect("create repo");
        let repo_str = repo.to_str().unwrap();

        // git init
        let status = Command::new("git")
            .args(["init", repo_str])
            .status()
            .expect("git init");
        assert!(status.success());

        // configure user.email and user.name so commit works
        Command::new("git")
            .args(["-C", repo_str, "config", "user.email", "test@test.com"])
            .status()
            .expect("git config email");
        Command::new("git")
            .args(["-C", repo_str, "config", "user.name", "Test"])
            .status()
            .expect("git config name");

        // write a file and commit
        let file = repo.join("README.md");
        std::fs::write(&file, "hello").expect("write file");
        Command::new("git")
            .args(["-C", repo_str, "add", "."])
            .status()
            .expect("git add");
        Command::new("git")
            .args(["-C", repo_str, "commit", "-m", "init"])
            .status()
            .expect("git commit");

        // call create_worktree_local
        let result = create_worktree_local(repo_str, "feat-x", None).await;
        assert!(result.is_ok(), "first call failed: {:?}", result);
        let wt_path = result.unwrap();
        assert!(
            wt_path.ends_with("/.worktrees/feat-x"),
            "path should end with /.worktrees/feat-x, got: {wt_path}"
        );
        assert!(
            std::path::Path::new(&wt_path).is_dir(),
            "worktree dir should exist: {wt_path}"
        );

        // second call — idempotent
        let result2 = create_worktree_local(repo_str, "feat-x", None).await;
        assert!(
            result2.is_ok(),
            "second (idempotent) call failed: {:?}",
            result2
        );
        assert_eq!(
            result2.unwrap(),
            wt_path,
            "idempotent call must return same path"
        );

        // cleanup
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn remote_script_must_be_quoted_to_survive_login_shell_retokenization() {
        // Regression for "zsh: parse error near `then`" on remote session
        // creation. ssh concatenates the trailing argv with spaces and the
        // remote LOGIN shell re-tokenizes the result, so `bash -lc <script>`
        // with an UNQUOTED `if ...; then ...; fi` splits at `;` and orphans
        // `then`. We reproduce that re-tokenization locally with `sh -c`.
        use std::process::Command;
        let script = "if true; then echo OK; fi";

        // RAW (the bug): the re-login shell mis-parses the orphaned `then`.
        let raw = Command::new("sh")
            .args(["-c", &format!("bash -lc {script}")])
            .output()
            .expect("sh");
        assert!(
            !raw.status.success(),
            "unquoted if/then must fail at the re-tokenizing login shell"
        );

        // QUOTED (the fix): crosses as one word, bash runs the whole script.
        let quoted = Command::new("sh")
            .args(["-c", &format!("bash -lc {}", quote(script))])
            .output()
            .expect("sh");
        assert!(
            quoted.status.success(),
            "quote'd script must run cleanly: {}",
            String::from_utf8_lossy(&quoted.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&quoted.stdout).trim(), "OK");
    }

    // ── Task H: background reconcile tick ──────────────────────────────────

    #[test]
    fn reconcile_interval_defaults_when_absent_or_garbage() {
        assert_eq!(read_reconcile_interval_secs(None), 20);
        assert_eq!(read_reconcile_interval_secs(Some("nonsense".into())), 20);
        assert_eq!(read_reconcile_interval_secs(Some("".into())), 20);
    }

    #[test]
    fn reconcile_interval_honours_explicit_values() {
        assert_eq!(read_reconcile_interval_secs(Some("5".into())), 5);
        // Surrounding whitespace is trimmed before parsing.
        assert_eq!(read_reconcile_interval_secs(Some(" 45 ".into())), 45);
        // 0 is the documented "disabled" sentinel; surfaced verbatim so the
        // tick-interval guard can turn it into None.
        assert_eq!(read_reconcile_interval_secs(Some("0".into())), 0);
    }

    #[test]
    fn list_freshness_window_falls_back_to_default_when_tick_disabled() {
        // Pull-only mode (interval 0) must still not re-probe on every focus.
        let store = Mutex::new(Store::open_in_memory().expect("store"));
        assert_eq!(
            list_freshness_window(&store),
            std::time::Duration::from_secs(DEFAULT_RECONCILE_INTERVAL_SECS as u64)
        );
        store
            .lock()
            .unwrap()
            .set_setting("reconcile.interval_secs", "0")
            .unwrap();
        assert_eq!(
            list_freshness_window(&store),
            std::time::Duration::from_secs(DEFAULT_RECONCILE_INTERVAL_SECS as u64)
        );
        store
            .lock()
            .unwrap()
            .set_setting("reconcile.interval_secs", "7")
            .unwrap();
        assert_eq!(
            list_freshness_window(&store),
            std::time::Duration::from_secs(7)
        );
    }

    #[test]
    fn reconcile_tick_interval_disabled_when_zero_or_negative() {
        // 0 = disabled (the documented "off" sentinel) and any non-positive
        // value must keep the tick from running rather than busy-loop.
        assert_eq!(reconcile_tick_interval(0), None);
        assert_eq!(reconcile_tick_interval(-5), None);
    }

    #[test]
    fn reconcile_tick_interval_enabled_for_positive_secs() {
        assert_eq!(
            reconcile_tick_interval(20),
            Some(std::time::Duration::from_secs(20))
        );
        assert_eq!(
            reconcile_tick_interval(1),
            Some(std::time::Duration::from_secs(1))
        );
    }

    #[tokio::test]
    async fn reconcile_now_is_callable_headless() {
        // The whole point of Task H: reconcile must be drivable from a
        // background task with just an in-memory Store + a bare SshClient — no
        // Tauri AppHandle. We assert it is *callable* this way (compiles, spawns
        // on the managed deps) without depending on the test box's real local
        // probe, which shells out to `tmux` / `claude agents --json` and can
        // block on a developer machine. A bounded timeout keeps the suite fast:
        //   - Ok(Ok(_))  → reconcile ran and returned (no real probe stalled)
        //   - Ok(Err(_)) → reconcile ran and surfaced an IpcError (still proves
        //                  the headless path executes end to end)
        //   - Err(_)     → the real local probe is blocking; the entry point is
        //                  still demonstrably callable (it was driven to await).
        let store = Mutex::new(Store::open_in_memory().expect("store"));
        let ssh = Arc::new(SshClient::new());
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            reconcile_now(&store, &ssh),
        )
        .await;
    }

    // ── Wave 2 Track D: addressing (MCP-6) ──

    fn seeded_store() -> Mutex<Store> {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        s.upsert_host("mefistos").unwrap();
        s.upsert_session("dev-a", "local", None, None, 1, 1, "running", None)
            .unwrap();
        s.upsert_session("dev-a", "mefistos", None, None, 1, 1, "running", None)
            .unwrap();
        s.upsert_session("dev-only", "local", None, None, 1, 1, "running", None)
            .unwrap();
        Mutex::new(s)
    }

    #[test]
    fn resolve_session_target_prefers_id_then_requires_full_pair() {
        let store = seeded_store();
        let s = store.lock().unwrap();
        let only = s.get_session("dev-only", "local").unwrap().unwrap();
        // id wins even when a (wrong) pair is also supplied
        let r = resolve_session_target(&s, Some(only.id), Some("mefistos"), Some("dev-a")).unwrap();
        assert_eq!(r.id, only.id);
        let r = resolve_session_target(&s, None, Some("mefistos"), Some("dev-a")).unwrap();
        assert_eq!(r.host_alias, "mefistos");
        assert_eq!(
            resolve_session_target(&s, None, Some("local"), None)
                .unwrap_err()
                .code,
            "E_INVALID"
        );
        assert_eq!(
            resolve_session_target(&s, None, None, Some("dev-a"))
                .unwrap_err()
                .code,
            "E_INVALID"
        );
        assert_eq!(
            resolve_session_target(&s, Some(9999), None, None)
                .unwrap_err()
                .code,
            "E_NOTFOUND"
        );
        assert_eq!(
            resolve_session_target(&s, None, Some("local"), Some("nope"))
                .unwrap_err()
                .code,
            "E_NOTFOUND"
        );
        assert_eq!(
            resolve_session_target(&s, None, Some("-oProxyCommand=x"), Some("dev-a"))
                .unwrap_err()
                .code,
            "E_INVALID"
        );
    }

    #[test]
    fn find_session_by_tmux_name_prefers_running_rows_over_ghosts() {
        let store = seeded_store();
        let s = store.lock().unwrap();
        // Ghost the mefistos copy: whoami must now resolve to the live one.
        s.conn_ref()
            .execute(
                "UPDATE sessions SET status='ghost', lost_at=5 WHERE tmux_name='dev-a' AND host_alias='mefistos'",
                [],
            )
            .unwrap();
        let row = find_session_by_tmux_name(&s, "dev-a").unwrap();
        assert_eq!(row.host_alias, "local");
        // Only ghosts left ⇒ they are still findable (one match).
        s.conn_ref()
            .execute(
                "UPDATE sessions SET status='ghost', lost_at=5 WHERE tmux_name='dev-a'",
                [],
            )
            .unwrap();
        assert_eq!(
            find_session_by_tmux_name(&s, "dev-a").unwrap_err().code,
            "E_AMBIGUOUS"
        );
    }

    #[test]
    fn prompt_derived_name_replaces_only_the_branch_default() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            let pid = s.upsert_project("o", "r", "/p/o/r").unwrap();
            let wid = s
                .upsert_worktree(
                    pid,
                    "fix-login",
                    "/p/o/r/.worktrees/fix-login",
                    Some("dev-o-r--fix-login"),
                )
                .unwrap();
            s.upsert_session(
                "dev-o-r--fix-login",
                "local",
                Some(pid),
                Some(wid),
                1,
                1,
                "running",
                None,
            )
            .unwrap();
            let default = s
                .default_friendly_name(
                    s.get_session("dev-o-r--fix-login", "local")
                        .unwrap()
                        .unwrap()
                        .id,
                )
                .unwrap()
                .expect("branch default");
            s.set_friendly_name("local", "dev-o-r--fix-login", Some(&default))
                .unwrap();
        }
        record_prompt_outcome(
            &store,
            "local",
            "dev-o-r--fix-login",
            "Rewrite the auth flow!",
        );
        {
            let s = store.lock().unwrap();
            let row = s
                .get_session("dev-o-r--fix-login", "local")
                .unwrap()
                .unwrap();
            assert_eq!(row.friendly_name.as_deref(), Some("rewrite the auth flow"));
            assert_eq!(row.last_prompt.as_deref(), Some("Rewrite the auth flow!"));
            // A chosen label survives the next prompt.
            s.set_friendly_name("local", "dev-o-r--fix-login", Some("My label"))
                .unwrap();
        }
        record_prompt_outcome(&store, "local", "dev-o-r--fix-login", "Another prompt here");
        let s = store.lock().unwrap();
        let row = s
            .get_session("dev-o-r--fix-login", "local")
            .unwrap()
            .unwrap();
        assert_eq!(row.friendly_name.as_deref(), Some("My label"));
        assert_eq!(row.last_prompt.as_deref(), Some("Another prompt here"));
    }

    #[test]
    fn find_session_by_tmux_name_returns_the_single_match_or_lists_candidates() {
        let store = seeded_store();
        let s = store.lock().unwrap();
        assert_eq!(
            find_session_by_tmux_name(&s, "dev-only")
                .unwrap()
                .host_alias,
            "local"
        );
        assert_eq!(
            find_session_by_tmux_name(&s, "ghost-name")
                .unwrap_err()
                .code,
            "E_NOTFOUND"
        );
        let err = find_session_by_tmux_name(&s, "dev-a").unwrap_err();
        assert_eq!(err.code, "E_AMBIGUOUS");
        let cands = err.details.unwrap()["candidates"].as_array().unwrap().len();
        assert_eq!(cands, 2);
    }

    // ── Wave 2 Track D: naming + PR probe ──

    #[test]
    fn friendly_name_from_prompt_takes_five_lowercase_words_without_punctuation() {
        assert_eq!(
            friendly_name_from_prompt("Fix the login bug, then add tests for it!").as_deref(),
            Some("fix the login bug then")
        );
        assert_eq!(
            friendly_name_from_prompt("  Refactor   SSH   layer  ").as_deref(),
            Some("refactor ssh layer")
        );
        assert_eq!(friendly_name_from_prompt("!!! ... ---"), None);
        assert_eq!(friendly_name_from_prompt(""), None);
        // Unicode letters survive, symbols do not.
        assert_eq!(
            friendly_name_from_prompt("Oprav chybu v prihlásení (rýchlo)").as_deref(),
            Some("oprav chybu v prihlásení rýchlo")
        );
    }

    /// A host shell that answers the PR probe with canned stdout and counts
    /// invocations, so the throttle is observable.
    struct CannedShell {
        stdout: String,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl HostShell for CannedShell {
        async fn run_script(&self, _host: &str, script: &str) -> Result<String, IpcError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert!(script.contains("gh pr view"), "probe script runs gh");
            Ok(self.stdout.clone())
        }
    }

    fn repo_session(name: &str, path: &str) -> crate::tmux::TmuxSession {
        crate::tmux::TmuxSession {
            name: name.to_string(),
            created: 1,
            last_activity: 1,
            attached: false,
            path: PathBuf::from(path),
        }
    }

    #[tokio::test]
    async fn reconcile_populates_pr_url_and_ci_status_and_throttles_the_probe() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let stdout = "__FLEET_PR__\tdev-a\t0\t{\"url\":\"https://github.com/o/r/pull/9\",\
                      \"statusCheckRollup\":[{\"status\":\"COMPLETED\",\"conclusion\":\"SUCCESS\"}]}\n\
                      __FLEET_PR__\tdev-b\t1\tno pull requests found for branch \"main\"\n";
        let shell = Arc::new(CannedShell {
            stdout: stdout.to_string(),
            calls: Arc::clone(&calls),
        });
        let live = vec![
            repo_session("dev-a", "/home/u/projects/github.com/o/r/.worktrees/a"),
            repo_session("dev-b", "/home/u/projects/github.com/o/r"),
            // Not a github-layout path: never probed.
            repo_session("scratch", "/tmp"),
        ];
        let probes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let probes_for_exec = Arc::clone(&probes);
        let deps = ReconcileDeps::fake_with_shell(
            move |_alias| {
                Box::new(ScriptedTmux {
                    sessions: live.clone(),
                    delay: std::time::Duration::from_millis(0),
                    hang: false,
                    probes: Arc::clone(&probes_for_exec),
                })
            },
            std::time::Duration::from_secs(5),
            shell,
        );
        {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
        }
        reconcile_sessions_with(&store, &deps).await.unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        {
            let s = store.lock().unwrap();
            let a = s.get_session("dev-a", "local").unwrap().unwrap();
            assert_eq!(a.pr_url.as_deref(), Some("https://github.com/o/r/pull/9"));
            assert_eq!(a.ci_status.as_deref(), Some("passing"));
            let b = s.get_session("dev-b", "local").unwrap().unwrap();
            assert_eq!(b.pr_url, None);
            let c = s.get_session("scratch", "local").unwrap().unwrap();
            assert_eq!(c.pr_url, None);
        }
        // Second pass within the TTL: the cache says nothing is due, so the
        // shell is not consulted and the stored fields survive.
        reconcile_sessions_with(&store, &deps).await.unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        let s = store.lock().unwrap();
        let a = s.get_session("dev-a", "local").unwrap().unwrap();
        assert_eq!(a.pr_url.as_deref(), Some("https://github.com/o/r/pull/9"));
    }

    #[tokio::test]
    async fn reconcile_survives_a_failing_pr_probe_shell() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        let probes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let live = vec![repo_session("dev-a", "/home/u/projects/github.com/o/r")];
        let deps = ReconcileDeps::fake(
            move |_alias| {
                Box::new(ScriptedTmux {
                    sessions: live.clone(),
                    delay: std::time::Duration::from_millis(0),
                    hang: false,
                    probes: Arc::clone(&probes),
                })
            },
            std::time::Duration::from_secs(5),
        );
        {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
        }
        reconcile_sessions_with(&store, &deps).await.unwrap();
        let s = store.lock().unwrap();
        let a = s.get_session("dev-a", "local").unwrap().unwrap();
        assert_eq!(a.status, "running");
        assert_eq!(a.pr_url, None);
    }
}

#[cfg(test)]
mod ghost_tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn recreate_session_errors_when_session_missing() {
        let store = std::sync::Mutex::new(Store::open_in_memory().unwrap());
        let args = RecreateSessionArgs {
            session_id: 999,
            force: false,
        };
        let ssh = std::sync::Arc::new(crate::ssh::SshClient::new());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(recreate_session(args, &store, &ssh))
            .unwrap_err();
        assert_eq!(err.code, "E_NOTFOUND");
    }

    #[test]
    fn recreate_session_errors_when_host_offline() {
        let store = std::sync::Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
                .unwrap();
            s.conn_ref()
                .execute("UPDATE hosts SET reachable=0 WHERE alias='local'", [])
                .unwrap();
        }
        let id = store
            .lock()
            .unwrap()
            .get_session("dev", "local")
            .unwrap()
            .unwrap()
            .id;
        let args = RecreateSessionArgs {
            session_id: id,
            force: false,
        };
        let ssh = std::sync::Arc::new(crate::ssh::SshClient::new());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(recreate_session(args, &store, &ssh))
            .unwrap_err();
        assert_eq!(err.code, "E_HOST_OFFLINE");
    }

    #[test]
    fn dismiss_ghost_rejects_non_ghost() {
        let store = std::sync::Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
                .unwrap();
        }
        let id = store
            .lock()
            .unwrap()
            .get_session("dev", "local")
            .unwrap()
            .unwrap()
            .id;
        let args = DismissGhostSessionArgs { session_id: id };
        let result = dismiss_ghost_session(args, &store);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, "E_INVALID_STATE");
    }

    #[test]
    fn dismiss_ghost_deletes_ghost_session() {
        let store = std::sync::Mutex::new(Store::open_in_memory().unwrap());
        {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            s.upsert_session("dev", "local", None, None, 1, 1, "running", None)
                .unwrap();
        }
        let id = store
            .lock()
            .unwrap()
            .get_session("dev", "local")
            .unwrap()
            .unwrap()
            .id;
        // Manually ghost it
        store
            .lock()
            .unwrap()
            .conn_ref()
            .execute(
                "UPDATE sessions SET status='ghost', lost_at=999 WHERE id=?1",
                rusqlite::params![id],
            )
            .unwrap();
        let args = DismissGhostSessionArgs { session_id: id };
        let result = dismiss_ghost_session(args, &store);
        assert!(result.is_ok());
        assert!(store
            .lock()
            .unwrap()
            .get_session_by_id(id)
            .unwrap()
            .is_none());
    }

    fn set_friendly_args(host: &str, tmux: &str, label: &str) -> SetFriendlyNameArgs {
        SetFriendlyNameArgs {
            host_alias: host.into(),
            tmux_name: tmux.into(),
            friendly_name: label.into(),
        }
    }

    #[test]
    fn set_friendly_name_round_trips_through_store() {
        let store = Mutex::new(Store::open_in_memory().expect("store"));
        {
            let s = store.lock().unwrap();
            s.upsert_host("h").unwrap();
            s.upsert_session("dev-x", "h", None, None, 1, 1, "running", None)
                .unwrap();
        }
        let row =
            set_session_friendly_name(set_friendly_args("h", "dev-x", "fix login bug"), &store)
                .expect("update");
        assert_eq!(row.friendly_name.as_deref(), Some("fix login bug"));
        // Persisted, not just returned.
        let stored = store
            .lock()
            .unwrap()
            .get_session("dev-x", "h")
            .unwrap()
            .unwrap();
        assert_eq!(stored.friendly_name.as_deref(), Some("fix login bug"));
    }

    #[test]
    fn set_friendly_name_trims_and_whitespace_clears() {
        let store = Mutex::new(Store::open_in_memory().expect("store"));
        {
            let s = store.lock().unwrap();
            s.upsert_host("h").unwrap();
            s.upsert_session("dev-x", "h", None, None, 1, 1, "running", None)
                .unwrap();
            // Seed an existing label so we can prove the clear path actually nulls it.
            s.set_friendly_name("h", "dev-x", Some("old label"))
                .unwrap();
        }
        // Padding around real content is trimmed.
        let row =
            set_session_friendly_name(set_friendly_args("h", "dev-x", "  trimmed label  "), &store)
                .expect("update");
        assert_eq!(row.friendly_name.as_deref(), Some("trimmed label"));
        // Whitespace-only input clears.
        let row = set_session_friendly_name(set_friendly_args("h", "dev-x", "   "), &store)
            .expect("clear");
        assert!(row.friendly_name.is_none());
        // Empty input also clears.
        let row = set_session_friendly_name(set_friendly_args("h", "dev-x", ""), &store)
            .expect("clear empty");
        assert!(row.friendly_name.is_none());
    }

    #[test]
    fn set_friendly_name_works_on_bg_session_rows() {
        // Regression: synthetic bg rows have `tmux_name = "bg:<uuid>"`, which
        // contains ':'. The create-mode `tmux_name` validator rejects ':',
        // so the lookup-mode `tmux_name_lookup` validator must be used here
        // — otherwise every bg agent fails with E_INVALID at the MCP layer.
        let store = Mutex::new(Store::open_in_memory().expect("store"));
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        let bg_name = format!("bg:{uuid}");
        {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            s.upsert_bg_session("local", &bg_name, None, uuid, Some("working"), 1)
                .unwrap();
        }
        let row = set_session_friendly_name(
            set_friendly_args("local", &bg_name, "review hardening spec"),
            &store,
        )
        .expect("bg row must be addressable");
        assert_eq!(row.tmux_name, bg_name);
        assert_eq!(row.kind, "bg");
        assert_eq!(row.friendly_name.as_deref(), Some("review hardening spec"));
    }

    #[test]
    fn set_friendly_name_returns_not_found_when_row_absent() {
        let store = Mutex::new(Store::open_in_memory().expect("store"));
        {
            let s = store.lock().unwrap();
            s.upsert_host("h").unwrap();
            // No session inserted.
        }
        let err = set_session_friendly_name(set_friendly_args("h", "ghost", "x"), &store)
            .expect_err("absent row");
        assert_eq!(err.code, "E_NOTFOUND");
    }

    #[test]
    fn set_friendly_name_rejects_invalid_input() {
        let store = Mutex::new(Store::open_in_memory().expect("store"));
        {
            let s = store.lock().unwrap();
            s.upsert_host("h").unwrap();
            s.upsert_session("dev-x", "h", None, None, 1, 1, "running", None)
                .unwrap();
        }
        // Control char rejected by validate::friendly_name.
        let err = set_session_friendly_name(set_friendly_args("h", "dev-x", "bad\nlabel"), &store)
            .expect_err("control char");
        assert_eq!(err.code, "E_INVALID");
        // Bad host alias rejected before any UPDATE.
        let err = set_session_friendly_name(set_friendly_args("-evil", "dev-x", "x"), &store)
            .expect_err("bad alias");
        assert_eq!(err.code, "E_INVALID");
    }
}

#[cfg(test)]
mod fill_session_name_tests {
    use super::*;

    fn args(
        worktree_id: Option<i64>,
        new_worktree: Option<&str>,
        kind: Option<&str>,
    ) -> NewSessionArgs {
        NewSessionArgs {
            host_alias: "local".into(),
            project_id: 1,
            worktree_id,
            name: String::new(),
            call_id: None,
            new_worktree: new_worktree.map(Into::into),
            base_branch: None,
            kind: kind.map(Into::into),
            start_command: None,
            friendly_name: None,
        }
    }

    fn seeded() -> (Store, i64, i64) {
        let s = Store::open_in_memory().expect("store");
        s.upsert_host("local").unwrap();
        s.upsert_host("mefistos").unwrap();
        let pid = s.upsert_project("o", "r", "/tmp/o/r").unwrap();
        assert_eq!(pid, 1);
        let main_id = s
            .upsert_worktree(pid, "main", "/tmp/o/r", Some("main"))
            .unwrap();
        let feat_id = s
            .upsert_worktree(pid, "feat-x", "/tmp/o/r/.worktrees/feat-x", Some("feat-x"))
            .unwrap();
        (s, main_id, feat_id)
    }

    #[test]
    fn deterministic_name_when_free() {
        let (s, main_id, feat_id) = seeded();
        assert_eq!(
            fill_session_name(&s, &args(Some(main_id), None, None)).unwrap(),
            "dev-o-r"
        );
        assert_eq!(
            fill_session_name(&s, &args(Some(feat_id), None, None)).unwrap(),
            "dev-o-r--feat-x"
        );
        assert_eq!(
            fill_session_name(&s, &args(None, Some("blue-sirius"), None)).unwrap(),
            "dev-o-r--blue-sirius"
        );
        assert_eq!(
            fill_session_name(&s, &args(Some(main_id), None, Some("shell"))).unwrap(),
            "dev-o-r-term"
        );
    }

    #[test]
    fn appends_generated_pair_when_deterministic_name_is_taken() {
        let (s, main_id, _) = seeded();
        s.upsert_session(
            "dev-o-r",
            "local",
            Some(1),
            Some(main_id),
            1,
            1,
            "running",
            None,
        )
        .unwrap();
        let name = fill_session_name(&s, &args(Some(main_id), None, None)).unwrap();
        let suffix = name.strip_prefix("dev-o-r--").expect("pair appended");
        assert!(
            crate::service::names::adjectives()
                .iter()
                .any(|a| suffix.starts_with(&format!("{a}-"))),
            "{name}"
        );
        // The same name on another host does not count as taken.
        s.upsert_session(
            "dev-o-r--feat-x",
            "mefistos",
            Some(1),
            None,
            1,
            1,
            "running",
            None,
        )
        .unwrap();
        assert_eq!(
            fill_session_name(&s, &args(None, Some("feat-x"), None)).unwrap(),
            "dev-o-r--feat-x"
        );
    }

    #[test]
    fn taken_slugs_include_worktrees_tmux_suffixes_and_friendly_names() {
        let (s, main_id, _) = seeded();
        s.upsert_session(
            "dev-o-r--amber-vega",
            "local",
            Some(1),
            Some(main_id),
            1,
            1,
            "running",
            None,
        )
        .unwrap();
        s.set_friendly_name("local", "dev-o-r--amber-vega", Some("Blue Sirius"))
            .unwrap();
        let taken = project_taken_slugs(&s, 1, "o", "r").unwrap();
        assert!(taken.contains("feat-x"), "worktree name");
        assert!(taken.contains("main"), "worktree name");
        assert!(taken.contains("amber-vega"), "tmux suffix");
        assert!(taken.contains("blue-sirius"), "slugified friendly name");
    }

    #[test]
    fn dots_and_colons_are_mapped_so_the_name_validates() {
        let (s, _, _) = seeded();
        let v12 = s
            .upsert_worktree(1, "v1.2", "/tmp/o/r/.worktrees/v1.2", Some("v1.2"))
            .unwrap();
        let name = fill_session_name(&s, &args(Some(v12), None, None)).unwrap();
        assert_eq!(name, "dev-o-r--v1-2");
        crate::validate::tmux_name(&name).expect("filled name validates");
    }

    #[tokio::test]
    async fn new_session_with_empty_name_is_filled_and_validated_end_to_end() {
        // Drive the real service entry point. The project's base_path does not
        // exist, so the call fails at the worktree step (E_GIT_SETUP, from
        // bash) — AFTER the empty name was minted and passed
        // `validate::tmux_name`. Before this change the same call failed with
        // E_INVALID ("session name must not be empty"). No tmux, no network.
        let s = Store::open_in_memory().expect("store");
        s.upsert_host("local").unwrap();
        s.upsert_project("o", "r", "/nonexistent/claude-fleet-test/o/r")
            .unwrap();
        let store = Mutex::new(s);
        let ssh = Arc::new(SshClient::new());
        let reg = CancellationRegistry::new();

        let err = new_session(args(None, Some("blue-sirius"), None), &store, &ssh, &reg)
            .await
            .expect_err("repo is not on disk");
        assert_eq!(err.code, "E_GIT_SETUP", "{err:?}");

        // Whitespace-only is treated as empty too.
        let mut ws = args(None, Some("red-comet"), None);
        ws.name = "   ".into();
        let err = new_session(ws, &store, &ssh, &reg).await.unwrap_err();
        assert_eq!(err.code, "E_GIT_SETUP", "{err:?}");

        // Control: an explicit invalid name is still rejected up front.
        let mut bad = args(None, Some("red-comet"), None);
        bad.name = "bad.name".into();
        let err = new_session(bad, &store, &ssh, &reg).await.unwrap_err();
        assert_eq!(err.code, "E_INVALID", "{err:?}");
    }

    #[test]
    fn generated_pair_avoids_slugs_already_used_on_the_project() {
        let (s, main_id, _) = seeded();
        s.upsert_session(
            "dev-o-r",
            "local",
            Some(1),
            Some(main_id),
            1,
            1,
            "running",
            None,
        )
        .unwrap();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..50 {
            let name = fill_session_name(&s, &args(Some(main_id), None, None)).unwrap();
            let suffix = name.strip_prefix("dev-o-r--").unwrap().to_string();
            // Worktree names on the project are off-limits.
            assert_ne!(suffix, "feat-x");
            seen.insert(suffix);
        }
        assert!(seen.len() > 1, "names are random, not fixed");
    }
}
