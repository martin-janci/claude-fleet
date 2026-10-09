//! PR shepherd rows (migration 136): the standing rule a person grants per
//! project and the episodes the shepherd recorded. The planner and the
//! runner are `service::pr_shepherd`; the design is
//! `docs/superpowers/specs/2026-10-08-pr-shepherd-design.md`.

use super::Store;
use rusqlite::{params, OptionalExtension};

/// What a rule lets the shepherd do, least first.
pub const SHEPHERD_LEVELS: [&str; 3] = ["watch", "nudge", "merge"];

/// Longest `recipes` text a rule carries (the migration's CHECK).
pub const SHEPHERD_RECIPES_MAX_CHARS: usize = 2000;

/// A person's standing rule for one project.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShepherdRuleRow {
    pub project_id: i64,
    pub level: String,
    pub granted_by: String,
    pub granted_at: i64,
    pub expires_at: Option<i64>,
    pub recipes: Option<String>,
}

impl ShepherdRuleRow {
    /// In force at `now`: not expired.
    pub fn active_at(&self, now: i64) -> bool {
        self.expires_at.is_none_or(|e| e > now)
    }
}

/// One merge the queue tried (migration 142).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShepherdMergeRow {
    pub session_id: i64,
    pub head_oid: String,
    pub project_id: i64,
    pub pr_url: String,
    pub at: i64,
    pub outcome: String,
}

/// One recorded episode: a problem on one pushed commit of a session's PR.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShepherdEpisodeRow {
    pub session_id: i64,
    pub head_oid: String,
    pub condition: String,
    pub pr_url: Option<String>,
    pub at: i64,
    pub outcome: String,
}

impl Store {
    /// Write (or replace) a project's rule. Refuses an unknown level and an
    /// over-long recipe text before SQLite's CHECK would.
    pub fn grant_shepherd_rule(&self, rule: &ShepherdRuleRow) -> Result<(), rusqlite::Error> {
        if !SHEPHERD_LEVELS.contains(&rule.level.as_str()) {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "level must be one of {}",
                SHEPHERD_LEVELS.join(", ")
            )));
        }
        if rule
            .recipes
            .as_deref()
            .is_some_and(|r| r.chars().count() > SHEPHERD_RECIPES_MAX_CHARS)
        {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "recipes must be at most {SHEPHERD_RECIPES_MAX_CHARS} characters"
            )));
        }
        self.conn.execute(
            "INSERT INTO pr_shepherd_rules \
               (project_id, level, granted_by, granted_at, expires_at, recipes) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(project_id) DO UPDATE SET level = excluded.level, \
               granted_by = excluded.granted_by, granted_at = excluded.granted_at, \
               expires_at = excluded.expires_at, recipes = excluded.recipes",
            params![
                rule.project_id,
                rule.level,
                rule.granted_by,
                rule.granted_at,
                rule.expires_at,
                rule.recipes
            ],
        )?;
        Ok(())
    }

    /// Remove a project's rule. `true` when there was one.
    pub fn revoke_shepherd_rule(&self, project_id: i64) -> Result<bool, rusqlite::Error> {
        let n = self.conn.execute(
            "DELETE FROM pr_shepherd_rules WHERE project_id = ?1",
            params![project_id],
        )?;
        Ok(n > 0)
    }

    /// Remove every rule (the shepherd's Pause all). Returns how many went.
    pub fn revoke_all_shepherd_rules(&self) -> Result<usize, rusqlite::Error> {
        self.conn.execute("DELETE FROM pr_shepherd_rules", [])
    }

    /// Every rule, expired ones included, by project id.
    pub fn list_shepherd_rules(&self) -> Result<Vec<ShepherdRuleRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT project_id, level, granted_by, granted_at, expires_at, recipes \
             FROM pr_shepherd_rules ORDER BY project_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ShepherdRuleRow {
                project_id: r.get(0)?,
                level: r.get(1)?,
                granted_by: r.get(2)?,
                granted_at: r.get(3)?,
                expires_at: r.get(4)?,
                recipes: r.get(5)?,
            })
        })?;
        rows.collect()
    }

    /// The recorded outcome of one episode, if the shepherd already saw it.
    pub fn shepherd_episode_outcome(
        &self,
        session_id: i64,
        head_oid: &str,
        condition: &str,
    ) -> Result<Option<String>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT outcome FROM pr_shepherd_episodes \
                 WHERE session_id = ?1 AND head_oid = ?2 AND condition = ?3",
                params![session_id, head_oid, condition],
                |r| r.get(0),
            )
            .optional()
    }

    /// How many nudges the shepherd sent session `session_id` since `since`
    /// (a failed send counts: it was an attempt).
    pub fn count_shepherd_nudges_since(
        &self,
        session_id: i64,
        since: i64,
    ) -> Result<u32, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM pr_shepherd_episodes \
                 WHERE session_id = ?1 AND at >= ?2 \
                   AND (outcome = 'nudged' OR outcome LIKE 'failed:%')",
                params![session_id, since],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n as u32)
    }

    /// Record an episode once: a second write for the same key is ignored,
    /// so the first outcome stands. Also writes the `pr_shepherd` timeline
    /// event (`<condition>:<outcome>`). `true` when the row was new.
    pub fn record_shepherd_episode(
        &self,
        ep: &ShepherdEpisodeRow,
    ) -> Result<bool, rusqlite::Error> {
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO pr_shepherd_episodes \
               (session_id, head_oid, condition, pr_url, at, outcome) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                ep.session_id,
                ep.head_oid,
                ep.condition,
                ep.pr_url,
                ep.at,
                ep.outcome
            ],
        )?;
        if n > 0 {
            let detail = format!("{}:{}", ep.condition, ep.outcome);
            if let Err(e) = self.insert_session_event(ep.session_id, "pr_shepherd", Some(&detail)) {
                tracing::warn!(
                    session_id = ep.session_id,
                    error = %e,
                    "[pr_shepherd] session_event insert failed"
                );
            }
        }
        Ok(n > 0)
    }

    /// The newest episodes, newest first, for `fleet-hub shepherd status`.
    pub fn list_shepherd_episodes(
        &self,
        limit: usize,
    ) -> Result<Vec<ShepherdEpisodeRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT session_id, head_oid, condition, pr_url, at, outcome \
             FROM pr_shepherd_episodes ORDER BY at DESC, session_id LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok(ShepherdEpisodeRow {
                session_id: r.get(0)?,
                head_oid: r.get(1)?,
                condition: r.get(2)?,
                pr_url: r.get(3)?,
                at: r.get(4)?,
                outcome: r.get(5)?,
            })
        })?;
        rows.collect()
    }

    /// Whether the merge queue already tried this head of this session's PR.
    /// A merge or a failure is final for the head; a refusal (`skipped:…`,
    /// GitHub's live answer said not yet) holds only until `retry_after`,
    /// so a stale reading is looked at again once the probe refreshed it.
    pub fn shepherd_merge_tried(
        &self,
        session_id: i64,
        head_oid: &str,
        retry_after: i64,
    ) -> Result<bool, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT 1 FROM pr_shepherd_merges WHERE session_id = ?1 AND head_oid = ?2 \
                 AND (outcome NOT LIKE 'skipped:%' OR at > ?3)",
                params![session_id, head_oid, retry_after],
                |_| Ok(()),
            )
            .optional()
            .map(|r| r.is_some())
    }

    /// When the queue last merged (or tried to merge) a PR of `project_id`.
    /// A skip is not an attempt: it touched nothing on GitHub.
    pub fn last_shepherd_merge_at(&self, project_id: i64) -> Result<Option<i64>, rusqlite::Error> {
        self.conn.query_row(
            "SELECT MAX(at) FROM pr_shepherd_merges \
             WHERE project_id = ?1 AND outcome NOT LIKE 'skipped:%'",
            params![project_id],
            |r| r.get(0),
        )
    }

    /// Record one merge attempt, with its `pr_shepherd` timeline event
    /// (`merge:<outcome>`). A head's merge or failure is written once; a
    /// refusal may be overwritten by the next attempt. `true` when written.
    pub fn record_shepherd_merge(&self, m: &ShepherdMergeRow) -> Result<bool, rusqlite::Error> {
        let n = self.conn.execute(
            "INSERT INTO pr_shepherd_merges \
               (session_id, head_oid, project_id, pr_url, at, outcome) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(session_id, head_oid) DO UPDATE SET \
               at = excluded.at, outcome = excluded.outcome \
             WHERE pr_shepherd_merges.outcome LIKE 'skipped:%'",
            params![
                m.session_id,
                m.head_oid,
                m.project_id,
                m.pr_url,
                m.at,
                m.outcome
            ],
        )?;
        if n > 0 {
            let detail = format!("merge:{}", m.outcome);
            if let Err(e) = self.insert_session_event(m.session_id, "pr_shepherd", Some(&detail)) {
                tracing::warn!(
                    session_id = m.session_id,
                    error = %e,
                    "[pr_shepherd] session_event insert failed"
                );
            }
        }
        Ok(n > 0)
    }

    /// The newest merge attempts, newest first.
    pub fn list_shepherd_merges(
        &self,
        limit: usize,
    ) -> Result<Vec<ShepherdMergeRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT session_id, head_oid, project_id, pr_url, at, outcome \
             FROM pr_shepherd_merges ORDER BY at DESC, session_id LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok(ShepherdMergeRow {
                session_id: r.get(0)?,
                head_oid: r.get(1)?,
                project_id: r.get(2)?,
                pr_url: r.get(3)?,
                at: r.get(4)?,
                outcome: r.get(5)?,
            })
        })?;
        rows.collect()
    }
}
