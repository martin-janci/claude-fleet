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
/// gets retried next tick rather than waiting out `max_age`.
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
        loop {
            tokio::select! {
                biased;
                _ = token.cancelled() => break,
                _ = ticker.tick() => {}
            }
            pass(&store, &ssh, period, &mut seen, &mut owed).await;
        }
    }))
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
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        super::changesets::reconcile::after_scan_pass(store, ssh)
    }))
    .is_err()
    {
        tracing::error!("catalog scan tick: the changeset reconcile pass panicked");
    }
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

    #[test]
    fn no_duplicates_when_a_host_is_both_due_and_owed() {
        let hosts = vec![h("stale", true, Some(1))];
        let due = hosts_due(1000, &hosts, 500, false);
        assert_eq!(due, vec!["stale".to_string()]);
        let owed: BTreeSet<String> = ["stale".to_string()].into_iter().collect();
        assert_eq!(with_owed(due, &owed, &hosts), vec!["stale".to_string()]);
    }
}
