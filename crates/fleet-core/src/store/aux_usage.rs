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
/// [`NewAuxUsage::origin`] of a commit message drafted from a session's
/// staged diff (Orbit Fleet 5.12).
pub const AUX_ORIGIN_COMMIT_MESSAGE: &str = "commit_message";
/// [`NewAuxUsage::origin`] of a finished mission's release note (redesign
/// 9.11).
pub const AUX_ORIGIN_RELEASE_NOTE: &str = "release_note";
/// [`NewAuxUsage::origin`] of Today's morning brief (redesign 9.11).
pub const AUX_ORIGIN_MORNING_BRIEF: &str = "morning_brief";
/// [`NewAuxUsage::origin`] of a brief drafted from a ticket (redesign 6.10).
pub const AUX_ORIGIN_BRIEF: &str = "brief";
/// [`NewAuxUsage::origin`] of a watcher's "Since 13:20" summary of a
/// session (Orbit Fleet 11.11).
pub const AUX_ORIGIN_WATCH_SUMMARY: &str = "watch_summary";
/// [`NewAuxUsage::origin`] of a stuck mission's triage card (redesign
/// 9.10).
pub const AUX_ORIGIN_TRIAGE: &str = "triage";
/// [`NewAuxUsage::origin`] of a person's context help at a shell or
/// composer prompt line (`service::context_help`).
pub const AUX_ORIGIN_CONTEXT_HELP: &str = "context_help";

/// Every origin above: each is also a run kind (`RUN_KINDS`).
pub const AUX_ORIGINS: &[&str] = &[
    AUX_ORIGIN_PLANNER,
    AUX_ORIGIN_SUMMARY,
    AUX_ORIGIN_COMMIT_MESSAGE,
    AUX_ORIGIN_RELEASE_NOTE,
    AUX_ORIGIN_MORNING_BRIEF,
    AUX_ORIGIN_BRIEF,
    AUX_ORIGIN_WATCH_SUMMARY,
    AUX_ORIGIN_TRIAGE,
    AUX_ORIGIN_CONTEXT_HELP,
];

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
        self.aux_usage_where("mission_id = ?1", mission_id)
    }

    /// The runs booked with `origin`, newest first.
    pub fn aux_usage_of_origin(&self, origin: &str) -> Result<Vec<AuxUsageRow>, IpcError> {
        self.aux_usage_where("origin = ?1", origin)
    }

    fn aux_usage_where(
        &self,
        filter: &str,
        arg: impl rusqlite::ToSql,
    ) -> Result<Vec<AuxUsageRow>, IpcError> {
        let mut st = self.conn.prepare(&format!(
            "SELECT id, origin, host_alias, model, mission_id, org_id, claude_session_id, \
             input_tokens, output_tokens, cost_micros, at FROM aux_usage \
             WHERE {filter} ORDER BY id DESC"
        ))?;
        let rows = st.query_map([arg], |r| {
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
