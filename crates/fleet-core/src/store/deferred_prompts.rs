//! Deferred prompts (`deferred_prompts`, migration 131, redesign step 5.10):
//! prompts for a session that was busy when they were sent, typed in once it
//! is idle. The decisions (send now or keep, when to type, retries) are
//! `service::sessions::deferred`'s; this is the table.

use super::Store;
use rusqlite::{OptionalExtension, Result};

/// A typing attempt that fails is retried until this many have failed; the
/// row is then marked failed and stays visible with its error.
pub const DEFERRED_MAX_ATTEMPTS: i64 = 3;

/// One deferred prompt, as `queued_prompts` lists it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeferredPromptRow {
    pub id: i64,
    pub session_id: i64,
    pub body: String,
    pub created_at: i64,
    #[serde(default)]
    pub delivered_at: Option<i64>,
    #[serde(default)]
    pub attempts: i64,
    #[serde(default)]
    pub failed_at: Option<i64>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub cancelled_at: Option<i64>,
}

const COLS: &str =
    "id, session_id, body, created_at, delivered_at, attempts, failed_at, error, cancelled_at";

const PENDING: &str = "delivered_at IS NULL AND failed_at IS NULL AND cancelled_at IS NULL";

fn row(r: &rusqlite::Row<'_>) -> Result<DeferredPromptRow> {
    Ok(DeferredPromptRow {
        id: r.get(0)?,
        session_id: r.get(1)?,
        body: r.get(2)?,
        created_at: r.get(3)?,
        delivered_at: r.get(4)?,
        attempts: r.get(5)?,
        failed_at: r.get(6)?,
        error: r.get(7)?,
        cancelled_at: r.get(8)?,
    })
}

impl Store {
    /// Keep a prompt for `session_id` until it is idle; answers the row id.
    pub fn insert_deferred_prompt(&self, session_id: i64, body: &str, now: i64) -> Result<i64> {
        self.conn
            .prepare_cached(
                "INSERT INTO deferred_prompts (session_id, body, created_at) VALUES (?1, ?2, ?3)",
            )?
            .execute(rusqlite::params![session_id, body, now])?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The oldest prompt still waiting for `session_id`.
    pub fn next_deferred_prompt(&self, session_id: i64) -> Result<Option<DeferredPromptRow>> {
        self.conn
            .prepare_cached(&format!(
                "SELECT {COLS} FROM deferred_prompts \
                 WHERE session_id = ?1 AND {PENDING} ORDER BY id LIMIT 1"
            ))?
            .query_row([session_id], row)
            .optional()
    }

    /// Sessions with at least one prompt waiting, oldest first.
    pub fn sessions_with_deferred_prompts(&self) -> Result<Vec<i64>> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT session_id FROM deferred_prompts WHERE {PENDING} \
             GROUP BY session_id ORDER BY MIN(id)"
        ))?;
        let ids = st.query_map([], |r| r.get(0))?;
        ids.collect()
    }

    /// Take a waiting prompt for typing. `false` when it is no longer
    /// waiting (delivered by a racing call, cancelled, failed): only the
    /// caller that gets `true` types it.
    pub fn claim_deferred_prompt(&self, id: i64, now: i64) -> Result<bool> {
        let n = self
            .conn
            .prepare_cached(&format!(
                "UPDATE deferred_prompts SET delivered_at = ?2 WHERE id = ?1 AND {PENDING}"
            ))?
            .execute(rusqlite::params![id, now])?;
        Ok(n == 1)
    }

    /// A claimed prompt was not typed: count the attempt, and either hand it
    /// back to the queue or, past [`DEFERRED_MAX_ATTEMPTS`], mark it failed.
    /// Answers whether it failed for good.
    pub fn release_deferred_prompt(&self, id: i64, error: &str, now: i64) -> Result<bool> {
        self.conn
            .prepare_cached(
                "UPDATE deferred_prompts SET attempts = attempts + 1, error = ?2, \
                 delivered_at = NULL, \
                 failed_at = CASE WHEN attempts + 1 >= ?3 THEN ?4 ELSE NULL END \
                 WHERE id = ?1",
            )?
            .execute(rusqlite::params![id, error, DEFERRED_MAX_ATTEMPTS, now])?;
        let failed: Option<i64> = self
            .conn
            .prepare_cached("SELECT failed_at FROM deferred_prompts WHERE id = ?1")?
            .query_row([id], |r| r.get(0))
            .optional()?
            .flatten();
        Ok(failed.is_some())
    }

    /// Take back a prompt that has not gone out. `false` when there is none
    /// waiting under that id.
    pub fn cancel_deferred_prompt(&self, id: i64, now: i64) -> Result<bool> {
        let n = self
            .conn
            .prepare_cached(&format!(
                "UPDATE deferred_prompts SET cancelled_at = ?2 WHERE id = ?1 AND {PENDING}"
            ))?
            .execute(rusqlite::params![id, now])?;
        Ok(n == 1)
    }

    pub fn get_deferred_prompt(&self, id: i64) -> Result<Option<DeferredPromptRow>> {
        self.conn
            .prepare_cached(&format!(
                "SELECT {COLS} FROM deferred_prompts WHERE id = ?1"
            ))?
            .query_row([id], row)
            .optional()
    }

    /// What is waiting, and what failed, for one session or for every
    /// session; delivered and cancelled rows are history and left out.
    pub fn list_deferred_prompts(&self, session_id: Option<i64>) -> Result<Vec<DeferredPromptRow>> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {COLS} FROM deferred_prompts \
             WHERE (?1 IS NULL OR session_id = ?1) \
               AND delivered_at IS NULL AND cancelled_at IS NULL \
             ORDER BY id"
        ))?;
        let rows = st.query_map([session_id], row)?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_session() -> (Store, i64) {
        let s = Store::open_in_memory().unwrap();
        s.upsert_host("local").unwrap();
        let id = s
            .upsert_session("s", "local", None, None, 0, 0, "running", None)
            .unwrap();
        (s, id)
    }

    #[test]
    fn a_prompt_is_typed_once_then_leaves_the_queue() {
        let (s, sid) = store_with_session();
        let a = s.insert_deferred_prompt(sid, "first", 10).unwrap();
        let b = s.insert_deferred_prompt(sid, "second", 11).unwrap();
        assert_eq!(s.sessions_with_deferred_prompts().unwrap(), vec![sid]);
        assert_eq!(s.next_deferred_prompt(sid).unwrap().unwrap().id, a);
        assert!(s.claim_deferred_prompt(a, 20).unwrap());
        assert!(
            !s.claim_deferred_prompt(a, 21).unwrap(),
            "a second claim loses"
        );
        assert_eq!(s.next_deferred_prompt(sid).unwrap().unwrap().id, b);
        let listed: Vec<i64> = s
            .list_deferred_prompts(Some(sid))
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(listed, vec![b]);
    }

    #[test]
    fn a_failed_attempt_returns_it_until_the_attempts_run_out() {
        let (s, sid) = store_with_session();
        let id = s.insert_deferred_prompt(sid, "p", 10).unwrap();
        for n in 1..DEFERRED_MAX_ATTEMPTS {
            assert!(s.claim_deferred_prompt(id, 20).unwrap());
            assert!(
                !s.release_deferred_prompt(id, "ssh down", 21).unwrap(),
                "attempt {n}"
            );
        }
        assert!(s.claim_deferred_prompt(id, 30).unwrap());
        assert!(s.release_deferred_prompt(id, "ssh down", 31).unwrap());
        assert!(s.next_deferred_prompt(sid).unwrap().is_none());
        let r = s.get_deferred_prompt(id).unwrap().unwrap();
        assert_eq!(
            (r.attempts, r.failed_at, r.error.as_deref()),
            (DEFERRED_MAX_ATTEMPTS, Some(31), Some("ssh down"))
        );
        assert_eq!(
            s.list_deferred_prompts(None).unwrap().len(),
            1,
            "a failed row stays listed"
        );
    }

    #[test]
    fn a_cancelled_prompt_is_never_claimed() {
        let (s, sid) = store_with_session();
        let id = s.insert_deferred_prompt(sid, "p", 10).unwrap();
        assert!(s.cancel_deferred_prompt(id, 11).unwrap());
        assert!(!s.cancel_deferred_prompt(id, 12).unwrap());
        assert!(!s.claim_deferred_prompt(id, 13).unwrap());
        assert!(s.sessions_with_deferred_prompts().unwrap().is_empty());
        assert!(s.list_deferred_prompts(Some(sid)).unwrap().is_empty());
    }

    #[test]
    fn rows_go_with_their_session() {
        let (s, sid) = store_with_session();
        s.insert_deferred_prompt(sid, "p", 10).unwrap();
        s.conn
            .execute("DELETE FROM sessions WHERE id = ?1", [sid])
            .unwrap();
        assert!(s.list_deferred_prompts(None).unwrap().is_empty());
    }
}
