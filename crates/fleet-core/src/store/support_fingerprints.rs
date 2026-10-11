//! The support agent's fingerprint records (migration 175): which bug each
//! fingerprint a rule's investigations found was filed as or linked to.
//! The support agent is `service::agents::support`.

use super::Store;
use rusqlite::{params, OptionalExtension};

/// One fingerprint of one rule's scope.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SupportFingerprintRow {
    pub scope: String,
    pub fingerprint: String,
    pub exception: String,
    pub bug_key: Option<String>,
    pub first_job_id: Option<i64>,
    pub last_job_id: Option<i64>,
    pub seen: i64,
    pub first_seen: i64,
    pub last_seen: i64,
}

const COLS: &str = "scope, fingerprint, exception, bug_key, first_job_id, last_job_id, seen, \
                    first_seen, last_seen";

impl Store {
    pub fn support_fingerprint(
        &self,
        scope: &str,
        fingerprint: &str,
    ) -> Result<Option<SupportFingerprintRow>, rusqlite::Error> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {COLS} FROM support_fingerprints WHERE scope = ?1 AND fingerprint = ?2"
                ),
                params![scope, fingerprint],
                |r| {
                    Ok(SupportFingerprintRow {
                        scope: r.get(0)?,
                        fingerprint: r.get(1)?,
                        exception: r.get(2)?,
                        bug_key: r.get(3)?,
                        first_job_id: r.get(4)?,
                        last_job_id: r.get(5)?,
                        seen: r.get(6)?,
                        first_seen: r.get(7)?,
                        last_seen: r.get(8)?,
                    })
                },
            )
            .optional()
    }

    /// A job found `fingerprint`: the first sighting adds the row, a later
    /// one counts it. Returns the row as it was before (`None` the first time).
    pub fn see_support_fingerprint(
        &self,
        scope: &str,
        fingerprint: &str,
        exception: &str,
        job_id: i64,
        now: i64,
    ) -> Result<Option<SupportFingerprintRow>, rusqlite::Error> {
        let before = self.support_fingerprint(scope, fingerprint)?;
        self.conn.execute(
            "INSERT INTO support_fingerprints \
               (scope, fingerprint, exception, first_job_id, last_job_id, seen, first_seen, last_seen) \
             VALUES (?1, ?2, ?3, ?4, ?4, 1, ?5, ?5) \
             ON CONFLICT (scope, fingerprint) DO UPDATE SET \
               seen = seen + 1, last_job_id = excluded.last_job_id, last_seen = excluded.last_seen",
            params![scope, fingerprint, exception, job_id, now],
        )?;
        Ok(before)
    }

    /// The bug a fingerprint was filed as or linked to.
    pub fn set_support_fingerprint_bug(
        &self,
        scope: &str,
        fingerprint: &str,
        bug_key: &str,
        now: i64,
    ) -> Result<bool, rusqlite::Error> {
        Ok(self.conn.execute(
            "UPDATE support_fingerprints SET bug_key = ?3, last_seen = MAX(last_seen, ?4) \
             WHERE scope = ?1 AND fingerprint = ?2",
            params![scope, fingerprint, bug_key, now],
        )? == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fingerprint_is_counted_and_learns_its_bug() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(
            s.see_support_fingerprint("jira:1:L2S", "ab", "NPE", 7, 100)
                .unwrap(),
            None
        );
        let before = s
            .see_support_fingerprint("jira:1:L2S", "ab", "NPE", 9, 200)
            .unwrap()
            .unwrap();
        assert_eq!((before.seen, before.last_job_id), (1, Some(7)));
        assert!(s
            .set_support_fingerprint_bug("jira:1:L2S", "ab", "PD-4", 250)
            .unwrap());
        assert!(!s
            .set_support_fingerprint_bug("jira:1:L2S", "zz", "PD-4", 250)
            .unwrap());
        let row = s.support_fingerprint("jira:1:L2S", "ab").unwrap().unwrap();
        assert_eq!(
            (
                row.seen,
                row.first_job_id,
                row.last_job_id,
                row.bug_key.as_deref(),
                row.first_seen,
                row.last_seen
            ),
            (2, Some(7), Some(9), Some("PD-4"), 100, 250)
        );
        assert_eq!(
            s.support_fingerprint("jira:2:L2S", "ab").unwrap(),
            None,
            "per scope"
        );
    }
}
