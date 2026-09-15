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
//!   returns the same snapshot, so it never emits.

use crate::events::EventBus;
use crate::ipc_error::lock;
use crate::ipc_error::IpcError;
use crate::service::account_usage::{self, AccountUsageSnapshot, UsageCache};
use crate::ssh::SshExec;
use crate::store::{HostRow, Store};
use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::Semaphore;

/// At most this many usage fetches run at once, across every account.
const MAX_CONCURRENT_FETCHES: usize = 2;

fn lock_cache(cache: &Mutex<UsageCache>) -> std::sync::MutexGuard<'_, UsageCache> {
    cache.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Distinct non-null `account_uuid`s across `hosts`, first-seen order (the
/// poller's account list — see the module doc on why this differs from
/// [`list_account_usage`]'s source).
pub(crate) fn distinct_account_uuids(hosts: &[HostRow]) -> Vec<String> {
    let mut seen = HashSet::new();
    hosts
        .iter()
        .filter_map(|h| h.account_uuid.clone())
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
}

impl AccountUsagePoller {
    pub(crate) fn new() -> Self {
        Self {
            in_flight: Mutex::new(HashSet::new()),
            limit: Arc::new(Semaphore::new(MAX_CONCURRENT_FETCHES)),
        }
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
) -> AccountUsageSnapshot {
    let before = lock_cache(cache).snapshot(account_uuid);
    let after =
        account_usage::fetch_account_usage_with(account_uuid, hosts, ssh, cache, false).await;
    if after != before {
        bus.account_usage_updated(&after);
    }
    after
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
) -> Vec<tokio::task::JoinHandle<()>> {
    let mut handles = Vec::new();
    for account in distinct_account_uuids(&hosts) {
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
        handles.push(tokio::spawn(async move {
            let _permit = poller
                .limit
                .clone()
                .acquire_owned()
                .await
                .expect("usage semaphore is never closed");
            fetch_and_emit(&account, &hosts, ssh.as_ref(), &cache, bus.as_ref()).await;
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
pub(crate) fn list_account_usage(
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

/// `refresh_account_usage`: fetch now if the per-account floor allows, else
/// the current (unchanged) snapshot — its `next_try_at` tells the caller
/// when a refresh becomes possible. `E_NOTFOUND` when `account_uuid` is not
/// a known account.
pub(crate) async fn refresh_account_usage(
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
                "E_NOTFOUND",
                format!("account {account_uuid} not found"),
            ));
        }
        s.list_hosts()?
    };
    Ok(fetch_and_emit(account_uuid, &hosts, ssh, cache, bus).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::RecordingEventBus;
    use crate::service::account_usage::{Clock, FetchResult, UsageOutcome, UsageOutcomeKind};
    use crate::ssh_fake::FakeSsh;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{ExitStatus, Output};
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
        let handles = poll_due_accounts(&poller, hosts, ssh, Arc::clone(&cache), bus);
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
                    status: ExitStatus::from_raw(0),
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
        let handles = poll_due_accounts(&poller, hosts, ssh_dyn, cache, bus);
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
        );
        assert_eq!(first.len(), 1, "first call spawns the fetch");
        // Called again while the first fetch is still in flight (it hangs
        // for 150ms and we have not awaited it yet).
        let second = poll_due_accounts(&poller, hosts, ssh, cache, bus);
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
        let handles = poll_due_accounts(&poller, hosts, ssh, cache, bus);
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
        let handles = poll_due_accounts(&poller, hosts, ssh, cache, bus);
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
}
