//! Work graph retention (M12.3): the sweep the GC tick runs, and what
//! `work_admin { status | sweep_now }` reports. The rules — what is kept and
//! why — are in `store::work_retention`.
//!
//! * **Bounded.** At most [`RETENTION_TICK_CAP`] rows per table per sweep,
//!   deleted [`RETENTION_BATCH`] at a time, each batch under its own lock;
//!   a backlog drains over later ticks.
//! * **Settings only** (D23): three windows in days, `0` = keep forever; no
//!   per-org override.
//! * **Hub-internal.** The GC tick runs it; the only other trigger is the
//!   master's `work_admin { action: sweep_now }` (and the standalone
//!   desktop's own Settings, the same master).

use crate::ipc_error::{lock, IpcError};
use crate::service::settings;
use crate::store::{RetentionTable, Store};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Rows deleted per lock.
pub const RETENTION_BATCH: usize = 200;
/// Rows deleted per table per sweep.
pub const RETENTION_TICK_CAP: usize = 2_000;
/// Where the last sweep is recorded: a `settings` row outside the registry,
/// so no settings path can write it.
pub const LAST_SWEEP_KEY: &str = "internal.work_retention_last_sweep";

/// The three windows, in days (`0` = forever).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionDays {
    pub journal: i64,
    pub tracker_items: i64,
    pub timeline_work_events: i64,
}

fn days_of(s: &Store, key: &str) -> i64 {
    let raw = s.get_setting(key).ok().flatten();
    settings::resolve(key, raw.as_deref())
        .parse()
        .unwrap_or_default()
}

impl RetentionDays {
    pub fn from_store(s: &Store) -> Self {
        // An operator's M2 `work.journal_days` counts until the new key is
        // set, but never shortens the window: it was chosen when confirmed
        // work was kept forever. `0` (forever) stands; otherwise the longer.
        let current = days_of(s, settings::WORK_RETENTION_JOURNAL_DAYS);
        let journal = match s.get_setting(settings::WORK_RETENTION_JOURNAL_DAYS) {
            Ok(Some(_)) => current,
            _ => match s
                .get_setting(settings::LEGACY_WORK_JOURNAL_DAYS)
                .ok()
                .flatten()
                .and_then(|v| v.trim().parse::<i64>().ok())
                .filter(|d| (0..=3650).contains(d))
            {
                Some(0) => 0,
                Some(d) => d.max(current),
                None => current,
            },
        };
        Self {
            journal,
            tracker_items: days_of(s, settings::WORK_RETENTION_TRACKER_ITEMS_DAYS),
            timeline_work_events: days_of(s, settings::WORK_RETENTION_TIMELINE_WORK_EVENTS_DAYS),
        }
    }

    pub fn of(&self, t: RetentionTable) -> i64 {
        match t {
            RetentionTable::Journal => self.journal,
            RetentionTable::TrackerItems => self.tracker_items,
            RetentionTable::WorkEvents => self.timeline_work_events,
        }
    }
}

fn setting_of(t: RetentionTable) -> &'static str {
    match t {
        RetentionTable::Journal => settings::WORK_RETENTION_JOURNAL_DAYS,
        RetentionTable::TrackerItems => settings::WORK_RETENTION_TRACKER_ITEMS_DAYS,
        RetentionTable::WorkEvents => settings::WORK_RETENTION_TIMELINE_WORK_EVENTS_DAYS,
    }
}

/// One sweep's deletions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionSweep {
    pub at: i64,
    #[serde(default)]
    pub journal: usize,
    #[serde(default)]
    pub tracker_items: usize,
    #[serde(default)]
    pub timeline_work_events: usize,
}

impl RetentionSweep {
    fn add(&mut self, t: RetentionTable, n: usize) {
        match t {
            RetentionTable::Journal => self.journal += n,
            RetentionTable::TrackerItems => self.tracker_items += n,
            RetentionTable::WorkEvents => self.timeline_work_events += n,
        }
    }
}

/// One table in `status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionTableStatus {
    pub table: String,
    pub setting: String,
    /// `0` = forever.
    pub days: i64,
    pub rows: i64,
    /// Dry run: what a sweep down to zero would delete now.
    pub would_delete: i64,
}

/// `work_admin { action: status }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionStatus {
    pub tables: Vec<RetentionTableStatus>,
    #[serde(default)]
    pub last_sweep: Option<RetentionSweep>,
    pub tick_cap: usize,
}

/// Row counts, the dry run and the last sweep. One short lock per query.
pub fn status(store: &Mutex<Store>, now: i64) -> Result<RetentionStatus, IpcError> {
    let days = RetentionDays::from_store(&*lock(store)?);
    let mut tables = Vec::new();
    for t in RetentionTable::ALL {
        let rows = lock(store)?.retention_rows(t)?;
        let would_delete = lock(store)?.retention_eligible(t, now, days.of(t))?;
        tables.push(RetentionTableStatus {
            table: t.table().into(),
            setting: setting_of(t).into(),
            days: days.of(t),
            rows,
            would_delete,
        });
    }
    let last_sweep = lock(store)?
        .get_setting(LAST_SWEEP_KEY)?
        .and_then(|v| serde_json::from_str(&v).ok());
    Ok(RetentionStatus {
        tables,
        last_sweep,
        tick_cap: RETENTION_TICK_CAP,
    })
}

/// One sweep at the production caps; recorded as the last sweep.
pub fn sweep(store: &Mutex<Store>, now: i64) -> RetentionSweep {
    sweep_capped(store, now, RETENTION_BATCH, RETENTION_TICK_CAP)
}

/// One sweep: per table, batches of `batch` until `cap` rows or nothing
/// left. Best-effort, like the other GC passes: a failed batch stops that
/// table for this sweep and is logged.
pub fn sweep_capped(store: &Mutex<Store>, now: i64, batch: usize, cap: usize) -> RetentionSweep {
    let mut out = RetentionSweep {
        at: now,
        ..Default::default()
    };
    let days = match store.lock() {
        Ok(s) => RetentionDays::from_store(&s),
        Err(_) => return out,
    };
    for t in RetentionTable::ALL {
        let mut done = 0;
        while done < cap {
            let want = batch.min(cap - done);
            let n = match store.lock() {
                Ok(s) => s.retention_delete_batch(t, now, days.of(t), want),
                Err(_) => break,
            };
            match n {
                Ok(n) => {
                    done += n;
                    if n < want {
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!(table = t.table(), error = %e, "[gc] retention sweep failed");
                    break;
                }
            }
        }
        out.add(t, done);
    }
    if let Ok(s) = store.lock() {
        if let Ok(v) = serde_json::to_string(&out) {
            if let Err(e) = s.set_setting(LAST_SWEEP_KEY, &v) {
                tracing::warn!(error = %e, "[gc] retention sweep not recorded");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Mutex<Store> {
        Mutex::new(Store::open_in_memory().unwrap())
    }

    #[test]
    fn the_legacy_journal_window_never_shortens_and_yields_to_the_new_key() {
        let st = store();
        let days = |st: &Mutex<Store>| RetentionDays::from_store(&st.lock().unwrap());
        assert_eq!(
            days(&st),
            RetentionDays {
                journal: 365,
                tracker_items: 180,
                timeline_work_events: 180
            }
        );
        let legacy = |v: &str| {
            st.lock()
                .unwrap()
                .set_setting(settings::LEGACY_WORK_JOURNAL_DAYS, v)
                .unwrap()
        };
        legacy("30");
        assert_eq!(days(&st).journal, 365, "never shorter than the default");
        legacy("900");
        assert_eq!(days(&st).journal, 900, "a longer M2 window stands");
        legacy("0");
        assert_eq!(days(&st).journal, 0, "forever stands");
        settings::set(
            &st.lock().unwrap(),
            settings::WORK_RETENTION_JOURNAL_DAYS,
            "0",
        )
        .unwrap();
        assert_eq!(days(&st).journal, 0, "the new key wins, 0 included");
    }

    #[test]
    fn the_tick_cap_holds_and_the_dry_run_equals_what_the_sweeps_remove() {
        let st = store();
        for i in 0..13 {
            st.lock()
                .unwrap()
                .append_journal(
                    Some(&format!("c{i}")),
                    None,
                    "progress",
                    "hook",
                    Some("b"),
                    None,
                )
                .unwrap();
        }
        let now = crate::service::catalog::now_secs() + 400 * 86_400;
        let dry = status(&st, now).unwrap().tables[0].would_delete;
        assert_eq!(dry, 13);
        let swept: Vec<usize> = (0..4)
            .map(|_| sweep_capped(&st, now, 2, 5).journal)
            .collect();
        assert_eq!(swept, [5, 5, 3, 0], "at most the cap per sweep");
        assert_eq!(swept.iter().sum::<usize>() as i64, dry);
        assert_eq!(status(&st, now).unwrap().tables[0].rows, 0);
    }

    #[test]
    fn zero_keeps_forever_through_the_settings() {
        let st = store();
        st.lock()
            .unwrap()
            .append_journal(Some("c"), None, "progress", "hook", Some("b"), None)
            .unwrap();
        settings::set(
            &st.lock().unwrap(),
            settings::WORK_RETENTION_JOURNAL_DAYS,
            "0",
        )
        .unwrap();
        let now = crate::service::catalog::now_secs() + 4_000 * 86_400;
        assert_eq!(status(&st, now).unwrap().tables[0].would_delete, 0);
        assert_eq!(sweep(&st, now).journal, 0);
        assert_eq!(status(&st, now).unwrap().tables[0].rows, 1);
    }

    #[test]
    fn a_sweep_is_recorded_and_status_reports_it() {
        let st = store();
        let now = 1_000_000_000;
        assert!(status(&st, now).unwrap().last_sweep.is_none());
        let swept = sweep(&st, now);
        let s = status(&st, now).unwrap();
        assert_eq!(s.last_sweep, Some(swept));
        assert_eq!(
            s.tables
                .iter()
                .map(|t| t.table.as_str())
                .collect::<Vec<_>>(),
            ["work_journal", "work_items", "session_events"]
        );
        assert_eq!(s.tick_cap, RETENTION_TICK_CAP);
        // The record is not a setting anyone can write.
        assert!(settings::set(&st.lock().unwrap(), LAST_SWEEP_KEY, "{}").is_err());
    }
}
