//! Every background loop reports (redesign step 8.1): one registry holds,
//! per loop, when it last ran, when it runs next and how that went, and
//! `fleet_health.loops` reads it. Every loop that acts on the person's
//! behalf (missions, GC, catalog scans, playbooks, the PR shepherd, repairs,
//! syncs, routines, host refresh) also asks [`gate`] first, which answers
//! "paused" while `automation.paused` is on: the one switch a person flips
//! to stop every automatic action at once. Each of them is proven to stop
//! by a behaviour test next to its pass (`pause_all_stops_*`).
//!
//! The rest keep running while paused, each with the reason it says in
//! [`LoopSpec::keeps_running`] (shown in the Automation view): they observe
//! (reconcile, usage polls, update checks), keep the hub's links open, or
//! keep bookkeeping that only records what already happened. [`LOOPS`] says
//! which loop is which; its order is the order `fleet_health` lists them in.
//!
//! The registry is per process, like [`crate::service::tick::tick_stats`]:
//! a desktop that does not run a loop lists it with no run at all.

use crate::service::settings;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// One background loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopSpec {
    pub name: &'static str,
    pub label: &'static str,
    /// Stops while `automation.paused` is on.
    pub pausable: bool,
    /// Why a loop that is not pausable keeps running on Pause all, in a
    /// short sentence the Automation view shows. `None` exactly when
    /// [`Self::pausable`].
    pub keeps_running: Option<&'static str>,
}

/// A loop that acts on the person's behalf: Pause all stops it.
const fn acts(name: &'static str, label: &'static str) -> LoopSpec {
    LoopSpec {
        name,
        label,
        pausable: true,
        keeps_running: None,
    }
}

/// A loop that keeps running on Pause all, and `why`.
const fn keeps(name: &'static str, label: &'static str, why: &'static str) -> LoopSpec {
    LoopSpec {
        name,
        label,
        pausable: false,
        keeps_running: Some(why),
    }
}

/// Every loop, in the order health lists them. A name here must be
/// reported by its loop, and a pausable one gated
/// (`tests::every_loop_reports_and_every_pausable_one_is_gated`) and
/// stopped (each loop's `pause_all_stops_*` test).
pub const LOOPS: &[LoopSpec] = &[
    keeps(
        "reconcile",
        "Reconcile",
        "Only reads what each host runs; it starts and stops nothing.",
    ),
    keeps(
        "stale_working",
        "Stale working",
        "Only bookkeeping: a session nothing has moved for a while reads Idle.",
    ),
    keeps(
        "forms",
        "Chat forms expiry",
        "Expires unanswered forms and deletes their secrets from hosts; pausing would leave secrets behind.",
    ),
    acts("playbooks", "Stuck playbooks"),
    acts("pr_shepherd", "PR shepherd"),
    acts("gc", "Garbage collection"),
    keeps("usage", "Session usage", "Only reads token usage."),
    keeps(
        "search_index",
        "Search index",
        "Only copies conversation text into the search index, when search.index_transcripts is on.",
    ),
    keeps(
        "tasks",
        "Task sweep",
        "Only bookkeeping: a task whose worker is gone reads Failed.",
    ),
    keeps(
        "reports",
        "Error reports",
        "Only collects this hub's own errors and ages old ones out.",
    ),
    acts("repair", "Worktree repair"),
    acts("worktree_prune", "Worktree prune"),
    keeps("account_usage", "Account usage", "Only reads account limits."),
    acts("trackers", "Tracker sync"),
    acts("missions", "Missions"),
    acts("routines", "Routines"),
    acts("catalog_scan", "Catalog sync"),
    acts("local_sync", "Local folder sync"),
    acts("reprovision", "Host refresh"),
    keeps(
        "peers",
        "Hub links",
        "Keeps linked hubs connected; what comes over a link is a person's request.",
    ),
    keeps("updates", "Update check", "Only checks for updates; it installs nothing."),
];

/// [`LoopHealth::result`] values.
pub const RESULT_OK: &str = "ok";
pub const RESULT_ERROR: &str = "error";
pub const RESULT_PAUSED: &str = "paused";

/// One row of `fleet_health.loops`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopHealth {
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub pausable: bool,
    /// [`LoopSpec::keeps_running`]: why it runs on while paused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keeps_running: Option<String>,
    /// When it last ran or was skipped as paused (unix seconds).
    #[serde(default)]
    pub last_run_at: Option<i64>,
    /// When it is due next, when its period is known.
    #[serde(default)]
    pub next_run_at: Option<i64>,
    /// [`RESULT_OK`], [`RESULT_ERROR`] or [`RESULT_PAUSED`]; `None` before
    /// its first run in this process.
    #[serde(default)]
    pub result: Option<String>,
    /// The last failure's text, kept after a later success.
    #[serde(default)]
    pub last_error: Option<String>,
    /// Runs since the process started, paused passes excluded.
    #[serde(default)]
    pub runs: u64,
    #[serde(default)]
    pub failures: u64,
}

/// How one pass went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Error(String),
    Paused,
}

impl<E: std::fmt::Display> From<Result<(), E>> for Outcome {
    fn from(r: Result<(), E>) -> Self {
        match r {
            Ok(()) => Outcome::Ok,
            Err(e) => Outcome::Error(e.to_string()),
        }
    }
}

#[derive(Debug, Default, Clone)]
struct State {
    last_run_at: Option<i64>,
    next_run_at: Option<i64>,
    result: Option<&'static str>,
    last_error: Option<String>,
    runs: u64,
    failures: u64,
}

/// The loops' last runs.
#[derive(Default)]
pub struct Registry {
    rows: Mutex<HashMap<&'static str, State>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a pass of `name` at `now`; `next_in` is its period when known.
    /// A name [`LOOPS`] does not list is ignored.
    pub fn record(&self, name: &str, now: i64, outcome: Outcome, next_in: Option<Duration>) {
        let Some(spec) = find(name) else {
            debug_assert!(false, "loop {name:?} is not in LOOPS");
            return;
        };
        let Ok(mut rows) = self.rows.lock() else {
            return;
        };
        let st = rows.entry(spec.name).or_default();
        st.last_run_at = Some(now);
        st.next_run_at = next_in.map(|d| now + d.as_secs() as i64);
        match outcome {
            Outcome::Ok => {
                st.runs += 1;
                st.result = Some(RESULT_OK);
            }
            Outcome::Error(e) => {
                st.runs += 1;
                st.failures += 1;
                st.result = Some(RESULT_ERROR);
                st.last_error = Some(e);
            }
            Outcome::Paused => st.result = Some(RESULT_PAUSED),
        }
    }

    /// One row per [`LOOPS`] entry, in its order.
    pub fn snapshot(&self) -> Vec<LoopHealth> {
        let rows = self.rows.lock().map(|r| r.clone()).unwrap_or_default();
        LOOPS
            .iter()
            .map(|spec| {
                let st = rows.get(spec.name).cloned().unwrap_or_default();
                LoopHealth {
                    name: spec.name.into(),
                    label: spec.label.into(),
                    pausable: spec.pausable,
                    keeps_running: spec.keeps_running.map(str::to_string),
                    last_run_at: st.last_run_at,
                    next_run_at: st.next_run_at,
                    result: st.result.map(str::to_string),
                    last_error: st.last_error,
                    runs: st.runs,
                    failures: st.failures,
                }
            })
            .collect()
    }
}

/// The process's registry.
pub fn registry() -> &'static Registry {
    static REG: OnceLock<Registry> = OnceLock::new();
    REG.get_or_init(Registry::new)
}

pub fn find(name: &str) -> Option<&'static LoopSpec> {
    LOOPS.iter().find(|l| l.name == name)
}

/// `automation.paused`.
pub fn paused(s: &Store) -> bool {
    settings::get_bool(s, settings::AUTOMATION_PAUSED)
}

/// Whether `name` may run now, from a store the caller already holds.
/// While paused, a pausable loop is recorded as paused and told no.
pub fn gate_in(name: &str, s: &Store, next_in: Option<Duration>) -> bool {
    let pausable = find(name).is_some_and(|l| l.pausable);
    if pausable && paused(s) {
        registry().record(name, now_unix(), Outcome::Paused, next_in);
        return false;
    }
    true
}

/// [`gate_in`] taking the store's lock for one read. Never call it with the
/// guard held. A poisoned lock does not pause anything.
pub fn gate(name: &str, store: &Mutex<Store>, next_in: Option<Duration>) -> bool {
    match store.lock() {
        Ok(s) => gate_in(name, &s, next_in),
        Err(_) => true,
    }
}

/// Record a finished pass of `name` in the process's registry.
pub fn report(name: &str, outcome: impl Into<Outcome>, next_in: Option<Duration>) {
    registry().record(name, now_unix(), outcome.into(), next_in);
}

fn now_unix() -> i64 {
    crate::store::now_unix()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row<'a>(rows: &'a [LoopHealth], name: &str) -> &'a LoopHealth {
        rows.iter().find(|r| r.name == name).unwrap()
    }

    #[test]
    fn a_pass_records_its_time_next_run_and_result() {
        let reg = Registry::new();
        reg.record("gc", 100, Outcome::Ok, Some(Duration::from_secs(60)));
        reg.record("missions", 100, Outcome::Error("no host".into()), None);
        reg.record("missions", 120, Outcome::Ok, None);
        let rows = reg.snapshot();
        assert_eq!(rows.len(), LOOPS.len(), "every loop is listed, run or not");
        let gc = row(&rows, "gc");
        assert_eq!(
            (
                gc.last_run_at,
                gc.next_run_at,
                gc.result.as_deref(),
                gc.runs
            ),
            (Some(100), Some(160), Some(RESULT_OK), 1)
        );
        let m = row(&rows, "missions");
        assert_eq!((m.runs, m.failures), (2, 1));
        assert_eq!(m.result.as_deref(), Some(RESULT_OK));
        assert_eq!(
            m.last_error.as_deref(),
            Some("no host"),
            "kept after a success"
        );
        let r = row(&rows, "reconcile");
        assert_eq!((r.last_run_at, r.result.as_ref()), (None, None));
    }

    #[test]
    fn a_paused_pass_is_not_a_run() {
        let reg = Registry::new();
        reg.record("gc", 5, Outcome::Paused, None);
        let gc = &reg.snapshot()[LOOPS.iter().position(|l| l.name == "gc").unwrap()];
        assert_eq!((gc.result.as_deref(), gc.runs), (Some(RESULT_PAUSED), 0));
        assert_eq!(gc.last_run_at, Some(5));
    }

    #[test]
    fn pause_all_stops_pausable_loops_only() {
        let store = Mutex::new(Store::open_in_memory().unwrap());
        assert!(gate("gc", &store, None), "not paused by default");
        settings::set(&store.lock().unwrap(), settings::AUTOMATION_PAUSED, "true").unwrap();
        for l in LOOPS {
            assert_eq!(
                gate(l.name, &store, None),
                !l.pausable,
                "{} while paused",
                l.name
            );
        }
        let rows = registry().snapshot();
        assert_eq!(
            row(&rows, "catalog_scan").result.as_deref(),
            Some(RESULT_PAUSED)
        );
    }

    /// Every loop is either stopped by Pause all or says why it is not.
    #[test]
    fn every_loop_that_keeps_running_says_why() {
        for l in LOOPS {
            assert_eq!(l.pausable, l.keeps_running.is_none(), "{}", l.name);
            if let Some(why) = l.keeps_running {
                assert!(why.ends_with('.') && why.len() < 120, "{}: {why}", l.name);
            }
        }
        let rows = registry().snapshot();
        assert_eq!(
            row(&rows, "reconcile").keeps_running.as_deref(),
            LOOPS[0].keeps_running
        );
        assert_eq!(row(&rows, "gc").keeps_running, None);
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<_> = LOOPS.iter().map(|l| l.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), LOOPS.len());
    }

    /// The crate's sources, as (path, text).
    fn sources() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let text = std::fs::read_to_string(&p).unwrap();
                    out.push((p.display().to_string(), text));
                }
            }
        }
        let mut out = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut out,
        );
        out
    }

    /// Every loop in [`LOOPS`] reports from somewhere in the crate, every
    /// pausable one asks the gate, and no call site names a loop the
    /// table lacks (a typo would report into nothing).
    #[test]
    fn every_loop_reports_and_every_pausable_one_is_gated() {
        let src = sources();
        let calls = |prefix: &str| -> Vec<String> {
            let mut names = Vec::new();
            for (path, text) in &src {
                // By component, not by string: Windows spells it `service\loops.rs`.
                if std::path::Path::new(path)
                    .ends_with(std::path::Path::new("service").join("loops.rs"))
                {
                    continue;
                }
                for part in text.split(prefix).skip(1) {
                    if let Some(name) = part
                        .trim_start()
                        .strip_prefix('"')
                        .and_then(|p| p.split('"').next())
                    {
                        names.push(name.to_string());
                    }
                }
            }
            names
        };
        let reports = calls("loops::report(");
        let gates: Vec<String> = calls("loops::gate(")
            .into_iter()
            .chain(calls("loops::gate_in("))
            .collect();
        for l in LOOPS {
            assert!(
                reports.iter().any(|n| n == l.name),
                "{} never reports",
                l.name
            );
            if l.pausable {
                assert!(
                    gates.iter().any(|n| n == l.name),
                    "{} is never gated",
                    l.name
                );
            }
        }
        for n in reports.iter().chain(&gates) {
            assert!(find(n).is_some(), "{n:?} is not in LOOPS");
        }
    }
}
