//! The reconcile core: host probes (tmux, `claude agents`, pane intel, PR
//! probe), the fleet-wide gate and freshness window, the per-host writer, and
//! the `list_sessions` / `refresh_sessions` / `reconcile_now` entry points.

use super::*;

/// Number of pane lines captured per work session for the reconcile intel
/// probe. Eight lines covers the REPL footer (status bar / context %) plus the
/// last tool line or prompt without dragging in scrollback.
pub(super) const PANE_TAIL_LINES: u32 = 8;

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
pub(super) const HOST_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

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
pub(super) type PaneIntelMap =
    std::collections::HashMap<String, crate::service::pane_intel::PaneIntel>;

/// Map of `tmux_name` → PR probe result, gathered off-lock (see `outcome.rs`).
pub(super) type PrInfoMap = std::collections::HashMap<String, crate::service::outcome::PrInfo>;

/// Runs one shell script on a host and returns its stdout. Abstracted so the
/// reconcile PR probe (and the playbook / GC helpers) are testable without a
/// real host. The production impl is `RealHostShell`.
#[async_trait::async_trait]
pub(crate) trait HostShell: Send + Sync {
    async fn run_script(&self, host: &str, script: &str) -> Result<String, IpcError>;
}

/// `bash -lc <script>` locally or over ssh, bounded by `timeout`.
pub(crate) struct RealHostShell {
    pub(super) ssh: Arc<SshClient>,
    pub(super) timeout: std::time::Duration,
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
pub(super) const PR_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Most sessions probed for a PR per host per pass. Bounds the script's
/// worst case (`PR_PROBE_BATCH` sequential `gh` calls); the rest are picked
/// up on later passes as the per-session cache expires.
pub(super) const PR_PROBE_BATCH: usize = 12;

/// One host's probe result, carried from the off-lock probe task to the
/// under-lock writer.
pub(super) struct HostProbe {
    pub(super) host: HostRow,
    pub(super) result: Result<Vec<crate::tmux::TmuxSession>, IpcError>,
    pub(super) agent_rows: Vec<crate::claude_agents::ClaudeAgentRow>,
    pub(super) intel: PaneIntelMap,
    /// `tmux_name → gh pr view` result for the sessions probed THIS pass
    /// (PROD-5). A name absent from the map was not probed (cache still
    /// fresh, host has no `gh`, or the probe failed) and keeps its stored
    /// `pr_url` / `ci_status`.
    pub(super) pr_info: PrInfoMap,
    /// Unix-epoch second the probe STARTED. Forwarded as
    /// `HostReconcile::probe_started_at` so the writer never ghosts a row that
    /// a newer probe (e.g. `new_session`'s own reconcile) stamped after this
    /// probe listed tmux (BE-3).
    pub(super) started_at: i64,
}

/// Executor factory + probe budget for the reconcile core. Production uses
/// `exec_for` (local tmux or ssh-wrapped tmux) under `HOST_PROBE_TIMEOUT`;
/// tests inject fakes so the fan-out, the gate and the ghost guard are
/// exercisable without a real host (BE-7).
pub(crate) struct ReconcileDeps {
    pub(super) exec: ExecFactory,
    pub(super) probe_timeout: std::time::Duration,
    /// Shell used for the per-host `gh pr view` probe (PROD-5).
    pub(super) shell: Arc<dyn HostShell>,
    /// Per-session probe throttle; production shares one process-wide cache,
    /// tests get a fresh one per deps.
    pub(super) pr_cache: Arc<crate::service::outcome::PrProbeCache>,
}

/// `host alias → tmux executor` factory used by `ReconcileDeps`.
pub(super) type ExecFactory = Box<dyn Fn(&str) -> Box<dyn TmuxExec> + Send + Sync>;

impl ReconcileDeps {
    pub(super) fn real(ssh: &Arc<SshClient>) -> Arc<Self> {
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
    pub(super) fn fake_with_shell(
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
    pub(super) running: tokio::sync::Mutex<()>,
    pub(super) last_completed: std::sync::Mutex<Option<std::time::Instant>>,
    /// Number of full passes that ran to completion (tests use it to prove a
    /// call caused zero / exactly one pass).
    pub(super) passes: std::sync::atomic::AtomicU64,
}

/// RAII token for a running pass. Drop without `complete()` (a pass that
/// errored) leaves `last_completed` untouched so the next caller retries.
pub struct ReconcilePass<'a> {
    pub(super) _guard: tokio::sync::MutexGuard<'a, ()>,
    pub(super) gate: &'a ReconcileGate,
}

impl ReconcilePass<'_> {
    pub(super) fn complete(self) {
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
pub(super) async fn capture_pane_intel(
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
pub(super) fn reconcile_write_one_host(
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
                        tracing::warn!(
                            host = %host.alias,
                            session = %tmux_name,
                            error = %e,
                            "[reconcile] session_event insert failed"
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
                tracing::warn!(
                    host = %host.alias,
                    error = %e,
                    "[reconcile] mark_sessions_reconciled failed"
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
pub(super) fn unmatched_bg_agents<'a>(
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
pub(super) fn reconcile_bg_agents(
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
            tracing::warn!(
                host = %host_alias,
                claude_session_id = %session_id,
                error = %e,
                "[reconcile] bg upsert failed"
            );
        }
    }
    if let Err(e) = s.ghost_and_clean_bg_sessions(host_alias, &keep, now_unix()) {
        tracing::warn!(host = %host_alias, error = %e, "[reconcile] bg cleanup failed");
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
pub(super) async fn probe_one_host(
    host: HostRow,
    paths: HostPaths,
    deps: &ReconcileDeps,
) -> HostProbe {
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
pub(super) async fn probe_pr_info(
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
            // Best-effort and retried every pass: debug, not warn.
            tracing::debug!(host = %host, error = %e, "[reconcile] pr probe failed");
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
pub(super) async fn probe_with_timeout(
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
            tracing::warn!(
                host = %host.alias,
                timeout = ?timeout,
                "[reconcile] host probe exceeded its wall clock; marking unreachable (last-known sessions kept)"
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
            Err(_elapsed) => tracing::debug!(
                host = %probe.host.alias,
                timeout = ?PR_PROBE_TIMEOUT,
                "[reconcile] pr probe timed out; outcome fields kept"
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
            Err(e) => tracing::error!(error = %e, "[reconcile] probe task panicked"),
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
            tracing::error!(
                host = %probe.host.alias,
                error = %e,
                "[reconcile] write failed (rolled back; retried next pass)"
            );
        }
    }
    Ok(())
}

/// Claim the gate and run one full pass. Returns `Ok(false)` without probing
/// when another pass is already running (the caller then serves stored rows).
pub(super) async fn run_full_reconcile(
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
pub(super) fn list_freshness_window(store: &Mutex<Store>) -> std::time::Duration {
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
pub(super) async fn list_sessions_with(
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

pub(super) async fn reconcile_one_host_with(
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

/// Accept a `claude agents --json` status only when it is in the documented
/// `ClaudeStatus` vocabulary. An unknown value (a newer CLI, a typo upstream)
/// is logged once per reconcile and dropped so the pane-derived fallback wins
/// instead of the DB silently diverging from what the MCP docs promise.
pub(super) fn known_agent_status(tmux_name: &str, status: Option<&str>) -> Option<String> {
    let raw = status?;
    match raw.parse::<crate::service::pane_intel::ClaudeStatus>() {
        Ok(st) => Some(st.as_str().to_string()),
        Err(_) => {
            // Repeats every pass while the CLI keeps reporting it: debug.
            tracing::debug!(
                session = %tmux_name,
                status = ?raw,
                vocabulary = %crate::service::pane_intel::ClaudeStatus::vocabulary_doc(),
                "[reconcile] dropping an unknown claude agents status"
            );
            None
        }
    }
}
