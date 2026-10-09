//! `aux_usage` (migration 128, redesign 8.2): what fleet's own `claude -p`
//! runs cost, each with its origin. See `service::work::orchestrate` (the
//! planner) and `service::work::summary`.

use super::Store;
use crate::ipc_error::IpcError;
use rusqlite::params;

/// [`NewAuxUsage::origin`] of a mission's planner run.
pub const AUX_ORIGIN_PLANNER: &str = "planner";
/// [`NewAuxUsage::origin`] of a past conversation's summary.
pub const AUX_ORIGIN_SUMMARY: &str = "summary";
/// [`NewAuxUsage::origin`] of a finished mission's release note (redesign
/// 9.11).
pub const AUX_ORIGIN_RELEASE_NOTE: &str = "release_note";
/// [`NewAuxUsage::origin`] of Today's morning brief (redesign 9.11).
pub const AUX_ORIGIN_BRIEF: &str = "brief";

/// One run to book.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewAuxUsage {
    pub origin: &'static str,
    pub host_alias: String,
    pub model: String,
    pub mission_id: Option<i64>,
    pub org_id: Option<i64>,
    pub claude_session_id: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cost_micros: i64,
    pub at: i64,
}

/// A booked run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuxUsageRow {
    pub id: i64,
    pub origin: String,
    pub host_alias: String,
    pub model: String,
    pub mission_id: Option<i64>,
    pub org_id: Option<i64>,
    pub claude_session_id: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cost_micros: i64,
    pub at: i64,
}

impl Store {
    pub fn insert_aux_usage(&self, u: &NewAuxUsage) -> Result<i64, IpcError> {
        self.conn.execute(
            "INSERT INTO aux_usage (origin, host_alias, model, mission_id, org_id, \
             claude_session_id, input_tokens, output_tokens, cost_micros, at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                u.origin,
                u.host_alias,
                u.model,
                u.mission_id,
                u.org_id,
                u.claude_session_id,
                u.input_tokens,
                u.output_tokens,
                u.cost_micros.max(0),
                u.at,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// A mission's booked runs, newest first.
    pub fn mission_aux_usage(&self, mission_id: i64) -> Result<Vec<AuxUsageRow>, IpcError> {
        let mut st = self.conn.prepare(
            "SELECT id, origin, host_alias, model, mission_id, org_id, claude_session_id, \
             input_tokens, output_tokens, cost_micros, at FROM aux_usage \
             WHERE mission_id = ?1 ORDER BY id DESC",
        )?;
        let rows = st.query_map([mission_id], |r| {
            Ok(AuxUsageRow {
                id: r.get(0)?,
                origin: r.get(1)?,
                host_alias: r.get(2)?,
                model: r.get(3)?,
                mission_id: r.get(4)?,
                org_id: r.get(5)?,
                claude_session_id: r.get(6)?,
                input_tokens: r.get(7)?,
                output_tokens: r.get(8)?,
                cost_micros: r.get(9)?,
                at: r.get(10)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}
