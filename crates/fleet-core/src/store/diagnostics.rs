//! Reads for `fleet-diagnostics` (`service::fleet_agents::diagnostics`):
//! the self debug agent's view of what happened to sessions in a window.

use super::Store;
use rusqlite::Result;
use serde::Serialize;

/// One timeline event, with the session it happened to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WindowEventRow {
    pub at: i64,
    pub session_id: i64,
    pub tmux_name: String,
    pub host_alias: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Store {
    /// Timeline events of `kinds` in `[since, until)`, every session, newest
    /// first, at most `limit`.
    pub fn session_events_between(
        &self,
        since: i64,
        until: i64,
        kinds: &[&str],
        limit: i64,
    ) -> Result<Vec<WindowEventRow>> {
        if kinds.is_empty() {
            return Ok(Vec::new());
        }
        let marks = (4..4 + kinds.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT e.at, e.session_id, s.tmux_name, s.host_alias, e.kind, e.detail \
             FROM session_events e JOIN sessions s ON s.id = e.session_id \
             WHERE e.at >= ?1 AND e.at < ?2 AND e.kind IN ({marks}) \
             ORDER BY e.at DESC, e.id DESC LIMIT ?3"
        );
        let mut params: Vec<rusqlite::types::Value> =
            vec![since.into(), until.into(), limit.into()];
        params.extend(
            kinds
                .iter()
                .map(|k| rusqlite::types::Value::from(k.to_string())),
        );
        let mut st = self.conn.prepare(&sql)?;
        let rows = st
            .query_map(rusqlite::params_from_iter(params), |r| {
                Ok(WindowEventRow {
                    at: r.get(0)?,
                    session_id: r.get(1)?,
                    tmux_name: r.get(2)?,
                    host_alias: r.get(3)?,
                    kind: r.get(4)?,
                    detail: r.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>>>()?;
        Ok(rows)
    }
}
