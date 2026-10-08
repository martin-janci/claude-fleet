//! The add-host wizard's saved drafts and the fleet-agent install jobs
//! (migration 124, Orbit Fleet 4.9). The rules (which checks run, how an
//! install proceeds) are in `service::host_setup` and
//! `service::agent_install`; this is the rows.

use super::{now_unix, Store};
use rusqlite::{OptionalExtension, Result};

/// One live check of the wizard's "Check the host" step.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SetupCheck {
    /// `ssh` | `tmux` | `git` | `agent` | `disk` | `agents`
    pub key: String,
    /// `ok` | `warn` | `fail` | `na`
    pub state: String,
    /// What the row says, e.g. `tmux 3.4` or `SSH as martin@mercury`.
    pub label: String,
    /// The short note at the row's end, e.g. `18 ms` or `ok`.
    #[serde(default)]
    pub detail: String,
}

/// A wizard someone left half-way.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HostSetupRow {
    pub ssh_alias: String,
    /// The name the host will have in fleet.
    pub alias: String,
    /// The wizard step it was left on, 1..=5.
    pub step: i64,
    pub checks: Vec<SetupCheck>,
    /// What the person picked on the later steps; the frontend's own shape.
    pub answers: serde_json::Value,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One fleet-agent install job.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentInstallRow {
    pub id: i64,
    pub host_alias: String,
    pub version: String,
    /// `running` | `done` | `failed`
    pub state: String,
    /// `target` | `download` | `start` | `connect` | `done`
    pub step: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
}

const SETUP_COLS: &str = "ssh_alias, alias, step, checks, answers, created_at, updated_at";
const INSTALL_COLS: &str = "id, host_alias, version, state, step, detail, started_at, finished_at";

fn setup_row(r: &rusqlite::Row<'_>) -> Result<HostSetupRow> {
    let checks: String = r.get(3)?;
    let answers: String = r.get(4)?;
    Ok(HostSetupRow {
        ssh_alias: r.get(0)?,
        alias: r.get(1)?,
        step: r.get(2)?,
        // A row this build cannot read loses its checks, not the draft: the
        // wizard runs them again.
        checks: serde_json::from_str(&checks).unwrap_or_default(),
        answers: serde_json::from_str(&answers)
            .unwrap_or_else(|_| serde_json::Value::Object(Default::default())),
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
    })
}

fn install_row(r: &rusqlite::Row<'_>) -> Result<AgentInstallRow> {
    Ok(AgentInstallRow {
        id: r.get(0)?,
        host_alias: r.get(1)?,
        version: r.get(2)?,
        state: r.get(3)?,
        step: r.get(4)?,
        detail: r.get(5)?,
        started_at: r.get(6)?,
        finished_at: r.get(7)?,
    })
}

impl Store {
    /// Save a draft: insert, or replace what an earlier save of the same SSH
    /// alias held (its `created_at` stays).
    pub fn save_host_setup(
        &self,
        ssh_alias: &str,
        alias: &str,
        step: i64,
        checks: &[SetupCheck],
        answers: &serde_json::Value,
    ) -> Result<HostSetupRow> {
        let at = now_unix();
        let checks = serde_json::to_string(checks).unwrap_or_else(|_| "[]".into());
        let answers = serde_json::to_string(answers).unwrap_or_else(|_| "{}".into());
        self.conn.execute(
            "INSERT INTO host_setups (ssh_alias, alias, step, checks, answers, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
             ON CONFLICT(ssh_alias) DO UPDATE SET
               alias = excluded.alias, step = excluded.step, checks = excluded.checks,
               answers = excluded.answers, updated_at = excluded.updated_at",
            rusqlite::params![ssh_alias, alias, step, checks, answers, at],
        )?;
        Ok(self.host_setup(ssh_alias)?.expect("the row just written"))
    }

    /// Replace only the checks of a draft; `false` when there is none.
    pub fn set_host_setup_checks(&self, ssh_alias: &str, checks: &[SetupCheck]) -> Result<bool> {
        let checks = serde_json::to_string(checks).unwrap_or_else(|_| "[]".into());
        let n = self.conn.execute(
            "UPDATE host_setups SET checks = ?2, updated_at = ?3 WHERE ssh_alias = ?1",
            rusqlite::params![ssh_alias, checks, now_unix()],
        )?;
        Ok(n > 0)
    }

    pub fn host_setup(&self, ssh_alias: &str) -> Result<Option<HostSetupRow>> {
        self.conn
            .query_row(
                &format!("SELECT {SETUP_COLS} FROM host_setups WHERE ssh_alias = ?1"),
                [ssh_alias],
                setup_row,
            )
            .optional()
    }

    /// Every draft, the most recently touched first.
    pub fn host_setups(&self) -> Result<Vec<HostSetupRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {SETUP_COLS} FROM host_setups ORDER BY updated_at DESC, ssh_alias"
        ))?;
        let rows = st.query_map([], setup_row)?;
        rows.collect()
    }

    /// `true` when a draft was there.
    pub fn delete_host_setup(&self, ssh_alias: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM host_setups WHERE ssh_alias = ?1", [ssh_alias])?
            > 0)
    }

    /// A new install job, `running` at step `target`.
    pub fn insert_agent_install(&self, host_alias: &str, version: &str) -> Result<AgentInstallRow> {
        self.conn.execute(
            "INSERT INTO agent_installs (host_alias, version, state, step, started_at)
             VALUES (?1, ?2, 'running', 'target', ?3)",
            rusqlite::params![host_alias, version, now_unix()],
        )?;
        let id = self.conn.last_insert_rowid();
        Ok(self.agent_install(id)?.expect("the row just inserted"))
    }

    /// Move a running job on to `step`.
    pub fn set_agent_install_step(&self, id: i64, step: &str, detail: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE agent_installs SET step = ?2, detail = ?3 WHERE id = ?1 AND state = 'running'",
            rusqlite::params![id, step, detail],
        )?;
        Ok(())
    }

    /// End a running job: `done` or `failed`, with the line a person reads.
    pub fn finish_agent_install(&self, id: i64, state: &str, detail: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE agent_installs SET state = ?2, detail = ?3, finished_at = ?4
             WHERE id = ?1 AND state = 'running'",
            rusqlite::params![id, state, detail, now_unix()],
        )?;
        Ok(())
    }

    pub fn agent_install(&self, id: i64) -> Result<Option<AgentInstallRow>> {
        self.conn
            .query_row(
                &format!("SELECT {INSTALL_COLS} FROM agent_installs WHERE id = ?1"),
                [id],
                install_row,
            )
            .optional()
    }

    /// The newest jobs first, at most `limit`, optionally for one host.
    pub fn agent_installs(
        &self,
        host_alias: Option<&str>,
        limit: i64,
    ) -> Result<Vec<AgentInstallRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {INSTALL_COLS} FROM agent_installs
             WHERE ?1 IS NULL OR host_alias = ?1 ORDER BY id DESC LIMIT ?2"
        ))?;
        let rows = st.query_map(rusqlite::params![host_alias, limit], install_row)?;
        rows.collect()
    }

    /// Fail every job still `running` that started before `before`: the
    /// process that ran it is gone (a hub restart), so nobody will finish
    /// it. Returns how many.
    pub fn fail_stale_agent_installs(&self, before: i64, detail: &str) -> Result<usize> {
        self.conn.execute(
            "UPDATE agent_installs SET state = 'failed', detail = ?2, finished_at = ?3
             WHERE state = 'running' AND started_at < ?1",
            rusqlite::params![before, detail, now_unix()],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(key: &str, state: &str) -> SetupCheck {
        SetupCheck {
            key: key.into(),
            state: state.into(),
            label: key.into(),
            detail: String::new(),
        }
    }

    #[test]
    fn a_draft_survives_reopening_the_database_file() {
        // The plan's Verify for 4.9: the wizard resumes after an app restart.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fleet.db");
        {
            let s = Store::open_with_bus(&path, std::sync::Arc::new(crate::events::NoopEventBus))
                .unwrap();
            s.save_host_setup(
                "mercury",
                "mercury",
                2,
                &[check("ssh", "ok"), check("tmux", "ok")],
                &serde_json::json!({"agents": ["claude"]}),
            )
            .unwrap();
        }
        let s =
            Store::open_with_bus(&path, std::sync::Arc::new(crate::events::NoopEventBus)).unwrap();
        let rows = s.host_setups().unwrap();
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!((r.ssh_alias.as_str(), r.step), ("mercury", 2));
        assert_eq!(r.checks.len(), 2);
        assert_eq!(r.answers["agents"][0], "claude");
    }

    #[test]
    fn saving_again_replaces_the_draft_and_keeps_its_start() {
        let s = Store::open_in_memory().unwrap();
        let first = s
            .save_host_setup("m", "mercury", 1, &[], &serde_json::json!({}))
            .unwrap();
        let second = s
            .save_host_setup(
                "m",
                "merc",
                3,
                &[check("ssh", "fail")],
                &serde_json::json!({}),
            )
            .unwrap();
        assert_eq!(second.created_at, first.created_at);
        assert_eq!((second.alias.as_str(), second.step), ("merc", 3));
        assert!(s.set_host_setup_checks("m", &[]).unwrap());
        assert!(s.host_setup("m").unwrap().unwrap().checks.is_empty());
        assert!(!s.set_host_setup_checks("nope", &[]).unwrap());
        assert!(s.delete_host_setup("m").unwrap());
        assert!(s.host_setups().unwrap().is_empty());
    }

    #[test]
    fn an_install_job_runs_once_to_its_end() {
        let s = Store::open_in_memory().unwrap();
        let j = s.insert_agent_install("mercury", "0.5.4").unwrap();
        assert_eq!((j.state.as_str(), j.step.as_str()), ("running", "target"));
        s.set_agent_install_step(j.id, "download", Some("x86_64"))
            .unwrap();
        s.finish_agent_install(j.id, "done", "connected").unwrap();
        // A finished job is not moved again.
        s.set_agent_install_step(j.id, "start", None).unwrap();
        s.finish_agent_install(j.id, "failed", "late").unwrap();
        let j = s.agent_install(j.id).unwrap().unwrap();
        assert_eq!(
            (j.state.as_str(), j.step.as_str(), j.detail.as_deref()),
            ("done", "download", Some("connected"))
        );
        assert!(j.finished_at.is_some());
        assert_eq!(s.agent_installs(Some("mercury"), 5).unwrap().len(), 1);
        assert!(s.agent_installs(Some("venus"), 5).unwrap().is_empty());
    }

    #[test]
    fn a_job_left_running_by_a_restart_is_failed() {
        let s = Store::open_in_memory().unwrap();
        let j = s.insert_agent_install("mercury", "0.5.4").unwrap();
        assert_eq!(s.fail_stale_agent_installs(j.started_at, "x").unwrap(), 0);
        assert_eq!(
            s.fail_stale_agent_installs(j.started_at + 1, "interrupted")
                .unwrap(),
            1
        );
        let j = s.agent_install(j.id).unwrap().unwrap();
        assert_eq!(j.state, "failed");
    }
}
