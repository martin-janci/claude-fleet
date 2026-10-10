//! The background reconcile tick started from Tauri setup, and the
//! independent account-usage poll loop started beside it.

use crate::events::EventBus;
use crate::service::account_usage::UsageCache;
use crate::service::account_usage_poll::{self, AccountUsagePoller};
use crate::ssh::SshExec;
use crate::store::{HostRow, Store};
use crate::{service, ssh};
use futures_util::FutureExt as _;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Cadence of the account-usage poll loop (Task 4). Independent of
/// `reconcile.interval_secs` — which may be `0`, disabling the reconcile
/// tick entirely — because `service::account_usage`'s own 5-minute-per-account
/// floor already caps real requests; this loop only decides how often to ASK
/// whether an account is due.
const USAGE_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// The last reconcile pass, as `fleet_health.hub.reconcile` reports it
/// (perf-logs §5: tick failures were `warn!` lines and nothing else).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReconcileStats {
    #[serde(default)]
    pub last_started_at: Option<i64>,
    #[serde(default)]
    pub last_finished_at: Option<i64>,
    #[serde(default)]
    pub last_duration_ms: Option<i64>,
    /// When the last CLEAN pass finished; a later failure leaves it alone,
    /// so it latches "a pass of this process has succeeded" (the readiness
    /// file's `first_reconcile`, update-channel design §8.4).
    #[serde(default)]
    pub last_ok_at: Option<i64>,
    /// Failed passes since the last good one.
    #[serde(default)]
    pub consecutive_failures: u32,
    /// Failed passes since the process started; never resets.
    #[serde(default)]
    pub failures_total: u64,
    #[serde(default)]
    pub last_error: Option<String>,
}

/// What `fleet_health.hub` and the `/metrics` reconcile gauges read: when
/// this process started and how its last reconcile pass went, written by
/// the tick loop. One per process (`tick_stats()`), like `reconcile_gate()`.
pub struct TickStats {
    started: std::time::Instant,
    started_at: i64,
    reconcile: Mutex<ReconcileStats>,
}

impl TickStats {
    pub fn new(now: i64) -> Self {
        Self {
            started: std::time::Instant::now(),
            started_at: now,
            reconcile: Mutex::new(ReconcileStats::default()),
        }
    }

    /// Stamp the start of a pass; the `Instant` goes back into [`finish`](Self::finish).
    pub fn begin(&self, now: i64) -> std::time::Instant {
        if let Ok(mut r) = self.reconcile.lock() {
            r.last_started_at = Some(now);
        }
        std::time::Instant::now()
    }

    /// `Ok(true)`: a pass ran. `Ok(false)`: skipped, another pass was
    /// running — nothing but the start stamp changes. `Err`: the pass failed.
    pub fn finish(&self, started: std::time::Instant, now: i64, outcome: Result<bool, String>) {
        let Ok(mut r) = self.reconcile.lock() else {
            return;
        };
        let duration_ms = started.elapsed().as_millis() as i64;
        match outcome {
            Ok(false) => {}
            Ok(true) => {
                r.last_finished_at = Some(now);
                r.last_ok_at = Some(now);
                r.last_duration_ms = Some(duration_ms);
                r.consecutive_failures = 0;
                r.last_error = None;
            }
            Err(e) => {
                r.last_finished_at = Some(now);
                r.last_duration_ms = Some(duration_ms);
                r.consecutive_failures += 1;
                r.failures_total += 1;
                r.last_error = Some(e);
            }
        }
    }

    pub fn uptime_secs(&self) -> i64 {
        self.started.elapsed().as_secs() as i64
    }

    pub fn started_at(&self) -> i64 {
        self.started_at
    }

    pub fn reconcile(&self) -> ReconcileStats {
        self.reconcile.lock().map(|r| r.clone()).unwrap_or_default()
    }
}

/// The process-wide stats, created on first use. `fleet-hub serve` touches
/// it before the ticks start so `started_at` is the serve start.
pub fn tick_stats() -> Arc<TickStats> {
    static STATS: std::sync::LazyLock<Arc<TickStats>> =
        std::sync::LazyLock::new(|| Arc::new(TickStats::new(unix_now())));
    Arc::clone(&STATS)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Runs `on_tick` once per tick of `ticker`, checking `token` for
/// cancellation only BETWEEN passes (issue #144): a pass already running
/// when `token` is cancelled runs to completion, and only then does the loop
/// exit — it never starts another pass afterward. `biased` makes an
/// already-cancelled token win over an already-elapsed tick, so cancelling
/// before the loop ever runs means `on_tick` is never called at all. Shared
/// by [`spawn_reconcile_tick`], [`spawn_account_usage_tick`] and the mission
/// and routine loops (review r06 F7) so this
/// behaviour — and its test coverage below — lives in one place.
///
/// A pass that panics is caught and logged, and the loop goes on to the next
/// tick: uncaught, the panic ended the spawned task, whose `JoinHandle` is
/// read only at shutdown, so reconcile stopped for good while the process
/// (and its readiness heartbeat) stayed up.
pub(crate) async fn run_cancellable_tick<F, Fut>(
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
        if AssertUnwindSafe(on_tick()).catch_unwind().await.is_err() {
            tracing::error!("tick pass panicked; continuing with the next tick");
        }
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

    // J5 quick answer: after each pass, new agent questions are asked about
    // in a task of their own (off by default; one short lock when off).
    let quick = service::decide::quick_answer::QuickAnswerTrigger::new(
        service::decide::DecideCtx::jev(std::sync::Arc::clone(&store)),
    );
    Some(crate::rt::spawn(async move {
        let mut ticker = tokio::time::interval(period);
        // Drop missed ticks rather than firing them back-to-back after a slow
        // pass (the default Burst behaviour would defeat the overlap guard).
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        run_cancellable_tick(ticker, token, || {
            let store = &store;
            let quick = &quick;
            let ssh = &ssh;
            async move {
                let stats = tick_stats();
                let started = stats.begin(unix_now());
                let outcome = service::sessions::reconcile_now(store, ssh).await;
                match &outcome {
                    Ok(true) => {
                        quick.after_pass();
                    }
                    Ok(false) => {
                        tracing::debug!("a reconcile pass is already running; skipping tick")
                    }
                    Err(e) => tracing::warn!("reconcile tick: reconcile failed: {e}"),
                }
                let outcome = outcome.map_err(|e| e.to_string());
                service::loops::report("reconcile", outcome.clone().map(|_| ()), Some(period));
                stats.finish(started, unix_now(), outcome);
                // Lifecycle F2: a `working` row nothing has moved for
                // `reconcile.stale_working_secs` becomes `idle` (+ the
                // `stale_working` attention reason), before the playbooks
                // read the fresh statuses.
                let stale = service::sessions::age_out_stale_working(store);
                if stale > 0 {
                    tracing::info!("reconcile tick: {stale} stale working row(s) demoted");
                }
                // …and its stamp lifts once it asks nothing: working or
                // blocked again, or older than `reconcile.stale_working_ttl_secs`.
                let lifted = service::sessions::expire_stale_working(store);
                if lifted > 0 {
                    tracing::debug!("reconcile tick: {lifted} stale working stamp(s) lifted");
                }
                service::loops::report("stale_working", Ok::<_, String>(()), Some(period));
                // Chat forms: expire unanswered ones, drop old rows, and
                // remove secret files nobody needs from their hosts.
                let now = unix_now();
                let forms = service::forms::expire_and_purge(store, now);
                let swept = service::forms::sweep_secret_dirs(store, &**ssh, now).await;
                if forms + swept > 0 {
                    tracing::debug!(
                        "reconcile tick: {forms} form row(s) aged, {swept} secret dir(s) removed"
                    );
                }
                service::loops::report("forms", Ok::<_, String>(()), Some(period));
                // Wave 2 Track D: lifecycle automation rides the same tick, after
                // the pass so it sees fresh `stuck_kind` / `idle_since` stamps.
                // Both are opt-in through settings and cheap when off. Their
                // work is best-effort: a failure is logged inside and never
                // stops the loop.
                if service::loops::gate("playbooks", store, Some(period)) {
                    let n = service::playbooks::run(store, ssh).await;
                    if n > 0 {
                        tracing::info!("reconcile tick: applied {n} stuck playbook(s)");
                    }
                    service::loops::report("playbooks", Ok::<_, String>(()), Some(period));
                }
                // PR shepherd: projects a person granted a rule for get their
                // sessions' conflicting or red PRs recorded, and nudged at
                // `nudge`. No rule, no work: one indexed read and out. Stops
                // while `automation.paused` is on.
                // Detached and single-flight: its `gh` calls must not hold the tick.
                if service::loops::gate("pr_shepherd", store, Some(period)) {
                    service::pr_shepherd::spawn_run(store, ssh, period);
                }
                // Step 5.10: a prompt queued for a busy session goes in once the
                // session is idle; the Stop hook delivers it first, this catches
                // a hook that never came. Detached and single-flight.
                service::sessions::deferred::spawn_deliver_all_due(store, ssh);
                // A background agent's "stop after" limits (migration 149):
                // stopped once past its deadline or spend cap. Detached and
                // single-flight; one indexed read when none are live.
                service::bg_sessions::spawn_enforce_stop_limits(store, ssh);
                // Wave 5 G-hub-latency Task 1: single-flight, off the tick
                // body — a slow sweep (it does SSH work) must not stretch
                // the tick past its period. Mirrors `service::usage::spawn_collect`.
                service::gc::spawn_sweep(store, ssh);
                // Wave 5 G1: per-session token usage, once per usage.interval_secs,
                // in its own single-flight task so a slow host never stalls the tick.
                service::usage::spawn_collect(store, ssh);
                // Search phase 3: conversation text into the search index,
                // only with `search.index_transcripts` on (off by default).
                service::search_index::spawn_index(store, ssh);
                // Wave 3 Track E: fail open tasks whose worker died or that
                // outlived `tasks.max_age_secs` (also swept by list/wait calls).
                if let Ok(s) = store.lock() {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    let swept = service::tasks::sweep_open_tasks(&s, now);
                    drop(s);
                    match &swept {
                        Ok(failed) if !failed.is_empty() => {
                            tracing::info!("reconcile tick: failed {} stale task(s)", failed.len())
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!("reconcile tick: task sweep failed: {e}"),
                    }
                    service::loops::report("tasks", swept.map(|_| ()), Some(period));
                }
                // Error reports (spec 2026-09-21): the hub's own ERROR events
                // join the table, and rows past `reports.max_age_secs` go.
                {
                    let now = fleet_proto::report::now_unix();
                    let _ = service::reports::drain_own_ring(store, now);
                    let swept = service::reports::sweep_by_age(store, now);
                    if swept > 0 {
                        tracing::info!("reconcile tick: swept {swept} old error report(s)");
                    }
                    service::loops::report("reports", Ok::<_, String>(()), Some(period));
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
    // The last-known usage from before a restart, before the first poll.
    let restored = account_usage_poll::restore_usage(&store, &cache);
    // …and the limits in it, for `needs_attention`'s `account_limit` before
    // the first poll answers (step 2.6).
    bus.attention_seeded(&[], &restored);
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
                let next = Some(USAGE_POLL_INTERVAL);
                let hosts: Vec<HostRow> = match store.lock() {
                    Ok(s) => match s.list_hosts() {
                        // One hidden/local rule for every host loop (hub-ops F6).
                        Ok(h) => crate::service::hosts::active_hosts(
                            h,
                            crate::service::hub::local_host_enabled(),
                        ),
                        Err(e) => {
                            tracing::warn!("account usage tick: list_hosts failed: {e}");
                            service::loops::report("account_usage", Err(e), next);
                            return;
                        }
                    },
                    Err(e) => {
                        tracing::warn!("account usage tick: store mutex poisoned: {e}");
                        service::loops::report("account_usage", Err(e.to_string()), next);
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
                    Some(Arc::clone(store)),
                );
                service::loops::report("account_usage", Ok::<_, String>(()), next);
            }
        })
        .await;
    })
}

/// The clock a lease is stamped with: read when the lease is taken, not
/// the pass's start (review r06), so a routine or mission reached late in a long pass
/// does not get a lease that is already near its end. Never before `now`,
/// the pass's own clock.
pub fn lease_clock(now: i64) -> i64 {
    now.max(crate::store::now_unix())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Review r06: a lease is stamped with the clock when it is taken, not
    /// the pass's start, and never before the pass's own clock.
    #[test]
    fn a_lease_is_stamped_with_a_fresh_clock() {
        let pass_start = crate::store::now_unix() - 600;
        assert!(super::lease_clock(pass_start) >= pass_start + 600);
        let ahead = crate::store::now_unix() + 3600;
        assert_eq!(super::lease_clock(ahead), ahead);
    }

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

    /// A pass that panics must not end the loop: the next tick runs another.
    #[tokio::test(start_paused = true)]
    async fn a_panicking_pass_does_not_stop_the_loop() {
        let token = CancellationToken::new();
        let ticker = tokio::time::interval(Duration::from_millis(1));
        let ran = Arc::new(AtomicUsize::new(0));
        let ran2 = Arc::clone(&ran);
        let token2 = token.clone();
        run_cancellable_tick(ticker, token, move || {
            let ran = Arc::clone(&ran2);
            let token = token2.clone();
            async move {
                if ran.fetch_add(1, Ordering::SeqCst) == 0 {
                    panic!("first pass fails");
                }
                token.cancel();
            }
        })
        .await;
        assert_eq!(
            ran.load(Ordering::SeqCst),
            2,
            "the pass after the panicking one must still run"
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

    #[test]
    fn tick_stats_track_the_last_pass_and_consecutive_failures() {
        let t = TickStats::new(1_000);
        let s = t.begin(1_010);
        t.finish(s, 1_011, Ok(true));
        let r = t.reconcile();
        assert_eq!(
            (
                r.last_started_at,
                r.last_finished_at,
                r.consecutive_failures
            ),
            (Some(1_010), Some(1_011), 0)
        );
        assert!(r.last_duration_ms.is_some());
        for at in [1_030, 1_050] {
            let s = t.begin(at);
            t.finish(s, at + 1, Err("E_SSH: boom".into()));
        }
        let r = t.reconcile();
        assert_eq!(
            (
                r.consecutive_failures,
                r.failures_total,
                r.last_error.as_deref()
            ),
            (2, 2, Some("E_SSH: boom"))
        );
        let s = t.begin(1_070);
        t.finish(s, 1_071, Ok(false));
        assert_eq!(
            t.reconcile().consecutive_failures,
            2,
            "a skipped tick is not a pass"
        );
        let s = t.begin(1_090);
        t.finish(s, 1_091, Ok(true));
        assert_eq!(
            t.reconcile().consecutive_failures,
            0,
            "a good pass clears the streak"
        );
        assert_eq!(t.reconcile().last_error, None);
        assert_eq!(t.reconcile().failures_total, 2, "the total never resets");
        assert_eq!(t.started_at(), 1_000);
        assert!(t.uptime_secs() >= 0);
    }

    /// `last_ok_at` latches the last clean pass: a failure or a skipped
    /// tick after it leaves it where it was.
    #[test]
    fn last_ok_at_moves_only_on_a_clean_pass() {
        let t = TickStats::new(1_000);
        let s = t.begin(1_005);
        t.finish(s, 1_006, Err("E_SSH: boom".into()));
        assert_eq!(t.reconcile().last_ok_at, None, "a failure is not ok");
        let s = t.begin(1_010);
        t.finish(s, 1_011, Ok(true));
        assert_eq!(t.reconcile().last_ok_at, Some(1_011));
        let s = t.begin(1_020);
        t.finish(s, 1_021, Err("E_SSH: boom".into()));
        let r = t.reconcile();
        assert_eq!(r.last_ok_at, Some(1_011), "a failure keeps the latch");
        assert_eq!(r.last_finished_at, Some(1_021));
        let s = t.begin(1_030);
        t.finish(s, 1_031, Ok(false));
        assert_eq!(
            t.reconcile().last_ok_at,
            Some(1_011),
            "a skip is not a pass"
        );
    }
}
