//! What an org admin may learn of a member's own sessions (redesign step
//! 11.7c): that they exist, never what they are.
//!
//! A session's metadata is content (spec §4.3), so nothing here returns a
//! row, a name or a host: only the ids, for the service to COUNT against
//! the asking admin's own grants. The same discipline as
//! `person_grants_in_org` and the unclaimed counts.

use super::Store;
use crate::ipc_error::IpcError;

impl Store {
    /// The live sessions of `org` with an owner, as `(owner, session id)`.
    /// Live is `status = 'running' AND lost_at IS NULL`, as everywhere a
    /// count of running sessions is taken.
    pub fn live_owned_sessions_in_org(&self, org: i64) -> Result<Vec<(i64, i64)>, IpcError> {
        let mut st = self.conn.prepare(concat!(
            "SELECT s.owner_person_id, s.id FROM sessions s \
              WHERE s.owner_person_id IS NOT NULL AND s.status = 'running' \
                AND s.lost_at IS NULL AND ",
            crate::session_org_sql!("s"),
            " = ?1"
        ))?;
        let rows = st.query_map(rusqlite::params![org], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

/// One live session as an unsaved org rule would see it
/// ([`Store::preview_org_rule`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RulePreviewRow {
    pub session_id: i64,
    /// The rule's own condition holds for it, whatever other rules say.
    pub matched: bool,
    /// Its org now.
    pub before: Option<i64>,
    /// Its org once the rule is added (a more specific rule still wins).
    pub after: Option<i64>,
}

/// Carries the answer out of a savepoint that is always rolled back.
enum Rolled {
    Done(Vec<RulePreviewRow>),
    Sql(rusqlite::Error),
}

impl From<rusqlite::Error> for Rolled {
    fn from(e: rusqlite::Error) -> Self {
        Rolled::Sql(e)
    }
}

impl Store {
    /// What adding `rule` (already normalised) would do to the live
    /// sessions (`status = 'running' AND lost_at IS NULL`), answered by the
    /// same SQL that places a session (`session_org_sql!`), so rule
    /// precedence is the real one. Nothing is written: the rule is added
    /// inside a savepoint that is always rolled back. `matched` is read with
    /// the rule alone (every other rule and host route set aside in the same
    /// savepoint).
    pub fn preview_org_rule(
        &self,
        rule: &super::OrgRuleRow,
    ) -> Result<Vec<RulePreviewRow>, IpcError> {
        const LIVE_ORGS: &str = concat!(
            "SELECT s.id, ",
            crate::session_org_sql!("s"),
            " FROM sessions s WHERE s.status = 'running' AND s.lost_at IS NULL ORDER BY s.id"
        );
        let read = |c: &rusqlite::Connection| -> rusqlite::Result<Vec<(i64, Option<i64>)>> {
            let mut st = c.prepare(LIVE_ORGS)?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        };
        let insert = |c: &rusqlite::Connection| {
            c.execute(
                "INSERT INTO org_rules (org_id, owner, repo, path_prefix, host_alias) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    rule.org_id,
                    rule.owner,
                    rule.repo,
                    rule.path_prefix,
                    rule.host_alias
                ],
            )
        };
        let out = self.in_savepoint("preview_org_rule", |c| -> Result<(), Rolled> {
            let before = read(c)?;
            insert(c)?;
            let after = read(c)?;
            c.execute_batch("DELETE FROM org_rules; UPDATE hosts SET org_id = NULL;")?;
            insert(c)?;
            let alone = read(c)?;
            let rows = before
                .iter()
                .zip(after.iter())
                .zip(alone.iter())
                .map(|((b, a), m)| RulePreviewRow {
                    session_id: b.0,
                    matched: m.1 == Some(rule.org_id),
                    before: b.1,
                    after: a.1,
                })
                .collect();
            Err(Rolled::Done(rows))
        });
        match out {
            Err(Rolled::Done(rows)) => Ok(rows),
            Err(Rolled::Sql(e)) => Err(e.into()),
            Ok(()) => unreachable!("the preview always rolls back"),
        }
    }
}
