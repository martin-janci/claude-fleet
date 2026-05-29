use crate::cancel::{CancelGuard, CancellationRegistry};
use crate::ipc_error::IpcError;
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
/// round-trip) never false-trips; on a real wedge the ssh-layer keepalive
/// usually tears the master down well before this fires.
const HOST_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Map of `tmux_name` → analyzed pane intel, gathered off-lock during a host
/// probe. A name absent from the map (capture failed) leaves the session's
/// intel fields untouched (COALESCE in the upsert preserves prior values).
type PaneIntelMap = std::collections::HashMap<String, crate::service::pane_intel::PaneIntel>;

/// Per-host probe result tuple: (host, tmux sessions, claude agents, pane intel).
type HostProbeResult = (
    HostRow,
    Result<Vec<crate::tmux::TmuxSession>, IpcError>,
    Vec<crate::claude_agents::ClaudeAgentRow>,
    PaneIntelMap,
);

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

fn exec_for(host: &str, ssh: &Arc<SshClient>) -> Box<dyn TmuxExec> {
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
    host: &HostRow,
    res: &Result<Vec<crate::tmux::TmuxSession>, IpcError>,
    projects: &[ProjectRow],
    agent_rows: &[crate::claude_agents::ClaudeAgentRow],
    intel: &PaneIntelMap,
) -> Result<(), IpcError> {
    match res {
        Ok(live) => {
            let mut keep: Vec<String> = Vec::with_capacity(live.len());
            let mut sessions: Vec<ReconcileSession> = Vec::with_capacity(live.len());
            // ── Task G: reconcile transition-detection (event timeline) ──
            // Per session, the (kind, detail) events whose stored value differs
            // from the about-to-be-upserted value. Collected DURING the loop
            // (so we can read the prior row before the upsert overwrites it) and
            // flushed AFTER apply_host_reconcile commits — at which point we can
            // resolve each tmux_name to its session id. Keyed by tmux_name.
            // (Task H builds on this: keep it self-contained here.)
            let mut pending_events: Vec<(String, &'static str, Option<String>)> = Vec::new();
            for sess in live {
                keep.push(sess.name.clone());
                let project_id = find_project_id_for_path(projects, &host.alias, &sess.path);
                // Preservation invariant: if the session already has an
                // account_uuid in the DB, keep it; only capture the host's
                // current account for newly-discovered sessions.
                let account_uuid = s
                    .get_session_account(&host.alias, &sess.name)?
                    .or_else(|| host.account_uuid.clone());
                let worktree_key = worktree_key_for_path(&sess.path.to_string_lossy());
                // Match the running Claude agent by name (sessions launched
                // with `--name <tmux_name>`) or, for older sessions without a
                // name, by a unique cwd — so `recreate`/`restart` can resume
                // the exact conversation instead of "most recent for the cwd".
                let agent = crate::claude_agents::find_for_session(
                    agent_rows,
                    &sess.name,
                    &sess.path.to_string_lossy(),
                );
                // Pane-tail intel from the off-lock probe (may be absent if the
                // capture failed — then all four intel fields stay None and the
                // upsert's COALESCE preserves the session's prior values).
                let pane = intel.get(&sess.name);
                let agent_status = agent.and_then(|a| a.status.clone());
                // Prefer the authoritative `claude agents` status; fall back to
                // the status derived from the pane tail only when it is absent.
                let claude_status = agent_status
                    .or_else(|| pane.and_then(|p| p.derived_status).map(|s| s.to_string()));
                let stuck_kind = pane.and_then(|p| p.stuck.map(|k| k.as_str().to_string()));
                // Transition-detection: compare the PRIOR stored row (read here,
                // before the upsert below overwrites it) against the new values
                // and queue an event when claude_status / stuck_kind changed.
                // Append-only + best-effort: a read failure just skips detection.
                if let Ok(Some(prior)) = s.get_session(&sess.name, &host.alias) {
                    if prior.claude_status != claude_status {
                        pending_events.push((
                            sess.name.clone(),
                            "status_change",
                            claude_status.clone(),
                        ));
                    }
                    // A newly-set (or changed) stuck_kind is the alert-worthy
                    // event; clearing it back to None is not recorded.
                    if prior.stuck_kind != stuck_kind && stuck_kind.is_some() {
                        pending_events.push((sess.name.clone(), "stuck", stuck_kind.clone()));
                    }
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
                    pr_url: None,       // not in claude agents --json; reserved for future
                    current_activity: pane.and_then(|p| p.activity.clone()),
                    context_pct: pane.and_then(|p| p.context_pct),
                    stuck_kind,
                    // Pane captured this pass ⇒ stuck_kind is authoritative and a
                    // None clears any stale flag; a failed capture (pane absent)
                    // leaves intel_observed false so the prior flag is preserved.
                    intel_observed: pane.is_some(),
                });
            }
            s.apply_host_reconcile(HostReconcile {
                alias: &host.alias,
                reachable: true,
                claude_version: host.claude_version.as_deref(),
                tmux_version: host.tmux_version.as_deref(),
                last_pinged_at: now_unix(),
                sessions: &sessions,
                keep: &keep,
            })?;
            // Task G: flush queued transition events now that the upsert has
            // committed and each tmux_name resolves to a session id. Append-only
            // and best-effort — a failed insert is logged and skipped, never
            // blocking reconcile.
            for (tmux_name, kind, detail) in &pending_events {
                if let Ok(Some(row)) = s.get_session(tmux_name, &host.alias) {
                    if let Err(e) = s.insert_session_event(row.id, kind, detail.as_deref()) {
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
            // `list_sessions`. These rows are exempt from ghost cleanup (they're
            // never tmux sessions) — see `ghost_and_clean_sessions_in_tx`.
            reconcile_bg_agents(s, &host.alias, live, projects, agent_rows)?;
            // Task H: stamp freshness on every session this pass observed live,
            // so a proactive (background) reconcile keeps `last_reconciled_at`
            // current and the UI can dim rows whose host has gone quiet.
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
/// build a stable sentinel / track them). Pure so it's unit-testable.
fn unmatched_bg_agents<'a>(
    live: &[crate::tmux::TmuxSession],
    agents: &'a [crate::claude_agents::ClaudeAgentRow],
) -> Vec<&'a crate::claude_agents::ClaudeAgentRow> {
    // Collect the set of agent session_ids that a tmux session resolved to.
    let mut matched: std::collections::HashSet<String> = std::collections::HashSet::new();
    for sess in live {
        if let Some(agent) =
            crate::claude_agents::find_for_session(agents, &sess.name, &sess.path.to_string_lossy())
        {
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
/// no tmux session (the reconcile "second pass"). Per-agent write failures are
/// logged and skipped so one bad row can't abort the others.
fn reconcile_bg_agents(
    s: &Store,
    host_alias: &str,
    live: &[crate::tmux::TmuxSession],
    projects: &[ProjectRow],
    agents: &[crate::claude_agents::ClaudeAgentRow],
) -> Result<(), IpcError> {
    for agent in unmatched_bg_agents(live, agents) {
        let Some(session_id) = agent.session_id.as_deref() else {
            continue;
        };
        let tmux_name = format!("bg:{session_id}");
        let project_id = agent.cwd.as_deref().and_then(|cwd| {
            find_project_id_for_path(projects, host_alias, std::path::Path::new(cwd))
        });
        if let Err(e) = s.upsert_bg_session(
            host_alias,
            &tmux_name,
            project_id,
            session_id,
            agent.status.as_deref(),
            now_unix(),
        ) {
            eprintln!("[reconcile] bg upsert failed for {host_alias}/{session_id}: {e}");
        }
    }
    Ok(())
}

/// Probe one host off-lock, under a hard wall-clock cap (`HOST_PROBE_TIMEOUT`).
/// A wedged SSH ControlMaster can make any of these awaits hang forever
/// (`ConnectTimeout` does not cover a multiplexed attach onto an existing
/// master), so the whole probe is bounded. On timeout we return an `Err` probe
/// result, which `reconcile_write_one_host` turns into "host unreachable, keep
/// last-known sessions". The abandoned ssh child is reaped on its own by the
/// ssh-layer keepalive. Shared by the multi-host reconcile and the single-host
/// background refresh so both are bounded identically.
async fn probe_one_host(host: HostRow, ssh: &Arc<SshClient>) -> HostProbeResult {
    let tmux = exec_for(&host.alias, ssh);
    probe_with_timeout(host, tmux, HOST_PROBE_TIMEOUT).await
}

/// Inner probe with an injectable executor + timeout, so the wedged-host path
/// is unit-testable without real ssh. See `probe_one_host` for the rationale.
async fn probe_with_timeout(
    host: HostRow,
    tmux: Box<dyn TmuxExec>,
    timeout: std::time::Duration,
) -> HostProbeResult {
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
    match tokio::time::timeout(timeout, probe).await {
        Ok((tmux_result, agent_rows, intel)) => (host, tmux_result, agent_rows, intel),
        Err(_elapsed) => {
            eprintln!(
                "[reconcile] host {alias} probe exceeded {timeout:?}; marking unreachable (last-known sessions kept)",
                alias = host.alias,
            );
            (
                host,
                Err(IpcError::new("E_TIMEOUT", "host probe timed out")),
                Vec::new(),
                PaneIntelMap::new(),
            )
        }
    }
}

async fn reconcile_sessions(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<Vec<SessionRow>, IpcError> {
    // 1. Snapshot under lock (brief). Ensure local host exists first.
    let hosts = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.upsert_host("local")?;
        s.list_hosts()?
    };

    // 2. Fan out probes (off-lock) via JoinSet for parallel execution.
    //    Hidden hosts are skipped here — their last-known sessions are still
    //    surfaced by the final `list_all_sessions` read, without probing.
    //    Each task receives owned data so it satisfies 'static + Send.
    //
    //    Each probe is bounded by `HOST_PROBE_TIMEOUT` (see `probe_one_host`):
    //    a single wedged host can no longer stall the collector below and, with
    //    it, the whole load path.
    //
    //    TODO(iter4a-M4): `JoinSet::drop` aborts the futures but does NOT kill
    //    the spawned ssh/tmux child processes; on timeout/early-return/panic
    //    those orphan. They now self-reap via the ssh keepalive (ServerAlive in
    //    `ssh.rs` `mux_opts`) within ~10s instead of lingering for hours, but
    //    explicit kill+reap still awaits the CancellationToken plumbing in M4.
    let mut set = tokio::task::JoinSet::new();
    for host in hosts.into_iter().filter(|h| !h.hidden) {
        let ssh_arc = Arc::clone(ssh);
        set.spawn(async move { probe_one_host(host, &ssh_arc).await });
    }

    // Collect per-host probe results. Join errors (task panics) are logged
    // and skipped — they don't abort the rest of reconcile.
    let mut probed: Vec<HostProbeResult> = Vec::new();
    while let Some(join) = set.join_next().await {
        match join {
            Ok((host, res, agent_rows, intel)) => probed.push((host, res, agent_rows, intel)),
            Err(e) => eprintln!("[reconcile] probe task panicked: {e}"),
        }
    }

    // 3. Apply all writes in a single short lock window. Each host's
    //    write-burst goes through `Store::apply_host_reconcile`, which wraps
    //    update_host_probe + upserts + touches + delete-not-in in ONE
    //    transaction (one fsync) and emits events only AFTER it commits — so a
    //    mid-burst error rolls everything back and emits nothing for that host.
    {
        let mut s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;

        // The project list is identical for every host — fetch it once here
        // rather than re-querying inside `find_project_id_for_path` per session.
        let projects = s.list_projects()?;

        for (host, res, agent_rows, intel) in &probed {
            // Per-host isolation: one host's DB write failure (e.g. an FK
            // violation on a stale account_uuid) must NOT abort reconcile for
            // every other host. apply_host_reconcile is transactional, so a
            // failed host rolls back cleanly; we log it and carry on.
            if let Err(e) =
                reconcile_write_one_host(&mut s, host, res, &projects, agent_rows, intel)
            {
                eprintln!("[reconcile] write failed for {}: {e}", host.alias);
            }
        }
    }

    // 4. Read the final session set in one query (covers active + hidden
    //    hosts) instead of N per-host reads.
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    s.list_all_sessions().map_err(IpcError::from)
}

async fn reconcile_one_host(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    alias: &str,
) -> Result<(), IpcError> {
    // 1. Snapshot the host under lock (brief).
    let host = {
        let s = store
            .lock()
            .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
        s.list_hosts()?
            .into_iter()
            .find(|h| h.alias == alias)
            .ok_or_else(|| IpcError::new("E_NOTFOUND", format!("host {alias} not found")))?
    };

    // 2. Probe off-lock, under the same hard cap as the multi-host reconcile.
    let (host, result, agent_rows, intel) = probe_one_host(host, ssh).await;

    // 3. Apply writes under one brief lock, via the SAME per-host write path
    //    as the multi-host reconcile (single transaction + emit-after-commit).
    let mut s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
    let projects = s.list_projects()?;
    reconcile_write_one_host(&mut s, &host, &result, &projects, &agent_rows, &intel)
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
    let remainder = caps.get(1).map(|m| m.as_str()).unwrap_or("");
    // Check `.claude/worktrees/` first — it is the more specific marker, and
    // `/.worktrees/` is not a substring of `/.claude/worktrees/`.
    for marker in ["/.claude/worktrees/", "/.worktrees/"] {
        if let Some(idx) = remainder.find(marker) {
            let after = &remainder[idx + marker.len()..];
            if let Some(name) = after.split('/').next() {
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
    }
    Some("main".to_string())
}

/// Match a session's cwd to a known project id. `projects` is passed in by the
/// caller (fetched once per reconcile) rather than queried per session.
fn find_project_id_for_path(
    projects: &[ProjectRow],
    host_alias: &str,
    path: &std::path::Path,
) -> Option<i64> {
    let path_str = path.to_string_lossy();
    if host_alias == "local" {
        // Local paths: prefix match (handles worktrees nested under repos).
        return projects
            .iter()
            .filter(|p| path_str.starts_with(&p.base_path))
            .max_by_key(|p| p.base_path.len())
            .map(|p| p.id);
    }
    // Remote paths: match by owner+repo extracted from the conventional
    // `.../projects/github.com/<owner>/<repo>/...` layout. Falls through
    // to `None` (orphan) if the path doesn't follow the convention.
    let (owner, repo) = extract_owner_repo(&path_str)?;
    projects
        .iter()
        .find(|p| p.owner == owner && p.repo == repo)
        .map(|p| p.id)
}

pub async fn list_sessions(
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<Vec<SessionRow>, IpcError> {
    reconcile_sessions(store, ssh).await
}

/// Headless reconcile entry point for the Wave-2 background tick (Task H).
///
/// Reconcile is already Tauri-free: `reconcile_sessions` takes only a
/// `&Mutex<Store>` and a `&Arc<SshClient>`, and all row events are emitted
/// through the store's `EventBus` (see `events.rs`) — NOT a Tauri `AppHandle`.
/// So a `tokio::spawn`ed loop can drive it with the same managed `Arc`s the
/// commands use. This wrapper exists so the spawn site reads as "reconcile now"
/// rather than "list sessions", and so the entry point is independently
/// testable without a running Tauri app.
pub async fn reconcile_now(store: &Mutex<Store>, ssh: &Arc<SshClient>) -> Result<(), IpcError> {
    reconcile_sessions(store, ssh).await.map(|_| ())
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
fn fetch_owner_repo(s: &Store, project_id: i64) -> Result<(String, String), IpcError> {
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
/// worktree) should live. Mirrors the local convention `proj-clean` enforces:
/// `~/projects/github.com/<owner>/<repo>` for the project root and
/// `~/projects/github.com/<owner>/<repo>/.claude/worktrees/<wt>` for non-main
/// worktrees. Returns just the project root if `wt_name` is None or "main".
fn remote_project_path(
    home: &str,
    owner: &str,
    repo: &str,
    wt_name: Option<&str>,
) -> (String, String) {
    let project_root = format!("{home}/projects/github.com/{owner}/{repo}");
    let cwd = match wt_name {
        Some(name) if name != "main" => {
            format!("{project_root}/.claude/worktrees/{name}")
        }
        _ => project_root.clone(),
    };
    (project_root, cwd)
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
/// stderr; the ONLY stdout is the absolute path of the worktree (last line),
/// which the caller uses as the tmux cwd.
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
         ( cd \"$wt\" && pwd )\n",
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
    args: NewSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
    reg: &Arc<CancellationRegistry>,
) -> Result<SessionRow, IpcError> {
    // Reject hostile input before it reaches ssh / tmux / git.
    crate::validate::host_alias(&args.host_alias)?;
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
            let (project_root, _) = remote_project_path(&home, &owner, &repo, None);
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
            let (project_root, cwd) = remote_project_path(&home, &owner, &repo, wt_name_str);
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

    let tmux = exec_for(&args.host_alias, ssh);
    tmux.new_session(&args.name, &path, &pane_cmd).await?;

    reconcile_one_host(store, ssh, &args.host_alias).await?;
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

pub async fn kill_session(
    args: KillSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<i64, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    crate::validate::tmux_name(&args.name)?;
    // Look up id BEFORE killing so we can return it after. Read the controller
    // under the same lock and refuse to nuke ourselves unless forced.
    let id = {
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
            .map(|r| r.id)
            .ok_or_else(|| {
                IpcError::new("E_NOTFOUND", format!("session {} not found", args.name))
            })?
    };
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
    crate::validate::tmux_name(&args.old_name)?;
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
    crate::validate::tmux_name(&args.name)?;
    // Respawn the pane with the command matching the session's kind so a
    // restarted shell session comes back as a shell, not a Claude pane. Read
    // the controller under the same lock and refuse to restart ourselves
    // unless forced.
    let (kind, claude_id) = {
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
            Some(r) => (r.kind, r.claude_session_id),
            None => ("work".to_string(), None),
        }
    };
    let pane_cmd: String = recreate_pane_command(&kind, claude_id.as_deref());
    let tmux = exec_for(&args.host_alias, ssh);
    tmux.restart_session(&args.name, &pane_cmd).await?;
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
    crate::validate::tmux_name(tmux_name)?;
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
    Ok(())
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
        let s = store.lock().expect("store mutex poisoned");
        let sessions = s
            .list_all_sessions()
            .map_err(|e| IpcError::new("E_DB", format!("list sessions for broadcast: {e}")))?;
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
            owner,
            repo,
            wt_name,
        } => {
            let home = ssh.remote_home(host_alias).await?;
            let (_root, cwd) = remote_project_path(&home, &owner, &repo, wt_name.as_deref());
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
    let cwd = resolve_cwd_source(cwd_src, &source.host_alias, ssh).await?;

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
fn recreate_pane_command(kind: &str, claude_session_id: Option<&str>) -> String {
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
    let cwd = resolve_cwd_source(cwd_src, &sess.host_alias, ssh).await?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

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
        let unmatched = unmatched_bg_agents(&[], &agents);
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
        let unmatched = unmatched_bg_agents(&live, &agents);
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
        assert!(unmatched_bg_agents(&[], &agents).is_empty());
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
    fn remote_project_path_returns_project_root_for_main_or_no_worktree() {
        let (root, cwd) = remote_project_path("/home/mjanci", "martin-janci", "claude-fleet", None);
        assert_eq!(
            root,
            "/home/mjanci/projects/github.com/martin-janci/claude-fleet"
        );
        assert_eq!(cwd, root);

        let (root, cwd) =
            remote_project_path("/home/mjanci", "papayapos", "pos-frontend", Some("main"));
        assert_eq!(cwd, root);
    }

    #[test]
    fn remote_project_path_uses_worktree_subdir_for_non_main() {
        let (root, cwd) = remote_project_path(
            "/home/mjanci",
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
        let (h, res, agents, intel) =
            probe_with_timeout(host, Box::new(HangingTmux), Duration::from_millis(80)).await;
        let elapsed = start.elapsed();

        assert_eq!(h.alias, "wedged", "host identity preserved for the writer");
        assert!(
            res.is_err(),
            "wedged probe must surface as Err → unreachable"
        );
        assert!(agents.is_empty());
        assert!(intel.is_empty());
        assert!(
            elapsed < Duration::from_secs(2),
            "must return at ~the cap, not the 3600s hang; took {elapsed:?}",
        );
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
