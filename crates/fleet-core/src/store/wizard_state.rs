//! Wizards left half-way (migration 164, gap plan G7.2): one row per kind,
//! key and person, so a wizard resumes on another device. Who may read and
//! write a row and what a row may hold are in `service::wizard_state`; this
//! is the rows. The add-host wizard's drafts (`store::host_setup`) are the
//! `add_host` rows.

use super::{now_unix, Store};
use rusqlite::{OptionalExtension, Result};
use serde::{Deserialize, Serialize};

/// One wizard left half-way.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WizardStateRow {
    /// add_host | add_project | add_account | new_session | link_peer | form
    pub kind: String,
    /// Which one of the kind: the SSH alias of an add-host wizard, `""` for a
    /// wizard a person runs one of at a time.
    pub key: String,
    /// Whose it is; `None` = fleet's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_id: Option<i64>,
    /// What the resume line names ("mercury", "acme/api").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The step it was left on, from 1.
    pub step: i64,
    /// What the person picked: the wizard's own shape. Never a secret.
    pub answers: serde_json::Value,
    /// The add-host wizard's last live checks; `[]` for the others.
    #[serde(default)]
    pub checks: serde_json::Value,
    /// The device that saved it last; `None` = the hub's own desktop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// What one save writes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WizardStateWrite<'a> {
    pub kind: &'a str,
    pub key: &'a str,
    pub person_id: Option<i64>,
    pub label: Option<&'a str>,
    pub step: i64,
    pub answers: Option<&'a serde_json::Value>,
    /// `None` keeps the checks a row already has.
    pub checks: Option<&'a serde_json::Value>,
    pub device: Option<&'a str>,
}

const COLS: &str =
    "kind, key, person_id, label, step, answers, checks, device, created_at, updated_at";

fn json_or(raw: String, empty: serde_json::Value) -> serde_json::Value {
    serde_json::from_str(&raw).unwrap_or(empty)
}

fn row(r: &rusqlite::Row<'_>) -> Result<WizardStateRow> {
    Ok(WizardStateRow {
        kind: r.get(0)?,
        key: r.get(1)?,
        person_id: r.get(2)?,
        label: r.get(3)?,
        step: r.get(4)?,
        // A row this build cannot read loses its answers, not the draft.
        answers: json_or(r.get(5)?, serde_json::Value::Object(Default::default())),
        checks: json_or(r.get(6)?, serde_json::Value::Array(Vec::new())),
        device: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

impl Store {
    /// Insert a wizard, or replace what an earlier save of the same kind,
    /// key and person held (its `created_at` stays, and its checks when the
    /// write names none).
    pub fn save_wizard_state(&self, w: &WizardStateWrite<'_>) -> Result<WizardStateRow> {
        let at = now_unix();
        let answers = w
            .answers
            .map(|a| a.to_string())
            .unwrap_or_else(|| "{}".into());
        let checks = w.checks.map(|c| c.to_string());
        self.conn.execute(
            "INSERT INTO wizard_state
               (kind, key, person_id, label, step, answers, checks, device, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, COALESCE(?7, '[]'), ?8, ?9, ?9)
             ON CONFLICT (kind, key, COALESCE(person_id, 0)) DO UPDATE SET
               label = excluded.label, step = excluded.step, answers = excluded.answers,
               checks = COALESCE(?7, wizard_state.checks), device = excluded.device,
               updated_at = excluded.updated_at",
            rusqlite::params![
                w.kind,
                w.key,
                w.person_id,
                w.label,
                w.step,
                answers,
                checks,
                w.device,
                at
            ],
        )?;
        Ok(self
            .wizard_state(w.kind, w.key, w.person_id)?
            .expect("the row just written"))
    }

    /// Replace only a wizard's checks; `false` when there is none.
    pub fn set_wizard_checks(
        &self,
        kind: &str,
        key: &str,
        person_id: Option<i64>,
        checks: &serde_json::Value,
    ) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE wizard_state SET checks = ?4, updated_at = ?5
             WHERE kind = ?1 AND key = ?2 AND COALESCE(person_id, 0) = COALESCE(?3, 0)",
            rusqlite::params![kind, key, person_id, checks.to_string(), now_unix()],
        )?;
        Ok(n > 0)
    }

    pub fn wizard_state(
        &self,
        kind: &str,
        key: &str,
        person_id: Option<i64>,
    ) -> Result<Option<WizardStateRow>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {COLS} FROM wizard_state
                     WHERE kind = ?1 AND key = ?2 AND COALESCE(person_id, 0) = COALESCE(?3, 0)"
                ),
                rusqlite::params![kind, key, person_id],
                row,
            )
            .optional()
    }

    /// Every wizard, of `kind` when given, the most recently touched first.
    pub fn wizard_states(&self, kind: Option<&str>) -> Result<Vec<WizardStateRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {COLS} FROM wizard_state WHERE ?1 IS NULL OR kind = ?1
             ORDER BY updated_at DESC, kind, key"
        ))?;
        let rows = st.query_map([kind], row)?;
        rows.collect()
    }

    /// `true` when a wizard was there.
    pub fn delete_wizard_state(
        &self,
        kind: &str,
        key: &str,
        person_id: Option<i64>,
    ) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM wizard_state
             WHERE kind = ?1 AND key = ?2 AND COALESCE(person_id, 0) = COALESCE(?3, 0)",
            rusqlite::params![kind, key, person_id],
        )? > 0)
    }

    /// Forget wizards untouched since `before` (unix secs); how many.
    pub fn purge_wizard_states(&self, before: i64) -> Result<usize> {
        self.conn
            .execute("DELETE FROM wizard_state WHERE updated_at < ?1", [before])
    }
}
