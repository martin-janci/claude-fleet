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
use tokio_util::sync::CancellationToken;

/// Cadence of the account-usage poll loop (Task 4). Independent of
/// `reconcile.interval_secs` — which may be `0`, disabling the reconcile
/// tick entirely — because `service::account_usage`'s own 5-minute-per-account
/// floor already caps real requests; this loop only decides how often to ASK
/// whether an account is due.
const USAGE_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Runs `on_tick` once per tick of `ticker`, checking `token` for
/// cancellation only BETWEEN passes (issue #144): a pass already running
/// when `token` is cancelled runs to completion, and only then does the loop
/// exit — it never starts another pass afterward. `biased` makes an
/// already-cancelled token win over an already-elapsed tick, so cancelling
/// before the loop ever runs means `on_tick` is never called at all. Shared
/// by [`spawn_reconcile_tick`] and [`spawn_account_usage_tick`] so this
/// behaviour — and its test coverage below — lives in one place.
async fn run_cancellable_tick<F, Fut>(
    mut ticker: tokio::time::Interval,
    token: CancellationToken,
    mut on_tick: F,
) where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => {
                tracing::debug!("tick loop: cancellation requested; exiting without starting another pass");
                break;
            }
            _ = ticker.tick() => {}
        }
        on_tick().await;
    }
}

/// Spawn the proactive background reconcile loop (Task H). The interval is read
/// once at startup from the `reconcile.interval_secs` setting; `0` disables the
/// tick entirely (reconcile then stays pull-only) and `None` is returned —
/// nothing is spawned. Overlap is prevented by the process-wide
/// `service::sessions::ReconcileGate` shared with `list_sessions` (Tauri
/// command + MCP tool): a tick that fires while ANY caller's pass is still
/// running is skipped rather than queued.
///
/// `token` is cancelled to stop the loop (issue #144): cancellation is only
/// observed between passes, so a pass already running finishes before the
/// loop exits. The desktop, which does not await this handle on shutdown,
/// passes a token that is never cancelled so its behaviour is unchanged;
/// `fleet-hub serve` cancels a real one and awaits the returned handle,
/// bounded, before tearing down the SSH masters the pass might still be using.
pub fn spawn_reconcile_tick(
    store: std::sync::Arc<Mutex<Store>>,
    ssh: std::sync::Arc<ssh::SshClient>,
    token: CancellationToken,
) -> Option<tokio::task::JoinHandle<()>> {
    let interval_secs = {
        let raw = store
            .lock()
            .ok()
            .and_then(|s| s.get_setting("reconcile.interval_secs").ok().flatten());
        service::sessions::read_reconcile_interval_secs(raw)
    };
    let Some(period) = service::sessions::reconcile_tick_interval(interval_secs) else {
        tracing::info!("reconcile tick disabled (reconcile.interval_secs={interval_secs})");
        return None;
    };
    tracing::info!("reconcile tick enabled every {}s", period.as_secs());

    Some(crate::rt::spawn(async move {
        let mut ticker = tokio::time::interval(period);
        // Drop missed ticks rather than firing them back-to-back after a slow
        // pass (the default Burst behaviour would defeat the overlap guard).
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        run_cancellable_tick(ticker, token, || {
            let store = &store;
            let ssh = &ssh;
            async move {
                match service::sessions::reconcile_now(store, ssh).await {
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
                let n = service::playbooks::run(store, ssh).await;
                if n > 0 {
                    tracing::info!("reconcile tick: applied {n} stuck playbook(s)");
                }
                let _ = service::gc::maybe_sweep(store, ssh).await;
                // Wave 5 G1: per-session token usage, once per usage.interval_secs,
                // in its own single-flight task so a slow host never stalls the tick.
                service::usage::spawn_collect(store, ssh);
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
                service::repair_tick::maybe_run(store, ssh);
                // Drop remote worktree rows whose checkout is gone and no longer
                // registered with git (removed without ExitWorktree). One
                // read-only probe per reachable host; detached and rate-limited.
                service::worktree_prune::maybe_run(store, ssh);
            }
        })
        .await;
    }))
}

/// Spawn the independent account-usage poll loop (Task 4). Ticks every
/// [`USAGE_POLL_INTERVAL`] regardless of the reconcile tick's own interval
/// (or whether it is disabled): each tick reads the current host rows and
/// hands them to [`account_usage_poll::poll_due_accounts`], which spawns a
/// bounded, de-duplicated fetch per due account and returns immediately — a
/// slow or hung host can delay neither this loop's next tick nor the
/// reconcile tick, which does not touch usage at all.
///
/// `token` behaves exactly as it does for [`spawn_reconcile_tick`]: cancelled
/// only between passes, so an in-flight tick (which itself only reads rows
/// and fires off detached fetches — see above) finishes before the loop exits.
pub fn spawn_account_usage_tick(
    store: Arc<Mutex<Store>>,
    ssh: Arc<ssh::SshClient>,
    cache: Arc<Mutex<UsageCache>>,
    bus: Arc<dyn EventBus>,
    token: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let poller = Arc::new(AccountUsagePoller::new());
    crate::rt::spawn(async move {
        let mut ticker = tokio::time::interval(USAGE_POLL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        run_cancellable_tick(ticker, token, || {
            let store = &store;
            let ssh = &ssh;
            let cache = &cache;
            let bus = &bus;
            let poller = &poller;
            async move {
                let hosts: Vec<HostRow> = match store.lock() {
                    Ok(s) => match s.list_hosts() {
                        Ok(h) => h,
                        Err(e) => {
                            tracing::warn!("account usage tick: list_hosts failed: {e}");
                            return;
                        }
                    },
                    Err(e) => {
                        tracing::warn!("account usage tick: store mutex poisoned: {e}");
                        return;
                    }
                };
                let ssh_dyn: Arc<dyn SshExec> = Arc::clone(ssh) as Arc<dyn SshExec>;
                // Fire-and-forget: the handles are only useful to tests, which
                // call `account_usage_poll::poll_due_accounts` directly.
                let _handles = account_usage_poll::poll_due_accounts(
                    poller,
                    Arc::new(hosts),
                    ssh_dyn,
                    Arc::clone(cache),
                    Arc::clone(bus),
                );
            }
        })
        .await;
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// (a) A token cancelled before the loop ever runs must exit without
    /// starting a single pass — not even the one a freshly constructed
    /// `tokio::time::interval` fires immediately.
    #[tokio::test(start_paused = true)]
    async fn a_cancelled_token_exits_without_starting_a_pass() {
        let token = CancellationToken::new();
        token.cancel();
        let ticker = tokio::time::interval(Duration::from_secs(60));
        let ran = Arc::new(AtomicUsize::new(0));
        let ran2 = Arc::clone(&ran);
        run_cancellable_tick(ticker, token, move || {
            let ran = Arc::clone(&ran2);
            async move {
                ran.fetch_add(1, Ordering::SeqCst);
            }
        })
        .await;
        assert_eq!(
            ran.load(Ordering::SeqCst),
            0,
            "a pre-cancelled token must not let a pass start"
        );
    }

    /// (b) Cancelling WHILE a pass is running must let that pass finish
    /// before the loop exits — the exact promise
    /// docs/superpowers/specs/2026-09-17-hub-daemon-design.md makes for
    /// SIGTERM during reconcile.
    #[tokio::test(start_paused = true)]
    async fn cancelling_during_a_pass_lets_it_finish_then_exits() {
        let token = CancellationToken::new();
        let ticker = tokio::time::interval(Duration::from_millis(1));
        let completed = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(tokio::sync::Notify::new());
        let proceed = Arc::new(tokio::sync::Notify::new());

        let completed2 = Arc::clone(&completed);
        let started2 = Arc::clone(&started);
        let proceed2 = Arc::clone(&proceed);
        let token2 = token.clone();

        let handle = tokio::spawn(async move {
            run_cancellable_tick(ticker, token2, move || {
                let completed = Arc::clone(&completed2);
                let started = Arc::clone(&started2);
                let proceed = Arc::clone(&proceed2);
                async move {
                    // Signal the pass has started, then block until the test
                    // lets it continue — this is the seam that lets the test
                    // cancel the token WHILE a pass is in flight.
                    started.notify_one();
                    proceed.notified().await;
                    completed.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await;
        });

        // Wait for the first (and only) pass to start.
        started.notified().await;
        // Cancel while it is still running.
        token.cancel();
        assert_eq!(
            completed.load(Ordering::SeqCst),
            0,
            "the pass must not have completed yet"
        );
        // Let the in-flight pass finish.
        proceed.notify_one();
        // The loop must exit on its own (no second pass, since the ticker is
        // 1ms and would otherwise fire many more times before this returns).
        handle.await.unwrap();
        assert_eq!(
            completed.load(Ordering::SeqCst),
            1,
            "the in-flight pass must have completed exactly once, and no more"
        );
    }
}
