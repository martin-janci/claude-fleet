//! The background reconcile tick started from Tauri setup.

use crate::store::Store;
use crate::{service, ssh};
use std::sync::Mutex;

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

    // `tauri::async_runtime::spawn`, NOT bare `tokio::spawn`: this runs from the
    // Tauri `setup` closure on the main thread (inside the macOS
    // `did_finish_launching` callback), where no tokio runtime is entered. A
    // bare `tokio::spawn` there panics ("no reactor running"), and because the
    // callback can't unwind the panic aborts the process. The Tauri runtime
    // handle works from any context (same reason the MCP server uses it).
    tauri::async_runtime::spawn(async move {
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
