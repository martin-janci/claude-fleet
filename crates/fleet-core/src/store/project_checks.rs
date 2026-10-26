//! A project's AI check switch (migration 213): `projects.check_mode` and
//! `projects.check_command`, read apart from [`super::ProjectRow`]. What the
//! check does with them is `service::work::ticket_check`.

use super::Store;
use crate::ipc_error::{codes, IpcError};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// No check: the default, and what a NULL `check_mode` reads as.
pub const CHECK_OFF: &str = "off";
/// Test now is offered on a finished ticket; nothing starts by itself.
pub const CHECK_ASK: &str = "ask";
/// The check starts when a ticket's work is finished.
pub const CHECK_AUTO: &str = "auto";
/// Every mode, in the order the switches offer them.
pub const CHECK_MODES: [&str; 3] = [CHECK_OFF, CHECK_ASK, CHECK_AUTO];

/// One project's switch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectCheck {
    /// One of [`CHECK_MODES`].
    pub mode: String,
    /// The test command, when one is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

impl Store {
    /// Project `id`'s switch; `None` when there is no such project.
    pub fn project_check(&self, id: i64) -> Result<Option<ProjectCheck>, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT check_mode, check_command FROM projects WHERE id = ?1",
                rusqlite::params![id],
                |r| {
                    let mode: Option<String> = r.get(0)?;
                    Ok(ProjectCheck {
                        mode: mode
                            .filter(|m| CHECK_MODES.contains(&m.as_str()))
                            .unwrap_or_else(|| CHECK_OFF.to_string()),
                        command: r.get(1)?,
                    })
                },
            )
            .optional()?)
    }

    /// Set project `id`'s switch. `false` when there is no such project.
    /// The caller has checked the command; an unknown mode is refused here.
    pub fn set_project_check(
        &self,
        id: i64,
        mode: &str,
        command: Option<&str>,
    ) -> Result<bool, IpcError> {
        if !CHECK_MODES.contains(&mode) {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!("check mode {mode:?}; one of {}", CHECK_MODES.join(", ")),
            ));
        }
        let mode = (mode != CHECK_OFF).then_some(mode);
        let n = self.conn.execute(
            "UPDATE projects SET check_mode = ?2, check_command = ?3 WHERE id = ?1",
            rusqlite::params![id, mode, command],
        )?;
        Ok(n > 0)
    }

    /// Whether any project has its check on `auto`: the sweep's first
    /// read, so a fleet with none does nothing more.
    pub fn any_auto_check(&self) -> Result<bool, IpcError> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM projects WHERE check_mode = ?1 LIMIT 1",
                rusqlite::params![CHECK_AUTO],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// The finished work a check may be owed for: per work item, its newest
    /// `implement` or `wrapup` task, when that one is done, was created
    /// after `since`, carries evidence, and no `test` task of the item came
    /// after it. Newest first, at most `limit` task ids.
    pub fn ticket_check_candidates(&self, since: i64, limit: i64) -> Result<Vec<i64>, IpcError> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id FROM tasks t \
              WHERE t.created_at >= ?1 AND t.state = 'done' \
                AND t.work_item_id IS NOT NULL AND t.evidence_json IS NOT NULL \
                AND COALESCE(t.role, 'implement') IN ('implement', 'wrapup') \
                AND NOT EXISTS (SELECT 1 FROM tasks n \
                                 WHERE n.work_item_id = t.work_item_id AND n.id > t.id \
                                   AND COALESCE(n.role, 'implement') IN ('implement', 'wrapup', 'test')) \
              ORDER BY t.id DESC LIMIT ?2",
        )?;
        let ids = stmt
            .query_map(rusqlite::params![since, limit], |r| r.get(0))?
            .collect::<Result<Vec<i64>, _>>()?;
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(s: &Store) -> i64 {
        s.upsert_project("acme", "api", "/srv/acme/api").unwrap()
    }

    #[test]
    fn a_project_tests_nothing_until_a_person_turns_it_on() {
        let s = Store::open_in_memory().unwrap();
        let p = project(&s);
        assert_eq!(s.project_check(p).unwrap().unwrap().mode, CHECK_OFF);
        assert!(!s.any_auto_check().unwrap());
        assert!(s
            .set_project_check(p, CHECK_AUTO, Some("cargo test"))
            .unwrap());
        assert!(s.any_auto_check().unwrap());
        assert_eq!(
            s.project_check(p).unwrap().unwrap(),
            ProjectCheck {
                mode: CHECK_AUTO.into(),
                command: Some("cargo test".into())
            }
        );
        assert!(s.set_project_check(p, CHECK_OFF, None).unwrap());
        assert_eq!(s.project_check(p).unwrap().unwrap().mode, CHECK_OFF);
        assert!(s.project_check(p + 99).unwrap().is_none());
        assert!(!s.set_project_check(p + 99, CHECK_ASK, None).unwrap());
        assert_eq!(
            s.set_project_check(p, "sometimes", None).unwrap_err().code,
            codes::E_INVALID
        );
    }
}
