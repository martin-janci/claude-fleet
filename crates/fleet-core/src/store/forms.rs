//! Chat forms (`form_requests`, migration 115): a form an agent asked a
//! person to fill. Every change wakes `form_notify` (the `ask` tool's wait)
//! and bumps the asking session's `row_version` with a `session:updated`,
//! because the row carries `pending_form`.

use super::{now_unix, Store};
use rusqlite::{OptionalExtension, Result};

pub const FORM_STATES: [&str; 5] = ["pending", "answered", "declined", "cancelled", "expired"];

#[derive(Debug, Clone, PartialEq)]
pub struct FormRow {
    pub id: i64,
    pub form_id: String,
    pub session_id: i64,
    pub host_alias: String,
    pub spec: String,
    pub why: Option<String>,
    pub state: String,
    pub answers: Option<String>,
    pub note: Option<String>,
    pub answered_by: Option<String>,
    pub secrets_on_host: bool,
    pub created_at: i64,
    pub decided_at: Option<i64>,
}

pub struct NewForm<'a> {
    pub form_id: &'a str,
    pub session_id: i64,
    pub host_alias: &'a str,
    pub spec: &'a str,
    pub why: Option<&'a str>,
}

pub struct FormFinish<'a> {
    pub state: &'a str,
    pub answers: Option<&'a str>,
    pub note: Option<&'a str>,
    pub answered_by: Option<&'a str>,
    pub secrets_on_host: bool,
}

const COLS: &str = "id, form_id, session_id, host_alias, spec, why, state, answers, note, \
                    answered_by, secrets_on_host, created_at, decided_at";

fn row(r: &rusqlite::Row<'_>) -> Result<FormRow> {
    Ok(FormRow {
        id: r.get(0)?,
        form_id: r.get(1)?,
        session_id: r.get(2)?,
        host_alias: r.get(3)?,
        spec: r.get(4)?,
        why: r.get(5)?,
        state: r.get(6)?,
        answers: r.get(7)?,
        note: r.get(8)?,
        answered_by: r.get(9)?,
        secrets_on_host: r.get::<_, i64>(10)? != 0,
        created_at: r.get(11)?,
        decided_at: r.get(12)?,
    })
}

impl Store {
    pub fn form_notify(&self) -> std::sync::Arc<tokio::sync::Notify> {
        self.form_notify.clone()
    }

    /// The asking session's row changed (its `pending_form`): bump it so
    /// the optimistic merge takes the new row, announce it, wake waiters.
    fn form_touched(&self, session_ids: &[i64]) -> Result<()> {
        for id in session_ids {
            self.conn.execute(
                "UPDATE sessions SET row_version = row_version + 1 WHERE id = ?1",
                [id],
            )?;
            self.emit_session(*id)?;
        }
        self.form_notify.notify_waiters();
        Ok(())
    }

    pub fn insert_form(&self, f: &NewForm<'_>) -> Result<FormRow> {
        self.conn.execute(
            "INSERT INTO form_requests (form_id, session_id, host_alias, spec, why, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                f.form_id,
                f.session_id,
                f.host_alias,
                f.spec,
                f.why,
                now_unix()
            ],
        )?;
        self.form_touched(&[f.session_id])?;
        self.form(f.form_id)
            .map(|r| r.expect("the row just inserted"))
    }

    pub fn form(&self, form_id: &str) -> Result<Option<FormRow>> {
        self.conn
            .query_row(
                &format!("SELECT {COLS} FROM form_requests WHERE form_id = ?1"),
                [form_id],
                row,
            )
            .optional()
    }

    pub fn pending_form_of_session(&self, session_id: i64) -> Result<Option<FormRow>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {COLS} FROM form_requests WHERE session_id = ?1 AND state = 'pending'"
                ),
                [session_id],
                row,
            )
            .optional()
    }

    /// Newest first, at most 200.
    pub fn forms(&self, session_id: Option<i64>, state: Option<&str>) -> Result<Vec<FormRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {COLS} FROM form_requests
              WHERE (?1 IS NULL OR session_id = ?1) AND (?2 IS NULL OR state = ?2)
              ORDER BY id DESC LIMIT 200"
        ))?;
        let rows = st.query_map(rusqlite::params![session_id, state], row)?;
        rows.collect()
    }

    /// Finish a PENDING form. `false` when it was no longer pending (the
    /// caller lost a race: answered elsewhere, withdrawn, expired).
    pub fn finish_form(&self, form_id: &str, f: &FormFinish<'_>) -> Result<bool> {
        let Some(current) = self.form(form_id)? else {
            return Ok(false);
        };
        let n = self.conn.execute(
            "UPDATE form_requests
                SET state = ?2, answers = ?3, note = ?4, answered_by = ?5,
                    secrets_on_host = ?6, decided_at = ?7
              WHERE form_id = ?1 AND state = 'pending'",
            rusqlite::params![
                form_id,
                f.state,
                f.answers,
                f.note,
                f.answered_by,
                i64::from(f.secrets_on_host),
                now_unix()
            ],
        )?;
        if n == 1 {
            self.form_touched(&[current.session_id])?;
        }
        Ok(n == 1)
    }

    pub fn cancel_forms_of_session(&self, session_id: i64) -> Result<usize> {
        let n = self.conn.execute(
            "UPDATE form_requests SET state = 'cancelled', decided_at = ?2
              WHERE session_id = ?1 AND state = 'pending'",
            rusqlite::params![session_id, now_unix()],
        )?;
        if n > 0 {
            self.form_touched(&[session_id])?;
        }
        Ok(n)
    }

    /// Pending forms created before `older_than` become `expired`.
    pub fn expire_forms(&self, older_than: i64) -> Result<usize> {
        let mut st = self.conn.prepare(
            "UPDATE form_requests SET state = 'expired', decided_at = ?2
              WHERE state = 'pending' AND created_at < ?1 RETURNING session_id",
        )?;
        let ids: Vec<i64> = st
            .query_map(rusqlite::params![older_than, now_unix()], |r| r.get(0))?
            .collect::<Result<_>>()?;
        drop(st);
        if !ids.is_empty() {
            self.form_touched(&ids)?;
        }
        Ok(ids.len())
    }

    /// Delete decided rows older than `decided_before`, except one whose
    /// secrets may still be on its host (the sweep clears that first).
    pub fn purge_forms(&self, decided_before: i64) -> Result<usize> {
        self.conn.execute(
            "DELETE FROM form_requests
              WHERE state <> 'pending' AND decided_at < ?1 AND secrets_on_host = 0",
            [decided_before],
        )
    }

    /// `(form_id, host_alias)` of every form whose secret directory should
    /// go: its session is a ghost, or it was decided before `decided_before`.
    pub fn forms_to_sweep(&self, decided_before: i64) -> Result<Vec<(String, String)>> {
        let mut st = self.conn.prepare(
            "SELECT f.form_id, f.host_alias FROM form_requests f
               LEFT JOIN sessions s ON s.id = f.session_id
              WHERE f.secrets_on_host = 1
                AND (s.id IS NULL OR s.status = 'ghost'
                     OR (f.decided_at IS NOT NULL AND f.decided_at < ?1))
              ORDER BY f.id",
        )?;
        let rows = st.query_map([decided_before], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    pub fn mark_form_swept(&self, form_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE form_requests SET secrets_on_host = 0 WHERE form_id = ?1",
            [form_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_support::store_with_recorder;
    use crate::store::PendingForm;

    fn seed(s: &Store) -> i64 {
        s.upsert_host("h").unwrap();
        s.upsert_session("dev", "h", None, None, 1, 1, "running", None)
            .unwrap()
    }

    fn new<'a>(form_id: &'a str, session_id: i64) -> NewForm<'a> {
        NewForm {
            form_id,
            session_id,
            host_alias: "h",
            spec: r#"{"spec":"fleet.form/1","title":"Pick one","steps":[]}"#,
            why: Some("to know"),
        }
    }

    #[test]
    fn a_pending_form_shows_on_its_session_row_and_bumps_it() {
        let (s, bus) = store_with_recorder();
        let sid = seed(&s);
        let before = s.get_session_by_id(sid).unwrap().unwrap();
        assert_eq!(before.pending_form, None);
        bus.take();
        s.insert_form(&new("f_one", sid)).unwrap();
        let after = s.get_session_by_id(sid).unwrap().unwrap();
        assert_eq!(
            after.pending_form,
            Some(PendingForm {
                form_id: "f_one".into(),
                title: "Pick one".into()
            })
        );
        assert!(
            after.row_version > before.row_version,
            "the merge guard sees it"
        );
        assert_eq!(bus.names(), vec!["session:updated"]);
    }

    #[test]
    fn a_second_pending_form_for_one_session_is_refused() {
        let (s, _) = store_with_recorder();
        let sid = seed(&s);
        s.insert_form(&new("f_one", sid)).unwrap();
        let err = s.insert_form(&new("f_two", sid)).unwrap_err();
        assert!(err.to_string().contains("UNIQUE"), "{err}");
    }

    #[test]
    fn finishing_clears_the_row_once_and_wakes_waiters() {
        let (s, bus) = store_with_recorder();
        let sid = seed(&s);
        s.insert_form(&new("f_one", sid)).unwrap();
        let notify = s.form_notify();
        let woke = notify.notified();
        tokio::pin!(woke);
        // `enable` is true only when the notification already fired.
        assert!(!woke.as_mut().enable(), "registered, not yet woken");
        bus.take();
        let done = FormFinish {
            state: "answered",
            answers: Some(r#"{"answers":{},"secrets":{}}"#),
            note: None,
            answered_by: Some("ada (desktop)"),
            secrets_on_host: false,
        };
        assert!(s.finish_form("f_one", &done).unwrap());
        assert!(
            !s.finish_form("f_one", &done).unwrap(),
            "only a pending form finishes"
        );
        assert_eq!(
            s.get_session_by_id(sid).unwrap().unwrap().pending_form,
            None
        );
        assert_eq!(bus.names(), vec!["session:updated"]);
        let row = s.form("f_one").unwrap().unwrap();
        assert_eq!(row.state, "answered");
        assert!(row.decided_at.is_some());
        assert!(
            futures_util::FutureExt::now_or_never(woke).is_some(),
            "the waiter was woken"
        );
    }

    #[test]
    fn expiry_purge_and_sweep_pick_the_right_rows() {
        let (s, _) = store_with_recorder();
        let sid = seed(&s);
        s.insert_form(&new("f_old", sid)).unwrap();
        s.conn_ref()
            .execute(
                "UPDATE form_requests SET created_at = 10 WHERE form_id = 'f_old'",
                [],
            )
            .unwrap();
        assert_eq!(s.expire_forms(100).unwrap(), 1);
        assert_eq!(s.form("f_old").unwrap().unwrap().state, "expired");
        s.conn_ref()
            .execute(
                "UPDATE form_requests SET decided_at = 50, secrets_on_host = 1 WHERE form_id = 'f_old'",
                [],
            )
            .unwrap();
        assert_eq!(
            s.forms_to_sweep(100).unwrap(),
            vec![("f_old".to_string(), "h".to_string())]
        );
        assert_eq!(
            s.purge_forms(100).unwrap(),
            0,
            "a row with secrets on its host stays"
        );
        s.mark_form_swept("f_old").unwrap();
        assert!(s.forms_to_sweep(100).unwrap().is_empty());
        assert_eq!(s.purge_forms(100).unwrap(), 1);
    }

    #[test]
    fn a_ghost_sessions_form_is_swept_at_once() {
        let (s, _) = store_with_recorder();
        let sid = seed(&s);
        s.insert_form(&new("f_one", sid)).unwrap();
        s.conn_ref()
            .execute(
                "UPDATE form_requests SET secrets_on_host = 1 WHERE form_id = 'f_one'",
                [],
            )
            .unwrap();
        assert!(
            s.forms_to_sweep(0).unwrap().is_empty(),
            "a live session keeps its secrets"
        );
        s.mark_session_killed(sid, 5).unwrap();
        assert_eq!(s.forms_to_sweep(0).unwrap().len(), 1);
    }

    #[test]
    fn a_killed_session_cancels_its_pending_form() {
        let (s, _) = store_with_recorder();
        let sid = seed(&s);
        s.insert_form(&new("f_one", sid)).unwrap();
        assert_eq!(s.cancel_forms_of_session(sid).unwrap(), 1);
        assert_eq!(s.form("f_one").unwrap().unwrap().state, "cancelled");
        assert_eq!(s.cancel_forms_of_session(sid).unwrap(), 0);
    }
}
