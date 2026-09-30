//! Assets S1a: rescan hosts without anyone pressing Scan. Hourly by
//! default, it rescans every reachable host whose inventory is older than a
//! day, and all of them after the catalog HEAD or a sync changed.

use crate::ssh::SshClient;
use crate::store::Store;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
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
            // A borrow via `with_personal` rather than `personal()`'s full
            // clone: only the `head` string needs to leave the closure,
            // not the whole catalog (assets, resources and all). Nothing
            // loaded, or the registry lock poisoned, both `continue` —
            // this tick just has nothing to compare against yet.
            let head = match super::registry::with_personal(|c| Ok(c.head.clone())) {
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
            let now_key = (head, last_sync);
            let changed = seen.as_ref() != Some(&now_key);
            let due = hosts_due(
                super::now_secs(),
                &hosts,
                setting_secs(&store, CATALOG_SCAN_MAX_AGE_SECS),
                changed,
            );
            let due = with_owed(due, &owed, &hosts);
            for alias in due {
                match super::inventory::scan_hosts(&store, &ssh, Some(&alias)).await {
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
            seen = Some(now_key);
        }
    }))
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
