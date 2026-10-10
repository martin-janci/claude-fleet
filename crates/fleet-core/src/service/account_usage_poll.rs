//! Task 4 glue for `service::account_usage` (security-reviewed and untouched
//! here): decides WHEN to fetch an account's usage and WHERE the result
//! goes. `account_usage.rs` owns the fetch itself (the script, the parser,
//! the floor/backoff cache) and is not modified by this module.
//!
//! Three jobs:
//! - [`AccountUsagePoller`] + [`poll_due_accounts`]: the background poll,
//!   called from `service::tick`'s independent usage-poll loop. Bounded to
//!   [`MAX_CONCURRENT_FETCHES`] concurrent fetches and de-duplicated so an
//!   account already being fetched is never spawned twice. Spawning is
//!   fire-and-forget — the caller (a tick loop) is never blocked by a slow
//!   or hung host.
//! - [`list_account_usage`] / [`refresh_account_usage`]: the read-only and
//!   floor-respecting-refresh commands, thin enough to share `fetch_and_emit`
//!   with the poller so both paths emit the same event under the same rule.
//! - [`fetch_and_emit`]: fetch, then emit `EventBus::account_usage_updated`
//!   only when the snapshot actually changed — a call the floor turns away
//!   returns the same snapshot, so it never emits. A NEW successful answer is
//!   also written to `account_usage_snapshots` (redesign step 2.5), which
//!   [`restore_usage`] seeds the cache from after a restart.

use crate::events::EventBus;
use crate::ipc_error::lock;
use crate::ipc_error::{codes, IpcError};
use crate::service::account_usage::{self, AccountUsageSnapshot, UsageCache};
use crate::ssh::SshExec;
use crate::store::{HostRow, Store, UsageSnapshotRow};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::Semaphore;

/// At most this many usage fetches run at once, across every account.
const MAX_CONCURRENT_FETCHES: usize = 2;

fn lock_cache(cache: &Mutex<UsageCache>) -> std::sync::MutexGuard<'_, UsageCache> {
    cache.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Distinct non-null `account_uuid`s across `hosts`, first-seen order (the
/// poller's account list — see the module doc on why this differs from
/// [`list_account_usage`]'s source). An API-key account (M15 step G2.9)
/// is left out: it has no usage windows to fetch, only spend.
pub(crate) fn distinct_account_uuids(hosts: &[HostRow]) -> Vec<String> {
    let mut seen = HashSet::new();
    // A host's own login first, then its login profiles' (docs/accounts.md).
    hosts
        .iter()
        .filter_map(|h| h.account_uuid.clone())
        .chain(
            hosts
                .iter()
                .flat_map(|h| h.claude_profiles.iter().flatten())
                .filter_map(|p| p.account_uuid.clone()),
        )
        .filter(|u| !crate::service::add_account::is_api_key_account(u))
        .filter(|u| seen.insert(u.clone()))
        .collect()
}

/// Bounded concurrency + in-flight de-duplication for the background poller.
/// One instance lives for the app's lifetime, shared across every tick of
/// `service::tick`'s usage-poll loop.
///
/// The in-flight set is a scheduling optimisation, not the correctness
/// backstop: `account_usage::UsageCache` itself reserves an attempt (bumps
/// `next_try`) under its own lock as soon as it decides to proceed, so even a
/// genuine race here could never cause two real SSH requests for the same
/// account inside the floor. This set exists so a second call within the
/// same tick (or a slow fetch still waiting on a semaphore permit, which the
/// cache has not reserved yet) does not spawn a redundant task.
pub(crate) struct AccountUsagePoller {
    in_flight: Mutex<HashSet<String>>,
    limit: Arc<Semaphore>,
    /// The reset time of each account's limit as the last pass saw it, for
    /// [`reannounce_lapsed`].
    limits: Mutex<HashMap<String, i64>>,
}

impl AccountUsagePoller {
    pub(crate) fn new() -> Self {
        Self {
            in_flight: Mutex::new(HashSet::new()),
            limit: Arc::new(Semaphore::new(MAX_CONCURRENT_FETCHES)),
            limits: Mutex::new(HashMap::new()),
        }
    }
}

/// PURE: whether a limit that reset at `was_reset` has lapsed with no new
/// reading to say so: the facts no longer hold the account limited, and the
/// last answer (`fetched_at`) is from before the reset. A reading after it
/// went through [`fetch_and_emit`], which announced the move itself.
fn limit_lapsed(was_reset: Option<i64>, limited_now: bool, fetched_at: Option<i64>) -> bool {
    match was_reset {
        Some(r) => !limited_now && fetched_at.is_none_or(|f| f < r),
        None => false,
    }
}

/// Review r05 F9: an account's limit that resets between two readings
/// (a host down, or simply before the next poll) changes no snapshot, so
/// [`fetch_and_emit`] never re-announces its sessions, and a phone keeps the
/// `account_limit` stamp on them. Each pass compares the bus's facts with
/// the reset it saw last and re-announces the account's live sessions once
/// the reset has passed.
fn reannounce_lapsed(
    poller: &AccountUsagePoller,
    account: &str,
    bus: &dyn EventBus,
    cache: &Mutex<UsageCache>,
    store: Option<&Mutex<Store>>,
) {
    let limit = bus.attention_facts().limited_accounts.get(account).copied();
    let was_reset = {
        let mut limits = poller.limits.lock().unwrap_or_else(PoisonError::into_inner);
        match limit.and_then(|l| l.resets_at) {
            Some(r) => limits.insert(account.to_string(), r),
            None => limits.remove(account),
        }
    };
    let fetched_at = lock_cache(cache).snapshot(account).fetched_at;
    if !limit_lapsed(was_reset, limit.is_some(), fetched_at) {
        return;
    }
    let Some(store) = store else { return };
    let sent = match store.lock() {
        Ok(s) => s
            .reemit_live_sessions_on_account(account)
            .map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    if let Err(e) = sent {
        tracing::warn!(account = %account, "re-announcing the account's sessions failed: {e}");
    }
}

impl Default for AccountUsagePoller {
    fn default() -> Self {
        Self::new()
    }
}

/// Fetch `account_uuid`'s usage and, only if the snapshot actually changed,
/// emit `EventBus::account_usage_updated`. Shared by the poller and
/// `refresh_account_usage` so both follow the same "emit iff changed" rule
/// (a call the floor turns away returns the same snapshot untouched, so nothing
/// is emitted).
async fn fetch_and_emit(
    account_uuid: &str,
    hosts: &[HostRow],
    ssh: &dyn SshExec,
    cache: &Mutex<UsageCache>,
    bus: &dyn EventBus,
    store: Option<&Mutex<Store>>,
) -> AccountUsageSnapshot {
    let before = lock_cache(cache).snapshot(account_uuid);
    let after =
        account_usage::fetch_account_usage_with(account_uuid, hosts, ssh, cache, false).await;
    if let Some(store) = store {
        persist_if_new(store, &before, &after);
    }
    if after.is_newsworthy_change(&before) {
        let was = account_blocked(bus, account_uuid);
        bus.account_usage_updated(&after);
        // The hub stamps `needs_attention` onto session frames only, so an
        // account moving into or out of a limit or a lost login re-announces
        // its live sessions, or a phone following the stream keeps the old
        // stamp on an idle one (review r05 F9). Only a bus that follows the
        // facts (the hub's) can see a move; every other bus knows none.
        if let Some(store) = store {
            if account_blocked(bus, account_uuid) != was {
                let sent = match store.lock() {
                    Ok(s) => s
                        .reemit_live_sessions_on_account(account_uuid)
                        .map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                };
                if let Err(e) = sent {
                    tracing::warn!(account = %account_uuid, "re-announcing the account's sessions failed: {e}");
                }
            }
        }
    }
    after
}

/// Whether `bus`'s facts hold `account` at a limit or without a login: the
/// two account inputs of a session's `Blocked` reasons.
fn account_blocked(bus: &dyn EventBus, account: &str) -> (bool, bool) {
    let f = bus.attention_facts();
    (
        f.limited_accounts.contains_key(account),
        f.uncredentialed_accounts.contains(account),
    )
}

/// Write `after`'s usage to the history when it is a new successful answer
/// (its `fetched_at` moved). A failure is logged, never surfaced: the cache
/// already holds the answer, and history is a convenience on top of it.
fn persist_if_new(
    store: &Mutex<Store>,
    before: &AccountUsageSnapshot,
    after: &AccountUsageSnapshot,
) {
    let (Some(usage), Some(fetched_at)) = (&after.usage, after.fetched_at) else {
        return;
    };
    if before.fetched_at == Some(fetched_at) {
        return;
    }
    let row = UsageSnapshotRow {
        account_uuid: after.account_uuid.clone(),
        fetched_at,
        usage: usage.clone(),
        subscription: after.subscription.clone(),
        source_host: after.source_host.clone(),
    };
    let written = match store.lock() {
        Ok(s) => s.insert_usage_snapshot(&row).map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    if let Err(e) = written {
        tracing::warn!(account = %row.account_uuid, "account usage history: write failed: {e}");
    }
}

/// Seed `cache` with each account's newest stored answer, so a restart keeps
/// the last-known usage (redesign step 2.5). Accounts already in the cache
/// are left alone; nothing is fetched or emitted. Answers the restored
/// accounts' snapshots, for the bus's attention facts (step 2.6).
pub(crate) fn restore_usage(
    store: &Mutex<Store>,
    cache: &Mutex<UsageCache>,
) -> Vec<AccountUsageSnapshot> {
    let rows = match store.lock() {
        Ok(s) => s.latest_usage_snapshots().map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    let rows = match rows {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("account usage history: read failed: {e}");
            return Vec::new();
        }
    };
    let mut c = lock_cache(cache);
    let mut restored = Vec::with_capacity(rows.len());
    for r in rows {
        c.restore(
            &r.account_uuid,
            r.usage,
            r.subscription,
            r.fetched_at,
            r.source_host,
        );
        restored.push(c.snapshot(&r.account_uuid));
    }
    restored
}

/// Spawn a background fetch for every account in `hosts` that is due and not
/// already in flight, bounded to [`MAX_CONCURRENT_FETCHES`] concurrent
/// fetches. Returns immediately: nothing here is awaited, so a slow or hung
/// host can never delay the caller (a reconcile tick or this poller's own
/// loop). The returned handles are for tests to wait on deterministically;
/// production callers drop them — dropping a `JoinHandle` does not cancel
/// the task.
pub(crate) fn poll_due_accounts(
    poller: &Arc<AccountUsagePoller>,
    hosts: Arc<Vec<HostRow>>,
    ssh: Arc<dyn SshExec>,
    cache: Arc<Mutex<UsageCache>>,
    bus: Arc<dyn EventBus>,
    store: Option<Arc<Mutex<Store>>>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let mut handles = Vec::new();
    for account in distinct_account_uuids(&hosts) {
        reannounce_lapsed(poller, &account, bus.as_ref(), &cache, store.as_deref());
        // Cheap pre-filter: skip the common case (not due yet) without
        // touching the in-flight set or spawning anything.
        if !lock_cache(&cache).due(&account) {
            continue;
        }
        {
            let mut inflight = poller
                .in_flight
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if !inflight.insert(account.clone()) {
                continue; // already being fetched (or queued for a permit)
            }
        }
        let poller = Arc::clone(poller);
        let hosts = Arc::clone(&hosts);
        let ssh = Arc::clone(&ssh);
        let cache = Arc::clone(&cache);
        let bus = Arc::clone(&bus);
        let store = store.clone();
        handles.push(tokio::spawn(async move {
            let _permit = poller
                .limit
                .clone()
                .acquire_owned()
                .await
                .expect("usage semaphore is never closed");
            fetch_and_emit(
                &account,
                &hosts,
                ssh.as_ref(),
                &cache,
                bus.as_ref(),
                store.as_deref(),
            )
            .await;
            poller
                .in_flight
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&account);
        }));
    }
    handles
}

/// `list_account_usage`: every known account's cached snapshot (`accounts`
/// table — every account fleet has ever seen, not just those currently
/// attached to a host, unlike the poller's [`distinct_account_uuids`]). Never
/// fetches: an account with no cache entry reads as `never_fetched`.
pub fn list_account_usage(
    store: &Mutex<Store>,
    cache: &Mutex<UsageCache>,
) -> Result<Vec<AccountUsageSnapshot>, IpcError> {
    let accounts = {
        let s = lock(store)?;
        s.list_accounts()?
    };
    let c = lock_cache(cache);
    Ok(accounts.iter().map(|a| c.snapshot(&a.uuid)).collect())
}

/// The hub's `account_usage` tool: every known account's latest usage
/// answer as the store's bus followed it from `account_usage:updated` (and
/// the history `restore_usage` seeded at startup). The usage tick's own
/// cache stays private to the tick; the bus holds every answer that changed
/// something, so only `next_try_at` can lag. Never fetches. An account the
/// bus has no answer for is `never_fetched`.
pub fn served_account_usage(store: &Mutex<Store>) -> Result<Vec<AccountUsageSnapshot>, IpcError> {
    let s = lock(store)?;
    let known: std::collections::HashMap<String, AccountUsageSnapshot> = s
        .bus_account_usage()
        .into_iter()
        .map(|u| (u.account_uuid.clone(), u))
        .collect();
    Ok(s.list_accounts()?
        .iter()
        .map(|a| {
            known
                .get(&a.uuid)
                .cloned()
                .unwrap_or_else(|| AccountUsageSnapshot::never_fetched(&a.uuid))
        })
        .collect())
}

/// `refresh_account_usage`: fetch now if the per-account floor allows, else
/// the current (unchanged) snapshot — its `next_try_at` tells the caller
/// when a refresh becomes possible. `E_NOTFOUND` when `account_uuid` is not
/// a known account.
pub async fn refresh_account_usage(
    account_uuid: &str,
    store: &Mutex<Store>,
    ssh: &dyn SshExec,
    cache: &Mutex<UsageCache>,
    bus: &dyn EventBus,
) -> Result<AccountUsageSnapshot, IpcError> {
    let hosts = {
        let s = lock(store)?;
        let known = s
            .list_accounts()?
            .into_iter()
            .any(|a| a.uuid == account_uuid);
        if !known {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("account {account_uuid} not found"),
            ));
        }
        s.list_hosts()?
    };
    Ok(fetch_and_emit(account_uuid, &hosts, ssh, cache, bus, Some(store)).await)
}

/// `account_usage_history`: one account's stored snapshots fetched at or
/// after `since` (unix seconds), oldest first — what the Accounts page draws
/// its 5-hour and weekly history from (redesign step 4.1). Never fetches.
/// `E_NOTFOUND` when `account_uuid` is not a known account.
pub fn account_usage_history(
    account_uuid: &str,
    since: i64,
    store: &Mutex<Store>,
) -> Result<Vec<UsageSnapshotRow>, IpcError> {
    let s = lock(store)?;
    if !s.list_accounts()?.iter().any(|a| a.uuid == account_uuid) {
        return Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("account {account_uuid} not found"),
        ));
    }
    Ok(s.usage_history(account_uuid, since)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::transport::exit_status;
    use crate::events::RecordingEventBus;
    use crate::service::account_usage::{
        Clock, FetchResult, UsageOutcome, UsageOutcomeKind, USAGE_POLL_FLOOR_SECS,
    };
    use crate::ssh_fake::FakeSsh;
    use std::process::Output;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    // ── a deterministic clock, independent of account_usage.rs's private
    //    test-only FakeClock (its `Clock` trait is public, so any caller can
    //    inject one) ──────────────────────────────────────────────────────

    struct TestClock {
        base: Instant,
        state: Mutex<(Duration, i64)>,
    }

    impl TestClock {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                base: Instant::now(),
                state: Mutex::new((Duration::ZERO, 1_770_000_000)),
            })
        }
        /// Move both hands forward together, so a floor measured in monotonic
        /// time and a `next_try_at` in unix seconds agree about how long it
        /// has been.
        fn advance(&self, secs: u64) {
            let mut st = self.state.lock().unwrap();
            st.0 += Duration::from_secs(secs);
            st.1 += secs as i64;
        }
    }

    impl Clock for TestClock {
        fn now_instant(&self) -> Instant {
            self.base + self.state.lock().unwrap().0
        }
        fn now_unix(&self) -> i64 {
            self.state.lock().unwrap().1
        }
    }

    fn host(alias: &str, account: &str) -> HostRow {
        HostRow {
            alias: alias.to_string(),
            ssh_alias: None,
            reachable: true,
            claude_version: None,
            tmux_version: None,
            hidden: false,
            last_pinged_at: None,
            account_uuid: Some(account.to_string()),
            provisioned: true,
            transport: "ssh".to_string(),
            org_id: None,
            claude_version_at: None,
            disk_home_free_kb: None,
            disk_home_total_kb: None,
            disk_tmp_free_kb: None,
            load_1m: None,
            mem_avail_kb: None,
            uptime_secs: None,
            health_at: None,
            last_hook_at: None,
            agent_version: None,
            provisioned_at: None,
            provision_stale: false,
            unclaimed_sessions: None,
            provision_warning: None,
            auth_overrides: None,
            claude_profiles: None,
            cpu_count: None,
            mem_total_kb: None,
            boot_at: None,
            latency_ms: None,
            worktree_kb: None,
            worktree_at: None,
            agents_on_path: None,
            last_reachable_at: None,
            last_probe_error_code: None,
            last_probe_error: None,
            harnesses: None,
        }
    }

    /// stdout that `parse_usage_output` classifies as `NoCredentials`: a
    /// single host-specific outcome, so a fetch ends after one call with no
    /// further candidates to try.
    const NO_CREDENTIALS_OUTPUT: &str = "__usage_start__\n__no_credentials__\n";

    // ── poll_due_accounts ────────────────────────────────────────────────

    #[tokio::test]
    async fn fetches_only_due_accounts() {
        let hosts = Arc::new(vec![host("h1", "acct-due"), host("h2", "acct-not-due")]);
        let cache = Arc::new(Mutex::new(UsageCache::with_clock(TestClock::new())));
        // Pre-record a recent success for acct-not-due so its floor hasn't
        // elapsed.
        cache.lock().unwrap().record(
            "acct-not-due",
            FetchResult::Answered {
                host: "h2".to_string(),
                outcome: UsageOutcome::Ok {
                    usage: Default::default(),
                    subscription: None,
                },
                notes: vec![],
            },
        );
        let fake = FakeSsh::new();
        fake.on(
            crate::ssh_fake::Match::Any,
            crate::ssh_fake::Reply::ok(NO_CREDENTIALS_OUTPUT),
        );
        let ssh: Arc<dyn SshExec> = Arc::new(fake.clone());
        let poller = Arc::new(AccountUsagePoller::new());
        let bus: Arc<dyn EventBus> = Arc::new(RecordingEventBus::new());
        let handles = poll_due_accounts(&poller, hosts, ssh, Arc::clone(&cache), bus, None);
        assert_eq!(handles.len(), 1, "only the due account should be polled");
        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(fake.calls_for("h1").len(), 1);
        assert!(fake.calls_for("h2").is_empty(), "not-due account untouched");
    }

    #[tokio::test]
    async fn bounds_concurrency_to_two() {
        struct SlowCountingSsh {
            delay: Duration,
            current: AtomicUsize,
            max: AtomicUsize,
        }
        #[async_trait::async_trait]
        impl SshExec for SlowCountingSsh {
            async fn run(&self, _: &str, _: &[&str], _: Duration) -> Result<Output, IpcError> {
                unimplemented!()
            }
            async fn run_bounded(
                &self,
                host: &str,
                args: &[&str],
                ct: Duration,
                wc: Duration,
            ) -> Result<Output, IpcError> {
                self.run_bounded_capped(host, args, ct, wc, usize::MAX)
                    .await
            }
            async fn run_bounded_capped(
                &self,
                _host: &str,
                _args: &[&str],
                _connect_timeout: Duration,
                _wall_clock: Duration,
                _max_output: usize,
            ) -> Result<Output, IpcError> {
                let cur = self.current.fetch_add(1, Ordering::SeqCst) + 1;
                self.max.fetch_max(cur, Ordering::SeqCst);
                tokio::time::sleep(self.delay).await;
                self.current.fetch_sub(1, Ordering::SeqCst);
                Ok(Output {
                    status: exit_status(0),
                    stdout: NO_CREDENTIALS_OUTPUT.as_bytes().to_vec(),
                    stderr: Vec::new(),
                })
            }
            async fn run_cancellable(
                &self,
                _: &str,
                _: &[&str],
                _: Duration,
                _: tokio_util::sync::CancellationToken,
            ) -> Result<Output, IpcError> {
                unimplemented!()
            }
            async fn run_bounded_cancellable(
                &self,
                _: &str,
                _: &[&str],
                _: Duration,
                _: Duration,
                _: tokio_util::sync::CancellationToken,
            ) -> Result<Output, IpcError> {
                unimplemented!()
            }
            async fn upload_file(
                &self,
                _: &str,
                _: &std::path::Path,
                _: &str,
                _: Duration,
            ) -> Result<(), IpcError> {
                unimplemented!()
            }
            async fn remote_home(&self, _: &str) -> Result<String, IpcError> {
                unimplemented!()
            }
        }

        let hosts = Arc::new(vec![
            host("h1", "acct-1"),
            host("h2", "acct-2"),
            host("h3", "acct-3"),
            host("h4", "acct-4"),
        ]);
        let cache = Arc::new(Mutex::new(UsageCache::with_clock(TestClock::new())));
        let ssh = Arc::new(SlowCountingSsh {
            delay: Duration::from_millis(60),
            current: AtomicUsize::new(0),
            max: AtomicUsize::new(0),
        });
        let ssh_dyn: Arc<dyn SshExec> = ssh.clone();
        let poller = Arc::new(AccountUsagePoller::new());
        let bus: Arc<dyn EventBus> = Arc::new(RecordingEventBus::new());
        let handles = poll_due_accounts(&poller, hosts, ssh_dyn, cache, bus, None);
        assert_eq!(handles.len(), 4);
        for h in handles {
            h.await.unwrap();
        }
        assert!(
            ssh.max.load(Ordering::SeqCst) <= MAX_CONCURRENT_FETCHES,
            "max observed concurrency {} exceeds the {} limit",
            ssh.max.load(Ordering::SeqCst),
            MAX_CONCURRENT_FETCHES
        );
        assert_eq!(
            ssh.max.load(Ordering::SeqCst),
            MAX_CONCURRENT_FETCHES,
            "with 4 due accounts and a 2-slot limit, concurrency should reach the cap"
        );
    }

    #[tokio::test]
    async fn in_flight_account_is_not_spawned_again() {
        let hosts = Arc::new(vec![host("h1", "acct-1")]);
        let cache = Arc::new(Mutex::new(UsageCache::with_clock(TestClock::new())));
        let fake = FakeSsh::new();
        fake.set_wall_clock(Duration::from_secs(5));
        fake.on(
            crate::ssh_fake::Match::Any,
            crate::ssh_fake::Reply::Hang {
                for_: Duration::from_millis(150),
            },
        );
        let ssh: Arc<dyn SshExec> = Arc::new(fake.clone());
        let poller = Arc::new(AccountUsagePoller::new());
        let bus: Arc<dyn EventBus> = Arc::new(RecordingEventBus::new());

        let first = poll_due_accounts(
            &poller,
            Arc::clone(&hosts),
            Arc::clone(&ssh),
            Arc::clone(&cache),
            Arc::clone(&bus),
            None,
        );
        assert_eq!(first.len(), 1, "first call spawns the fetch");
        // Called again while the first fetch is still in flight (it hangs
        // for 150ms and we have not awaited it yet).
        let second = poll_due_accounts(&poller, hosts, ssh, cache, bus, None);
        assert!(
            second.is_empty(),
            "in-flight account must not be spawned again"
        );

        for h in first {
            h.await.unwrap();
        }
        assert_eq!(fake.calls().len(), 1, "exactly one ssh call was ever made");
    }

    #[tokio::test]
    async fn poll_due_accounts_never_blocks_the_caller() {
        let hosts = Arc::new(vec![host("h1", "acct-1")]);
        let cache = Arc::new(Mutex::new(UsageCache::with_clock(TestClock::new())));
        let fake = FakeSsh::new();
        fake.set_wall_clock(Duration::from_secs(5));
        fake.on(
            crate::ssh_fake::Match::Any,
            crate::ssh_fake::Reply::Hang {
                for_: Duration::from_secs(2),
            },
        );
        let ssh: Arc<dyn SshExec> = Arc::new(fake);
        let poller = Arc::new(AccountUsagePoller::new());
        let bus: Arc<dyn EventBus> = Arc::new(RecordingEventBus::new());

        let start = Instant::now();
        let handles = poll_due_accounts(&poller, hosts, ssh, cache, bus, None);
        let elapsed = start.elapsed();
        assert_eq!(handles.len(), 1);
        assert!(
            elapsed < Duration::from_millis(200),
            "poll_due_accounts must return immediately, took {elapsed:?}"
        );
        // Let the spawned (hung) task finish so it doesn't outlive the test
        // runtime; we don't care about its outcome here.
        drop(handles);
    }

    #[tokio::test]
    async fn emits_once_for_a_changed_snapshot_and_not_for_not_due() {
        let hosts = Arc::new(vec![host("h1", "acct-1"), host("h2", "acct-2")]);
        let cache = Arc::new(Mutex::new(UsageCache::with_clock(TestClock::new())));
        // acct-2 is not due — pre-seed a recent success.
        cache.lock().unwrap().record(
            "acct-2",
            FetchResult::Answered {
                host: "h2".to_string(),
                outcome: UsageOutcome::Ok {
                    usage: Default::default(),
                    subscription: None,
                },
                notes: vec![],
            },
        );
        let fake = FakeSsh::new();
        fake.on(
            crate::ssh_fake::Match::Any,
            crate::ssh_fake::Reply::ok(NO_CREDENTIALS_OUTPUT),
        );
        let ssh: Arc<dyn SshExec> = Arc::new(fake);
        let poller = Arc::new(AccountUsagePoller::new());
        let recording = Arc::new(RecordingEventBus::new());
        let bus: Arc<dyn EventBus> = recording.clone();
        let handles = poll_due_accounts(&poller, hosts, ssh, cache, bus, None);
        for h in handles {
            h.await.unwrap();
        }
        let events = recording.take();
        assert_eq!(
            events,
            vec!["account_usage:updated:acct-1".to_string()],
            "only the due, changed account should emit"
        );
    }

    /// `fetch_and_emit` is documented as "emit iff changed", and for an
    /// account whose answer never changes — an unreachable host, a machine
    /// with no credentials file — it used to emit forever: `next_try_at`
    /// moves on every attempt and the derived `PartialEq` compared it.
    #[tokio::test]
    async fn an_unchanged_account_emits_once_however_often_it_is_polled() {
        let hosts = vec![host("h1", "acct-1")];
        let clock = TestClock::new();
        let cache = Mutex::new(UsageCache::with_clock(clock.clone()));
        let fake = FakeSsh::new();
        fake.on(
            crate::ssh_fake::Match::Any,
            crate::ssh_fake::Reply::ok(NO_CREDENTIALS_OUTPUT),
        );
        let ssh: Arc<dyn SshExec> = Arc::new(fake);
        let recording = RecordingEventBus::new();

        let first = fetch_and_emit("acct-1", &hosts, ssh.as_ref(), &cache, &recording, None).await;
        clock.advance(USAGE_POLL_FLOOR_SECS as u64 + 10);
        let second = fetch_and_emit("acct-1", &hosts, ssh.as_ref(), &cache, &recording, None).await;

        assert_eq!(
            recording.take(),
            vec!["account_usage:updated:acct-1".to_string()],
            "the same answer, twice, is one event"
        );
        assert_ne!(
            first.next_try_at, second.next_try_at,
            "the second attempt really did happen and really did reschedule — \
             which is exactly the field that must not count as news"
        );
        assert_eq!(first.status, second.status);
    }

    // ── list_account_usage / refresh_account_usage ──────────────────────

    fn store_with_account(uuid: &str) -> Mutex<Store> {
        let s = Store::open_in_memory().unwrap();
        s.upsert_account(&crate::store::AccountRow {
            uuid: uuid.to_string(),
            email: Some("a@example.com".to_string()),
            display_name: None,
            organization_name: None,
            organization_uuid: None,
            seat_tier: None,
            last_seen_at: Some(1),
            nickname: None,
            has_extra_usage: false,
        })
        .unwrap();
        Mutex::new(s)
    }

    #[test]
    fn list_account_usage_never_fetched_for_unknown_and_reflects_cache() {
        let store = store_with_account("acct-1");
        let cache = Mutex::new(UsageCache::new());
        let snaps = list_account_usage(&store, &cache).unwrap();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].account_uuid, "acct-1");
        assert_eq!(snaps[0].status, UsageOutcomeKind::NeverFetched);

        cache.lock().unwrap().record(
            "acct-1",
            FetchResult::Answered {
                host: "h1".to_string(),
                outcome: UsageOutcome::Ok {
                    usage: Default::default(),
                    subscription: Some("max".to_string()),
                },
                notes: vec![],
            },
        );
        let snaps = list_account_usage(&store, &cache).unwrap();
        assert_eq!(snaps[0].status, UsageOutcomeKind::Ok);
        assert_eq!(snaps[0].subscription.as_deref(), Some("max"));
    }

    /// The hub's `account_usage` tool serves what the bus followed: the
    /// answer an `account_usage:updated` carried, and `never_fetched` for an
    /// account nothing has answered for. Off the hub the bus knows none.
    #[test]
    fn served_usage_is_what_the_bus_followed() {
        let bus = Arc::new(crate::events::BroadcastEventBus::new(4));
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        for uuid in ["acct-1", "acct-2"] {
            s.upsert_account(&crate::store::AccountRow {
                uuid: uuid.to_string(),
                email: None,
                display_name: None,
                organization_name: None,
                organization_uuid: None,
                seat_tier: None,
                last_seen_at: Some(1),
                nickname: None,
                has_extra_usage: false,
            })
            .unwrap();
        }
        let store = Mutex::new(s);
        let mut answered = AccountUsageSnapshot::never_fetched("acct-1");
        answered.status = UsageOutcomeKind::Ok;
        answered.subscription = Some("max".to_string());
        answered.fetched_at = Some(100);
        bus.emit(&crate::events::RowChange::AccountUsageUpdated(
            answered.clone(),
        ));

        let served = served_account_usage(&store).unwrap();
        assert_eq!(served.len(), 2);
        let one = served.iter().find(|u| u.account_uuid == "acct-1").unwrap();
        assert_eq!(one, &answered);
        let two = served.iter().find(|u| u.account_uuid == "acct-2").unwrap();
        assert_eq!(two.status, UsageOutcomeKind::NeverFetched);

        let desktop = store_with_account("acct-1");
        assert_eq!(
            served_account_usage(&desktop).unwrap()[0].status,
            UsageOutcomeKind::NeverFetched
        );
    }

    #[tokio::test]
    async fn refresh_within_floor_makes_no_ssh_call_and_returns_snapshot() {
        let store = store_with_account("acct-1");
        {
            let s = store.lock().unwrap();
            s.insert_host("h1", None).unwrap();
            s.set_host_account("h1", Some("acct-1")).unwrap();
            s.update_host_probe("h1", true, None, None, 1).unwrap();
        }
        let cache = Mutex::new(UsageCache::with_clock(TestClock::new()));
        // Already fetched moments ago: inside the floor.
        cache.lock().unwrap().record(
            "acct-1",
            FetchResult::Answered {
                host: "h1".to_string(),
                outcome: UsageOutcome::Ok {
                    usage: Default::default(),
                    subscription: None,
                },
                notes: vec![],
            },
        );
        let fake = FakeSsh::new();
        let bus = RecordingEventBus::new();
        let before = cache.lock().unwrap().snapshot("acct-1");
        let after = refresh_account_usage("acct-1", &store, &fake, &cache, &bus)
            .await
            .unwrap();
        assert!(
            fake.calls().is_empty(),
            "must not call ssh inside the floor"
        );
        assert_eq!(after, before);
        assert!(
            after.next_try_at > 0,
            "next_try_at should tell the caller when a refresh becomes possible"
        );
        assert!(bus.take().is_empty(), "unchanged snapshot must not emit");
    }

    #[tokio::test]
    async fn refresh_unknown_uuid_is_not_found() {
        let store = store_with_account("acct-1");
        let cache = Mutex::new(UsageCache::new());
        let fake = FakeSsh::new();
        let bus = RecordingEventBus::new();
        let err = refresh_account_usage("does-not-exist", &store, &fake, &cache, &bus)
            .await
            .unwrap_err();
        assert_eq!(err.code, "E_NOTFOUND");
        assert!(fake.calls().is_empty());
    }

    #[tokio::test]
    async fn refresh_past_the_floor_fetches_and_emits() {
        let store = store_with_account("acct-1");
        {
            let s = store.lock().unwrap();
            s.insert_host("h1", None).unwrap();
            s.set_host_account("h1", Some("acct-1")).unwrap();
            s.update_host_probe("h1", true, None, None, 1).unwrap();
        }
        let cache = Mutex::new(UsageCache::new());
        let fake = FakeSsh::new();
        fake.on(
            crate::ssh_fake::Match::Any,
            crate::ssh_fake::Reply::ok(NO_CREDENTIALS_OUTPUT),
        );
        let bus = RecordingEventBus::new();
        let after = refresh_account_usage("acct-1", &store, &fake, &cache, &bus)
            .await
            .unwrap();
        assert_eq!(after.status, UsageOutcomeKind::NoCredentials);
        assert_eq!(fake.calls_for("h1").len(), 1);
        assert_eq!(bus.take(), vec!["account_usage:updated:acct-1".to_string()]);
    }

    // ── usage history (redesign step 2.5) ────────────────────────────────

    const OK_OUTPUT: &str =
        "__usage_start__\n__subscription__=max\n__http_status__=200\n__body__\n\
        {\"five_hour\":{\"utilization\":35.0,\"resets_at\":\"2026-02-06T22:00:00+00:00\"},\
        \"seven_day\":{\"utilization\":14.0,\"resets_at\":\"2026-02-12T20:00:00+00:00\"}}";

    fn store_with_logged_in_host() -> Arc<Mutex<Store>> {
        let store = store_with_account("acct-1");
        {
            let s = store.lock().unwrap();
            s.insert_host("h1", None).unwrap();
            s.set_host_account("h1", Some("acct-1")).unwrap();
            s.update_host_probe("h1", true, None, None, 1).unwrap();
        }
        Arc::new(store)
    }

    #[tokio::test]
    async fn a_poll_writes_a_row_and_a_restart_keeps_the_last_value() {
        let store = store_with_logged_in_host();
        let hosts = Arc::new(store.lock().unwrap().list_hosts().unwrap());
        let clock = TestClock::new();
        let cache = Arc::new(Mutex::new(UsageCache::with_clock(clock.clone())));
        let fake = FakeSsh::new();
        fake.on(
            crate::ssh_fake::Match::Any,
            crate::ssh_fake::Reply::ok(OK_OUTPUT),
        );
        let ssh: Arc<dyn SshExec> = Arc::new(fake.clone());
        let bus: Arc<dyn EventBus> = Arc::new(RecordingEventBus::new());
        let poller = Arc::new(AccountUsagePoller::new());
        let poll = || {
            poll_due_accounts(
                &poller,
                Arc::clone(&hosts),
                Arc::clone(&ssh),
                Arc::clone(&cache),
                Arc::clone(&bus),
                Some(Arc::clone(&store)),
            )
        };
        for h in poll() {
            h.await.unwrap();
        }
        let rows = store.lock().unwrap().usage_history("acct-1", 0).unwrap();
        assert_eq!(rows.len(), 1, "one successful poll, one row");
        assert_eq!(rows[0].usage.five_hour.as_ref().unwrap().utilization, 35.0);
        assert!(rows[0]
            .usage
            .seven_day
            .as_ref()
            .unwrap()
            .resets_at
            .is_some());
        assert_eq!(rows[0].subscription.as_deref(), Some("max"));
        assert_eq!(rows[0].source_host.as_deref(), Some("h1"));

        // Inside the floor nothing is fetched, so nothing is written.
        assert!(poll().is_empty());
        clock.advance(USAGE_POLL_FLOOR_SECS as u64);
        for h in poll() {
            h.await.unwrap();
        }
        assert_eq!(
            store
                .lock()
                .unwrap()
                .usage_history("acct-1", 0)
                .unwrap()
                .len(),
            2
        );

        // A restart: a fresh cache seeded from the store shows the last value
        // before any poll, and is still due straight away.
        let before = cache.lock().unwrap().snapshot("acct-1");
        let fresh = Mutex::new(UsageCache::new());
        restore_usage(&store, &fresh);
        let after = list_account_usage(&store, &fresh).unwrap();
        assert_eq!(after[0].status, UsageOutcomeKind::Ok);
        assert_eq!(after[0].usage, before.usage);
        assert_eq!(after[0].fetched_at, before.fetched_at);
        assert_eq!(after[0].source_host.as_deref(), Some("h1"));
        assert!(fresh.lock().unwrap().due("acct-1"));
    }

    #[tokio::test]
    async fn a_failed_answer_writes_no_row() {
        let store = store_with_logged_in_host();
        let cache = Mutex::new(UsageCache::new());
        let fake = FakeSsh::new();
        fake.on(
            crate::ssh_fake::Match::Any,
            crate::ssh_fake::Reply::ok(NO_CREDENTIALS_OUTPUT),
        );
        let bus = RecordingEventBus::new();
        refresh_account_usage("acct-1", &store, &fake, &cache, &bus)
            .await
            .unwrap();
        assert!(store
            .lock()
            .unwrap()
            .usage_history("acct-1", 0)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn restore_never_overwrites_a_live_entry() {
        let store = store_with_account("acct-1");
        store
            .lock()
            .unwrap()
            .insert_usage_snapshot(&UsageSnapshotRow {
                account_uuid: "acct-1".into(),
                fetched_at: 5,
                usage: Default::default(),
                subscription: Some("old".into()),
                source_host: None,
            })
            .unwrap();
        let cache = Mutex::new(UsageCache::new());
        cache.lock().unwrap().record(
            "acct-1",
            FetchResult::Answered {
                host: "h1".to_string(),
                outcome: UsageOutcome::Ok {
                    usage: Default::default(),
                    subscription: Some("new".to_string()),
                },
                notes: vec![],
            },
        );
        restore_usage(&store, &cache);
        let snap = cache.lock().unwrap().snapshot("acct-1");
        assert_eq!(snap.subscription.as_deref(), Some("new"));
    }

    #[test]
    fn history_is_read_from_since_and_an_unknown_account_is_not_found() {
        let store = store_with_account("acct-1");
        {
            let s = store.lock().unwrap();
            for at in [100, 200, 300] {
                s.insert_usage_snapshot(&UsageSnapshotRow {
                    account_uuid: "acct-1".into(),
                    fetched_at: at,
                    usage: Default::default(),
                    subscription: None,
                    source_host: None,
                })
                .unwrap();
            }
        }
        let at: Vec<i64> = account_usage_history("acct-1", 200, &store)
            .unwrap()
            .iter()
            .map(|r| r.fetched_at)
            .collect();
        assert_eq!(at, vec![200, 300]);
        let err = account_usage_history("nope", 0, &store).unwrap_err();
        assert_eq!(err.code, "E_NOTFOUND");
    }

    /// Review r05 F9: an account moving into or out of a lost login (or a
    /// limit) re-announces its live sessions, so the hub's stamped
    /// `needs_attention` follows; a dead row, another account's session and
    /// an answer that moves nothing send no session frame.
    #[tokio::test]
    async fn an_account_entering_or_leaving_a_block_re_announces_its_live_sessions() {
        let bus = Arc::new(crate::events::BroadcastEventBus::new(64));
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        for uuid in ["acct-1", "acct-2"] {
            s.upsert_account(&crate::store::AccountRow {
                uuid: uuid.into(),
                ..Default::default()
            })
            .unwrap();
        }
        s.insert_host("h1", None).unwrap();
        let mut ids = Vec::new();
        for (name, account, status) in [
            ("idle-1", "acct-1", "running"),
            ("ghost-1", "acct-1", "ghost"),
            ("idle-2", "acct-2", "running"),
        ] {
            let id = s
                .upsert_session(name, "h1", None, None, 1, 1, status, None)
                .unwrap();
            s.conn_for_test()
                .execute(
                    "UPDATE sessions SET account_uuid = ?1, claude_status = 'idle' WHERE id = ?2",
                    rusqlite::params![account, id],
                )
                .unwrap();
            ids.push(id);
        }
        let store = Mutex::new(s);
        let hosts = vec![host("h1", "acct-1")];
        let clock = TestClock::new();
        let cache = Mutex::new(UsageCache::with_clock(clock.clone()));
        let mut rx = bus.subscribe();
        let mut frames = || {
            let mut out = Vec::new();
            while let Ok(m) = rx.try_recv() {
                let id = m.payload.get("id").and_then(|v| v.as_i64());
                let reason = m.payload["needs_attention"]["reason"]
                    .as_str()
                    .map(String::from);
                out.push((m.name, id, reason));
            }
            out
        };
        let fetch = |out: &'static str| {
            let fake = FakeSsh::new();
            fake.on(crate::ssh_fake::Match::Any, crate::ssh_fake::Reply::ok(out));
            fake
        };

        let gone = fetch("__usage_start__\n__login_expired__\n");
        fetch_and_emit("acct-1", &hosts, &gone, &cache, bus.as_ref(), Some(&store)).await;
        assert_eq!(
            frames(),
            vec![
                ("account_usage:updated", None, None),
                (
                    "session:updated",
                    Some(ids[0]),
                    Some("no_credentials".into())
                ),
            ]
        );

        clock.advance(8 * 86_400);
        let ok = fetch(OK_OUTPUT);
        fetch_and_emit("acct-1", &hosts, &ok, &cache, bus.as_ref(), Some(&store)).await;
        assert_eq!(
            frames(),
            vec![
                ("account_usage:updated", None, None),
                ("session:updated", Some(ids[0]), None),
            ],
            "signed in again: the stamp comes off"
        );

        clock.advance(8 * 86_400);
        fetch_and_emit("acct-1", &hosts, &ok, &cache, bus.as_ref(), Some(&store)).await;
        assert!(
            frames().iter().all(|f| f.0 != "session:updated"),
            "a new reading that moves no block re-announces nothing"
        );
    }

    /// Review r05 F9: a limit that resets with no new reading re-announces
    /// the account's live sessions once; a reading after the reset (which
    /// announced itself) or a limit still in force does not.
    #[test]
    fn a_limit_that_lapses_with_no_new_reading_re_announces_its_sessions() {
        assert!(limit_lapsed(Some(100), false, Some(50)));
        assert!(limit_lapsed(Some(100), false, None));
        assert!(
            !limit_lapsed(Some(100), false, Some(150)),
            "a newer reading said so"
        );
        assert!(!limit_lapsed(Some(100), true, Some(50)), "still limited");
        assert!(!limit_lapsed(None, false, Some(50)), "never seen limited");

        let bus = Arc::new(crate::events::BroadcastEventBus::new(64));
        let s = Store::open_with_bus_in_memory(bus.clone()).unwrap();
        s.upsert_account(&crate::store::AccountRow {
            uuid: "acct-1".into(),
            ..Default::default()
        })
        .unwrap();
        s.insert_host("h1", None).unwrap();
        let id = s
            .upsert_session("idle-1", "h1", None, None, 1, 1, "running", None)
            .unwrap();
        s.conn_for_test()
            .execute(
                "UPDATE sessions SET account_uuid = 'acct-1', claude_status = 'idle' WHERE id = ?1",
                [id],
            )
            .unwrap();
        let store = Mutex::new(s);
        let cache = Mutex::new(UsageCache::default());
        let poller = AccountUsagePoller::new();
        // The last pass saw the account limited until a reset that has
        // passed; the bus no longer holds it limited.
        poller
            .limits
            .lock()
            .unwrap()
            .insert("acct-1".into(), crate::store::now_unix() - 60);
        let mut rx = bus.subscribe();
        reannounce_lapsed(&poller, "acct-1", bus.as_ref(), &cache, Some(&store));
        let mut sent = Vec::new();
        while let Ok(m) = rx.try_recv() {
            sent.push((m.name, m.payload.get("id").and_then(|v| v.as_i64())));
        }
        assert_eq!(sent, vec![("session:updated", Some(id))]);
        reannounce_lapsed(&poller, "acct-1", bus.as_ref(), &cache, Some(&store));
        assert!(rx.try_recv().is_err(), "once per lapse");
    }
}
