//! Assets S1a: rescan hosts without anyone pressing Scan. Hourly by
//! default, it rescans every reachable host whose inventory is older than a
//! day, and all of them after the catalog HEAD or a person's sync changed
//! (SB6's own runs do not count). After each pass it runs the changeset
//! reconcile pass (Assets M4).

use crate::ssh::SshClient;
use crate::store::Store;
use std::collections::BTreeSet;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// One host's due-for-a-rescan inputs, pure so [`hosts_due`] can be tested
/// without a store.
pub struct HostDue {
    pub alias: String,
    pub reachable: bool,
    pub hidden: bool,
    pub last_scan: Option<i64>,
}

/// Which hosts should be (re)scanned this pass: not hidden, reachable (or
/// `local`, which has no SSH reachability of its own), and either
/// `everything_changed`, or never scanned, or stale past `max_age`.
pub fn hosts_due(
    now: i64,
    hosts: &[HostDue],
    max_age: i64,
    everything_changed: bool,
) -> Vec<String> {
    hosts
        .iter()
        .filter(|h| !h.hidden && (h.reachable || h.alias == "local"))
        .filter(|h| everything_changed || h.last_scan.is_none_or(|t| now - t > max_age))
        .map(|h| h.alias.clone())
        .collect()
}

/// `due`, plus whichever `owed` hosts are currently eligible (not hidden,
/// reachable or `local`) and not already in `due` — a host whose previous
/// scan failed or came back partial stays owed across passes regardless of
/// staleness, so a transient failure during a catalog/sync-triggered sweep
/// gets retried on the next pass rather than waiting out `max_age`. That
/// pass comes ahead of the next tick ([`early_retry`]).
pub fn with_owed(due: Vec<String>, owed: &BTreeSet<String>, hosts: &[HostDue]) -> Vec<String> {
    let mut out = due;
    for h in hosts {
        if owed.contains(&h.alias)
            && !h.hidden
            && (h.reachable || h.alias == "local")
            && !out.contains(&h.alias)
        {
            out.push(h.alias.clone());
        }
    }
    out
}

/// Drop from `owed` every host that is no longer in the fleet or is hidden:
/// only a successful scan removed one, and [`with_owed`] never scans those,
/// so a deleted host stayed owed until the hub restarted — keeping the loop
/// health red and the early-retry backoff at its cap (review w07-1). An
/// unreachable host stays owed; it is tried once it is back.
pub fn prune_owed(owed: &mut BTreeSet<String>, hosts: &[HostDue]) {
    owed.retain(|a| hosts.iter().any(|h| h.alias == *a && !h.hidden));
}

/// Hosts something outside the tick has asked to be rescanned on its next
/// pass (a harness choice changed, `harness_set::set_host_harnesses`). The
/// tick moves them into its own `owed` set, so a failed rescan is retried
/// like any other owed host.
static REQUESTED: LazyLock<Mutex<BTreeSet<String>>> = LazyLock::new(|| Mutex::new(BTreeSet::new()));

fn requested() -> std::sync::MutexGuard<'static, BTreeSet<String>> {
    // Plain data: a poisoned set is still consistent.
    REQUESTED.lock().unwrap_or_else(|e| e.into_inner())
}

/// Mark `alias` as owed a rescan on the tick's next pass, whatever its
/// inventory's age.
pub fn owe_rescan(alias: &str) {
    requested().insert(alias.to_string());
}

/// Hand over (and clear) every [`owe_rescan`] request.
fn take_requested() -> BTreeSet<String> {
    std::mem::take(&mut *requested())
}

/// Whether [`owe_rescan`] was called for `alias` and the tick has not taken
/// it yet.
#[cfg(test)]
pub(crate) fn rescan_requested(alias: &str) -> bool {
    requested().contains(alias)
}

/// The sync half of the tick's "everything changed" key: the newest sync
/// run a person made (directly, or by applying a card). SB6's own runs are
/// left out (final review I3): each one already rescans the hosts it
/// touched, and counting them would rescan the whole fleet on every pass
/// while one host keeps failing.
pub(crate) fn sync_key(s: &Store) -> Option<i64> {
    s.last_person_sync_run_id().ok().flatten()
}

fn setting_secs(store: &Mutex<Store>, key: &str) -> i64 {
    store
        .lock()
        .map(|s| crate::service::settings::get_secs(&s, key) as i64)
        .unwrap_or(0)
}

/// Started only by a process that owns its fleet (`FleetTasks::
/// start_catalog_scan_tick` on a standalone desktop, and `fleet-hub serve`),
/// exactly like the tracker sync — a paired desktop must not run it.
///
/// Its interval is `catalog.scan_check_secs` (default 3600; `0` turns it
/// off, raised to a floor of five minutes otherwise). Each pass reads the
/// host list and the newest scan per host under one scoped `Store` lock
/// (dropped before the `.await` below), then rescans whichever hosts
/// [`hosts_due`] names.
pub fn spawn_catalog_scan_tick(
    store: Arc<Mutex<Store>>,
    ssh: Arc<SshClient>,
    token: CancellationToken,
) -> Option<tokio::task::JoinHandle<()>> {
    use crate::service::settings::CATALOG_SCAN_CHECK_SECS;
    let check = setting_secs(&store, CATALOG_SCAN_CHECK_SECS);
    if check <= 0 {
        tracing::info!("catalog scan tick disabled (catalog.scan_check_secs=0)");
        return None;
    }
    let period = Duration::from_secs(check.max(300) as u64);
    Some(crate::rt::spawn(async move {
        let mut ticker = tokio::time::interval(period);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut seen: Option<(String, Option<i64>)> = None;
        // A host whose scan failed or came back partial (`scan_hosts`
        // returned `Err`, or a status other than "scanned") stays here across
        // passes until it succeeds, independent of `hosts_due`'s staleness
        // check — see `with_owed`.
        let mut owed: BTreeSet<String> = BTreeSet::new();
        // The wait before a pass ahead of the next tick, while a host is
        // owed; see `early_retry`.
        let mut early: Option<Duration> = None;
        let mut backoff = OWED_RETRY;
        loop {
            tokio::select! {
                biased;
                _ = token.cancelled() => break,
                _ = ticker.tick() => {}
                _ = wait(early) => {}
            }
            early = None;
            let before = owed.clone();
            if pass(&store, &ssh, period, &mut seen, &mut owed).await == Pass::Ran {
                (early, backoff) = after_pass(&before, &owed, backoff, period);
            }
        }
    }))
}

/// The first wait before an owed host is tried again ahead of the next tick.
/// Long enough for the hosts' agents to reconnect to a hub that just
/// started: its first pass runs before any of them has, so every agent host
/// fails it (2026-11-16: all four, one second after the listener came up).
const OWED_RETRY: Duration = Duration::from_secs(120);

/// After a pass: the wait before a pass ahead of the next tick (`None`:
/// leave it to the tick), and the backoff to hand in after the next pass.
/// While a host is owed the wait doubles from [`OWED_RETRY`] and stops once
/// it would reach `period`, so a host that keeps failing costs a few extra
/// scans and is then tried once per tick, as before. It starts over when
/// nothing is owed.
fn early_retry(owed: bool, backoff: Duration, period: Duration) -> (Option<Duration>, Duration) {
    if !owed {
        return (None, OWED_RETRY);
    }
    if backoff >= period {
        return (None, backoff);
    }
    (Some(backoff), backoff.saturating_mul(2))
}

/// [`early_retry`] after a pass that left `owed` (it began with `before`).
/// A host newly owed starts its own episode at [`OWED_RETRY`]: the backoff
/// another host's long failure ran up does not deny it the early retry
/// (review w07-1).
fn after_pass(
    before: &BTreeSet<String>,
    owed: &BTreeSet<String>,
    backoff: Duration,
    period: Duration,
) -> (Option<Duration>, Duration) {
    let backoff = if owed.iter().any(|a| !before.contains(a)) {
        OWED_RETRY
    } else {
        backoff
    };
    early_retry(!owed.is_empty(), backoff, period)
}

/// Sleep `d`, or forever without one.
async fn wait(d: Option<Duration>) {
    match d {
        Some(d) => tokio::time::sleep(d).await,
        None => std::future::pending().await,
    }
}

/// What one [`pass`] did.
#[derive(Debug, PartialEq, Eq)]
enum Pass {
    /// Pause all (redesign 8.1) is on: nothing scanned, nothing synced.
    Paused,
    /// No catalog loaded, or the store unreadable: nothing to compare yet.
    Skipped,
    Ran,
}

/// One pass of the tick: rescan the due and owed hosts, then reconcile.
async fn pass(
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
    period: Duration,
    seen: &mut Option<(String, Option<i64>)>,
    owed: &mut BTreeSet<String>,
) -> Pass {
    use crate::service::settings::CATALOG_SCAN_MAX_AGE_SECS;
    // Pause all (redesign 8.1): nothing scanned, nothing synced.
    if !crate::service::loops::gate("catalog_scan", store, Some(period)) {
        return Pass::Paused;
    }
    // Assets M3: every loaded catalog's HEAD, not just personal's — an
    // org catalog that moves must rescan too. Nothing loaded (no
    // personal), or the lock poisoned: nothing to compare yet.
    let head = match super::registry::with_catalogs(|m| {
        if !m.values().any(|c| c.org_id.is_none()) {
            return Err(crate::ipc_error::IpcError::new(
                super::E_CATALOG_NOT_CONFIGURED,
                "catalog not loaded",
            ));
        }
        Ok(super::registry::heads_key(m))
    }) {
        Ok(head) => head,
        Err(_) => return Pass::Skipped,
    };
    // All store reads in one scoped guard, dropped before any await.
    let (hosts, last_sync) = {
        let Ok(s) = store.lock() else {
            return Pass::Skipped;
        };
        let Ok(list) = s.list_hosts() else {
            return Pass::Skipped;
        };
        let last = s.inventory_last_scans().unwrap_or_default();
        let sync = sync_key(&s);
        let hosts: Vec<HostDue> = list
            .into_iter()
            .map(|h| HostDue {
                last_scan: last.get(&h.alias).copied(),
                alias: h.alias,
                reachable: h.reachable,
                hidden: h.hidden,
            })
            .collect();
        (hosts, sync)
    };
    let now_key = (head, last_sync);
    let changed = seen.as_ref() != Some(&now_key);
    let due = hosts_due(
        super::now_secs(),
        &hosts,
        setting_secs(store, CATALOG_SCAN_MAX_AGE_SECS),
        changed,
    );
    owed.extend(take_requested());
    prune_owed(owed, &hosts);
    let due = with_owed(due, owed, &hosts);
    for alias in due {
        match super::inventory::scan_hosts(store, ssh, Some(&alias)).await {
            Ok(results) => match results.iter().find(|r| r.host == alias) {
                Some(r) if r.status == "scanned" => {
                    owed.remove(&alias);
                }
                Some(r) => {
                    owed.insert(alias.clone());
                    tracing::warn!(
                        host = %alias,
                        status = %r.status,
                        detail = ?r.detail,
                        "catalog scan tick: host not fully scanned; retrying next pass"
                    );
                }
                None => {
                    owed.insert(alias.clone());
                }
            },
            Err(e) => {
                owed.insert(alias.clone());
                tracing::warn!(host = %alias, "catalog scan tick: {}", e.message);
            }
        }
    }
    // Assets M4 (R19): the reconcile pass — cards, automatic hides —
    // after every pass that got this far (a personal catalog is
    // loaded), then SB6's additive sync as a detached task (R17). It
    // never waits: an apply in flight skips both, and it never
    // touches `owed` or `seen`. A panic in it is logged and stops
    // only this pass's reconcile (or that one SB6 run), never the
    // tick loop.
    let _ = crate::service::loops::caught_sync(
        "catalog scan tick: the changeset reconcile pass",
        || super::changesets::reconcile::after_scan_pass(store, ssh),
    );
    *seen = Some(now_key);
    let outcome = if owed.is_empty() {
        Ok(())
    } else {
        Err(format!("{} host(s) owe a rescan", owed.len()))
    };
    crate::service::loops::report("catalog_scan", outcome, Some(period));
    Pass::Ran
}

#[cfg(test)]
mod tests {
    use super::*;
    fn h(alias: &str, reachable: bool, last: Option<i64>) -> HostDue {
        HostDue {
            alias: alias.into(),
            reachable,
            hidden: false,
            last_scan: last,
        }
    }

    /// `owe_rescan` parks the host for the tick; the tick's own drain
    /// (`take_requested`, a `mem::take`) is not exercised here because the
    /// set is process-wide and other tests park hosts in it concurrently.
    #[test]
    fn owe_rescan_parks_the_host_for_the_next_pass() {
        let alias = format!("owed-{}", uuid::Uuid::new_v4());
        assert!(!rescan_requested(&alias));
        owe_rescan(&alias);
        assert!(rescan_requested(&alias));
    }

    /// Redesign 8.1: with Pause all on, a pass scans nothing and leaves a
    /// requested rescan where it was.
    #[tokio::test]
    async fn pause_all_stops_the_pass_before_it_scans() {
        let store = Arc::new(Mutex::new(Store::open_in_memory().unwrap()));
        crate::service::settings::set(
            &store.lock().unwrap(),
            crate::service::settings::AUTOMATION_PAUSED,
            "true",
        )
        .unwrap();
        let alias = format!("paused-{}", uuid::Uuid::new_v4());
        owe_rescan(&alias);
        let (mut seen, mut owed) = (None, BTreeSet::new());
        let ssh = Arc::new(SshClient::new());
        let got = pass(&store, &ssh, Duration::from_secs(300), &mut seen, &mut owed).await;
        assert_eq!(got, Pass::Paused);
        assert!(
            rescan_requested(&alias),
            "the request waits for an unpaused pass"
        );
        assert!(seen.is_none() && owed.is_empty());
    }

    /// A pass that leaves a host owed is followed by one ahead of the next
    /// tick: after two minutes, then doubling, and no longer once the wait
    /// would reach the tick's own period. An hourly tick used to be the only
    /// retry, so a hub's first pass (which runs before its hosts' agents
    /// have reconnected) left every agent host unscanned for an hour.
    #[test]
    fn an_owed_host_is_retried_ahead_of_the_next_tick() {
        let period = Duration::from_secs(3600);
        let mins = |m: u64| Duration::from_secs(m * 60);
        let (mut waits, mut backoff) = (Vec::new(), OWED_RETRY);
        for _ in 0..8 {
            let (early, next) = early_retry(true, backoff, period);
            waits.push(early);
            backoff = next;
        }
        let want: Vec<Option<Duration>> = [2, 4, 8, 16, 32]
            .into_iter()
            .map(|m| Some(mins(m)))
            .chain([None; 3])
            .collect();
        assert_eq!(
            waits, want,
            "a host that keeps failing goes back to once per tick"
        );
        // Nothing owed: no early pass, and the next failure starts over.
        assert_eq!(early_retry(false, backoff, period), (None, OWED_RETRY));
        // The shortest tick (five minutes) still gets its early passes.
        let short = Duration::from_secs(300);
        assert_eq!(early_retry(true, OWED_RETRY, short).0, Some(mins(2)));
        assert_eq!(early_retry(true, mins(4), short).0, Some(mins(4)));
        assert_eq!(early_retry(true, mins(8), short).0, None);
    }

    /// `wait` is what the tick selects on beside its ticker: it ends after
    /// the wait it was given, and never without one.
    #[tokio::test(start_paused = true)]
    async fn the_early_wait_ends_only_when_one_is_set() {
        let t0 = tokio::time::Instant::now();
        wait(Some(OWED_RETRY)).await;
        assert_eq!(t0.elapsed(), OWED_RETRY);
        let never = tokio::time::timeout(Duration::from_secs(86_400), wait(None)).await;
        assert!(never.is_err(), "no wait set: only the tick runs a pass");
    }

    #[test]
    fn stale_or_never_scanned_reachable_hosts_are_due() {
        let hosts = vec![
            h("fresh", true, Some(990)),
            h("stale", true, Some(100)),
            h("never", true, None),
            h("down", false, None),
        ];
        assert_eq!(
            hosts_due(1000, &hosts, 500, false),
            vec!["stale".to_string(), "never".to_string()]
        );
    }

    #[test]
    fn a_catalog_or_sync_change_makes_every_reachable_host_due() {
        let hosts = vec![h("fresh", true, Some(990)), h("down", false, Some(990))];
        assert_eq!(
            hosts_due(1000, &hosts, 500, true),
            vec!["fresh".to_string()]
        );
    }

    #[test]
    fn local_is_due_even_when_its_reachable_flag_is_false_and_hidden_never_is() {
        let mut hidden = h("gone", true, None);
        hidden.hidden = true;
        let hosts = vec![h("local", false, None), hidden];
        assert_eq!(
            hosts_due(1000, &hosts, 500, false),
            vec!["local".to_string()]
        );
    }

    #[test]
    fn an_owed_host_is_added_even_when_the_pass_is_unchanged_and_its_scan_is_fresh() {
        // "fresh" has a recent scan and the pass is unchanged, so
        // `hosts_due` alone would not pick it — but it is still owed from a
        // previous failed pass.
        let hosts = vec![h("fresh", true, Some(990))];
        let due = hosts_due(1000, &hosts, 500, false);
        assert!(due.is_empty());
        let owed: BTreeSet<String> = ["fresh".to_string()].into_iter().collect();
        assert_eq!(with_owed(due, &owed, &hosts), vec!["fresh".to_string()]);
    }

    #[test]
    fn an_owed_host_that_is_hidden_or_unreachable_is_not_added() {
        let mut hidden = h("hidden", true, None);
        hidden.hidden = true;
        let unreachable = h("gone", false, None);
        let hosts = vec![hidden, unreachable];
        let owed: BTreeSet<String> = ["hidden".to_string(), "gone".to_string()]
            .into_iter()
            .collect();
        assert_eq!(with_owed(Vec::new(), &owed, &hosts), Vec::<String>::new());
    }

    /// Review w07-1: a deleted or hidden host leaves `owed`; an unreachable
    /// one stays, to be tried once it is back.
    #[test]
    fn a_removed_or_hidden_owed_host_is_no_longer_owed() {
        let mut hidden = h("hidden", true, None);
        hidden.hidden = true;
        let hosts = vec![hidden, h("down", false, None), h("ok", true, None)];
        let mut owed: BTreeSet<String> = ["hidden", "down", "ok", "deleted"]
            .into_iter()
            .map(str::to_string)
            .collect();
        prune_owed(&mut owed, &hosts);
        assert_eq!(
            owed.into_iter().collect::<Vec<_>>(),
            vec!["down".to_string(), "ok".to_string()]
        );
    }

    /// Review w07-1: the backoff is per failure episode. One host owed for
    /// days runs it up to the cap; a second host failing then still gets
    /// the early retry, while the same set owed again does not.
    #[test]
    fn a_newly_owed_host_gets_the_early_retry_whatever_the_backoff() {
        let period = Duration::from_secs(3600);
        let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
        let capped = Duration::from_secs(64 * 60);
        assert_eq!(
            after_pass(&set(&["old"]), &set(&["old"]), capped, period),
            (None, capped)
        );
        assert_eq!(
            after_pass(&set(&["old"]), &set(&["old", "beta"]), capped, period),
            (Some(OWED_RETRY), OWED_RETRY * 2)
        );
        assert_eq!(
            after_pass(&set(&["old"]), &set(&[]), capped, period),
            (None, OWED_RETRY)
        );
    }

    #[test]
    fn no_duplicates_when_a_host_is_both_due_and_owed() {
        let hosts = vec![h("stale", true, Some(1))];
        let due = hosts_due(1000, &hosts, 500, false);
        assert_eq!(due, vec!["stale".to_string()]);
        let owed: BTreeSet<String> = ["stale".to_string()].into_iter().collect();
        assert_eq!(with_owed(due, &owed, &hosts), vec!["stale".to_string()]);
    }
}
