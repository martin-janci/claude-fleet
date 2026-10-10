//! Start rules (migration 144, redesign step 8.11). Who may see and change
//! a rule, the pattern grammar, the tally that offers one and where a rule
//! decides a start are in `service::start_rules`; this is the rows.

use super::Store;
use crate::ipc_error::IpcError;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// How a rule's starts run beyond the project and host (migration 161): the
/// fallback host, the account, the model and effort, the agent. `None`
/// everywhere = the host's defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartRuleLaunch {
    pub fallback_host: Option<String>,
    pub profile: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub agent: Option<String>,
}

/// `start_rules.state` values. Only `active` decides a start.
pub const START_RULE_STATES: [&str; 4] = ["counting", "offered", "active", "dismissed"];

/// One start rule (or fleet's tally toward one), as a reader receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartRuleRow {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_person_id: Option<i64>,
    /// A key pattern, `*` for any run of characters (`PD-*`).
    pub pattern: String,
    pub project_id: i64,
    /// `None` = the project's last host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_alias: Option<String>,
    /// Where a start lands when `host_alias` is unreachable (migration 161).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_host: Option<String>,
    /// The credential profile the session bills (the Account); `None` = the
    /// host's own login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// `claude --model`; `None` = the host's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Reasoning effort; `None` = the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// `claude` | `codex`; `None` = Claude Code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// counting | offered | active | dismissed
    pub state: String,
    /// Identical person starts in a row.
    #[serde(default)]
    pub confirmations: i64,
    /// Starts the rule decided.
    #[serde(default)]
    pub hits: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_hit_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

const COLS: &str = "id, org_id, owner_person_id, pattern, project_id, host_alias, state, \
                    confirmations, hits, last_hit_at, created_at, updated_at, \
                    fallback_host, profile, model, effort, agent";

fn rule(r: &rusqlite::Row<'_>) -> rusqlite::Result<StartRuleRow> {
    Ok(StartRuleRow {
        id: r.get(0)?,
        org_id: r.get(1)?,
        owner_person_id: r.get(2)?,
        pattern: r.get(3)?,
        project_id: r.get(4)?,
        host_alias: r.get(5)?,
        state: r.get(6)?,
        confirmations: r.get(7)?,
        hits: r.get(8)?,
        last_hit_at: r.get(9)?,
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
        fallback_host: r.get(12)?,
        profile: r.get(13)?,
        model: r.get(14)?,
        effort: r.get(15)?,
        agent: r.get(16)?,
    })
}

impl Store {
    /// Every rule and tally row, oldest first.
    pub fn list_start_rules(&self) -> Result<Vec<StartRuleRow>, IpcError> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {COLS} FROM start_rules ORDER BY id"))?;
        let rows = stmt.query_map([], rule)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The rows of `org` (`None` = tasks of no org) in `state`, oldest
    /// first.
    pub fn start_rules_in(
        &self,
        org: Option<i64>,
        state: &str,
    ) -> Result<Vec<StartRuleRow>, IpcError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLS} FROM start_rules \
             WHERE COALESCE(org_id, 0) = COALESCE(?1, 0) AND state = ?2 ORDER BY id"
        ))?;
        let rows = stmt.query_map(rusqlite::params![org, state], rule)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn get_start_rule(&self, id: i64) -> Result<Option<StartRuleRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLS} FROM start_rules WHERE id = ?1"),
                [id],
                rule,
            )
            .optional()?)
    }

    /// The row for `org`, `pattern` (any case) and `project`, whatever its
    /// state: there is at most one.
    pub fn find_start_rule(
        &self,
        org: Option<i64>,
        pattern: &str,
        project_id: i64,
    ) -> Result<Option<StartRuleRow>, IpcError> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {COLS} FROM start_rules \
                     WHERE COALESCE(org_id, 0) = COALESCE(?1, 0) \
                       AND UPPER(pattern) = UPPER(?2) AND project_id = ?3"
                ),
                rusqlite::params![org, pattern, project_id],
                rule,
            )
            .optional()?)
    }

    /// Insert a row in `state`, with `confirmations` already counted.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_start_rule(
        &self,
        org: Option<i64>,
        owner: Option<i64>,
        pattern: &str,
        project_id: i64,
        host_alias: Option<&str>,
        state: &str,
        confirmations: i64,
        now: i64,
    ) -> Result<StartRuleRow, IpcError> {
        self.conn.execute(
            "INSERT INTO start_rules (org_id, owner_person_id, pattern, project_id, host_alias, \
               state, confirmations, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            rusqlite::params![
                org,
                owner,
                pattern,
                project_id,
                host_alias,
                state,
                confirmations,
                now
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.get_start_rule(id)?.ok_or_else(|| {
            IpcError::new(
                crate::ipc_error::codes::E_INTERNAL,
                "start rule vanished after insert",
            )
        })
    }

    /// Rewrite a rule's pattern, project, host and how its starts run.
    pub fn update_start_rule(
        &self,
        id: i64,
        pattern: &str,
        project_id: i64,
        host_alias: Option<&str>,
        launch: &StartRuleLaunch,
        now: i64,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE start_rules SET pattern = ?2, project_id = ?3, host_alias = ?4, \
               updated_at = ?5, fallback_host = ?6, profile = ?7, model = ?8, effort = ?9, \
               agent = ?10 WHERE id = ?1",
            rusqlite::params![
                id,
                pattern,
                project_id,
                host_alias,
                now,
                launch.fallback_host,
                launch.profile,
                launch.model,
                launch.effort,
                launch.agent
            ],
        )?;
        Ok(())
    }

    /// Move a row to `state`; `owner` is written when `Some`.
    pub fn set_start_rule_state(
        &self,
        id: i64,
        state: &str,
        owner: Option<i64>,
        now: i64,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE start_rules SET state = ?2, \
               owner_person_id = COALESCE(?3, owner_person_id), updated_at = ?4 WHERE id = ?1",
            rusqlite::params![id, state, owner, now],
        )?;
        Ok(())
    }

    /// Set a row's tally (and its state, which the tally decides).
    pub fn set_start_rule_tally(
        &self,
        id: i64,
        confirmations: i64,
        state: &str,
        now: i64,
    ) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE start_rules SET confirmations = ?2, state = ?3, updated_at = ?4 WHERE id = ?1",
            rusqlite::params![id, confirmations, state, now],
        )?;
        Ok(())
    }

    /// One more start the rule decided.
    pub fn note_start_rule_hit(&self, id: i64, now: i64) -> Result<(), IpcError> {
        self.conn.execute(
            "UPDATE start_rules SET hits = hits + 1, last_hit_at = ?2 WHERE id = ?1",
            rusqlite::params![id, now],
        )?;
        Ok(())
    }

    pub fn delete_start_rule(&self, id: i64) -> Result<bool, IpcError> {
        Ok(self
            .conn
            .execute("DELETE FROM start_rules WHERE id = ?1", [id])?
            > 0)
    }
}
