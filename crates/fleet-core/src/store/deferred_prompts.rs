//! Deferred prompts (`deferred_prompts`, migration 133, redesign step 5.10):
//! prompts for a session that was busy when they were sent, typed in once it
//! is idle. The decisions (send now or keep, when to type, retries) are
//! `service::sessions::deferred`'s; this is the table. Migration 155 (M15
//! step G1.8) adds Send later's time choices: a time before which a prompt
//! is not typed, a wait for its account's usage limit to reset, and dropping
//! it when its session is archived first.

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
    /// Not typed before this unix second (migration 155). Absent from an
    /// older hub, and when the prompt waits only for an idle moment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before: Option<i64>,
    /// Not typed while the session's account is at or past
    /// `accounts.pause_at`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub until_limit_reset: bool,
    /// Dropped, not typed, once the session is archived.
    #[serde(default, skip_serializing_if = "is_false")]
    pub skip_if_archived: bool,
    /// When it was dropped because the session was archived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped_at: Option<i64>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// When a deferred prompt may be typed, besides "the session is idle".
/// `Default` is the plain step 5.10 prompt: the next idle moment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeferredTiming {
    pub not_before: Option<i64>,
    pub until_limit_reset: bool,
    pub skip_if_archived: bool,
}

const COLS: &str = "id, session_id, body, created_at, delivered_at, attempts, failed_at, error, \
                    cancelled_at, not_before, until_limit_reset, skip_if_archived, skipped_at";

/// The partial index `deferred_prompts_pending` (migration 155) says the same.
const PENDING: &str = "delivered_at IS NULL AND failed_at IS NULL AND cancelled_at IS NULL \
                       AND skipped_at IS NULL";

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
        not_before: r.get(9)?,
        until_limit_reset: r.get(10)?,
        skip_if_archived: r.get(11)?,
        skipped_at: r.get(12)?,
    })
}

impl Store {
    /// Keep a prompt for `session_id` until it is idle; answers the row id.
    pub fn insert_deferred_prompt(&self, session_id: i64, body: &str, now: i64) -> Result<i64> {
        self.insert_deferred_prompt_timed(session_id, body, DeferredTiming::default(), now)
    }

    /// [`Self::insert_deferred_prompt`] with Send later's time choices.
    pub fn insert_deferred_prompt_timed(
        &self,
        session_id: i64,
        body: &str,
        timing: DeferredTiming,
        now: i64,
    ) -> Result<i64> {
        self.conn
            .prepare_cached(
                "INSERT INTO deferred_prompts (session_id, body, created_at, not_before, \
                 until_limit_reset, skip_if_archived) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?
            .execute(rusqlite::params![
                session_id,
                body,
                now,
                timing.not_before,
                timing.until_limit_reset,
                timing.skip_if_archived
            ])?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The session's waiting prompts whose time has come at `now` (no
    /// `not_before`, or one at or before `now`), oldest first. The limit
    /// wait is the service's to apply.
    pub fn due_deferred_prompts(
        &self,
        session_id: i64,
        now: i64,
    ) -> Result<Vec<DeferredPromptRow>> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {COLS} FROM deferred_prompts \
             WHERE session_id = ?1 AND {PENDING} \
               AND (not_before IS NULL OR not_before <= ?2) ORDER BY id"
        ))?;
        let rows = st.query_map(rusqlite::params![session_id, now], row)?;
        rows.collect()
    }

    /// Drop the session's waiting prompts that were to be skipped once it is
    /// archived; answers how many.
    pub fn skip_archived_deferred_prompts(&self, session_id: i64, now: i64) -> Result<usize> {
        self.conn
            .prepare_cached(&format!(
                "UPDATE deferred_prompts SET skipped_at = ?2, \
                 error = 'skipped: the session was archived first' \
                 WHERE session_id = ?1 AND skip_if_archived = 1 AND {PENDING}"
            ))?
            .execute(rusqlite::params![session_id, now])
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
        self.claim_deferred_prompt_in(id, now, None)
    }

    /// [`Self::claim_deferred_prompt`] for one idle moment, the one that
    /// began at `idle_since`: `false` as well when another of the session's
    /// prompts was claimed since then (review r06: the Stop hook's delivery
    /// and the reconcile backstop each saw the same idle session and typed
    /// one prompt each). `None`: no moment is known, no such check.
    pub fn claim_deferred_prompt_in(
        &self,
        id: i64,
        now: i64,
        idle_since: Option<i64>,
    ) -> Result<bool> {
        let n = self
            .conn
            .prepare_cached(&format!(
                "UPDATE deferred_prompts SET delivered_at = ?2 WHERE id = ?1 AND {PENDING} \
                 AND NOT EXISTS (SELECT 1 FROM deferred_prompts d \
                   WHERE d.session_id = deferred_prompts.session_id \
                     AND d.delivered_at >= ?3)"
            ))?
            .execute(rusqlite::params![id, now, idle_since])?;
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
    /// session; delivered, cancelled and skipped rows are history and left
    /// out.
    pub fn list_deferred_prompts(&self, session_id: Option<i64>) -> Result<Vec<DeferredPromptRow>> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {COLS} FROM deferred_prompts \
             WHERE (?1 IS NULL OR session_id = ?1) \
               AND delivered_at IS NULL AND cancelled_at IS NULL AND skipped_at IS NULL \
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
    fn a_timed_prompt_is_due_only_from_its_time() {
        let (s, sid) = store_with_session();
        let timed = DeferredTiming {
            not_before: Some(100),
            ..Default::default()
        };
        let later = s
            .insert_deferred_prompt_timed(sid, "later", timed, 10)
            .unwrap();
        let now = s.insert_deferred_prompt(sid, "now", 11).unwrap();
        let due = |at| -> Vec<i64> {
            s.due_deferred_prompts(sid, at)
                .unwrap()
                .iter()
                .map(|r| r.id)
                .collect()
        };
        assert_eq!(due(99), vec![now], "the timed one is not due yet");
        assert_eq!(due(100), vec![later, now], "then oldest first");
        let r = s.get_deferred_prompt(later).unwrap().unwrap();
        assert_eq!(
            (r.not_before, r.until_limit_reset, r.skip_if_archived),
            (Some(100), false, false)
        );
    }

    #[test]
    fn only_prompts_marked_skip_are_dropped_when_archived() {
        let (s, sid) = store_with_session();
        let skip = DeferredTiming {
            skip_if_archived: true,
            ..Default::default()
        };
        let a = s.insert_deferred_prompt_timed(sid, "a", skip, 10).unwrap();
        let b = s.insert_deferred_prompt(sid, "b", 11).unwrap();
        assert_eq!(s.skip_archived_deferred_prompts(sid, 20).unwrap(), 1);
        assert_eq!(s.skip_archived_deferred_prompts(sid, 21).unwrap(), 0);
        assert!(
            !s.claim_deferred_prompt(a, 22).unwrap(),
            "a skipped row is not pending"
        );
        assert_eq!(
            s.get_deferred_prompt(a).unwrap().unwrap().skipped_at,
            Some(20)
        );
        let listed: Vec<i64> = s
            .list_deferred_prompts(Some(sid))
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(listed, vec![b]);
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
