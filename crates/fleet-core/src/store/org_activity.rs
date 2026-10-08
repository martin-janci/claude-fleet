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
