//! Settings proposals and the settings audit trail (declarative pages P5,
//! migration 083). The rules live in `service::settings::review`; this is
//! the rows.

use super::{now_unix, Store};
use rusqlite::{OptionalExtension, Result};

/// At most this many audit rows are kept; the oldest go first.
pub const SETTING_AUDIT_KEEP: i64 = 5_000;
/// Decided proposals are kept this long (seconds), for the record.
pub const DECIDED_PROPOSAL_KEEP_SECS: i64 = 30 * 24 * 60 * 60;

/// One proposed value for one registered setting.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SettingProposalRow {
    pub id: i64,
    pub at: i64,
    pub key: String,
    /// The value it would store (normalised).
    pub value: String,
    /// The effective value when it was proposed.
    pub before: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_detail: Option<String>,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
}

/// One write of a registered setting.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SettingAuditRow {
    pub id: i64,
    pub at: i64,
    pub key: String,
    /// `None` when the key was unset (its default applied).
    pub before: Option<String>,
    pub after: String,
    pub actor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal_id: Option<i64>,
}

/// A proposal to insert.
pub struct NewSettingProposal<'a> {
    pub key: &'a str,
    pub value: &'a str,
    pub before: &'a str,
    pub why: Option<&'a str>,
    pub source: &'a str,
    pub source_detail: Option<&'a str>,
}

const PROPOSAL_COLS: &str =
    "id, at, key, value, before, why, source, source_detail, state, decided_at, decided_by";

fn proposal_row(r: &rusqlite::Row<'_>) -> Result<SettingProposalRow> {
    Ok(SettingProposalRow {
        id: r.get(0)?,
        at: r.get(1)?,
        key: r.get(2)?,
        value: r.get(3)?,
        before: r.get(4)?,
        why: r.get(5)?,
        source: r.get(6)?,
        source_detail: r.get(7)?,
        state: r.get(8)?,
        decided_at: r.get(9)?,
        decided_by: r.get(10)?,
    })
}

impl Store {
    /// Insert a pending proposal, superseding any pending one for the same
    /// key, and drop decided proposals past [`DECIDED_PROPOSAL_KEEP_SECS`].
    pub fn insert_setting_proposal(
        &self,
        p: &NewSettingProposal<'_>,
    ) -> Result<SettingProposalRow> {
        let now = now_unix();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE setting_proposals SET state = 'superseded', decided_at = ?2
             WHERE state = 'pending' AND key = ?1",
            rusqlite::params![p.key, now],
        )?;
        tx.execute(
            "DELETE FROM setting_proposals WHERE state != 'pending' AND decided_at < ?1",
            rusqlite::params![now - DECIDED_PROPOSAL_KEEP_SECS],
        )?;
        tx.execute(
            "INSERT INTO setting_proposals (at, key, value, before, why, source, source_detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                now,
                p.key,
                p.value,
                p.before,
                p.why,
                p.source,
                p.source_detail
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        self.setting_proposal(id)
            .map(|r| r.expect("the row just inserted"))
    }

    pub fn setting_proposal(&self, id: i64) -> Result<Option<SettingProposalRow>> {
        self.conn
            .query_row(
                &format!("SELECT {PROPOSAL_COLS} FROM setting_proposals WHERE id = ?1"),
                [id],
                proposal_row,
            )
            .optional()
    }

    /// Pending proposals, oldest first.
    pub fn pending_setting_proposals(&self) -> Result<Vec<SettingProposalRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {PROPOSAL_COLS} FROM setting_proposals WHERE state = 'pending' ORDER BY id"
        ))?;
        let rows = st.query_map([], proposal_row)?;
        rows.collect()
    }

    pub fn count_pending_setting_proposals(&self) -> Result<i64> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM setting_proposals WHERE state = 'pending'",
            [],
            |r| r.get(0),
        )
    }

    /// Mark a pending proposal decided. `false` when it was not pending.
    pub fn decide_setting_proposal(&self, id: i64, state: &str, by: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE setting_proposals SET state = ?2, decided_at = ?3, decided_by = ?4
             WHERE id = ?1 AND state = 'pending'",
            rusqlite::params![id, state, now_unix(), by],
        )?;
        Ok(n == 1)
    }

    /// Record one write, keeping at most [`SETTING_AUDIT_KEEP`] rows.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_setting_audit(
        &self,
        key: &str,
        before: Option<&str>,
        after: &str,
        actor: &str,
        actor_detail: Option<&str>,
        proposal_id: Option<i64>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO setting_audit (at, key, before, after, actor, actor_detail, proposal_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                now_unix(),
                key,
                before,
                after,
                actor,
                actor_detail,
                proposal_id
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.conn.execute(
            "DELETE FROM setting_audit WHERE id <= ?1",
            [id - SETTING_AUDIT_KEEP],
        )?;
        Ok(())
    }

    /// The keys written after audit row `after`, oldest first and each once,
    /// with the newest row's id (`after` when there is none). `after: None`
    /// only reads the newest id: where a watcher starts from. The running
    /// hub follows writes another process made to its database this way
    /// (`fleet-hub settings apply`).
    pub fn setting_audit_since(&self, after: Option<i64>) -> Result<(i64, Vec<String>)> {
        let Some(after) = after else {
            let last: i64 =
                self.conn
                    .query_row("SELECT COALESCE(MAX(id), 0) FROM setting_audit", [], |r| {
                        r.get(0)
                    })?;
            return Ok((last, Vec::new()));
        };
        let mut st = self
            .conn
            .prepare("SELECT id, key FROM setting_audit WHERE id > ?1 ORDER BY id")?;
        let rows = st.query_map([after], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut last = after;
        let mut keys: Vec<String> = Vec::new();
        for row in rows {
            let (id, key) = row?;
            last = id;
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        Ok((last, keys))
    }

    /// A key's writes, newest first.
    pub fn setting_audit(&self, key: &str, limit: i64) -> Result<Vec<SettingAuditRow>> {
        let mut st = self.conn.prepare(
            "SELECT id, at, key, before, after, actor, actor_detail, proposal_id
             FROM setting_audit WHERE key = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = st.query_map(rusqlite::params![key, limit], |r| {
            Ok(SettingAuditRow {
                id: r.get(0)?,
                at: r.get(1)?,
                key: r.get(2)?,
                before: r.get(3)?,
                after: r.get(4)?,
                actor: r.get(5)?,
                actor_detail: r.get(6)?,
                proposal_id: r.get(7)?,
            })
        })?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new<'a>(key: &'a str, value: &'a str) -> NewSettingProposal<'a> {
        NewSettingProposal {
            key,
            value,
            before: "7",
            why: Some("fewer stale sessions"),
            source: "agent",
            source_detail: None,
        }
    }

    #[test]
    fn a_newer_proposal_for_the_same_key_supersedes_the_pending_one() {
        let s = Store::open_in_memory().unwrap();
        let a = s
            .insert_setting_proposal(&new("work.recent_days", "3"))
            .unwrap();
        let b = s
            .insert_setting_proposal(&new("work.recent_days", "4"))
            .unwrap();
        s.insert_setting_proposal(&new("gc.enabled", "true"))
            .unwrap();
        let pending = s.pending_setting_proposals().unwrap();
        assert_eq!(
            pending.iter().map(|p| p.value.as_str()).collect::<Vec<_>>(),
            ["4", "true"]
        );
        assert_eq!(
            s.setting_proposal(a.id).unwrap().unwrap().state,
            "superseded"
        );
        assert!(s
            .decide_setting_proposal(b.id, "applied", "person")
            .unwrap());
        assert!(!s
            .decide_setting_proposal(b.id, "rejected", "person")
            .unwrap());
        assert_eq!(s.count_pending_setting_proposals().unwrap(), 1);
    }

    #[test]
    fn the_audit_is_per_key_newest_first() {
        let s = Store::open_in_memory().unwrap();
        s.insert_setting_audit("gc.enabled", None, "true", "person", None, None)
            .unwrap();
        s.insert_setting_audit(
            "gc.enabled",
            Some("true"),
            "false",
            "agent",
            Some("control API"),
            Some(3),
        )
        .unwrap();
        s.insert_setting_audit("work.recent_days", None, "3", "person", None, None)
            .unwrap();
        let h = s.setting_audit("gc.enabled", 10).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].after, "false");
        assert_eq!(h[0].proposal_id, Some(3));
        assert_eq!(h[1].before, None);
    }

    #[test]
    fn audit_since_names_each_key_written_after_a_row_once() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.setting_audit_since(None).unwrap(), (0, vec![]));
        s.insert_setting_audit("gc.enabled", None, "true", "person", None, None)
            .unwrap();
        let (start, keys) = s.setting_audit_since(None).unwrap();
        assert!(start > 0);
        assert!(keys.is_empty(), "None only reads where to start");
        assert_eq!(s.setting_audit_since(Some(start)).unwrap(), (start, vec![]));
        s.insert_setting_audit("work.recent_days", None, "3", "person", None, None)
            .unwrap();
        s.insert_setting_audit("gc.enabled", Some("true"), "false", "person", None, None)
            .unwrap();
        s.insert_setting_audit("work.recent_days", Some("3"), "4", "person", None, None)
            .unwrap();
        let (last, keys) = s.setting_audit_since(Some(start)).unwrap();
        assert_eq!(last, start + 3);
        assert_eq!(keys, ["work.recent_days", "gc.enabled"]);
    }
}
