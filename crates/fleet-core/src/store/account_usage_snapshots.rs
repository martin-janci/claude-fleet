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

/// What a usage report calls an account, and whether a limit stops it.
///
/// Read from the columns `accounts` has held since migration 028, not as an
/// `AccountRow`: that joins tables of migrations 217 and 218, and a report
/// over the history is also run on an older database (`--db`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UsageAccount {
    pub uuid: String,
    /// The nickname a person gave it, else its email, else its display name,
    /// else its uuid.
    pub name: String,
    /// Past its limit the account spends pay-as-you-go money instead of
    /// stopping (`accounts.has_extra_usage`).
    pub has_extra_usage: bool,
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
            // One index seek per account, not a correlated subquery per
            // history row: 35 days of five-minute snapshots made this a
            // 30-50 ms scan on every over-limit check (review r16).
            "SELECT {COLS} FROM account_usage_snapshots s \
             WHERE s.id IN (SELECT (SELECT id FROM account_usage_snapshots \
                                    WHERE account_uuid = a.account_uuid \
                                    ORDER BY fetched_at DESC, id DESC LIMIT 1) \
                            FROM (SELECT DISTINCT account_uuid \
                                  FROM account_usage_snapshots) a) \
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

    /// Every account's snapshots fetched at or after `since`, and before
    /// them each account's newest one from before `since` (what it read as
    /// `since` began): account by account, each oldest first
    /// (`service::limit_census`).
    pub fn usage_history_all(&self, since: i64) -> Result<Vec<UsageSnapshotRow>> {
        let mut st = self.conn.prepare_cached(&format!(
            // The newest-before rows by one index seek per account, as in
            // `latest_usage_snapshots`.
            "SELECT {COLS} FROM account_usage_snapshots WHERE fetched_at >= ?1 \
             UNION ALL \
             SELECT {COLS} FROM account_usage_snapshots s \
             WHERE s.id IN (SELECT (SELECT id FROM account_usage_snapshots \
                                    WHERE account_uuid = a.account_uuid AND fetched_at < ?1 \
                                    ORDER BY fetched_at DESC, id DESC LIMIT 1) \
                            FROM (SELECT DISTINCT account_uuid \
                                  FROM account_usage_snapshots) a) \
             ORDER BY 1, 2"
        ))?;
        let rows = st.query_map(rusqlite::params![since], row)?;
        rows.collect()
    }

    /// The accounts a login is signed in to now: each host's own and its
    /// profiles', hidden hosts left out, by uuid. Read from columns `hosts`
    /// has held since migration 114, for the reason [`UsageAccount`] gives.
    pub fn usage_login_accounts(&self) -> Result<Vec<String>> {
        let mut st = self
            .conn
            .prepare_cached("SELECT account_uuid, claude_profiles FROM hosts WHERE hidden = 0")?;
        let mut accounts = std::collections::BTreeSet::new();
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let own: Option<String> = r.get(0)?;
            let profiles: Option<String> = r.get(1)?;
            let profiles = profiles
                .and_then(|t| serde_json::from_str::<Vec<super::HostProfileRow>>(&t).ok())
                .unwrap_or_default();
            accounts.extend(own);
            accounts.extend(profiles.into_iter().filter_map(|p| p.account_uuid));
        }
        Ok(accounts.into_iter().collect())
    }

    /// Every known account by the name a report prints, by uuid.
    pub fn usage_accounts(&self) -> Result<Vec<UsageAccount>> {
        let mut st = self.conn.prepare_cached(
            "SELECT uuid, nickname, email, display_name, has_extra_usage \
             FROM accounts ORDER BY uuid",
        )?;
        let rows = st.query_map([], |r| {
            let uuid: String = r.get(0)?;
            let named = |i: usize| -> Result<Option<String>> {
                let v: Option<String> = r.get(i)?;
                Ok(v.filter(|v| !v.trim().is_empty()))
            };
            let name = match (named(1)?, named(2)?, named(3)?) {
                (Some(n), _, _) | (None, Some(n), _) | (None, None, Some(n)) => n,
                (None, None, None) => uuid.clone(),
            };
            Ok(UsageAccount {
                uuid,
                name,
                has_extra_usage: r.get(4)?,
            })
        })?;
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

    #[test]
    fn every_accounts_history_comes_account_by_account_with_what_it_read_as_it_began() {
        let s = Store::open_in_memory().unwrap();
        for (account, at) in [
            ("b", 300),
            ("a", 400),
            ("b", 100),
            ("a", 200),
            ("a", 50),
            ("a", 20),
            ("c", 10),
            ("d", 500),
        ] {
            s.insert_usage_snapshot(&snap(account, at, 1.0)).unwrap();
        }
        let history = |since: i64| -> Vec<(String, i64)> {
            s.usage_history_all(since)
                .unwrap()
                .into_iter()
                .map(|r| (r.account_uuid, r.fetched_at))
                .collect()
        };
        let named = |want: &[(&str, i64)]| -> Vec<(String, i64)> {
            want.iter().map(|(a, at)| (a.to_string(), *at)).collect()
        };
        assert_eq!(
            history(100),
            named(&[
                ("a", 50),
                ("a", 200),
                ("a", 400),
                ("b", 100),
                ("b", 300),
                ("c", 10),
                ("d", 500)
            ]),
            "from `since` on, inclusive, and the one reading before it: a's of 50, not of 20"
        );
        assert_eq!(history(0).len(), 8, "nothing is before the first");
        assert_eq!(
            history(1_000),
            named(&[("a", 400), ("b", 300), ("c", 10), ("d", 500)]),
            "an account with nothing since is there with its last reading"
        );
    }

    #[test]
    fn the_accounts_logins_use_now_are_each_hosts_own_and_its_profiles() {
        use crate::store::HostProfileRow;
        let s = Store::open_in_memory().unwrap();
        let profile = |name: &str, account: Option<&str>| HostProfileRow {
            name: name.into(),
            account_uuid: account.map(Into::into),
            email: None,
            credential: None,
        };
        for host in ["h1", "h2", "h3", "gone"] {
            s.insert_host(host, Some(host)).unwrap();
        }
        for uuid in ["own", "old"] {
            s.upsert_account(&crate::store::AccountRow {
                uuid: uuid.into(),
                ..Default::default()
            })
            .unwrap();
        }
        s.set_host_account("h1", Some("own")).unwrap();
        s.set_host_profiles(
            "h1",
            &[profile("work", Some("spare")), profile("new", None)],
        )
        .unwrap();
        // The same account on a second host, as a profile.
        s.set_host_profiles("h2", &[profile("main", Some("own"))])
            .unwrap();
        // h3 has no login read yet; a hidden host's logins are nobody's.
        s.set_host_account("gone", Some("old")).unwrap();
        s.set_host_profiles("gone", &[profile("p", Some("older"))])
            .unwrap();
        assert_eq!(
            s.usage_login_accounts().unwrap(),
            vec!["old", "older", "own", "spare"]
        );
        s.set_host_hidden("gone", true).unwrap();
        assert_eq!(s.usage_login_accounts().unwrap(), vec!["own", "spare"]);
    }

    #[test]
    fn an_account_is_called_by_its_nickname_then_email_then_display_name_then_uuid() {
        use crate::store::AccountRow;
        let s = Store::open_in_memory().unwrap();
        let account = |uuid: &str, email: Option<&str>, display: Option<&str>| AccountRow {
            uuid: uuid.into(),
            email: email.map(Into::into),
            display_name: display.map(Into::into),
            has_extra_usage: uuid == "b",
            ..Default::default()
        };
        s.upsert_account(&account("a", Some("ada@x"), Some("Ada")))
            .unwrap();
        s.upsert_account(&account("b", Some("bob@x"), Some("Bob")))
            .unwrap();
        s.upsert_account(&account("c", None, Some("Cy"))).unwrap();
        s.upsert_account(&account("d", None, None)).unwrap();
        s.upsert_account(&account("e", Some(" "), Some("Eve")))
            .unwrap();
        s.set_account_nickname("a", Some("work")).unwrap();
        let got: Vec<(String, String, bool)> = s
            .usage_accounts()
            .unwrap()
            .into_iter()
            .map(|a| (a.uuid, a.name, a.has_extra_usage))
            .collect();
        let want = [
            ("a", "work", false),
            ("b", "bob@x", true),
            ("c", "Cy", false),
            ("d", "d", false),
            ("e", "Eve", false),
        ];
        assert_eq!(got, want.map(|(u, n, x)| (u.to_string(), n.to_string(), x)));
    }
}
