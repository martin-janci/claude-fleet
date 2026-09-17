//! The background reconcile tick started from Tauri setup, and the
//! independent account-usage poll loop started beside it.

use crate::events::EventBus;
use crate::service::account_usage::UsageCache;
use crate::service::account_usage_poll::{self, AccountUsagePoller};
use crate::ssh::SshExec;
use crate::store::{HostRow, Store};
use crate::{service, ssh};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Cadence of the account-usage poll loop (Task 4). Independent of
/// `reconcile.interval_secs` — which may be `0`, disabling the reconcile
/// tick entirely — because `service::account_usage`'s own 5-minute-per-account
/// floor already caps real requests; this loop only decides how often to ASK
/// whether an account is due.
const USAGE_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Spawn the proactive background reconcile loop (Task H). The interval is read
/// once at startup from the `reconcile.interval_secs` setting; `0` disables the
/// tick entirely (reconcile then stays pull-only). Overlap is prevented by the
/// process-wide `service::sessions::ReconcileGate` shared with `list_sessions`
/// (Tauri command + MCP tool): a tick that fires while ANY caller's pass is
/// still running is skipped rather than queued.
pub(crate) fn spawn_reconcile_tick(
    store: std::sync::Arc<Mutex<Store>>,
    ssh: std::sync::Arc<ssh::SshClient>,
) {
    let interval_secs = {
        let raw = store
            .lock()
            .ok()
            .and_then(|s| s.get_setting("reconcile.interval_secs").ok().flatten());
        service::sessions::read_reconcile_interval_secs(raw)
    };
    let Some(period) = service::sessions::reconcile_tick_interval(interval_secs) else {
        tracing::info!("reconcile tick disabled (reconcile.interval_secs={interval_secs})");
        return;
    };
    tracing::info!("reconcile tick enabled every {}s", period.as_secs());

    fleet_core::rt::spawn(async move {
        let mut ticker = tokio::time::interval(period);
        // Drop missed ticks rather than firing them back-to-back after a slow
        // pass (the default Burst behaviour would defeat the overlap guard).
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            match service::sessions::reconcile_now(&store, &ssh).await {
                Ok(true) => {}
                Ok(false) => {
                    tracing::debug!("a reconcile pass is already running; skipping tick")
                }
                Err(e) => tracing::warn!("reconcile tick: reconcile failed: {e}"),
            }
            // Wave 2 Track D: lifecycle automation rides the same tick, after
            // the pass so it sees fresh `stuck_kind` / `idle_since` stamps.
            // Both are opt-in through settings and cheap when off. Their
            // work is best-effort: a failure is logged inside and never
            // stops the loop.
            let n = service::playbooks::run(&store, &ssh).await;
            if n > 0 {
                tracing::info!("reconcile tick: applied {n} stuck playbook(s)");
            }
            let _ = service::gc::maybe_sweep(&store, &ssh).await;
            // Wave 5 G1: per-session token usage, once per usage.interval_secs,
            // in its own single-flight task so a slow host never stalls the tick.
            service::usage::spawn_collect(&store, &ssh);
            // Wave 3 Track E: fail open tasks whose worker died or that
            // outlived `tasks.max_age_secs` (also swept by list/wait calls).
            if let Ok(s) = store.lock() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                match service::tasks::sweep_open_tasks(&s, now) {
                    Ok(failed) if !failed.is_empty() => {
                        tracing::info!("reconcile tick: failed {} stale task(s)", failed.len())
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("reconcile tick: task sweep failed: {e}"),
                }
            }
            // Opt-in (`repair.auto_on_tick`): re-add vanished worktrees with
            // the create-only automatic policy. Detached and rate-limited.
            service::repair_tick::maybe_run(&store, &ssh);
            // Drop remote worktree rows whose checkout is gone and no longer
            // registered with git (removed without ExitWorktree). One
            // read-only probe per reachable host; detached and rate-limited.
            service::worktree_prune::maybe_run(&store, &ssh);
        }
    });
}

/// Spawn the independent account-usage poll loop (Task 4). Ticks every
/// [`USAGE_POLL_INTERVAL`] regardless of the reconcile tick's own interval
/// (or whether it is disabled): each tick reads the current host rows and
/// hands them to [`account_usage_poll::poll_due_accounts`], which spawns a
/// bounded, de-duplicated fetch per due account and returns immediately — a
/// slow or hung host can delay neither this loop's next tick nor the
/// reconcile tick, which does not touch usage at all.
pub(crate) fn spawn_account_usage_tick(
    store: Arc<Mutex<Store>>,
    ssh: Arc<ssh::SshClient>,
    cache: Arc<Mutex<UsageCache>>,
    bus: Arc<dyn EventBus>,
) {
    let poller = Arc::new(AccountUsagePoller::new());
    fleet_core::rt::spawn(async move {
        let mut ticker = tokio::time::interval(USAGE_POLL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let hosts: Vec<HostRow> = match store.lock() {
                Ok(s) => match s.list_hosts() {
                    Ok(h) => h,
                    Err(e) => {
                        tracing::warn!("account usage tick: list_hosts failed: {e}");
                        continue;
                    }
                },
                Err(e) => {
                    tracing::warn!("account usage tick: store mutex poisoned: {e}");
                    continue;
                }
            };
            let ssh_dyn: Arc<dyn SshExec> = Arc::clone(&ssh) as Arc<dyn SshExec>;
            // Fire-and-forget: the handles are only useful to tests, which
            // call `account_usage_poll::poll_due_accounts` directly.
            let _handles = account_usage_poll::poll_due_accounts(
                &poller,
                Arc::new(hosts),
                ssh_dyn,
                Arc::clone(&cache),
                Arc::clone(&bus),
            );
        }
    });
}
