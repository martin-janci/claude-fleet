//! Host-reboot recovery, part 2: plan and execute a batch restore of a
//! host's resumable lost sessions over the existing [`recreate_session`]
//! primitive (Task 3 of the host-reboot recovery plan; Task 4 wraps this in
//! an MCP tool + Tauri command).
//!
//! [`plan_restore`] is pure/read-only (no ssh, no writes) and is shared by
//! `dry_run` and the real execution path, so a caller previewing a restore
//! sees exactly the entries that will actually be attempted.
//! [`restore_host_sessions_with`] is the test seam: production wires it to
//! [`recreate_session`] via [`restore_host_sessions`]; tests inject a fake
//! closure so the batching/stagger/failure-isolation behaviour is exercisable
//! without a real host.

use super::*;
use crate::ipc_error::codes;
use crate::ipc_error::lock;
use futures_util::stream::{FuturesUnordered, StreamExt};
use serde::Serialize;
use std::future::Future;
use std::pin::Pin;

#[derive(Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "RestoreHostSessionsParams")]
pub struct RestoreHostSessionsArgs {
    /// Host whose lost sessions to restore.
    pub host_alias: String,
    /// The plan only: no ssh, no writes.
    #[serde(default)]
    pub dry_run: bool,
    /// Only these sessions (default: every restorable one).
    #[serde(default)]
    pub session_ids: Option<Vec<i64>>,
}

/// One planned restore action. `action` is `"restore"` for a session the
/// batch will attempt to resume, `"skip"` (with `reason` set) for one an
/// explicit `session_ids` request named that cannot be restored — or, in
/// either plan, for the fleet controller, which the batch never recreates. `tmux_name`
/// is `None` only for an id that names no session at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RestorePlanEntry {
    pub session_id: i64,
    pub tmux_name: Option<String>,
    pub cwd: Option<String>,
    pub claude_session_id: Option<String>,
    pub friendly_name: Option<String>,
    pub action: String,
    pub reason: Option<String>,
}

/// The result of one restore attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RestoreOutcome {
    pub session_id: i64,
    pub tmux_name: String,
    pub ok: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RestoreReport {
    pub host_alias: String,
    pub dry_run: bool,
    pub plan: Vec<RestorePlanEntry>,
    pub results: Vec<RestoreOutcome>,
}

/// One in-flight launch in `restore_host_sessions_with`'s batch loop: the
/// plan index, the session id, its tmux/claude ids (carried through for the
/// timeline event + log once the call resolves), and the `recreate` result —
/// `None` when the row was no longer lost at launch time, so `recreate` was
/// never called (see [`still_lost`]).
type LaunchedRestore<'a> = Pin<
    Box<
        dyn Future<
                Output = (
                    usize,
                    i64,
                    String,
                    Option<String>,
                    Option<Result<SessionRow, IpcError>>,
                ),
            > + Send
            + 'a,
    >,
>;

/// The `error` of a restore entry skipped at launch because its row was no
/// longer lost — another restore (or a manual recreate) got to it first.
pub(crate) const RESTORED_ELSEWHERE: &str = "restored elsewhere";

/// Hosts with a non-dry-run restore in progress. A row stays lost until
/// `recreate_session`'s final `restore_session`, so a second restore of the
/// same host would re-plan rows the first is still rebuilding, and
/// `recreate_session`'s kill-then-create would kill the pane the first call
/// just started. One restore per host at a time; a second is refused with
/// `E_INVALID_STATE`. Different hosts never block each other.
#[derive(Default)]
pub(crate) struct RestoresInFlight {
    hosts: std::sync::Mutex<std::collections::HashSet<String>>,
}

impl RestoresInFlight {
    /// Claim `host_alias`, or refuse when a restore of it is already running.
    /// The claim is released when the returned guard drops — on every exit
    /// path, including an early `?` return, a panic or the caller's future
    /// being dropped mid-flight.
    fn claim(&self, host_alias: &str) -> Result<RestoreClaim<'_>, IpcError> {
        let mut hosts = self.hosts.lock().unwrap_or_else(|e| e.into_inner());
        if !hosts.insert(host_alias.to_string()) {
            return Err(IpcError::new(
                codes::E_INVALID_STATE,
                format!(
                    "a restore of {host_alias} is already in progress; wait for it to finish \
                     (the host's sessions come back as it goes)"
                ),
            ));
        }
        Ok(RestoreClaim {
            owner: self,
            host_alias: host_alias.to_string(),
        })
    }

    #[cfg(test)]
    fn is_claimed(&self, host_alias: &str) -> bool {
        self.hosts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(host_alias)
    }
}

/// RAII claim on one host in [`RestoresInFlight`].
struct RestoreClaim<'a> {
    owner: &'a RestoresInFlight,
    host_alias: String,
}

impl Drop for RestoreClaim<'_> {
    fn drop(&mut self) {
        self.owner
            .hosts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.host_alias);
    }
}

/// The process-wide registry production restores go through.
static RESTORES_IN_FLIGHT: std::sync::LazyLock<RestoresInFlight> =
    std::sync::LazyLock::new(RestoresInFlight::default);

/// Re-read one planned row right before its recreate: `false` when it is no
/// longer lost (restored by someone else since the plan was made), so the
/// batch must not kill-and-rebuild a pane that is already back. A row that
/// vanished counts as still lost — `recreate` then reports it properly.
fn still_lost(store: &Mutex<Store>, session_id: i64) -> Result<bool, IpcError> {
    let s = lock(store)?;
    Ok(s.get_session_by_id(session_id)?
        .is_none_or(|r| r.lost_at.is_some()))
}

/// A session kind that is a real tmux pane fleet can recreate — i.e. every
/// kind except a pane-less `claude --bg` / externally-run agent row (see
/// `reconcile_agent_rows`), which has nothing for `recreate_session` to
/// rebuild.
fn is_tmux_kind(kind: &str) -> bool {
    !matches!(kind, "bg" | "external")
}

/// The plan `cwd` for one row: the worktree row's path when `worktree_id` is
/// set; otherwise, for `local`, the owning project's `base_path`; otherwise
/// `None` (a remote orphan session — `recreate_session` resolves its own cwd
/// via `cwd_source_for_session`, this is purely an informational preview).
fn plan_cwd(s: &Store, row: &SessionRow) -> Option<String> {
    if let Some(wid) = row.worktree_id {
        return s.get_worktree_row(wid).ok().flatten().map(|w| w.path);
    }
    if row.host_alias == "local" {
        if let Some(pid) = row.project_id {
            return s
                .list_projects()
                .ok()
                .into_iter()
                .flatten()
                .find(|p| p.id == pid)
                .map(|p| p.base_path);
        }
    }
    None
}

/// The skip reason for the registered fleet controller: the batch recreates
/// with `force: false`, which `guard_not_controller` refuses for it, so it
/// would only ever fail — say how to bring it back instead.
pub(crate) const CONTROLLER_SKIP_REASON: &str =
    "fleet controller: recreate it explicitly with force";

/// Whether `row` is the registered fleet controller — the same test
/// `guard_not_controller` applies (host + tmux name).
fn is_controller(controller: Option<&(String, String)>, row: &SessionRow) -> bool {
    controller.is_some_and(|(h, n)| *h == row.host_alias && *n == row.tmux_name)
}

/// Every lost, resumable session on the host, ordered by `lost_at` then `id`
/// — the `session_ids`-less plan. The fleet controller is listed as a skip
/// ([`CONTROLLER_SKIP_REASON`]), since the batch cannot recreate it.
fn plan_all_lost(s: &Store, rows: &[SessionRow]) -> Result<Vec<RestorePlanEntry>, IpcError> {
    let controller = s.get_controller()?;
    let mut candidates: Vec<&SessionRow> = rows
        .iter()
        .filter(|r| r.lost_at.is_some() && is_tmux_kind(&r.kind) && r.claude_session_id.is_some())
        .collect();
    candidates.sort_by_key(|r| (r.lost_at.unwrap_or(i64::MAX), r.id));
    Ok(candidates
        .into_iter()
        .map(|r| {
            let controller = is_controller(controller.as_ref(), r);
            RestorePlanEntry {
                session_id: r.id,
                tmux_name: Some(r.tmux_name.clone()),
                cwd: plan_cwd(s, r),
                claude_session_id: r.claude_session_id.clone(),
                friendly_name: r.friendly_name.clone(),
                action: if controller { "skip" } else { "restore" }.into(),
                reason: controller.then(|| CONTROLLER_SKIP_REASON.to_string()),
            }
        })
        .collect())
}

/// One `session_ids` entry: look the id up fleet-wide (not just on this
/// host) so a row that exists but lives elsewhere is reported as "not found
/// on this host" with its real `tmux_name`, distinct from an id that names
/// no session at all (`tmux_name: None`).
fn plan_one_explicit(s: &Store, host_alias: &str, id: i64) -> Result<RestorePlanEntry, IpcError> {
    let Some(row) = s.get_session_by_id(id)? else {
        return Ok(RestorePlanEntry {
            session_id: id,
            tmux_name: None,
            cwd: None,
            claude_session_id: None,
            friendly_name: None,
            action: "skip".into(),
            reason: Some("not found on this host".to_string()),
        });
    };
    let base = RestorePlanEntry {
        session_id: id,
        tmux_name: Some(row.tmux_name.clone()),
        cwd: None,
        claude_session_id: row.claude_session_id.clone(),
        friendly_name: row.friendly_name.clone(),
        action: "restore".into(),
        reason: None,
    };
    if row.host_alias != host_alias {
        return Ok(RestorePlanEntry {
            action: "skip".into(),
            reason: Some("not found on this host".to_string()),
            ..base
        });
    }
    // Only compute cwd once we know the row is actually on this host — cwd
    // resolution assumes the caller is asking about a session ON host_alias.
    let base = RestorePlanEntry {
        cwd: plan_cwd(s, &row),
        ..base
    };
    if !is_tmux_kind(&row.kind) {
        return Ok(RestorePlanEntry {
            action: "skip".into(),
            reason: Some("background agent: resume it with its own tooling".to_string()),
            ..base
        });
    }
    if row.lost_at.is_none() {
        return Ok(RestorePlanEntry {
            action: "skip".into(),
            reason: Some("not lost".to_string()),
            ..base
        });
    }
    if row.claude_session_id.is_none() {
        return Ok(RestorePlanEntry {
            action: "skip".into(),
            reason: Some("no claude conversation id to resume".to_string()),
            ..base
        });
    }
    if is_controller(s.get_controller()?.as_ref(), &row) {
        return Ok(RestorePlanEntry {
            action: "skip".into(),
            reason: Some(CONTROLLER_SKIP_REASON.to_string()),
            ..base
        });
    }
    Ok(base)
}

/// The `session_ids` plan: one entry per requested id, in request order,
/// deduplicated (a repeated id is silently dropped after its first entry).
fn plan_explicit_ids(
    s: &Store,
    host_alias: &str,
    ids: &[i64],
) -> Result<Vec<RestorePlanEntry>, IpcError> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(ids.len());
    for &id in ids {
        if !seen.insert(id) {
            continue;
        }
        out.push(plan_one_explicit(s, host_alias, id)?);
    }
    Ok(out)
}

/// Build the restore plan for `args`. Pure read-only: no ssh, no writes.
/// Shared by `dry_run` and the real execution path.
pub fn plan_restore(
    s: &Store,
    args: &RestoreHostSessionsArgs,
) -> Result<Vec<RestorePlanEntry>, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    match &args.session_ids {
        None => {
            let rows = s.list_sessions_for_host(&args.host_alias)?;
            plan_all_lost(s, &rows)
        }
        Some(ids) => plan_explicit_ids(s, &args.host_alias, ids),
    }
}

/// Resolve `restore.batch_size` the way `read_lost_ttl_cutoff` resolves
/// `sessions.lost_ttl_secs`: `settings::resolve` over the raw stored value,
/// falling back to the registry default (and, defensively, to `1` — a batch
/// of size `0` would starve the restore forever).
fn read_restore_batch_size(raw: Option<String>) -> usize {
    crate::service::settings::resolve(crate::service::settings::RESTORE_BATCH_SIZE, raw.as_deref())
        .parse::<usize>()
        .unwrap_or(4)
        .max(1)
}

/// Resolve `restore.stagger_ms` the same way.
fn read_restore_stagger_ms(raw: Option<String>) -> u64 {
    crate::service::settings::resolve(crate::service::settings::RESTORE_STAGGER_MS, raw.as_deref())
        .parse::<u64>()
        .unwrap_or(3000)
}

/// Record one restore attempt's outcome on the session timeline and log it.
/// Best-effort: an event-write failure is only `warn!`-logged, never
/// propagated (Task G convention — see `service::sessions::prompt::record_session_event`).
fn record_restore_outcome(
    store: &Mutex<Store>,
    host_alias: &str,
    session_id: i64,
    tmux_name: &str,
    claude_session_id: Option<&str>,
    result: &Result<SessionRow, IpcError>,
) {
    let (kind, detail): (&str, Option<String>) = match result {
        Ok(_) => ("session_restored", None),
        Err(e) => ("session_restore_failed", Some(e.to_string())),
    };
    match lock(store) {
        Ok(s) => {
            if let Err(e) = s.insert_session_event(session_id, kind, detail.as_deref()) {
                tracing::warn!(
                    host_alias,
                    session_id,
                    error = %e,
                    "[restore] session_event insert failed"
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                host_alias,
                session_id,
                error = %e,
                "[restore] store lock failed recording a restore event"
            );
        }
    }
    match result {
        Ok(_) => tracing::info!(
            host_alias,
            tmux_name,
            claude_session_id,
            session_id,
            "[restore] session restored"
        ),
        Err(e) => tracing::info!(
            host_alias,
            tmux_name,
            claude_session_id,
            session_id,
            error = %e,
            "[restore] session restore failed"
        ),
    }
}

/// Plan and execute a batch restore. `recreate` is the injectable "resume
/// one session" primitive — production wires it to [`recreate_session`] via
/// [`restore_host_sessions`]; tests inject a fake so batching, staggering and
/// failure isolation are exercisable without a real host.
///
/// `dry_run` returns the plan with empty `results` and never calls
/// `recreate` or writes anything. Otherwise the host is claimed in
/// `in_flight` for the whole call (a concurrent restore of the same host gets
/// `E_INVALID_STATE`), the host must be known and `reachable` (checked
/// before any `recreate` call, `E_HOST_OFFLINE` otherwise), and each entry's
/// row is re-read right before its `recreate`: one that is no longer lost is
/// reported `ok: false` with [`RESTORED_ELSEWHERE`] and not recreated. The
/// `restore` entries are launched in plan order, at
/// most `restore.batch_size` in flight, with `restore.stagger_ms` between
/// successive launches (no stagger wait before the first). One entry's
/// `Err` never aborts the others; `results` comes back ordered like the
/// plan's `restore` entries regardless of completion order.
pub(crate) async fn restore_host_sessions_with<F, Fut>(
    args: RestoreHostSessionsArgs,
    store: &Mutex<Store>,
    in_flight: &RestoresInFlight,
    recreate: F,
) -> Result<RestoreReport, IpcError>
where
    F: Fn(i64) -> Fut + Send + Sync,
    Fut: Future<Output = Result<SessionRow, IpcError>> + Send,
{
    let plan = {
        let s = lock(store)?;
        plan_restore(&s, &args)?
    };
    if args.dry_run {
        return Ok(RestoreReport {
            host_alias: args.host_alias,
            dry_run: true,
            plan,
            results: Vec::new(),
        });
    }

    let _claim = in_flight.claim(&args.host_alias)?;

    let (batch_size, stagger_ms) = {
        let s = lock(store)?;
        let reachable = s
            .get_host_row(&args.host_alias)?
            .map(|h| h.reachable)
            .unwrap_or(false);
        if !reachable {
            return Err(IpcError::new(
                codes::E_HOST_OFFLINE,
                format!("host {} is not reachable", args.host_alias),
            ));
        }
        let batch_raw = s
            .get_setting(crate::service::settings::RESTORE_BATCH_SIZE)
            .ok()
            .flatten();
        let stagger_raw = s
            .get_setting(crate::service::settings::RESTORE_STAGGER_MS)
            .ok()
            .flatten();
        (
            read_restore_batch_size(batch_raw),
            read_restore_stagger_ms(stagger_raw),
        )
    };

    let restore_entries: Vec<&RestorePlanEntry> =
        plan.iter().filter(|e| e.action == "restore").collect();
    let n = restore_entries.len();
    let host_alias = args.host_alias;
    let mut results: Vec<Option<RestoreOutcome>> = (0..n).map(|_| None).collect();

    if n > 0 {
        // At most `batch_size` permits in flight; a launch consumes one, the
        // pushed future's own drop (once it completes) releases it back —
        // `try_acquire` rather than a blocking `.await` because nothing else
        // would drive `launched` forward while a blocking acquire waited.
        let sem = tokio::sync::Semaphore::new(batch_size);
        let stagger_dur = std::time::Duration::from_millis(stagger_ms);
        let recreate_ref = &recreate;
        let mut launched: FuturesUnordered<LaunchedRestore<'_>> = FuturesUnordered::new();
        let mut idx = 0usize;
        let mut completed = 0usize;
        // `Some` while waiting out the stagger gap before the NEXT launch;
        // `None` means "free to launch as soon as a permit is available"
        // (true at the start — no stagger wait before the first launch).
        let mut pending_sleep: Option<Pin<Box<tokio::time::Sleep>>> = None;

        loop {
            while idx < n && pending_sleep.is_none() {
                let Ok(permit) = sem.try_acquire() else {
                    break;
                };
                let entry = restore_entries[idx];
                let this_idx = idx;
                let sid = entry.session_id;
                let tmux_name = entry.tmux_name.clone().unwrap_or_default();
                let claude_session_id = entry.claude_session_id.clone();
                launched.push(Box::pin(async move {
                    let _permit = permit;
                    let res = match still_lost(store, sid) {
                        Ok(true) => Some(recreate_ref(sid).await),
                        Ok(false) => None,
                        Err(e) => Some(Err(e)),
                    };
                    (this_idx, sid, tmux_name, claude_session_id, res)
                }));
                idx += 1;
                if idx < n {
                    pending_sleep = Some(Box::pin(tokio::time::sleep(stagger_dur)));
                }
            }
            if completed == n {
                break;
            }
            tokio::select! {
                _ = async {
                    if let Some(sleep) = pending_sleep.as_mut() {
                        sleep.await;
                    }
                }, if pending_sleep.is_some() => {
                    pending_sleep = None;
                }
                item = launched.next(), if !launched.is_empty() => {
                    if let Some((i, sid, tmux_name, claude_session_id, res)) = item {
                        let outcome = match res {
                            None => {
                                tracing::info!(
                                    host_alias,
                                    tmux_name,
                                    session_id = sid,
                                    "[restore] skipped: no longer lost"
                                );
                                RestoreOutcome {
                                    session_id: sid,
                                    tmux_name,
                                    ok: false,
                                    error: Some(RESTORED_ELSEWHERE.to_string()),
                                }
                            }
                            Some(res) => {
                                record_restore_outcome(
                                    store,
                                    &host_alias,
                                    sid,
                                    &tmux_name,
                                    claude_session_id.as_deref(),
                                    &res,
                                );
                                match res {
                                    Ok(_) => RestoreOutcome {
                                        session_id: sid,
                                        tmux_name,
                                        ok: true,
                                        error: None,
                                    },
                                    Err(e) => RestoreOutcome {
                                        session_id: sid,
                                        tmux_name,
                                        ok: false,
                                        error: Some(e.message.clone()),
                                    },
                                }
                            }
                        };
                        results[i] = Some(outcome);
                        completed += 1;
                    }
                }
            }
        }
    }

    Ok(RestoreReport {
        host_alias,
        dry_run: false,
        plan,
        results: results
            .into_iter()
            .map(|r| r.expect("every restore entry produced exactly one result"))
            .collect(),
    })
}

/// Restore a host's lost sessions over `recreate_session`.
pub async fn restore_host_sessions(
    args: RestoreHostSessionsArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<RestoreReport, IpcError> {
    restore_host_sessions_with(args, store, &RESTORES_IN_FLIGHT, |id| {
        recreate_session(
            RecreateSessionArgs {
                session_id: id,
                force: false,
            },
            store,
            ssh,
        )
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Seed a host + a mix of sessions: three lost tmux rows (with claude
    /// ids — `lost1` also carries a project/worktree and a friendly_name, so
    /// the plan's `cwd`/`friendly_name` population is actually exercised),
    /// one live tmux row, one lost `bg` row (with a claude id), and one lost
    /// tmux row with NO claude id. Returns `(store, ids)` where `ids` is
    /// `(lost1, lost2, lost3, live, lost_bg, lost_no_claude_id)`.
    fn seed_mixed_host(host: &str) -> (Store, (i64, i64, i64, i64, i64, i64)) {
        let mut s = Store::open_in_memory().expect("open");
        s.upsert_host(host).unwrap();
        s.apply_host_reconcile(HostReconcile {
            alias: host,
            reachable: true,
            claude_version: None,
            tmux_version: None,
            last_pinged_at: 0,
            probe_started_at: 0,
            sessions: &[],
            keep: &[],
            lost_ttl_cutoff: None,
            skip_prune: false,
            reconciled_at: None,
        })
        .unwrap();

        let pid = s.upsert_project("o", "r", "/base/r").unwrap();
        let wid = s
            .upsert_worktree_on(
                host,
                pid,
                "feat",
                "/base/r/.claude/worktrees/feat",
                Some("feat"),
            )
            .unwrap();
        let lost1 = s
            .upsert_session("lost-1", host, Some(pid), Some(wid), 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(lost1, "claude-1").unwrap();
        s.set_friendly_name(host, "lost-1", Some("My Friendly Name"))
            .unwrap();
        let lost2 = s
            .upsert_session("lost-2", host, None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(lost2, "claude-2").unwrap();
        let lost3 = s
            .upsert_session("lost-3", host, None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(lost3, "claude-3").unwrap();
        let live = s
            .upsert_session("live", host, None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(live, "claude-live").unwrap();
        let lost_bg = s
            .upsert_bg_session(
                host,
                "bg:claude-bg",
                None,
                "claude-bg",
                Some("working"),
                1,
                "bg",
                1,
            )
            .unwrap();
        let lost_no_claude_id = s
            .upsert_session("lost-no-claude", host, None, None, 1, 1, "running", None)
            .unwrap();

        // Mark everything except "live" lost in one pass.
        s.mark_host_sessions_lost(host, "host_reboot", &["live".to_string()], 500, 0)
            .unwrap();

        (s, (lost1, lost2, lost3, live, lost_bg, lost_no_claude_id))
    }

    fn session_events_count(s: &Store, session_id: i64) -> usize {
        s.list_session_events(session_id, 1000).unwrap().len()
    }

    #[tokio::test]
    async fn dry_run_plans_every_lost_resumable_session_and_touches_nothing() {
        let (s, (lost1, lost2, lost3, live, lost_bg, lost_no_claude_id)) = seed_mixed_host("h");
        let store = Mutex::new(s);
        let counter = std::sync::Arc::new(AtomicUsize::new(0));

        let before = lock(&store).unwrap().list_sessions_for_host("h").unwrap();
        let before_events: usize = [lost1, lost2, lost3, live, lost_bg, lost_no_claude_id]
            .iter()
            .map(|&id| session_events_count(&lock(&store).unwrap(), id))
            .sum();

        let counter_for_closure = std::sync::Arc::clone(&counter);
        let report = restore_host_sessions_with(
            RestoreHostSessionsArgs {
                host_alias: "h".to_string(),
                dry_run: true,
                session_ids: None,
            },
            &store,
            &RestoresInFlight::default(),
            move |_id| {
                let counter = std::sync::Arc::clone(&counter_for_closure);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Err(IpcError::new(
                        codes::E_INTERNAL,
                        "must not be called in dry_run",
                    ))
                }
            },
        )
        .await
        .unwrap();

        assert!(report.dry_run);
        assert!(report.results.is_empty());
        let restore_ids: Vec<i64> = report
            .plan
            .iter()
            .filter(|e| e.action == "restore")
            .map(|e| e.session_id)
            .collect();
        assert_eq!(restore_ids, vec![lost1, lost2, lost3]);
        for entry in report.plan.iter().filter(|e| e.action == "restore") {
            assert!(entry.tmux_name.is_some());
            assert!(entry.claude_session_id.is_some());
            assert_eq!(entry.reason, None);
        }
        // `lost1` carries a worktree + friendly_name (see `seed_mixed_host`):
        // its plan entry must surface both, via the worktree-path branch of
        // `plan_cwd`.
        let lost1_entry = report
            .plan
            .iter()
            .find(|e| e.session_id == lost1)
            .expect("lost1 is a restore entry");
        assert_eq!(
            lost1_entry.cwd.as_deref(),
            Some("/base/r/.claude/worktrees/feat"),
            "cwd must come from the worktree row's path"
        );
        assert_eq!(
            lost1_entry.friendly_name.as_deref(),
            Some("My Friendly Name")
        );
        // `lost2`/`lost3` have neither a worktree nor a project, and the host
        // isn't `local`, so neither `plan_cwd` branch applies: cwd stays
        // `None` (and there's no friendly_name to surface).
        let lost2_entry = report
            .plan
            .iter()
            .find(|e| e.session_id == lost2)
            .expect("lost2 is a restore entry");
        assert_eq!(lost2_entry.cwd, None);
        assert_eq!(lost2_entry.friendly_name, None);
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "dry_run must not call recreate"
        );

        let after = lock(&store).unwrap().list_sessions_for_host("h").unwrap();
        assert_eq!(before, after, "dry_run must not write anything");
        let after_events: usize = [lost1, lost2, lost3, live, lost_bg, lost_no_claude_id]
            .iter()
            .map(|&id| session_events_count(&lock(&store).unwrap(), id))
            .sum();
        assert_eq!(
            before_events, after_events,
            "dry_run must not insert events"
        );
    }

    /// The other half of `plan_cwd`'s two populated branches: a `local` lost
    /// row with a project but NO worktree falls back to the project's
    /// `base_path`. `plan_restore` is pure/sync, so this is tested directly
    /// rather than through `restore_host_sessions_with`.
    #[test]
    fn plan_cwd_falls_back_to_the_project_base_path_on_local_without_a_worktree() {
        let mut s = Store::open_in_memory().expect("open");
        s.upsert_host("local").unwrap();
        s.apply_host_reconcile(HostReconcile {
            alias: "local",
            reachable: true,
            claude_version: None,
            tmux_version: None,
            last_pinged_at: 0,
            probe_started_at: 0,
            sessions: &[],
            keep: &[],
            lost_ttl_cutoff: None,
            skip_prune: false,
            reconciled_at: None,
        })
        .unwrap();
        let pid = s.upsert_project("o", "r", "/base/r").unwrap();
        let orphan = s
            .upsert_session("orphan", "local", Some(pid), None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(orphan, "claude-orphan").unwrap();
        s.mark_host_sessions_lost("local", "host_reboot", &[], 500, 0)
            .unwrap();

        let plan = plan_restore(
            &s,
            &RestoreHostSessionsArgs {
                host_alias: "local".to_string(),
                dry_run: true,
                session_ids: None,
            },
        )
        .unwrap();
        let entry = plan
            .iter()
            .find(|e| e.session_id == orphan)
            .expect("orphan is a restore entry");
        assert_eq!(entry.action, "restore");
        assert_eq!(
            entry.cwd.as_deref(),
            Some("/base/r"),
            "cwd must fall back to the project's base_path"
        );
    }

    #[tokio::test]
    async fn explicit_ids_report_skips_with_reasons() {
        let (s, (_lost1, _lost2, lost3, live, lost_bg, lost_no_claude_id)) = seed_mixed_host("h");
        s.upsert_host("other").unwrap();
        let other_host_row = s
            .upsert_session("elsewhere", "other", None, None, 1, 1, "running", None)
            .unwrap();
        let store = Mutex::new(s);

        let ids = vec![
            live,
            lost_bg,
            lost_no_claude_id,
            other_host_row,
            999_999,
            lost3,
            lost3,
        ];
        let report = restore_host_sessions_with(
            RestoreHostSessionsArgs {
                host_alias: "h".to_string(),
                dry_run: true,
                session_ids: Some(ids),
            },
            &store,
            &RestoresInFlight::default(),
            |_id| async move { unreachable!("dry_run never calls recreate") },
        )
        .await
        .unwrap();

        assert_eq!(
            report.plan.len(),
            6,
            "the repeated lost3 id is deduplicated"
        );
        assert_eq!(report.plan[0].session_id, live);
        assert_eq!(report.plan[0].action, "skip");
        assert_eq!(report.plan[0].reason.as_deref(), Some("not lost"));

        assert_eq!(report.plan[1].session_id, lost_bg);
        assert_eq!(report.plan[1].action, "skip");
        assert_eq!(
            report.plan[1].reason.as_deref(),
            Some("background agent: resume it with its own tooling")
        );

        assert_eq!(report.plan[2].session_id, lost_no_claude_id);
        assert_eq!(report.plan[2].action, "skip");
        assert_eq!(
            report.plan[2].reason.as_deref(),
            Some("no claude conversation id to resume")
        );

        assert_eq!(report.plan[3].session_id, other_host_row);
        assert_eq!(report.plan[3].action, "skip");
        assert_eq!(
            report.plan[3].reason.as_deref(),
            Some("not found on this host")
        );
        assert!(
            report.plan[3].tmux_name.is_some(),
            "a row on another host still names its tmux_name"
        );

        assert_eq!(report.plan[4].session_id, 999_999);
        assert_eq!(report.plan[4].action, "skip");
        assert_eq!(
            report.plan[4].reason.as_deref(),
            Some("not found on this host")
        );
        assert_eq!(
            report.plan[4].tmux_name, None,
            "an unknown id has no tmux_name"
        );

        assert_eq!(report.plan[5].session_id, lost3);
        assert_eq!(report.plan[5].action, "restore");
        assert_eq!(report.plan[5].reason, None);
    }

    #[tokio::test]
    async fn one_failure_does_not_stop_the_batch() {
        let mut s = Store::open_in_memory().expect("open");
        s.upsert_host("h").unwrap();
        s.apply_host_reconcile(HostReconcile {
            alias: "h",
            reachable: true,
            claude_version: None,
            tmux_version: None,
            last_pinged_at: 0,
            probe_started_at: 0,
            sessions: &[],
            keep: &[],
            lost_ttl_cutoff: None,
            skip_prune: false,
            reconciled_at: None,
        })
        .unwrap();
        let a = s
            .upsert_session("a", "h", None, None, 1, 1, "running", None)
            .unwrap();
        s.set_claude_session_id(a, "claude-a").unwrap();
        let b = s
            .upsert_session("b", "h", None, None, 2, 2, "running", None)
            .unwrap();
        s.set_claude_session_id(b, "claude-b").unwrap();
        let c = s
            .upsert_session("c", "h", None, None, 3, 3, "running", None)
            .unwrap();
        s.set_claude_session_id(c, "claude-c").unwrap();
        s.mark_host_sessions_lost("h", "host_reboot", &[], 500, 0)
            .unwrap();
        // Keep the test fast: it does not exercise stagger timing.
        s.set_setting("restore.stagger_ms", "0").unwrap();
        let store = Mutex::new(s);
        // A plain `&store` reference so the `move` closure below captures a
        // (Copy) reference instead of trying to move `store` itself — which
        // would conflict with the `&store` argument passed to the same call.
        let store_for_closure = &store;

        let report = restore_host_sessions_with(
            RestoreHostSessionsArgs {
                host_alias: "h".to_string(),
                dry_run: false,
                session_ids: None,
            },
            &store,
            &RestoresInFlight::default(),
            move |id| async move {
                if id == b {
                    Err(IpcError::new(codes::E_REPAIR_REQUIRED, "worktree gone"))
                } else {
                    let s = lock(store_for_closure).unwrap();
                    Ok(s.get_session_by_id(id).unwrap().unwrap())
                }
            },
        )
        .await
        .unwrap();

        assert_eq!(report.results.len(), 3);
        assert_eq!(report.results[0].session_id, a);
        assert!(report.results[0].ok);
        assert_eq!(report.results[1].session_id, b);
        assert!(!report.results[1].ok);
        assert!(report.results[1]
            .error
            .as_deref()
            .unwrap_or("")
            .contains("worktree gone"));
        assert_eq!(report.results[2].session_id, c);
        assert!(report.results[2].ok);

        let s = lock(&store).unwrap();
        let a_events = s.list_session_events(a, 10).unwrap();
        assert_eq!(
            a_events
                .iter()
                .filter(|e| e.kind == "session_restored")
                .count(),
            1,
            "exactly one restored event per success: {a_events:?}"
        );
        let b_events = s.list_session_events(b, 10).unwrap();
        let failed = b_events
            .iter()
            .find(|e| e.kind == "session_restore_failed")
            .expect("failure event recorded");
        assert!(failed
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("worktree gone"));
        let c_events = s.list_session_events(c, 10).unwrap();
        assert_eq!(
            c_events
                .iter()
                .filter(|e| e.kind == "session_restored")
                .count(),
            1,
            "exactly one restored event per success: {c_events:?}"
        );
    }

    #[tokio::test]
    async fn unreachable_host_fails_fast() {
        let mut s = Store::open_in_memory().expect("open");
        s.upsert_host("h").unwrap();
        s.apply_host_reconcile(HostReconcile {
            alias: "h",
            reachable: false,
            claude_version: None,
            tmux_version: None,
            last_pinged_at: 0,
            probe_started_at: 0,
            sessions: &[],
            keep: &[],
            lost_ttl_cutoff: None,
            skip_prune: false,
            reconciled_at: None,
        })
        .unwrap();
        let store = Mutex::new(s);
        let counter = std::sync::Arc::new(AtomicUsize::new(0));
        let counter_for_closure = std::sync::Arc::clone(&counter);

        let err = restore_host_sessions_with(
            RestoreHostSessionsArgs {
                host_alias: "h".to_string(),
                dry_run: false,
                session_ids: None,
            },
            &store,
            &RestoresInFlight::default(),
            move |_id| {
                let counter = std::sync::Arc::clone(&counter_for_closure);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Err(IpcError::new(codes::E_INTERNAL, "must not be called"))
                }
            },
        )
        .await
        .unwrap_err();

        assert_eq!(err.code, codes::E_HOST_OFFLINE);
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    /// Add a reachable `host` with one lost, resumable tmux row per `names`
    /// entry; returns their ids in order.
    fn seed_reachable_lost(s: &mut Store, host: &str, names: &[&str]) -> Vec<i64> {
        s.upsert_host(host).unwrap();
        s.apply_host_reconcile(HostReconcile {
            alias: host,
            reachable: true,
            claude_version: None,
            tmux_version: None,
            last_pinged_at: 0,
            probe_started_at: 0,
            sessions: &[],
            keep: &[],
            lost_ttl_cutoff: None,
            skip_prune: false,
            reconciled_at: None,
        })
        .unwrap();
        let ids: Vec<i64> = names
            .iter()
            .map(|n| {
                let id = s
                    .upsert_session(n, host, None, None, 1, 1, "running", None)
                    .unwrap();
                s.set_claude_session_id(id, &format!("claude-{host}-{n}"))
                    .unwrap();
                id
            })
            .collect();
        s.mark_host_sessions_lost(host, "host_reboot", &[], 500, 0)
            .unwrap();
        ids
    }

    fn run_args(host: &str) -> RestoreHostSessionsArgs {
        RestoreHostSessionsArgs {
            host_alias: host.to_string(),
            dry_run: false,
            session_ids: None,
        }
    }

    /// A second restore of a host whose restore is still running is refused
    /// (it would re-plan rows the first is rebuilding, and recreate's
    /// kill-then-create would kill the fresh panes); the first call is
    /// unaffected, a different host is not blocked, and the claim is released
    /// once the first call returns.
    #[tokio::test]
    async fn a_second_restore_of_the_same_host_is_refused_while_one_runs() {
        let mut s = Store::open_in_memory().expect("open");
        let h_ids = seed_reachable_lost(&mut s, "h", &["a", "b"]);
        let h2_ids = seed_reachable_lost(&mut s, "h2", &["c"]);
        s.set_setting("restore.stagger_ms", "0").unwrap();
        let store = Mutex::new(s);
        let store_ref = &store;
        let registry = RestoresInFlight::default();

        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
        let entered_tx = std::sync::Mutex::new(Some(entered_tx));
        let release = tokio::sync::Notify::new();
        let (entered_ref, release_ref) = (&entered_tx, &release);
        let first_calls = AtomicUsize::new(0);
        let first_calls_ref = &first_calls;

        let first =
            restore_host_sessions_with(run_args("h"), &store, &registry, move |id| async move {
                first_calls_ref.fetch_add(1, Ordering::SeqCst);
                let tx = entered_ref.lock().unwrap().take();
                if let Some(tx) = tx {
                    let _ = tx.send(());
                    release_ref.notified().await;
                }
                let s = lock(store_ref).unwrap();
                Ok(s.get_session_by_id(id).unwrap().unwrap())
            });
        let second = async {
            entered_rx.await.unwrap();
            assert!(registry.is_claimed("h"));
            let refused =
                restore_host_sessions_with(run_args("h"), &store, &registry, |_id| async {
                    unreachable!("a refused restore must not recreate anything")
                })
                .await
                .unwrap_err();
            let other =
                restore_host_sessions_with(run_args("h2"), &store, &registry, |id| async move {
                    let s = lock(store_ref).unwrap();
                    Ok(s.get_session_by_id(id).unwrap().unwrap())
                })
                .await
                .unwrap();
            release_ref.notify_one();
            (refused, other)
        };
        let (first, (refused, other)) = tokio::join!(first, second);

        assert_eq!(refused.code, codes::E_INVALID_STATE, "{refused:?}");
        assert!(
            refused.message.contains("already in progress"),
            "{refused:?}"
        );
        let first = first.unwrap();
        assert_eq!(
            first
                .results
                .iter()
                .map(|r| (r.session_id, r.ok))
                .collect::<Vec<_>>(),
            h_ids.iter().map(|&id| (id, true)).collect::<Vec<_>>(),
            "the first call is unaffected by the refused one"
        );
        assert_eq!(first_calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            other
                .results
                .iter()
                .map(|r| (r.session_id, r.ok))
                .collect::<Vec<_>>(),
            vec![(h2_ids[0], true)],
            "a different host is not blocked"
        );
        assert!(!registry.is_claimed("h"), "released after the call returns");
        assert!(!registry.is_claimed("h2"));
    }

    /// The claim is released on an early error return too (RAII), so a
    /// failed restore never wedges the host until the process restarts.
    #[tokio::test]
    async fn the_host_claim_is_released_on_an_error_return() {
        let mut s = Store::open_in_memory().expect("open");
        seed_reachable_lost(&mut s, "h", &["a"]);
        s.apply_host_reconcile(HostReconcile {
            alias: "h",
            reachable: false,
            claude_version: None,
            tmux_version: None,
            last_pinged_at: 0,
            probe_started_at: 0,
            sessions: &[],
            keep: &[],
            lost_ttl_cutoff: None,
            skip_prune: true,
            reconciled_at: None,
        })
        .unwrap();
        let store = Mutex::new(s);
        let registry = RestoresInFlight::default();

        let err = restore_host_sessions_with(run_args("h"), &store, &registry, |_id| async {
            unreachable!("an offline host is refused before any recreate")
        })
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_HOST_OFFLINE);
        assert!(!registry.is_claimed("h"));
        // And a retry is not refused as "in progress".
        let err = restore_host_sessions_with(run_args("h"), &store, &registry, |_id| async {
            unreachable!("an offline host is refused before any recreate")
        })
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_HOST_OFFLINE);
    }

    /// A row that is no longer lost when its turn comes (restored by someone
    /// else since the plan was made) is reported, not recreated: recreating
    /// it would kill a pane that is already back.
    #[tokio::test]
    async fn a_row_restored_since_the_plan_is_not_recreated() {
        let mut s = Store::open_in_memory().expect("open");
        let ids = seed_reachable_lost(&mut s, "h", &["a", "b"]);
        let (a, b) = (ids[0], ids[1]);
        // One at a time, so `b` is launched only after `a`'s recreate ran.
        s.set_setting("restore.batch_size", "1").unwrap();
        s.set_setting("restore.stagger_ms", "0").unwrap();
        let store = Mutex::new(s);
        let store_ref = &store;
        let recreated = std::sync::Mutex::new(Vec::<i64>::new());
        let recreated_ref = &recreated;

        let report = restore_host_sessions_with(
            run_args("h"),
            &store,
            &RestoresInFlight::default(),
            move |id| async move {
                recreated_ref.lock().unwrap().push(id);
                let s = lock(store_ref).unwrap();
                // While `a` is being recreated, `b` is restored elsewhere.
                s.restore_session(b).unwrap();
                Ok(s.get_session_by_id(id).unwrap().unwrap())
            },
        )
        .await
        .unwrap();

        assert_eq!(
            *recreated.lock().unwrap(),
            vec![a],
            "b must not be recreated"
        );
        assert_eq!(report.results.len(), 2);
        assert!(report.results[0].ok);
        assert_eq!(report.results[1].session_id, b);
        assert!(!report.results[1].ok);
        assert_eq!(report.results[1].error.as_deref(), Some(RESTORED_ELSEWHERE));
        let s = lock(&store).unwrap();
        assert!(
            s.list_session_events(b, 10)
                .unwrap()
                .iter()
                .all(|e| e.kind != "session_restore_failed" && e.kind != "session_restored"),
            "a skip is not a restore attempt on the timeline"
        );
    }

    /// A host fleet has no row for fails fast just like an unreachable one:
    /// `E_HOST_OFFLINE`, before any recreate.
    #[tokio::test]
    async fn a_missing_host_row_fails_fast() {
        let store = Mutex::new(Store::open_in_memory().expect("open"));
        let counter = AtomicUsize::new(0);
        let counter_ref = &counter;

        let err = restore_host_sessions_with(
            run_args("ghost-host"),
            &store,
            &RestoresInFlight::default(),
            move |_id| async move {
                counter_ref.fetch_add(1, Ordering::SeqCst);
                Err(IpcError::new(codes::E_INTERNAL, "must not be called"))
            },
        )
        .await
        .unwrap_err();

        assert_eq!(err.code, codes::E_HOST_OFFLINE);
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    /// The fleet controller can only be recreated with force, which the
    /// batch never passes: both plans list it as a skip with a reason saying
    /// how to bring it back, and the batch never tries it.
    #[tokio::test]
    async fn a_lost_fleet_controller_is_skipped_with_a_reason() {
        let mut s = Store::open_in_memory().expect("open");
        let ids = seed_reachable_lost(&mut s, "h", &["ctl", "other"]);
        let (ctl, other) = (ids[0], ids[1]);
        s.set_controller("h", "ctl").unwrap();
        s.set_setting("restore.stagger_ms", "0").unwrap();
        let store = Mutex::new(s);
        let store_ref = &store;

        let explicit = plan_restore(
            &lock(&store).unwrap(),
            &RestoreHostSessionsArgs {
                host_alias: "h".to_string(),
                dry_run: true,
                session_ids: Some(vec![ctl, other]),
            },
        )
        .unwrap();
        assert_eq!(explicit[0].action, "skip");
        assert_eq!(explicit[0].reason.as_deref(), Some(CONTROLLER_SKIP_REASON));
        assert_eq!(explicit[1].action, "restore");

        let recreated = std::sync::Mutex::new(Vec::<i64>::new());
        let recreated_ref = &recreated;
        let report = restore_host_sessions_with(
            run_args("h"),
            &store,
            &RestoresInFlight::default(),
            move |id| async move {
                recreated_ref.lock().unwrap().push(id);
                let s = lock(store_ref).unwrap();
                Ok(s.get_session_by_id(id).unwrap().unwrap())
            },
        )
        .await
        .unwrap();
        let ctl_entry = report
            .plan
            .iter()
            .find(|e| e.session_id == ctl)
            .expect("the controller is listed in the plan");
        assert_eq!(ctl_entry.action, "skip");
        assert_eq!(ctl_entry.reason.as_deref(), Some(CONTROLLER_SKIP_REASON));
        assert_eq!(*recreated.lock().unwrap(), vec![other]);
        assert_eq!(
            report
                .results
                .iter()
                .map(|r| r.session_id)
                .collect::<Vec<_>>(),
            vec![other]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn concurrency_never_exceeds_batch_size() {
        let mut s = Store::open_in_memory().expect("open");
        s.upsert_host("h").unwrap();
        s.apply_host_reconcile(HostReconcile {
            alias: "h",
            reachable: true,
            claude_version: None,
            tmux_version: None,
            last_pinged_at: 0,
            probe_started_at: 0,
            sessions: &[],
            keep: &[],
            lost_ttl_cutoff: None,
            skip_prune: false,
            reconciled_at: None,
        })
        .unwrap();
        let mut ids = Vec::new();
        for i in 0..6 {
            let id = s
                .upsert_session(&format!("s{i}"), "h", None, None, 1, 1, "running", None)
                .unwrap();
            s.set_claude_session_id(id, &format!("claude-{i}")).unwrap();
            ids.push(id);
        }
        s.mark_host_sessions_lost("h", "host_reboot", &[], 500, 0)
            .unwrap();
        s.set_setting("restore.batch_size", "2").unwrap();
        s.set_setting("restore.stagger_ms", "0").unwrap();
        let store = Mutex::new(s);
        // See `one_failure_does_not_stop_the_batch` for why this indirection
        // is needed: the `move` closure below must capture a `&Mutex<Store>`
        // (Copy), not `store` itself, since `store` is also borrowed by the
        // `&store` argument passed to the same call.
        let store_for_closure = &store;

        let in_flight = std::sync::Arc::new(AtomicUsize::new(0));
        let max_in_flight = std::sync::Arc::new(AtomicUsize::new(0));
        let in_flight_c = std::sync::Arc::clone(&in_flight);
        let max_in_flight_c = std::sync::Arc::clone(&max_in_flight);

        let report = restore_host_sessions_with(
            RestoreHostSessionsArgs {
                host_alias: "h".to_string(),
                dry_run: false,
                session_ids: None,
            },
            &store,
            &RestoresInFlight::default(),
            move |id| {
                let in_flight = std::sync::Arc::clone(&in_flight_c);
                let max_in_flight = std::sync::Arc::clone(&max_in_flight_c);
                let store = store_for_closure;
                async move {
                    let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_in_flight.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    let s = lock(store).unwrap();
                    Ok(s.get_session_by_id(id).unwrap().unwrap())
                }
            },
        )
        .await
        .unwrap();

        assert_eq!(report.results.len(), 6);
        assert_eq!(max_in_flight.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn launches_are_staggered() {
        let mut s = Store::open_in_memory().expect("open");
        s.upsert_host("h").unwrap();
        s.apply_host_reconcile(HostReconcile {
            alias: "h",
            reachable: true,
            claude_version: None,
            tmux_version: None,
            last_pinged_at: 0,
            probe_started_at: 0,
            sessions: &[],
            keep: &[],
            lost_ttl_cutoff: None,
            skip_prune: false,
            reconciled_at: None,
        })
        .unwrap();
        let mut ids = Vec::new();
        for i in 0..3 {
            let id = s
                .upsert_session(&format!("s{i}"), "h", None, None, 1, 1, "running", None)
                .unwrap();
            s.set_claude_session_id(id, &format!("claude-{i}")).unwrap();
            ids.push(id);
        }
        s.mark_host_sessions_lost("h", "host_reboot", &[], 500, 0)
            .unwrap();
        s.set_setting("restore.batch_size", "4").unwrap();
        s.set_setting("restore.stagger_ms", "3000").unwrap();
        let store = Mutex::new(s);
        let store_for_closure = &store;

        let starts = std::sync::Arc::new(std::sync::Mutex::new(Vec::<tokio::time::Instant>::new()));
        let starts_c = std::sync::Arc::clone(&starts);

        let report = restore_host_sessions_with(
            RestoreHostSessionsArgs {
                host_alias: "h".to_string(),
                dry_run: false,
                session_ids: None,
            },
            &store,
            &RestoresInFlight::default(),
            move |id| {
                let starts = std::sync::Arc::clone(&starts_c);
                let store = store_for_closure;
                async move {
                    starts.lock().unwrap().push(tokio::time::Instant::now());
                    let s = lock(store).unwrap();
                    Ok(s.get_session_by_id(id).unwrap().unwrap())
                }
            },
        )
        .await
        .unwrap();

        assert_eq!(report.results.len(), 3);
        let starts = starts.lock().unwrap();
        let mut sorted = starts.clone();
        sorted.sort();
        assert_eq!(sorted.len(), 3);
        for pair in sorted.windows(2) {
            assert!(
                pair[1].duration_since(pair[0]) >= std::time::Duration::from_millis(3000),
                "launches must be at least stagger_ms apart: {:?}",
                sorted
            );
        }
    }
}
