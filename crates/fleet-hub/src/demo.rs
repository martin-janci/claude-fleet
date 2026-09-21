//! `fleet-hub demo-seed` — fake fleet rows, so a freshly paired client has
//! something to draw.
//!
//! The problem is small and specific. A phone or a desktop paired to a hub that
//! has never run a session shows an empty list, which is indistinguishable from
//! a broken pairing: no sessions, no hosts, no error. Somebody setting the app
//! up for the first time — or a script setting it up unattended — cannot tell
//! "it works and there is nothing here" from "it does not work".
//!
//! Every row written here is marked as demo **by its own name**
//! ([`DEMO_PREFIX`]), and that is the whole mechanism by which [`clear`] can
//! remove exactly what [`seed`] added. There is deliberately no `is_demo`
//! column: a migration to support a development convenience would put the
//! concept in every production database for good.
//!
//! Nothing here adds a `Store` method. Everything uses the public API the hub
//! already has, so this module cannot quietly become a second way to write a
//! session row.

use fleet_core::store::Store;

/// Every demo row's name begins with this.
///
/// Unmistakable next to a real host on purpose. A prefix like `dev-` or `test-`
/// is the sort of thing a real machine is also called, and the prefix is the
/// only thing standing between [`clear`] and somebody's actual fleet.
pub(crate) const DEMO_PREFIX: &str = "demo-";

/// The sessions seeded on each host: `(suffix, claude_status)`.
///
/// One of each state a client draws differently. `blocked` is what the "needs
/// attention" filter keeps, and a client that filters or groups wrongly only
/// shows it when at least one row is in each state — which on a real fleet
/// happens when it happens, not when somebody is looking.
const SESSIONS: &[(&str, &str)] = &[("api", "working"), ("ui", "blocked"), ("docs", "completed")];

/// Hosts to seed, in order. Two is enough to exercise grouping by host.
const HOSTS: &[&str] = &["box", "pine", "mac", "nuc"];

/// What one seeded fleet looks like.
pub(crate) struct Plan {
    pub hosts: usize,
}

impl Default for Plan {
    fn default() -> Self {
        Self { hosts: 2 }
    }
}

/// Insert the demo rows; returns how many sessions were written.
///
/// `now` is a parameter rather than read from the clock so that a test can
/// assert exact timestamps, and so every row in one seed shares a clock.
pub(crate) fn seed(store: &Store, plan: &Plan, now: i64) -> Result<usize, String> {
    let mut sessions = 0;
    for h in 0..plan.hosts {
        let alias = format!("{DEMO_PREFIX}{}", HOSTS[h % HOSTS.len()]);
        store
            .insert_host(&alias, Some(&format!("{alias}.local")))
            .map_err(|e| format!("insert host {alias}: {e}"))?;
        // The second host is left unreachable. A client that draws
        // reachability wrongly reveals it only when something is actually
        // down, which on a demo fleet is never unless it is arranged.
        store
            .update_host_probe(&alias, h == 0, Some("2.0.1"), Some("3.4"), now)
            .map_err(|e| format!("probe host {alias}: {e}"))?;

        let project = store
            .upsert_project(
                "demo",
                &format!("widget-{h}"),
                &format!("/srv/demo/widget-{h}"),
            )
            .map_err(|e| format!("insert project: {e}"))?;

        for (suffix, claude_status) in SESSIONS {
            let name = format!("{DEMO_PREFIX}{suffix}-{h}");
            // `upsert_bg_session` rather than `upsert_session`: it is the one
            // that carries `claude_status`, which is the field every client
            // branches on, and `bg` is a real kind rather than an invented one.
            store
                .upsert_bg_session(
                    &alias,
                    &name,
                    Some(project),
                    &format!("{name}-claude-id"),
                    Some(claude_status),
                    now - 60,
                    "bg",
                    now,
                )
                .map_err(|e| format!("insert session {name}: {e}"))?;
            sessions += 1;
        }
    }
    Ok(sessions)
}

/// Remove every row [`seed`] wrote, by name, and nothing else.
///
/// Sessions first, then hosts: a host row is what a session points at, and
/// removing it first would leave rows naming a host that is gone.
pub(crate) fn clear(store: &Store) -> Result<usize, String> {
    let mut removed = 0;
    for s in store.list_all_sessions().map_err(|e| e.to_string())? {
        if s.tmux_name.starts_with(DEMO_PREFIX) {
            store.delete_session(s.id).map_err(|e| e.to_string())?;
            removed += 1;
        }
    }
    for h in store.list_hosts().map_err(|e| e.to_string())? {
        if h.alias.starts_with(DEMO_PREFIX) {
            store.delete_host(&h.alias).map_err(|e| e.to_string())?;
        }
    }
    Ok(removed)
}

/// Whether this store holds hosts or sessions that [`seed`] did not write.
///
/// The question is "are there rows without the prefix", not "is the database
/// empty": re-seeding a store that holds only demo rows is the ordinary case
/// and must not need `--force`.
pub(crate) fn holds_real_rows(store: &Store) -> Result<bool, String> {
    let hosts = store.list_hosts().map_err(|e| e.to_string())?;
    if hosts.iter().any(|h| !h.alias.starts_with(DEMO_PREFIX)) {
        return Ok(true);
    }
    let sessions = store.list_all_sessions().map_err(|e| e.to_string())?;
    Ok(sessions
        .iter()
        .any(|s| !s.tmux_name.starts_with(DEMO_PREFIX)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_core::events::NoopEventBus;
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open_with_bus(&dir.path().join("state.db"), Arc::new(NoopEventBus)).unwrap();
        (dir, s)
    }

    #[test]
    fn a_seeded_fleet_has_something_of_each_kind() {
        let (_d, s) = store();

        let n = seed(&s, &Plan::default(), 1_000).unwrap();

        assert_eq!(n, SESSIONS.len() * 2, "one set of sessions per host");
        let hosts = s.list_hosts().unwrap();
        assert_eq!(hosts.len(), 2);
        // One reachable and one not: a client that draws reachability wrongly
        // only reveals it when something is actually down.
        assert!(hosts.iter().any(|h| h.reachable));
        assert!(hosts.iter().any(|h| !h.reachable));

        let sessions = s.list_all_sessions().unwrap();
        // And one blocked, which is what the "needs attention" filter keeps.
        assert!(
            sessions
                .iter()
                .any(|x| x.claude_status.as_deref() == Some("blocked")),
            "a demo fleet with nothing blocked cannot exercise the filter"
        );
    }

    /// Re-seeding is the ordinary case and must not need `--force`.
    #[test]
    fn seeding_twice_does_not_multiply_the_fleet() {
        let (_d, s) = store();

        seed(&s, &Plan::default(), 1_000).unwrap();
        assert!(!holds_real_rows(&s).unwrap(), "its own rows are not 'real'");
        seed(&s, &Plan::default(), 2_000).unwrap();

        assert_eq!(s.list_hosts().unwrap().len(), 2, "upserted, not duplicated");
        assert_eq!(s.list_all_sessions().unwrap().len(), SESSIONS.len() * 2);
    }

    /// The claim `--clear` rests on, and the one worth being sure of: it
    /// removes what `seed` wrote and leaves everything else alone.
    #[test]
    fn clear_removes_only_the_demo_rows() {
        let (_d, s) = store();
        s.insert_host("prod-box", None).unwrap();
        s.upsert_bg_session(
            "prod-box",
            "real-work",
            None,
            "rw",
            Some("working"),
            1,
            "bg",
            1,
        )
        .unwrap();
        seed(&s, &Plan::default(), 1_000).unwrap();

        let removed = clear(&s).unwrap();

        assert_eq!(removed, SESSIONS.len() * 2);
        assert_eq!(
            s.list_hosts()
                .unwrap()
                .iter()
                .map(|h| h.alias.clone())
                .collect::<Vec<_>>(),
            vec!["prod-box".to_string()],
            "a real host must survive its neighbours being cleared"
        );
        let left = s.list_all_sessions().unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].tmux_name, "real-work");
    }

    /// The guard that stops a live fleet being seeded.
    #[test]
    fn a_store_with_real_rows_is_recognised() {
        let (_d, s) = store();
        assert!(
            !holds_real_rows(&s).unwrap(),
            "an empty store is not 'real'"
        );

        s.insert_host("prod-box", None).unwrap();

        assert!(holds_real_rows(&s).unwrap(), "a host nobody seeded is real");
    }

    /// …including when the only real thing is a session on a demo host, which
    /// is the case a host-only check would miss.
    #[test]
    fn a_real_session_alone_counts_as_real() {
        let (_d, s) = store();
        seed(&s, &Plan::default(), 1_000).unwrap();
        assert!(!holds_real_rows(&s).unwrap());

        s.upsert_bg_session(
            "demo-box",
            "real-work",
            None,
            "rw",
            Some("working"),
            1,
            "bg",
            1,
        )
        .unwrap();

        assert!(
            holds_real_rows(&s).unwrap(),
            "a session nobody seeded is real even on a seeded host"
        );
    }

    /// Clearing a store that was never seeded is not an error.
    #[test]
    fn clearing_nothing_is_not_an_error() {
        let (_d, s) = store();
        s.insert_host("prod-box", None).unwrap();

        assert_eq!(clear(&s).unwrap(), 0);
        assert_eq!(s.list_hosts().unwrap().len(), 1, "and touches nothing");
    }
}
