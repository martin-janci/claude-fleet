//! Autofix findings (service/autofix): one row per episode of a detected
//! broken state. See `migrations/195_autofix.sql`.

use super::Store;
use rusqlite::{OptionalExtension, Result};

/// The states in which a row is the live episode for its `(kind, subject)`.
pub const AUTOFIX_LIVE: [&str; 4] = ["open", "verifying", "apply_failed", "dismissed"];

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AutofixRow {
    pub id: i64,
    pub kind: String,
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_alias: Option<String>,
    pub severity: String,
    /// JSON: `service::autofix::FindingBody`.
    pub body: String,
    pub state: String,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dismissed_until: Option<i64>,
    pub attempts: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_attempt_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

pub struct NewAutofixFinding<'a> {
    pub kind: &'a str,
    pub subject: &'a str,
    pub host_alias: Option<&'a str>,
    pub severity: &'a str,
    pub body: &'a str,
}

const COLS: &str = "id, kind, subject, host_alias, severity, body, state, first_seen_at, \
                    last_seen_at, dismissed_until, attempts, last_attempt_at, last_error";
const LIVE_SQL: &str = "state IN ('open', 'verifying', 'apply_failed', 'dismissed')";

fn row(r: &rusqlite::Row<'_>) -> Result<AutofixRow> {
    Ok(AutofixRow {
        id: r.get(0)?,
        kind: r.get(1)?,
        subject: r.get(2)?,
        host_alias: r.get(3)?,
        severity: r.get(4)?,
        body: r.get(5)?,
        state: r.get(6)?,
        first_seen_at: r.get(7)?,
        last_seen_at: r.get(8)?,
        dismissed_until: r.get(9)?,
        attempts: r.get(10)?,
        last_attempt_at: r.get(11)?,
        last_error: r.get(12)?,
    })
}

impl Store {
    /// Refresh the live episode for `(kind, subject)`, or open one. Returns
    /// the row id and whether it was newly opened.
    pub fn upsert_autofix_finding(
        &self,
        f: &NewAutofixFinding<'_>,
        now: i64,
    ) -> Result<(i64, bool)> {
        let tx = self.conn.unchecked_transaction()?;
        let live: Option<i64> = tx
            .query_row(
                &format!(
                    "SELECT id FROM autofix_findings WHERE kind = ?1 AND subject = ?2 AND {LIVE_SQL}"
                ),
                rusqlite::params![f.kind, f.subject],
                |r| r.get(0),
            )
            .optional()?;
        let out = match live {
            Some(id) => {
                tx.execute(
                    "UPDATE autofix_findings
                     SET last_seen_at = ?2, severity = ?3, body = ?4, host_alias = ?5
                     WHERE id = ?1",
                    rusqlite::params![id, now, f.severity, f.body, f.host_alias],
                )?;
                (id, false)
            }
            None => {
                tx.execute(
                    "INSERT INTO autofix_findings
                       (kind, subject, host_alias, severity, body, state, first_seen_at, last_seen_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'open', ?6, ?6)",
                    rusqlite::params![f.kind, f.subject, f.host_alias, f.severity, f.body, now],
                )?;
                (tx.last_insert_rowid(), true)
            }
        };
        tx.commit()?;
        Ok(out)
    }

    pub fn live_autofix_findings(&self) -> Result<Vec<AutofixRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {COLS} FROM autofix_findings WHERE {LIVE_SQL} ORDER BY first_seen_at, id"
        ))?;
        let rows = st.query_map([], row)?;
        rows.collect()
    }

    pub fn autofix_finding(&self, id: i64) -> Result<Option<AutofixRow>> {
        self.conn
            .query_row(
                &format!("SELECT {COLS} FROM autofix_findings WHERE id = ?1"),
                [id],
                row,
            )
            .optional()
    }

    /// Move a live row to `state`. Returns false, and writes nothing, when the
    /// row is no longer live (already cleared): a cleared episode is history.
    pub fn set_autofix_state(
        &self,
        id: i64,
        state: &str,
        error: Option<&str>,
        now: i64,
    ) -> Result<bool> {
        let n = self.conn.execute(
            &format!(
                "UPDATE autofix_findings
                 SET state = ?2, last_error = ?3, last_seen_at = MAX(last_seen_at, ?4)
                 WHERE id = ?1 AND {LIVE_SQL}"
            ),
            rusqlite::params![id, state, error, now],
        )?;
        Ok(n == 1)
    }

    /// Count an attempt on a live row and move it to `verifying`. Returns
    /// false, and writes nothing, when the row is no longer live.
    pub fn mark_autofix_attempt(&self, id: i64, now: i64) -> Result<bool> {
        let n = self.conn.execute(
            &format!(
                "UPDATE autofix_findings
                 SET attempts = attempts + 1, last_attempt_at = ?2, state = 'verifying', last_error = NULL
                 WHERE id = ?1 AND {LIVE_SQL}"
            ),
            rusqlite::params![id, now],
        )?;
        Ok(n == 1)
    }

    pub fn dismiss_autofix_finding(&self, id: i64, until: i64) -> Result<bool> {
        let n = self.conn.execute(
            &format!(
                "UPDATE autofix_findings SET state = 'dismissed', dismissed_until = ?2
                 WHERE id = ?1 AND {LIVE_SQL}"
            ),
            rusqlite::params![id, until],
        )?;
        Ok(n == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn new<'a>(kind: &'a str, subject: &'a str) -> NewAutofixFinding<'a> {
        NewAutofixFinding {
            kind,
            subject,
            host_alias: Some("h1"),
            severity: "blocking",
            body: "{}",
        }
    }

    #[test]
    fn the_same_kind_and_subject_is_one_live_episode() {
        let s = Store::open_in_memory().unwrap();
        let (a, opened) = s
            .upsert_autofix_finding(&new("operator_unbound", "token:ux-agent"), 10)
            .unwrap();
        assert!(opened);
        let (b, opened) = s
            .upsert_autofix_finding(&new("operator_unbound", "token:ux-agent"), 20)
            .unwrap();
        assert_eq!(a, b);
        assert!(!opened);
        let row = s.autofix_finding(a).unwrap().unwrap();
        assert_eq!(
            (row.first_seen_at, row.last_seen_at, row.state.as_str()),
            (10, 20, "open")
        );
    }

    #[test]
    fn a_cleared_episode_lets_a_recurrence_open_a_new_row() {
        let s = Store::open_in_memory().unwrap();
        let (a, _) = s.upsert_autofix_finding(&new("k", "hub"), 10).unwrap();
        assert!(s.set_autofix_state(a, "cleared", None, 11).unwrap());
        let (b, opened) = s.upsert_autofix_finding(&new("k", "hub"), 12).unwrap();
        assert!(opened);
        assert_ne!(a, b);
        assert_eq!(s.live_autofix_findings().unwrap().len(), 1);
    }

    #[test]
    fn an_attempt_counts_and_moves_to_verifying() {
        let s = Store::open_in_memory().unwrap();
        let (a, _) = s.upsert_autofix_finding(&new("k", "hub"), 10).unwrap();
        assert!(s.mark_autofix_attempt(a, 15).unwrap());
        let row = s.autofix_finding(a).unwrap().unwrap();
        assert_eq!(
            (row.attempts, row.last_attempt_at, row.state.as_str()),
            (1, Some(15), "verifying")
        );
    }

    #[test]
    fn dismiss_records_until() {
        let s = Store::open_in_memory().unwrap();
        let (a, _) = s.upsert_autofix_finding(&new("k", "hub"), 10).unwrap();
        assert!(s.dismiss_autofix_finding(a, 999).unwrap());
        let row = s.autofix_finding(a).unwrap().unwrap();
        assert_eq!(
            (row.state.as_str(), row.dismissed_until),
            ("dismissed", Some(999))
        );
    }

    #[test]
    fn state_writes_leave_a_cleared_row_alone() {
        let s = Store::open_in_memory().unwrap();
        let (a, _) = s.upsert_autofix_finding(&new("k", "hub"), 10).unwrap();
        assert!(s.set_autofix_state(a, "cleared", None, 11).unwrap());
        assert!(!s
            .set_autofix_state(a, "apply_failed", Some("x"), 12)
            .unwrap());
        assert!(!s.mark_autofix_attempt(a, 13).unwrap());
        let row = s.autofix_finding(a).unwrap().unwrap();
        assert_eq!(
            (row.state.as_str(), row.attempts, row.last_error),
            ("cleared", 0, None)
        );
    }
}
