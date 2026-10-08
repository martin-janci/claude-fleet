//! Account usage history (`account_usage_snapshots`, migration 122, redesign
//! step 2.5): one row per successful usage fetch. The in-memory
//! `UsageCache` stays the source of truth while the app runs; these rows are
//! what it is seeded from after a restart and what the Accounts page reads
//! for its 5-hour and weekly history. See `service::account_usage_poll`.

use super::Store;
use crate::service::account_usage::{AccountUsage, Window};
use rusqlite::Result;

/// How long a snapshot is kept: five weeks, so a weekly window's history
/// always spans at least one whole earlier week.
pub const USAGE_HISTORY_KEEP_SECS: i64 = 35 * 86_400;

/// One stored snapshot.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UsageSnapshotRow {
    pub account_uuid: String,
    /// Unix seconds.
    pub fetched_at: i64,
    pub usage: AccountUsage,
    pub subscription: Option<String>,
    pub source_host: Option<String>,
}

const COLS: &str = "account_uuid, fetched_at, five_hour_pct, five_hour_resets_at, \
                    seven_day_pct, seven_day_resets_at, seven_day_opus_pct, \
                    seven_day_opus_resets_at, seven_day_sonnet_pct, \
                    seven_day_sonnet_resets_at, subscription, source_host";

fn window(r: &rusqlite::Row<'_>, pct: usize) -> Result<Option<Window>> {
    let utilization: Option<f64> = r.get(pct)?;
    let resets_at: Option<i64> = r.get(pct + 1)?;
    Ok(utilization.map(|utilization| Window {
        utilization,
        resets_at,
    }))
}

fn row(r: &rusqlite::Row<'_>) -> Result<UsageSnapshotRow> {
    Ok(UsageSnapshotRow {
        account_uuid: r.get(0)?,
        fetched_at: r.get(1)?,
        usage: AccountUsage {
            five_hour: window(r, 2)?,
            seven_day: window(r, 4)?,
            seven_day_opus: window(r, 6)?,
            seven_day_sonnet: window(r, 8)?,
        },
        subscription: r.get(10)?,
        source_host: r.get(11)?,
    })
}

fn pct(w: &Option<Window>) -> Option<f64> {
    w.as_ref().map(|w| w.utilization)
}

fn resets(w: &Option<Window>) -> Option<i64> {
    w.as_ref().and_then(|w| w.resets_at)
}

impl Store {
    /// Record one successful fetch and drop this account's rows older than
    /// [`USAGE_HISTORY_KEEP_SECS`] before it.
    pub fn insert_usage_snapshot(&self, snap: &UsageSnapshotRow) -> Result<()> {
        let u = &snap.usage;
        self.conn
            .prepare_cached(&format!(
                "INSERT INTO account_usage_snapshots ({COLS}) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
            ))?
            .execute(rusqlite::params![
                snap.account_uuid,
                snap.fetched_at,
                pct(&u.five_hour),
                resets(&u.five_hour),
                pct(&u.seven_day),
                resets(&u.seven_day),
                pct(&u.seven_day_opus),
                resets(&u.seven_day_opus),
                pct(&u.seven_day_sonnet),
                resets(&u.seven_day_sonnet),
                snap.subscription,
                snap.source_host,
            ])?;
        self.conn
            .prepare_cached(
                "DELETE FROM account_usage_snapshots \
                 WHERE account_uuid = ?1 AND fetched_at < ?2",
            )?
            .execute(rusqlite::params![
                snap.account_uuid,
                snap.fetched_at - USAGE_HISTORY_KEEP_SECS
            ])?;
        Ok(())
    }

    /// Each account's newest snapshot (what a restart seeds the cache with).
    pub fn latest_usage_snapshots(&self) -> Result<Vec<UsageSnapshotRow>> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {COLS} FROM account_usage_snapshots s \
             WHERE id = (SELECT id FROM account_usage_snapshots \
                         WHERE account_uuid = s.account_uuid \
                         ORDER BY fetched_at DESC, id DESC LIMIT 1) \
             ORDER BY account_uuid"
        ))?;
        let rows = st.query_map([], row)?;
        rows.collect()
    }

    /// One account's snapshots fetched at or after `since`, oldest first.
    pub fn usage_history(&self, account_uuid: &str, since: i64) -> Result<Vec<UsageSnapshotRow>> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {COLS} FROM account_usage_snapshots \
             WHERE account_uuid = ?1 AND fetched_at >= ?2 \
             ORDER BY fetched_at, id"
        ))?;
        let rows = st.query_map(rusqlite::params![account_uuid, since], row)?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(account: &str, at: i64, five: f64) -> UsageSnapshotRow {
        UsageSnapshotRow {
            account_uuid: account.into(),
            fetched_at: at,
            usage: AccountUsage {
                five_hour: Some(Window {
                    utilization: five,
                    resets_at: Some(at + 3600),
                }),
                seven_day: Some(Window {
                    utilization: 40.0,
                    resets_at: None,
                }),
                seven_day_opus: None,
                seven_day_sonnet: None,
            },
            subscription: Some("max".into()),
            source_host: Some("mercury".into()),
        }
    }

    #[test]
    fn a_snapshot_round_trips_with_its_windows_and_reset_times() {
        let s = Store::open_in_memory().unwrap();
        let a = snap("acc-1", 1_000_000, 12.5);
        s.insert_usage_snapshot(&a).unwrap();
        assert_eq!(s.usage_history("acc-1", 0).unwrap(), vec![a]);
    }

    #[test]
    fn latest_is_the_newest_row_per_account() {
        let s = Store::open_in_memory().unwrap();
        s.insert_usage_snapshot(&snap("a", 100, 1.0)).unwrap();
        s.insert_usage_snapshot(&snap("a", 400, 4.0)).unwrap();
        s.insert_usage_snapshot(&snap("b", 200, 2.0)).unwrap();
        let latest = s.latest_usage_snapshots().unwrap();
        let picked: Vec<(&str, i64)> = latest
            .iter()
            .map(|r| (r.account_uuid.as_str(), r.fetched_at))
            .collect();
        assert_eq!(picked, vec![("a", 400), ("b", 200)]);
    }

    #[test]
    fn history_is_oldest_first_from_since_and_prunes_past_retention() {
        let s = Store::open_in_memory().unwrap();
        let now = 10 * USAGE_HISTORY_KEEP_SECS;
        s.insert_usage_snapshot(&snap("a", now - USAGE_HISTORY_KEEP_SECS - 1, 1.0))
            .unwrap();
        s.insert_usage_snapshot(&snap("b", 5, 9.0)).unwrap();
        s.insert_usage_snapshot(&snap("a", now - 100, 2.0)).unwrap();
        s.insert_usage_snapshot(&snap("a", now, 3.0)).unwrap();
        let at: Vec<i64> = s
            .usage_history("a", 0)
            .unwrap()
            .iter()
            .map(|r| r.fetched_at)
            .collect();
        assert_eq!(at, vec![now - 100, now], "the row past retention is gone");
        assert_eq!(s.usage_history("a", now).unwrap().len(), 1);
        assert_eq!(
            s.usage_history("b", 0).unwrap().len(),
            1,
            "pruning is per account"
        );
    }
}
