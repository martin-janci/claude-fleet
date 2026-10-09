//! Per-worktree sizes from the host probe (Orbit Fleet M15, G1.9): what
//! removing one session's worktree would free, for Tidy's "frees about
//! 2.1 GB" (and later Archive and a finished mission).
//!
//! The reconcile pass already asks each host, once per
//! [`super::reconcile::WORKTREE_SIZE_REFRESH_SECS`], how much disk fleet's
//! worktrees hold (`hosts.worktree_kb`). The same `du` now answers per path,
//! and the per-path answers are kept HERE, in memory, rather than in a
//! table: they are a measurement hours old by design, nothing else needs
//! them durable, and a process that starts without them simply measures
//! the host on its first pass (see [`known`]).
//!
//! Keyed by `(host alias, worktree path as the store spells it)`. A path the
//! last measurement did not cover reads `None` ("not measured"), never 0.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

#[derive(Debug, Default)]
struct HostSizes {
    /// Unix seconds of the measurement.
    at: i64,
    by_path: HashMap<String, i64>,
}

static SIZES: LazyLock<Mutex<HashMap<String, HostSizes>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Replace `host`'s measurement with `sizes` (`(path, kB)`), taken at `at`.
/// An attempt that measured nothing (no worktrees, or `du` did not answer)
/// still records the host, so [`known`] holds and the next try waits the
/// full interval, as the host total's stamp does.
pub(crate) fn record(host: &str, at: i64, sizes: impl IntoIterator<Item = (String, i64)>) {
    let entry = HostSizes {
        at,
        by_path: sizes.into_iter().collect(),
    };
    if let Ok(mut m) = SIZES.lock() {
        m.insert(host.to_string(), entry);
    }
}

/// This process has measured `host` (or tried to) since it started.
pub(crate) fn known(host: &str) -> bool {
    SIZES.lock().is_ok_and(|m| m.contains_key(host))
}

/// The last measured size of the worktree at `path` on `host`, in kB, and
/// when it was measured. `None` = not measured.
pub fn size_kb(host: &str, path: &str) -> Option<(i64, i64)> {
    let m = SIZES.lock().ok()?;
    let h = m.get(host)?;
    h.by_path.get(path).map(|kb| (*kb, h.at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_measurement_replaces_the_hosts_last_and_a_missing_path_is_unmeasured() {
        let host = "wt-sizes-test-host";
        assert!(!known(host));
        record(
            host,
            100,
            [("/w/a".to_string(), 2048), ("/w/b".to_string(), 0)],
        );
        assert!(known(host));
        assert_eq!(size_kb(host, "/w/a"), Some((2048, 100)));
        assert_eq!(size_kb(host, "/w/b"), Some((0, 100)));
        assert_eq!(size_kb(host, "/w/c"), None);
        assert_eq!(size_kb("wt-sizes-other-host", "/w/a"), None);
        // A failed read records the host with nothing: known, unmeasured.
        record(host, 200, []);
        assert!(known(host));
        assert_eq!(size_kb(host, "/w/a"), None);
    }
}
