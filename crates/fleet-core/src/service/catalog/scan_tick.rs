//! Assets S1a: rescan hosts without anyone pressing Scan. Hourly by
//! default, it rescans every reachable host whose inventory is older than a
//! day, and all of them after the catalog HEAD or a sync changed.

use crate::ipc_error::IpcError;
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

fn setting_secs(store: &Mutex<Store>, key: &str) -> i64 {
    store
        .lock()
        .map(|s| crate::service::settings::get_secs(&s, key) as i64)
        .unwrap_or(0)
}

/// Whether the catalog HEAD or the last sync moved since the previous pass.
///
/// Named so the arm is testable: the tick's own trigger for "rescan every
/// host" is this comparison, and nothing drove it before.
///
/// The FIRST pass is not a change. `tokio::time::interval`'s first tick
/// completes immediately and `seen` starts as `None`, so every process start
/// swept every reachable host over SSH to learn what it already had in the
/// store — and a hub that restarts often never stopped scanning. The first
/// pass only primes the key; what is genuinely stale is `hosts_due`'s own
/// job, through `catalog.scan_max_age_secs`, and that still runs.
fn changed(seen: Option<&ScanKey>, now: &ScanKey) -> bool {
    seen.is_some() && seen != Some(now)
}

/// The catalog HEAD plus the last sync's finish time — what a pass compares
/// against the previous one.
type ScanKey = (String, Option<i64>);

/// Record how one host's rescan went: a host that did not come back fully
/// scanned stays OWED, independent of `hosts_due`'s staleness check, so the
/// next pass retries it.
///
/// Extracted from the tick's loop body because that body is the only thing
/// that populates and clears `owed`, and no test drove it — the six tests
/// around it all call the pure helpers with hand-built sets.
fn settle(
    owed: &mut BTreeSet<String>,
    alias: &str,
    res: &Result<Vec<crate::service::catalog::inventory::HostScanResult>, IpcError>,
) {
    match res {
        Ok(results) => match results.iter().find(|r| r.host == alias) {
            // The only outcome that clears the debt.
            Some(r) if r.status == "scanned" => {
                owed.remove(alias);
            }
            Some(r) => {
                owed.insert(alias.to_string());
                tracing::warn!(
                    host = %alias,
                    status = %r.status,
                    detail = ?r.detail,
                    "catalog scan tick: host not fully scanned; retrying next pass"
                );
            }
            // Asked for one host and it is not in the answer at all.
            None => {
                owed.insert(alias.to_string());
                tracing::warn!(
                    host = %alias,
                    "catalog scan tick: host missing from its own scan result; retrying next pass"
                );
            }
        },
        Err(e) => {
            owed.insert(alias.to_string());
            tracing::warn!(host = %alias, "catalog scan tick: {}", e.message);
        }
    }
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
    use crate::service::settings::{CATALOG_SCAN_CHECK_SECS, CATALOG_SCAN_MAX_AGE_SECS};
    let check = setting_secs(&store, CATALOG_SCAN_CHECK_SECS);
    if check <= 0 {
        tracing::info!("catalog scan tick disabled (catalog.scan_check_secs=0)");
        return None;
    }
    let period = Duration::from_secs(check.max(300) as u64);
    Some(crate::rt::spawn(async move {
        let mut ticker = tokio::time::interval(period);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut seen: Option<ScanKey> = None;
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
            // A separate process loads the catalog and writes the record
            // (`fleet-hub catalog set` / `catalog reload`), so without this the
            // registry copy in THIS process is left behind for good: the "HEAD
            // changed" trigger below could never fire from a CLI reload, and
            // every `compute_states` diff ran against a superseded asset set.
            // `spawn_blocking` because the refresh runs git when the record has
            // actually moved; the common path is cheap. `ensure_fresh` loads
            // EVERY catalog (Assets M3), which is the set `heads_key` below
            // then reads.
            {
                let s = Arc::clone(&store);
                match tokio::task::spawn_blocking(move || super::ensure_fresh(&s)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        tracing::warn!("catalog scan tick: refresh: {}", e.message);
                    }
                    Err(e) => tracing::warn!("catalog scan tick: refresh panicked: {e}"),
                }
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
                Err(_) => continue,
            };
            // All store reads in one scoped guard, dropped before any await.
            let (hosts, last_sync) = {
                let Ok(s) = store.lock() else { continue };
                let Ok(list) = s.list_hosts() else { continue };
                let last = s.inventory_last_scans().unwrap_or_default();
                let sync = s.last_sync_run().ok().flatten().map(|r| r.finished_at);
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
            let now_key: ScanKey = (head, last_sync);
            let changed = changed(seen.as_ref(), &now_key);
            let due = hosts_due(
                super::now_secs(),
                &hosts,
                setting_secs(&store, CATALOG_SCAN_MAX_AGE_SECS),
                changed,
            );
            owed.extend(take_requested());
            let due = with_owed(due, &owed, &hosts);
            for alias in due {
                // Between hosts, too. The token was read once at the top of a
                // pass, so a shutdown (or a cancelled sync) during a sweep of
                // twenty hosts still went to all twenty over SSH. The host
                // already in flight finishes — `scan_hosts` builds its own
                // token — and stays owed, so the next pass retries it.
                if token.is_cancelled() {
                    owed.insert(alias);
                    break;
                }
                let res = super::inventory::scan_hosts(&store, &ssh, Some(&alias)).await;
                settle(&mut owed, &alias, &res);
            }
            seen = Some(now_key);
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scanned(host: &str, status: &str) -> crate::service::catalog::inventory::HostScanResult {
        crate::service::catalog::inventory::HostScanResult {
            host: host.into(),
            status: status.into(),
            detail: None,
            rows: 0,
        }
    }

    /// Every arm of the owed-set bookkeeping. Before this it lived inline in
    /// the spawned task, which no test drove: the arms around it were all
    /// exercised through the pure helpers with hand-built sets, so the one
    /// piece of real state the tick carries across passes was untested.
    #[test]
    fn settle_clears_a_debt_only_on_a_full_scan() {
        let mut owed = BTreeSet::new();

        // a clean scan clears
        owed.insert("a".to_string());
        settle(&mut owed, "a", &Ok(vec![scanned("a", "scanned")]));
        assert!(owed.is_empty(), "a scanned host owes nothing");

        // a partial status keeps the debt
        settle(&mut owed, "a", &Ok(vec![scanned("a", "skipped")]));
        assert!(owed.contains("a"), "skipped stays owed");
        owed.clear();
        settle(&mut owed, "a", &Ok(vec![scanned("a", "failed")]));
        assert!(owed.contains("a"), "failed stays owed");

        // the host missing from its OWN result is owed, not silently cleared
        owed.clear();
        settle(&mut owed, "a", &Ok(vec![scanned("b", "scanned")]));
        assert!(owed.contains("a"), "another host's success is not ours");

        // an empty answer is the same case
        owed.clear();
        settle(&mut owed, "a", &Ok(vec![]));
        assert!(owed.contains("a"));

        // an error is owed
        owed.clear();
        settle(&mut owed, "a", &Err(IpcError::new("E_SCAN", "boom")));
        assert!(owed.contains("a"));

        // and one host's outcome never disturbs another's debt
        owed.clear();
        owed.insert("keep".to_string());
        settle(&mut owed, "a", &Ok(vec![scanned("a", "scanned")]));
        assert_eq!(
            owed.iter().cloned().collect::<Vec<_>>(),
            vec!["keep".to_string()]
        );
    }

    /// The "rescan everything" trigger: the catalog HEAD or the last sync
    /// moving since the previous pass.
    ///
    /// The first assertion was the other way round — "the first pass has
    /// nothing to compare" — which, with `interval`'s immediately-completing
    /// first tick, made every process start sweep every reachable host over SSH
    /// to learn what the store already held. Nothing MOVED at a start, and
    /// staleness is `hosts_due`'s job; changing this assertion is the fix.
    #[test]
    fn changed_fires_on_a_new_head_or_a_new_sync() {
        let a: ScanKey = ("head-a".into(), Some(10));
        let b: ScanKey = ("head-b".into(), Some(10));
        let a_later: ScanKey = ("head-a".into(), Some(20));
        assert!(
            !changed(None, &a),
            "the first pass primes the key; it is not a change"
        );
        assert!(
            !changed(Some(&a), &a),
            "an unchanged key does not re-trigger"
        );
        assert!(changed(Some(&a), &b), "a new catalog HEAD triggers");
        assert!(changed(Some(&a), &a_later), "a newer sync triggers");
        let never_synced: ScanKey = ("head-a".into(), None);
        assert!(changed(Some(&never_synced), &a), "first sync triggers");
    }

    fn h(alias: &str, reachable: bool, last: Option<i64>) -> HostDue {
        HostDue {
            alias: alias.into(),
            reachable,
            hidden: false,
            last_scan: last,
        }
    }

    /// `owe_rescan` parks a host, and the tick's own drain takes it.
    ///
    /// The drain — `owed.extend(take_requested())`, a `mem::take` of the
    /// process-wide set — was exercised by nothing, as this test's own doc
    /// comment used to concede: it is the only mechanism by which an
    /// out-of-band "rescan this host" request ever reaches a pass, so a
    /// `mem::take` that took the wrong thing, or a drain dropped from the loop,
    /// would have been invisible.
    ///
    /// `REQUESTED` is process-wide, so this is the ONE test that touches it:
    /// a second one parking an alias could have it taken from under it. Anything
    /// else the drain happens to pick up is parked again.
    #[test]
    fn owe_rescan_parks_a_host_and_the_tick_drains_it() {
        let alias = format!("owed-{}", uuid::Uuid::new_v4());
        assert!(!rescan_requested(&alias));
        owe_rescan(&alias);
        assert!(rescan_requested(&alias));

        // What the tick does with it, on the line the loop runs.
        let mut owed: BTreeSet<String> = BTreeSet::new();
        owed.insert("already-owed".to_string());
        let taken = take_requested();
        let others: Vec<String> = taken.iter().filter(|a| **a != alias).cloned().collect();
        owed.extend(taken);
        assert!(owed.contains(&alias), "the request reached the pass");
        assert!(
            owed.contains("already-owed"),
            "and did not replace the debt"
        );
        assert!(
            !rescan_requested(&alias),
            "a request is taken once: the next pass does not re-scan for it"
        );
        for a in others {
            owe_rescan(&a);
        }
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
