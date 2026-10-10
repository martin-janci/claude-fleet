//! The reconcile core: host probes (tmux, `claude agents`, pane intel, PR
//! probe), the fleet-wide gate and freshness window, the per-host writer, and
//! the `list_sessions` / `refresh_sessions` / `reconcile_now` entry points.

use super::*;
use crate::ipc_error::codes;
use crate::ipc_error::lock;
use crate::store::StartSource;
use std::collections::HashMap;

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
/// the existing "unreachable, keep last-known sessions" branch.
///
/// Safety net above the per-call wall clock: the batched probe is one call
/// (30 s wall clock, which already resets a wedged master), agents a second;
/// 2 × 30 + 5.
pub(crate) const HOST_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(65);

/// Production cadence for `claude agents --json` per host (a node cold
/// start): once per minute, not every reconcile pass. See `agents_due`.
pub(crate) const AGENTS_CADENCE: std::time::Duration = std::time::Duration::from_secs(60);

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

/// Fallback when `sessions.lost_ttl_secs` is unset or unparseable — 14 days,
/// matching the registry default in `service::settings::SESSIONS_LOST_TTL_SECS`.
pub(super) const DEFAULT_LOST_TTL_SECS: i64 = 1_209_600;

/// Resolve the mass-loss TTL cutoff from the raw `sessions.lost_ttl_secs`
/// setting value, the same way `read_reconcile_interval_secs` resolves its
/// key (`settings::resolve` → parse → fall back to the registry default). A
/// value `<= 0` DISABLES the exemption entirely (`None`, today's exemption-
/// free behaviour); otherwise the cutoff is `now - ttl`, passed straight
/// through to `HostReconcile::lost_ttl_cutoff` / `ghost_and_clean_bg_sessions`.
pub(super) fn read_lost_ttl_cutoff(raw: Option<String>, now: i64) -> Option<i64> {
    let ttl = crate::service::settings::resolve(
        crate::service::settings::SESSIONS_LOST_TTL_SECS,
        raw.as_deref(),
    )
    .parse::<i64>()
    .unwrap_or(DEFAULT_LOST_TTL_SECS);
    if ttl <= 0 {
        None
    } else {
        Some(now - ttl)
    }
}

/// Resolve the external-ghost grace from the raw `gc.external_lost_ttl_secs`
/// value, like [`read_lost_ttl_cutoff`]: `<= 0` disables the grace (`None`,
/// reaped on the next pass); otherwise the cutoff is `now - grace`.
pub(super) fn read_external_grace_cutoff(raw: Option<String>, now: i64) -> Option<i64> {
    let grace = crate::service::settings::resolve(
        crate::service::settings::GC_EXTERNAL_LOST_TTL_SECS,
        raw.as_deref(),
    )
    .parse::<i64>()
    .unwrap_or(3600);
    if grace <= 0 {
        None
    } else {
        Some(now - grace)
    }
}

/// Why every session on a reachable host should be treated as lost this
/// pass, or `None` for a normal pass. Each comparison needs BOTH sides
/// known, so a first probe after upgrade or a failed identity read never
/// mass-marks a host.
pub(super) fn mass_loss_verdict(
    stored: &StoredIdentity,
    observed: Option<&crate::tmux::HostIdentity>,
) -> Option<&'static str> {
    let obs = observed?;
    if let (Some(s), Some(o)) = (stored.boot_id.as_deref(), obs.boot_id.as_deref()) {
        if s != o {
            return Some("host_reboot");
        }
    }
    match (stored.tmux_server_pid, obs.tmux_server_pid) {
        (Some(_), None) => Some("tmux_server_gone"),
        (Some(s), Some(o)) if s != o => Some("tmux_server_gone"),
        // `(None, None)` is deliberately NOT a verdict: a host whose server
        // was already absent last pass has already been marked, and a host
        // first seen with no server has no stored evidence of loss.
        _ => None,
    }
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
                codes::E_SHELL,
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
        Err(IpcError::new(codes::E_SHELL, "no shell in this test"))
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
    crate::ssh::run_shell(ssh.as_ref(), host, script, timeout).await
}

/// Wall clock for one host's PR probe script (one `gh pr view` per due
/// session, sequential). Runs AFTER the reachability probe, outside
/// `HOST_PROBE_TIMEOUT`, so a slow GitHub API can never flip a host to
/// unreachable.
pub(crate) const PR_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Most sessions probed for a PR per host per pass. Bounds the script's
/// worst case (`PR_PROBE_BATCH` sequential `gh` calls); the rest are picked
/// up on later passes as the per-session cache expires.
pub(super) const PR_PROBE_BATCH: usize = 12;

/// How old a host's `claude_version_at` may be before the next pass asks
/// the host for its versions again (`---FLEET:versions`, one `claude
/// --version` node start per host per window). The desktop's `claude_old`
/// badge trusts a stamp younger than `health.version_max_age_secs` (24 h),
/// so a 6 h refresh keeps it honest with room to spare.
pub const VERSIONS_REFRESH_SECS: i64 = 6 * 3600;

/// Whether this pass should ask the host for its versions.
pub(super) fn versions_due(claude_version_at: Option<i64>, now: i64) -> bool {
    claude_version_at.is_none_or(|at| now - at >= VERSIONS_REFRESH_SECS)
}

/// How often a host is asked how much disk fleet's worktrees hold (Orbit
/// Fleet 4.6). A `du` over checkouts with `node_modules` can take seconds,
/// so it runs on this interval, after reachability is settled, under
/// [`WORKTREE_SIZE_TIMEOUT`].
pub const WORKTREE_SIZE_REFRESH_SECS: i64 = 6 * 3600;

/// The wall clock for one worktree-size read. On timeout the stored size is
/// kept and the next try waits a full [`WORKTREE_SIZE_REFRESH_SECS`].
pub(crate) const WORKTREE_SIZE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The wall clock for the latency round trip (an empty command).
pub(crate) const ROUND_TRIP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(6);

/// Whether this pass should ask the host for its worktree size.
pub(super) fn worktree_size_due(worktree_at: Option<i64>, now: i64) -> bool {
    worktree_at.is_none_or(|at| now - at >= WORKTREE_SIZE_REFRESH_SECS)
}

/// `du -sk` over fleet's worktrees on the host (not the projects' main
/// checkouts): one `wtp=<index> <kB>` line per path it could measure (the
/// index into `paths`; G1.9, the per-session size), then their sum as one
/// `wtkb=<kB>` line. Every path is quoted with [`crate::shell::quote`]; a
/// leading `~/` becomes `"$HOME"/` so it still expands. `None` when the
/// host has no worktrees to measure.
pub(super) fn worktree_size_script(paths: &[&str]) -> Option<String> {
    let quoted: Vec<String> = paths
        .iter()
        .filter(|p| !p.is_empty())
        .map(|p| match p.strip_prefix("~/") {
            Some(rest) => format!("\"$HOME\"/{}", crate::shell::quote(rest)),
            None => crate::shell::quote(p),
        })
        .collect();
    if quoted.is_empty() {
        return None;
    }
    Some(format!(
        "s=0; i=0; for p in {}; do k=$(du -sk -- \"$p\" 2>/dev/null | cut -f1); \
         case \"$k\" in ''|*[!0-9]*) ;; *) printf 'wtp=%s %s\\n' \"$i\" \"$k\"; s=$((s + k));; esac; \
         i=$((i + 1)); done; printf 'wtkb=%s\\n' \"$s\"",
        quoted.join(" ")
    ))
}

/// Parse [`worktree_size_script`]'s per-path lines: `(index, kB)` for every
/// `wtp=` line whose index is below `n` (the number of paths asked about).
pub(super) fn parse_worktree_sizes(stdout: &str, n: usize) -> Vec<(usize, i64)> {
    stdout
        .lines()
        .filter_map(|l| l.strip_prefix("wtp="))
        .filter_map(|rest| {
            let (i, kb) = rest.trim().split_once(' ')?;
            let i: usize = i.parse().ok()?;
            let kb: i64 = kb.trim().parse().ok()?;
            (i < n && kb >= 0).then_some((i, kb))
        })
        .collect()
}

/// Parse [`worktree_size_script`] output: the `wtkb=` line, else `None`.
pub(super) fn parse_worktree_size(stdout: &str) -> Option<i64> {
    stdout
        .lines()
        .find_map(|l| l.strip_prefix("wtkb="))
        .and_then(|v| v.trim().parse().ok())
}

/// One host's probe result, carried from the off-lock probe task to the
/// under-lock writer.
pub(super) struct HostProbe {
    pub(super) host: HostRow,
    /// The versions section, when this pass asked and the host answered
    /// (task 1). `None`: not due, or unanswerable — the stored versions
    /// are kept and the stamp does not move.
    pub(super) versions: Option<crate::tmux::HostVersions>,
    /// This pass's health sample (task 2), when the host answered it.
    pub(super) health: Option<crate::tmux::HostHealthSample>,
    pub(super) result: Result<Vec<crate::tmux::TmuxSession>, IpcError>,
    /// `None`: not asked this pass (cadence) or unanswerable — the bg
    /// pruner is skipped.
    pub(super) agent_rows: Option<Vec<crate::claude_agents::ClaudeAgentRow>>,
    /// `sessionId → transcript mtime (unix s)` for this pass's `Background`
    /// agents (one extra host call, only when there is at least one; no bg
    /// agent ⇒ `Some` empty map). `None` when that call failed (spawn error,
    /// non-zero exit, timeout) or the whole probe timed out: the mtimes are
    /// unknown, so `reconcile_agent_rows` applies neither the inactive rule
    /// nor dismissal revival that pass — every agent counts as active.
    pub(super) agent_mtimes: Option<std::collections::HashMap<String, i64>>,
    pub(super) intel: PaneIntelMap,
    /// The pane tails `intel` was read from, by `tmux_name`. `intel` is
    /// Claude Code's reading; a row whose agent is another (Codex) is read
    /// again from its tail by that agent's adapter when the row is known.
    pub(super) pane_tails: std::collections::HashMap<String, String>,
    /// `tmux_name → gh pr view` result for the sessions probed THIS pass
    /// (PROD-5). A name absent from the map was not probed (cache still
    /// fresh, host has no `gh`, or the probe failed) and keeps its stored
    /// `pr_url` / `ci_status`.
    pub(super) pr_info: PrInfoMap,
    /// The Claude account the host is logged into per this pass's read of
    /// its `~/.claude.json` (`TmuxExec::read_oauth_account`). `None` when
    /// the read failed or the executor cannot tell — the stored link is then
    /// left untouched, never cleared. `local` always carries `None` here
    /// (it is synced by `service::hosts::sync_local_account` instead).
    pub(super) account: Option<crate::service::hosts::OauthAccount>,
    /// The host's login profiles this pass (`TmuxExec::read_profiles`):
    /// `None` = could not tell, and the stored list is left alone.
    pub(super) profiles: Option<Vec<crate::tmux::HostProfile>>,
    /// This pass's read of the host's boot identity (`TmuxExec::host_identity`),
    /// read BEFORE `list_sessions` (so the list can never be older than the
    /// identity that judges it) and discarded unless the list succeeded. `None`
    /// when `list_sessions` failed, the whole probe timed out, or the
    /// executor could not tell — `reconcile_write_one_host` (`mass_loss_verdict`)
    /// consumes this to distinguish "host rebooted" from "could not tell".
    pub(super) identity: Option<crate::tmux::HostIdentity>,
    /// Unix-epoch second the probe STARTED. Forwarded as
    /// `HostReconcile::probe_started_at` so the writer never ghosts a row that
    /// a newer probe (e.g. `new_session`'s own reconcile) stamped after this
    /// probe listed tmux (BE-3).
    pub(super) started_at: i64,
    /// The worktree-size read (Orbit Fleet 4.6): `None` = not asked this
    /// pass; `Some(None)` = asked, no answer (the stamp still moves);
    /// `Some(Some(kb))` = the size.
    pub(super) worktree_kb: Option<Option<i64>>,
    /// `tmux_name → the rollout its Codex is writing`, for this host's live
    /// Codex rows (`codex::rollouts_script`). A name absent from the map was
    /// not found this pass and keeps its stored conversation.
    pub(super) codex_rollouts: HashMap<String, crate::agent_adapter::codex::PaneRollout>,
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
    /// Home directory `reconcile_sessions_with` reads every pass to keep
    /// `local`'s linked Claude account in sync (see
    /// `service::hosts::sync_local_account` — it links a new/changed
    /// account, refreshes an unchanged one's fields, and otherwise leaves
    /// the existing link untouched). `None` in ordinary test deps
    /// (`fake`/`fake_with_shell`) so the huge majority of reconcile tests
    /// never read a REAL `~/.claude.json` — only `fake_with_local_home` sets
    /// it, for tests that specifically exercise this behaviour. `real()`
    /// always sets it to the directory holding the real `.claude.json`
    /// (`CLAUDE_CONFIG_DIR`, else `$HOME`: `hosts::local_claude_json_dir`).
    pub(super) local_home: Option<std::path::PathBuf>,
    /// Whether this hub's own machine is a fleet host. False on a
    /// `fleet-hub` daemon (`hub.local_host = false`): no `local` row is
    /// created, an existing one is never probed, and the local Claude
    /// account is not read.
    pub(super) local_host: bool,
    /// How often `claude agents --json` (a node cold start) is asked per
    /// host. `0` = every pass (tests).
    pub(super) agents_every: std::time::Duration,
    /// When each host was last asked. `ReconcileDeps::real` is built fresh on
    /// every `list_sessions` / `refresh_sessions` / `reconcile_now` call (see
    /// each entry point below), so this must be the SAME map across those
    /// builds — a process-wide static, like `service::outcome::pr_probe_cache()`
    /// — or `agents_due` never sees a prior ask and `AGENTS_CADENCE` never
    /// applies. `ReconcileDeps::fake*` build a fresh `Arc` per call instead, so
    /// tests stay isolated from each other and from production's map.
    pub(super) last_agents: Arc<dashmap::DashMap<String, std::time::Instant>>,
}

/// The process-wide `last_agents` map every `ReconcileDeps::real` shares (see
/// the field doc above). Mirrors `service::outcome::pr_probe_cache()`.
fn last_agents_map() -> Arc<dashmap::DashMap<String, std::time::Instant>> {
    static MAP: std::sync::LazyLock<Arc<dashmap::DashMap<String, std::time::Instant>>> =
        std::sync::LazyLock::new(|| Arc::new(dashmap::DashMap::new()));
    Arc::clone(&MAP)
}

/// `host alias → tmux executor` factory used by `ReconcileDeps`.
pub(super) type ExecFactory = Box<dyn Fn(&str) -> Box<dyn TmuxExec> + Send + Sync>;

impl ReconcileDeps {
    pub(super) fn real(ssh: &Arc<SshClient>, local_host: bool) -> Arc<Self> {
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
            local_home: local_host.then(crate::service::hosts::local_claude_json_dir),
            local_host,
            agents_every: AGENTS_CADENCE,
            last_agents: last_agents_map(),
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
            local_home: None,
            local_host: true,
            // Tests want the agents probe on every pass unless they build
            // their own `ReconcileDeps` with an explicit cadence.
            agents_every: std::time::Duration::ZERO,
            // A fresh map per fake deps: tests must stay isolated from each
            // other (and from production's shared map above).
            last_agents: Arc::new(dashmap::DashMap::new()),
        })
    }

    /// Like `fake`, but with `local_home` set so a test can exercise the
    /// `local`-account-linking step (see `ReconcileDeps::local_home`).
    #[cfg(test)]
    pub(crate) fn fake_with_local_home(
        exec: impl Fn(&str) -> Box<dyn TmuxExec> + Send + Sync + 'static,
        probe_timeout: std::time::Duration,
        local_home: std::path::PathBuf,
    ) -> Arc<Self> {
        let deps = Self::fake(exec, probe_timeout);
        // Freshly constructed above — refcount is 1, so `get_mut` succeeds.
        let mut deps = deps;
        Arc::get_mut(&mut deps).expect("fresh Arc").local_home = Some(local_home);
        deps
    }

    /// Like `fake`, but `local_host: false` — the hub's own machine is not a
    /// fleet host (a headless `fleet-hub` daemon): no `local` row is created,
    /// a pre-existing one is never probed, and the local Claude account is
    /// not read.
    #[cfg(test)]
    pub(crate) fn fake_without_local(
        exec: impl Fn(&str) -> Box<dyn TmuxExec> + Send + Sync + 'static,
        probe_timeout: std::time::Duration,
    ) -> Arc<Self> {
        let mut deps = Self::fake(exec, probe_timeout);
        Arc::get_mut(&mut deps).expect("fresh Arc").local_host = false;
        deps
    }

    /// Override the agents cadence on a freshly built `Arc<ReconcileDeps>`
    /// (e.g. `ReconcileDeps::fake(..).with_agents_every(Duration::from_secs(60))`),
    /// for a test that must make `agents_due` say "not yet" on a later pass
    /// instead of `fake`'s always-due zero cadence.
    #[cfg(test)]
    pub(crate) fn with_agents_every(self: Arc<Self>, every: std::time::Duration) -> Arc<Self> {
        let mut deps = self;
        Arc::get_mut(&mut deps).expect("fresh Arc").agents_every = every;
        deps
    }
}

/// Whether this pass asks `host` for its agents; records the ask.
pub(super) fn agents_due(deps: &ReconcileDeps, alias: &str, now: std::time::Instant) -> bool {
    let due = match deps.last_agents.get(alias) {
        Some(last) => now.duration_since(*last) >= deps.agents_every,
        None => true,
    };
    if due {
        deps.last_agents.insert(alias.to_string(), now);
    }
    due
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

    /// Claim the single pass slot, waiting for a pass already running.
    pub async fn begin(&self) -> ReconcilePass<'_> {
        ReconcilePass {
            _guard: self.running.lock().await,
            gate: self,
        }
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

    /// `true` once at least one full pass has completed in this process,
    /// regardless of the freshness window. `list_sessions_with` uses this to
    /// tell a cold start (nothing probed yet — keep the first listing
    /// synchronous so it is not empty) from ordinary staleness (a pass has
    /// run before, so a stale caller can be served the stored rows at once
    /// while a background pass catches up).
    pub(super) fn has_completed_once(&self) -> bool {
        self.last_completed.lock().ok().is_some_and(|l| l.is_some())
    }

    /// Completed full passes so far.
    #[cfg(test)]
    pub fn passes(&self) -> u64 {
        self.passes.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// The process-wide gate every production entry point shares. An `Arc` (not
/// a `&'static` reference) so `list_sessions_with` can clone it into a
/// detached background pass without borrowing beyond the call.
pub fn reconcile_gate() -> Arc<ReconcileGate> {
    static GATE: std::sync::LazyLock<Arc<ReconcileGate>> =
        std::sync::LazyLock::new(|| Arc::new(ReconcileGate::new()));
    Arc::clone(&GATE)
}

/// Analyze the pane tail captured for every live session on a host by the
/// batched probe. A tail absent (capture failed) or empty is skipped (no map
/// entry) rather than aborting — reconcile must be robust to a session whose
/// pane just vanished.
pub(super) fn intel_from_tails(tails: &std::collections::HashMap<String, String>) -> PaneIntelMap {
    tails
        .iter()
        .filter(|(_, t)| !t.is_empty())
        .map(|(name, t)| (name.clone(), crate::agent_adapter::claude().analyze_pane(t)))
        .collect()
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

#[cfg(test)]
thread_local! {
    /// Test-only fault injection: when set to a host alias,
    /// `reconcile_write_one_host` fails that host's write AFTER every store
    /// write of its reachable branch has run (upsert, PR signals, timeline
    /// events, bg rows), so a test can check what an error that late leaves
    /// behind. Thread-local, so parallel tests cannot trip each other.
    pub(super) static FAIL_HOST_WRITE_AFTER_WRITES: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// `Err` when [`FAIL_HOST_WRITE_AFTER_WRITES`] names `alias`; `Ok` always in
/// a non-test build.
fn injected_host_write_fault(alias: &str) -> Result<(), IpcError> {
    #[cfg(test)]
    if FAIL_HOST_WRITE_AFTER_WRITES.with(|f| f.borrow().as_deref() == Some(alias)) {
        return Err(IpcError::new(
            codes::E_INTERNAL,
            format!("injected write fault for {alias}"),
        ));
    }
    let _ = alias;
    Ok(())
}

/// A known row's stored values read before the reconcile write.
struct Prior {
    claude_status: Option<String>,
    stuck_kind: Option<String>,
    claude_session_id: Option<String>,
}

/// The rollout this pass found for `tmux_name`, when the stored row runs
/// Codex (a first sighting is no Codex row yet).
fn codex_rollout<'a>(
    row: Option<&SessionRow>,
    probe: &'a HostProbe,
    tmux_name: &str,
) -> Option<&'a crate::agent_adapter::codex::PaneRollout> {
    row.filter(|r| r.agent == crate::store::AGENT_CODEX && r.kind != "shell")?;
    probe.codex_rollouts.get(tmux_name)
}

/// Fallback rebind (spec §1.4): after the reconcile write, open the
/// conversation of a row that `claude agents` moved onto another id (no
/// hooks, or an old CLI), or first showed carrying one (a new row, or one
/// whose id was NULL). Opened as `unknown`; the upsert already cleared the
/// stale transcript path. Best-effort: a failure is logged, never fatal —
/// unless it took the host's transaction with it ([`Store::ensure_in_tx`]).
fn open_reconciled_conversation(
    s: &Store,
    host_alias: &str,
    row: &SessionRow,
    old_claude_id: Option<&str>,
    transcript_path: Option<&str>,
) -> Result<(), IpcError> {
    let Some(new_id) = row.claude_session_id.as_deref() else {
        return Ok(());
    };
    // The upsert is the one guard for "never bind one id to two rows" and
    // for "never undo a newer hook rebind": it refuses such an id, so the
    // stored id only differs from the prior one when this pass may own it.
    if old_claude_id == Some(new_id) {
        return Ok(());
    }
    match s.rebind_conversation(row.id, new_id, StartSource::Unknown, transcript_path, None) {
        Ok(_) => {
            if let Err(e) = s.insert_session_event_for(
                row.id,
                Some(new_id),
                "conversation_started",
                Some(StartSource::Unknown.as_str()),
            ) {
                tracing::warn!(host = %host_alias, session = %row.tmux_name, error = %e.message,
                    "[reconcile] session_event insert failed");
                s.ensure_in_tx()?;
            }
        }
        Err(e) => {
            tracing::warn!(host = %host_alias, session = %row.tmux_name, error = %e.message,
                "[reconcile] conversation rebind failed");
            s.ensure_in_tx()?;
        }
    }
    Ok(())
}

/// Apply one host's probe result to the store. Extracted from the reconcile
/// loop so a per-host write failure can be isolated (logged) without `?`
/// aborting the whole multi-host reconcile.
///
/// A reachable host's whole write — account link, mass-loss marks, boot
/// identity, the session upsert/ghost burst, PR signals, timeline events,
/// conversation rebinds and the pane-less agent rows — commits in ONE
/// transaction (`Store::atomically`, Task 4): one commit per host instead of
/// one per statement, and an error anywhere rolls the host back as a whole.
/// Events are held until that commit and dropped on rollback. Every store
/// helper reached from inside it is SAVEPOINT-based or a plain statement, so
/// none of them opens a second `BEGIN`.
pub(super) fn reconcile_write_one_host(
    s: &mut Store,
    probe: &HostProbe,
    projects: &[ProjectRow],
) -> Result<(), IpcError> {
    let host = &probe.host;
    match &probe.result {
        Ok(live) => s.atomically(|s| write_reachable_host(s, probe, projects, live))?,
        Err(e) => {
            // Mark host unreachable; surface last-known sessions so the UI
            // can render them dimmed/red. We KEEP them (no delete).
            tracing::warn!(host = %host.alias, code = %e.code, error = %e.message, "[reconcile] host unreachable");
            s.apply_host_reconcile_failed(
                HostReconcile {
                    alias: &host.alias,
                    reachable: false,
                    claude_version: host.claude_version.as_deref(),
                    tmux_version: host.tmux_version.as_deref(),
                    last_pinged_at: now_unix(),
                    probe_started_at: probe.started_at,
                    sessions: &[],
                    keep: &[],
                    lost_ttl_cutoff: None,
                    skip_prune: false,
                    reconciled_at: None,
                },
                (&e.code, &e.message),
            )?;
        }
    }
    Ok(())
}

/// The reachable-host half of [`reconcile_write_one_host`], run inside its
/// per-host transaction (`s` is the `&Store` `atomically` hands over).
fn write_reachable_host(
    s: &Store,
    probe: &HostProbe,
    projects: &[ProjectRow],
    live: &[crate::tmux::TmuxSession],
) -> Result<(), IpcError> {
    let host = &probe.host;
    let paths = HostPaths::for_host(s, &host.alias);
    // `for_host` swallows its reads (settings, worktrees); one SQLite
    // answered with a rollback must stop the plain writes below.
    s.ensure_in_tx()?;
    // `None` (not asked this pass, or unanswerable) reads as "no agents"
    // for pairing/status purposes ONLY — the pruner below is gated
    // separately on `probe.agent_rows` itself, so a `None` never ghosts a
    // bg row.
    let agent_rows: &[crate::claude_agents::ClaudeAgentRow] =
        probe.agent_rows.as_deref().unwrap_or(&[]);
    let intel = &probe.intel;
    // Relink the host to the account it is logged into NOW, before
    // the sessions below are attributed: a session first seen in the
    // same pass as the account switch must carry the new account,
    // not the one snapshotted into `host` before the probe. `None`
    // (read failed / logged out / `local`) leaves the link alone.
    // Best-effort: a failure here must not cost the host its
    // session reconcile — fall back to the snapshotted link.
    let host_account =
        match crate::service::hosts::sync_host_account(s, &host.alias, probe.account.as_ref()) {
            Ok(uuid) => uuid,
            Err(e) => {
                tracing::warn!(
                    host = %host.alias,
                    error = %e.message,
                    "[reconcile] host account sync failed; keeping the stored link"
                );
                s.ensure_in_tx()?;
                host.account_uuid.clone()
            }
        };
    // The host's login profiles and the account each is logged into, so a
    // session under a profile is attributed to THAT login (docs/accounts.md).
    // Best-effort like the host account above.
    let profile_accounts = match crate::service::hosts::sync_host_profiles(
        s,
        &host.alias,
        probe.profiles.as_deref(),
    ) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(
                host = %host.alias,
                error = %e.message,
                "[reconcile] host profile sync failed; keeping the stored list"
            );
            s.ensure_in_tx()?;
            Default::default()
        }
    };
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
    // read-back. `None` is a first sighting (no stored row yet).
    let mut priors: Vec<(String, Option<Prior>)> = Vec::with_capacity(live.len());
    // Every row on this host (ghost / lost / pane-less included), read
    // once: each live session's prior row, account and stale-working
    // memory come from here, not from three point queries per session.
    // `s` is held for this whole function and the loop below only reads,
    // so the map is what the write sees.
    let stored: HashMap<String, SessionRow> = s
        .list_sessions_for_host(&host.alias)?
        .into_iter()
        .map(|r| (r.tmux_name.clone(), r))
        .collect();
    // The Claude id every stored row holds, so an inferred cwd match can be
    // checked against the ids other rows already own.
    let stored_ids: HashMap<String, Option<String>> = stored
        .iter()
        .map(|(name, r)| (name.clone(), r.claude_session_id.clone()))
        .collect();
    let agents = pair_session_agents(live, agent_rows, &stored_ids, host.alias == "local");
    for sess in live {
        keep.push(sess.name.clone());
        let project_id = find_project_id_for_path(projects, &host.alias, &sess.path, &paths);
        // The PRIOR stored row (transition detection below, and the
        // stale-working veto); `None` is a first sighting.
        let prior_row = stored.get(&sess.name);
        // Preservation invariant: if the session already has an
        // account_uuid in the DB, keep it; only capture the host's
        // current account for newly-discovered sessions. A session under a
        // credential profile bills that profile's login, never the host's:
        // it takes the profile's account when the host reported one, and
        // otherwise keeps what it has.
        let account_uuid = match prior_row {
            Some(p) if p.claude_profile.is_some() => p
                .claude_profile
                .as_ref()
                .and_then(|name| profile_accounts.get(name).cloned().flatten())
                .or_else(|| p.account_uuid.clone()),
            _ => prior_row
                .and_then(|p| p.account_uuid.clone())
                .or_else(|| host_account.clone()),
        };
        let worktree_key = worktree_key_for_host(&sess.path.to_string_lossy(), &paths);
        // The running Claude agent this session is paired with (see
        // `pair_session_agents`) — its id lets `recreate`/`restart`
        // resume the exact conversation instead of "most recent for
        // the cwd".
        let agent = agents.get(&sess.name).copied();
        // Pane-tail intel from the off-lock probe (may be absent if the
        // capture failed — then all four intel fields stay None and the
        // upsert's COALESCE preserves the session's prior values).
        // A row running another agent (Codex) is read by its own adapter.
        let own_read = prior_row
            .filter(|r| r.agent != crate::store::AGENT_CLAUDE && r.kind != "shell")
            .and_then(|r| crate::agent_adapter::by_id(&r.agent))
            .and_then(|a| {
                let tail = probe.pane_tails.get(&sess.name)?;
                (!tail.is_empty()).then(|| a.analyze_pane(tail))
            });
        let pane = own_read.as_ref().or_else(|| intel.get(&sess.name));
        // PR probe result for this pass (PROD-5). `pr_observed`
        // makes the values authoritative so a closed PR's stale
        // link clears; an unprobed session keeps its stored fields.
        let pr = probe.pr_info.get(&sess.name);
        let agent_status = known_agent_status(&sess.name, agent.and_then(|a| a.status.as_deref()));
        // Already vocabulary-checked (and logged when dropped) by
        // `known_agent_status` above, so the reparse can't fail.
        let agent_status_typed = agent_status
            .as_deref()
            .and_then(|s| s.parse::<crate::service::pane_intel::ClaudeStatus>().ok());
        let pane_status = pane.and_then(|p| p.derived_status);
        // The veto is armed by the attention stamp OR by the demotion's own
        // memory (`stale_demoted_at`): an attach or
        // `reconcile.stale_working_ttl_secs` ends the reason, not the
        // demotion.
        let stale =
            prior_row.is_some_and(|p| p.stale_working_at.is_some() || p.stale_demoted_at.is_some());
        // Prefer the authoritative `claude agents` status; fall back to
        // the pane heuristic per `status_candidate` — full weight when
        // this pass actually asked, `Blocked`-only otherwise (a
        // cadence-skipped or unanswerable pass must not let a weak
        // pane guess overwrite the stored status every time). A row the
        // tick demoted for staleness keeps its `idle` unless the pane
        // itself shows a turn (`stale_working_veto`, F2).
        let claude_status = stale_working_veto(
            stale,
            status_candidate(
                probe.agent_rows.is_some(),
                agent_status_typed,
                pane_status,
                prior_row.is_some_and(|p| p.claude_status.as_deref() == Some("blocked")),
            ),
            pane_status,
        )
        .map(|s| s.as_str().to_string());
        let stuck_kind = pane.and_then(|p| p.stuck.map(|k| k.as_str().to_string()));
        // Transition-detection: remember the PRIOR stored values (the
        // upsert below overwrites them). A first sighting skips the
        // status/stuck detection but still opens its conversation.
        priors.push((
            sess.name.clone(),
            prior_row.map(|p| Prior {
                claude_status: p.claude_status.clone(),
                stuck_kind: p.stuck_kind.clone(),
                claude_session_id: p.claude_session_id.clone(),
            }),
        ));
        sessions.push(ReconcileSession {
            tmux_name: &sess.name,
            project_id,
            created_at: sess.created,
            last_activity_at: sess.last_activity,
            account_uuid,
            worktree_key,
            // A Codex pane's conversation is the rollout found for it; it
            // has no `claude agents` row.
            claude_session_id: agent
                .and_then(|a| a.session_id.clone())
                .or_else(|| codex_rollout(prior_row, probe, &sess.name).map(|r| r.id.clone())),
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
            pr_evidence: pr.and_then(|p| p.evidence.clone()),
            tmux_pane_id: sess.pane_id.clone(),
            pending_input: pane.and_then(|p| p.pending_input.clone()),
            // The spinner on screen is life, whether or not this pass asked
            // `claude agents`: the stale-working sweep that runs right after
            // this pass must not demote a row its pane shows working (F2's
            // demote-lift-demote flap during one long tool call).
            pane_working: pane_status == Some(crate::service::pane_intel::ClaudeStatus::Working),
        });
    }
    let now = now_unix();
    // ── Task 6: reboot / vanished-tmux-server safety net ──
    // Compare this pass's observed boot identity against the LAST
    // STORED one (read before the write below overwrites it) — a
    // changed boot id or tmux server pid on an otherwise-reachable
    // host means every session on it was lost, not merely the ones
    // absent from `keep` this pass. Keep the read `Result` around
    // (not just `.unwrap_or_default()`): a failed read must block
    // the identity WRITE below too (see the comment there), not
    // just fall back for the comparison.
    let stored_identity_read = s.get_host_identity(&host.alias);
    s.ensure_in_tx()?;
    let stored_identity_read_ok = stored_identity_read.is_ok();
    let stored_identity = stored_identity_read.unwrap_or_default();
    let verdict = mass_loss_verdict(&stored_identity, probe.identity.as_ref());
    // `true` only once `mark_host_sessions_lost` is known to have
    // actually marked at least one row — a verdict whose mark
    // failed, or that had nothing left to mark (every affected row
    // already ghost, or exempted by the BE-3 guard below), must not
    // suppress the routine prune (fix 4) nor let the identity write
    // below proceed as if the verdict's side effect had landed
    // (fix 3).
    let mut marked_any = false;
    let mut mark_failed = false;
    if let Some(reason) = verdict {
        // `keep` names tmux sessions only; the pane-less agent rows
        // this probe saw live (`bg:<id>`, synthesised by
        // `reconcile_agent_rows` below) must be spared too, or a
        // `host_reboot` verdict marks an agent running NOW lost and
        // the agent pass revives it in the same call — leaving a
        // spurious permanent `lost` event behind.
        let mut verdict_keep = keep.clone();
        verdict_keep.extend(live_agent_row_names(
            live,
            agent_rows,
            host.alias == "local",
        ));
        match s.mark_host_sessions_lost(&host.alias, reason, &verdict_keep, now, probe.started_at) {
            Ok(lost) => {
                // Its re-read of each reclassified row is best-effort.
                s.ensure_in_tx()?;
                // Reclassified rows count too: they are this
                // verdict's first loss record (a failed earlier pass
                // only ghosted them as `missing`), so the routine
                // prune must not reap them this pass either.
                marked_any = !lost.is_empty();
                for row in lost.marked.iter().chain(&lost.reclassified) {
                    if let Err(e) = s.insert_session_event(row.id, "lost", Some(reason)) {
                        tracing::warn!(
                            host = %host.alias,
                            session = %row.tmux_name,
                            error = %e,
                            "[reconcile] lost event insert failed"
                        );
                        s.ensure_in_tx()?;
                    }
                }
            }
            Err(e) => {
                mark_failed = true;
                tracing::warn!(
                    host = %host.alias,
                    error = %e,
                    "[reconcile] mark lost failed"
                );
                s.ensure_in_tx()?;
            }
        }
    }
    // Persist the observed identity AFTER computing the verdict
    // against the previously stored value — writing it first would
    // make every future comparison compare the identity with
    // itself and the verdict would never fire again. Never write
    // when the probe could not tell (`None`): that would erase a
    // known-good identity on a transient read failure. Also never
    // write when the stored-identity READ above failed (writing
    // now would silently consume a verdict opportunity the read
    // failure hid — the comparison used `unwrap_or_default()`,
    // which is not the same as "nothing changed"), or when a
    // verdict fired but its `mark_host_sessions_lost` call failed
    // (the identity would then move on with the loss never
    // recorded, so the next pass sees a normal pass and never
    // retries the mark).
    if stored_identity_read_ok && !mark_failed {
        if let Some(id) = &probe.identity {
            if let Err(e) =
                s.set_host_identity(&host.alias, id.boot_id.as_deref(), id.tmux_server_pid)
            {
                tracing::warn!(
                    host = %host.alias,
                    error = %e,
                    "[reconcile] identity write failed"
                );
                s.ensure_in_tx()?;
            }
        }
    }
    let lost_ttl_raw = s
        .get_setting(crate::service::settings::SESSIONS_LOST_TTL_SECS)
        .ok()
        .flatten();
    // Task 1: a version the host answered THIS pass replaces the stored
    // one; a missing answer (not due, or the binary said nothing) keeps
    // it. Only an answered `claude --version` moves the stamp — the stamp
    // means "read from the host", never "asked".
    let probed = probe.versions.as_ref();
    let claude_version = probed
        .and_then(|v| v.claude_version.as_deref())
        .or(host.claude_version.as_deref());
    let tmux_version = probed
        .and_then(|v| v.tmux_version.as_deref())
        .or(host.tmux_version.as_deref());
    if probed.is_some_and(|v| v.claude_version.is_some()) {
        s.set_host_versions_at(&host.alias, now)?;
    }
    // Task 2: the health sample, every pass the host answered it.
    if let Some(h) = &probe.health {
        s.set_host_health(&host.alias, h, now)?;
    }
    if let Some(kb) = probe.worktree_kb {
        s.set_host_worktree_size(&host.alias, kb, now)?;
    }
    s.ensure_in_tx()?;
    s.apply_host_reconcile_in_tx(HostReconcile {
        alias: &host.alias,
        reachable: true,
        claude_version,
        tmux_version,
        last_pinged_at: now,
        probe_started_at: probe.started_at,
        sessions: &sessions,
        keep: &keep,
        lost_ttl_cutoff: read_lost_ttl_cutoff(lost_ttl_raw, now),
        // A pass that just mass-marked this host's sessions lost
        // must not immediately re-ghost (and restart the reap clock
        // on) those very rows via the routine keep-set prune below —
        // but only when something was actually marked; otherwise
        // the prune is a normal no-op pass, per the plan's ruling.
        skip_prune: marked_any,
        // Task H: stamp freshness on every session this pass observed live,
        // so a proactive (background) reconcile keeps `last_reconciled_at`
        // current and the UI can dim rows whose host has gone quiet. It is
        // also the BE-3 ghost guard's evidence (`probe_started_at` above).
        // Carried by the upsert itself (Task 4), not a second UPDATE after
        // it; an unchanged pass bumps no `row_version` (migration 063).
        reconciled_at: Some(now),
    })?;
    // Work detection (M4.2): the PR probe's signals, written outside
    // the upsert (never in its `ON CONFLICT` list) and only for the
    // few sessions probed this pass. A change re-resolves the
    // session's links. Best-effort.
    for (tmux_name, info) in &probe.pr_info {
        let signals = match (&info.pr_url, &info.signals) {
            (None, _) => None,
            (Some(_), Some(sig)) => serde_json::to_string(sig).ok(),
            // Basic fields only (an older `gh`): nothing to say.
            (Some(_), None) => continue,
        };
        match s.set_pr_signals(&host.alias, tmux_name, signals.as_deref()) {
            Ok(Some(sid)) => {
                if let Err(e) = crate::service::work::detect::resolve_session(s, sid) {
                    tracing::debug!(error = %e.message, "[work] PR resolve failed");
                    s.ensure_in_tx()?;
                }
                // The merged-PR stamp, once more now that the links have
                // settled (native item status, design 2026-09-28 §2):
                // `set_pr_signals` already tried, but the resolve above may
                // have only just confirmed the link the stamp needs. Both
                // calls are on a signal that CHANGED this pass — a stale
                // merged signal must never stamp work the session was
                // pointed at later — and both are idempotent, so the second
                // writes only what the first could not see yet.
                if info.signals.as_ref().is_some_and(|sg| sg.is_merged()) {
                    if let Err(e) = s.stamp_derived_done_for_session(sid) {
                        tracing::debug!(error = %e.message, "[work] merged PR did not stamp");
                        s.ensure_in_tx()?;
                    }
                }
                // Write-back (M13.4e): after the links settled, queue the
                // PR's remote link where an admin allows it. Idempotent.
                if let Some(url) = info.pr_url.as_deref() {
                    if let Err(e) = crate::service::trackers::write_back::on_pr(s, sid, url) {
                        tracing::debug!(error = %e.message, "[work] PR write-back not queued");
                        s.ensure_in_tx()?;
                    }
                }
            }
            Ok(None) => {}
            Err(e) => {
                tracing::debug!(error = %e.message, "[work] PR signals not stored");
                s.ensure_in_tx()?;
            }
        }
    }
    // Task G: the upsert has run (inside this host's transaction, so these
    // reads see it) — read each known row back and record a transition
    // only where the STORED value changed.
    // Append-only and best-effort — a failed insert is logged and
    // skipped, never blocking reconcile.
    for (tmux_name, prior) in &priors {
        let row = match s.get_session(tmux_name, &host.alias) {
            Ok(Some(row)) => row,
            Ok(None) => continue,
            Err(_) => {
                s.ensure_in_tx()?;
                continue;
            }
        };
        let rollout = codex_rollout(Some(&row), probe, tmux_name)
            .filter(|r| row.claude_session_id.as_deref() == Some(r.id.as_str()));
        open_reconciled_conversation(
            s,
            &host.alias,
            &row,
            prior.as_ref().and_then(|p| p.claude_session_id.as_deref()),
            rollout.map(|r| r.path.as_str()),
        )?;
        // A Codex row's transcript is its rollout: stored, so every read
        // goes straight to it (a no-op once it is).
        if let Some(r) = rollout {
            if let Err(e) = s.set_transcript_path_for_row(row.id, &r.id, &r.path) {
                tracing::warn!(host = %host.alias, session = %row.tmux_name, error = %e.message,
                    "[reconcile] codex rollout path write failed");
                s.ensure_in_tx()?;
            }
        }
        let Some(prior) = prior else {
            continue;
        };
        let mut events: Vec<(&str, Option<&str>)> = Vec::new();
        if row.claude_status != prior.claude_status {
            events.push(("status_change", row.claude_status.as_deref()));
        }
        // A newly-set (or changed) stuck_kind is the alert-worthy
        // event; clearing it back to None is not recorded.
        if row.stuck_kind.is_some() && row.stuck_kind != prior.stuck_kind {
            tracing::info!(
                host = %host.alias,
                session = %tmux_name,
                kind = row.stuck_kind.as_deref().unwrap_or("?"),
                was = prior.stuck_kind.as_deref().unwrap_or("none"),
                "[reconcile] stuck"
            );
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
                s.ensure_in_tx()?;
            }
        }
    }
    // SECOND pass: `claude agents` rows that matched NO tmux session
    // are never in `keep` and would otherwise be invisible. Surface
    // each as a synthetic pane-less SessionRow (`kind='external'` for
    // interactive sessions, `kind='bg'` for background jobs) so it
    // appears in `list_sessions`. These rows are exempt from the
    // tmux-keyed ghost cleanup (`Store::ghost_and_clean`) —
    // instead they are pruned inside `reconcile_agent_rows` against
    // the current `claude agents --json` result, so dead agents can't
    // accumulate.
    if let Some(agents) = &probe.agent_rows {
        reconcile_agent_rows(
            s,
            &host.alias,
            live,
            projects,
            agents,
            probe.agent_mtimes.as_ref(),
            now,
            probe.started_at,
        )?;
    }
    injected_host_write_fault(&host.alias)?;
    Ok(())
}

/// Pair each live tmux session on a host with the Claude agent whose id and
/// status reconcile records for it, keyed by tmux name. Pure so it's
/// unit-testable.
///
/// A by-name match (a session launched with `--name <tmux_name>`) is
/// authoritative and always pairs, so its id follows the conversation (e.g.
/// after `/clear`). A match by unique cwd is only an inference — when two
/// fleet sessions share a cwd and only one has registered its agent yet,
/// the cwd match hands the other session the first one's agent. So a cwd
/// match pairs only when all of these hold:
///
/// * the agent has a session id (otherwise nothing ties it to this session);
/// * the session's stored id (`stored_ids`) is NULL or already that id — an
///   inference never overwrites an id the row has;
/// * no OTHER row on the host (any state, pane-less rows included) holds
///   that id;
/// * no other live session matched that agent by name, and no other live
///   session inferred the same agent by cwd this pass.
///
/// A rejected cwd match pairs nothing: the agent's status is not attributed
/// to the session either (it falls back to the pane-derived status).
pub(super) fn pair_session_agents<'a>(
    live: &[crate::tmux::TmuxSession],
    agents: &'a [crate::claude_agents::ClaudeAgentRow],
    stored_ids: &HashMap<String, Option<String>>,
    is_local: bool,
) -> HashMap<String, &'a crate::claude_agents::ClaudeAgentRow> {
    let matches: Vec<_> = live
        .iter()
        .filter_map(|sess| {
            crate::claude_agents::find_for_session(
                agents,
                &sess.name,
                &sess.path.to_string_lossy(),
                is_local,
            )
            .map(|m| (sess.name.as_str(), m))
        })
        .collect();
    let named: std::collections::HashSet<&str> = matches
        .iter()
        .filter(|(_, m)| m.by_name)
        .filter_map(|(_, m)| m.row.session_id.as_deref())
        .collect();
    let mut inferred: HashMap<&str, usize> = HashMap::new();
    for (_, m) in matches.iter().filter(|(_, m)| !m.by_name) {
        if let Some(id) = m.row.session_id.as_deref() {
            *inferred.entry(id).or_default() += 1;
        }
    }
    let mut out = HashMap::new();
    for (name, m) in matches {
        let accept = m.by_name
            || m.row.session_id.as_deref().is_some_and(|id| {
                let own = stored_ids.get(name).and_then(|v| v.as_deref());
                own.is_none_or(|own| own == id)
                    && !named.contains(id)
                    && inferred.get(id) == Some(&1)
                    && !stored_ids
                        .iter()
                        .any(|(other, v)| other != name && v.as_deref() == Some(id))
            });
        if accept {
            out.insert(name.to_string(), m.row);
        }
    }
    out
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
        if let Some(m) = crate::claude_agents::find_for_session(
            agents,
            &sess.name,
            &sess.path.to_string_lossy(),
            is_local,
        ) {
            if let Some(id) = m.row.session_id.as_deref() {
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

/// The synthetic `sessions.tmux_name` of a pane-less agent row
/// (`kind='bg'` / `'external'`) — `bg:<claude session id>`.
pub(super) fn agent_row_name(session_id: &str) -> String {
    format!("bg:{session_id}")
}

/// The label an `external` row takes from its agent's `name`: trimmed and
/// capped to the 80 characters `validate::friendly_name` allows. `None` for a
/// missing or blank name, and for one carrying a control character (the CLI
/// output is not ours; `set_friendly_name` refuses those too), so such a pass
/// keeps the row's current label.
pub(super) fn agent_display_name(raw: Option<&str>) -> Option<String> {
    let name = raw?.trim();
    if name.is_empty() || name.chars().any(char::is_control) {
        return None;
    }
    Some(
        name.chars()
            .take(80)
            .collect::<String>()
            .trim_end()
            .to_string(),
    )
}

/// The row names [`reconcile_agent_rows`] keys this probe's live pane-less
/// agents under: every [`unmatched_bg_agents`] entry with a session id.
/// Dismissed agents are included — their row was deleted on dismissal, so
/// naming them in a keep set is harmless.
pub(super) fn live_agent_row_names(
    live: &[crate::tmux::TmuxSession],
    agents: &[crate::claude_agents::ClaudeAgentRow],
    is_local: bool,
) -> Vec<String> {
    unmatched_bg_agents(live, agents, is_local)
        .into_iter()
        .filter_map(|a| a.session_id.as_deref().map(agent_row_name))
        .collect()
}

/// Seconds without transcript activity after which a non-working background
/// agent is shown as `stopped` (spec §2).
pub(super) const AGENT_INACTIVE_SECS: i64 = 86_400;

/// A background agent is inactive when its status is not `working` and its
/// last known activity is at least [`AGENT_INACTIVE_SECS`] old. An unknown
/// activity time means active — never guess an agent dead.
pub(super) fn agent_is_inactive(
    status: Option<&str>,
    last_activity: Option<i64>,
    now: i64,
) -> bool {
    status != Some("working") && last_activity.is_some_and(|t| now - t >= AGENT_INACTIVE_SECS)
}

/// Upsert a synthetic pane-less SessionRow for every `claude agents` row that
/// has no tmux session (the reconcile "second pass"): `kind='external'` for an
/// interactive session running outside fleet, `kind='bg'` for a background
/// job. A bg agent idle for [`AGENT_INACTIVE_SECS`] (transcript mtime from
/// `mtimes`, else `started_at`) is stored as `stopped`. An agent the user
/// dismissed is skipped until it shows activity newer than the dismissal,
/// which clears the dismissal. `mtimes` is `None` when the transcript probe
/// failed: the mtimes are unknown (an empty map would read as "no
/// transcript" and let an old `started_at` retire a live agent), so that
/// pass skips the inactive rule entirely and keeps every dismissal in
/// force. Then prune the host's pane-less rows whose
/// agent is NOT in this pass. The prune is two-phase (ghost this pass,
/// hard-delete next pass) via `ghost_and_clean_bg_sessions`, so a
/// transiently-failed agents probe — which comes back as an empty list — only
/// ghosts rows for one cycle instead of deleting them. Per-agent write
/// failures are logged and skipped so one bad row can't abort the others.
///
/// `probe_started_at` is this pass's `HostProbe::started_at`, threaded
/// through to `upsert_bg_session`'s staleness guard so a pass that listed the
/// agents before a row was lost cannot revive it on the way out.
#[allow(clippy::too_many_arguments)]
pub(super) fn reconcile_agent_rows(
    s: &Store,
    host_alias: &str,
    live: &[crate::tmux::TmuxSession],
    projects: &[ProjectRow],
    agents: &[crate::claude_agents::ClaudeAgentRow],
    mtimes: Option<&std::collections::HashMap<String, i64>>,
    now: i64,
    probe_started_at: i64,
) -> Result<(), IpcError> {
    let mut keep: Vec<String> = Vec::new();
    let paths = HostPaths::for_host(s, host_alias);
    s.ensure_in_tx()?;
    let dismissed = match s.dismissed_agents(host_alias) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(
                host = %host_alias,
                error = %e,
                "[reconcile] reading dismissed agents failed"
            );
            s.ensure_in_tx()?;
            Default::default()
        }
    };
    for agent in unmatched_bg_agents(live, agents, host_alias == "local") {
        let Some(session_id) = agent.session_id.as_deref() else {
            continue;
        };
        let last_activity = mtimes
            .and_then(|m| m.get(session_id).copied())
            .or(agent.started_at);
        if let Some(&at) = dismissed.get(session_id) {
            match last_activity {
                // Revive only on known evidence: with the mtimes unknown the
                // dismissal stays in force.
                Some(t) if mtimes.is_some() && t > at => {
                    if let Err(e) = s.clear_agent_dismissal(host_alias, session_id) {
                        tracing::warn!(
                            host = %host_alias,
                            claude_session_id = %session_id,
                            error = %e,
                            "[reconcile] clearing agent dismissal failed"
                        );
                        s.ensure_in_tx()?;
                    }
                }
                // Still dismissed. Not in `keep`, so a leftover row (if any)
                // is pruned below.
                _ => continue,
            }
        }
        let tmux_name = agent_row_name(session_id);
        // Keep the sentinel even if the upsert below fails — ghosting an
        // existing row over a transient write error would be wrong.
        keep.push(tmux_name.clone());
        let kind = match agent.kind {
            crate::claude_agents::AgentKind::Interactive => "external",
            crate::claude_agents::AgentKind::Background => "bg",
        };
        let project_id = agent.cwd.as_deref().and_then(|cwd| {
            find_project_id_for_path(projects, host_alias, std::path::Path::new(cwd), &paths)
        });
        // Same vocabulary filter as tmux rows: an unknown value is logged and
        // dropped (the upsert's COALESCE then keeps the prior status).
        let mut status = known_agent_status(&tmux_name, agent.status.as_deref());
        if kind == "bg"
            && mtimes.is_some()
            && agent_is_inactive(status.as_deref(), last_activity, now)
        {
            status = Some("stopped".to_string());
        }
        if let Err(e) = s.upsert_bg_session(
            host_alias,
            &tmux_name,
            project_id,
            session_id,
            status.as_deref(),
            now,
            kind,
            probe_started_at,
        ) {
            tracing::warn!(
                host = %host_alias,
                claude_session_id = %session_id,
                error = %e,
                "[reconcile] bg upsert failed"
            );
            s.ensure_in_tx()?;
            continue;
        }
        // Without it an external row reads as its `bg:<uuid>` sentinel.
        if kind == "external" {
            if let Some(name) = agent_display_name(agent.name.as_deref()) {
                if let Err(e) = s.set_external_agent_name(host_alias, &tmux_name, &name) {
                    tracing::warn!(
                        host = %host_alias,
                        claude_session_id = %session_id,
                        error = %e,
                        "[reconcile] external agent label failed"
                    );
                    s.ensure_in_tx()?;
                }
            }
        }
    }
    // Same TTL cutoff as the tmux-keyed prune in `reconcile_write_one_host`
    // (a `host_reboot` verdict marks bg rows lost too — only
    // `tmux_server_gone` is tmux-only — so a resumable bg row deserves the
    // same exemption; an `external` row never gets it, see
    // `Store::ghost_and_clean`). Read fresh here rather than threaded through as a
    // parameter so this function's signature (and its many direct callers
    // in tests) is unchanged.
    let lost_ttl_raw = s
        .get_setting(crate::service::settings::SESSIONS_LOST_TTL_SECS)
        .ok()
        .flatten();
    let grace_raw = s
        .get_setting(crate::service::settings::GC_EXTERNAL_LOST_TTL_SECS)
        .ok()
        .flatten();
    s.ensure_in_tx()?;
    let lost_ttl_cutoff = read_lost_ttl_cutoff(lost_ttl_raw, now);
    let external_grace_cutoff = read_external_grace_cutoff(grace_raw, now);
    if let Err(e) = s.ghost_and_clean_bg_sessions(
        host_alias,
        &keep,
        now,
        lost_ttl_cutoff,
        external_grace_cutoff,
    ) {
        tracing::warn!(host = %host_alias, error = %e, "[reconcile] bg cleanup failed");
        s.ensure_in_tx()?;
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
    let fetch_agents = agents_due(deps, &host.alias, std::time::Instant::now());
    probe_with_timeout(
        host,
        tmux,
        deps.probe_timeout,
        Some((deps.shell.as_ref(), deps.pr_cache.as_ref(), &paths)),
        fetch_agents,
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
    // Stamped BEFORE the script runs: a transport failure, or the caller's
    // PR_PROBE_TIMEOUT dropping this future, must not retry the same batch on
    // every 20 s pass, nor keep the sessions past the first PR_PROBE_BATCH
    // from ever being due (review r16).
    cache.mark_probed(host, due.iter().map(|(n, _)| n.as_str()));
    let script = build_pr_probe_script(&due);
    let stdout = match shell.run_script(host, &script).await {
        Ok(out) => out,
        Err(e) => {
            // Best-effort and retried after the cache's TTL: debug, not warn.
            tracing::debug!(host = %host, error = %e, "[reconcile] pr probe failed");
            return PrInfoMap::new();
        }
    };
    match parse_pr_probe_output(&stdout) {
        ProbeOutput::NoGh | ProbeOutput::NoAuth => {
            cache.mark_no_gh(host);
            PrInfoMap::new()
        }
        // Every target the script ran for was throttled above, observed or not.
        ProbeOutput::Results(map) => map,
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
    fetch_agents: bool,
) -> HostProbe {
    // Recorded BEFORE the first await: this is the instant the probe's view of
    // the host stops being current (BE-3 ghost guard).
    let started_at = now_unix();
    let probe = async {
        // Boot identity, the session list, the oauth account and every live
        // session's pane tail, in ONE round trip (`RemoteTmux` batches them
        // into a single delimited script; the default composition on other
        // executors still runs them sequentially). Identity is read BEFORE
        // the list on the wire: a tmux server that dies between the two
        // reads then shows up as sessions missing from the list (the next
        // pass's verdict catches it) rather than as a verdict whose `keep`
        // still names the sessions it just lost. Only trustworthy when we
        // actually reached the host this pass, so it is discarded below
        // when the list fails.
        // The versions section rides the same script, but only on a pass
        // that is due for it (task 1): `claude --version` is a node start.
        let want_versions = versions_due(host.claude_version_at, started_at);
        let snap = tmux.probe_snapshot(PANE_TAIL_LINES, want_versions).await;
        let tmux_result = snap.sessions;
        let identity = if tmux_result.is_ok() {
            snap.identity
        } else {
            None
        };
        let versions = if tmux_result.is_ok() {
            snap.versions
        } else {
            None
        };
        let health = if tmux_result.is_ok() {
            snap.health
        } else {
            None
        };
        // Which account the host is logged into NOW — so a `claude /login`
        // as someone else on a remote host relinks it within one pass
        // instead of waiting for a manual Re-probe. Skipped when the list
        // failed: the host is about to be marked unreachable and the read
        // would only be one more round trip into a dead ssh.
        let account = if tmux_result.is_ok() {
            snap.account
        } else {
            None
        };
        let profiles = if tmux_result.is_ok() {
            snap.profiles
        } else {
            None
        };
        // One pane-tail read per live session, parsed into reconcile intel.
        let intel = intel_from_tails(&snap.pane_tails);
        let pane_tails = snap.pane_tails;
        // `None`: not due this pass (cadence) or the host could not be
        // asked. Either way the bg pruner below must not run this pass —
        // treating "not asked" as "no agents" is exactly what ghosts every
        // background row.
        let agent_rows = if fetch_agents && tmux_result.is_ok() {
            tmux.list_claude_agents().await
        } else {
            None
        };
        // Transcript mtimes feed the inactive-bg-agent rule; one host call,
        // only when this pass saw a background agent.
        let agent_mtimes = match &agent_rows {
            None => None,
            Some(rows) => {
                let bg_ids: Vec<String> = rows
                    .iter()
                    .filter(|a| a.kind == crate::claude_agents::AgentKind::Background)
                    .filter_map(|a| a.session_id.clone())
                    .collect();
                if bg_ids.is_empty() {
                    Some(std::collections::HashMap::new())
                } else {
                    tmux.transcript_mtimes(&bg_ids).await
                }
            }
        };
        (
            tmux_result,
            agent_rows,
            agent_mtimes,
            (intel, pane_tails),
            (account, profiles),
            identity,
            versions,
            health,
        )
    };
    let mut probe = match tokio::time::timeout(timeout, probe).await {
        Ok((
            result,
            agent_rows,
            agent_mtimes,
            (intel, pane_tails),
            (account, profiles),
            identity,
            versions,
            health,
        )) => HostProbe {
            host,
            versions,
            health,
            result,
            agent_rows,
            agent_mtimes,
            intel,
            pane_tails,
            pr_info: PrInfoMap::new(),
            account,
            profiles,
            identity,
            started_at,
            worktree_kb: None,
            codex_rollouts: HashMap::new(),
        },
        Err(_elapsed) => {
            tracing::warn!(
                host = %host.alias,
                timeout = ?timeout,
                "[reconcile] host probe exceeded its wall clock; marking unreachable (last-known sessions kept)"
            );
            return HostProbe {
                host,
                versions: None,
                health: None,
                result: Err(IpcError::new(codes::E_TIMEOUT, "host probe timed out")),
                agent_rows: None,
                agent_mtimes: None,
                intel: PaneIntelMap::new(),
                pane_tails: Default::default(),
                pr_info: PrInfoMap::new(),
                account: None,
                profiles: None,
                identity: None,
                started_at,
                worktree_kb: None,
                codex_rollouts: HashMap::new(),
            };
        }
    };
    // Latency (Orbit Fleet 4.6): one empty command, timed, once the host
    // has answered. Its own bound, like the PR probe below, so a slow
    // round trip never costs the host its "reachable" verdict.
    if probe.result.is_ok() {
        if let Some(h) = probe.health.as_mut() {
            h.latency_ms = tokio::time::timeout(ROUND_TRIP_TIMEOUT, tmux.round_trip_ms())
                .await
                .ok()
                .flatten();
        }
    }
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
    // Worktree size (Orbit Fleet 4.6): due once per interval, bounded on
    // its own, best-effort.
    if let (Ok(_), Some((shell, _, paths))) = (&probe.result, pr_probe) {
        // Also due when this process has no per-worktree sizes for the host
        // yet (G1.9): they live in memory only, so a restart measures once.
        if worktree_size_due(probe.host.worktree_at, started_at)
            || !super::worktree_sizes::known(&probe.host.alias)
        {
            let names: Vec<&str> = paths.named.iter().map(|(p, _)| p.as_str()).collect();
            let mut per_path = Vec::new();
            probe.worktree_kb = Some(match worktree_size_script(&names) {
                None => Some(0),
                Some(script) => tokio::time::timeout(
                    WORKTREE_SIZE_TIMEOUT,
                    shell.run_script(&probe.host.alias, &script),
                )
                .await
                .ok()
                .and_then(Result::ok)
                .and_then(|out| {
                    per_path = parse_worktree_sizes(&out, names.len())
                        .into_iter()
                        .map(|(i, kb)| (names[i].to_string(), kb))
                        .collect();
                    parse_worktree_size(&out)
                }),
            });
            super::worktree_sizes::record(&probe.host.alias, started_at, per_path);
        }
    }
    // Codex conversations: Codex names its own, so each live Codex pane's
    // id and rollout are looked up on the host, bounded on their own like
    // the steps above. A failure finds nothing and the stored ids stay.
    if let (Ok(live), Some((shell, _, paths))) = (&probe.result, pr_probe) {
        let names: Vec<&str> = paths
            .codex_panes
            .iter()
            .filter(|n| live.iter().any(|s| &s.name == *n))
            .map(String::as_str)
            .collect();
        if let Some(script) = crate::agent_adapter::codex::rollouts_script(&names) {
            if let Ok(Ok(out)) = tokio::time::timeout(
                CODEX_ROLLOUT_TIMEOUT,
                shell.run_script(&probe.host.alias, &script),
            )
            .await
            {
                probe.codex_rollouts = crate::agent_adapter::codex::parse_rollouts(&out);
            }
        }
    }
    probe
}

/// Bound on the Codex rollout lookup of one host.
const CODEX_ROLLOUT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Full fleet pass: probe every non-hidden host in parallel and apply each
/// host's result as its probe completes, under its own short store-lock
/// window — a slow host delays only its own rows. Callers are expected
/// to hold a `ReconcilePass` from the shared gate (see `run_full_reconcile`);
/// this function does not take the gate itself so tests can drive it directly.
pub(crate) async fn reconcile_sessions_with(
    store: &Mutex<Store>,
    deps: &Arc<ReconcileDeps>,
) -> Result<(), IpcError> {
    // 0. Ensure the `local` row exists (idempotent; step 1 does this again
    //    but `sync_local_account` needs the row to already be there — on the
    //    very first pass of a fresh install there is no `local` row yet, and
    //    `set_host_account` is a no-op UPDATE against a row that doesn't
    //    exist). Skipped entirely when `local_host` is false (a headless
    //    `fleet-hub` daemon): no row is created and the local Claude account
    //    is never read, even if a `state.db` copied from a desktop already
    //    carries a `local` row.
    if deps.local_host {
        {
            let s = lock(store)?;
            s.upsert_host("local")?;
        }
        // Sync the local Claude account every pass — not just once — so an
        // account switch (logout + login as someone else) is picked up, not
        // just a first-time link (see `ReconcileDeps::local_home` and
        // `service::hosts::sync_local_account`: `local`'s account has no
        // other automatic discovery path). Best-effort: a probe hiccup here
        // must not abort session reconcile.
        if let Some(home) = deps.local_home.clone() {
            if let Err(e) = crate::service::hosts::sync_local_account(store, home).await {
                tracing::warn!(error = %e.message, "[reconcile] local account probe failed");
            }
        }
    }

    // 1. Snapshot under lock (brief). Ensure local host exists first (unless
    //    this hub opted local out — see step 0); a pre-existing `local` row
    //    is filtered out of the snapshot so it is never fanned out to probe.
    let hosts = {
        let s = lock(store)?;
        if deps.local_host {
            s.upsert_host("local")?;
        }
        // One hidden/local rule for every host loop (hub-ops F6,
        // `hosts::is_active`): hidden rows never enter the snapshot, their
        // sessions stay frozen at their last-known state; a disabled
        // `local` is reaped by `reap_unprobed_local` (step 4).
        crate::service::hosts::active_hosts(s.list_hosts()?, deps.local_host)
            .into_iter()
            .map(|h| {
                let paths = HostPaths::for_host(&s, &h.alias);
                (h, paths)
            })
            .collect::<Vec<_>>()
    };

    // 2. Fan out probes (off-lock) via JoinSet for parallel execution.
    //    Hidden hosts never reach here (see the snapshot above) — their
    //    last-known sessions are still surfaced by the final
    //    `list_all_sessions` read, without probing.
    //    Each task receives owned data so it satisfies 'static + Send.
    //
    //    Each probe is bounded by `deps.probe_timeout` (see `probe_one_host`):
    //    a single wedged host can no longer stall the collector below and, with
    //    it, the whole load path.
    //
    //    `JoinSet::drop` aborts the futures but does NOT kill spawned ssh
    //    children by itself; the ssh layer's own wall clock
    //    (`SshClient::run_child`) kills and reaps them and resets the master.
    //    The project list is identical for every host — fetch it once here
    //    rather than re-querying inside `find_project_id_for_path` per session.
    let projects = {
        let s = lock(store)?;
        s.list_projects()?
    };
    let mut set = tokio::task::JoinSet::new();
    // Hidden rows never enter the snapshot (`active_hosts`, step 1).
    for (host, paths) in hosts {
        let deps = Arc::clone(deps);
        set.spawn(async move { probe_one_host(host, paths, &deps).await });
    }

    // 3. Apply each host's result AS ITS PROBE COMPLETES, taking the store
    //    lock once per host (BE-12), rather than after every host has
    //    joined: `HOST_PROBE_TIMEOUT` (65 s) is far past the 20 s tick, and
    //    one wedged host used to hold every other host's rows back for that
    //    long (perf-logs §3). The write is unchanged — each host's whole
    //    write (account link, mass-loss marks, boot identity, the
    //    `apply_host_reconcile_in_tx` burst, PR signals, timeline events,
    //    conversation rebinds and bg rows) runs in `reconcile_write_one_host`
    //    under ONE `Store::atomically` transaction, events held until it
    //    commits, a failed host rolled back alone. Join errors (task panics)
    //    are logged and skipped — they don't abort the rest of reconcile.
    while let Some(join) = set.join_next().await {
        let probe = match join {
            Ok(probe) => probe,
            Err(e) => {
                tracing::error!(error = %e, "[reconcile] probe task panicked");
                continue;
            }
        };
        {
            let mut s = lock(store)?;
            // Per-host isolation: one host's DB write failure (e.g. an FK
            // violation on a stale account_uuid) must NOT abort reconcile
            // for every other host. The host's whole write is one
            // transaction, so a failed host rolls back cleanly; we log it
            // and carry on.
            if let Err(e) = reconcile_write_one_host(&mut s, &probe, &projects) {
                tracing::error!(
                    host = %probe.host.alias,
                    error = %e,
                    "[reconcile] write failed (rolled back; retried next pass)"
                );
            }
        }
        // The guard is dropped above; now give a waiting reader its turn
        // (Task 4). `std::sync::Mutex` is not fair: re-locking straight away
        // for the next host usually wins against a reader already blocked
        // on it, so a reader could wait out the whole write phase. Yielding
        // lets the runtime schedule that reader's task before this one
        // comes back for the lock.
        tokio::task::yield_now().await;
    }

    // 4. `local` on a hub without a local host (`hub.local_host=false`) is
    //    the one host nothing will ever probe, yet it can still hold rows (a
    //    copied desktop store). Reap them here so they are not immortal
    //    (data-sync F2/F5). A user-HIDDEN host is left alone: Hide is
    //    reversible (Undo, Unhide), so its rows stay frozen at their
    //    last-known state — names, work links, timeline and resumable
    //    `claude_session_id`s intact — until the host is shown again.
    reap_unprobed_local(store, deps, now_unix());
    Ok(())
}

/// Step 4 of [`reconcile_sessions_with`]: ghost, then (next pass) delete the
/// rows of `local` when this process has no local host. That is exactly the
/// `local` case of [`crate::service::hosts::is_active`] — hidden hosts, the
/// other inactive case, keep their rows. Best-effort: a failure is logged
/// and the next pass tries again.
pub(super) fn reap_unprobed_local(store: &Mutex<Store>, deps: &ReconcileDeps, now: i64) {
    use crate::service::projects::LOCAL_HOST;
    if deps.local_host {
        return;
    }
    let Ok(s) = lock(store) else { return };
    match s.reap_host_ghosts(LOCAL_HOST, now) {
        Ok(n) if n > 0 => tracing::info!(
            host = LOCAL_HOST,
            reaped = n,
            "[reconcile] reaped rows of the disabled local host"
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!(
            host = LOCAL_HOST,
            error = %e,
            "[reconcile] reap of the disabled local host failed"
        ),
    }
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

/// A pass of this caller's own: waits for one already running, then runs.
/// A forced listing needs it: the running pass may have probed its hosts
/// before the session the caller just started existed, so its rows cannot
/// answer "what is there now" (review r06, from r17 F4).
async fn run_own_reconcile(
    store: &Mutex<Store>,
    deps: &Arc<ReconcileDeps>,
    gate: &ReconcileGate,
) -> Result<(), IpcError> {
    let pass = gate.begin().await;
    reconcile_sessions_with(store, deps).await?;
    pass.complete();
    Ok(())
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
///
/// `force` always awaits a pass of its own inline (an explicit user refresh
/// must show its own result), after any pass already running. Otherwise: a COLD start — no pass has completed in this
/// process yet (`gate.has_completed_once()` false) — also awaits one inline,
/// so the very first listing is not empty. Once at least one pass has
/// completed, a stale gate no longer probes inline: the stored rows are
/// served at once and a background pass is started through the SAME gate
/// (so it can never overlap one already running), catching the fleet up for
/// the next read instead of making this caller pay for it.
pub(super) async fn list_sessions_with(
    store: &Arc<Mutex<Store>>,
    deps: &Arc<ReconcileDeps>,
    gate: &Arc<ReconcileGate>,
    window: std::time::Duration,
    force: bool,
) -> Result<Vec<SessionRow>, IpcError> {
    list_sessions_with_reader(store, store, deps, gate, window, force).await
}

/// [`list_sessions_with`], with the final read of stored rows through
/// `reader` (the hub's read pool) whenever this call ran no pass inline. A
/// call that did run one — `force`, or a cold start — reads its rows back
/// through the writer that just wrote them.
pub(super) async fn list_sessions_with_reader(
    store: &Arc<Mutex<Store>>,
    reader: &Mutex<Store>,
    deps: &Arc<ReconcileDeps>,
    gate: &Arc<ReconcileGate>,
    window: std::time::Duration,
    force: bool,
) -> Result<Vec<SessionRow>, IpcError> {
    let mut reader = reader;
    if force {
        run_own_reconcile(store, deps, gate).await?;
        reader = &**store;
    } else if !gate.has_completed_once() {
        // Cold start: keep today's inline pass so the first listing isn't
        // empty. `Ok(false)` here (another caller won the gate concurrently)
        // just falls through to the stored rows, same as before.
        run_full_reconcile(store, deps, gate).await?;
        reader = &**store;
    } else if !gate.is_fresh(window) {
        // A pass has completed before and the gate is stale: serve the
        // stored rows now and catch up detached. `try_begin` inside
        // `run_full_reconcile` is what actually prevents overlap with a
        // pass already running (e.g. the background tick, or another
        // caller's own spawned catch-up) — this spawn is just one more
        // racer for that single slot.
        let store = Arc::clone(store);
        let deps = Arc::clone(deps);
        let gate = Arc::clone(gate);
        crate::rt::spawn(async move {
            if let Err(e) = run_full_reconcile(&store, &deps, &gate).await {
                tracing::warn!(error = %e, "[reconcile] background catch-up pass failed");
            }
        });
    }
    let s = lock(reader)?;
    s.list_all_sessions().map_err(IpcError::from)
}

pub(super) async fn reconcile_one_host_with(
    store: &Mutex<Store>,
    deps: &ReconcileDeps,
    alias: &str,
) -> Result<(), IpcError> {
    // `local` is off on this hub (`hub.local_host = false`): refuse before
    // any lookup or probe, even if a `state.db` copied from a desktop still
    // carries a `local` row (the same row the fleet-wide pass never probes).
    crate::service::hub::check_local_allowed(alias, deps.local_host)?;
    // 1. Snapshot the host under lock (brief).
    let (host, paths) = {
        let s = lock(store)?;
        let host = s
            .list_hosts()?
            .into_iter()
            .find(|h| h.alias == alias)
            .ok_or_else(|| IpcError::new(codes::E_NOTFOUND, format!("host {alias} not found")))?;
        let paths = HostPaths::for_host(&s, alias);
        (host, paths)
    };

    // 2. Probe off-lock, under the same hard cap as the multi-host reconcile.
    let probe = probe_one_host(host, paths, deps).await;

    // 3. Apply writes under one brief lock, via the SAME per-host write path
    //    as the multi-host reconcile (single transaction + emit-after-commit).
    let mut s = lock(store)?;
    let projects = s.list_projects()?;
    reconcile_write_one_host(&mut s, &probe, &projects)
}

/// Test access to the private single-host reconcile entry point, so
/// `reconcile_tests` (outside the `sessions` module) can exercise it exactly
/// like `run_full_reconcile_for_test`.
#[cfg(test)]
pub(crate) async fn reconcile_one_host_with_for_test(
    store: &Mutex<Store>,
    deps: &ReconcileDeps,
    alias: &str,
) -> Result<(), IpcError> {
    reconcile_one_host_with(store, deps, alias).await
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
    reconcile_one_host_with(store, &ReconcileDeps::real(ssh, local_host(store)), alias).await
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
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
) -> Result<Vec<SessionRow>, IpcError> {
    list_sessions_reading(store, store, ssh).await
}

/// [`list_sessions`] reading the settings it needs, and the stored rows it
/// serves, through `reader` — the hub's read pool, so a listing served from
/// the store never waits on the writer. A pass it has to run inline (a cold
/// start) still writes through `store`, and its rows are then read back
/// through `store` too.
pub async fn list_sessions_reading(
    store: &Arc<Mutex<Store>>,
    reader: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<Vec<SessionRow>, IpcError> {
    let window = list_freshness_window(reader);
    list_sessions_with_reader(
        store,
        reader,
        &ReconcileDeps::real(ssh, local_host(reader)),
        &reconcile_gate(),
        window,
        false,
    )
    .await
}

/// `list_sessions` for an explicit user refresh: ignores the freshness window
/// and runs a pass of its own, after waiting for one already in flight (whose
/// probes may predate a session just started). Never overlaps that pass.
pub async fn refresh_sessions(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
) -> Result<Vec<SessionRow>, IpcError> {
    let window = list_freshness_window(store);
    list_sessions_with(
        store,
        &ReconcileDeps::real(ssh, local_host(store)),
        &reconcile_gate(),
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
    run_full_reconcile(
        store,
        &ReconcileDeps::real(ssh, local_host(store)),
        &reconcile_gate(),
    )
    .await
}

/// `hub.local_host` for this pass; a poisoned lock counts as "true" (the
/// desktop default) so reconcile keeps its old behaviour on error. Off, too,
/// whenever the process has no local host at all
/// ([`crate::service::hub::local_host_enabled`]): a Windows desktop never
/// writes the setting, and reading only the setting re-marked its hidden
/// `local` row reachable and linked the Windows Claude account to it every
/// pass.
pub(super) fn local_host(store: &Mutex<Store>) -> bool {
    crate::service::hub::local_host_enabled()
        && store
            .lock()
            .map(|s| crate::service::hub::read_local_host(&s))
            .unwrap_or(true)
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

/// The `claude_status` candidate for one live session this pass.
///
/// `agents_asked` is `probe.agent_rows.is_some()` — whether `claude agents
/// --json` was actually asked this pass (see `agents_due`): a cadence-
/// skipped host or an unanswerable call both leave `agent_status` at
/// `None` before this function is even reached.
///
/// When agents WERE asked: the authoritative agent status wins, falling
/// back to the pane heuristic when absent — this is the pre-cadence
/// behaviour, unchanged.
///
/// When agents were NOT asked: `agent_status` is always `None` (no
/// pairing ran), so falling back to the pane heuristic unconditionally
/// would overwrite a stored authoritative/hook-stamped status with a weak
/// pane guess on every skipped pass — at the 60 s agents / 20 s reconcile
/// cadence that is 2 of every 3 passes, producing status flicker and
/// spurious `status_change` events. Only `Blocked` (a real dialog or
/// stuck pane) is strong enough pane evidence to surface without waiting
/// for the next agents pass; any other pane verdict yields `None` so the
/// upsert's `COALESCE(excluded.claude_status, claude_status)` keeps
/// whatever is stored (a hook-stamped `idle`/`working` survives between
/// agent fetches).
///
/// Two exceptions, both about a dialog, the one thing the pane sees better
/// than anyone:
///  - a pane showing a dialog (`Blocked`) wins even over `claude agents`:
///    the CLI can report `working` while a permission dialog is up, and
///    letting it win flipped the row every agents pass (and re-alerted a
///    phone each time it came back);
///  - a stored `blocked` (`stored_blocked`) whose pane now shows a turn or
///    the input box is over: a dialog answered in the terminal fires no
///    hook, so without this `blocked` (and its "needs you") lasted until the
///    next agents pass or the Stop hook.
pub(super) fn status_candidate(
    agents_asked: bool,
    agent_status: Option<crate::service::pane_intel::ClaudeStatus>,
    pane: Option<crate::service::pane_intel::ClaudeStatus>,
    stored_blocked: bool,
) -> Option<crate::service::pane_intel::ClaudeStatus> {
    use crate::service::pane_intel::ClaudeStatus;
    if pane == Some(ClaudeStatus::Blocked) {
        return pane;
    }
    if agents_asked {
        return agent_status.or(pane);
    }
    match pane {
        Some(ClaudeStatus::Working | ClaudeStatus::Idle) if stored_blocked => pane,
        _ => None,
    }
}

/// A row the tick demoted for staleness (`stale_working_at` or, once an
/// attach or the TTL has ended the attention reason, `stale_demoted_at`
/// set) is not handed back to `working` by the cached `claude agents`
/// status alone.
/// Only the pane's own spinner (`Working`) lifts the demotion; a `Blocked`
/// pane still surfaces; anything else yields `None` so the upsert keeps
/// the stored `idle`.
pub(super) fn stale_working_veto(
    stale: bool,
    candidate: Option<crate::service::pane_intel::ClaudeStatus>,
    pane: Option<crate::service::pane_intel::ClaudeStatus>,
) -> Option<crate::service::pane_intel::ClaudeStatus> {
    use crate::service::pane_intel::ClaudeStatus;
    if !stale {
        return candidate;
    }
    match (candidate, pane) {
        (Some(ClaudeStatus::Working), Some(ClaudeStatus::Working)) => Some(ClaudeStatus::Working),
        (Some(ClaudeStatus::Working), Some(ClaudeStatus::Blocked)) => Some(ClaudeStatus::Blocked),
        (Some(ClaudeStatus::Working), _) => None,
        (other, _) => other,
    }
}

/// The tick's stale-`working` sweep: reads `reconcile.stale_working_secs`
/// and demotes every qualifying row (`Store::age_out_stale_working`) —
/// only rows a reconcile pass observed within that window, so a skipped
/// or failed pass, or an unreachable host, demotes nothing. Best-effort;
/// returns how many rows were demoted.
pub fn age_out_stale_working(store: &Mutex<Store>) -> usize {
    let Ok(s) = store.lock() else {
        return 0;
    };
    let secs = crate::service::settings::get_secs(
        &s,
        crate::service::settings::RECONCILE_STALE_WORKING_SECS,
    ) as i64;
    match s.age_out_stale_working(now_unix(), secs) {
        Ok(rows) => {
            for r in &rows {
                tracing::info!(
                    host = %r.host_alias,
                    session = %r.tmux_name,
                    "[reconcile] stale working demoted to idle"
                );
            }
            rows.len()
        }
        Err(e) => {
            tracing::warn!(error = %e, "[reconcile] stale-working sweep failed");
            0
        }
    }
}

/// The tick's companion to [`age_out_stale_working`]: reads
/// `reconcile.stale_working_ttl_secs` and lifts every stamp that no longer
/// asks anything (`Store::expire_stale_working`). Best-effort; returns how
/// many rows were lifted.
pub fn expire_stale_working(store: &Mutex<Store>) -> usize {
    let Ok(s) = store.lock() else {
        return 0;
    };
    let ttl = crate::service::settings::get_secs(
        &s,
        crate::service::settings::RECONCILE_STALE_WORKING_TTL_SECS,
    ) as i64;
    match s.expire_stale_working(now_unix(), ttl) {
        Ok(rows) => rows.len(),
        Err(e) => {
            tracing::warn!(error = %e, "[reconcile] stale-working expiry failed");
            0
        }
    }
}
